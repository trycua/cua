// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The streaming client behind [`cua_spaces::stream::StreamSession`]: it
//! attaches to a ticket's media socket (rcdp wire v2) and drives
//! [`cua_media_client::v2::MediaClient`], which enforces the decoder
//! invariants (keyframe gating per codec epoch, geometry epochs, per-track
//! audio loss). Encoded frames and audio packets go to the session's
//! [`FrameSink`] / [`AudioSink`] callbacks on one dedicated delivery thread.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use cua_media_client::v2::{MediaClient, MediaEvent, MediaPhase};
use cua_media_protocol::InteractiveInputEvent;
use cua_spaces::extension::BoxFuture;
use cua_spaces::stream::{
    AudioPacket, AudioSink, DELIVERY_QUEUE, FrameSink, StreamClient, StreamConnection, StreamEvent,
    StreamStats, StreamTicket, VideoFrame, explain_input_refusal,
};
use cua_spaces::{Error, Result};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

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
        let (mut tx, mut rx) = ws.split();
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
                let mut close = (1006u16, "socket ended".to_string());
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
                                let _ = tx.send(tungstenite::Message::Close(None)).await;
                                close = (1000, "closed by client".into());
                                break;
                            }
                        },
                        msg = rx.next() => match msg {
                            Some(Ok(tungstenite::Message::Text(text))) => match client.apply_text(text.as_str()) {
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
                            Some(Ok(tungstenite::Message::Binary(bytes))) => match client.apply_binary(&bytes) {
                                Ok(Some(MediaEvent::Video { descriptor, payload })) => {
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
                            Some(Ok(tungstenite::Message::Close(frame))) => {
                                if let Some(f) = frame {
                                    close = (u16::from(f.code), f.reason.to_string());
                                }
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                close = (1006, e.to_string());
                                break;
                            }
                            None => break,
                        },
                    }
                    for text in outgoing {
                        if tx
                            .send(tungstenite::Message::Text(text.into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
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
