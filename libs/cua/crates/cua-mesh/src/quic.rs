// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Streams between peers: QUIC (quinn) over a virtual mesh socket.
//!
//! WireGuard authenticates and encrypts every packet; QUIC on top gives
//! reliable, congestion-controlled, multiplexed streams that survive path
//! changes underneath (direct to relay and back, a new NAT mapping) because
//! the QUIC connection lives between two mesh addresses that never change.
//! Each stream starts with a small header naming the service it wants.
//! Each peer's QUIC certificate is pinned by the SHA-256 the coordinator
//! distributes with its node key.

use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::key::NodeIdentity;
use crate::packet::MAX_UDP_PAYLOAD;
use crate::vnet::VSocket;
use crate::{Error, Result};

/// The mesh port every node's stream endpoint listens on.
pub const STREAM_PORT: u16 = 7443;
/// ALPN of mesh streams.
pub const ALPN: &[u8] = b"cua-mesh/1";
const MAGIC: &[u8; 4] = b"CMS1";
const SERVER_NAME: &str = "cua-mesh";

/// What a stream asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StreamHeader {
    /// The service (`spacesd`, ...).
    pub service: String,
    /// The dialer's per-forward session key, if any (see the crate docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

fn transport() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(10)));
    // Long enough to ride out a relay or coordinator restart or a network
    // change; the WireGuard and disco layers recover underneath.
    t.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(Duration::from_secs(120)).expect("valid timeout"),
    ));
    t.initial_mtu(1200);
    let mut mtu = quinn::MtuDiscoveryConfig::default();
    mtu.upper_bound(MAX_UDP_PAYLOAD as u16);
    t.mtu_discovery_config(Some(mtu));
    t.stream_receive_window(quinn::VarInt::from_u32(16 << 20));
    t.receive_window(quinn::VarInt::from_u32(64 << 20));
    t.send_window(64 << 20);
    t.max_concurrent_bidi_streams(quinn::VarInt::from_u32(2048));
    t.max_concurrent_uni_streams(quinn::VarInt::from_u32(0));
    t
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A QUIC endpoint on the mesh.
pub(crate) fn endpoint(identity: &NodeIdentity, socket: Arc<VSocket>) -> Result<quinn::Endpoint> {
    let cert = CertificateDer::from(identity.cert_der.clone());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.cert_key_der.clone()));
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| Error::Io(format!("tls: {e}")))?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| Error::Io(format!("tls: {e}")))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.max_early_data_size = 0;
    let crypto =
        QuicServerConfig::try_from(tls).map_err(|e| Error::Io(format!("quic tls: {e}")))?;
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    server.transport_config(Arc::new(transport()));
    quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(server),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .map_err(|e| Error::Io(format!("quic endpoint: {e}")))
}

/// Client config pinning the peer's certificate SHA-256 (hex).
pub(crate) fn client_config(pin: &str) -> Result<quinn::ClientConfig> {
    let pin = hex::decode(pin).map_err(|_| Error::Protocol("bad certificate pin".into()))?;
    let verifier = Arc::new(PinVerifier {
        pin,
        provider: provider(),
    });
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| Error::Io(format!("tls: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto =
        QuicClientConfig::try_from(tls).map_err(|e| Error::Io(format!("quic tls: {e}")))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(Arc::new(transport()));
    Ok(cfg)
}

/// Connects to `peer`'s stream endpoint.
pub(crate) async fn connect(
    endpoint: &quinn::Endpoint,
    peer: Ipv6Addr,
    pin: &str,
) -> Result<quinn::Connection> {
    let cfg = client_config(pin)?;
    let addr = SocketAddr::V6(SocketAddrV6::new(peer, STREAM_PORT, 0, 0));
    endpoint
        .connect_with(cfg, addr, SERVER_NAME)
        .map_err(|e| Error::Unreachable(format!("quic connect: {e}")))?
        .await
        .map_err(|e| Error::Unreachable(format!("quic connect: {e}")))
}

/// Writes a stream header.
pub(crate) async fn write_header(send: &mut quinn::SendStream, h: &StreamHeader) -> Result<()> {
    let json = serde_json::to_vec(h).map_err(|e| Error::Protocol(e.to_string()))?;
    let mut buf = Vec::with_capacity(6 + json.len());
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&(json.len() as u16).to_be_bytes());
    buf.extend_from_slice(&json);
    send.write_all(&buf)
        .await
        .map_err(|e| Error::Unreachable(e.to_string()))
}

/// Reads a stream header (at most 4 KiB).
pub(crate) async fn read_header(recv: &mut quinn::RecvStream) -> Result<StreamHeader> {
    let mut head = [0u8; 6];
    recv.read_exact(&mut head)
        .await
        .map_err(|e| Error::Protocol(format!("stream header: {e}")))?;
    if &head[..4] != MAGIC {
        return Err(Error::Protocol("not a mesh stream".into()));
    }
    let len = u16::from_be_bytes([head[4], head[5]]) as usize;
    if len > 4096 {
        return Err(Error::Protocol("stream header too large".into()));
    }
    let mut json = vec![0u8; len];
    recv.read_exact(&mut json)
        .await
        .map_err(|e| Error::Protocol(format!("stream header: {e}")))?;
    serde_json::from_slice(&json).map_err(|e| Error::Protocol(format!("stream header: {e}")))
}

/// Certificate pinning: the end-entity certificate's SHA-256 must match;
/// the handshake signature is still verified with the certificate's key.
#[derive(Debug)]
struct PinVerifier {
    pin: Vec<u8>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let got = Sha256::digest(end_entity.as_ref());
        if got.as_slice() == self.pin.as_slice() {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Bidirectional stream as one `AsyncRead + AsyncWrite`.
pub type MeshStream = tokio::io::Join<quinn::RecvStream, quinn::SendStream>;

/// Joins a QUIC stream's halves.
pub fn join(send: quinn::SendStream, recv: quinn::RecvStream) -> MeshStream {
    tokio::io::join(recv, send)
}

/// Copies between a local TCP connection and a mesh stream until both
/// sides finish.
pub(crate) async fn splice(mut tcp: tokio::net::TcpStream, mut stream: MeshStream) {
    let _ = tcp.set_nodelay(true);
    let _ =
        tokio::io::copy_bidirectional_with_sizes(&mut tcp, &mut stream, 64 << 10, 64 << 10).await;
    let _ = stream.shutdown().await;
}
