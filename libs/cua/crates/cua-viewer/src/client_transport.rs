// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Native-client transport selection.
//!
//! WebSocket remains the compatibility path. QUIC carries lossless protocol
//! packets on one ordered bidirectional stream and reassembles replaceable
//! video from authenticated, unordered datagrams.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_media_protocol::{ClientMessage, WireHeader};
use cua_media_transport::{
    decode_packet, encode_packet, read_packet, write_packet, VideoDatagramReassembler,
};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt as _, StreamExt as _};
use quinn::crypto::rustls::QuicClientConfig;
use quinn::{ClientConfig, Endpoint, SendStream, TransportConfig};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncRead;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::ClientResult;

const QUIC_SERVER_NAME: &str = "cua-spacesd.local";
const QUIC_DATAGRAM_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const QUIC_IDLE_TIMEOUT: Duration = Duration::from_secs(6);

type NativeWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WebSocketSink = SplitSink<NativeWebSocket, Message>;
type WebSocketSource = SplitStream<NativeWebSocket>;

pub(super) enum TransportEvent {
    Packet(WireHeader, Vec<u8>),
    VideoLoss {
        incomplete_frames: u64,
        lost_keyframe: bool,
    },
    Ping(Vec<u8>),
    Closed,
}

enum ClientSink {
    WebSocket(WebSocketSink),
    Quic(SendStream),
    /// The v2 link owns both directions (see `source`).
    V2,
}

enum ClientSource {
    WebSocket(WebSocketSource),
    Quic(Box<QuicSource>),
    V2(Box<crate::v2::V2Link>),
}

struct QuicSource {
    _endpoint: Endpoint,
    connection: quinn::Connection,
    reliable: UnboundedReceiver<ReliableEvent>,
    reassembler: VideoDatagramReassembler,
    pending: VecDeque<TransportEvent>,
}

enum ReliableEvent {
    Packet(WireHeader, Vec<u8>),
    Closed,
    Error(String),
}

pub(super) struct ClientTransport {
    sink: ClientSink,
    source: ClientSource,
}

impl ClientTransport {
    pub(super) async fn connect(
        url: &str,
        certificate_sha256: Option<&str>,
        token: Option<&str>,
    ) -> ClientResult<Self> {
        if url.starts_with("http://") || url.starts_with("https://") {
            // rcdp wire v2: gRPC OpenMedia + ticketed /media or direct QUIC.
            let output: Box<dyn crate::v2::AudioOutput> =
                Box::new(crate::v2::speaker::Speaker::default());
            let link = crate::v2::V2Link::connect(url, token, Some(output))
                .await
                .map_err(|error| -> Box<dyn std::error::Error> { error })?;
            return Ok(Self {
                sink: ClientSink::V2,
                source: ClientSource::V2(Box::new(link)),
            });
        }
        if url.starts_with("quic://") {
            Self::connect_quic(url, certificate_sha256).await
        } else {
            let (socket, _) = tokio_tungstenite::connect_async(url).await?;
            let (sink, source) = socket.split();
            Ok(Self {
                sink: ClientSink::WebSocket(sink),
                source: ClientSource::WebSocket(source),
            })
        }
    }

    async fn connect_quic(url: &str, certificate_sha256: Option<&str>) -> ClientResult<Self> {
        let expected = parse_sha256(
            certificate_sha256.ok_or("QUIC requires --quic-cert-sha256 or bundled pin")?,
        )?;
        let parsed = url::Url::parse(url)?;
        if parsed.scheme() != "quic" || parsed.path() != "" && parsed.path() != "/" {
            return Err("QUIC URL must be quic://HOST:PORT".into());
        }
        let host = parsed.host_str().ok_or("QUIC URL has no host")?;
        let port = parsed.port().ok_or("QUIC URL has no port")?;
        let mut addresses = tokio::net::lookup_host((host, port)).await?;
        let server = addresses
            .next()
            .ok_or("QUIC host resolved to no addresses")?;
        let bind = match server.ip() {
            IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        };
        let mut endpoint = Endpoint::client(bind)?;
        let rustls = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(PinnedCertificate::new(expected))
            .with_no_client_auth();
        let mut config = ClientConfig::new(Arc::new(QuicClientConfig::try_from(rustls)?));
        let mut transport = TransportConfig::default();
        transport
            .max_idle_timeout(Some(QUIC_IDLE_TIMEOUT.try_into()?))
            .keep_alive_interval(Some(Duration::from_secs(2)))
            .datagram_receive_buffer_size(Some(QUIC_DATAGRAM_BUFFER_BYTES))
            .datagram_send_buffer_size(0)
            .max_concurrent_bidi_streams(1_u8.into())
            .max_concurrent_uni_streams(0_u8.into());
        config.transport_config(Arc::new(transport));
        endpoint.set_default_client_config(config);
        let connection = endpoint.connect(server, QUIC_SERVER_NAME)?.await?;
        let (send, reliable) = connection.open_bi().await?;
        Ok(Self {
            sink: ClientSink::Quic(send),
            source: ClientSource::Quic(Box::new(QuicSource {
                _endpoint: endpoint,
                connection,
                reliable: spawn_reliable_reader(reliable),
                reassembler: VideoDatagramReassembler::default(),
                pending: VecDeque::new(),
            })),
        })
    }

    pub(super) async fn send(&mut self, message: ClientMessage) -> ClientResult<()> {
        if let ClientSource::V2(link) = &mut self.source {
            return link
                .send(message)
                .await
                .map_err(|error| -> Box<dyn std::error::Error> { error });
        }
        let header = WireHeader::Client(message);
        match &mut self.sink {
            ClientSink::WebSocket(sink) => {
                sink.send(Message::Text(serde_json::to_string(&header)?))
                    .await?;
            }
            ClientSink::Quic(send) => write_packet(send, &header, &[]).await?,
            ClientSink::V2 => {}
        }
        Ok(())
    }

    pub(super) async fn send_payload(
        &mut self,
        message: ClientMessage,
        payload: &[u8],
    ) -> ClientResult<()> {
        let header = WireHeader::Client(message);
        match &mut self.sink {
            ClientSink::WebSocket(sink) => {
                sink.send(Message::Binary(encode_packet(&header, payload)?))
                    .await?;
            }
            ClientSink::Quic(send) => write_packet(send, &header, payload).await?,
            // Payload messages (file clipboard) are not offered on v2.
            ClientSink::V2 => {}
        }
        Ok(())
    }

    pub(super) async fn send_pong(&mut self, payload: Vec<u8>) -> ClientResult<()> {
        if let ClientSink::WebSocket(sink) = &mut self.sink {
            sink.send(Message::Pong(payload)).await?;
        }
        Ok(())
    }

    pub(super) fn transport_rtt(&self) -> Option<Duration> {
        match &self.source {
            ClientSource::WebSocket(_) => None,
            ClientSource::Quic(source) => Some(source.connection.rtt()),
            ClientSource::V2(link) => link.rtt(),
        }
    }

    pub(super) async fn next(&mut self) -> ClientResult<TransportEvent> {
        match &mut self.source {
            ClientSource::V2(link) => {
                let event = link
                    .next()
                    .await
                    .map_err(|error| -> Box<dyn std::error::Error> { error })?;
                Ok(match event {
                    crate::v2::V2Event::Server(message) => {
                        TransportEvent::Packet(WireHeader::Server(message), Vec::new())
                    }
                    crate::v2::V2Event::Packet(header, payload) => {
                        TransportEvent::Packet(header, payload)
                    }
                    crate::v2::V2Event::VideoLoss {
                        incomplete_frames,
                        lost_keyframe,
                    } => TransportEvent::VideoLoss {
                        incomplete_frames,
                        lost_keyframe,
                    },
                    crate::v2::V2Event::Closed(code) => {
                        tracing::info!(close_code = ?code, "media socket closed");
                        TransportEvent::Closed
                    }
                })
            }
            ClientSource::WebSocket(source) => loop {
                let Some(message) = source.next().await else {
                    return Ok(TransportEvent::Closed);
                };
                match message? {
                    Message::Text(text) => {
                        return Ok(TransportEvent::Packet(
                            serde_json::from_str(&text)?,
                            Vec::new(),
                        ));
                    }
                    Message::Binary(packet) => {
                        let (header, payload) = decode_packet(&packet)?;
                        return Ok(TransportEvent::Packet(header, payload));
                    }
                    Message::Ping(payload) => return Ok(TransportEvent::Ping(payload)),
                    Message::Close(_) => return Ok(TransportEvent::Closed),
                    Message::Pong(_) | Message::Frame(_) => {}
                }
            },
            ClientSource::Quic(source) => {
                if let Some(event) = source.pending.pop_front() {
                    return Ok(event);
                }
                loop {
                    tokio::select! {
                        packet = source.reliable.recv() => {
                            return match packet {
                                Some(ReliableEvent::Packet(header, payload)) => {
                                    Ok(TransportEvent::Packet(header, payload))
                                }
                                Some(ReliableEvent::Closed) | None => Ok(TransportEvent::Closed),
                                Some(ReliableEvent::Error(error)) => Err(error.into()),
                            };
                        }
                        datagram = source.connection.read_datagram() => {
                            let datagram = datagram?;
                            let update = source.reassembler.push(&datagram, Instant::now())?;
                            if let Some(packet) = update.packet {
                                let (header, payload) = decode_packet(&packet)?;
                                source.pending.push_back(TransportEvent::Packet(header, payload));
                            }
                            if update.dropped_incomplete > 0 {
                                return Ok(TransportEvent::VideoLoss {
                                    incomplete_frames: update.dropped_incomplete,
                                    lost_keyframe: update.lost_keyframe,
                                });
                            }
                            if let Some(event) = source.pending.pop_front() {
                                return Ok(event);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn spawn_reliable_reader<R>(mut reliable: R) -> UnboundedReceiver<ReliableEvent>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let (sender, receiver) = unbounded_channel();
    tokio::spawn(async move {
        loop {
            let event = match read_packet(&mut reliable).await {
                Ok(Some((header, payload))) => ReliableEvent::Packet(header, payload),
                Ok(None) => ReliableEvent::Closed,
                Err(error) => ReliableEvent::Error(error.to_string()),
            };
            let terminal = matches!(event, ReliableEvent::Closed | ReliableEvent::Error(_));
            if sender.send(event).is_err() || terminal {
                break;
            }
        }
    });
    receiver
}

fn parse_sha256(value: &str) -> ClientResult<[u8; 32]> {
    let value = value.trim();
    if value.len() != 64 {
        return Err("QUIC certificate SHA-256 must contain 64 hexadecimal characters".into());
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let pair = std::str::from_utf8(pair)?;
        output[index] = u8::from_str_radix(pair, 16)
            .map_err(|_| "QUIC certificate SHA-256 contains non-hexadecimal characters")?;
    }
    Ok(output)
}

#[derive(Debug)]
struct PinnedCertificate {
    expected_sha256: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl PinnedCertificate {
    fn new(expected_sha256: [u8; 32]) -> Arc<Self> {
        Arc::new(Self {
            expected_sha256,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        })
    }
}

impl rustls::client::danger::ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let actual = Sha256::digest(end_entity.as_ref());
        if actual.as_slice() != self.expected_sha256 {
            return Err(rustls::Error::General(
                "RCDP QUIC certificate fingerprint mismatch".into(),
            ));
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_media_protocol::ServerMessage;
    use cua_media_transport::encode_packet;
    use tokio::io::AsyncWriteExt as _;

    #[test]
    fn certificate_pin_parser_is_strict() {
        let pin = "0123456789abcdef".repeat(4);
        assert_eq!(parse_sha256(&pin).unwrap()[0], 0x01);
        assert!(parse_sha256("01").is_err());
        assert!(parse_sha256(&"z".repeat(64)).is_err());
    }

    #[tokio::test]
    async fn reliable_reader_keeps_framing_when_consumer_wait_is_cancelled() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let mut receiver = spawn_reliable_reader(reader);
        let packet = encode_packet(&WireHeader::Server(ServerMessage::Authenticated), &[]).unwrap();
        let split = packet.len() / 2;
        writer.write_all(&packet[..split]).await.unwrap();

        tokio::select! {
            biased;
            _ = tokio::task::yield_now() => {}
            _ = receiver.recv() => panic!("partial packet must not be emitted"),
        }

        writer.write_all(&packet[split..]).await.unwrap();
        let Some(ReliableEvent::Packet(header, payload)) = receiver.recv().await else {
            panic!("complete reliable packet was not emitted");
        };
        assert_eq!(header, WireHeader::Server(ServerMessage::Authenticated));
        assert!(payload.is_empty());
    }
}
