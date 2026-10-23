//! The event stream the Daytona SDK listens on: Socket.IO (protocol 5) over
//! an Engine.IO (protocol 4) WebSocket at `/api/socket.io/`. The client
//! authenticates in the namespace connect (`{"token": <api key>}`); the
//! server pushes `sandbox.created` and `sandbox.state.updated` for that
//! key's sandboxes and pings every 25 s. Only what the SDK uses: WebSocket
//! transport, the default namespace, server-to-client events.

use super::Daytona;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const PING_INTERVAL: Duration = Duration::from_secs(25);
const PING_TIMEOUT: Duration = Duration::from_secs(20);

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/api/socket.io/", get(upgrade))
        .route("/api/socket.io", get(upgrade))
        .with_state(d)
}

async fn upgrade(
    State(d): State<Arc<Daytona>>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> axum::response::Response {
    match ws.ok() {
        Some(ws) => ws.on_upgrade(move |s| session(s, d)),
        None => (
            http::StatusCode::BAD_REQUEST,
            r#"{"code":3,"message":"Bad request: only the websocket transport is served"}"#,
        )
            .into_response(),
    }
}

/// Engine.IO open packet.
fn open_packet(sid: &str) -> String {
    format!(
        "0{}",
        json!({
            "sid": sid,
            "upgrades": [],
            "pingInterval": PING_INTERVAL.as_millis() as u64,
            "pingTimeout": PING_TIMEOUT.as_millis() as u64,
            "maxPayload": 1_000_000,
        })
    )
}

/// The auth token of a Socket.IO namespace connect (`40{...}`).
pub fn connect_token(packet: &str) -> Option<Option<String>> {
    let rest = packet.strip_prefix("40")?;
    // An explicit namespace other than "/" is not served.
    if rest.starts_with('/') && !rest.starts_with("/,") {
        return None;
    }
    let body = rest.trim_start_matches("/,");
    if body.is_empty() {
        return Some(None);
    }
    let v: Value = serde_json::from_str(body).ok()?;
    Some(v.get("token").and_then(Value::as_str).map(str::to_string))
}

async fn session(mut ws: WebSocket, d: Arc<Daytona>) {
    let sid = crate::auth::new_token()[..20].to_string();
    if ws
        .send(Message::Text(open_packet(&sid).into()))
        .await
        .is_err()
    {
        return;
    }
    let mut events = d.events.subscribe();
    let mut key_id: Option<String> = None;
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;
    let mut last_pong = tokio::time::Instant::now();
    loop {
        tokio::select! {
            m = ws.recv() => {
                let Some(Ok(m)) = m else { return };
                let Message::Text(t) = m else { continue };
                let t = t.to_string();
                if t == "3" {
                    last_pong = tokio::time::Instant::now();
                } else if t == "1" {
                    return;
                } else if let Some(token) = connect_token(&t) {
                    let entry = match token {
                        Some(tok) => d.keys.read().await.find(&tok).cloned(),
                        None => None,
                    };
                    match entry {
                        Some(k) => {
                            key_id = Some(k.id());
                            let ack = format!("40{}", json!({"sid": crate::auth::new_token()[..20].to_string()}));
                            if ws.send(Message::Text(ack.into())).await.is_err() {
                                return;
                            }
                        }
                        None => {
                            let _ = ws
                                .send(Message::Text(format!("44{}", json!({"message": "Unauthorized"})).into()))
                                .await;
                        }
                    }
                } else if t.starts_with("41") {
                    key_id = None;
                }
            }
            e = events.recv() => {
                let Ok((kid, name, payload)) = e else { continue };
                if key_id.as_deref() != Some(kid.as_str()) {
                    continue;
                }
                let pkt = format!("42{}", json!([name, payload]));
                if ws.send(Message::Text(pkt.into())).await.is_err() {
                    return;
                }
            }
            _ = ping.tick() => {
                if last_pong.elapsed() > PING_INTERVAL + PING_TIMEOUT {
                    return;
                }
                if ws.send(Message::Text("2".into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namespace_connect() {
        assert_eq!(
            connect_token(r#"40{"token":"dtn_x"}"#),
            Some(Some("dtn_x".into()))
        );
        assert_eq!(connect_token("40"), Some(None));
        assert_eq!(connect_token("40/admin,{}"), None);
        assert_eq!(connect_token("42[\"x\"]"), None);
        assert!(open_packet("s").starts_with("0{"));
    }
}
