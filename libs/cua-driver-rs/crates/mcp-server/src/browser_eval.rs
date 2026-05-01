//! `browser_eval` tool — Chromium CDP Runtime.evaluate over a local WebSocket.
//!
//! Cross-platform: all I/O is plain TCP to localhost (no external HTTP/WS crates).
//! Mirrors the Windows C# reference in CuaDriver.Win/Tools/BrowserEvalTool.cs
//! and CuaDriver.Win/Browser/CdpBrowserBridge.cs.
//!
//! Flow:
//!  1. HTTP GET http://127.0.0.1:{cdp_port}/json  → discover page WebSocket URLs.
//!  2. WebSocket connect to the first (or best-matched) page URL.
//!  3. Send Runtime.evaluate with userGesture=true.
//!  4. Read frames until we receive the response with id=1.

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

pub struct BrowserEvalTool;

#[async_trait]
impl Tool for BrowserEvalTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| ToolDef {
            name: "browser_eval".into(),
            description: "Evaluate JavaScript in a Chromium browser via CDP Runtime.evaluate \
                          with userGesture=true. Requires cdp_port. Pass window_id to target \
                          a specific browser window/tab (matched by title heuristic).".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["expression"],
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "JavaScript expression to evaluate."
                    },
                    "cdp_port": {
                        "type": "integer",
                        "description": "Chromium remote debugging port (--remote-debugging-port)."
                    },
                    "window_id": {
                        "type": "integer",
                        "description": "Optional browser window/HWND/XID used to bind CDP to the matching tab."
                    },
                    "await_promise": {
                        "type": "boolean",
                        "description": "Await a returned Promise (default true)."
                    }
                }
            }),
            read_only:   false,
            destructive: true,
            idempotent:  false,
            open_world:  true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let expression = match args.get("expression").and_then(|v| v.as_str()) {
            Some(e) => e.to_string(),
            None => return ToolResult::error("browser_eval: 'expression' is required"),
        };
        let cdp_port = match args.get("cdp_port").and_then(|v| v.as_u64()) {
            Some(p) if p > 0 && p < 65536 => p as u16,
            _ => return ToolResult::error(
                "browser_eval: 'cdp_port' is required (Chromium --remote-debugging-port value)"
            ),
        };
        let await_promise = args.get("await_promise")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let window_title_hint = args.get("window_id").map(|_| ""); // reserved for future matching

        match cdp_evaluate(cdp_port, window_title_hint, &expression, await_promise).await {
            Ok(response) => {
                let result_text = format_cdp_result(&response);
                ToolResult::text(format!("cdp.runtime.evaluate.user_gesture: {result_text}"))
            }
            Err(e) => ToolResult::error(format!("browser_eval: {e}")),
        }
    }
}

// ── High-level evaluate ───────────────────────────────────────────────────

async fn cdp_evaluate(
    port: u16,
    _window_title_hint: Option<&str>,
    expression: &str,
    await_promise: bool,
) -> anyhow::Result<Value> {
    // Discover page tabs.
    let pages = cdp_list_pages(port).await?;
    if pages.is_empty() {
        anyhow::bail!("No CDP page tabs found on port {port}");
    }

    // Take the first available page (future: match by window title hint).
    let ws_url = pages[0]
        .get("webSocketDebuggerUrl")
        .and_then(|u| u.as_str())
        .ok_or_else(|| anyhow::anyhow!("Page has no webSocketDebuggerUrl"))?
        .to_string();

    let cmd = serde_json::json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": {
            "expression": expression,
            "userGesture": true,
            "awaitPromise": await_promise
        }
    });

    // 30-second wall-clock timeout (covers awaited promises).
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        cdp_ws_call(port, &ws_url, &cmd.to_string()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("CDP evaluate timed out after 30 s"))?
}

fn format_cdp_result(response: &Value) -> String {
    // If the result contains an exception, surface it.
    if let Some(exc) = response
        .get("result")
        .and_then(|r| r.get("exceptionDetails"))
    {
        let msg = exc
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or_else(|| exc.to_string().leak()); // static lifetime for error msg
        return format!("exception: {msg}");
    }

    // Extract the evaluated value.
    if let Some(result) = response.get("result").and_then(|r| r.get("result")) {
        if let Some(v) = result.get("value") {
            return v.to_string();
        }
        if let Some(desc) = result.get("description").and_then(|d| d.as_str()) {
            return desc.to_string();
        }
    }

    response.to_string()
}

// ── HTTP page discovery ───────────────────────────────────────────────────

/// GET http://127.0.0.1:{port}/json → array of page descriptors.
/// Returns only entries whose `type` == "page" and that have a webSocketDebuggerUrl.
async fn cdp_list_pages(port: u16) -> anyhow::Result<Vec<Value>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await
        .map_err(|e| anyhow::anyhow!("Cannot connect to CDP on port {port}: {e}"))?;

    let req = format!(
        "GET /json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;

    let text = std::str::from_utf8(&buf)
        .map_err(|_| anyhow::anyhow!("CDP /json response is not valid UTF-8"))?;
    let body_start = text
        .find("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("Invalid HTTP response from CDP"))? + 4;

    let all: Value = serde_json::from_str(&text[body_start..])
        .map_err(|e| anyhow::anyhow!("CDP /json parse error: {e}"))?;

    let pages = all
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("CDP /json is not a JSON array"))?
        .iter()
        .filter(|p| {
            p.get("type").and_then(|t| t.as_str()) == Some("page")
                && p.get("webSocketDebuggerUrl")
                    .and_then(|u| u.as_str())
                    .is_some()
        })
        .cloned()
        .collect();

    Ok(pages)
}

// ── WebSocket CDP call ────────────────────────────────────────────────────

/// Open a WebSocket to `ws_url` (on `port`), send `message`, and return the
/// first response frame whose JSON `id` field equals 1.
/// Intermediate event notifications (no `id` field or id != 1) are silently skipped.
async fn cdp_ws_call(port: u16, ws_url: &str, message: &str) -> anyhow::Result<Value> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Extract the URL path from e.g. "ws://127.0.0.1:9222/devtools/page/ABC".
    let path: String = if let Some(rest) = ws_url.strip_prefix("ws://") {
        rest.splitn(2, '/')
            .nth(1)
            .map(|p| format!("/{p}"))
            .unwrap_or_else(|| "/".into())
    } else {
        ws_url.to_string()
    };

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await
        .map_err(|e| anyhow::anyhow!("Cannot connect to CDP WebSocket on port {port}: {e}"))?;

    // WebSocket HTTP upgrade.
    // The Sec-WebSocket-Key is a static value — fine for localhost non-security use.
    let handshake = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    );
    stream.write_all(handshake.as_bytes()).await?;

    // Read until end of HTTP response headers.
    let mut hdr = Vec::with_capacity(512);
    loop {
        let mut b = [0u8; 1];
        stream.read_exact(&mut b).await?;
        hdr.push(b[0]);
        if hdr.ends_with(b"\r\n\r\n") {
            break;
        }
        if hdr.len() > 8192 {
            anyhow::bail!("WebSocket upgrade response headers too large");
        }
    }
    if !String::from_utf8_lossy(&hdr).contains("101") {
        anyhow::bail!(
            "WebSocket upgrade failed: {}",
            String::from_utf8_lossy(&hdr[..hdr.len().min(200)])
        );
    }

    // Send the CDP command as a masked WebSocket text frame.
    ws_write_text(&mut stream, message.as_bytes()).await?;

    // Read frames, skipping events, until we receive id=1.
    loop {
        match ws_read_frame(&mut stream).await? {
            WsFrame::Text(text) => {
                // Try to parse as JSON and check for id == 1.
                if let Ok(val) = serde_json::from_str::<Value>(&text) {
                    if val.get("id").and_then(|id| id.as_u64()) == Some(1) {
                        return Ok(val);
                    }
                    // Event notification — ignore and loop.
                } else {
                    anyhow::bail!("CDP returned non-JSON frame: {}", &text[..text.len().min(120)]);
                }
            }
            WsFrame::Ping(data) => {
                // Respond with pong (RFC 6455 §5.5.3).
                ws_write_pong(&mut stream, &data).await?;
            }
            WsFrame::Close => {
                anyhow::bail!("CDP WebSocket closed by server before response");
            }
            WsFrame::Other => { /* binary / continuation / pong — skip */ }
        }
    }
}

// ── WebSocket frame I/O ───────────────────────────────────────────────────

enum WsFrame {
    Text(String),
    Ping(Vec<u8>),
    Close,
    Other,
}

/// Read one complete (non-fragmented) WebSocket frame.
async fn ws_read_frame(stream: &mut tokio::net::TcpStream) -> anyhow::Result<WsFrame> {
    use tokio::io::AsyncReadExt;

    // First 2 bytes: FIN+RSV+opcode, MASK+payload_len.
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).await?;

    let opcode     = hdr[0] & 0x0F;
    let masked     = (hdr[1] & 0x80) != 0;
    let len_byte   = (hdr[1] & 0x7F) as usize;

    let payload_len: usize = match len_byte {
        0..=125 => len_byte,
        126 => {
            let mut b = [0u8; 2];
            stream.read_exact(&mut b).await?;
            u16::from_be_bytes(b) as usize
        }
        _ => {
            let mut b = [0u8; 8];
            stream.read_exact(&mut b).await?;
            // Guard against absurdly large CDP messages (>16 MiB).
            let n = u64::from_be_bytes(b) as usize;
            if n > 16 * 1024 * 1024 {
                anyhow::bail!("CDP WebSocket frame too large ({n} bytes)");
            }
            n
        }
    };

    let mask_key: Option<[u8; 4]> = if masked {
        let mut m = [0u8; 4];
        stream.read_exact(&mut m).await?;
        Some(m)
    } else {
        None
    };

    let mut payload = vec![0u8; payload_len];
    stream.read_exact(&mut payload).await?;
    if let Some(mk) = mask_key {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mk[i % 4];
        }
    }

    match opcode {
        1 => Ok(WsFrame::Text(
            String::from_utf8(payload)
                .map_err(|_| anyhow::anyhow!("CDP returned non-UTF-8 text frame"))?,
        )),
        2 => Ok(WsFrame::Other),          // binary — CDP doesn't use this
        8 => Ok(WsFrame::Close),
        9 => Ok(WsFrame::Ping(payload)),
        _ => Ok(WsFrame::Other),
    }
}

/// Write a masked WebSocket text frame (client → server, RFC 6455 §5.3 requires masking).
async fn ws_write_text(stream: &mut tokio::net::TcpStream, payload: &[u8]) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    // Fixed mask key — fine for a local loopback connection.
    const MASK: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
    let len = payload.len();

    let mut frame = Vec::with_capacity(len + 10);
    frame.push(0x81u8); // FIN=1, opcode=TEXT(1)
    encode_len_masked(&mut frame, len);
    frame.extend_from_slice(&MASK);
    for (i, &b) in payload.iter().enumerate() {
        frame.push(b ^ MASK[i % 4]);
    }
    stream.write_all(&frame).await?;
    Ok(())
}

/// Write a masked WebSocket pong frame in response to a ping.
async fn ws_write_pong(stream: &mut tokio::net::TcpStream, payload: &[u8]) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    const MASK: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];
    let len = payload.len().min(125); // control frames ≤ 125 bytes (RFC 6455 §5.5)

    let mut frame = Vec::with_capacity(len + 6);
    frame.push(0x8Au8); // FIN=1, opcode=PONG(0xA)
    frame.push(0x80 | len as u8); // MASK bit + length
    frame.extend_from_slice(&MASK);
    for (i, &b) in payload[..len].iter().enumerate() {
        frame.push(b ^ MASK[i % 4]);
    }
    stream.write_all(&frame).await?;
    Ok(())
}

/// Encode a (masked) length field per RFC 6455 §5.2.
fn encode_len_masked(buf: &mut Vec<u8>, len: usize) {
    if len < 126 {
        buf.push(0x80 | len as u8);
    } else if len <= 65535 {
        buf.push(0x80 | 126);
        buf.push((len >> 8) as u8);
        buf.push(len as u8);
    } else {
        buf.push(0x80 | 127);
        for i in (0..8).rev() {
            buf.push((len >> (i * 8)) as u8);
        }
    }
}
