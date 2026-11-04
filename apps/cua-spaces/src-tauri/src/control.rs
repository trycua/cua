// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Loopback control server: how `cua daemon mcp` (and any other Spaces host
//! using `cua_spaces::operator::ControlServerDisplay`) asks this app to
//! present a Space on the operator's desktop: pin it as picture-in-picture,
//! open its viewer, or stream one of its windows into a native window.
//!
//! That is UI, so it stays in the app; everything headless (hotspot,
//! streams, files) lives in the SDK. Binds `127.0.0.1:<ephemeral>`, writes
//! `{port, token}` to `<cua home>/spaces-control.json` (mode 0600) and
//! requires `Bearer <token>` on every request.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::core::AppCore;

/// Spawn the control server on Tauri's async runtime (best-effort; a bind
/// failure only disables MCP-driven presentation, never the app).
pub fn spawn(app: AppHandle, core: Arc<AppCore>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = serve(app, core).await {
            eprintln!("[cua-spaces] control server disabled: {error}");
        }
    });
}

/// `<cua home>/spaces-control.json`, where `ControlServerDisplay` looks.
pub fn control_file(home: &std::path::Path) -> PathBuf {
    home.join("spaces-control.json")
}

/// Writes `{port, token}` with owner-only permissions.
pub fn write_control_file(path: &std::path::Path, port: u16, token: &str) -> Result<(), String> {
    let json = serde_json::json!({ "port": port, "token": token }).to_string();
    // Owner-only from creation: the token is never readable by others.
    cua_daemon::write_private(path, (json + "\n").as_bytes()).map_err(|e| e.to_string())
}

async fn serve(app: AppHandle, core: Arc<AppCore>) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = cua_daemon::random_token();
    write_control_file(&control_file(core.home()), port, &token)?;
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        let app = app.clone();
        let core = core.clone();
        let token = token.clone();
        tauri::async_runtime::spawn(async move {
            let _ = handle(stream, app, core, token).await;
        });
    }
}

/// Whether an `Authorization` header value carries `token`, compared in
/// constant time.
fn bearer_matches(value: &str, token: &str) -> bool {
    let expected = format!("Bearer {token}");
    let (a, b) = (value.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Parsed HTTP/1.1 request: method, path, and the (already length-bounded) body.
struct Request {
    method: String,
    path: String,
    authorized: bool,
    body: String,
}

async fn read_request(stream: &mut TcpStream, expected_token: &str) -> Result<Request, String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    // Read until end-of-headers.
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed before headers".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            return Err("request headers too large".into());
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    let mut authorized = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            } else if name == "authorization" {
                authorized = bearer_matches(value, expected_token);
            }
        }
    }

    // Body: whatever's already buffered past the headers, plus the rest.
    let mut body = buf[header_end + 4..].to_vec();
    if content_length > 1024 * 1024 {
        return Err("request body too large".into());
    }
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Ok(Request {
        method,
        path,
        authorized,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

async fn respond(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())
}

async fn handle(
    mut stream: TcpStream,
    app: AppHandle,
    core: Arc<AppCore>,
    token: String,
) -> Result<(), String> {
    let request = read_request(&mut stream, &token).await?;
    if !request.authorized {
        return respond(&mut stream, 401, "{\"error\":\"unauthorized\"}").await;
    }
    let (status, body) = route(&request, &app, &core).await;
    respond(&mut stream, status, &body).await
}

async fn route(request: &Request, app: &AppHandle, core: &Arc<AppCore>) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/pip/pin") => window_action(app, core, &request.body, WindowAction::Pin).await,
        ("POST", "/pip/unpin") => match field(&request.body, "space_id") {
            Some(space_id) => {
                let space_id = canonical(core, &space_id);
                let configs = app.state::<crate::viewer_windows::ViewerConfigs>();
                match crate::viewer_windows::unpin_space_pip_impl(app, &configs, &space_id) {
                    Ok(()) => (200, "{\"ok\":true}".into()),
                    Err(error) => (500, error_json(&error)),
                }
            }
            None => (400, "{\"error\":\"space_id required\"}".into()),
        },
        ("POST", "/viewer/open") => {
            window_action(app, core, &request.body, WindowAction::Viewer).await
        }
        ("POST", "/window/stream") => stream_window_action(app, core, &request.body).await,
        _ => (404, "{\"error\":\"not found\"}".into()),
    }
}

/// The body of `/window/stream` (`cua_spaces::operator::WindowStreamRequest`).
#[derive(Debug, serde::Deserialize, PartialEq)]
pub struct WindowStreamBody {
    pub space_id: String,
    pub window_id: String,
    #[serde(default)]
    pub app_name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub media_url: String,
    #[serde(default)]
    pub media_session_id: String,
    #[serde(default)]
    pub replica: bool,
}

async fn stream_window_action(app: &AppHandle, core: &Arc<AppCore>, body: &str) -> (u16, String) {
    let body: WindowStreamBody = match serde_json::from_str(body) {
        Ok(b) => b,
        Err(e) => return (400, error_json(&format!("invalid body: {e}"))),
    };
    let space = window_request(core, &body.space_id);
    let media = (!body.media_url.is_empty()).then_some((body.media_url, body.media_session_id));
    let configs = app.state::<crate::viewer_windows::ViewerConfigs>();
    match crate::viewer_windows::open_winone_impl(
        app,
        &configs,
        space,
        &body.window_id,
        &body.app_name,
        &body.title,
        body.replica,
        media,
    )
    .await
    {
        Ok(()) => (200, "{\"ok\":true}".into()),
        Err(error) => (500, error_json(&error)),
    }
}

fn field(body: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get(key).and_then(|s| s.as_str()).map(String::from))
}

/// The canonical Space id (`local:<name>`, ...) for any accepted spelling (falls back to
/// the input, so an unknown id still names a window label).
fn canonical(core: &AppCore, space: &str) -> String {
    core.spaces()
        .resolve(space)
        .map(|id| id.to_string())
        .unwrap_or_else(|_| space.to_string())
}

/// A viewer request for a registered Space (name from the registry).
pub fn window_request(core: &AppCore, space: &str) -> crate::viewer_windows::SpaceWindowRequest {
    let id = canonical(core, space);
    let name = core
        .spaces()
        .list()
        .ok()
        .and_then(|l| {
            l.into_iter()
                .find(|i| i.id == id)
                .map(|i| crate::core::display_name(&i.id, &i.name))
        })
        .unwrap_or_else(|| "Space".into());
    crate::viewer_windows::SpaceWindowRequest {
        id,
        name,
        controller: Some("agent".into()),
        os: None,
    }
}

enum WindowAction {
    Pin,
    Viewer,
}

async fn window_action(
    app: &AppHandle,
    core: &Arc<AppCore>,
    body: &str,
    action: WindowAction,
) -> (u16, String) {
    let Some(space_id) = field(body, "space_id") else {
        return (400, "{\"error\":\"space_id required\"}".into());
    };
    let request = window_request(core, &space_id);
    let configs = app.state::<crate::viewer_windows::ViewerConfigs>();
    let result = match action {
        WindowAction::Pin => crate::viewer_windows::pin_space_pip_impl(app, &configs, request),
        WindowAction::Viewer => {
            crate::viewer_windows::open_space_window_impl(app, &configs, request)
        }
    };
    match result {
        Ok(()) => (200, "{\"ok\":true}".into()),
        Err(error) => (500, error_json(&error)),
    }
}

fn error_json(message: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "error": message })).unwrap_or_default()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mcp_window_stream_request_deserializes() {
        // Exactly what `cua_spaces::operator::ControlServerDisplay` posts.
        let req = cua_spaces::operator::WindowStreamRequest {
            space_id: "space://direct/127.0.0.1:3211".into(),
            window_id: "0x1f".into(),
            app_name: "Firefox".into(),
            title: "Home".into(),
            media_url: "ws://127.0.0.1:3211/media?ticket=t".into(),
            media_session_id: "m1".into(),
        };
        let body: WindowStreamBody =
            serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(body.window_id, "0x1f");
        assert_eq!(body.media_url, "ws://127.0.0.1:3211/media?ticket=t");
        assert!(!body.replica);
    }

    #[test]
    fn bearer_matching_is_exact() {
        assert!(bearer_matches("Bearer tok", "tok"));
        assert!(!bearer_matches("Bearer to", "tok"));
        assert!(!bearer_matches("Bearer tokk", "tok"));
        assert!(!bearer_matches("bearer tok", "tok"));
        assert!(!bearer_matches("", "tok"));
    }

    #[test]
    fn the_control_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = control_file(dir.path());
        write_control_file(&path, 4242, "tok").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["port"], 4242);
        assert_eq!(v["token"], "tok");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
