// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Media link for the benchmark: `StreamService` over gRPC, then the rcdp
//! wire v2 media socket (WebSocket `/media` or direct QUIC) attached with the
//! ticket (libs/cua/proto/MEDIA.md). Raw events only; decoding and metrics
//! live in the caller.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use cua_media_protocol::v2::{self, decode_server_text, encode_client_text};
use cua_media_transport::VideoDatagramReassembler;
use cua_proto::env::v1 as pb;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
pub struct Bearer(Option<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>);

impl tonic::service::Interceptor for Bearer {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> std::result::Result<tonic::Request<()>, tonic::Status> {
        if let Some(value) = &self.0 {
            request
                .metadata_mut()
                .insert("authorization", value.clone());
        }
        Ok(request)
    }
}

pub type Grpc = pb::stream_service_client::StreamServiceClient<
    tonic::service::interceptor::InterceptedService<tonic::transport::Channel, Bearer>,
>;

pub async fn grpc(url: &str, token: &str) -> Result<Grpc> {
    let channel = tonic::transport::Endpoint::from_shared(url.to_owned())?
        .connect_timeout(Duration::from_secs(10))
        .tcp_nodelay(true)
        .connect()
        .await?;
    let bearer = format!("Bearer {token}")
        .parse()
        .map_err(|_| "token is not header text")?;
    Ok(
        pb::stream_service_client::StreamServiceClient::with_interceptor(
            channel,
            Bearer(Some(bearer)),
        )
        .max_decoding_message_size(64 * 1024 * 1024),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Ws,
    Quic,
}

impl std::str::FromStr for Transport {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "ws" => Ok(Self::Ws),
            "quic" => Ok(Self::Quic),
            other => Err(format!("unknown transport {other}")),
        }
    }
}

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
    reliable: tokio::sync::mpsc::Receiver<Option<Vec<u8>>>,
    reassembler: VideoDatagramReassembler,
}

/// One raw media-plane item.
#[derive(Debug)]
pub enum Raw {
    Text(String),
    Binary(Vec<u8>),
    /// Incomplete QUIC video packets dropped by reassembly.
    Loss {
        incomplete: u64,
        _keyframe: bool,
    },
    Closed(Option<u16>),
}

/// Loss injection on the receive side (simulated network loss).
#[derive(Debug, Clone, Copy, Default)]
pub struct Impair {
    /// Fraction (0..1) of *video* QUIC datagrams discarded before reassembly.
    pub quic_video_drop: f64,
    pub seed: u64,
}

pub struct Link {
    grpc: Grpc,
    media: Media,
    pub session_id: String,
    impair: Impair,
    rng: u64,
    pub datagrams: u64,
    pub datagrams_dropped: u64,
    pub wire_bytes: u64,
    queued: VecDeque<Raw>,
}

pub struct OpenParams {
    pub target: pb::media_target::Target,
    pub transport: Transport,
    pub max_fps: u32,
    pub audio: bool,
    /// Where to dial QUIC (the advertised port is the guest's, which a
    /// published container maps elsewhere).
    pub quic_addr: Option<SocketAddr>,
    pub ws_base: String,
    pub impair: Impair,
    pub policy: pb::SessionPolicy,
}

impl Link {
    pub async fn open(mut grpc: Grpc, params: OpenParams) -> Result<Self> {
        let open = grpc
            .open_media(pb::OpenMediaRequest {
                target: Some(pb::MediaTarget {
                    target: Some(params.target),
                }),
                codecs: vec![pb::MediaCodec::H264 as i32],
                max_fps: params.max_fps,
                max_dimension: 0,
                bitrate_kbps: 0,
                policy: params.policy as i32,
                geometry_control: pb::GeometryControl::ObserveOnly as i32,
                prefer_quic: params.transport == Transport::Quic,
                audio: params.audio.then(|| pb::AudioOptions {
                    enabled: true,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await?
            .into_inner();
        let media = match params.transport {
            Transport::Quic => {
                let quic = open
                    .quic
                    .as_ref()
                    .ok_or("driver offered no QUIC endpoint (is CUA_ENV_QUIC_PORT set?)")?;
                let addr = match params.quic_addr {
                    Some(addr) => addr,
                    None => {
                        let host = url_host(&params.ws_base)?;
                        let mut addrs =
                            tokio::net::lookup_host(format!("{host}:{}", quic.port)).await?;
                        addrs.next().ok_or("QUIC host resolved to nothing")?
                    }
                };
                Media::Quic(Box::new(
                    connect_quic(addr, &quic.certificate_sha256, &open.ticket).await?,
                ))
            }
            Transport::Ws => Media::Ws(Box::new(connect_ws(&params.ws_base, &open.ticket).await?)),
        };
        Ok(Self {
            grpc,
            media,
            session_id: open.media_session_id.clone(),
            impair: params.impair,
            rng: params.impair.seed | 1,
            datagrams: 0,
            datagrams_dropped: 0,
            wire_bytes: 0,
            queued: VecDeque::new(),
        })
    }

    fn roll(&mut self) -> f64 {
        // xorshift64*: deterministic per seed.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    pub async fn send(&mut self, message: &v2::ClientMessage) -> Result<()> {
        let text = encode_client_text(message);
        match &mut self.media {
            Media::Ws(socket) => socket.send(Message::Text(text.into())).await?,
            Media::Quic(quic) => {
                cua_media_transport::quic::write_message(&mut quic.send, text.as_bytes()).await?
            }
        }
        Ok(())
    }

    /// Next raw item, or `None` on timeout.
    pub async fn next(&mut self, timeout: Duration) -> Result<Option<Raw>> {
        if let Some(raw) = self.queued.pop_front() {
            return Ok(Some(raw));
        }
        match tokio::time::timeout(timeout, self.next_raw()).await {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        }
    }

    async fn next_raw(&mut self) -> Result<Raw> {
        loop {
            match &mut self.media {
                Media::Ws(socket) => {
                    let Some(message) = socket.next().await else {
                        return Ok(Raw::Closed(None));
                    };
                    match message? {
                        Message::Text(text) => {
                            self.wire_bytes += text.len() as u64;
                            return Ok(Raw::Text(text.to_string()));
                        }
                        Message::Binary(bytes) => {
                            self.wire_bytes += bytes.len() as u64;
                            return Ok(Raw::Binary(bytes.to_vec()));
                        }
                        Message::Close(frame) => {
                            return Ok(Raw::Closed(frame.map(|f| u16::from(f.code))))
                        }
                        Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                        Message::Pong(_) | Message::Frame(_) => {}
                    }
                }
                Media::Quic(quic) => {
                    tokio::select! {
                        message = quic.reliable.recv() => {
                            return Ok(match message.flatten() {
                                Some(bytes) => {
                                    self.wire_bytes += bytes.len() as u64;
                                    if bytes.first() == Some(&b'{') {
                                        Raw::Text(String::from_utf8(bytes)?)
                                    } else {
                                        Raw::Binary(bytes)
                                    }
                                }
                                None => Raw::Closed(quic_close_code(&quic.connection)),
                            });
                        }
                        datagram = quic.connection.read_datagram() => {
                            let Ok(datagram) = datagram else {
                                return Ok(Raw::Closed(quic_close_code(&quic.connection)));
                            };
                            self.datagrams += 1;
                            self.wire_bytes += datagram.len() as u64;
                            if cua_media_transport::audio::is_audio_packet(&datagram) {
                                return Ok(Raw::Binary(datagram.to_vec()));
                            }
                            let drop = self.impair.quic_video_drop;
                            if drop > 0.0 && self.roll() < drop {
                                self.datagrams_dropped += 1;
                                continue;
                            }
                            let Media::Quic(quic) = &mut self.media else { unreachable!() };
                            let update = quic.reassembler.push(&datagram, Instant::now())?;
                            if update.dropped_incomplete > 0 {
                                if let Some(packet) = update.packet {
                                    self.queued.push_back(Raw::Binary(packet));
                                }
                                return Ok(Raw::Loss { incomplete: update.dropped_incomplete, _keyframe: update.lost_keyframe });
                            }
                            if let Some(packet) = update.packet {
                                return Ok(Raw::Binary(packet));
                            }
                        }
                    }
                }
            }
        }
    }

    pub async fn close(mut self) {
        match &mut self.media {
            Media::Ws(socket) => {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    futures_util::SinkExt::close(&mut **socket),
                )
                .await;
            }
            Media::Quic(quic) => quic.connection.close(0u32.into(), b"bench done"),
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            self.grpc.close_media(pb::CloseMediaRequest {
                media_session_id: self.session_id.clone(),
            }),
        )
        .await;
    }

    pub fn quic_rtt(&self) -> Option<Duration> {
        match &self.media {
            Media::Quic(quic) => Some(quic.connection.rtt()),
            Media::Ws(_) => None,
        }
    }
}

pub fn decode_text(text: &str) -> Option<v2::ServerMessage> {
    decode_server_text(text).ok()
}

fn url_host(url: &str) -> Result<String> {
    let parsed = url::Url::parse(url)?;
    Ok(parsed.host_str().ok_or("URL has no host")?.to_owned())
}

async fn connect_ws(base: &str, ticket: &str) -> Result<Socket> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    let parsed = url::Url::parse(base)?;
    let scheme = if parsed.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    let host = parsed.host_str().ok_or("URL has no host")?;
    let port = parsed.port_or_known_default().ok_or("URL has no port")?;
    let mut request = format!("{scheme}://{host}:{port}/media").into_client_request()?;
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("rcdp.v2, cua.ticket.{ticket}").parse()?,
    );
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| "media WebSocket connect timed out")??;
    if let tokio_tungstenite::MaybeTlsStream::Plain(stream) = socket.get_ref() {
        let _ = stream.set_nodelay(true);
    }
    Ok(socket)
}

async fn connect_quic(server: SocketAddr, pin: &str, ticket: &str) -> Result<QuicMedia> {
    let pin = cua_media_transport::quic::parse_sha256(pin)
        .ok_or("QuicEndpoint has no valid certificate_sha256")?;
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
    // Bounded: a slow consumer back-pressures the reliable stream.
    let (sender, reliable) = tokio::sync::mpsc::channel(1024);
    tokio::spawn(async move {
        loop {
            let message = cua_media_transport::quic::read_message(&mut receive)
                .await
                .ok()
                .flatten();
            let end = message.is_none();
            if sender.send(message).await.is_err() || end {
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
    })
}

fn quic_close_code(connection: &quinn::Connection) -> Option<u16> {
    match connection.close_reason() {
        Some(quinn::ConnectionError::ApplicationClosed(close)) => {
            let digits = format!("{:x}", close.error_code.into_inner());
            digits
                .parse::<u16>()
                .ok()
                .filter(|v| *v < 1000)
                .map(|v| 4000 + v)
        }
        _ => None,
    }
}
