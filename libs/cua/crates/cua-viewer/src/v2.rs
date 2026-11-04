// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! rcdp wire v2 client link (MEDIA.md): gRPC `StreamService.OpenMedia`
//! mints a ticket, the media socket (WebSocket `/media` or direct QUIC)
//! attaches with it, and the server speaks first.
//!
//! The native UI loop was written against the v1 socket handshake
//! (`authenticate` → `hello` → `list_windows` → `open_session`). [`V2Link`]
//! keeps that loop unchanged: it answers the v1 handshake messages locally
//! or over gRPC and translates the session traffic both ways. Audio never
//! reaches the UI loop; RAU2 packets are decoded with `cua-media-codec` and
//! played through an [`AudioOutput`].
//!
//! Targets: a window handle from `ListTargets`, or `display:<id>` for a
//! whole display (the display is listed as a pseudo-window).

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;

use std::time::{Duration, Instant};

use cua_media_protocol::v2::{self, decode_server_text, encode_client_text};
use cua_media_protocol::{
    ClientMessage, Hello, ServerMessage, SessionOpened, SurfaceGeometry, TargetEpoch, TargetHandle,
    VideoCodec, WindowDescriptor, WindowGeometryControl, WireHeader,
};
use cua_media_transport::VideoDatagramReassembler;
use cua_proto::env::v1 as pb;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

/// Media packets kept while waiting for `session_opened` / `audio_config`.
const MAX_EARLY_MEDIA: usize = 256;

pub type V2Result<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Window handles with this prefix name a display (`display:<id>`).
pub const DISPLAY_PREFIX: &str = "display:";

/// Events the UI loop consumes (v1-shaped).
#[derive(Debug)]
pub enum V2Event {
    Server(ServerMessage),
    Packet(WireHeader, Vec<u8>),
    VideoLoss {
        incomplete_frames: u64,
        lost_keyframe: bool,
    },
    /// The media socket closed (with the v2 close code when known).
    Closed(Option<u16>),
}

/// Where decoded audio goes.
pub trait AudioOutput: Send {
    /// Interleaved samples at `sample_rate`/`channels`.
    fn write(&mut self, samples: &[i16], sample_rate: u32, channels: u16);
}

/// Connection options parsed from a `http(s)://host:port[/?quic=1&audio=0]` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2Options {
    pub server: String,
    pub host: String,
    pub secure: bool,
    pub prefer_quic: bool,
    pub audio: bool,
}

impl V2Options {
    pub fn parse(url: &str) -> V2Result<Self> {
        let parsed = url::Url::parse(url)?;
        let secure = match parsed.scheme() {
            "http" => false,
            "https" => true,
            _ => return Err("v2 server URL must be http:// or https://".into()),
        };
        let host = parsed
            .host_str()
            .ok_or("server URL has no host")?
            .to_owned();
        let port = parsed
            .port_or_known_default()
            .ok_or("server URL has no port")?;
        let flag = |name: &str, default: bool| {
            parsed
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| matches!(value.as_ref(), "1" | "true" | "yes"))
                .unwrap_or(default)
        };
        let host_port = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        Ok(Self {
            server: format!("{}://{host_port}", parsed.scheme()),
            host,
            secure,
            prefer_quic: flag("quic", false),
            audio: flag("audio", true),
        })
    }
}

#[derive(Clone)]
struct Bearer(Option<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>);

impl tonic::service::Interceptor for Bearer {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(value) = &self.0 {
            request
                .metadata_mut()
                .insert("authorization", value.clone());
        }
        Ok(request)
    }
}

type Grpc = pb::stream_service_client::StreamServiceClient<
    tonic::service::interceptor::InterceptedService<tonic::transport::Channel, Bearer>,
>;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

enum Media {
    Ws(Box<Socket>),
    Quic(Box<QuicMedia>),
}

struct QuicMedia {
    _endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    send: quinn::SendStream,
    reliable: tokio::sync::mpsc::UnboundedReceiver<Option<Vec<u8>>>,
    reassembler: VideoDatagramReassembler,
    pending: VecDeque<Vec<u8>>,
}

/// Raw media-plane input before translation.
enum Raw {
    Text(String),
    Binary(Vec<u8>),
    Loss {
        incomplete_frames: u64,
        lost_keyframe: bool,
    },
    Closed(Option<u16>),
}

pub struct V2Link {
    options: V2Options,
    grpc: Grpc,
    media: Option<Media>,
    media_session_id: Option<String>,
    pending: VecDeque<V2Event>,
    /// Media that raced ahead of `session_opened` (QUIC datagrams are not
    /// ordered with the stream); replayed after it.
    early: VecDeque<Raw>,
    audio: AudioPlayer,
    /// Kept so an `OpenSession` for a display maps back to its id.
    displays: HashMap<String, String>,
}

impl V2Link {
    pub async fn connect(
        url: &str,
        token: Option<&str>,
        output: Option<Box<dyn AudioOutput>>,
    ) -> V2Result<Self> {
        let options = V2Options::parse(url)?;
        let mut endpoint = tonic::transport::Endpoint::from_shared(options.server.clone())?
            .connect_timeout(Duration::from_secs(10));
        if options.secure {
            endpoint = endpoint
                .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())?;
        }
        let channel = endpoint.connect().await?;
        let bearer = token
            .map(|token| format!("Bearer {token}").parse())
            .transpose()
            .map_err(|_| "token is not valid header text")?;
        let grpc = pb::stream_service_client::StreamServiceClient::with_interceptor(
            channel,
            Bearer(bearer),
        )
        .max_decoding_message_size(64 * 1024 * 1024);
        Ok(Self {
            options,
            grpc,
            media: None,
            media_session_id: None,
            pending: VecDeque::new(),
            early: VecDeque::new(),
            audio: AudioPlayer::new(output),
            displays: HashMap::new(),
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn audio_stats(&self) -> AudioStats {
        self.audio.stats
    }

    pub fn rtt(&self) -> Option<Duration> {
        match &self.media {
            Some(Media::Quic(quic)) => Some(quic.connection.rtt()),
            _ => None,
        }
    }

    /// v1 client message in; v2 traffic or a synthesized reply out.
    pub async fn send(&mut self, message: ClientMessage) -> V2Result<()> {
        match message {
            ClientMessage::Authenticate { .. } => self
                .pending
                .push_back(V2Event::Server(ServerMessage::Authenticated)),
            ClientMessage::Hello(hello) => {
                self.pending
                    .push_back(V2Event::Server(ServerMessage::Hello(Hello {
                        protocol_name: hello.protocol_name,
                        capabilities: Vec::new(),
                        ..Hello::default()
                    })))
            }
            ClientMessage::ListWindows { on_screen_only } => {
                let windows = self.list_targets(on_screen_only).await?;
                self.pending
                    .push_back(V2Event::Server(ServerMessage::Windows { windows }));
            }
            ClientMessage::OpenSession(open) => {
                let opened = self.open(&open).await?;
                self.pending
                    .push_back(V2Event::Server(ServerMessage::SessionOpened(opened)));
            }
            ClientMessage::CloseSession { .. } => self.close().await,
            ClientMessage::Action(action) => {
                self.send_v2(&v2::ClientMessage::Action(action)).await?
            }
            ClientMessage::InteractiveInput(batch) => {
                self.send_v2(&v2::ClientMessage::InteractiveInput(batch))
                    .await?
            }
            ClientMessage::RequestKeyframe { session_id } => {
                self.send_v2(&v2::ClientMessage::RequestKeyframe { session_id })
                    .await?
            }
            ClientMessage::SetStreamPreferences(preferences) => {
                self.send_v2(&v2::ClientMessage::SetStreamPreferences(preferences))
                    .await?
            }
            ClientMessage::SetWindowGeometry(request) => {
                self.send_v2(&v2::ClientMessage::SetWindowGeometry(request))
                    .await?
            }
            ClientMessage::GetStats { session_id } => {
                self.send_v2(&v2::ClientMessage::GetStats { session_id })
                    .await?
            }
            // Clipboard and the other v1-only messages are never advertised on v2.
            other => {
                tracing::debug!(target: "cua_spacesd_client::v2", message = ?std::mem::discriminant(&other), "v1 message has no v2 equivalent")
            }
        }
        Ok(())
    }

    pub async fn next(&mut self) -> V2Result<V2Event> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(event);
            }
            let raw = match self.early.pop_front() {
                Some(raw) => raw,
                None => self.next_raw().await?,
            };
            if let Some(event) = self.translate(raw) {
                return Ok(event);
            }
        }
    }

    async fn list_targets(&mut self, on_screen_only: bool) -> V2Result<Vec<WindowDescriptor>> {
        let response = self
            .grpc
            .list_targets(pb::ListTargetsRequest {
                include_windows: true,
                window_filter: None,
            })
            .await?
            .into_inner();
        let mut windows = Vec::new();
        for target in response.targets {
            match target.target {
                Some(pb::stream_target::Target::Display(display)) => {
                    let handle = format!("{DISPLAY_PREFIX}{}", display.id);
                    self.displays.insert(handle.clone(), display.id.clone());
                    let size = display.native_size.unwrap_or_default();
                    windows.push(WindowDescriptor {
                        window: TargetHandle(handle),
                        target_epoch: TargetEpoch(1),
                        app_name: "Display".into(),
                        title: if display.name.is_empty() {
                            display.id.clone()
                        } else {
                            display.name.clone()
                        },
                        geometry: SurfaceGeometry {
                            width_px: size.width,
                            height_px: size.height,
                            scale_factor: if display.scale_factor > 0.0 {
                                display.scale_factor
                            } else {
                                1.0
                            },
                        },
                        visible: target.available,
                    });
                }
                Some(pb::stream_target::Target::Window(window)) => {
                    if on_screen_only && !window.on_screen {
                        continue;
                    }
                    let Some(reference) = window.r#ref else {
                        continue;
                    };
                    let bounds = window.bounds.unwrap_or_default();
                    windows.push(WindowDescriptor {
                        window: TargetHandle(reference.id),
                        target_epoch: TargetEpoch(reference.epoch),
                        app_name: window.app.map(|app| app.name).unwrap_or_default(),
                        title: window.title,
                        geometry: SurfaceGeometry {
                            width_px: bounds.width.max(0.0) as u32,
                            height_px: bounds.height.max(0.0) as u32,
                            scale_factor: 1.0,
                        },
                        visible: target.available && window.on_screen,
                    });
                }
                None => {}
            }
        }
        Ok(windows)
    }

    async fn open(&mut self, open: &cua_media_protocol::OpenSession) -> V2Result<SessionOpened> {
        self.close().await;
        let target = match open.window.0.strip_prefix(DISPLAY_PREFIX) {
            Some(display) => pb::media_target::Target::DisplayId(
                self.displays
                    .get(&open.window.0)
                    .cloned()
                    .unwrap_or_else(|| display.to_owned()),
            ),
            None => pb::media_target::Target::Window(pb::WindowRef {
                id: open.window.0.clone(),
                epoch: open.target_epoch.0,
            }),
        };
        let codecs = open
            .accepted_codecs
            .iter()
            .filter_map(|codec| match codec {
                VideoCodec::H264 => Some(pb::MediaCodec::H264 as i32),
                VideoCodec::Bgra => Some(pb::MediaCodec::Bgra as i32),
                _ => None,
            })
            .collect();
        let policy = match open.policy {
            cua_media_protocol::SessionPolicy::ViewOnly => pb::SessionPolicy::ViewOnly,
            cua_media_protocol::SessionPolicy::BackgroundOnly => pb::SessionPolicy::BackgroundOnly,
            cua_media_protocol::SessionPolicy::AllowActivation => {
                pb::SessionPolicy::AllowActivation
            }
        };
        let opened = self
            .grpc
            .open_media(pb::OpenMediaRequest {
                target: Some(pb::MediaTarget {
                    target: Some(target),
                }),
                codecs,
                max_fps: u32::from(open.max_fps),
                max_dimension: open.max_dimension,
                bitrate_kbps: open.target_bitrate_kbps.unwrap_or(0),
                policy: policy as i32,
                geometry_control: if open.geometry_control == WindowGeometryControl::Bidirectional {
                    pb::GeometryControl::Bidirectional as i32
                } else {
                    pb::GeometryControl::ObserveOnly as i32
                },
                prefer_quic: self.options.prefer_quic,
                audio: self.options.audio.then(|| pb::AudioOptions {
                    enabled: true,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await?
            .into_inner();
        self.media_session_id = Some(opened.media_session_id.clone());
        let media = match opened.quic.as_ref().filter(|_| self.options.prefer_quic) {
            Some(quic) => {
                let port = u16::try_from(quic.port)?;
                Media::Quic(Box::new(
                    connect_quic(
                        &self.options.host,
                        port,
                        &quic.certificate_sha256,
                        &opened.ticket,
                    )
                    .await?,
                ))
            }
            None => Media::Ws(Box::new(
                connect_ws(&self.options, &opened.ws_path, &opened.ticket).await?,
            )),
        };
        self.media = Some(media);
        // Server-first greeting: hello, then session_opened.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut hello = None;
        loop {
            let raw = tokio::time::timeout_at(deadline.into(), self.next_raw())
                .await
                .map_err(|_| "no session_opened within 10 s")??;
            match raw {
                Raw::Text(text) => match decode_server_text(&text)? {
                    v2::ServerMessage::Hello(server) => hello = Some(server),
                    v2::ServerMessage::SessionOpened(session) => {
                        let hello = hello.ok_or("session_opened before hello")?;
                        if hello.selected_version != v2::WIRE_VERSION {
                            return Err(format!(
                                "server selected wire version {}",
                                hello.selected_version
                            )
                            .into());
                        }
                        return Ok(v1_opened(session, &hello));
                    }
                    v2::ServerMessage::Error { code, message, .. } => {
                        return Err(format!("server error {code:?}: {message}").into())
                    }
                    _ => {}
                },
                Raw::Closed(code) => {
                    return Err(format!("media socket closed during attach ({code:?})").into())
                }
                early @ (Raw::Binary(_) | Raw::Loss { .. }) => {
                    if self.early.len() < MAX_EARLY_MEDIA {
                        self.early.push_back(early);
                    }
                }
            }
        }
    }

    async fn close(&mut self) {
        self.early.clear();
        if let Some(media) = self.media.take() {
            match media {
                Media::Ws(mut socket) => {
                    let _ = futures_util::SinkExt::close(&mut *socket).await;
                }
                Media::Quic(quic) => quic.connection.close(0u32.into(), b"client closed"),
            }
        }
        if let Some(id) = self.media_session_id.take() {
            let _ = self
                .grpc
                .close_media(pb::CloseMediaRequest {
                    media_session_id: id,
                })
                .await;
        }
    }

    async fn send_v2(&mut self, message: &v2::ClientMessage) -> V2Result<()> {
        let text = encode_client_text(message);
        match self.media.as_mut().ok_or("no media session is open")? {
            Media::Ws(socket) => socket.send(Message::Text(text)).await?,
            Media::Quic(quic) => {
                cua_media_transport::quic::write_message(&mut quic.send, text.as_bytes()).await?
            }
        }
        Ok(())
    }

    async fn next_raw(&mut self) -> V2Result<Raw> {
        let Some(media) = self.media.as_mut() else {
            return Ok(Raw::Closed(None));
        };
        match media {
            Media::Ws(socket) => loop {
                let Some(message) = socket.next().await else {
                    return Ok(Raw::Closed(None));
                };
                match message? {
                    Message::Text(text) => return Ok(Raw::Text(text)),
                    Message::Binary(bytes) => return Ok(Raw::Binary(bytes)),
                    Message::Close(frame) => {
                        return Ok(Raw::Closed(frame.map(|frame| u16::from(frame.code))))
                    }
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                    Message::Pong(_) | Message::Frame(_) => {}
                }
            },
            Media::Quic(quic) => {
                if let Some(packet) = quic.pending.pop_front() {
                    return Ok(Raw::Binary(packet));
                }
                loop {
                    tokio::select! {
                        message = quic.reliable.recv() => {
                            return Ok(match message.flatten() {
                                Some(bytes) if bytes.first() == Some(&b'{') => Raw::Text(String::from_utf8(bytes)?),
                                Some(bytes) => Raw::Binary(bytes),
                                None => Raw::Closed(quic_close_code(&quic.connection)),
                            });
                        }
                        datagram = quic.connection.read_datagram() => {
                            let Ok(datagram) = datagram else {
                                return Ok(Raw::Closed(quic_close_code(&quic.connection)));
                            };
                            if cua_media_transport::audio::is_audio_packet(&datagram) {
                                return Ok(Raw::Binary(datagram.to_vec()));
                            }
                            let update = quic.reassembler.push(&datagram, Instant::now())?;
                            if let Some(packet) = update.packet {
                                quic.pending.push_back(packet);
                            }
                            if update.dropped_incomplete > 0 {
                                return Ok(Raw::Loss {
                                    incomplete_frames: update.dropped_incomplete,
                                    lost_keyframe: update.lost_keyframe,
                                });
                            }
                            if let Some(packet) = quic.pending.pop_front() {
                                return Ok(Raw::Binary(packet));
                            }
                        }
                    }
                }
            }
        }
    }

    fn translate(&mut self, raw: Raw) -> Option<V2Event> {
        match raw {
            Raw::Closed(code) => {
                self.media = None;
                Some(V2Event::Closed(code))
            }
            Raw::Loss {
                incomplete_frames,
                lost_keyframe,
            } => Some(V2Event::VideoLoss {
                incomplete_frames,
                lost_keyframe,
            }),
            Raw::Binary(bytes) if cua_media_transport::audio::is_audio_packet(&bytes) => {
                self.audio.packet(&bytes);
                None
            }
            Raw::Binary(bytes) => match cua_media_transport::decode_packet(&bytes) {
                Ok((header, payload)) => Some(V2Event::Packet(header, payload)),
                Err(error) => {
                    tracing::warn!(target: "cua_spacesd_client::v2", %error, "undecodable media packet");
                    None
                }
            },
            Raw::Text(text) => {
                let message = match decode_server_text(&text) {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::warn!(target: "cua_spacesd_client::v2", %error, "undecodable control message");
                        return None;
                    }
                };
                let v1 = match message {
                    v2::ServerMessage::AudioConfig(config) => {
                        self.audio.configure(&config);
                        return None;
                    }
                    v2::ServerMessage::KeyframeRequested { session_id } => {
                        ServerMessage::KeyframeRequested { session_id }
                    }
                    v2::ServerMessage::StreamPreferencesApplied(preferences) => {
                        ServerMessage::StreamPreferencesApplied(preferences)
                    }
                    v2::ServerMessage::WindowGeometryResult(result) => {
                        ServerMessage::WindowGeometryResult(result)
                    }
                    v2::ServerMessage::Lifecycle { session_id, event } => {
                        ServerMessage::Lifecycle { session_id, event }
                    }
                    v2::ServerMessage::InteractiveInputAcknowledgement(ack) => {
                        ServerMessage::InteractiveInputAcknowledgement(ack)
                    }
                    v2::ServerMessage::ActionResult(result) => ServerMessage::ActionResult(result),
                    v2::ServerMessage::ActionFrameCorrelation(correlation) => {
                        ServerMessage::ActionFrameCorrelation(correlation)
                    }
                    v2::ServerMessage::WindowState(state) => ServerMessage::WindowState(state),
                    v2::ServerMessage::Stats(stats) => ServerMessage::Stats(stats.stream),
                    v2::ServerMessage::Error { code, message, .. } => {
                        tracing::warn!(target: "cua_spacesd_client::v2", ?code, %message, "server error");
                        return None;
                    }
                    _ => return None,
                };
                Some(V2Event::Server(v1))
            }
        }
    }
}

fn quic_close_code(connection: &quinn::Connection) -> Option<u16> {
    match connection.close_reason() {
        Some(quinn::ConnectionError::ApplicationClosed(close)) => {
            // 0x401 -> 4401: the hex digits read as decimal after "4".
            let code = close.error_code.into_inner();
            let digits = format!("{code:x}");
            digits
                .parse::<u16>()
                .ok()
                .filter(|value| *value < 1000)
                .map(|value| 4000 + value)
        }
        _ => None,
    }
}

fn v1_opened(session: v2::SessionOpened, hello: &v2::Hello) -> SessionOpened {
    let mut capabilities = session.capabilities;
    for capability in &hello.capabilities {
        if !capabilities.contains(capability) {
            capabilities.push(capability.clone());
        }
    }
    SessionOpened {
        session_id: session.session_id,
        target_epoch: session.target_epoch,
        geometry_epoch: session.geometry_epoch,
        codec_epoch: session.codec_epoch,
        geometry: session.geometry,
        codec: session.codec,
        max_fps: session.max_fps,
        max_dimension: session.max_dimension,
        target_bitrate_kbps: session.target_bitrate_kbps,
        capabilities,
        action_capabilities: session.action_capabilities,
        policy: session.policy,
        geometry_control: session.geometry_control,
    }
}

async fn connect_ws(options: &V2Options, ws_path: &str, ticket: &str) -> V2Result<Socket> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    let scheme = if options.secure { "wss" } else { "ws" };
    let authority = options.server.split("://").nth(1).unwrap_or_default();
    // Header-free ticket form (MEDIA.md): drop `?ticket=` from the path.
    let path = ws_path
        .split('?')
        .next()
        .filter(|path| !path.is_empty())
        .unwrap_or("/media");
    let mut request = format!("{scheme}://{authority}{path}").into_client_request()?;
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!(
            "{}, {}{ticket}",
            cua_media_protocol::v2::WS_SUBPROTOCOL,
            cua_media_protocol::v2::WS_TICKET_SUBPROTOCOL_PREFIX
        )
        .parse()?,
    );
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| "media WebSocket connect timed out")??;
    Ok(socket)
}

async fn connect_quic(host: &str, port: u16, pin: &str, ticket: &str) -> V2Result<QuicMedia> {
    let pin = cua_media_transport::quic::parse_sha256(pin)
        .ok_or("QuicEndpoint has no valid certificate_sha256")?;
    let server: SocketAddr = tokio::net::lookup_host((host, port))
        .await?
        .next()
        .ok_or("QUIC host resolved to no addresses")?;
    let bind: SocketAddr = if server.is_ipv4() {
        "0.0.0.0:0".parse()?
    } else {
        "[::]:0".parse()?
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(cua_media_transport::quic::client_config(pin)?);
    let connection = tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.connect(server, "cua-spacesd.local")?,
    )
    .await
    .map_err(|_| "QUIC connect timed out")??;
    let (mut send, mut receive) = connection.open_bi().await?;
    let ticket = encode_client_text(&v2::ClientMessage::Ticket {
        ticket: ticket.to_owned(),
    });
    cua_media_transport::quic::write_message(&mut send, ticket.as_bytes()).await?;
    let (sender, reliable) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let message = cua_media_transport::quic::read_message(&mut receive)
                .await
                .ok()
                .flatten();
            let end = message.is_none();
            if sender.send(message).is_err() || end {
                return;
            }
        }
    });
    Ok(QuicMedia {
        _endpoint: endpoint,
        connection,
        send,
        reliable,
        reassembler: VideoDatagramReassembler::default(),
        pending: VecDeque::new(),
    })
}

/// Audio counters (for stats and tests).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioStats {
    pub packets: u64,
    pub decoded_samples: u64,
    pub concealed_frames: u64,
    pub dropped: u64,
}

struct AudioTrack {
    decoder: Box<dyn cua_media_codec::audio::codec::AudioDecoder>,
    config_epoch: u8,
    sample_rate: u32,
    channels: u16,
    last_sequence: Option<u32>,
}

/// Decodes RAU2 packets per track and plays them.
pub struct AudioPlayer {
    tracks: HashMap<u16, AudioTrack>,
    /// Packets that arrived before their track's `audio_config`.
    unconfigured: VecDeque<Vec<u8>>,
    output: Option<Box<dyn AudioOutput>>,
    stats: AudioStats,
}

/// Longest gap filled with concealment; longer gaps restart playback.
const MAX_CONCEALED_FRAMES: u32 = 5;

impl AudioPlayer {
    pub fn new(output: Option<Box<dyn AudioOutput>>) -> Self {
        Self {
            tracks: HashMap::new(),
            unconfigured: VecDeque::new(),
            output,
            stats: AudioStats::default(),
        }
    }

    pub fn configure(&mut self, config: &v2::AudioConfig) {
        use cua_media_codec::audio::codec::{AudioCodecKind, AudioEncodingConfig};
        if config.direction != v2::AudioDirection::Down || self.output.is_none() {
            return;
        }
        let codec = match config.codec {
            v2::AudioCodecName::Opus => AudioCodecKind::Opus,
            v2::AudioCodecName::PcmS16le => AudioCodecKind::PcmS16le,
            v2::AudioCodecName::Unknown => return,
        };
        let format = cua_media_codec::audio::AudioFormat {
            sample_rate: config.sample_rate_hz,
            channels: u16::from(config.channels),
        };
        let encoding = AudioEncodingConfig {
            codec,
            format,
            bitrate_kbps: config.bitrate_kbps.max(6),
            fec: config.fec,
            dtx: config.dtx,
            frame_us: u64::from(config.frame_ms) * 1_000,
            ..AudioEncodingConfig::default()
        };
        match cua_media_codec::audio::codec::open_audio_decoder(&encoding) {
            Ok(decoder) => {
                self.tracks.insert(
                    config.track_id,
                    AudioTrack {
                        decoder,
                        config_epoch: config.config_epoch,
                        sample_rate: format.sample_rate,
                        channels: format.channels,
                        last_sequence: None,
                    },
                );
            }
            Err(error) => {
                tracing::warn!(target: "cua_spacesd_client::v2", %error, track = config.track_id, "cannot decode audio track")
            }
        }
        let (ready, waiting): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.unconfigured)
            .into_iter()
            .partition(|packet| {
                cua_media_transport::audio::decode_audio_packet(packet)
                    .is_ok_and(|(header, _)| header.track_id == config.track_id)
            });
        self.unconfigured = waiting;
        for packet in ready {
            self.packet(&packet);
        }
    }

    pub fn packet(&mut self, bytes: &[u8]) {
        let Ok((header, payload)) = cua_media_transport::audio::decode_audio_packet(bytes) else {
            self.stats.dropped += 1;
            return;
        };
        let Some(track) = self.tracks.get_mut(&header.track_id) else {
            if self.output.is_some() && self.unconfigured.len() < MAX_EARLY_MEDIA {
                self.unconfigured.push_back(bytes.to_vec());
            } else {
                self.stats.dropped += 1;
            }
            return;
        };
        let Some(output) = self.output.as_mut() else {
            return;
        };
        if header.config_epoch != track.config_epoch {
            self.stats.dropped += 1;
            return;
        }
        self.stats.packets += 1;
        let frame_samples = usize::from(header.frame_samples);
        if let Some(last) = track.last_sequence {
            let gap = header.sequence.wrapping_sub(last).wrapping_sub(1);
            if !header.discontinuity
                && gap > 0
                && gap <= MAX_CONCEALED_FRAMES
                && cua_media_transport::audio::sequence_after(header.sequence, last)
            {
                for _ in 0..gap {
                    if let Ok(samples) = track.decoder.conceal(frame_samples) {
                        self.stats.concealed_frames += 1;
                        output.write(&samples, track.sample_rate, track.channels);
                    }
                }
            } else if !cua_media_transport::audio::sequence_after(header.sequence, last) {
                self.stats.dropped += 1;
                return;
            }
        }
        track.last_sequence = Some(header.sequence);
        if header.dtx || payload.is_empty() {
            return;
        }
        match track.decoder.decode(payload) {
            Ok(samples) => {
                self.stats.decoded_samples += samples.len() as u64;
                output.write(&samples, track.sample_rate, track.channels);
            }
            Err(_) => self.stats.dropped += 1,
        }
    }
}

/// Speaker output through cpal on a dedicated thread (the stream is not
/// `Send` everywhere). Keeps at most ~200 ms queued; older audio is dropped.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod speaker {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};

    const MAX_QUEUED_MS: usize = 200;

    #[derive(Default)]
    pub struct Speaker {
        queue: Arc<Mutex<VecDeque<i16>>>,
        format: Option<(u32, u16)>,
        stop: Option<std::sync::mpsc::Sender<()>>,
    }

    impl Drop for Speaker {
        fn drop(&mut self) {
            self.stop.take();
        }
    }

    impl Speaker {
        fn start(&mut self, sample_rate: u32, channels: u16) {
            self.stop.take();
            self.queue.lock().unwrap().clear();
            let queue = self.queue.clone();
            let (stop, stopped) = std::sync::mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("cua-viewer-audio".into())
                .spawn(move || {
                    let host = cpal::default_host();
                    let Some(device) = host.default_output_device() else {
                        tracing::warn!(target: "cua_spacesd_client::v2", "no audio output device");
                        return;
                    };
                    let config = cpal::StreamConfig {
                        channels,
                        sample_rate: cpal::SampleRate(sample_rate),
                        buffer_size: cpal::BufferSize::Default,
                    };
                    let stream = device.build_output_stream(
                        &config,
                        move |output: &mut [f32], _| {
                            let mut queue = queue.lock().unwrap();
                            for sample in output.iter_mut() {
                                *sample = queue.pop_front().map(|value| f32::from(value) / 32768.0).unwrap_or(0.0);
                            }
                        },
                        |error| tracing::warn!(target: "cua_spacesd_client::v2", %error, "audio output error"),
                        None,
                    );
                    match stream {
                        Ok(stream) => {
                            if let Err(error) = stream.play() {
                                tracing::warn!(target: "cua_spacesd_client::v2", %error, "audio output did not start");
                                return;
                            }
                            // Park until the Speaker is dropped or restarted.
                            let _ = stopped.recv();
                        }
                        Err(error) => tracing::warn!(target: "cua_spacesd_client::v2", %error, sample_rate, channels, "cannot open audio output"),
                    }
                })
                .ok();
            self.stop = Some(stop);
            self.format = Some((sample_rate, channels));
        }
    }

    impl super::AudioOutput for Speaker {
        fn write(&mut self, samples: &[i16], sample_rate: u32, channels: u16) {
            if self.format != Some((sample_rate, channels)) {
                self.start(sample_rate, channels);
            }
            let limit = sample_rate as usize * usize::from(channels) * MAX_QUEUED_MS / 1000;
            let mut queue = self.queue.lock().unwrap();
            queue.extend(samples.iter().copied());
            let excess = queue.len().saturating_sub(limit);
            queue.drain(..excess);
        }
    }
}

#[cfg(test)]
mod tests;
