// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Streamable-HTTP MCP transport that serves cua-driver's OWN MCP over this
//! daemon's embedded cua-driver registry.
//!
//! This is how an agent drives the window it is streaming without a bespoke
//! routing layer: it talks to cua-driver's real MCP (`tools/list` + `tools/call`
//! via the shared `cua_driver_core::server::handle_request`), served here by
//! cua-spacesd. Because the tools run in THIS process's cua-driver, their cursor moves
//! fire the cursor hook (see cua-spacesd-desktop `on_cursor_event`), so viewers
//! see the agent cursor as an overlay — with no synthesized routing.
//!
//! Bound on the rcdp listener's address (so a guest listening on every
//! interface lets a client outside it, the Spaces MCP on the host, reach it)
//! and gated the same way the rcdp WS is: a non-loopback bind requires a
//! bearer token, and a token-less loopback bind only answers local clients
//! (no cross-origin browser requests, JSON bodies only).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use cua_driver_core::protocol::{Request, Response};
use cua_driver_core::server::handle_request;
use cua_driver_core::tool::ToolRegistry;

#[derive(Clone)]
struct McpState {
    registry: Arc<ToolRegistry>,
    token: Option<String>,
}

/// Serve cua-driver's MCP on `addr` over `registry`. Returns once the listener
/// is bound; runs until the process exits.
pub async fn serve_mcp(
    addr: SocketAddr,
    registry: Arc<ToolRegistry>,
    token: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_mcp_listener(listener, registry, token).await
}

/// Serves an already bound listener (see [`serve_mcp`]).
pub async fn serve_mcp_listener(
    listener: tokio::net::TcpListener,
    registry: Arc<ToolRegistry>,
    token: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = token.filter(|t| !t.is_empty());
    if token.is_none() && !listener.local_addr()?.ip().is_loopback() {
        return Err("a non-loopback MCP listener requires a token".into());
    }
    let state = McpState { registry, token };
    let app = Router::new()
        .route("/mcp", post(dispatch))
        .with_state(state);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Why a request is refused before any tool runs, if it is.
fn refusal(token: Option<&str>, headers: &HeaderMap) -> Option<(StatusCode, &'static str)> {
    match token {
        Some(expected) => {
            let ok = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(cua_spacesd_server::auth::parse_bearer)
                .is_some_and(|got| {
                    cua_spacesd_server::util::constant_time_eq(got.as_bytes(), expected.as_bytes())
                });
            (!ok).then_some((StatusCode::UNAUTHORIZED, "invalid token"))
        }
        None => {
            // Token-less (loopback only): no web page may reach the tools,
            // neither through a foreign origin / host name nor through a
            // CORS-exempt form or text/plain POST.
            let uri = axum::http::Uri::from_static("/mcp");
            if let Some(reason) = cua_spacesd_server::browser_guard::refusal(headers, &uri) {
                return Some((StatusCode::FORBIDDEN, reason));
            }
            let json = headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
            (!json).then_some((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "content-type must be application/json",
            ))
        }
    }
}

async fn dispatch(
    State(state): State<McpState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // Same bearer-token gate as the rcdp WS. The Spaces MCP sends it as
    // `Authorization: Bearer <token>`.
    if let Some(refused) = refusal(state.token.as_deref(), &headers) {
        return refused.into_response();
    }
    let mut req: Request = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return json(&Response::parse_error()),
    };
    // Notifications (no id) — e.g. notifications/initialized — get an empty 200.
    if req.id.is_none() {
        return StatusCode::OK.into_response();
    }
    let id = req.id.take().unwrap_or(serde_json::Value::Null);
    // cua-driver now implements ToolProvider on ToolRegistry itself rather than
    // on Arc<ToolRegistry>, so hand over the inner registry.
    let response = handle_request(req, id, state.registry.as_ref()).await;
    json(&response)
}

fn json(resp: &Response) -> axum::response::Response {
    match serde_json::to_vec(resp) {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            bytes,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("serialize error: {e}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn token_is_required_when_configured() {
        let t = Some("s3cret");
        assert!(refusal(t, &headers(&[])).is_some());
        assert!(refusal(t, &headers(&[("authorization", "Bearer nope")])).is_some());
        assert!(refusal(t, &headers(&[("authorization", "bearer s3cret")])).is_none());
    }

    #[test]
    fn tokenless_loopback_refuses_browsers() {
        let json = ("content-type", "application/json");
        let host = ("host", "127.0.0.1:8801");
        assert!(refusal(None, &headers(&[json, host])).is_none());
        // CORS-exempt simple POSTs a page could send without a preflight.
        assert!(refusal(None, &headers(&[("content-type", "text/plain"), host])).is_some());
        assert!(refusal(None, &headers(&[host])).is_some());
        // Foreign origins and rebound host names.
        assert!(refusal(
            None,
            &headers(&[json, host, ("origin", "https://evil.example")])
        )
        .is_some());
        assert!(refusal(None, &headers(&[json, ("host", "evil.example:8801")])).is_some());
    }
}
