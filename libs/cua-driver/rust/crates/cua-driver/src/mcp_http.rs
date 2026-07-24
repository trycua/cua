//! Streamable-HTTP MCP transport for the daemon (trycua/cua#1799).
//!
//! Why: over **stdio**, one `cua-driver mcp` process is a single pipe, so all of
//! a client's tool calls — including those from multiple subagents — serialize.
//! The daemon itself is already concurrent (a task per connection). This HTTP
//! front-end lets each agent open its **own** connection to the shared daemon:
//! per-connection FIFO ordering keeps a single agent's ordered calls correct,
//! while distinct connections run truly in parallel. That parallelism is sound
//! because the per-`(pid, window_id)` element cache + per-session cursor make
//! concurrent cross-connection actions non-colliding (see the session-identity
//! work in this PR).
//!
//! Minimal hand-rolled HTTP/1.1 — no new dependency, mirroring how the daemon
//! already hand-rolls its UDS line protocol. `POST` with a JSON-RPC body → the
//! shared MCP dispatch (`cua_driver_core::server::handle_request`) → an
//! `application/json` JSON-RPC response. Each TCP connection is its own task, so
//! N clients run concurrently. (SSE streaming + transport-level session headers
//! are a follow-up; tool calls are request/response, so `application/json`
//! suffices.) Loopback-only — a local automation surface, not a public endpoint.

use std::net::SocketAddr;
use std::sync::Arc;

use cua_driver_core::protocol::{Request, Response};
use cua_driver_core::server::{
    handle_request, session_tool_context, tool_observation_timer, StdioExecutionPath,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

const MCP_HTTP_TOKEN_ENV: &str = "CUA_DRIVER_RS_MCP_HTTP_TOKEN";

/// Resolve the configured HTTP MCP port: `CUA_DRIVER_RS_MCP_HTTP_PORT` (> 0), or
/// `None` (disabled — the daemon spawns the listener only when this is set).
pub fn configured_port() -> anyhow::Result<Option<u16>> {
    configured_port_from(std::env::var_os("CUA_DRIVER_RS_MCP_HTTP_PORT"))
}

fn configured_port_from(value: Option<std::ffi::OsString>) -> anyhow::Result<Option<u16>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| anyhow::anyhow!("CUA_DRIVER_RS_MCP_HTTP_PORT must be valid UTF-8"))?;
    let port = value.parse::<u16>().map_err(|_| {
        anyhow::anyhow!("CUA_DRIVER_RS_MCP_HTTP_PORT must be an integer from 1 to 65535")
    })?;
    if port == 0 {
        anyhow::bail!("CUA_DRIVER_RS_MCP_HTTP_PORT must be greater than zero");
    }
    Ok(Some(port))
}

/// Spawn the HTTP MCP listener bound to `127.0.0.1:port` (loopback only).
pub fn spawn(sdk: Arc<crate::sdk_adapter::SdkAdapter>, port: u16) -> anyhow::Result<()> {
    let token = Arc::<str>::from(configured_auth_token().map_err(anyhow::Error::msg)?);
    tokio::spawn(async move {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                info!("MCP HTTP transport listening on http://{addr}/mcp (one connection per agent → parallel)");
                loop {
                    match listener.accept().await {
                        Ok((stream, peer)) => {
                            let sdk = sdk.clone();
                            let token = token.clone();
                            tokio::spawn(async move {
                                if let Err(e) = serve_conn(stream, sdk, token).await {
                                    debug!(%peer, "MCP HTTP connection closed: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            warn!("MCP HTTP accept error: {e}");
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }
            Err(e) => warn!("MCP HTTP transport disabled — bind {addr} failed: {e}"),
        }
    });
    Ok(())
}

/// Handle one TCP connection: a keep-alive loop of HTTP requests. Requests on a
/// single connection stay FIFO-ordered (so one agent's ordered calls are safe);
/// parallelism comes from DISTINCT connections, each its own task.
async fn serve_conn(
    mut stream: TcpStream,
    sdk: Arc<crate::sdk_adapter::SdkAdapter>,
    token: Arc<str>,
) -> anyhow::Result<()> {
    loop {
        let Some(req) = read_http_request(&mut stream, &token).await? else {
            return Ok(()); // clean EOF
        };
        let keep_alive = req.keep_alive;
        if !req.authorized {
            write_http(
                &mut stream,
                401,
                br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32001,"message":"Authentication required"}}"#,
                false,
            )
            .await?;
            return Ok(());
        } else if !req.method.eq_ignore_ascii_case("POST") {
            write_http(
                &mut stream,
                405,
                br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Use POST /mcp with a JSON-RPC body"}}"#,
                keep_alive,
            )
            .await?;
        } else {
            match dispatch(&req.body, &sdk).await {
                Some(resp_json) => {
                    write_http(&mut stream, 200, resp_json.as_bytes(), keep_alive).await?
                }
                // Notification (no id): MCP wants 202 Accepted, no body.
                None => write_http(&mut stream, 202, b"", keep_alive).await?,
            }
        }
        // Honor the client's Connection: close (and HTTP/1.0 default) — close the
        // connection so a client reading until EOF doesn't hang. Parallelism comes
        // from distinct connections regardless of keep-alive.
        if !keep_alive {
            return Ok(());
        }
    }
}

/// Parse a JSON-RPC request body and dispatch via the shared MCP handler. Returns
/// `Some(json)` for a request, or `None` for a notification (no `id`). Applies the
/// caller-declared `session` identity so HTTP behaves identically to stdio.
async fn dispatch(body: &[u8], sdk: &Arc<crate::sdk_adapter::SdkAdapter>) -> Option<String> {
    let mut req: Request = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => return Some(serialize(&Response::parse_error())),
    };
    req.id.as_ref()?;
    let initialize_metadata = req.initialize_metadata();
    let session_context = session_tool_context(
        &req,
        |name| sdk.is_known_tool(name),
        cua_driver_core::session::SessionTransport::McpHttp,
    );
    let id = req.id.clone().unwrap_or(serde_json::Value::Null);
    apply_session_identity(&mut req);
    let timer = http_tool_observation_timer(&req, |name| sdk.is_known_tool(name));
    let response = handle_request(req, id, sdk.as_ref()).await;
    if let Some(timer) = timer {
        let outcome = timer.finish(&response);
        if let Some(context) = session_context {
            context.complete(&outcome);
        }
        crate::telemetry::capture_tool_completed(outcome, crate::telemetry::Transport::McpHttp);
    }
    if let Some(metadata) = initialize_metadata {
        crate::telemetry::capture_mcp_session_started(
            metadata,
            crate::telemetry::Transport::McpHttp,
        );
    }
    Some(serialize(&response))
}

fn http_tool_observation_timer(
    req: &Request,
    is_known_tool: impl Fn(&str) -> bool,
) -> Option<cua_driver_core::server::ToolObservationTimer> {
    tool_observation_timer(req, is_known_tool, StdioExecutionPath::DirectDaemon)
}

fn serialize(resp: &Response) -> String {
    serde_json::to_string(resp).unwrap_or_else(|e| {
        format!(r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#)
    })
}

/// Mirror an explicit `session` arg into `_session_id` (the per-session config /
/// recording key) and refresh its idle-TTL — the HTTP-side equivalent of
/// `serve.rs::apply_session_identity`. The agent cursor reads `session` directly
/// (so it already works); this keeps config + recording session-scoping
/// consistent across transports.
fn apply_session_identity(req: &mut Request) {
    let Some(params) = req.params.as_mut() else {
        return;
    };
    let Some(args) = params.get_mut("arguments").and_then(|a| a.as_object_mut()) else {
        return;
    };
    let session = args
        .get("session")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned());
    if let Some(sess) = session {
        args.entry("_session_id")
            .or_insert_with(|| serde_json::Value::String(sess.clone()));
        cua_driver_core::session::touch_session(&sess);
    }
}

/// One parsed HTTP/1.1 request.
struct HttpRequest {
    method: String,
    #[allow(dead_code)]
    path: String,
    body: Vec<u8>,
    /// Whether to keep the connection open after responding (HTTP/1.1 default;
    /// false if the client sent `Connection: close` or spoke HTTP/1.0).
    keep_alive: bool,
    authorized: bool,
}

/// Read one HTTP/1.1 request, or `None` on clean EOF. Minimal: request line +
/// headers until CRLFCRLF, then `Content-Length` bytes.
async fn read_http_request(
    stream: &mut TcpStream,
    token: &str,
) -> anyhow::Result<Option<HttpRequest>> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            return Ok(None); // EOF — peer closed
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > 64 * 1024 {
            anyhow::bail!("HTTP headers too large");
        }
    }
    let head_str = String::from_utf8_lossy(&head);
    let mut lines = head_str.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("/").to_owned();
    let version = parts.next().unwrap_or("HTTP/1.1");
    let mut content_length = 0usize;
    let mut keep_alive = version.eq_ignore_ascii_case("HTTP/1.1"); // 1.1 defaults to keep-alive
    let mut authorized = false;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            } else if k.eq_ignore_ascii_case("connection") {
                if v.eq_ignore_ascii_case("close") {
                    keep_alive = false;
                } else if v.eq_ignore_ascii_case("keep-alive") {
                    keep_alive = true;
                }
            } else if k.eq_ignore_ascii_case("authorization") {
                authorized = v.split_once(' ').is_some_and(|(scheme, candidate)| {
                    scheme.eq_ignore_ascii_case("bearer") && constant_time_equal(candidate, token)
                });
            }
        }
    }
    if content_length > 16 * 1024 * 1024 {
        anyhow::bail!("HTTP body too large");
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).await?;
    }
    Ok(Some(HttpRequest {
        method,
        path,
        body,
        keep_alive,
        authorized,
    }))
}

fn configured_auth_token() -> Result<String, String> {
    configured_auth_token_from(std::env::var_os(MCP_HTTP_TOKEN_ENV))
}

fn configured_auth_token_from(value: Option<std::ffi::OsString>) -> Result<String, String> {
    let token = value
        .ok_or_else(|| {
            format!(
                "{MCP_HTTP_TOKEN_ENV} must be set to a host-generated bearer token when the HTTP endpoint is enabled"
            )
        })?
        .into_string()
        .map_err(|_| format!("{MCP_HTTP_TOKEN_ENV} must be valid UTF-8"))?;
    if token.len() < 32
        || token.len() > 4096
        || token
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!(
            "{MCP_HTTP_TOKEN_ENV} must contain 32-4096 non-whitespace, non-control characters"
        ));
    }
    Ok(token)
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut difference = left.len() ^ right.len();
    let width = left.len().max(right.len());
    for index in 0..width {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

async fn write_http(
    stream: &mut TcpStream,
    status: u16,
    body: &[u8],
    keep_alive: bool,
) -> anyhow::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        401 => "Unauthorized",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let conn = if keep_alive { "keep-alive" } else { "close" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {conn}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_session_identity_mirrors_session_to_session_id() {
        let mut req: Request = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "click", "arguments": { "pid": 1, "session": "alpha" } }
        }))
        .unwrap();
        apply_session_identity(&mut req);
        let args = req.params.unwrap();
        let args = args.get("arguments").unwrap();
        assert_eq!(args.get("_session_id").unwrap(), "alpha");
        assert_eq!(args.get("session").unwrap(), "alpha");
    }

    #[test]
    fn apply_session_identity_noop_without_session() {
        let mut req: Request = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "list_apps", "arguments": {} }
        }))
        .unwrap();
        apply_session_identity(&mut req);
        let args = req.params.unwrap();
        assert!(args.get("arguments").unwrap().get("_session_id").is_none());
    }

    #[test]
    fn http_observes_tool_calls_but_not_initialize_requests() {
        let tool_call: Request = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "unknown", "arguments": {} }
        }))
        .unwrap();
        assert!(http_tool_observation_timer(&tool_call, |_| false).is_some());

        let initialize: Request = serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {}
        }))
        .unwrap();
        assert!(http_tool_observation_timer(&initialize, |_| false).is_none());
    }

    #[test]
    fn configured_port_parses_env() {
        assert_eq!(configured_port_from(None).unwrap(), None);
        assert_eq!(
            configured_port_from(Some("43123".into())).unwrap(),
            Some(43123)
        );
        assert!(configured_port_from(Some("0".into())).is_err());
        assert!(configured_port_from(Some("not-a-port".into())).is_err());
    }

    #[test]
    fn bearer_comparison_requires_the_exact_value() {
        assert!(constant_time_equal(
            "0123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdef"
        ));
        assert!(!constant_time_equal(
            "0123456789abcdef0123456789abcdee",
            "0123456789abcdef0123456789abcdef"
        ));
        assert!(!constant_time_equal(
            "0123456789abcdef0123456789abcdef-extra",
            "0123456789abcdef0123456789abcdef"
        ));
    }

    #[test]
    fn configured_auth_token_fails_closed() {
        assert!(configured_auth_token_from(None).is_err());
        assert!(configured_auth_token_from(Some("too-short".into())).is_err());
        assert!(
            configured_auth_token_from(Some("0123456789abcdef 123456789abcdef".into())).is_err()
        );
        assert_eq!(
            configured_auth_token_from(Some("0123456789abcdef0123456789abcdef".into())).unwrap(),
            "0123456789abcdef0123456789abcdef"
        );
    }

    #[tokio::test]
    async fn http_tools_list_is_the_sdk_inventory() {
        let _runtime_guard = crate::test_runtime_lock().lock().await;
        let sdk = crate::sdk_adapter::SdkAdapter::load(crate::build_driver_without_cursor())
            .await
            .expect("SDK adapter");
        let expected = sdk.tools_list();
        let response = dispatch(
            br#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}"#,
            &sdk,
        )
        .await
        .expect("JSON-RPC response");
        let response: serde_json::Value = serde_json::from_str(&response).expect("response JSON");
        assert_eq!(response["result"]["tools"], expected["tools"]);
        sdk.shutdown().await.expect("SDK shutdown");
    }
}
