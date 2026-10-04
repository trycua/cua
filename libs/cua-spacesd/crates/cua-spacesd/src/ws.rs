// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! WebSocket binding for the RCDP daemon.
//!
//! Control packets travel as WebSocket text messages carrying one JSON
//! `WireHeader`. Video packets travel as binary messages with the same
//! length-prefixed layout as the local transport: two network-order `u32`
//! lengths, the JSON header, then the payload. This matches the HTML client.

use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cua_media_protocol::{ClientMessage, ServerErrorCode, ServerMessage, WireHeader};
use cua_spacesd_session::{Connection, OutboundPacket, ServerRuntime};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

use crate::presence::PresenceHub;

pub struct WsDaemon {
    pub runtime: Arc<ServerRuntime>,
    pub presence: Arc<PresenceHub>,
    /// Required authentication token. `None` accepts any `authenticate`.
    pub token: Option<String>,
}

static NEXT_WS_CONNECTION: AtomicU64 = AtomicU64::new(1);

pub async fn serve_ws(
    address: SocketAddr,
    daemon: Arc<WsDaemon>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let listener = TcpListener::bind(address).await?;
    tracing::info!(%address, "RCDP WebSocket listener ready");
    serve_ws_listener(listener, daemon).await
}

/// Serves an already bound listener (see [`serve_ws`]).
pub async fn serve_ws_listener(
    listener: TcpListener,
    daemon: Arc<WsDaemon>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // A token-less loopback listener trusts local processes, not web pages:
    // refuse handshakes from other origins or host names.
    let guard_browsers = daemon.token.is_none() && listener.local_addr()?.ip().is_loopback();
    loop {
        let (stream, peer) = listener.accept().await?;
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let connection_id = NEXT_WS_CONNECTION.fetch_add(1, Ordering::Relaxed);
            if let Err(error) =
                serve_ws_connection(stream, connection_id, daemon.clone(), guard_browsers).await
            {
                tracing::debug!(%peer, %error, "RCDP WebSocket connection ended");
            }
            daemon.presence.unregister(connection_id);
        });
    }
}

fn control_message(message: &ServerMessage) -> Result<Message, serde_json::Error> {
    Ok(Message::Text(serde_json::to_string(&WireHeader::Server(
        message.clone(),
    ))?))
}

fn encode_packet(packet: OutboundPacket) -> Result<Message, serde_json::Error> {
    match packet {
        OutboundPacket::Control(message) => control_message(&message),
        OutboundPacket::Video {
            descriptor,
            payload,
        } => {
            let header = serde_json::to_vec(&WireHeader::Video(descriptor))?;
            let mut frame = Vec::with_capacity(8 + header.len() + payload.len());
            frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
            frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            frame.extend_from_slice(&header);
            frame.extend_from_slice(&payload);
            Ok(Message::Binary(frame))
        }
        OutboundPacket::AppIcon {
            descriptor,
            payload,
        } => {
            let header = serde_json::to_vec(&WireHeader::AppIcon(descriptor))?;
            let mut frame = Vec::with_capacity(8 + header.len() + payload.len());
            frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
            frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            frame.extend_from_slice(&header);
            frame.extend_from_slice(&payload);
            Ok(Message::Binary(frame))
        }
        OutboundPacket::ClipboardFiles { message, payload } => {
            let header = serde_json::to_vec(&WireHeader::Server(message))?;
            let mut frame = Vec::with_capacity(8 + header.len() + payload.len());
            frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
            frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            frame.extend_from_slice(&header);
            frame.extend_from_slice(&payload);
            Ok(Message::Binary(frame))
        }
    }
}

use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};

/// Refuses a browser handshake from another origin or host name when
/// `guard_browsers` (a token-less loopback listener).
#[allow(clippy::result_large_err)]
fn handshake_check(
    guard_browsers: bool,
    request: &HandshakeRequest,
    response: HandshakeResponse,
) -> Result<HandshakeResponse, ErrorResponse> {
    if guard_browsers {
        if let Some(reason) =
            cua_spacesd_server::browser_guard::refusal(request.headers(), request.uri())
        {
            let mut refused = ErrorResponse::new(Some(reason.to_owned()));
            *refused.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            return Err(refused);
        }
    }
    Ok(response)
}

#[allow(clippy::result_large_err)] // tungstenite's handshake callback type
async fn serve_ws_connection(
    stream: TcpStream,
    connection_id: u64,
    daemon: Arc<WsDaemon>,
    guard_browsers: bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let check = move |request: &HandshakeRequest, response: HandshakeResponse| {
        handshake_check(guard_browsers, request, response)
    };
    let websocket = tokio_tungstenite::accept_hdr_async(stream, check).await?;
    let (mut sink, mut source) = websocket.split();
    let mut connection: Connection = daemon.runtime.connect();
    let mut presence_rx = daemon.presence.register(connection_id);
    let mut authenticated = daemon.token.is_none();
    let outbound_ready = connection.outbound_ready();
    let mut deadline_tick = tokio::time::interval(Duration::from_millis(50));
    deadline_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // WebSocket clients use the same authentication phase machine whether a
    // deployment requires a token or trusts its loopback boundary. Complete
    // that phase eagerly for the latter so clients do not have to guess which
    // handshake the server selected.
    if authenticated {
        sink.send(control_message(&ServerMessage::Authenticated)?)
            .await?;
    }

    loop {
        tokio::select! {
            biased;
            incoming = source.next() => {
                let Some(incoming) = incoming else {
                    return Ok(());
                };
                let message = incoming?;
                match message {
                    Message::Text(text) => {
                        let header: WireHeader = match serde_json::from_str(&text) {
                            Ok(header) => header,
                            Err(error) => {
                                sink.send(control_message(&ServerMessage::Error {
                                    code: ServerErrorCode::UnsupportedMessage,
                                    message: format!("invalid wire header: {error}"),
                                })?).await?;
                                continue;
                            }
                        };
                        let WireHeader::Client(client_message) = header else {
                            sink.send(control_message(&ServerMessage::Error {
                                code: ServerErrorCode::UnsupportedMessage,
                                message: "clients must send client-direction packets".into(),
                            })?).await?;
                            continue;
                        };
                        if let ClientMessage::Authenticate { token } = &client_message {
                            if daemon.token.as_deref().is_none_or(|expected| {
                                cua_spacesd_server::util::constant_time_eq(
                                    expected.as_bytes(),
                                    token.as_bytes(),
                                )
                            }) {
                                authenticated = true;
                                sink.send(control_message(&ServerMessage::Authenticated)?).await?;
                            } else {
                                sink.send(control_message(&ServerMessage::Error {
                                    code: ServerErrorCode::HelloRequired,
                                    message: "invalid authentication token".into(),
                                })?).await?;
                                return Ok(());
                            }
                            continue;
                        }
                        if !authenticated {
                            sink.send(control_message(&ServerMessage::Error {
                                code: ServerErrorCode::HelloRequired,
                                message: "authenticate before other messages".into(),
                            })?).await?;
                            return Ok(());
                        }
                        if let Some(responses) = daemon.presence.handle(connection_id, &client_message) {
                            for response in responses {
                                sink.send(control_message(&response)?).await?;
                            }
                            continue;
                        }
                        for packet in connection.handle(client_message).await {
                            sink.send(encode_packet(packet)?).await?;
                        }
                    }
                    Message::Binary(packet) => {
                        let (header, payload) = match cua_media_transport::decode_packet(&packet) {
                            Ok(packet) => packet,
                            Err(error) => {
                                sink.send(control_message(&ServerMessage::Error {
                                    code: ServerErrorCode::UnsupportedMessage,
                                    message: format!("invalid binary packet: {error}"),
                                })?).await?;
                                continue;
                            }
                        };
                        let WireHeader::Client(client_message) = header else {
                            return Err("client binary packet had the wrong direction".into());
                        };
                        if matches!(client_message, ClientMessage::Authenticate { .. }) {
                            return Err("authentication must be a control message".into());
                        }
                        if !authenticated {
                            return Err("authenticate before binary messages".into());
                        }
                        if daemon.presence.handle(connection_id, &client_message).is_some() {
                            return Err("presence messages cannot carry binary payloads".into());
                        }
                        for packet in connection.handle_packet(client_message, payload).await {
                            sink.send(encode_packet(packet)?).await?;
                        }
                    }
                    Message::Ping(payload) => sink.send(Message::Pong(payload)).await?,
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
            presence_message = presence_rx.recv() => {
                if let Some(message) = presence_message {
                    sink.send(control_message(&message)?).await?;
                }
            }
            _ = outbound_ready.notified() => {
                for packet in connection.drain_outbound() {
                    sink.send(encode_packet(packet)?).await?;
                }
            }
            _ = deadline_tick.tick() => {
                for packet in connection.drain_outbound() {
                    sink.send(encode_packet(packet)?).await?;
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::result_large_err)]
mod guard_tests {
    use super::*;

    fn request(origin: Option<&str>) -> HandshakeRequest {
        let mut b = HandshakeRequest::builder()
            .uri("/")
            .header("host", "127.0.0.1:3211");
        if let Some(origin) = origin {
            b = b.header("origin", origin);
        }
        b.body(()).unwrap()
    }

    #[test]
    fn tokenless_loopback_handshakes_refuse_foreign_origins() {
        let ok =
            |guard, origin| handshake_check(guard, &request(origin), HandshakeResponse::default());
        assert!(ok(true, None).is_ok());
        assert!(ok(true, Some("http://localhost:8080")).is_ok());
        let refused = ok(true, Some("https://evil.example")).unwrap_err();
        assert_eq!(refused.status(), 403);
        // Token-protected or non-loopback listeners (the Fleet gateway) are
        // not guarded here.
        assert!(ok(false, Some("https://evil.example")).is_ok());
    }
}
