// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Direct QUIC media binding of rcdp wire v2 (MEDIA.md §2, §9, §12.6).
//!
//! - ALPN `rcdp/2`. The client pins the server's self-signed certificate by
//!   its SHA-256 (`QuicEndpoint.certificate_sha256` from `OpenMedia`).
//! - One client-opened bidirectional **reliable stream** carries messages
//!   framed as `u32` big-endian length + the message bytes. A message is
//!   exactly what a WebSocket frame would carry: a JSON control message
//!   (starts with `{`), or a binary audio packet (`RAU2`) too large for a
//!   datagram. The first client message must be
//!   `{"type":"ticket","payload":{"ticket":"…"}}`.
//! - **Datagrams** carry video as `RVD2` fragments of the length-prefixed
//!   video packet, and audio as whole `RAU2` packets. Audio is sent first
//!   and video yields datagram buffer space to it.
//! - Application close codes mirror the WebSocket close codes
//!   (`v2::close_code::quic_error`).

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_media_protocol::v2::{encode_client_text, ClientMessage};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// Largest reliable-stream message accepted (control headers are ≤ 1 MiB;
/// oversize audio packets are far smaller).
pub const MAX_STREAM_MESSAGE_BYTES: usize = 2 * 1024 * 1024;

/// Write one length-delimited message.
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &[u8],
) -> std::io::Result<()> {
    if message.len() > MAX_STREAM_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "message too large",
        ));
    }
    writer.write_u32(message.len() as u32).await?;
    writer.write_all(message).await?;
    writer.flush().await
}

/// Read one length-delimited message; `Ok(None)` at a clean end of stream.
pub async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Vec<u8>>> {
    let length = match reader.read_u32().await {
        Ok(length) => length as usize,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    };
    if length > MAX_STREAM_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut message = vec![0; length];
    reader.read_exact(&mut message).await?;
    Ok(Some(message))
}

/// Lowercase hex SHA-256 of a DER certificate.
pub fn certificate_sha256(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Parse a 64-character hex SHA-256 pin.
pub fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    let value = value.trim();
    if value.len() != 64 {
        return None;
    }
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(value.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(output)
}

/// A self-signed server identity (in memory; a new one per process).
pub struct QuicIdentity {
    pub certificate_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
}

impl QuicIdentity {
    pub fn generate() -> Result<Self, String> {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["cua-spacesd.local".into()])
                .map_err(|error| error.to_string())?;
        Ok(Self {
            certificate_der: cert.der().to_vec(),
            private_key_der: signing_key.serialize_der(),
        })
    }

    pub fn certificate_sha256(&self) -> String {
        certificate_sha256(&self.certificate_der)
    }

    /// Server config: ALPN `rcdp/2`, datagrams on, one bidi stream.
    pub fn server_config(&self) -> Result<quinn::ServerConfig, String> {
        self.server_config_with_alpns(&[cua_media_protocol::v2::QUIC_ALPN])
    }

    /// [`Self::server_config`] offering several ALPN protocols (in
    /// preference order), for a listener that also serves the presence
    /// channel (`cua-presence/1`). The negotiated one is in the connection's
    /// handshake data.
    pub fn server_config_with_alpns(&self, alpns: &[&[u8]]) -> Result<quinn::ServerConfig, String> {
        let certificate = CertificateDer::from(self.certificate_der.clone());
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(self.private_key_der.clone());
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key.into())
        .map_err(|error| error.to_string())?;
        tls.alpn_protocols = alpns.iter().map(|alpn| alpn.to_vec()).collect();
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
            .map_err(|error| error.to_string())?;
        let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        let transport = Arc::get_mut(&mut config.transport).ok_or("shared transport config")?;
        transport
            .max_idle_timeout(Some(
                std::time::Duration::from_secs(60)
                    .try_into()
                    .expect("idle timeout"),
            ))
            .keep_alive_interval(Some(std::time::Duration::from_secs(5)))
            .datagram_receive_buffer_size(Some(4 * 1024 * 1024))
            .datagram_send_buffer_size(4 * 1024 * 1024)
            .max_concurrent_bidi_streams(1_u8.into())
            .max_concurrent_uni_streams(0_u8.into());
        Ok(config)
    }
}

/// Client config that accepts exactly the pinned certificate.
pub fn client_config(pin: [u8; 32]) -> Result<quinn::ClientConfig, String> {
    client_config_with_alpn(pin, cua_media_protocol::v2::QUIC_ALPN)
}

/// [`client_config`] for another ALPN on the same listener (for example
/// `cua_media_protocol::presence::PRESENCE_ALPN`).
pub fn client_config_with_alpn(pin: [u8; 32], alpn: &[u8]) -> Result<quinn::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { pin, provider }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![alpn.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|error| error.to_string())?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    transport
        .datagram_receive_buffer_size(Some(4 * 1024 * 1024))
        .keep_alive_interval(Some(std::time::Duration::from_secs(5)));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// One input from a [`MediaQuic`] connection, in the shape a WebSocket
/// carries it.
#[derive(Debug, PartialEq, Eq)]
pub enum QuicItem {
    /// A JSON control message.
    Text(String),
    /// A whole binary message: a reassembled length-prefixed video packet,
    /// or a `RAU2` audio packet.
    Binary(Vec<u8>),
    /// Video packets that lost fragments (the client should resync on a
    /// keyframe).
    Loss {
        incomplete_frames: u64,
        lost_keyframe: bool,
    },
    /// The connection ended, with the WebSocket close code its application
    /// close mirrors when it had one.
    Closed(Option<u16>),
}

/// A client attached to a direct QUIC media listener: the ticket is sent,
/// the reliable stream and datagrams are read as [`QuicItem`]s.
pub struct MediaQuic {
    _endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    send: quinn::SendStream,
    reliable: tokio::sync::mpsc::UnboundedReceiver<Option<Vec<u8>>>,
    reassembler: crate::VideoDatagramReassembler,
    pending: VecDeque<Vec<u8>>,
}

impl MediaQuic {
    /// Connects to `host:port`, pinning the certificate to `pin` (the
    /// `QuicEndpoint.certificate_sha256` hex), and sends `ticket`. The
    /// handshake must finish within `timeout`.
    pub async fn connect(
        host: &str,
        port: u16,
        pin: &str,
        ticket: &str,
        timeout: Duration,
    ) -> Result<Self, String> {
        let pin = parse_sha256(pin).ok_or("QuicEndpoint has no valid certificate_sha256")?;
        let server: SocketAddr = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| format!("resolving {host}: {error}"))?
            .next()
            .ok_or("QUIC host resolved to no addresses")?;
        let bind: SocketAddr = if server.is_ipv4() {
            (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let mut endpoint = quinn::Endpoint::client(bind).map_err(|error| error.to_string())?;
        endpoint.set_default_client_config(client_config(pin)?);
        let connecting = endpoint
            .connect(server, "cua-spacesd.local")
            .map_err(|error| error.to_string())?;
        let connection = tokio::time::timeout(timeout, connecting)
            .await
            .map_err(|_| "QUIC connect timed out")?
            .map_err(|error| error.to_string())?;
        let (mut send, mut receive) = connection
            .open_bi()
            .await
            .map_err(|error| error.to_string())?;
        let ticket = encode_client_text(&ClientMessage::Ticket {
            ticket: ticket.to_owned(),
        });
        write_message(&mut send, ticket.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        let (sender, reliable) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let message = read_message(&mut receive).await.ok().flatten();
                let end = message.is_none();
                if sender.send(message).is_err() || end {
                    return;
                }
            }
        });
        Ok(Self {
            _endpoint: endpoint,
            connection,
            send,
            reliable,
            reassembler: crate::VideoDatagramReassembler::default(),
            pending: VecDeque::new(),
        })
    }

    /// The next input. Cancel-safe: a dropped call loses nothing.
    pub async fn next(&mut self) -> Result<QuicItem, String> {
        if let Some(packet) = self.pending.pop_front() {
            return Ok(QuicItem::Binary(packet));
        }
        loop {
            tokio::select! {
                message = self.reliable.recv() => {
                    return match message.flatten() {
                        Some(bytes) if bytes.first() == Some(&b'{') => String::from_utf8(bytes)
                            .map(QuicItem::Text)
                            .map_err(|error| error.to_string()),
                        Some(bytes) => Ok(QuicItem::Binary(bytes)),
                        None => Ok(QuicItem::Closed(self.close_code())),
                    };
                }
                datagram = self.connection.read_datagram() => {
                    let Ok(datagram) = datagram else {
                        return Ok(QuicItem::Closed(self.close_code()));
                    };
                    if crate::audio::is_audio_packet(&datagram) {
                        return Ok(QuicItem::Binary(datagram.to_vec()));
                    }
                    let update = self
                        .reassembler
                        .push(&datagram, Instant::now())
                        .map_err(|error| error.to_string())?;
                    if let Some(packet) = update.packet {
                        self.pending.push_back(packet);
                    }
                    if update.dropped_incomplete > 0 {
                        return Ok(QuicItem::Loss {
                            incomplete_frames: update.dropped_incomplete,
                            lost_keyframe: update.lost_keyframe,
                        });
                    }
                    if let Some(packet) = self.pending.pop_front() {
                        return Ok(QuicItem::Binary(packet));
                    }
                }
            }
        }
    }

    /// Sends one client control message on the reliable stream.
    pub async fn send_text(&mut self, text: &str) -> std::io::Result<()> {
        write_message(&mut self.send, text.as_bytes()).await
    }

    /// The connection's current round-trip estimate.
    pub fn rtt(&self) -> Duration {
        self.connection.rtt()
    }

    /// Closes the connection.
    pub fn close(&self) {
        self.connection.close(0u32.into(), b"client closed");
    }

    fn close_code(&self) -> Option<u16> {
        match self.connection.close_reason() {
            Some(quinn::ConnectionError::ApplicationClosed(close)) => {
                // 0x401 -> 4401: the hex digits read as decimal after "4".
                let digits = format!("{:x}", close.error_code.into_inner());
                digits
                    .parse::<u16>()
                    .ok()
                    .filter(|value| *value < 1000)
                    .map(|value| 4000 + value)
            }
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Pinned {
    pin: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if Sha256::digest(end_entity.as_ref()).as_slice() != self.pin {
            return Err(rustls::Error::General(
                "QUIC media certificate fingerprint mismatch".into(),
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

    #[tokio::test]
    async fn messages_round_trip_and_oversize_is_refused() {
        let (mut writer, mut reader) = tokio::io::duplex(64 * 1024);
        write_message(&mut writer, b"{\"type\":\"ping\"}")
            .await
            .unwrap();
        write_message(&mut writer, b"RAU2....").await.unwrap();
        drop(writer);
        assert_eq!(
            read_message(&mut reader).await.unwrap().unwrap(),
            b"{\"type\":\"ping\"}"
        );
        assert_eq!(
            read_message(&mut reader).await.unwrap().unwrap(),
            b"RAU2...."
        );
        assert!(read_message(&mut reader).await.unwrap().is_none());
        let mut sink = tokio::io::sink();
        assert!(
            write_message(&mut sink, &vec![0; MAX_STREAM_MESSAGE_BYTES + 1])
                .await
                .is_err()
        );
    }

    #[test]
    fn pins_parse_strictly() {
        let identity = QuicIdentity::generate().unwrap();
        let pin = identity.certificate_sha256();
        assert_eq!(parse_sha256(&pin).unwrap().len(), 32);
        assert!(parse_sha256("abc").is_none());
        assert!(parse_sha256(&"zz".repeat(32)).is_none());
    }
}
