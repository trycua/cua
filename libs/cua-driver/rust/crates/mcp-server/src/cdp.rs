//! Cross-platform CDP (Chrome DevTools Protocol) helper.
//!
//! A self-contained `Runtime.evaluate` client that speaks raw TCP — no
//! external HTTP/WS crates. Used by the cross-platform `page` tool's
//! `execute_javascript` action on Windows/Linux (and by the macOS Electron
//! backend, which retains its own copy under `platform-macos/src/browser/`
//! that predates this helper).
//!
//! Flow:
//!  1. HTTP GET http://127.0.0.1:{cdp_port}/json  → discover page WS URLs.
//!  2. WebSocket connect to the first page URL.
//!  3. Send Runtime.evaluate with userGesture=true.
//!  4. Read frames until we receive the response with id=1.

use serde_json::Value;

/// Evaluate `expression` in the first page tab exposed by a Chromium-family
/// browser listening on `port` (the value passed to `--remote-debugging-port`).
///
/// Returns the formatted result (or `exception: ...` if the evaluated code
/// raised). Caller is responsible for prefixing the human-readable label
/// (e.g. `"cdp.runtime.evaluate.user_gesture: ..."`).
pub async fn evaluate(
    port: u16,
    expression: &str,
    await_promise: bool,
) -> anyhow::Result<String> {
    let response = cdp_evaluate(port, expression, await_promise).await?;
    Ok(format_cdp_result(&response))
}

// ── High-level evaluate ───────────────────────────────────────────────────

async fn cdp_evaluate(
    port: u16,
    expression: &str,
    await_promise: bool,
) -> anyhow::Result<Value> {
    // Wrap the /json discovery in its own timeout — `cdp_list_pages` does an
    // unbounded `read_to_end`, and a half-open localhost socket (browser
    // mid-shutdown, firewall mismatch, port stolen by another process) would
    // otherwise hang us forever. 10 s is generous for a localhost HTTP roundtrip.
    let pages = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        cdp_list_pages(port),
    )
    .await
    .map_err(|_| anyhow::anyhow!("CDP /json discovery on port {port} timed out after 10 s"))??;
    if pages.is_empty() {
        anyhow::bail!("No CDP page tabs found on port {port}");
    }

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

    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        cdp_ws_call(port, &ws_url, &cmd.to_string()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("CDP evaluate timed out after 30 s"))?
}

fn format_cdp_result(response: &Value) -> String {
    if let Some(exc) = response
        .get("result")
        .and_then(|r| r.get("exceptionDetails"))
    {
        let msg = exc
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| exc.to_string());
        return format!("exception: {msg}");
    }

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

async fn cdp_list_pages(port: u16) -> anyhow::Result<Vec<Value>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
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
        .ok_or_else(|| anyhow::anyhow!("Invalid HTTP response from CDP"))?
        + 4;

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

async fn cdp_ws_call(port: u16, ws_url: &str, message: &str) -> anyhow::Result<Value> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let path: String = if let Some(rest) = ws_url.strip_prefix("ws://") {
        rest.splitn(2, '/')
            .nth(1)
            .map(|p| format!("/{p}"))
            .unwrap_or_else(|| "/".into())
    } else {
        ws_url.to_string()
    };

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| anyhow::anyhow!("Cannot connect to CDP WebSocket on port {port}: {e}"))?;

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

    ws_write_text(&mut stream, message.as_bytes()).await?;

    loop {
        match ws_read_frame(&mut stream).await? {
            WsFrame::Text(text) => {
                if let Ok(val) = serde_json::from_str::<Value>(&text) {
                    if val.get("id").and_then(|id| id.as_u64()) == Some(1) {
                        return Ok(val);
                    }
                } else {
                    anyhow::bail!(
                        "CDP returned non-JSON frame: {}",
                        &text[..text.len().min(120)]
                    );
                }
            }
            WsFrame::Ping(data) => {
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

async fn ws_read_frame(stream: &mut tokio::net::TcpStream) -> anyhow::Result<WsFrame> {
    use tokio::io::AsyncReadExt;

    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).await?;

    let opcode = hdr[0] & 0x0F;
    let masked = (hdr[1] & 0x80) != 0;
    let len_byte = (hdr[1] & 0x7F) as usize;

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
        1 => Ok(WsFrame::Text(String::from_utf8(payload).map_err(|_| {
            anyhow::anyhow!("CDP returned non-UTF-8 text frame")
        })?)),
        2 => Ok(WsFrame::Other),
        8 => Ok(WsFrame::Close),
        9 => Ok(WsFrame::Ping(payload)),
        _ => Ok(WsFrame::Other),
    }
}

async fn ws_write_text(stream: &mut tokio::net::TcpStream, payload: &[u8]) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    const MASK: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
    let len = payload.len();

    let mut frame = Vec::with_capacity(len + 10);
    frame.push(0x81u8);
    encode_len_masked(&mut frame, len);
    frame.extend_from_slice(&MASK);
    for (i, &b) in payload.iter().enumerate() {
        frame.push(b ^ MASK[i % 4]);
    }
    stream.write_all(&frame).await?;
    Ok(())
}

async fn ws_write_pong(stream: &mut tokio::net::TcpStream, payload: &[u8]) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    const MASK: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];
    let len = payload.len().min(125);

    let mut frame = Vec::with_capacity(len + 6);
    frame.push(0x8Au8);
    frame.push(0x80 | len as u8);
    frame.extend_from_slice(&MASK);
    for (i, &b) in payload[..len].iter().enumerate() {
        frame.push(b ^ MASK[i % 4]);
    }
    stream.write_all(&frame).await?;
    Ok(())
}

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
