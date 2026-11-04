//! In-process servers for tests (feature `testing`):
//!
//! - [`McpTestServer`]: an MCP server built with the official Rust SDK
//!   (rmcp, streamable HTTP at `/mcp`) whose tools return every kind of
//!   content block: `add` (text + `structuredContent`), `image` (a PNG of
//!   `bytes` length), `audio`, `resources` (a resource link and an embedded
//!   blob resource) and `fail` (a tool error).
//! - [`PipeTestServer`]: a raw HTTP server for pipe transparency: an SSE
//!   stream that never ends, an echo of method + headers + body, and large
//!   binary bodies.
//!
//! Both bind `127.0.0.1:0` and stop when dropped.

use axum::body::{Body, Bytes};
use axum::extract::Query;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// A PNG of exactly `len` bytes (a valid 1x1 image followed by padding in a
/// trailing chunk-free tail; decoders stop at IEND).
pub fn png_bytes(len: usize) -> Vec<u8> {
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let mut v = PNG_1X1.to_vec();
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    while v.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.push(x as u8);
    }
    v.truncate(len.max(PNG_1X1.len()));
    v
}

#[derive(Clone)]
struct TestTools;

fn tools() -> Value {
    let obj = |props: Value| json!({"type": "object", "properties": props});
    json!({"tools": [
        {"name": "add", "description": "Add two integers.",
         "inputSchema": obj(json!({"a": {"type": "integer"}, "b": {"type": "integer"}})),
         "outputSchema": obj(json!({"sum": {"type": "integer"}})),
         "annotations": {"readOnlyHint": true}},
        {"name": "image", "description": "A PNG of `bytes` bytes.",
         "inputSchema": obj(json!({"bytes": {"type": "integer"}}))},
        {"name": "audio", "description": "A short WAV clip.", "inputSchema": obj(json!({}))},
        {"name": "resources", "description": "A resource link and an embedded blob.",
         "inputSchema": obj(json!({}))},
        {"name": "fail", "description": "A tool error.", "inputSchema": obj(json!({}))},
    ]})
}

fn call(name: &str, args: &serde_json::Map<String, Value>) -> Value {
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    match name {
        "add" => {
            let a = args.get("a").and_then(Value::as_i64).unwrap_or(0);
            let b = args.get("b").and_then(Value::as_i64).unwrap_or(0);
            json!({"content": [{"type": "text", "text": (a + b).to_string()}],
                   "structuredContent": {"sum": a + b}})
        }
        "image" => {
            let n = args.get("bytes").and_then(Value::as_u64).unwrap_or(1024) as usize;
            json!({"content": [
                {"type": "text", "text": format!("{n} byte PNG")},
                {"type": "image", "data": b64(&png_bytes(n)), "mimeType": "image/png",
                 "annotations": {"audience": ["user"], "priority": 0.5},
                 "_meta": {"cua.test/source": "png_bytes"}},
            ]})
        }
        "audio" => json!({"content": [
            {"type": "audio", "data": b64(b"RIFF\x24\x00\x00\x00WAVEfmt "), "mimeType": "audio/wav"},
        ]}),
        "resources" => json!({"content": [
            {"type": "resource_link", "uri": "file:///srv/report.pdf", "name": "report.pdf",
             "mimeType": "application/pdf", "size": 12},
            {"type": "resource", "resource": {"uri": "mem://blob", "mimeType": "application/octet-stream",
             "blob": b64(&[0, 1, 2, 255])}},
            {"type": "resource", "resource": {"uri": "mem://text", "mimeType": "text/plain", "text": "hello"}},
        ]}),
        _ => {
            json!({"content": [{"type": "text", "text": format!("{name} failed")}], "isError": true})
        }
    }
}

impl ServerHandler for TestTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        serde_json::from_value(tools()).map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let args = request.arguments.clone().unwrap_or_default();
        let r: CallToolResult = serde_json::from_value(call(&request.name, &args))
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(r.into())
    }
}

/// The raw `tools/call` result JSON the test server returns for `name`.
pub fn expected_result(name: &str, args: Value) -> Value {
    call(
        name,
        args.as_object()
            .cloned()
            .as_ref()
            .unwrap_or(&Default::default()),
    )
}

/// A running server; stops when dropped.
pub struct TestServer {
    /// Base URL (`http://127.0.0.1:<port>`).
    pub url: String,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// The loopback port.
    pub fn port(&self) -> u16 {
        self.url
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(router: axum::Router) -> std::io::Result<TestServer> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(TestServer {
        url: format!("http://{addr}"),
        task,
    })
}

/// The rmcp test MCP server at `<prefix>/mcp`. `prefix` lets a test put it
/// behind an emulated gateway path (`""` for none).
pub struct McpTestServer;

impl McpTestServer {
    /// Starts it (legacy sessions on, so every revision rmcp speaks works).
    pub async fn start(prefix: &str) -> std::io::Result<TestServer> {
        let mut config = StreamableHttpServerConfig::default();
        config.sse_keep_alive = Some(Duration::from_secs(5));
        let service = StreamableHttpService::new(
            || Ok(TestTools),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let path = format!("{}/mcp", prefix.trim_end_matches('/'));
        serve(axum::Router::new().nest_service(&path, service)).await
    }

    /// Starts it behind cua-spacesd's `/mcp` auth: every request must carry
    /// `Bearer <token>` in `authorization` or `x-cua-env-authorization`,
    /// else `401`.
    pub async fn start_with_token(token: &str) -> std::io::Result<TestServer> {
        let mut config = StreamableHttpServerConfig::default();
        config.sse_keep_alive = Some(Duration::from_secs(5));
        let service = StreamableHttpService::new(
            || Ok(TestTools),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let expected = format!("Bearer {token}");
        let router =
            axum::Router::new()
                .nest_service("/mcp", service)
                .layer(axum::middleware::from_fn(
                    move |req: axum::extract::Request, next: axum::middleware::Next| {
                        let expected = expected.clone();
                        async move {
                            let authorized = ["authorization", "x-cua-env-authorization"]
                                .iter()
                                .any(|h| {
                                    req.headers().get(*h).and_then(|v| v.to_str().ok())
                                        == Some(expected.as_str())
                                });
                            if authorized {
                                next.run(req).await
                            } else {
                                (StatusCode::UNAUTHORIZED, "missing or invalid bearer token")
                                    .into_response()
                            }
                        }
                    },
                ));
        serve(router).await
    }
}

/// Raw endpoints for pipe tests:
///
/// - `GET /sse`: `text/event-stream` with `X-Accel-Buffering: no`, one event
///   every 50 ms, never ending.
/// - `ANY /echo`: `200`, body = the request body; response headers
///   `x-echo-method` and `x-echo-<name>` for every request header.
/// - `GET /big?bytes=N`: `N` bytes of [`png_bytes`], sent in 64 KiB chunks.
pub struct PipeTestServer;

impl PipeTestServer {
    /// Starts it under `prefix` (`""` for none).
    pub async fn start(prefix: &str) -> std::io::Result<TestServer> {
        let p = prefix.trim_end_matches('/');
        let router = axum::Router::new()
            .route(&format!("{p}/sse"), axum::routing::get(sse))
            .route(&format!("{p}/echo"), axum::routing::any(echo))
            .route(&format!("{p}/big"), axum::routing::get(big));
        serve(router).await
    }
}

async fn sse() -> Response {
    let stream = futures_util::stream::unfold(0u64, |n| async move {
        if n > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let ev = format!("id: {n}\nevent: message\ndata: {{\"n\":{n}}}\n\n");
        Some((Ok::<_, std::io::Error>(Bytes::from(ev)), n + 1))
    });
    (
        [
            ("content-type", "text/event-stream"),
            ("x-accel-buffering", "no"),
            ("cache-control", "no-cache"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

async fn echo(method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let mut resp = (StatusCode::OK, body).into_response();
    let h = resp.headers_mut();
    h.insert("x-echo-method", method.as_str().parse().unwrap());
    for (k, v) in &headers {
        if let Ok(name) = axum::http::HeaderName::try_from(format!("x-echo-{k}")) {
            h.append(name, v.clone());
        }
    }
    resp
}

async fn big(Query(q): Query<HashMap<String, usize>>) -> Response {
    let n = q.get("bytes").copied().unwrap_or(1 << 20);
    let data = Bytes::from(png_bytes(n));
    let chunks: Vec<Result<Bytes, std::io::Error>> = (0..data.len())
        .step_by(64 * 1024)
        .map(|i| Ok(data.slice(i..(i + 64 * 1024).min(data.len()))))
        .collect();
    (
        [("content-type", "application/octet-stream")],
        Body::from_stream(futures_util::stream::iter(chunks)),
    )
        .into_response()
}
