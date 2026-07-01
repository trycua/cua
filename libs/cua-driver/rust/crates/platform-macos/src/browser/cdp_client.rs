//! CDP (Chrome DevTools Protocol) client for Electron and other chromium-based apps.
//! Uses raw TCP for the HTTP /json endpoint and tokio-tungstenite for WebSocket.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

    /// Find a live CDP endpoint already listening on one of `pid`'s open TCP
    /// ports (e.g. because the app was launched with a debugging port).
    /// Unlike `find_page_target`, this doesn't guess at a fixed port list —
    /// it only sees ports the process itself opened. Callers that need to
    /// *activate* an inspector that isn't already listening (e.g. Electron's
    /// SIGUSR1 trick) do that themselves before falling back to this.
    ///
    /// Reuses `find_page_target`'s validation (parses the `/json` body and
    /// requires an actual "page" target) rather than the bare TCP-connect
    /// check `is_available` does — a port that merely accepts a connection
    /// and closes it (any number of unrelated local services do this) would
    /// otherwise pass as "available" with an empty, non-CDP response.
    ///
    /// Note: this specifically requires the classic `/json` HTTP discovery
    /// to work, so it won't find a port only reachable via the browser-level
    /// `devtools/browser` endpoint (see `CdpSession::connect`) — callers that
    /// already know the port (e.g. the user enabled `chrome://inspect`
    /// manually) don't need this lookup at all.
    pub async fn find_port_for_pid(pid: i32) -> Option<u16> {
        let ports = listening_ports(pid).await;
        Self::find_page_target(&ports).await
    }

    /// Evaluate JavaScript via CDP Runtime.evaluate on the first page target.
    /// Always opens a fresh connection — used only by the Electron inspector
    /// path (SIGUSR1-activated V8 inspector), which doesn't show the
    /// "Allow remote debugging?" confirmation the Chrome-toggle path does,
    /// so there's no reason to route it through `CdpSessionCache`.
    pub async fn evaluate(javascript: &str, port: u16) -> anyhow::Result<String> {
        let mut session = CdpSession::connect(port, None).await?;
        let obj = tokio::time::timeout(
            Duration::from_secs(10),
            session.call("Runtime.evaluate", serde_json::json!({
                "expression": javascript,
                "returnByValue": true,
                "awaitPromise": true
            })),
        ).await
        .map_err(|_| anyhow::anyhow!("CDP evaluate timed out after 10s"))??;

        parse_cdp_result(&obj)
    }
}

/// Caches one open `CdpSession` per port across calls, instead of opening a
/// fresh WebSocket connection every time.
///
/// This exists because Chrome's own "allow remote debugging" toggle
/// (`chrome://inspect/#remote-debugging`, the only way to reach a user's
/// real/default profile — see `resolve_cdp_port` in `tools/page.rs`) pops up
/// a live "Allow remote debugging?" confirmation on every *new* WebSocket
/// connection to the browser endpoint — confirmed by testing live,
/// repeatedly, against a real Chrome session: three separate
/// `insert_text`/`type_keystrokes` calls, each opening its own connection,
/// each got its own popup. Reusing one already-approved connection across
/// calls (re-attaching to a different tab on it via `Target.getTargets` +
/// `Target.attachToTarget` when needed, never opening a new socket) avoids
/// re-triggering that popup for anything but the very first call after a
/// fresh Chrome launch.
pub struct CdpSessionCache {
    sessions: AsyncMutex<HashMap<u16, Arc<AsyncMutex<CdpSession>>>>,
}

impl CdpSessionCache {
    pub fn new() -> Self {
        Self { sessions: AsyncMutex::new(HashMap::new()) }
    }

    async fn get_or_connect(&self, port: u16, target_url_contains: Option<&str>) -> anyhow::Result<Arc<AsyncMutex<CdpSession>>> {
        let mut map = self.sessions.lock().await;
        if let Some(existing) = map.get(&port) {
            return Ok(existing.clone());
        }
        let session = CdpSession::connect(port, target_url_contains).await?;
        let arc = Arc::new(AsyncMutex::new(session));
        map.insert(port, arc.clone());
        Ok(arc)
    }

    async fn evict(&self, port: u16) {
        self.sessions.lock().await.remove(&port);
    }

    /// Insert `text` at whatever currently holds DOM focus, via CDP's native
    /// `Input.insertText` — a single call, no synthesized keydown/char/keyup
    /// sequence. Chrome's renderer handles this the same way it handles an
    /// IME composition commit, which most rich-text editors (having to
    /// support CJK/IME input regardless) already treat as real input — so
    /// it lands more durably than a JS-level DOM write, at a fraction of the
    /// cost of `dispatch_keystrokes`. It does skip actual key events though,
    /// so page-level keydown/keyup handlers never fire; reach for
    /// `dispatch_keystrokes` instead if an editor's own keyboard shortcuts or
    /// per-keystroke validation need to see real keys.
    ///
    /// Caller must already have focused the target element.
    /// `target_url_contains`, when given, picks the page target whose URL
    /// contains this substring instead of just whichever tab is currently
    /// attached — a browser with more than one tab open has no other way to
    /// know which one the caller means (CDP target ids have no relation to
    /// a window_id).
    pub async fn insert_text(&self, text: &str, port: u16, target_url_contains: Option<&str>) -> anyhow::Result<()> {
        let arc = self.get_or_connect(port, target_url_contains).await?;
        let first_attempt = {
            let mut session = arc.lock().await;
            match session.ensure_target(port, target_url_contains).await {
                Ok(()) => do_insert_text(&mut session, text).await,
                Err(e) => Err(e),
            }
        };
        if first_attempt.is_ok() {
            return first_attempt;
        }

        // Stale/closed connection — evict and retry once against a fresh
        // session, so a dead cache entry self-heals instead of wedging
        // every future call.
        self.evict(port).await;
        let arc = self.get_or_connect(port, target_url_contains).await?;
        let mut session = arc.lock().await;
        session.ensure_target(port, target_url_contains).await?;
        do_insert_text(&mut session, text).await
    }

    /// Type `text` into whatever currently holds DOM focus, via per-character
    /// `Input.dispatchKeyEvent` (keyDown -> char -> keyUp). The `char`
    /// event's `text` field is what actually inserts the character;
    /// bracketing it with keyDown/keyUp gives the page's own keydown/keyup
    /// listeners a real event to see.
    ///
    /// This exists because rich-text editors (React/Draft.js-style
    /// contenteditable) reconcile their own state and can silently discard a
    /// one-shot DOM write (`execCommand`, `innerText =`) on the next render —
    /// a per-character keystroke stream is what those editors' own input
    /// pipeline actually observes as durable.
    ///
    /// Caller must already have focused the target element (e.g. via a prior
    /// click) — this dispatches to whatever element currently has focus, the
    /// same as a hardware keyboard would. `target_url_contains` — see
    /// `insert_text`.
    pub async fn dispatch_keystrokes(&self, text: &str, port: u16, target_url_contains: Option<&str>) -> anyhow::Result<()> {
        let arc = self.get_or_connect(port, target_url_contains).await?;
        let first_attempt = {
            let mut session = arc.lock().await;
            match session.ensure_target(port, target_url_contains).await {
                Ok(()) => do_dispatch_keystrokes(&mut session, text).await,
                Err(e) => Err(e),
            }
        };
        if first_attempt.is_ok() {
            return first_attempt;
        }

        self.evict(port).await;
        let arc = self.get_or_connect(port, target_url_contains).await?;
        let mut session = arc.lock().await;
        session.ensure_target(port, target_url_contains).await?;
        do_dispatch_keystrokes(&mut session, text).await
    }
}

impl Default for CdpSessionCache {
    fn default() -> Self { Self::new() }
}

async fn do_insert_text(session: &mut CdpSession, text: &str) -> anyhow::Result<()> {
    let obj = tokio::time::timeout(
        Duration::from_secs(10),
        session.call("Input.insertText", serde_json::json!({ "text": text })),
    ).await
    .map_err(|_| anyhow::anyhow!("CDP insertText timed out after 10s"))??;

    if let Some(err) = obj.get("error") {
        let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
        anyhow::bail!("CDP Input.insertText error: {msg}");
    }
    Ok(())
}

async fn do_dispatch_keystrokes(session: &mut CdpSession, text: &str) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        for ch in text.chars() {
            let key = ch.to_string();
            for event_type in ["keyDown", "char", "keyUp"] {
                // Only the `char` event carries `text`/`unmodifiedText` —
                // that's what actually inserts the character. Chrome
                // treats a non-empty `text` on `keyDown` as ALSO
                // producing input, so setting it on both double-types
                // every character (confirmed: "hello" -> "hheelllloo").
                let mut params = serde_json::json!({
                    "type": event_type,
                    "key": key,
                });
                if event_type == "char" {
                    params["text"] = serde_json::json!(key);
                    params["unmodifiedText"] = serde_json::json!(key);
                }
                let obj = session.call("Input.dispatchKeyEvent", params).await?;
                if let Some(err) = obj.get("error") {
                    let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
                    anyhow::bail!("CDP Input.dispatchKeyEvent error: {msg}");
                }
            }
            // Small gap between characters — a real keyboard doesn't emit
            // 36 events in the same tick, and some editors' autosave /
            // hashtag-parsing debounce keys off discrete keystrokes.
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        Ok(())
    }).await
    .map_err(|_| anyhow::anyhow!("CDP keystroke dispatch timed out after 30s"))?
}

/// An open CDP connection scoped to one page target, plus whatever session
/// routing that required.
///
/// Two ways to reach a page:
/// - **Classic** — the target's own `webSocketDebuggerUrl` from `/json`.
///   Every message goes straight to that page; no session id needed. This is
///   what a normal `--remote-debugging-port` launch gives you.
/// - **Browser-attached** — connect to the browser-level
///   `ws://host:port/devtools/browser` endpoint instead, `Target.getTargets`
///   to find a page, then `Target.attachToTarget{flatten:true}` to obtain a
///   `sessionId` that has to ride along on every subsequent message. This is
///   the path for a CDP port that doesn't serve `/json` at all — observed
///   with newer Chrome's in-browser "allow remote debugging" toggle for an
///   already-running profile, which opens the browser endpoint without the
///   classic HTTP discovery endpoints. Re-attaching to a *different* target
///   on this same connection (via `ensure_target`) is what lets
///   `CdpSessionCache` switch tabs without opening a new socket.
struct CdpSession {
    ws: WsStream,
    session_id: Option<String>,
    /// URL of whichever target we're currently attached to/connected to —
    /// used by `ensure_target` to decide whether a re-attach is needed.
    current_target_url: Option<String>,
    next_id: u64,
}

impl CdpSession {
    async fn connect(port: u16, target_url_contains: Option<&str>) -> anyhow::Result<Self> {
        match ws_url_for_page_target(port, target_url_contains).await {
            Ok((ws_url, target_url)) => {
                let (ws, _) = tokio_tungstenite::connect_async(&ws_url).await
                    .map_err(|e| anyhow::anyhow!("WebSocket connect failed: {e}"))?;
                Ok(Self { ws, session_id: None, current_target_url: Some(target_url), next_id: 1 })
            }
            Err(page_discovery_err) => {
                Self::connect_via_browser_endpoint(port, target_url_contains, page_discovery_err).await
            }
        }
    }

    async fn connect_via_browser_endpoint(
        port: u16,
        target_url_contains: Option<&str>,
        page_discovery_err: anyhow::Error,
    ) -> anyhow::Result<Self> {
        let browser_ws_url = format!("ws://127.0.0.1:{port}/devtools/browser");
        let (ws, _) = tokio_tungstenite::connect_async(&browser_ws_url).await.map_err(|e| {
            anyhow::anyhow!(
                "No /json page discovery on port {port} ({page_discovery_err}), and \
                 connecting to the browser-level endpoint also failed: {e}"
            )
        })?;
        let mut session = Self { ws, session_id: None, current_target_url: None, next_id: 1 };
        session.attach_to_target(port, target_url_contains).await?;
        Ok(session)
    }

    /// Make sure we're attached to the tab `target_url_contains` selects
    /// (or, with no hint, whatever we're already attached to — anything is
    /// fine). Re-attaching on an already-open browser-endpoint connection
    /// via `Target.getTargets`/`Target.attachToTarget` does NOT reopen the
    /// socket, so it doesn't re-trigger Chrome's confirmation popup —
    /// that's the whole reason this exists instead of always reconnecting.
    async fn ensure_target(&mut self, port: u16, target_url_contains: Option<&str>) -> anyhow::Result<()> {
        if self.session_id.is_some() {
            // Flattened browser-endpoint mode: always re-resolve + reattach
            // on the SAME connection (Target.getTargets/attachToTarget,
            // never a new socket, so it's cheap and doesn't reprompt).
            //
            // Deliberately does NOT skip this when `current_target_url`
            // already matches the hint — a brand-new tab can carry the
            // exact same URL as a tab that was just closed (e.g. reopening
            // the same compose page), and the cache has no way to tell
            // "same URL" apart from "same still-alive target" without
            // asking Chrome. Confirmed live: skipping this on a URL-only
            // match kept sending Input.insertText into a closed target's
            // dead session — no error, text just never appeared anywhere.
            self.attach_to_target(port, target_url_contains).await
        } else {
            // Classic mode is tied to one page's own websocket URL — there's
            // no in-place retarget, but this mode never shows the popup
            // (it's the dedicated-automation-profile route), so a full
            // reconnect here is cheap and harmless.
            *self = Self::connect(port, target_url_contains).await?;
            Ok(())
        }
    }

    async fn attach_to_target(&mut self, port: u16, target_url_contains: Option<&str>) -> anyhow::Result<()> {
        let targets = self.call("Target.getTargets", serde_json::json!({})).await?;
        let infos = targets["result"]["targetInfos"].as_array().cloned().unwrap_or_default();
        let pages: Vec<&serde_json::Value> = infos.iter()
            .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
            .collect();
        let target = pick_target(&pages, target_url_contains)
            .ok_or_else(|| anyhow::anyhow!("Target.getTargets returned no page target on port {port}"))?;
        let target_id = target.get("targetId").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Target.getTargets entry missing targetId"))?
            .to_owned();
        let target_url = target.get("url").and_then(|v| v.as_str()).unwrap_or("").to_owned();

        let attach = self.call(
            "Target.attachToTarget",
            serde_json::json!({ "targetId": target_id, "flatten": true }),
        ).await?;
        let session_id = attach["result"]["sessionId"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Target.attachToTarget did not return a sessionId"))?
            .to_owned();
        self.session_id = Some(session_id);
        self.current_target_url = Some(target_url);
        Ok(())
    }

    /// Send `method`/`params`, return the full response envelope once its
    /// `id` comes back (event frames and other in-flight ids are skipped).
    async fn call(&mut self, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;

        let mut request = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = &self.session_id {
            request["sessionId"] = serde_json::json!(sid);
        }

        self.ws.send(Message::Text(request.to_string().into())).await
            .map_err(|e| anyhow::anyhow!("WebSocket send failed: {e}"))?;

        loop {
            let msg = self.ws.next().await
                .ok_or_else(|| anyhow::anyhow!("WebSocket stream ended before response for id {id}"))??;
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => anyhow::bail!("WebSocket closed before response for id {id}"),
                _ => continue,
            };
            let obj: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("CDP response parse error: {e}"))?;
            // Skip CDP event frames (they have "method" key but no "id").
            if obj.get("method").is_some() { continue; }
            if obj.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return Ok(obj);
            }
        }
    }
}

/// Resolve the websocket debugger URL (and the target's own URL) for the
/// page target `target_url_contains` selects, via the classic `/json` HTTP
/// discovery. Errors (not just returns None) so `CdpSession::connect` can
/// report exactly why the classic path failed if the browser-endpoint
/// fallback also fails.
async fn ws_url_for_page_target(port: u16, target_url_contains: Option<&str>) -> anyhow::Result<(String, String)> {
    let json = http_get_json(port).await?;
    let targets: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| anyhow::anyhow!("CDP /json parse error: {e}"))?;

    let pages: Vec<&serde_json::Value> = targets.as_array()
        .map(|arr| arr.iter().filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page")).collect())
        .unwrap_or_default();

    let target = pick_target(&pages, target_url_contains)
        .ok_or_else(|| anyhow::anyhow!("No page target found on port {port}"))?;
    let ws_url = target.get("webSocketDebuggerUrl").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("No page target with webSocketDebuggerUrl found on port {port}"))?
        .to_owned();
    let target_url = target.get("url").and_then(|v| v.as_str()).unwrap_or("").to_owned();
    Ok((ws_url, target_url))
}

/// Pick the page target whose `url` contains `hint` (case-insensitive), or
/// fall back to the first page in `pages` if no hint is given or nothing
/// matches. Shared by both discovery paths (classic `/json` and
/// `Target.getTargets`) since a browser with more than one tab open is
/// otherwise picked non-deterministically — CDP target ids carry no
/// relationship to the caller's `window_id`.
fn pick_target<'a>(pages: &[&'a serde_json::Value], hint: Option<&str>) -> Option<&'a serde_json::Value> {
    if let Some(hint) = hint {
        let hint_lower = hint.to_ascii_lowercase();
        if let Some(t) = pages.iter().find(|t| {
            t.get("url")
                .and_then(|v| v.as_str())
                .is_some_and(|url| url.to_ascii_lowercase().contains(&hint_lower))
        }) {
            return Some(t);
        }
    }
    pages.first().copied()
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

/// Return TCP listening ports for a process via `lsof`.
pub(super) async fn listening_ports(pid: i32) -> Vec<u16> {
    let out = tokio::process::Command::new("lsof")
        .args([
            // `-a` ANDs the selection criteria together — without it, lsof
            // ORs `-p` with `-iTCP -sTCP:LISTEN`, returning every listening
            // port on the whole system in addition to this pid's fds.
            "-a",
            "-p", &pid.to_string(),
            "-iTCP", "-sTCP:LISTEN",
            "-Fn", "-P",
        ])
        .output()
        .await;

    let Ok(out) = out else { return vec![] };
    let text = String::from_utf8_lossy(&out.stdout);

    let mut ports = Vec::new();
    for line in text.lines() {
        // Lines starting with 'n' contain the address e.g. n*:9222 or n127.0.0.1:9222
        let trimmed = line.trim();
        if !trimmed.starts_with('n') { continue; }
        if let Some(colon_pos) = trimmed.rfind(':') {
            let port_str = &trimmed[colon_pos + 1..];
            if let Ok(p) = port_str.parse::<u16>() {
                ports.push(p);
            }
        }
    }
    ports
}

async fn http_get_json(port: u16) -> anyhow::Result<String> {
    // The whole round-trip is timeout-guarded as one unit, not just the
    // connect — see the read loop below for why a bare `read_to_end` isn't
    // safe here.
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await
            .map_err(|e| anyhow::anyhow!("TCP connect error: {e}"))?;

        let req = format!("GET /json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await?;

        // Chrome's CDP HTTP server ignores our `Connection: close` request
        // and keeps the socket open, replying with `Content-Length` instead
        // — so `read_to_end` (which waits for the peer to close, i.e. EOF)
        // hangs forever. Read incrementally until the header block is
        // complete, parse `Content-Length`, then read exactly that many
        // body bytes.
        let mut buf = Vec::new();
        let header_end = loop {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                anyhow::bail!("connection closed before HTTP headers completed");
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };

        let headers = String::from_utf8_lossy(&buf[..header_end]);
        let content_length: usize = headers
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.trim().eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap_or(0))
            })
            .unwrap_or(0);

        let mut body = buf[header_end..].to_vec();
        while body.len() < content_length {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).await?;
            if n == 0 { break; }
            body.extend_from_slice(&chunk[..n]);
        }
        if content_length > 0 && body.len() > content_length {
            body.truncate(content_length);
        }
        Ok(String::from_utf8_lossy(&body).to_string())
    }).await
    .map_err(|_| anyhow::anyhow!("HTTP request to CDP port {port} timed out after 5s"))?
}
