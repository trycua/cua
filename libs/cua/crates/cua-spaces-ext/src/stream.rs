// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The streaming client behind [`cua_spaces::stream::StreamSession`]: it
//! attaches to a ticket's media socket (rcdp wire v2) and drives
//! [`cua_media_client::v2::MediaClient`], which enforces the decoder
//! invariants (keyframe gating per codec epoch, geometry epochs, per-track
//! audio loss). Encoded frames and audio packets go to the session's
//! [`FrameSink`] / [`AudioSink`] callbacks on one dedicated delivery thread.
//!
//! A ticket with a [`StreamTicket::quic`] listener attaches there first
//! (datagram video, no head-of-line blocking) and falls back to its
//! WebSocket when the listener does not answer within [`QUIC_ATTACH`].

use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use cua_media_client::v2::{MediaClient, MediaEvent, MediaPhase};
use cua_media_protocol::InteractiveInputEvent;
use cua_media_transport::quic::{MediaQuic, QuicItem};
use cua_spaces::extension::BoxFuture;
use cua_spaces::stream::{
    AudioPacket, AudioSink, DELIVERY_QUEUE, FrameSink, QuicTarget, StreamClient, StreamConnection,
    StreamEvent, StreamStats, StreamTicket, VideoFrame, explain_input_refusal,
};
use cua_spaces::{Error, Result};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

/// How long a QUIC attach may take, through the server's first message,
/// before the session attaches by its WebSocket instead.
pub const QUIC_ATTACH: Duration = Duration::from_secs(3);

/// Media held while the session opens: QUIC datagrams can overtake the
/// greeting on the reliable stream.
const MAX_EARLY_MEDIA: usize = 256;

/// The Cua Spaces streaming client ([`StreamClient`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct MediaStreams;

/// Registers [`MediaStreams`] for this process
/// ([`cua_spaces::stream::register_stream_client`]).
pub fn register() {
    cua_spaces::stream::register_stream_client(Arc::new(MediaStreams));
}

impl StreamClient for MediaStreams {
    fn attach<'a>(
        &'a self,
        ticket: &'a StreamTicket,
        headers: &'a [(String, String)],
        frames: Arc<dyn FrameSink>,
        audio: Option<Arc<dyn AudioSink>>,
    ) -> BoxFuture<'a, Result<Box<dyn StreamConnection>>> {
        Box::pin(async move {
            let s = MediaStream::connect(ticket, headers, frames, audio).await?;
            Ok(Box::new(s) as Box<dyn StreamConnection>)
        })
    }
}

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    keyframes: AtomicU64,
    frames_dropped: AtomicU64,
    frames_gated: AtomicU64,
    keyframe_requests: AtomicU64,
    audio: AtomicU64,
    audio_lost: AtomicU64,
    events: AtomicU64,
    malformed: AtomicU64,
}

enum Item {
    Frame(VideoFrame),
    Audio(AudioPacket),
    Event(StreamEvent),
}

enum Control {
    Text(String),
    Input(Vec<InteractiveInputEvent>),
    RequestKeyframe,
    Close,
}

fn to_json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One input from the media socket.
enum Incoming {
    Text(String),
    Binary(Vec<u8>),
    /// Video lost in transit (QUIC datagrams): resync on a keyframe.
    Loss,
    /// Unreadable input, skipped.
    Malformed(String),
    Closed(u16, String),
}

/// The socket a session is attached by.
enum Link {
    Ws(Box<Socket>),
    Quic {
        quic: Box<MediaQuic>,
        /// The first input, read while deciding to keep QUIC.
        early: Option<Incoming>,
    },
}

impl Link {
    fn transport(&self) -> &'static str {
        match self {
            Link::Ws(_) => "websocket",
            Link::Quic { .. } => "quic",
        }
    }

    /// The next input. Cancel-safe.
    async fn recv(&mut self) -> Incoming {
        match self {
            Link::Ws(ws) => loop {
                return match ws.next().await {
                    Some(Ok(tungstenite::Message::Text(text))) => Incoming::Text(text.to_string()),
                    Some(Ok(tungstenite::Message::Binary(bytes))) => {
                        Incoming::Binary(bytes.to_vec())
                    }
                    Some(Ok(tungstenite::Message::Close(Some(f)))) => {
                        Incoming::Closed(u16::from(f.code), f.reason.to_string())
                    }
                    Some(Ok(tungstenite::Message::Close(None))) | None => {
                        Incoming::Closed(1006, "socket ended".into())
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => Incoming::Closed(1006, e.to_string()),
                };
            },
            Link::Quic { quic, early } => {
                if let Some(first) = early.take() {
                    return first;
                }
                quic_incoming(quic.next().await)
            }
        }
    }

    async fn send_text(&mut self, text: String) -> bool {
        match self {
            Link::Ws(ws) => ws
                .send(tungstenite::Message::Text(text.into()))
                .await
                .is_ok(),
            Link::Quic { quic, .. } => quic.send_text(&text).await.is_ok(),
        }
    }

    async fn close(&mut self) {
        match self {
            Link::Ws(ws) => {
                let _ = ws.send(tungstenite::Message::Close(None)).await;
            }
            Link::Quic { quic, .. } => quic.close(),
        }
    }
}

fn quic_incoming(item: std::result::Result<QuicItem, String>) -> Incoming {
    match item {
        Ok(QuicItem::Text(text)) => Incoming::Text(text),
        Ok(QuicItem::Binary(bytes)) => Incoming::Binary(bytes),
        Ok(QuicItem::Loss { .. }) => Incoming::Loss,
        Ok(QuicItem::Closed(code)) => {
            Incoming::Closed(code.unwrap_or(1006), "QUIC connection ended".into())
        }
        Err(e) => Incoming::Malformed(e),
    }
}

/// Attaches to `quic` with `ticket`, keeping the connection only once the
/// server has answered (a greeting or media): all of it within `within`.
async fn attach_quic(
    quic: &QuicTarget,
    ticket: &str,
    within: Duration,
) -> std::result::Result<Link, String> {
    let deadline = tokio::time::Instant::now() + within;
    let mut link = MediaQuic::connect(
        &quic.host,
        quic.port,
        &quic.certificate_sha256,
        ticket,
        within,
    )
    .await?;
    let first = tokio::time::timeout_at(deadline, link.next())
        .await
        .map_err(|_| "no answer from the QUIC media listener".to_string())?;
    match quic_incoming(first) {
        Incoming::Closed(code, reason) => {
            Err(format!("QUIC media listener closed ({code}): {reason}"))
        }
        first => Ok(Link::Quic {
            quic: Box::new(link),
            early: Some(first),
        }),
    }
}

async fn attach_ws(ticket: &StreamTicket, headers: &[(String, String)]) -> Result<Link> {
    let mut request = ticket
        .ws_url
        .as_str()
        .into_client_request()
        .map_err(|e| Error::Stream(e.to_string()))?;
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (
            http::HeaderName::from_bytes(k.as_bytes()),
            http::HeaderValue::from_str(v),
        ) {
            request.headers_mut().insert(k, v);
        }
    }
    cua_spacesd_client::transport::ensure_crypto_provider();
    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| match e {
            tungstenite::Error::Http(r) => {
                Error::Stream(format!("media socket refused: HTTP {}", r.status()))
            }
            other => Error::Stream(format!("media socket: {other}")),
        })?;
    Ok(Link::Ws(Box::new(ws)))
}

/// One attached media socket.
struct MediaStream {
    media_session_id: String,
    codec: String,
    control: tokio::sync::mpsc::UnboundedSender<Control>,
    counters: Arc<Counters>,
    closed: Arc<AtomicBool>,
    phase: Arc<Mutex<MediaPhase>>,
    pump: Option<tokio::task::JoinHandle<()>>,
}

impl MediaStream {
    async fn connect(
        ticket: &StreamTicket,
        headers: &[(String, String)],
        frames: Arc<dyn FrameSink>,
        audio: Option<Arc<dyn AudioSink>>,
    ) -> Result<MediaStream> {
        let timing = Instant::now();
        let quic = match &ticket.quic {
            Some(quic) => attach_quic(quic, &ticket.ticket, QUIC_ATTACH)
                .await
                .inspect_err(|error| {
                    tracing::debug!(
                        target: "cua::stream::timing",
                        hop = "quic_fallback",
                        ms = timing.elapsed().as_secs_f64() * 1e3,
                        %error,
                        "stream.open"
                    );
                })
                .ok(),
            None => None,
        };
        let mut link = match quic {
            Some(link) => link,
            None => attach_ws(ticket, headers).await?,
        };
        tracing::debug!(
            target: "cua::stream::timing",
            hop = "connection_established",
            transport = link.transport(),
            ms = timing.elapsed().as_secs_f64() * 1e3,
            "stream.open"
        );
        let counters = Arc::new(Counters::default());
        let closed = Arc::new(AtomicBool::new(false));
        let phase = Arc::new(Mutex::new(MediaPhase::Attaching));
        let (dtx, drx) = std::sync::mpsc::sync_channel::<Item>(DELIVERY_QUEUE);
        {
            let counters = counters.clone();
            std::thread::Builder::new()
                .name("cua-spaces-media".into())
                .spawn(move || {
                    while let Ok(item) = drx.recv() {
                        match item {
                            Item::Frame(f) => {
                                counters.frames.fetch_add(1, Ordering::Relaxed);
                                if f.keyframe {
                                    counters.keyframes.fetch_add(1, Ordering::Relaxed);
                                }
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
                .map_err(|e| Error::Stream(e.to_string()))?;
        }
        let (ctl_tx, mut ctl_rx) = tokio::sync::mpsc::unbounded_channel::<Control>();
        let pump = {
            let counters = counters.clone();
            let closed = closed.clone();
            let phase = phase.clone();
            tokio::spawn(async move {
                let mut client = MediaClient::new(1);
                let mut early: VecDeque<Incoming> = VecDeque::new();
                let mut replay: VecDeque<Incoming> = VecDeque::new();
                let mut close = (1006u16, "socket ended".to_string());
                // Per-hop clock of this open (`stream.open` at debug): the
                // first packet, the first video access unit, the first keyframe.
                let mut seen_packet = false;
                let mut seen_frame = false;
                let mut seen_key = false;
                let mark = |hop: &'static str| {
                    tracing::debug!(
                        target: "cua::stream::timing",
                        hop,
                        ms = timing.elapsed().as_secs_f64() * 1e3,
                        "stream.open"
                    );
                };
                let request_keyframe = |client: &mut MediaClient| -> Option<String> {
                    let r = client.request_keyframe(Instant::now());
                    if r.is_some() {
                        counters.keyframe_requests.fetch_add(1, Ordering::Relaxed);
                    }
                    r
                };
                loop {
                    let mut outgoing: Vec<String> = Vec::new();
                    tokio::select! {
                        ctl = ctl_rx.recv() => match ctl {
                            Some(Control::Text(t)) => outgoing.push(t),
                            Some(Control::Input(events)) => match client.input(events) {
                                Ok(frames) => outgoing.extend(frames),
                                Err(e) => tracing::warn!(error = %e, "input batch refused"),
                            },
                            Some(Control::RequestKeyframe) => outgoing.extend(request_keyframe(&mut client)),
                            Some(Control::Close) | None => {
                                link.close().await;
                                close = (1000, "closed by client".into());
                                break;
                            }
                        },
                        incoming = async {
                            match replay.pop_front() {
                                Some(held) => held,
                                None => link.recv().await,
                            }
                        } => match incoming {
                            held @ (Incoming::Binary(_) | Incoming::Loss)
                                if matches!(
                                    client.phase(),
                                    MediaPhase::Attaching | MediaPhase::Greeted
                                ) =>
                            {
                                if early.len() < MAX_EARLY_MEDIA {
                                    early.push_back(held);
                                }
                            }
                            Incoming::Text(text) => match client.apply_text(text.as_str()) {
                                Ok(event) => {
                                    let item = match event {
                                        MediaEvent::Opened(o) => StreamEvent::Opened(to_json(&o)),
                                        MediaEvent::AudioConfigured(c) => StreamEvent::AudioConfigured(to_json(&c)),
                                        MediaEvent::Lifecycle(l) => StreamEvent::Lifecycle(to_json(&l)),
                                        MediaEvent::Stats(s) => StreamEvent::Stats(to_json(&s)),
                                        MediaEvent::Message(m) => {
                                            let mut v = to_json(&m);
                                            explain_input_refusal(&mut v);
                                            StreamEvent::Message(v)
                                        }
                                        MediaEvent::Video { .. } | MediaEvent::Audio { .. } => continue,
                                    };
                                    *phase.lock().expect("phase") = client.phase();
                                    if dtx.send(Item::Event(item)).is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    counters.malformed.fetch_add(1, Ordering::Relaxed);
                                    tracing::debug!(error = %e, "media text ignored");
                                }
                            },
                            Incoming::Binary(bytes) => match client.apply_binary(&bytes) {
                                Ok(Some(MediaEvent::Video { descriptor, payload })) => {
                                    if !seen_frame {
                                        seen_frame = true;
                                        mark("first_frame");
                                    }
                                    if descriptor.keyframe && !seen_key {
                                        seen_key = true;
                                        mark("first_keyframe");
                                    }
                                    let frame = VideoFrame {
                                        sequence: descriptor.sequence.0,
                                        codec: format!("{:?}", descriptor.codec).to_lowercase(),
                                        keyframe: descriptor.keyframe,
                                        width: descriptor.width_px,
                                        height: descriptor.height_px,
                                        capture_timestamp_us: descriptor.capture_timestamp_us,
                                        codec_epoch: descriptor.codec_epoch.0,
                                        geometry_epoch: descriptor.geometry_epoch.0,
                                        data: payload,
                                    };
                                    if dtx.try_send(Item::Frame(frame)).is_err() {
                                        // A dropped dependent frame corrupts
                                        // the decoder: resync on a keyframe.
                                        counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
                                        client.decoder_failed();
                                        outgoing.extend(request_keyframe(&mut client));
                                    }
                                }
                                Ok(Some(MediaEvent::Audio { header, payload, lost })) => {
                                    counters.audio_lost.fetch_add(u64::from(lost), Ordering::Relaxed);
                                    let packet = AudioPacket {
                                        track_id: header.track_id,
                                        sequence: header.sequence,
                                        pts_us: header.pts_us,
                                        frame_samples: header.frame_samples,
                                        lost,
                                        dtx: header.dtx,
                                        data: payload,
                                    };
                                    if dtx.send(Item::Audio(packet)).is_err() {
                                        break;
                                    }
                                }
                                Ok(Some(_)) => {}
                                Ok(None) => {
                                    if client.awaiting_keyframe() {
                                        counters.frames_gated.fetch_add(1, Ordering::Relaxed);
                                        outgoing.extend(request_keyframe(&mut client));
                                    }
                                }
                                Err(e) => {
                                    counters.malformed.fetch_add(1, Ordering::Relaxed);
                                    tracing::debug!(error = %e, "media packet ignored");
                                }
                            },
                            Incoming::Loss => {
                                client.decoder_failed();
                                outgoing.extend(request_keyframe(&mut client));
                            }
                            Incoming::Malformed(e) => {
                                counters.malformed.fetch_add(1, Ordering::Relaxed);
                                tracing::debug!(error = %e, "media input ignored");
                            }
                            Incoming::Closed(code, reason) => {
                                close = (code, reason);
                                break;
                            }
                        },
                    }
                    if !seen_packet && client.phase() != MediaPhase::Attaching {
                        seen_packet = true;
                        mark("session_opened");
                    }
                    if !early.is_empty() && client.phase() == MediaPhase::Open {
                        replay.append(&mut early);
                    }
                    for text in outgoing {
                        if !link.send_text(text).await {
                            break;
                        }
                    }
                }
                if let Link::Quic { quic, .. } = &link {
                    quic.close();
                }
                closed.store(true, Ordering::SeqCst);
                *phase.lock().expect("phase") = MediaPhase::Closed;
                let _ = dtx.send(Item::Event(StreamEvent::Closed {
                    code: close.0,
                    reason: close.1,
                }));
            })
        };
        Ok(MediaStream {
            media_session_id: ticket.media_session_id.clone(),
            codec: ticket.codec.clone(),
            control: ctl_tx,
            counters,
            closed,
            phase,
            pump: Some(pump),
        })
    }

    fn send(&self, c: Control) -> Result<()> {
        if self.is_closed() {
            return Err(Error::Stream("the media session is closed".into()));
        }
        self.control
            .send(c)
            .map_err(|_| Error::Stream("the media session is closed".into()))
    }
}

impl StreamConnection for MediaStream {
    fn media_session_id(&self) -> &str {
        &self.media_session_id
    }

    fn codec(&self) -> &str {
        &self.codec
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn is_open(&self) -> bool {
        *self.phase.lock().expect("phase") == MediaPhase::Open
    }

    fn stats(&self) -> StreamStats {
        let c = &self.counters;
        StreamStats {
            frames: c.frames.load(Ordering::Relaxed),
            keyframes: c.keyframes.load(Ordering::Relaxed),
            frames_dropped: c.frames_dropped.load(Ordering::Relaxed),
            frames_gated: c.frames_gated.load(Ordering::Relaxed),
            keyframe_requests: c.keyframe_requests.load(Ordering::Relaxed),
            audio_packets: c.audio.load(Ordering::Relaxed),
            audio_lost: c.audio_lost.load(Ordering::Relaxed),
            events: c.events.load(Ordering::Relaxed),
            malformed: c.malformed.load(Ordering::Relaxed),
        }
    }

    fn request_keyframe(&self) -> Result<()> {
        self.send(Control::RequestKeyframe)
    }

    fn send_input(&self, events: Vec<serde_json::Value>) -> Result<()> {
        let events = events
            .into_iter()
            .map(serde_json::from_value::<InteractiveInputEvent>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::invalid(format!("interactive input: {e}")))?;
        self.send(Control::Input(events))
    }

    fn send_text(&self, json: String) -> Result<()> {
        self.send(Control::Text(json))
    }

    fn close(&mut self, wait: Duration) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.control.send(Control::Close);
            if let Some(pump) = self.pump.take() {
                let _ = tokio::time::timeout(wait, pump).await;
            }
        })
    }
}

impl Drop for MediaStream {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_media_protocol::v2::{self, encode_server_text};
    use cua_media_protocol::{
        CodecEpoch, FrameSequence, GeometryEpoch, SessionPolicy, SurfaceGeometry, TargetEpoch,
        VideoCodec, VideoFrameDescriptor, WindowGeometryControl, WindowSessionId, WireHeader,
    };
    use cua_media_transport::quic::QuicIdentity;

    const TICKET: &str = "ticket-1";

    fn greeting() -> Vec<String> {
        [
            v2::ServerMessage::Hello(v2::Hello::server(Vec::new())),
            v2::ServerMessage::SessionOpened(v2::SessionOpened {
                session_id: WindowSessionId("media-1".into()),
                target: v2::MediaTargetRef::Display {
                    display_id: "primary".into(),
                },
                target_epoch: TargetEpoch(1),
                geometry_epoch: GeometryEpoch(1),
                codec_epoch: CodecEpoch(1),
                geometry: SurfaceGeometry {
                    width_px: 8,
                    height_px: 8,
                    scale_factor: 1.0,
                },
                codec: VideoCodec::Bgra,
                video: true,
                max_fps: 30,
                max_dimension: 1920,
                target_bitrate_kbps: None,
                capabilities: Vec::new(),
                action_capabilities: Vec::new(),
                policy: SessionPolicy::ViewOnly,
                geometry_control: WindowGeometryControl::ObserveOnly,
            }),
        ]
        .iter()
        .map(encode_server_text)
        .collect()
    }

    fn keyframe() -> Vec<u8> {
        cua_media_transport::encode_packet(
            &WireHeader::Video(VideoFrameDescriptor {
                session_id: WindowSessionId("media-1".into()),
                sequence: FrameSequence(1),
                geometry_epoch: GeometryEpoch(1),
                codec_epoch: CodecEpoch(1),
                width_px: 8,
                height_px: 8,
                capture_timestamp_us: 1,
                encode_duration_us: None,
                codec: VideoCodec::Bgra,
                keyframe: true,
            }),
            &[7u8; 8 * 8 * 4],
        )
        .unwrap()
    }

    /// A QUIC media listener that takes [`TICKET`], sends a keyframe as
    /// datagrams before its greeting, then repeats it until the client goes
    /// away.
    fn serve_quic() -> QuicTarget {
        let identity = QuicIdentity::generate().unwrap();
        let endpoint = quinn::Endpoint::server(
            identity.server_config().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let port = endpoint.local_addr().unwrap().port();
        tokio::spawn(async move {
            // A client that refuses the certificate never gets here.
            let Ok(connection) = endpoint.accept().await.unwrap().await else {
                return;
            };
            let (mut send, mut receive) = connection.accept_bi().await.unwrap();
            let first = cua_media_transport::quic::read_message(&mut receive)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                v2::decode_client_text(&String::from_utf8_lossy(&first)),
                Ok(v2::ClientMessage::Ticket {
                    ticket: TICKET.into()
                })
            );
            let max = connection.max_datagram_size().unwrap();
            // Media overtakes the greeting, as datagrams may.
            for fragment in
                cua_media_transport::fragment_video_packet_v2(1, true, &keyframe(), max).unwrap()
            {
                connection.send_datagram(fragment.into()).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            for text in greeting() {
                cua_media_transport::quic::write_message(&mut send, text.as_bytes())
                    .await
                    .unwrap();
            }
            for packet_id in 2.. {
                for fragment in
                    cua_media_transport::fragment_video_packet_v2(packet_id, true, &keyframe(), max)
                        .unwrap()
                {
                    if connection.send_datagram(fragment.into()).is_err() {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        QuicTarget {
            host: "127.0.0.1".into(),
            port,
            certificate_sha256: identity.certificate_sha256(),
        }
    }

    /// A `/media` WebSocket that greets and sends one keyframe.
    async fn serve_ws() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            for text in greeting() {
                ws.send(tungstenite::Message::Text(text.into()))
                    .await
                    .unwrap();
            }
            ws.send(tungstenite::Message::Binary(keyframe().into()))
                .await
                .unwrap();
            while ws.next().await.is_some() {}
        });
        format!("ws://{address}/media?ticket={TICKET}")
    }

    fn ticket(ws_url: String, quic: Option<QuicTarget>) -> StreamTicket {
        StreamTicket {
            space: "direct:127.0.0.1:3211".into(),
            media_session_id: "media-1".into(),
            ws_url,
            ticket: TICKET.into(),
            ticket_expires_at: None,
            codec: "bgra".into(),
            wire_version: 2,
            frame_size: [8, 8],
            needs_gateway_headers: false,
            quic,
            raw: Default::default(),
        }
    }

    struct Frames(tokio::sync::mpsc::UnboundedSender<VideoFrame>);

    impl FrameSink for Frames {
        fn on_frame(&self, frame: VideoFrame) {
            let _ = self.0.send(frame);
        }
    }

    /// Attaches `ticket` and waits for its first frame.
    async fn first_frame(ticket: &StreamTicket) -> (MediaStream, VideoFrame) {
        let (sender, mut frames) = tokio::sync::mpsc::unbounded_channel();
        let stream = MediaStream::connect(ticket, &[], Arc::new(Frames(sender)), None)
            .await
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(10), frames.recv())
            .await
            .expect("a frame within 10 s")
            .unwrap();
        (stream, frame)
    }

    #[tokio::test]
    async fn a_ticket_with_a_quic_listener_streams_over_quic() {
        // Nothing listens on the WebSocket: the frame can only come by QUIC.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_url = format!("ws://{}/media", closed.local_addr().unwrap());
        drop(closed);
        let (mut stream, frame) = first_frame(&ticket(ws_url, Some(serve_quic()))).await;
        assert!(frame.keyframe);
        assert_eq!((frame.width, frame.height), (8, 8));
        assert!(stream.is_open());
        stream.close(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn an_unanswered_quic_listener_falls_back_to_the_websocket() {
        // A UDP port that takes the handshake and never answers it.
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let quic = QuicTarget {
            host: "127.0.0.1".into(),
            port: silent.local_addr().unwrap().port(),
            certificate_sha256: QuicIdentity::generate().unwrap().certificate_sha256(),
        };
        let started = Instant::now();
        let (mut stream, frame) = first_frame(&ticket(serve_ws().await, Some(quic))).await;
        assert!(frame.keyframe);
        assert!(started.elapsed() >= QUIC_ATTACH, "{:?}", started.elapsed());
        assert!(stream.is_open());
        stream.close(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn a_quic_listener_that_grants_no_stream_gives_up_within_the_deadline() {
        let identity = QuicIdentity::generate().unwrap();
        let mut config = identity.server_config().unwrap();
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(0_u8.into());
        config.transport_config(Arc::new(transport));
        let endpoint = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let quic = QuicTarget {
            host: "127.0.0.1".into(),
            port: endpoint.local_addr().unwrap().port(),
            certificate_sha256: identity.certificate_sha256(),
        };
        // The handshake completes; the client's stream never opens.
        let accepted = tokio::spawn(async move {
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            connection.closed().await
        });
        let within = Duration::from_millis(500);
        let started = Instant::now();
        let attached =
            tokio::time::timeout(Duration::from_secs(5), attach_quic(&quic, TICKET, within))
                .await
                .expect("attach_quic returns within its deadline");
        assert!(attached.is_err());
        assert!(started.elapsed() >= within, "{:?}", started.elapsed());
        accepted.abort();
    }

    #[tokio::test]
    async fn a_quic_listener_with_another_certificate_falls_back_to_the_websocket() {
        let quic = QuicTarget {
            certificate_sha256: QuicIdentity::generate().unwrap().certificate_sha256(),
            ..serve_quic()
        };
        let (mut stream, frame) = first_frame(&ticket(serve_ws().await, Some(quic))).await;
        assert!(frame.keyframe);
        stream.close(Duration::from_secs(1)).await;
    }
}
