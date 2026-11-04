// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! QUIC binding with reliable control streams and replaceable video datagrams.

use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cua_media_protocol::{ClientMessage, ServerErrorCode, ServerMessage, WireHeader};
use cua_media_transport::{
    encode_packet, fragment_video_packet, read_packet, write_packet, MAX_PAYLOAD_BYTES,
};
use cua_spacesd_session::{Connection, OutboundPacket};
use quinn::{Connection as QuicConnection, Endpoint, RecvStream, SendStream, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncRead;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::ws::WsDaemon;

const QUIC_DATAGRAM_BUFFER_BYTES: usize = 8 * 1024 * 1024;
// Clipboard polling and keepalives provide traffic even when the remote
// window is visually idle. Retire a genuinely unreachable connection quickly
// so its input and geometry leases cannot block an automatic client reconnect.
const QUIC_IDLE_TIMEOUT: Duration = Duration::from_secs(6);

static NEXT_QUIC_CONNECTION: AtomicU64 = AtomicU64::new(1);

enum ReliableEvent {
    Packet(WireHeader, Vec<u8>),
    Closed,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct QuicIdentity {
    certificate_der: Vec<u8>,
    private_key_der: Vec<u8>,
}

impl QuicIdentity {
    pub fn load_or_generate(directory: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let certificate_path = directory.join("certificate.der");
        let private_key_path = directory.join("private-key.der");
        match (certificate_path.is_file(), private_key_path.is_file()) {
            (true, true) => Ok(Self {
                certificate_der: std::fs::read(certificate_path)?,
                private_key_der: std::fs::read(private_key_path)?,
            }),
            (false, false) => {
                std::fs::create_dir_all(directory)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
                }
                let rcgen::CertifiedKey { cert, signing_key } =
                    rcgen::generate_simple_self_signed(vec!["cua-spacesd.local".into()])?;
                let certificate_der = cert.der().to_vec();
                let private_key_der = signing_key.serialize_der();
                std::fs::write(&certificate_path, &certificate_der)?;
                std::fs::write(&private_key_path, &private_key_der)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    std::fs::set_permissions(
                        &private_key_path,
                        std::fs::Permissions::from_mode(0o600),
                    )?;
                }
                Ok(Self {
                    certificate_der,
                    private_key_der,
                })
            }
            _ => Err(format!(
                "QUIC identity at {} is incomplete; preserve or remove both identity files",
                directory.display()
            )
            .into()),
        }
    }

    pub fn certificate_sha256(&self) -> String {
        Sha256::digest(&self.certificate_der)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn server_config(&self) -> Result<ServerConfig, Box<dyn Error + Send + Sync>> {
        let certificate = CertificateDer::from(self.certificate_der.clone());
        let private_key = PrivatePkcs8KeyDer::from(self.private_key_der.clone());
        let mut config = ServerConfig::with_single_cert(vec![certificate], private_key.into())?;
        let transport = Arc::get_mut(&mut config.transport)
            .ok_or("new QUIC server transport config is unexpectedly shared")?;
        transport
            .max_idle_timeout(Some(QUIC_IDLE_TIMEOUT.try_into()?))
            .keep_alive_interval(Some(Duration::from_secs(2)))
            .datagram_receive_buffer_size(Some(QUIC_DATAGRAM_BUFFER_BYTES))
            .datagram_send_buffer_size(QUIC_DATAGRAM_BUFFER_BYTES)
            .max_concurrent_bidi_streams(1_u8.into())
            .max_concurrent_uni_streams(0_u8.into());
        Ok(config)
    }
}

pub async fn serve_quic(
    address: SocketAddr,
    daemon: Arc<WsDaemon>,
    identity_directory: PathBuf,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !address.ip().is_loopback() && daemon.token.as_deref().is_none_or(str::is_empty) {
        return Err("a non-loopback QUIC listener requires an authentication token".into());
    }
    let identity = QuicIdentity::load_or_generate(&identity_directory)?;
    let fingerprint = identity.certificate_sha256();
    let endpoint = Endpoint::server(identity.server_config()?, address)?;
    tracing::info!(%address, certificate_sha256 = %fingerprint, "RCDP QUIC listener ready");
    eprintln!("cua-spacesd QUIC certificate SHA-256 {fingerprint}");
    while let Some(incoming) = endpoint.accept().await {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let result = async {
                let quic = incoming.await?;
                let peer = quic.remote_address();
                let connection_id = NEXT_QUIC_CONNECTION.fetch_add(1, Ordering::Relaxed);
                let (send, receive) = quic.accept_bi().await?;
                serve_quic_connection(quic, send, receive, connection_id, daemon.clone()).await?;
                Result::<_, Box<dyn Error + Send + Sync>>::Ok(peer)
            }
            .await;
            match result {
                Ok(peer) => tracing::debug!(%peer, "RCDP QUIC connection ended"),
                Err(error) => tracing::debug!(%error, "RCDP QUIC connection ended"),
            }
        });
    }
    Ok(())
}

async fn serve_quic_connection(
    quic: QuicConnection,
    mut send: SendStream,
    receive: RecvStream,
    connection_id: u64,
    daemon: Arc<WsDaemon>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut session: Connection = daemon.runtime.connect();
    let mut presence_rx = daemon.presence.register(connection_id);
    let mut authenticated = daemon.token.is_none();
    let outbound_ready = session.outbound_ready();
    let mut deadline_tick = tokio::time::interval(Duration::from_millis(50));
    deadline_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut video_packet_id = 0_u64;
    let mut reliable = spawn_reliable_reader(receive);

    if authenticated {
        write_packet(
            &mut send,
            &WireHeader::Server(ServerMessage::Authenticated),
            &[],
        )
        .await?;
    }

    // Capture every `?` from the transport loop so presence is always
    // unregistered before the connection task returns.
    let result: Result<(), Box<dyn Error + Send + Sync>> = async {
        loop {
            tokio::select! {
            biased;
            incoming = reliable.recv() => {
                let (header, payload) = match incoming {
                    Some(ReliableEvent::Packet(header, payload)) => (header, payload),
                    Some(ReliableEvent::Closed) | None => break Ok(()),
                    Some(ReliableEvent::Error(error)) => break Err(error.into()),
                };
                let WireHeader::Client(message) = header else {
                    break Err("client sent a non-client packet".into());
                };
                if let ClientMessage::Authenticate { token } = &message {
                    if daemon.token.as_deref().is_none_or(|expected| expected == token) {
                        authenticated = true;
                        write_packet(
                            &mut send,
                            &WireHeader::Server(ServerMessage::Authenticated),
                            &[],
                        ).await?;
                    } else {
                        write_packet(
                            &mut send,
                            &WireHeader::Server(ServerMessage::Error {
                                code: ServerErrorCode::HelloRequired,
                                message: "invalid authentication token".into(),
                            }),
                            &[],
                        ).await?;
                        break Ok(());
                    }
                    continue;
                }
                if !authenticated {
                    write_packet(
                        &mut send,
                        &WireHeader::Server(ServerMessage::Error {
                            code: ServerErrorCode::HelloRequired,
                            message: "authenticate before other messages".into(),
                        }),
                        &[],
                    ).await?;
                    break Ok(());
                }
                if let Some(responses) = daemon.presence.handle(connection_id, &message) {
                    for response in responses {
                        write_packet(&mut send, &WireHeader::Server(response), &[]).await?;
                    }
                    continue;
                }
                write_outbound(
                    &quic,
                    &mut send,
                    session.handle_packet(message, payload).await,
                    &mut video_packet_id,
                ).await?;
            }
            presence_message = presence_rx.recv() => {
                if let Some(message) = presence_message {
                    write_packet(&mut send, &WireHeader::Server(message), &[]).await?;
                }
            }
            _ = outbound_ready.notified() => {
                write_outbound(
                    &quic,
                    &mut send,
                    session.drain_outbound(),
                    &mut video_packet_id,
                ).await?;
            }
            _ = deadline_tick.tick() => {
                write_outbound(
                    &quic,
                    &mut send,
                    session.drain_outbound(),
                    &mut video_packet_id,
                ).await?;
            }
            }
        }
    }
    .await;
    daemon.presence.unregister(connection_id);
    result
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

async fn write_outbound(
    quic: &QuicConnection,
    send: &mut SendStream,
    packets: Vec<OutboundPacket>,
    video_packet_id: &mut u64,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    for packet in packets {
        match packet {
            OutboundPacket::Control(message) => {
                write_packet(send, &WireHeader::Server(message), &[]).await?;
            }
            OutboundPacket::AppIcon {
                descriptor,
                payload,
            } => {
                write_packet(send, &WireHeader::AppIcon(descriptor), &payload).await?;
            }
            OutboundPacket::ClipboardFiles { message, payload } => {
                write_packet(send, &WireHeader::Server(message), &payload).await?;
            }
            OutboundPacket::Video {
                descriptor,
                payload,
            } => {
                let Some(max_datagram_size) = quic.max_datagram_size() else {
                    return Err("peer did not enable QUIC datagrams".into());
                };
                *video_packet_id = video_packet_id.saturating_add(1);
                let keyframe = descriptor.keyframe;
                let packet = encode_packet(&WireHeader::Video(descriptor), &payload)?;
                if packet.len() > MAX_PAYLOAD_BYTES + 1024 * 1024 + 8 {
                    return Err("encoded QUIC video packet exceeds the transport limit".into());
                }
                let fragments =
                    fragment_video_packet(*video_packet_id, keyframe, &packet, max_datagram_size)?;
                for fragment in fragments {
                    // `send_datagram` is deliberately non-waiting: Quinn may
                    // evict older queued datagrams so current video never
                    // waits behind obsolete media.
                    quic.send_datagram(Bytes::from(fragment))?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_media_transport::encode_packet;
    use tokio::io::AsyncWriteExt as _;

    #[test]
    fn generated_quic_identity_is_stable_and_pinned() {
        let directory =
            std::env::temp_dir().join(format!("cua-env-quic-identity-{}", uuid::Uuid::new_v4()));
        let first = QuicIdentity::load_or_generate(&directory).unwrap();
        let second = QuicIdentity::load_or_generate(&directory).unwrap();
        assert_eq!(first.certificate_sha256(), second.certificate_sha256());
        assert_eq!(first.certificate_sha256().len(), 64);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn reliable_reader_keeps_partial_packet_across_other_wakeups() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let mut receiver = spawn_reliable_reader(reader);
        let packet = encode_packet(
            &WireHeader::Client(ClientMessage::ListWindows {
                on_screen_only: true,
            }),
            &[],
        )
        .unwrap();
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
        assert_eq!(
            header,
            WireHeader::Client(ClientMessage::ListWindows {
                on_screen_only: true,
            })
        );
        assert!(payload.is_empty());
    }
}
