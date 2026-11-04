// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cua_media_protocol::{ServerMessage, WireHeader};
use cua_media_transport::{decode_packet, encode_packet, MAX_PAYLOAD_BYTES};
use cua_spacesd_session::{Connection, OutboundPacket, ServerRuntime};
use futures_util::StreamExt as _;

const VIEWER: &str = include_str!("../../../examples/browser/window-stream-client.html");
type ClientPacket = (cua_media_protocol::ClientMessage, Vec<u8>);

#[derive(Debug, Clone)]
pub struct RemoteShareConfig {
    pub name: String,
    pub application_id: String,
    pub allowed_users: Vec<String>,
    pub required_capability: Option<String>,
}

impl RemoteShareConfig {
    pub fn validate(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        if self.name.is_empty() {
            return Err("remote share name must not be empty".into());
        }
        if self.application_id.is_empty() {
            return Err("remote share application ID must not be empty".into());
        }
        if self.allowed_users.is_empty() && self.required_capability.is_none() {
            return Err("remote share requires an allowed Tailscale user or app capability".into());
        }
        if self.allowed_users.iter().any(String::is_empty)
            || self
                .required_capability
                .as_ref()
                .is_some_and(String::is_empty)
        {
            return Err("remote authorization values must not be empty".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
struct RemoteState {
    runtime: Arc<ServerRuntime>,
    config: Arc<RemoteShareConfig>,
}

pub async fn serve_remote(
    address: SocketAddr,
    runtime: Arc<ServerRuntime>,
    config: RemoteShareConfig,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !address.ip().is_loopback() {
        return Err(
            format!("remote RCDP must bind loopback for Tailscale Serve, got {address}").into(),
        );
    }
    let listener = tokio::net::TcpListener::bind(address).await?;
    serve_remote_listener(listener, runtime, config).await
}

pub async fn serve_remote_listener(
    listener: tokio::net::TcpListener,
    runtime: Arc<ServerRuntime>,
    config: RemoteShareConfig,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let address = listener.local_addr()?;
    if !address.ip().is_loopback() {
        return Err(
            format!("remote RCDP must bind loopback for Tailscale Serve, got {address}").into(),
        );
    }
    config.validate()?;
    let state = RemoteState {
        runtime,
        config: Arc::new(config),
    };
    let application = Router::new()
        .route("/", get(viewer))
        .route("/healthz", get(health))
        .route("/v1/share", get(share))
        .route("/v1/connect", get(connect))
        .with_state(state);
    axum::serve(listener, application).await?;
    Ok(())
}

async fn viewer() -> Html<&'static str> {
    Html(VIEWER)
}

async fn health() -> &'static str {
    "ok\n"
}

async fn share(State(state): State<RemoteState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&headers, &state.config) {
        return response.into_response();
    }
    Json(serde_json::json!({
        "name": state.config.name,
        "application_id": state.config.application_id,
        "policy_ceiling": "background_only",
        "websocket": "/v1/connect",
        "codecs": ["h264", "bgra"],
    }))
    .into_response()
}

async fn connect(
    websocket: WebSocketUpgrade,
    State(state): State<RemoteState>,
    headers: HeaderMap,
) -> Response {
    let identity = match authorize(&headers, &state.config) {
        Ok(identity) => identity,
        Err(response) => return response.into_response(),
    };
    websocket
        .max_message_size(MAX_PAYLOAD_BYTES + 1024 * 1024 + 8)
        .max_frame_size(MAX_PAYLOAD_BYTES + 1024 * 1024 + 8)
        .on_upgrade(move |socket| serve_socket(socket, state.runtime.connect(), identity))
}

fn authorize(
    headers: &HeaderMap,
    config: &RemoteShareConfig,
) -> Result<String, (StatusCode, &'static str)> {
    let user = headers
        .get("tailscale-user-login")
        .and_then(|value| value.to_str().ok());
    if !config.allowed_users.is_empty()
        && !user.is_some_and(|user| config.allowed_users.iter().any(|allowed| allowed == user))
    {
        return Err((StatusCode::UNAUTHORIZED, "Tailscale user is not allowed"));
    }

    if let Some(required) = &config.required_capability {
        let has_capability = headers
            .get("tailscale-app-capabilities")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
            .and_then(|value| value.as_object().cloned())
            .is_some_and(|capabilities| capabilities.contains_key(required));
        if !has_capability {
            return Err((
                StatusCode::FORBIDDEN,
                "required Tailscale app capability is missing",
            ));
        }
    }

    Ok(user.unwrap_or("tailscale-capability").to_owned())
}

async fn serve_socket(mut socket: WebSocket, mut connection: Connection, identity: String) {
    tracing::debug!(%identity, "authorized remote RCDP connection");
    if send_control(&mut socket, ServerMessage::Authenticated)
        .await
        .is_err()
    {
        return;
    }
    let outbound_ready = connection.outbound_ready();
    let mut deadline_tick = tokio::time::interval(Duration::from_millis(50));
    deadline_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            incoming = socket.next() => {
                let Some(incoming) = incoming else { return; };
                let message = match incoming {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::debug!(%error, "remote RCDP WebSocket ended");
                        return;
                    }
                };
                match client_message(message) {
                    Ok(Some((message, payload))) => {
                        if write_outbound(&mut socket, connection.handle_packet(message, payload).await).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::debug!(%error, "invalid remote RCDP packet");
                        let _ = socket.close().await;
                        return;
                    }
                }
            }
            _ = outbound_ready.notified() => {
                if write_outbound(&mut socket, connection.drain_outbound()).await.is_err() {
                    return;
                }
            }
            _ = deadline_tick.tick() => {
                if write_outbound(&mut socket, connection.drain_outbound()).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn client_message(message: Message) -> Result<Option<ClientPacket>, Box<dyn Error + Send + Sync>> {
    let (header, payload) = match message {
        Message::Text(text) => (serde_json::from_str::<WireHeader>(&text)?, Vec::new()),
        Message::Binary(packet) => decode_packet(&packet)?,
        Message::Ping(_) | Message::Pong(_) => return Ok(None),
        Message::Close(_) => return Ok(None),
    };
    let WireHeader::Client(message) = header else {
        return Err("client sent a non-client packet".into());
    };
    if matches!(
        message,
        cua_media_protocol::ClientMessage::Authenticate { .. }
    ) {
        return Err("Tailscale-authenticated clients must start with hello".into());
    }
    Ok(Some((message, payload)))
}

async fn write_outbound(
    socket: &mut WebSocket,
    packets: Vec<OutboundPacket>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    for packet in packets {
        match packet {
            OutboundPacket::Control(message) => send_control(socket, message).await?,
            OutboundPacket::Video {
                descriptor,
                payload,
            } => {
                let packet = encode_packet(&WireHeader::Video(descriptor), &payload)?;
                socket.send(Message::Binary(packet)).await?;
            }
            OutboundPacket::AppIcon {
                descriptor,
                payload,
            } => {
                let packet = encode_packet(&WireHeader::AppIcon(descriptor), &payload)?;
                socket.send(Message::Binary(packet)).await?;
            }
            OutboundPacket::ClipboardFiles { message, payload } => {
                let packet = encode_packet(&WireHeader::Server(message), &payload)?;
                socket.send(Message::Binary(packet)).await?;
            }
        }
    }
    Ok(())
}

async fn send_control(
    socket: &mut WebSocket,
    message: ServerMessage,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let text = serde_json::to_string(&WireHeader::Server(message))?;
    socket.send(Message::Text(text)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RemoteShareConfig {
        RemoteShareConfig {
            name: "codex".into(),
            application_id: "com.openai.codex".into(),
            allowed_users: vec!["user@example.com".into()],
            required_capability: Some("trycua.com/cap/rcdp".into()),
        }
    }

    #[test]
    fn tailscale_authorization_requires_user_and_capability() {
        let mut headers = HeaderMap::new();
        headers.insert("tailscale-user-login", "user@example.com".parse().unwrap());
        assert!(authorize(&headers, &config()).is_err());
        headers.insert(
            "tailscale-app-capabilities",
            r#"{"trycua.com/cap/rcdp":[{}]}"#.parse().unwrap(),
        );
        assert_eq!(authorize(&headers, &config()).unwrap(), "user@example.com");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_non_loopback_listener() {
        assert!(serve_remote(
            "0.0.0.0:0".parse().unwrap(),
            crate::tests::fixture_runtime(),
            config(),
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("must bind loopback"));
    }
}
