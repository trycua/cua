//! CDP (Chrome DevTools Protocol) client for Electron and other chromium-based apps.
//! Uses raw TCP for the HTTP /json endpoint and tokio-tungstenite for WebSocket.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use futures_util::{SinkExt, StreamExt};

pub struct CdpClient;

impl CdpClient {
    /// Returns true if a CDP endpoint is listening on `port`.
    pub async fn is_available(port: u16) -> bool {
        http_get_json(port).await.is_ok()
    }

    /// Scan the given ports and return the first one that has a "page" target.
    pub async fn find_page_target(ports: &[u16]) -> Option<u16> {
        for &port in ports {
            if let Ok(json) = http_get_json(port).await {
                if let Ok(arr) = serde_json::from_str::<serde_json::Value>(&json) {
                    if let Some(list) = arr.as_array() {
                        let has_page = list.iter().any(|t| {
                            t.get("type").and_then(|v| v.as_str()) == Some("page")
                        });
                        if has_page {
                            return Some(port);
                        }
                    }
                }
            }
        }
        None
    }

    /// Evaluate JavaScript via CDP Runtime.evaluate on the first page target.
    pub async fn evaluate(javascript: &str, port: u16) -> anyhow::Result<String> {
        // Get list of targets.
        let json = http_get_json(port).await?;
        let targets: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| anyhow::anyhow!("CDP /json parse error: {e}"))?;

        // Find first page target with a websocket URL.
        let ws_url = targets.as_array()
            .and_then(|arr| {
                arr.iter().find(|t| {
                    t.get("type").and_then(|v| v.as_str()) == Some("page")
                })
            })
            .and_then(|t| t.get("webSocketDebuggerUrl").and_then(|v| v.as_str()))
            .ok_or_else(|| anyhow::anyhow!("No page target with webSocketDebuggerUrl found on port {port}"))?
            .to_owned();

        // Build the CDP request.
        let request = serde_json::json!({
            "id": 1,
            "method": "Runtime.evaluate",
            "params": {
                "expression": javascript,
                "returnByValue": true,
                "awaitPromise": true
            }
        });
        let request_str = serde_json::to_string(&request)?;

        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await
                .map_err(|e| anyhow::anyhow!("WebSocket connect failed: {e}"))?;

            ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(request_str.into())).await
                .map_err(|e| anyhow::anyhow!("WebSocket send failed: {e}"))?;

            // Read frames until we get one with id=1.
            loop {
                let msg = ws_stream.next().await
                    .ok_or_else(|| anyhow::anyhow!("WebSocket stream ended without response"))??;
                let text = match msg {
                    tokio_tungstenite::tungstenite::Message::Text(t) => t,
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        anyhow::bail!("WebSocket closed before response");
                    }
                    _ => continue,
                };
                let obj: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| anyhow::anyhow!("CDP response parse error: {e}"))?;
                // Skip CDP event frames (they have "method" key but no "id").
                if obj.get("method").is_some() { continue; }
                if obj.get("id").and_then(|v| v.as_u64()) == Some(1) {
                    return parse_cdp_result(&obj);
                }
            }
        }).await
        .map_err(|_| anyhow::anyhow!("CDP evaluate timed out after 10s"))??;

        Ok(result)
    }
}

fn parse_cdp_result(obj: &serde_json::Value) -> anyhow::Result<String> {
    if let Some(err) = obj.get("error") {
        let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
        anyhow::bail!("CDP error: {msg}");
    }
    let result = &obj["result"]["result"];
    if let Some(v) = result.get("value") {
        return Ok(match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => "null".to_owned(),
            other => other.to_string(),
        });
    }
    if let Some(desc) = result.get("description").and_then(|v| v.as_str()) {
        return Ok(desc.to_owned());
    }
    Ok("undefined".to_owned())
}

async fn http_get_json(port: u16) -> anyhow::Result<String> {
    let mut stream = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
    ).await
    .map_err(|_| anyhow::anyhow!("TCP connect timed out"))?
    .map_err(|e| anyhow::anyhow!("TCP connect error: {e}"))?;

    let req = format!("GET /json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;

    let s = String::from_utf8_lossy(&buf);
    // Split off HTTP headers.
    let body = s.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Ok(body)
}
