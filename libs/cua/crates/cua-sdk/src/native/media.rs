//! Media sessions: rcdp wire v2 over the `/media` WebSocket, delivered to
//! foreign callbacks as encoded access units (H.264 Annex B / BGRA / PNG)
//! and audio packets (Opus / PCM) with their metadata. Decoding stays in
//! the host language by default (VideoToolbox, WebCodecs, MediaCodec, ...).
//!
//! Callbacks run on one dedicated delivery thread per session, never on the
//! network task. When a sink falls behind, *video* frames are dropped
//! (latest-frame semantics, counted in `stats()`); audio packets and control
//! events are never dropped before the 1024-item queue bound.

use crate::MediaOpenOptions;
use crate::{CuaError, Result};
use cua_spacesd_client::pb;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
    mpsc,
};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

/// One encoded video access unit.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VideoFrame {
    /// Per-target frame sequence (may start anywhere; gaps mean replaced
    /// frames).
    pub sequence: u64,
    /// `h264`, `bgra` or `png`.
    pub codec: String,
    /// Random-access point (IDR with SPS/PPS for H.264).
    pub keyframe: bool,
    /// Payload width in pixels.
    pub width: u32,
    /// Payload height in pixels.
    pub height: u32,
    /// Media-clock capture time (µs).
    pub capture_timestamp_us: u64,
    /// Decoder configuration generation; reset the decoder when it changes.
    pub codec_epoch: u64,
    /// Coordinate generation.
    pub geometry_epoch: u64,
    /// Encoded payload.
    pub data: Vec<u8>,
    /// The packet's JSON header, verbatim.
    pub header_json: String,
}

/// One audio packet (MEDIA.md §12.2).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AudioPacket {
    /// Track id from `NegotiatedAudio`.
    pub track_id: u16,
    /// Per-track sequence (wraps mod 2³²).
    pub sequence: u32,
    /// Media-clock time of the first sample (µs).
    pub pts_us: u64,
    /// Samples per channel.
    pub frame_samples: u16,
    /// Must match the latest `audio_config` of the track.
    pub config_epoch: u8,
    /// The previous packet was dropped or the track restarted.
    pub discontinuity: bool,
    /// DTX / comfort-noise frame.
    pub dtx: bool,
    /// One Opus packet or interleaved s16le PCM.
    pub data: Vec<u8>,
}

/// A control message (`hello`, `session_opened`, `lifecycle`,
/// `audio_config`, `stats`, `error`, ...) or the close of the socket
/// (`kind = "closed"`, `json = {"code":…, "reason":…}`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaEvent {
    /// The message `type`.
    pub kind: String,
    /// The whole message as JSON.
    pub json: String,
}

/// Receives video frames and control events. Implementations must return
/// quickly and hand off decoding.
#[uniffi::export(with_foreign)]
pub trait FrameSink: Send + Sync {
    /// A video access unit.
    fn on_frame(&self, frame: VideoFrame);
    /// A control message or the socket close.
    fn on_event(&self, event: MediaEvent);
}

/// Receives audio packets. Must return quickly.
#[uniffi::export(with_foreign)]
pub trait AudioSink: Send + Sync {
    /// An audio packet.
    fn on_audio(&self, packet: AudioPacket);
}

/// Delivery counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MediaStats {
    /// Video frames delivered.
    pub frames: u64,
    /// Video frames dropped because the sink fell behind.
    pub frames_dropped: u64,
    /// Audio packets delivered.
    pub audio_packets: u64,
    /// Malformed binary messages ignored.
    pub malformed: u64,
    /// Control events delivered.
    pub events: u64,
}

struct Counters {
    frames: AtomicU64,
    frames_dropped: AtomicU64,
    audio: AtomicU64,
    malformed: AtomicU64,
    events: AtomicU64,
    // Stream-stats telemetry (`cua_stream_stats`, sent bucketed on close).
    started: std::time::Instant,
    received: AtomicU64,
    max_height: AtomicU32,
    /// 0: not decoded here, 1: software, 2: hardware.
    decode: AtomicU8,
    codec: Mutex<String>,
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            frames: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            audio: AtomicU64::new(0),
            malformed: AtomicU64::new(0),
            events: AtomicU64::new(0),
            started: std::time::Instant::now(),
            received: AtomicU64::new(0),
            max_height: AtomicU32::new(0),
            decode: AtomicU8::new(0),
            codec: Mutex::new(String::new()),
        }
    }
}

impl Counters {
    /// A received video frame, for the stream summary.
    fn saw_frame(&self, f: &VideoFrame) {
        self.received.fetch_add(1, Ordering::Relaxed);
        self.max_height.fetch_max(f.height, Ordering::Relaxed);
        let mut codec = self.codec.lock().unwrap_or_else(|p| p.into_inner());
        if codec.is_empty() {
            *codec = f.codec.clone();
        }
    }

    /// What the stream looked like, for `cua_stream_stats`.
    fn summary(&self) -> super::telemetry::StreamSummary {
        super::telemetry::StreamSummary {
            transport: "websocket",
            codec: self.codec.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            hw_decode: match self.decode.load(Ordering::Relaxed) {
                1 => Some(false),
                2 => Some(true),
                _ => None,
            },
            frames: self.received.load(Ordering::Relaxed),
            height: self.max_height.load(Ordering::Relaxed),
            duration: self.started.elapsed(),
        }
    }
}

enum Item {
    Frame(VideoFrame),
    Audio(AudioPacket),
    Event(MediaEvent),
}

/// A live media session.
#[derive(uniffi::Object)]
pub struct MediaSession {
    session_id: String,
    open_response_json: String,
    codec: String,
    client: Option<cua_spacesd_client::SpacesdClient>,
    outgoing: tokio::sync::mpsc::UnboundedSender<tungstenite::Message>,
    closed: Arc<AtomicBool>,
    counters: Arc<Counters>,
    pump: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// A parsed binary media message.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Binary {
    Video(VideoFrame),
    Audio(AudioPacket),
}

fn find_descriptor(v: &Value, depth: u32) -> Option<&serde_json::Map<String, Value>> {
    let obj = v.as_object()?;
    if obj.contains_key("sequence") && obj.contains_key("codec") {
        return Some(obj);
    }
    if depth == 0 {
        return None;
    }
    obj.values().find_map(|c| find_descriptor(c, depth - 1))
}

/// Parses one binary WebSocket message (MEDIA.md §12.2 / protocol v1).
pub(crate) fn parse_binary(b: &[u8]) -> Option<Binary> {
    if b.len() >= 4 && &b[..4] == cua_proto::AUDIO_PACKET_MAGIC.as_slice() {
        if b.len() < cua_proto::AUDIO_PACKET_HEADER_LEN
            || b[4] != 2
            || b[23] != 0
            || b[5] & 0xfc != 0
        {
            return None;
        }
        let track_id = u16::from_be_bytes([b[6], b[7]]);
        if track_id == 0 {
            return None;
        }
        return Some(Binary::Audio(AudioPacket {
            track_id,
            sequence: u32::from_be_bytes(b[8..12].try_into().ok()?),
            pts_us: u64::from_be_bytes(b[12..20].try_into().ok()?),
            frame_samples: u16::from_be_bytes([b[20], b[21]]),
            config_epoch: b[22],
            discontinuity: b[5] & 1 != 0,
            dtx: b[5] & 2 != 0,
            data: b[cua_proto::AUDIO_PACKET_HEADER_LEN..].to_vec(),
        }));
    }
    if b.len() < 8 {
        return None;
    }
    let hl = u32::from_be_bytes(b[0..4].try_into().ok()?) as usize;
    let pl = u32::from_be_bytes(b[4..8].try_into().ok()?) as usize;
    if hl > 1 << 20 || 8 + hl + pl != b.len() {
        return None;
    }
    let header: Value = serde_json::from_slice(&b[8..8 + hl]).ok()?;
    let d = find_descriptor(&header, 4)?;
    let u = |k: &str| d.get(k).and_then(Value::as_u64).unwrap_or(0);
    Some(Binary::Video(VideoFrame {
        sequence: u("sequence"),
        codec: d
            .get("codec")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        keyframe: d.get("keyframe").and_then(Value::as_bool).unwrap_or(false),
        width: u("width_px") as u32,
        height: u("height_px") as u32,
        capture_timestamp_us: u("capture_timestamp_us"),
        codec_epoch: u("codec_epoch"),
        geometry_epoch: u("geometry_epoch"),
        data: b[8 + hl..].to_vec(),
        header_json: String::from_utf8_lossy(&b[8..8 + hl]).into_owned(),
    }))
}

fn event_of(text: &str) -> MediaEvent {
    let kind = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    MediaEvent {
        kind,
        json: text.to_string(),
    }
}

/// Opens a session through `client` and starts pumping.
pub(crate) async fn open(
    client: cua_spacesd_client::SpacesdClient,
    headers: Vec<(String, String)>,
    options: MediaOpenOptions,
    frames: Arc<dyn FrameSink>,
    audio: Option<Arc<dyn AudioSink>>,
) -> Result<Arc<MediaSession>> {
    let mut req = pb::OpenMediaRequest {
        target: Some(pb::MediaTarget {
            target: Some(match (&options.window_handle, &options.display) {
                (Some(h), _) => pb::media_target::Target::Window(pb::WindowRef {
                    id: h.clone(),
                    ..Default::default()
                }),
                (None, d) => pb::media_target::Target::DisplayId(
                    d.clone().unwrap_or_else(|| "primary".into()),
                ),
            }),
        }),
        max_fps: options.max_fps,
        max_dimension: options.max_dimension,
        disable_video: options.disable_video,
        audio: options.audio.then(|| pb::AudioOptions {
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Some(extra) = options
        .request_json
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        let mut base = serde_json::to_value(&req)?;
        let patch: Value = serde_json::from_str(extra)?;
        if let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                b.insert(k.clone(), v.clone());
            }
        }
        req = serde_json::from_value(base)?;
    }
    let resp = client.stream().open_media(req).await?.into_inner();
    let ws_path = if resp.ws_path.is_empty() {
        format!(
            "{}?ticket={}",
            cua_proto::metadata::MEDIA_WS_PATH,
            resp.ticket
        )
    } else {
        resp.ws_path.clone()
    };
    let url = client.endpoint().ws_url(&ws_path);
    let codec = format!(
        "{:?}",
        pb::MediaCodec::try_from(resp.codec).unwrap_or_default()
    )
    .to_ascii_lowercase();
    let mut shown = resp.clone();
    shown.ticket = String::new();
    shown.ws_path = String::new();
    let json = serde_json::to_string(&shown)?;
    let conn = connect(&url, &headers, frames, audio).await?;
    Ok(Arc::new(conn.into_session(
        resp.media_session_id,
        json,
        codec,
        Some(client),
    )))
}

pub(crate) struct Conn {
    outgoing: tokio::sync::mpsc::UnboundedSender<tungstenite::Message>,
    closed: Arc<AtomicBool>,
    counters: Arc<Counters>,
    pump: tokio::task::JoinHandle<()>,
}

impl Conn {
    fn into_session(
        self,
        session_id: String,
        open_response_json: String,
        codec: String,
        client: Option<cua_spacesd_client::SpacesdClient>,
    ) -> MediaSession {
        MediaSession {
            session_id,
            open_response_json,
            codec,
            client,
            outgoing: self.outgoing,
            closed: self.closed,
            counters: self.counters,
            pump: Mutex::new(Some(self.pump)),
        }
    }
}

/// Attaches to a media WebSocket URL directly (tests, bridges).
pub(crate) async fn connect(
    url: &str,
    headers: &[(String, String)],
    frames: Arc<dyn FrameSink>,
    audio: Option<Arc<dyn AudioSink>>,
) -> Result<Conn> {
    let mut request = url
        .into_client_request()
        .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (
            http::HeaderName::from_bytes(k.as_bytes()),
            http::HeaderValue::from_str(v),
        ) {
            request.headers_mut().insert(k, v);
        }
    }
    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| match e {
            tungstenite::Error::Http(r) if r.status().as_u16() == 401 => {
                CuaError::Unauthenticated("media ticket rejected".into())
            }
            tungstenite::Error::Http(r) => CuaError::Transport(format!(
                "media socket: HTTP {}: {}",
                r.status(),
                String::from_utf8_lossy(r.body().as_deref().unwrap_or_default())
            )),
            other => CuaError::Transport(format!("media socket: {other}")),
        })?;
    let (mut tx, mut rx) = ws.split();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<tungstenite::Message>();
    let closed = Arc::new(AtomicBool::new(false));
    let counters = Arc::new(Counters::default());

    // Delivery thread: foreign callbacks never run on the network task.
    let (dtx, drx) = mpsc::sync_channel::<Item>(1024);
    {
        let counters = counters.clone();
        std::thread::Builder::new()
            .name("cua-media-sink".into())
            .spawn(move || {
                while let Ok(item) = drx.recv() {
                    match item {
                        Item::Frame(f) => {
                            counters.frames.fetch_add(1, Ordering::Relaxed);
                            frames.on_frame(f);
                        }
                        Item::Audio(a) => {
                            counters.audio.fetch_add(1, Ordering::Relaxed);
                            if let Some(s) = &audio {
                                s.on_audio(a);
                            }
                        }
                        Item::Event(e) => {
                            counters.events.fetch_add(1, Ordering::Relaxed);
                            frames.on_event(e);
                        }
                    }
                }
            })
            .map_err(|e| CuaError::Internal(e.to_string()))?;
    }

    let pump = {
        let closed = closed.clone();
        let counters = counters.clone();
        super::runtime().spawn(async move {
            let mut close_json = serde_json::json!({"code": 1006, "reason": "socket ended"});
            loop {
                tokio::select! {
                    out = out_rx.recv() => match out {
                        Some(m) => {
                            let is_close = matches!(m, tungstenite::Message::Close(_));
                            if tx.send(m).await.is_err() || is_close {
                                if is_close {
                                    close_json = serde_json::json!({"code": 1000, "reason": "closed by client"});
                                }
                                break;
                            }
                        }
                        None => break,
                    },
                    msg = rx.next() => match msg {
                        Some(Ok(tungstenite::Message::Binary(b))) => match parse_binary(&b) {
                            Some(Binary::Video(f)) => {
                                counters.saw_frame(&f);
                                if dtx.try_send(Item::Frame(f)).is_err() {
                                    counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Some(Binary::Audio(a)) => {
                                let _ = dtx.send(Item::Audio(a));
                            }
                            None => {
                                counters.malformed.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Some(Ok(tungstenite::Message::Text(t))) => {
                            let _ = dtx.send(Item::Event(event_of(t.as_str())));
                        }
                        Some(Ok(tungstenite::Message::Close(c))) => {
                            if let Some(c) = c {
                                close_json = serde_json::json!({
                                    "code": u16::from(c.code),
                                    "reason": c.reason.as_str(),
                                });
                            }
                            break;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            close_json = serde_json::json!({"code": 1006, "reason": e.to_string()});
                            break;
                        }
                        None => break,
                    },
                }
            }
            closed.store(true, Ordering::SeqCst);
            // Coarse, bucketed stream stats once per session; every opt-out
            // applies (cua-telemetry decides).
            super::telemetry::stream_ended(&counters.summary());
            let _ = dtx.send(Item::Event(MediaEvent {
                kind: "closed".into(),
                json: close_json.to_string(),
            }));
        })
    };
    Ok(Conn {
        outgoing: out_tx,
        closed,
        counters,
        pump,
    })
}

#[uniffi::export]
impl MediaSession {
    /// `OpenMediaResponse.media_session_id`.
    pub fn session_id(&self) -> String {
        self.session_id.clone()
    }

    /// Negotiated video codec (`h264`, `bgra`, `png`).
    pub fn codec(&self) -> String {
        self.codec.clone()
    }

    /// `OpenMediaResponse` as proto3 JSON (ticket removed).
    pub fn open_response_json(&self) -> String {
        self.open_response_json.clone()
    }

    /// Whether the socket has closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Delivery counters.
    pub fn stats(&self) -> MediaStats {
        let c = &self.counters;
        MediaStats {
            frames: c.frames.load(Ordering::Relaxed),
            frames_dropped: c.frames_dropped.load(Ordering::Relaxed),
            audio_packets: c.audio.load(Ordering::Relaxed),
            malformed: c.malformed.load(Ordering::Relaxed),
            events: c.events.load(Ordering::Relaxed),
        }
    }

    /// Sends a JSON control message on the media socket (for example
    /// `{"type":"request_keyframe","payload":{}}` or interactive input).
    pub fn send_control(&self, json: String) -> Result<()> {
        serde_json::from_str::<Value>(&json)?;
        self.outgoing
            .send(tungstenite::Message::Text(json.into()))
            .map_err(|_| CuaError::Closed("media socket closed".into()))
    }

    /// `StreamService.RequestKeyframe`.
    pub async fn request_keyframe(&self) -> Result<()> {
        let (c, id) = self.rpc()?;
        super::run(async move {
            c.stream()
                .request_keyframe(pb::RequestKeyframeRequest {
                    media_session_id: id,
                })
                .await?;
            Ok(())
        })
        .await
    }

    /// `StreamService.SetPreferences` from proto3 JSON (the session id is
    /// filled in).
    pub async fn set_preferences_json(&self, json: String) -> Result<()> {
        let (c, id) = self.rpc()?;
        let mut v: Value = serde_json::from_str(if json.trim().is_empty() { "{}" } else { &json })?;
        if let Some(o) = v.as_object_mut() {
            o.insert("mediaSessionId".into(), Value::String(id));
        }
        let req: pb::SetPreferencesRequest = serde_json::from_value(v)?;
        super::run(async move {
            c.stream().set_preferences(req).await?;
            Ok(())
        })
        .await
    }

    /// Closes the socket and the session.
    pub async fn close(&self) -> Result<()> {
        let _ = self.outgoing.send(tungstenite::Message::Close(None));
        let pump = self.pump.lock().unwrap().take();
        let rpc = self.rpc().ok();
        super::run(async move {
            if let Some(p) = pump {
                let _ = tokio::time::timeout(super::secs(2u64), p).await;
            }
            if let Some((c, id)) = rpc {
                let _ = c
                    .stream()
                    .close_media(pb::CloseMediaRequest {
                        media_session_id: id,
                    })
                    .await;
            }
            Ok(())
        })
        .await
    }
}

impl MediaSession {
    fn rpc(&self) -> Result<(cua_spacesd_client::SpacesdClient, String)> {
        match &self.client {
            Some(c) if !self.session_id.is_empty() => Ok((c.clone(), self.session_id.clone())),
            _ => Err(CuaError::Unsupported(
                "this media session has no RPC channel".into(),
            )),
        }
    }

    /// Rust hosts and tests: attach to a media WebSocket URL directly.
    pub async fn attach_url(
        url: String,
        headers: Vec<(String, String)>,
        frames: Arc<dyn FrameSink>,
        audio: Option<Arc<dyn AudioSink>>,
    ) -> Result<Arc<MediaSession>> {
        super::run(async move {
            let conn = connect(&url, &headers, frames, audio).await?;
            Ok(Arc::new(conn.into_session(
                String::new(),
                String::new(),
                String::new(),
                None,
            )))
        })
        .await
    }
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        let _ = self.outgoing.send(tungstenite::Message::Close(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_packet(header: &Value, payload: &[u8]) -> Vec<u8> {
        let h = serde_json::to_vec(header).unwrap();
        let mut b = (h.len() as u32).to_be_bytes().to_vec();
        b.extend((payload.len() as u32).to_be_bytes());
        b.extend(h);
        b.extend(payload);
        b
    }

    #[test]
    fn parses_video_packets_with_nested_descriptors() {
        let hdr = serde_json::json!({"direction":"server","message":{"type":"video_frame","payload":{
            "session_id":"s","sequence":41,"geometry_epoch":2,"codec_epoch":3,"width_px":640,
            "height_px":480,"capture_timestamp_us":123,"codec":"h264","keyframe":true}}});
        match parse_binary(&video_packet(&hdr, b"\x00\x00\x00\x01\x67")).unwrap() {
            Binary::Video(f) => {
                assert_eq!(
                    (f.sequence, f.width, f.height, f.codec_epoch),
                    (41, 640, 480, 3)
                );
                assert!(f.keyframe);
                assert_eq!(f.codec, "h264");
                assert_eq!(f.data, b"\x00\x00\x00\x01\x67");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn counters_summarise_the_stream_for_telemetry() {
        let c = Counters::default();
        assert_eq!(c.summary().frames, 0);
        let frame = |h: u32| VideoFrame {
            sequence: 1,
            codec: "h264".into(),
            keyframe: false,
            width: 16,
            height: h,
            capture_timestamp_us: 0,
            codec_epoch: 1,
            geometry_epoch: 1,
            data: vec![],
            header_json: String::new(),
        };
        c.saw_frame(&frame(720));
        c.saw_frame(&frame(1080));
        c.decode.store(1, Ordering::Relaxed);
        let s = c.summary();
        assert_eq!((s.frames, s.height), (2, 1080));
        assert_eq!((s.transport, s.codec.as_str()), ("websocket", "h264"));
        assert_eq!(s.hw_decode, Some(false));
        // No video: nothing to report.
        assert!(
            super::super::telemetry::stream_stats_event(&Counters::default().summary()).is_none()
        );
    }

    #[test]
    fn parses_audio_packets_and_rejects_bad_ones() {
        let mut b = b"RAU2".to_vec();
        b.push(2); // version
        b.push(0b11); // discontinuity + dtx
        b.extend(7u16.to_be_bytes());
        b.extend(9u32.to_be_bytes());
        b.extend(1_000_000u64.to_be_bytes());
        b.extend(960u16.to_be_bytes());
        b.push(4); // config epoch
        b.push(0);
        b.extend([1, 2, 3]);
        match parse_binary(&b).unwrap() {
            Binary::Audio(a) => {
                assert_eq!((a.track_id, a.sequence, a.pts_us), (7, 9, 1_000_000));
                assert_eq!((a.frame_samples, a.config_epoch), (960, 4));
                assert!(a.discontinuity && a.dtx);
                assert_eq!(a.data, vec![1, 2, 3]);
            }
            other => panic!("{other:?}"),
        }
        let mut bad = b.clone();
        bad[23] = 1; // reserved
        assert!(parse_binary(&bad).is_none());
        let mut bad = b.clone();
        bad[6] = 0;
        bad[7] = 0; // track 0
        assert!(parse_binary(&bad).is_none());
        assert!(parse_binary(b"\x00\x00").is_none());
        // Length mismatch.
        let mut v = video_packet(&serde_json::json!({"sequence":1,"codec":"png"}), b"x");
        v.push(0);
        assert!(parse_binary(&v).is_none());
    }
}

// ------------------------------------------------------------ decoder hooks

/// The session's outgoing control channel, for a decoder attached outside
/// this crate (the decoded-media callbacks ship with Cua Spaces, in
/// `cua-spaces-ffi`).
#[doc(hidden)]
#[derive(Clone)]
pub struct MediaOutgoing(tokio::sync::mpsc::UnboundedSender<tungstenite::Message>);

impl MediaOutgoing {
    /// Sends a client text message; false once the socket closed.
    pub fn send_text(&self, text: String) -> bool {
        self.0.send(tungstenite::Message::Text(text.into())).is_ok()
    }

    /// True once the socket closed.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

impl MediaSession {
    /// The outgoing control channel (see [`MediaOutgoing`]).
    #[doc(hidden)]
    pub fn outgoing(&self) -> MediaOutgoing {
        MediaOutgoing(self.outgoing.clone())
    }

    /// Records where frames were decoded (stream-stats telemetry).
    #[doc(hidden)]
    pub fn record_decode(&self, hardware: bool) {
        self.counters
            .decode
            .store(if hardware { 2 } else { 1 }, Ordering::Relaxed);
    }
}
