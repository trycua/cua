//! MCP streamable HTTP: `POST /mcp` with a JSON-RPC body, answered with
//! `application/json` (no server-initiated stream, so `GET /mcp` is 405).
//!
//! `initialize` mints an `Mcp-Session-Id`; later requests must carry it
//! (404 for an unknown one, which tells the client to re-initialize), and
//! `DELETE /mcp` ends it. An optional bearer protects the endpoint; the
//! daemon binds it to loopback with its own token.

use super::McpServer;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Session header.
pub const SESSION_HEADER: &str = "mcp-session-id";
/// Largest accepted request body (16 MiB).
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    server: McpServer,
    bearer: Option<String>,
    sessions: Arc<Mutex<HashSet<String>>>,
}

/// The `/mcp` router.
pub fn router(server: McpServer, bearer: Option<String>) -> axum::Router {
    let state = AppState {
        server,
        bearer,
        sessions: Arc::new(Mutex::new(HashSet::new())),
    };
    axum::Router::new()
        .route(
            cua_proto::metadata::MCP_PATH,
            post(handle_post)
                .get(method_not_allowed)
                .delete(handle_delete),
        )
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// Serves `router` on `listener` until the future is dropped.
pub async fn serve(
    server: McpServer,
    listener: tokio::net::TcpListener,
    bearer: Option<String>,
) -> std::io::Result<()> {
    axum::serve(listener, router(server, bearer)).await
}

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    match &state.bearer {
        None => true,
        Some(token) => headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|got| constant_time_eq(got.as_bytes(), token.as_bytes())),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        "this server sends no server-initiated stream",
    )
        .into_response()
}

async fn handle_delete(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if state.sessions.lock().expect("sessions").remove(&id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn handle_post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "missing or wrong bearer").into_response();
    }
    let message: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(super::error(
                    Value::Null,
                    super::codes::PARSE_ERROR,
                    &e.to_string(),
                )),
            )
                .into_response();
        }
    };
    let is_initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut new_session = None;
    if is_initialize {
        let id = format!("{:032x}", rand::random::<u128>());
        state.sessions.lock().expect("sessions").insert(id.clone());
        new_session = Some(id);
    } else if let Some(id) = &session
        && !state.sessions.lock().expect("sessions").contains(id)
    {
        return (
            StatusCode::NOT_FOUND,
            "unknown Mcp-Session-Id; initialize again",
        )
            .into_response();
    }
    match state.server.handle(message).await {
        None => StatusCode::ACCEPTED.into_response(),
        Some(response) => {
            let mut r = axum::Json(response).into_response();
            if let Some(id) = new_session
                && let Ok(v) = id.parse()
            {
                r.headers_mut().insert(SESSION_HEADER, v);
            }
            r
        }
    }
}
