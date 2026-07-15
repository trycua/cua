//! Pooled CDP browser-endpoint WebSocket client.
//!
//! One connection per endpoint URL; v1 serializes request/response
//! demux behind an async `Mutex` (a request holds the socket until its
//! matching `id` reply arrives; unsolicited events are skipped). This
//! trades throughput for simplicity — acceptable at v1's call rates.
//!
//! Only loopback `ws://` URLs are accepted: the endpoint is a local
//! browser the platform adapter proved we own, never a remote service.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Validate that `url` is a plain-`ws` loopback WebSocket URL.
/// Anything else — `wss`, remote hosts, hostnames that merely *resolve*
/// to loopback — is rejected; the endpoint contract is a literal
/// loopback address handed over by the platform adapter.
pub fn validate_loopback_ws_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("endpoint URL must use the ws:// scheme: {url}"))?;
    let authority = rest.split('/').next().unwrap_or("");
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        // [::1]:9222 form.
        bracketed.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    match host {
        "127.0.0.1" | "::1" | "localhost" => Ok(()),
        other => Err(format!("endpoint host {other:?} is not loopback: {url}")),
    }
}

/// A single browser-endpoint connection.
pub struct CdpConnection {
    ws: Mutex<WsStream>,
    next_id: AtomicU64,
}

impl CdpConnection {
    pub async fn connect(ws_url: &str) -> anyhow::Result<Self> {
        validate_loopback_ws_url(ws_url).map_err(|e| anyhow::anyhow!(e))?;
        let (ws, _resp) =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(ws_url))
                .await
                .map_err(|_| anyhow::anyhow!("CDP connect to {ws_url} timed out"))??;
        Ok(Self {
            ws: Mutex::new(ws),
            next_id: AtomicU64::new(1),
        })
    }

    /// Issue one CDP command and await its `id`-matched reply.
    /// `session_id` targets an attached (flattened) target session.
    /// Returns the `result` object, or an error carrying the CDP
    /// `error.message`.
    pub async fn call(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            msg["sessionId"] = Value::String(sid.to_owned());
        }

        // Hold the socket for the whole round trip — v1's serialized demux.
        let mut ws = self.ws.lock().await;
        tokio::time::timeout(CALL_TIMEOUT, async {
            ws.send(Message::Text(msg.to_string())).await?;
            loop {
                let frame = ws
                    .next()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("CDP socket closed during {method}"))??;
                let text = match frame {
                    Message::Text(t) => t,
                    Message::Ping(_) | Message::Pong(_) | Message::Binary(_) => continue,
                    Message::Close(_) => {
                        anyhow::bail!("CDP socket closed during {method}")
                    }
                    _ => continue,
                };
                let v: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("id").and_then(Value::as_u64) != Some(id) {
                    continue; // event or another caller's reply (serialized, so: event)
                }
                if let Some(err) = v.get("error") {
                    let code = err.get("code").and_then(Value::as_i64);
                    let emsg = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown CDP error");
                    if let Some(code) = code {
                        anyhow::bail!("CDP {method} failed ({code}): {emsg}");
                    }
                    anyhow::bail!("CDP {method} failed: {emsg}");
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("CDP {method} timed out after {CALL_TIMEOUT:?}"))?
    }
}

/// One pooled connection per endpoint URL.
pub struct CdpPool {
    conns: Mutex<HashMap<String, Arc<CdpConnection>>>,
}

impl CdpPool {
    pub fn new() -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
        }
    }

    /// Get the pooled connection for `ws_url`, dialing if needed.
    pub async fn get(&self, ws_url: &str) -> anyhow::Result<Arc<CdpConnection>> {
        let mut conns = self.conns.lock().await;
        if let Some(existing) = conns.get(ws_url) {
            return Ok(existing.clone());
        }
        let conn = Arc::new(CdpConnection::connect(ws_url).await?);
        conns.insert(ws_url.to_owned(), conn.clone());
        Ok(conn)
    }

    /// Drop a (likely dead) connection so the next call redials.
    pub async fn evict(&self, ws_url: &str) {
        self.conns.lock().await.remove(ws_url);
    }
}

impl Default for CdpPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_urls_are_accepted() {
        for ok in [
            "ws://127.0.0.1:9222/devtools/browser/abc",
            "ws://localhost:9222/devtools/browser/abc",
            "ws://[::1]:9222/devtools/browser/abc",
            "ws://127.0.0.1/devtools/browser/abc",
        ] {
            assert!(validate_loopback_ws_url(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn non_loopback_and_non_ws_urls_are_rejected() {
        for bad in [
            "wss://127.0.0.1:9222/devtools/browser/abc",
            "http://127.0.0.1:9222/json",
            "ws://10.0.0.5:9222/devtools/browser/abc",
            "ws://example.com:9222/devtools/browser/abc",
            "ws://127.0.0.1.evil.test:9222/x",
            "ws://[fe80::1]:9222/x",
            "",
        ] {
            assert!(validate_loopback_ws_url(bad).is_err(), "{bad}");
        }
    }
}
