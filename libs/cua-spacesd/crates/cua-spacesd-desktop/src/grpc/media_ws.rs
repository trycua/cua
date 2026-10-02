// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The `/media` WebSocket: rcdp wire v2 for one media session.
//!
//! The ticket is validated during the HTTP upgrade (401 for a missing,
//! unknown or expired ticket, 410 when its session is closed) and nothing is
//! read from the client before the server sends `hello` and
//! `session_opened`. Control messages are text frames; video and audio are
//! binary frames. The server pings every 20 s and closes an unresponsive
//! socket after 60 s (4408).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use cua_media_protocol::v2::{
    self, close_code, decode_client_text, encode_server_text, ErrorCode, ServerMessage,
};
use cua_spacesd_session::media::{Outbound, Viewer};
use futures_util::{SinkExt as _, StreamExt as _};

use super::DesktopState;
use cua_spacesd_server::{TicketError, TicketScope};

pub(crate) fn router(state: Arc<DesktopState>) -> Router {
    Router::new()
        .route(cua_proto::metadata::MEDIA_WS_PATH, get(upgrade))
        .with_state(state)
}

async fn upgrade(
    State(state): State<Arc<DesktopState>>,
    uri: Uri,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let validated = state
        .ctx
        .validate_request_ticket(&uri, &headers, TicketScope::Media)
        .or_else(|error| match (error, legacy_subprotocol_ticket(&headers)) {
            // `?ticket=` and `cua.ticket.<ticket>` are handled above; the
            // legacy `rcdp.v2.ticket.<ticket>` form is still accepted.
            (TicketError::Malformed, Some(ticket)) => state
                .ctx
                .validate_ticket(&ticket, TicketScope::Media)
                .map(|claims| {
                    (
                        claims,
                        Some(format!(
                            "{}{ticket}",
                            v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX
                        )),
                    )
                }),
            (error, _) => Err(error),
        });
    let (claims, ticket_subprotocol) = match validated {
        Ok(found) => found,
        Err(error) => {
            let reason = match error {
                TicketError::Expired => "ticket expired",
                TicketError::Malformed => "missing ticket",
                _ => "invalid ticket",
            };
            return (StatusCode::UNAUTHORIZED, reason).into_response();
        }
    };
    let Some(session) = state.media.session(&claims.resource) else {
        return (StatusCode::GONE, "media session is closed").into_response();
    };
    if session.is_closed() {
        return (StatusCode::GONE, "media session is closed").into_response();
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    // Echo the wire subprotocol when the client offered it (browsers
    // require the server to pick one of the offered values).
    let offered_v2 = headers
        .get_all(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|protocol| protocol.trim() == v2::WS_SUBPROTOCOL);
    let ws = match (offered_v2, ticket_subprotocol) {
        (true, _) => ws.protocols([v2::WS_SUBPROTOCOL]),
        (false, Some(protocol)) => ws.protocols([protocol]),
        (false, None) => ws,
    };
    ws.max_message_size(64 * 1024 * 1024 + 1024 * 1024)
        .on_upgrade(move |socket| async move {
            match session.attach() {
                Ok(viewer) => serve(socket, viewer).await,
                Err(code) => {
                    let mut socket = socket;
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code,
                            reason: "".into(),
                        })))
                        .await;
                }
            }
        })
}

fn legacy_subprotocol_ticket(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .find_map(|protocol| {
            protocol
                .trim()
                .strip_prefix(v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX)
                .map(str::to_owned)
        })
}

fn close(code: u16, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.to_owned().into(),
    }))
}

async fn serve(socket: WebSocket, mut viewer: Viewer) {
    let (mut sink, mut source) = socket.split();
    let mut ping = tokio::time::interval(Duration::from_secs(v2::PING_INTERVAL_SECS));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            outbound = viewer.next_outbound() => {
                let message = match outbound {
                    Outbound::Control(message) => Message::Text(encode_server_text(&message).into()),
                    Outbound::Audio(packet) => Message::Binary(packet.to_vec().into()),
                    Outbound::Video(packet) => Message::Binary(packet.encode().into()),
                    Outbound::Close(code) => {
                        let _ = sink.send(close(code, "")).await;
                        break;
                    }
                };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
            incoming = source.next() => {
                let Some(Ok(message)) = incoming else { break };
                last_heard = Instant::now();
                match message {
                    Message::Text(text) => match decode_client_text(&text) {
                        Ok(message) => viewer.handle(message).await,
                        Err(error) => {
                            let _ = sink
                                .send(Message::Text(
                                    encode_server_text(&ServerMessage::error(ErrorCode::InvalidMessage, error)).into(),
                                ))
                                .await;
                            let _ = sink.send(close(close_code::PROTOCOL_VIOLATION, "malformed control message")).await;
                            break;
                        }
                    },
                    Message::Binary(bytes) => viewer.handle_binary(&bytes),
                    Message::Ping(_) | Message::Pong(_) => {}
                    Message::Close(_) => break,
                }
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > Duration::from_secs(v2::IDLE_TIMEOUT_SECS) {
                    let _ = sink.send(close(close_code::IDLE_TIMEOUT, "idle")).await;
                    break;
                }
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
}
