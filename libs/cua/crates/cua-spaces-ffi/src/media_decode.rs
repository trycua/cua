// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Decoded media callbacks: the SDK's media sessions
//! (`SpacesdClient.open_media`) with H.264 decoded to BGRA (VideoToolbox on
//! macOS, OpenH264 elsewhere) and audio decoded to PCM, through
//! `cua-media-client`. Ships with Cua Spaces (source-available,
//! FSL-1.1-MIT); the open source SDK delivers the encoded frames.

#![allow(clippy::module_name_repetitions)]

use std::sync::{Arc, Mutex};

use cua_sdk::{
    AudioPacket, AudioSink, FrameSink, MediaEvent, MediaOpenOptions, MediaSession, Result,
    SpacesdClient, VideoFrame,
};

/// Where a decoder's keyframe requests go (the session's control channel).
pub(crate) trait Outgoing: Send + Sync {
    fn send_text(&self, text: String) -> bool;
    fn is_closed(&self) -> bool;
}

impl Outgoing for cua_sdk::MediaOutgoing {
    fn send_text(&self, text: String) -> bool {
        cua_sdk::MediaOutgoing::send_text(self, text)
    }
    fn is_closed(&self) -> bool {
        cua_sdk::MediaOutgoing::is_closed(self)
    }
}

#[cfg(test)]
impl Outgoing for tokio::sync::mpsc::UnboundedSender<tokio_tungstenite::tungstenite::Message> {
    fn send_text(&self, text: String) -> bool {
        self.send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .is_ok()
    }
    fn is_closed(&self) -> bool {
        tokio::sync::mpsc::UnboundedSender::is_closed(self)
    }
}

/// Opens a media session on `client` and delivers *decoded* video (packed
/// BGRA) and control events to `frames`. Lost references trigger a keyframe
/// request automatically. (Swift: `client.openMediaDecoded(...)`.)
#[uniffi::export]
pub async fn spacesd_open_media_decoded(
    client: Arc<SpacesdClient>,
    options: MediaOpenOptions,
    frames: Arc<dyn DecodedFrameSink>,
) -> Result<Arc<MediaSession>> {
    open_decoded(client, options, frames, None).await
}

/// [`spacesd_open_media_decoded`] that also delivers decoded PCM audio to
/// `pcm` (set `options.audio` to negotiate audio tracks).
#[uniffi::export]
pub async fn spacesd_open_media_decoded_with_audio(
    client: Arc<SpacesdClient>,
    options: MediaOpenOptions,
    frames: Arc<dyn DecodedFrameSink>,
    pcm: Arc<dyn PcmSink>,
) -> Result<Arc<MediaSession>> {
    open_decoded(client, options, frames, Some(pcm)).await
}

async fn open_decoded(
    client: Arc<SpacesdClient>,
    options: MediaOpenOptions,
    frames: Arc<dyn DecodedFrameSink>,
    pcm: Option<Arc<dyn PcmSink>>,
) -> Result<Arc<MediaSession>> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let decoding = Decoding::new(frames, pcm);
        let audio: Arc<dyn AudioSink> = decoding.clone();
        let sink: Arc<dyn FrameSink> = decoding.clone();
        let session = client.open_media_with_audio(options, sink, audio).await?;
        decoding.attach(&session);
        Ok(session)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (client, options, frames, pcm);
        Err(cua_sdk::CuaError::Unsupported(
            "decoded media is built for macOS, Windows and Linux; use open_media and decode in the host".into(),
        ))
    }
}

/// One decoded video frame : packed top-down BGRA.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DecodedVideoFrame {
    /// Per-target frame sequence.
    pub sequence: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row (`width * 4`).
    pub stride: u32,
    /// Pixel format; always `bgra`.
    pub format: String,
    /// Media-clock capture time (µs).
    pub capture_timestamp_us: u64,
    /// Decoder configuration generation of the source frame.
    pub codec_epoch: u64,
    /// Coordinate generation.
    pub geometry_epoch: u64,
    /// The source access unit was a keyframe.
    pub keyframe: bool,
    /// Bytes of the source (encoded) access unit.
    pub encoded_size: u64,
    /// When the encoded frame reached the decoder (Unix µs, this host).
    pub received_at_us: u64,
    /// Time spent decoding and converting it (µs).
    pub decode_duration_us: u64,
    /// Pixels.
    pub data: Vec<u8>,
}

/// Decoded audio : interleaved signed 16-bit PCM.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PcmAudio {
    /// Track id from `NegotiatedAudio`.
    pub track_id: u16,
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Interleaved channels.
    pub channels: u16,
    /// Media-clock time of the first sample (µs).
    pub pts_us: u64,
    /// Synthesized by packet-loss concealment for a lost packet.
    pub concealed: bool,
    /// RAU2 sequence of the packet this frame was decoded from (the packet
    /// that revealed the loss, for concealed frames).
    pub sequence: u32,
    /// RAU2 config epoch (the audio codec generation).
    pub config_epoch: u8,
    /// Bytes of the encoded packet (0 for concealed frames).
    pub encoded_size: u64,
    /// When the packet reached the decoder (Unix µs, this host).
    pub received_at_us: u64,
    /// Time spent decoding the packet (µs).
    pub decode_duration_us: u64,
    /// Interleaved samples.
    pub samples: Vec<i16>,
}

/// Receives decoded video frames and control events. The SDK decodes on
/// the session's delivery thread (VideoToolbox on macOS, OpenH264
/// elsewhere); implementations must return quickly.
#[uniffi::export(with_foreign)]
pub trait DecodedFrameSink: Send + Sync {
    /// A decoded frame.
    fn on_decoded_frame(&self, frame: DecodedVideoFrame);
    /// A control message, a `decode_error`, or the socket close.
    fn on_event(&self, event: MediaEvent);
}

/// Receives decoded audio. Must return quickly.
#[uniffi::export(with_foreign)]
pub trait PcmSink: Send + Sync {
    /// One decoded (or concealed) audio frame.
    fn on_pcm(&self, audio: PcmAudio);
}

/// Adapts the encoded callbacks to decoded ones.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) struct Decoding {
    frames: Arc<dyn DecodedFrameSink>,
    pcm: Option<Arc<dyn PcmSink>>,
    video: Mutex<cua_media_client::decode::VideoDecoder>,
    audio: Mutex<cua_media_client::decode::AudioDecoders>,
    keyframe: Mutex<KeyframeTarget>,
    decoder_logged: std::sync::atomic::AtomicBool,
    /// The session's counters (hardware vs software decode for telemetry).
    session: Mutex<Option<std::sync::Weak<MediaSession>>>,
}

/// Where keyframe requests go. Frames can reach the decoder before
/// [`Decoding::attach`] supplies the socket, and on `attach_url` the session
/// id is only known once `session_opened` is delivered. A request made
/// while either is missing is held (`pending`) and sent as soon as both are
/// known, so it is neither lost nor sent with an empty session id.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[derive(Default)]
struct KeyframeTarget {
    outgoing: Option<Arc<dyn Outgoing>>,
    session_id: String,
    pending: bool,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl KeyframeTarget {
    /// Sends a request now if the target is complete, else holds it.
    fn request(&mut self) {
        match &self.outgoing {
            Some(outgoing) if !self.session_id.is_empty() => {
                let message = serde_json::json!(
                    {"type": "request_keyframe", "payload": {"session_id": self.session_id}}
                );
                let _ = outgoing.send_text(message.to_string());
                self.pending = false;
            }
            _ => self.pending = true,
        }
    }

    /// Sends a held request once the target became complete.
    fn flush(&mut self) {
        if self.pending {
            self.request();
        }
    }
}

/// Re-requests a keyframe on the decoder's backoff while it waits for one,
/// until the session's socket closes or the decoding state is dropped. The
/// clock is Tokio's, so tests drive it with paused time.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
async fn keyframe_ticker(weak: std::sync::Weak<Decoding>, outgoing: Arc<dyn Outgoing>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let Some(this) = weak.upgrade() else { break };
        if outgoing.is_closed() {
            break;
        }
        let due = this
            .video
            .lock()
            .unwrap()
            .poll_keyframe_request(tokio::time::Instant::now().into_std());
        if due {
            this.send_keyframe_request();
        }
    }
}

/// Unix time in µs.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn unix_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl Decoding {
    pub(crate) fn new(
        frames: Arc<dyn DecodedFrameSink>,
        pcm: Option<Arc<dyn PcmSink>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            frames,
            pcm,
            video: Mutex::new(cua_media_client::decode::VideoDecoder::new()),
            audio: Mutex::new(cua_media_client::decode::AudioDecoders::new()),
            keyframe: Mutex::new(KeyframeTarget::default()),
            decoder_logged: std::sync::atomic::AtomicBool::new(false),
            session: Mutex::new(None),
        })
    }

    /// Where keyframe requests go once the session is attached. Also starts
    /// a ticker that re-requests a keyframe with backoff while the decoder
    /// waits for one, even when no frames arrive at all (a lost request or
    /// a static target would otherwise freeze the picture). It stops with
    /// the session.
    pub(crate) fn attach(self: &Arc<Self>, session: &Arc<MediaSession>) {
        *self.session.lock().unwrap() = Some(Arc::downgrade(session));
        let outgoing: Arc<dyn Outgoing> = Arc::new(session.outgoing());
        {
            let mut keyframe = self.keyframe.lock().unwrap();
            keyframe.outgoing = Some(outgoing.clone());
            if keyframe.session_id.is_empty() {
                keyframe.session_id = session.session_id();
            }
            keyframe.flush();
        }
        cua_sdk::support::spawn(keyframe_ticker(Arc::downgrade(self), outgoing));
    }

    /// The decoder paces requests (`VideoDecoder::poll_keyframe_request`).
    fn send_keyframe_request(&self) {
        self.keyframe.lock().unwrap().request();
    }

    fn error(&self, message: String) {
        self.frames.on_event(MediaEvent {
            kind: "decode_error".into(),
            json: serde_json::json!({"message": message}).to_string(),
        });
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl FrameSink for Decoding {
    fn on_frame(&self, frame: VideoFrame) {
        use cua_media_client::decode::{EncodedVideo, VideoOutcome};
        let received_at_us = unix_us();
        let started = std::time::Instant::now();
        let mut video = self.video.lock().unwrap();
        let outcome = video.decode(EncodedVideo {
            codec: &frame.codec,
            keyframe: frame.keyframe,
            sequence: frame.sequence,
            width: frame.width,
            height: frame.height,
            capture_timestamp_us: frame.capture_timestamp_us,
            codec_epoch: frame.codec_epoch,
            geometry_epoch: frame.geometry_epoch,
            data: &frame.data,
        });
        if matches!(outcome, Ok(VideoOutcome::Frame(_)))
            && !self
                .decoder_logged
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            tracing::info!(
                backend = video.backend().as_deref().unwrap_or("none"),
                hardware = ?video.hardware(),
                "video decoder opened"
            );
            let session = self
                .session
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|w| w.upgrade());
            if let (Some(hw), Some(session)) = (video.hardware(), session) {
                session.record_decode(hw);
            }
        }
        drop(video);
        match outcome {
            Ok(VideoOutcome::Frame(decoded)) => self.frames.on_decoded_frame(DecodedVideoFrame {
                sequence: decoded.sequence,
                width: decoded.width,
                height: decoded.height,
                stride: decoded.width * 4,
                format: "bgra".into(),
                capture_timestamp_us: decoded.capture_timestamp_us,
                codec_epoch: decoded.codec_epoch,
                geometry_epoch: decoded.geometry_epoch,
                keyframe: decoded.keyframe,
                encoded_size: decoded.encoded_size,
                received_at_us,
                decode_duration_us: started.elapsed().as_micros() as u64,
                data: decoded.bgra,
            }),
            Ok(VideoOutcome::Pending) => {}
            Ok(VideoOutcome::NeedKeyframe) => self.send_keyframe_request(),
            Err(message) => self.error(message),
        }
    }

    fn on_event(&self, event: MediaEvent) {
        match event.kind.as_str() {
            "audio_config" => {
                if let Err(message) = self.audio.lock().unwrap().configure_json(&event.json) {
                    self.error(message);
                }
            }
            "session_opened" => {
                let id = serde_json::from_str::<serde_json::Value>(&event.json)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/payload/session_id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    });
                if let Some(id) = id.filter(|id| !id.is_empty()) {
                    let mut keyframe = self.keyframe.lock().unwrap();
                    keyframe.session_id = id;
                    keyframe.flush();
                }
            }
            _ => {}
        }
        self.frames.on_event(event);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl AudioSink for Decoding {
    fn on_audio(&self, packet: AudioPacket) {
        let Some(pcm) = &self.pcm else { return };
        let received_at_us = unix_us();
        let started = std::time::Instant::now();
        let frames = self
            .audio
            .lock()
            .unwrap()
            .decode(cua_media_client::decode::EncodedAudio {
                track_id: packet.track_id,
                sequence: packet.sequence,
                pts_us: packet.pts_us,
                frame_samples: packet.frame_samples,
                config_epoch: packet.config_epoch,
                discontinuity: packet.discontinuity,
                dtx: packet.dtx,
                data: &packet.data,
            });
        let decode_duration_us = started.elapsed().as_micros() as u64;
        for frame in frames {
            pcm.on_pcm(PcmAudio {
                track_id: frame.track_id,
                sample_rate: frame.sample_rate,
                channels: frame.channels,
                pts_us: frame.pts_us,
                concealed: frame.concealed,
                sequence: packet.sequence,
                config_epoch: packet.config_epoch,
                encoded_size: if frame.concealed {
                    0
                } else {
                    packet.data.len() as u64
                },
                received_at_us,
                decode_duration_us,
                samples: frame.samples,
            });
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub mod media_session {
    //! Rust hosts and tests.
    use super::*;

    /// Attaches to a media WebSocket URL directly and receives decoded
    /// frames (and PCM when `pcm` is set).
    pub async fn attach_url_decoded(
        url: String,
        headers: Vec<(String, String)>,
        frames: Arc<dyn DecodedFrameSink>,
        pcm: Option<Arc<dyn PcmSink>>,
    ) -> Result<Arc<MediaSession>> {
        let decoding = Decoding::new(frames, pcm);
        let audio: Option<Arc<dyn AudioSink>> = Some(decoding.clone());
        let session = MediaSession::attach_url(url, headers, decoding.clone(), audio).await?;
        decoding.attach(&session);
        Ok(session)
    }
}

#[cfg(all(
    test,
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
mod decoded_tests {
    use super::*;
    use cua_media_codec::types::{
        Backend, EncoderConfig, VideoCodec as Codec, VideoFrame as RawFrame,
    };
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite;

    #[derive(Default)]
    struct Collect {
        frames: Mutex<Vec<DecodedVideoFrame>>,
        events: Mutex<Vec<MediaEvent>>,
        pcm: Mutex<Vec<PcmAudio>>,
    }

    impl DecodedFrameSink for Collect {
        fn on_decoded_frame(&self, frame: DecodedVideoFrame) {
            self.frames.lock().unwrap().push(frame);
        }
        fn on_event(&self, event: MediaEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    impl PcmSink for Collect {
        fn on_pcm(&self, audio: PcmAudio) {
            self.pcm.lock().unwrap().push(audio);
        }
    }

    fn video(sequence: u64, keyframe: bool, data: &[u8]) -> Vec<u8> {
        let header = serde_json::json!({"session_id":"m1","sequence":sequence,"geometry_epoch":1,
                "codec_epoch":1,"width_px":64,"height_px":48,"capture_timestamp_us":sequence*33_000,
                "codec":"h264","keyframe":keyframe});
        {
            let h = serde_json::to_vec(&header).unwrap();
            let mut b = (h.len() as u32).to_be_bytes().to_vec();
            b.extend((data.len() as u32).to_be_bytes());
            b.extend(h);
            b.extend(data);
            b
        }
    }

    fn audio(sequence: u32, data: &[u8]) -> Vec<u8> {
        let mut b = b"RAU2".to_vec();
        b.push(2);
        b.push(0);
        b.extend(1u16.to_be_bytes());
        b.extend(sequence.to_be_bytes());
        b.extend((u64::from(sequence) * 20_000).to_be_bytes());
        b.extend(960u16.to_be_bytes());
        b.push(1);
        b.push(0);
        b.extend(data);
        b
    }

    fn texts(rx: &mut tokio::sync::mpsc::UnboundedReceiver<tungstenite::Message>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(message) = rx.try_recv() {
            if let tungstenite::Message::Text(t) = message {
                out.push(t.as_str().to_string());
            }
        }
        out
    }

    #[test]
    fn keyframe_requests_wait_for_socket_and_session_id() {
        // Neither socket nor id: the request is held, not lost.
        let mut target = KeyframeTarget::default();
        target.request();
        assert!(target.pending);
        // Socket known (attach ran first) but no id yet: still held, never
        // sent with an empty session id.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        target.outgoing = Some(Arc::new(tx));
        target.flush();
        assert!(texts(&mut rx).is_empty() && target.pending);
        // `session_opened` supplies the id: exactly one request goes out.
        target.session_id = "m1".into();
        target.flush();
        target.flush();
        let sent = texts(&mut rx);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].contains("request_keyframe") && sent[0].contains("\"m1\""));
        assert!(!target.pending);
        // Complete target: requests go straight out.
        target.request();
        assert_eq!(texts(&mut rx).len(), 1);
    }

    /// No keyframe comes and no frames arrive: the ticker asks again on the
    /// decoder's backoff (500 ms, then 1 s, then 2 s), so a lost request
    /// cannot freeze the picture. Paused time makes the schedule exact.
    #[tokio::test(start_paused = true)]
    async fn ticker_re_requests_on_backoff_without_frames() {
        let sink = Arc::new(Collect::default());
        let decoding = Decoding::new(sink, None);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        {
            let mut target = decoding.keyframe.lock().unwrap();
            target.outgoing = Some(Arc::new(tx.clone()));
            target.session_id = "m1".into();
        }
        let ticker = tokio::spawn(keyframe_ticker(Arc::downgrade(&decoding), Arc::new(tx)));
        // Let the ticker start (and take its first tick) at time zero.
        tokio::task::yield_now().await;
        let mut count = 0;
        let mut elapsed = std::time::Duration::ZERO;
        // (time since start, requests sent so far): at 0, 0.5 s, 1.5 s, 3.5 s.
        for (at_ms, expected) in [
            (100, 1),
            (400, 1),
            (600, 2),
            (1400, 2),
            (1600, 3),
            (3400, 3),
            (3600, 4),
        ] {
            // Step the clock like a running process would, letting the
            // ticker run at every step.
            let at = std::time::Duration::from_millis(at_ms);
            while elapsed < at {
                tokio::time::advance(std::time::Duration::from_millis(10)).await;
                tokio::task::yield_now().await;
                elapsed += std::time::Duration::from_millis(10);
            }
            let sent = texts(&mut rx);
            assert!(
                sent.iter()
                    .all(|t| t.contains("request_keyframe") && t.contains("\"m1\"")),
                "{sent:?}"
            );
            count += sent.len();
            assert_eq!(count, expected, "requests by {at_ms} ms");
        }
        // Dropping the decoding state stops the ticker.
        drop(decoding);
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        ticker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn decoded_callbacks_deliver_bgra_and_pcm_and_request_keyframes() {
        // Real H.264 and Opus from cua-media-codec.
        let mut encoder = cua_media_codec::backends::open_encoder(
            Backend::OpenH264,
            &EncoderConfig::new(Codec::H264, 64, 48, 30),
        )
        .unwrap();
        let units: Vec<Vec<u8>> = (0..3u64)
            .map(|index| {
                let bgra: Vec<u8> = (0..64 * 48)
                    .flat_map(|p| [(p as u64 + index) as u8, 90, 180, 255])
                    .collect();
                encoder
                    .encode(&RawFrame::bgra(64, 48, index, &bgra))
                    .unwrap()
                    .remove(0)
                    .data
            })
            .collect();
        let mut opus = cua_media_codec::audio::codec::open_audio_encoder(
            &cua_media_codec::audio::codec::AudioEncodingConfig {
                dtx: false,
                ..cua_media_codec::audio::codec::AudioEncodingConfig::downlink()
            },
        )
        .unwrap();
        let opus_packet = opus.encode(&vec![500i16; 960 * 2]).unwrap().data;

        let unit_sizes: Vec<u64> = units.iter().map(|u| u.len() as u64).collect();
        let opus_size = opus_packet.len() as u64;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/media", listener.local_addr().unwrap());
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let text = |s: String| tungstenite::Message::Text(s.into());
            let bin = |b: Vec<u8>| tungstenite::Message::Binary(b.into());
            ws.send(text(
                r#"{"type":"session_opened","payload":{"session_id":"m1"}}"#.into(),
            ))
            .await
            .unwrap();
            ws.send(text(r#"{"type":"audio_config","payload":{"track_id":1,"config_epoch":1,"direction":"down",
                    "codec":"opus","sample_rate_hz":48000,"channels":2,"frame_ms":20,"bitrate_kbps":64,"fec":false,
                    "dtx":false,"opus_pre_skip":312,"source":{"source_id":"desktop","kind":"desktop"}}}"#.into()))
                    .await
                    .unwrap();
            ws.send(bin(video(1, true, &units[0]))).await.unwrap();
            ws.send(bin(video(2, false, &units[1]))).await.unwrap();
            ws.send(bin(audio(5, &opus_packet))).await.unwrap();
            ws.send(bin(audio(7, &opus_packet))).await.unwrap();
            // Sequence 3 lost: the client must ask for a keyframe.
            ws.send(bin(video(4, false, &units[2]))).await.unwrap();
            for _ in 0..16 {
                match tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await {
                    Ok(Some(Ok(tungstenite::Message::Text(t)))) => {
                        let _ = seen_tx.send(t.as_str().to_string());
                    }
                    _ => break,
                }
            }
        });

        let sink = Arc::new(Collect::default());
        let pcm: Arc<dyn PcmSink> = sink.clone();
        let session = media_session::attach_url_decoded(url, vec![], sink.clone(), Some(pcm))
            .await
            .unwrap();
        let request = tokio::time::timeout(std::time::Duration::from_secs(10), seen_rx.recv())
            .await
            .expect("keyframe request within 10 s")
            .unwrap();
        assert!(
            request.contains("request_keyframe") && request.contains("m1"),
            "{request}"
        );
        // The ticker's repeat requests are timed in
        // `ticker_re_requests_on_backoff_without_frames` with paused time.
        // Bounded wait for the end state (10 s), generous for loaded CI hosts.
        for _ in 0..500 {
            if sink.frames.lock().unwrap().len() >= 2 && sink.pcm.lock().unwrap().len() >= 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let frames = sink.frames.lock().unwrap().clone();
        assert_eq!(
            frames.len(),
            2,
            "keyframe and one delta; the delta after the gap is held"
        );
        assert_eq!(
            (frames[0].width, frames[0].height, frames[0].stride),
            (64, 48, 256)
        );
        assert_eq!(frames[0].data.len(), 64 * 48 * 4);
        assert_eq!(frames[0].format, "bgra");
        assert!(frames[0].keyframe && !frames[1].keyframe);
        assert_eq!(frames[0].encoded_size, unit_sizes[0]);
        assert_eq!(frames[1].encoded_size, unit_sizes[1]);
        assert!(
            frames[0].received_at_us > 0 && frames[1].received_at_us >= frames[0].received_at_us
        );
        assert_eq!(frames[0].codec_epoch, 1);
        let pcm = sink.pcm.lock().unwrap().clone();
        assert_eq!(pcm.len(), 3, "two decoded frames and one concealed");
        assert!(pcm[1].concealed && !pcm[2].concealed);
        assert_eq!((pcm[0].sequence, pcm[0].config_epoch), (5, 1));
        assert_eq!(pcm[0].encoded_size, opus_size);
        assert_eq!(pcm[1].encoded_size, 0, "concealed");
        assert_eq!(pcm[2].sequence, 7);
        assert!(pcm.iter().all(|p| p.received_at_us > 0));
        assert!(
            pcm.iter()
                .all(|p| p.samples.len() == 960 * 2 && p.sample_rate == 48_000 && p.channels == 2)
        );
        session.close().await.unwrap();
    }
}
