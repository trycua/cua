// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Direct QUIC media listener (rcdp wire v2 over quinn, default UDP 3212).
//!
//! Framing and identity live in `cua_media_transport::quic`. Per connection:
//! the client opens one bidirectional stream and sends the ticket first;
//! the server answers with the same greeting as the WebSocket and then
//! streams control messages on the stream, audio as whole RAU2 datagrams and
//! video as RVD2 fragments. Audio has priority: a video packet is dropped
//! (the client sees a packet-id gap and asks for a keyframe) rather than
//! queued when the datagram buffer could no longer take the next audio.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use cua_media_protocol::v2::{
    self, close_code, decode_client_text, encode_server_text, ClientMessage, ErrorCode,
    ServerMessage,
};
use cua_media_transport::quic::{read_message, write_message, QuicIdentity};
use cua_spacesd_server::TicketScope;
use cua_spacesd_session::media::Outbound;

use super::DesktopState;

/// Datagram buffer kept free for audio before video may be queued.
const AUDIO_RESERVE_BYTES: usize = 64 * 1024;

/// A running QUIC media listener.
#[derive(Clone)]
pub struct QuicMedia {
    pub port: u16,
    pub certificate_sha256: String,
    endpoint: quinn::Endpoint,
    pub(crate) video_dropped: Arc<AtomicU64>,
}

impl QuicMedia {
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.endpoint.local_addr().ok()
    }

    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"shutdown");
    }

    /// Video packets dropped to keep room for audio (all connections).
    pub fn video_packets_dropped(&self) -> u64 {
        self.video_dropped.load(Ordering::Relaxed)
    }
}

pub(crate) fn start(state: Arc<DesktopState>, address: SocketAddr) -> Result<QuicMedia, String> {
    let socket = std::net::UdpSocket::bind(address)
        .map_err(|error| format!("binding UDP {address}: {error}"))?;
    start_on(state, socket)
}

/// Start on an already-bound UDP socket (the driver binds it before it
/// publishes its config so `media_quic_port` is the real port).
pub(crate) fn start_on(
    state: Arc<DesktopState>,
    socket: std::net::UdpSocket,
) -> Result<QuicMedia, String> {
    let identity = QuicIdentity::generate()?;
    socket
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let runtime = quinn::default_runtime().ok_or("no async runtime for QUIC")?;
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        // The presence datagram channel shares the port and certificate.
        Some(identity.server_config_with_alpns(&[
            cua_media_protocol::v2::QUIC_ALPN,
            cua_media_protocol::presence::PRESENCE_ALPN.as_bytes(),
        ])?),
        socket,
        runtime,
    )
    .map_err(|error| error.to_string())?;
    let port = endpoint
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let media = QuicMedia {
        port,
        certificate_sha256: identity.certificate_sha256(),
        endpoint: endpoint.clone(),
        video_dropped: Arc::new(AtomicU64::new(0)),
    };
    let dropped = media.video_dropped.clone();
    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            let state = state.clone();
            let dropped = dropped.clone();
            tokio::spawn(async move {
                let Ok(connection) = incoming.await else {
                    return;
                };
                let presence = connection
                    .handshake_data()
                    .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
                    .and_then(|data| data.protocol)
                    .is_some_and(|alpn| {
                        alpn == cua_media_protocol::presence::PRESENCE_ALPN.as_bytes()
                    });
                if presence {
                    if let Err(error) =
                        super::presence_quic::serve(state.presence.clone(), connection).await
                    {
                        tracing::debug!(target: "cua_spacesd_client::media_quic", %error, "QUIC presence connection ended");
                    }
                    return;
                }
                if let Err(error) = serve(state, connection, dropped).await {
                    tracing::debug!(target: "cua_spacesd_client::media_quic", %error, "QUIC media connection ended");
                }
            });
        }
    });
    tracing::info!(target: "cua_spacesd_client::media_quic", port, certificate_sha256 = %media.certificate_sha256, "QUIC media listener ready");
    Ok(media)
}

/// Video is queued only while the datagram buffer keeps room for audio.
fn video_may_queue(buffer_space: usize, video_bytes: usize) -> bool {
    buffer_space >= video_bytes.saturating_add(AUDIO_RESERVE_BYTES)
}

fn close(connection: &quinn::Connection, code: u16) {
    connection.close(close_code::quic_error(code).into(), b"");
}

async fn serve(
    state: Arc<DesktopState>,
    connection: quinn::Connection,
    dropped: Arc<AtomicU64>,
) -> Result<(), String> {
    let (mut send, mut receive) =
        tokio::time::timeout(Duration::from_secs(10), connection.accept_bi())
            .await
            .map_err(|_| "no stream")?
            .map_err(|error| error.to_string())?;
    // The ticket must be the first message.
    let first = tokio::time::timeout(Duration::from_secs(10), read_message(&mut receive))
        .await
        .map_err(|_| "no ticket")?
        .map_err(|error| error.to_string())?;
    let ticket = match first
        .as_deref()
        .map(|bytes| decode_client_text(&String::from_utf8_lossy(bytes)))
    {
        Some(Ok(ClientMessage::Ticket { ticket })) => ticket,
        _ => {
            close(&connection, close_code::TICKET_INVALID);
            return Err("first message was not a ticket".into());
        }
    };
    let Ok(claims) = state.ctx.validate_ticket(&ticket, TicketScope::Media) else {
        close(&connection, close_code::TICKET_INVALID);
        return Err("invalid ticket".into());
    };
    let Some(session) = state.media.session(&claims.resource) else {
        close(&connection, close_code::SESSION_CLOSED);
        return Err("session closed".into());
    };
    let mut viewer = match session.attach() {
        Ok(viewer) => viewer,
        Err(code) => {
            close(&connection, code);
            return Err("attach refused".into());
        }
    };

    // Reliable-stream reader and datagram reader feed one channel so the
    // pump below never cancels a partial read.
    enum Incoming {
        Message(Vec<u8>),
        Datagram(Bytes),
        Closed,
    }
    let (sender, mut incoming) = tokio::sync::mpsc::channel::<Incoming>(256);
    let stream_sender = sender.clone();
    tokio::spawn(async move {
        loop {
            let event = match read_message(&mut receive).await {
                Ok(Some(message)) => Incoming::Message(message),
                _ => Incoming::Closed,
            };
            let closed = matches!(event, Incoming::Closed);
            if stream_sender.send(event).await.is_err() || closed {
                return;
            }
        }
    });
    let datagram_connection = connection.clone();
    tokio::spawn(async move {
        while let Ok(datagram) = datagram_connection.read_datagram().await {
            if sender.send(Incoming::Datagram(datagram)).await.is_err() {
                return;
            }
        }
        let _ = sender.send(Incoming::Closed).await;
    });

    let mut packet_id = 0u64;
    loop {
        tokio::select! {
            outbound = viewer.next_outbound() => {
                match outbound {
                    Outbound::Control(message) => {
                        write_message(&mut send, encode_server_text(&message).as_bytes())
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    Outbound::Audio(packet) => {
                        let fits = connection.max_datagram_size().is_some_and(|max| packet.len() <= max);
                        if fits {
                            let _ = connection.send_datagram(Bytes::from(packet.to_vec()));
                        } else {
                            write_message(&mut send, &packet).await.map_err(|error| error.to_string())?;
                        }
                    }
                    Outbound::Video(packet) => {
                        let Some(max) = connection.max_datagram_size() else {
                            close(&connection, close_code::PROTOCOL_VIOLATION);
                            return Err("peer did not enable datagrams".into());
                        };
                        let bytes = packet.encode();
                        packet_id += 1;
                        if !video_may_queue(connection.datagram_send_buffer_space(), bytes.len()) {
                            // Keep room for audio; the client recovers with a keyframe.
                            dropped.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        let fragments = cua_media_transport::fragment_video_packet_v2(packet_id, packet.keyframe, &bytes, max)
                            .map_err(|error| error.to_string())?;
                        for fragment in fragments {
                            let _ = connection.send_datagram(Bytes::from(fragment));
                        }
                    }
                    Outbound::Close(code) => {
                        let _ = send.finish();
                        close(&connection, code);
                        return Ok(());
                    }
                }
            }
            event = incoming.recv() => {
                match event {
                    Some(Incoming::Message(bytes)) => {
                        if cua_media_transport::audio::is_audio_packet(&bytes) {
                            viewer.handle_binary(&bytes);
                            continue;
                        }
                        match decode_client_text(&String::from_utf8_lossy(&bytes)) {
                            Ok(message) => viewer.handle(message).await,
                            Err(error) => {
                                let _ = write_message(
                                    &mut send,
                                    encode_server_text(&ServerMessage::error(ErrorCode::InvalidMessage, error)).as_bytes(),
                                )
                                .await;
                                close(&connection, close_code::PROTOCOL_VIOLATION);
                                return Ok(());
                            }
                        }
                    }
                    Some(Incoming::Datagram(datagram)) => {
                        if cua_media_transport::audio::is_audio_packet(&datagram) {
                            viewer.handle_binary(&datagram);
                        }
                    }
                    Some(Incoming::Closed) | None => return Ok(()),
                }
            }
        }
    }
}

// Keep the v2 module referenced for docs links.
#[allow(dead_code)]
const _ALPN: &[u8] = v2::QUIC_ALPN;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_yields_the_datagram_buffer_to_audio() {
        assert!(video_may_queue(4 * 1024 * 1024, 100_000));
        // Room for the video but not for audio after it: video is dropped.
        assert!(!video_may_queue(100_000 + AUDIO_RESERVE_BYTES - 1, 100_000));
        assert!(!video_may_queue(0, 1));
        assert!(!video_may_queue(usize::MAX - 1, usize::MAX));
    }
}
