// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `/mcp`: streamable-HTTP MCP over the in-process cua-driver tool registry.
//!
//! JSON-RPC dispatch is cua-driver's own (`cua_driver_core::server`), so the
//! semantics (initialize, server/discover, tools/list, tools/call
//! authorization) are exactly those of `cua-driver mcp`. This module only
//! adds the HTTP transport: bearer auth (either token header),
//! `Mcp-Session-Id` sessions, 202 for notifications, and JSON-RPC batches.
//!
//! Driver identity: cua-driver keys a caller's session (its agent cursor,
//! config and recording) by the transport session. That is the
//! `Mcp-Session-Id` unless the client names the agent it acts for in
//! `X-Cua-Agent-Session` (cua agent runs send their run id). One agent then
//! owns one driver session, and so one cursor, however many MCP sessions its
//! harness opens: a harness initializes a new session whenever it respawns
//! its agent process or reconnects, and never deletes the old one, so keying
//! by MCP session drew one agent as several cursors.
//!
//! A run ends with `DELETE /mcp` carrying `X-Cua-Agent-Session` and no
//! `Mcp-Session-Id`: its driver session ends and its cursor leaves presence
//! (`LEAVE_REASON_RUN_ENDED`).
//!
//! JSON-RPC 2.0 rules this adapter keeps: every response to a request
//! carries that request's `id` (errors included: an unknown method is
//! `-32601`, and a `server/discover` the server cannot answer is a
//! spec-correct error, never an id-less `-32600`); notifications (no `id`)
//! never get a response; a body that is not a request object is `-32600`
//! with the `id` when one can be read, else `null`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use cua_driver_core::protocol::{Request as RpcRequest, Response as RpcResponse};
use cua_driver_core::server::{handle_request_with_transport_session, ToolProvider};

use crate::context::ServerContext;
use crate::http::mcp_envelope::{self, Envelopes};

/// Session header of the streamable-HTTP transport.
pub const SESSION_HEADER: &str = "mcp-session-id";

/// The agent (run) a client acts for. Every MCP session carrying the same
/// value shares one cua-driver session, and so one agent cursor.
pub const AGENT_SESSION_HEADER: &str = "x-cua-agent-session";

/// The cua-driver session of a request: `agent-<X-Cua-Agent-Session>` when
/// the header is a usable id (1 to 64 of `[A-Za-z0-9._:-]`), else the MCP
/// session.
fn driver_session(headers: &HeaderMap, mcp_session: &str) -> String {
    headers
        .get(AGENT_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 64
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
        })
        .map(|agent| format!("agent-{agent}"))
        .unwrap_or_else(|| mcp_session.to_owned())
}

/// End one driver session and everything it owns (labelled sessions, the
/// agent cursor overlay, presence, recording).
///
/// Dispatch keys the session by cua-driver's runtime-private id, so the end
/// goes through [`cua_driver_core::session::end_trusted_adapter_transport`];
/// ending the bare id would leave the in-guest overlay cursor drawn until it
/// idled out. The bare id is ended too, for hooks keyed by it (and a provider
/// that does not scope ids).
fn end_driver_session(driver: &str) {
    cua_driver_core::session::end_trusted_adapter_transport(driver);
    cua_driver_core::session::end_session(driver);
}

/// State of the `/mcp` route.
#[derive(Clone)]
pub struct McpState {
    /// Server context (auth).
    pub ctx: ServerContext,
    /// The tool registry.
    pub tools: Arc<dyn ToolProvider>,
    /// Typed cua-driver envelopes (`ai.cua.driver.envelopes`), unless disabled
    /// (see [`crate::http::mcp_envelope`]).
    pub envelopes: Option<Arc<Envelopes>>,
}

fn json(value: &impl serde::Serialize, session: Option<&str>) -> Response {
    let mut response = (
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_vec(value).unwrap_or_default(),
    )
        .into_response();
    if let Some(session) = session.and_then(|s| HeaderValue::from_str(s).ok()) {
        response.headers_mut().insert(SESSION_HEADER, session);
    }
    response
}

/// `-32600` for a message that is not a valid request object, carrying its
/// `id` when that can be read (JSON-RPC 2.0 section 5.1: `null` otherwise).
fn invalid_request(value: &serde_json::Value) -> RpcResponse {
    let id = value
        .get("id")
        .filter(|id| id.is_string() || id.is_number())
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    RpcResponse::error(id, -32600, "Invalid Request")
}

async fn dispatch_one(
    state: &McpState,
    value: serde_json::Value,
    session: &str,
    driver: &str,
) -> Option<RpcResponse> {
    let request: RpcRequest = match serde_json::from_value(value.clone()) {
        Ok(r) => r,
        Err(_) => return Some(invalid_request(&value)),
    };
    // Notifications (no `id`) are processed for their effect only and never
    // answered, whatever their method.
    let id = request.id.clone()?;
    if let Some(envelopes) = &state.envelopes {
        if request.method.starts_with(mcp_envelope::PREFIX) {
            return Some(envelopes.handle(&request, session, driver).await);
        }
    }
    let initialize = request.method == "initialize";
    // The request keeps its `id`: cua-driver's per-request MCP checks
    // (`server/discover`, modern `_meta`) read it from the request itself.
    let mut response =
        handle_request_with_transport_session(request, id, state.tools.as_ref(), driver).await;
    if initialize && state.envelopes.is_some() {
        mcp_envelope::advertise(&mut response);
    }
    Some(response)
}

/// Handles `/mcp` (POST requests; GET has no server-initiated stream).
pub async fn handle(
    State(state): State<McpState>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state.ctx.check_bearer(&headers) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    }
    state.ctx.auth().record_headers(&headers, "MCP");
    match method {
        Method::POST => {}
        Method::DELETE => {
            let mcp_session = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
            if let (Some(envelopes), Some(session)) = (&state.envelopes, mcp_session) {
                envelopes.close_session(session);
            }
            // The end of an agent run: `DELETE /mcp` with `X-Cua-Agent-Session`
            // and no `Mcp-Session-Id` ends that run's driver session, which
            // takes its cursor (overlay and presence) with it. Deleting one
            // MCP session of a run does not: a harness may still hold others.
            // A plain MCP client (no agent header) owns its driver session
            // through its MCP session alone, so deleting that ends it.
            let agent = driver_session(&headers, "");
            let ended = match (mcp_session, agent.is_empty()) {
                (None, false) => Some(agent),
                (Some(session), true) if !session.is_empty() => Some(session.to_owned()),
                _ => None,
            };
            if let Some(driver) = ended {
                end_driver_session(&driver);
            }
            return StatusCode::NO_CONTENT.into_response();
        }
        _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("mcp-{}", crate::util::random_id(12)));
    let driver = driver_session(&headers, &session);
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return json(&RpcResponse::parse_error(), None),
    };
    match parsed {
        // An empty batch is itself an invalid request (JSON-RPC 2.0 section 6).
        serde_json::Value::Array(items) if items.is_empty() => {
            json(&invalid_request(&serde_json::Value::Null), Some(&session))
        }
        serde_json::Value::Array(items) => {
            let mut responses = Vec::new();
            for item in items {
                if let Some(r) = dispatch_one(&state, item, &session, &driver).await {
                    responses.push(r);
                }
            }
            if responses.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                json(&responses, Some(&session))
            }
        }
        value => match dispatch_one(&state, value, &session, &driver).await {
            Some(response) => json(&response, Some(&session)),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, Method};
    use serde_json::{json, Value};

    use super::*;
    use crate::config::ServerConfig;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// Records the `_session_id` cua-driver keys the agent cursor by.
    #[derive(Default)]
    struct Sessions(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl ToolProvider for Sessions {
        fn tools_list(&self) -> Value {
            json!({"tools": []})
        }

        async fn invoke_tool(&self, _name: &str, arguments: Value) -> Result<Value, String> {
            let session = arguments["_session_id"].as_str().unwrap_or("").to_owned();
            self.0.lock().unwrap().push(session);
            Ok(json!({"content": [], "isError": false}))
        }
    }

    fn state(tools: Arc<Sessions>) -> McpState {
        McpState {
            ctx: ServerContext::new(ServerConfig::default(), Some(TOKEN.into())),
            tools,
            envelopes: None,
        }
    }

    /// `move_cursor` from one MCP session; returns the session the server
    /// assigned (or echoed).
    async fn move_cursor(
        state: &McpState,
        mcp_session: Option<&str>,
        agent: Option<&str>,
    ) -> String {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
        );
        if let Some(s) = mcp_session {
            headers.insert(SESSION_HEADER, HeaderValue::from_str(s).unwrap());
        }
        if let Some(a) = agent {
            headers.insert(AGENT_SESSION_HEADER, HeaderValue::from_str(a).unwrap());
        }
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "move_cursor", "arguments": {"x": 10, "y": 10}}});
        let response = handle(
            State(state.clone()),
            Method::POST,
            headers,
            Bytes::from(serde_json::to_vec(&body).unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        response.headers()[SESSION_HEADER]
            .to_str()
            .unwrap()
            .to_owned()
    }

    /// The regression: a harness opens a second MCP session (a respawned
    /// agent process, a reconnect) during one run. Both must act as the same
    /// driver session, or the run is drawn as two agent cursors.
    #[tokio::test]
    async fn one_agent_run_is_one_driver_session_across_mcp_sessions() {
        let tools = Arc::new(Sessions::default());
        let state = state(tools.clone());
        let first = move_cursor(&state, None, Some("run-84a8dc1f")).await;
        let second = move_cursor(&state, None, Some("run-84a8dc1f")).await;
        move_cursor(&state, Some(&first), Some("run-84a8dc1f")).await;
        assert_ne!(first, second, "two MCP sessions");
        let seen = tools.0.lock().unwrap().clone();
        assert_eq!(seen, vec!["agent-run-84a8dc1f"; 3]);

        // Another run is another cursor.
        move_cursor(&state, Some(&first), Some("run-00000002")).await;
        assert_eq!(
            tools.0.lock().unwrap().last().unwrap(),
            "agent-run-00000002"
        );
    }

    /// Without the header (a plain MCP client) the MCP session is the driver
    /// session, as before; an unusable header value is ignored.
    #[tokio::test]
    async fn without_an_agent_the_mcp_session_is_the_driver_session() {
        let tools = Arc::new(Sessions::default());
        let state = state(tools.clone());
        let a = move_cursor(&state, None, None).await;
        let b = move_cursor(&state, None, Some("bad value/../x")).await;
        let long = "x".repeat(65);
        let c = move_cursor(&state, Some("mcp-client-chosen"), Some(&long)).await;
        assert_eq!(c, "mcp-client-chosen");
        assert_eq!(*tools.0.lock().unwrap(), vec![a, b, c]);
    }

    /// A session-owning tool through the real registry, which keys the
    /// caller's session (and so its overlay cursor) runtime-privately.
    struct CursorTool {
        def: cua_driver_core::tool::ToolDef,
        seen: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl cua_driver_core::tool::Tool for CursorTool {
        fn def(&self) -> &cua_driver_core::tool::ToolDef {
            &self.def
        }

        async fn invoke(&self, args: Value) -> cua_driver_core::protocol::ToolResult {
            let session = args["_session_id"].as_str().unwrap_or("").to_owned();
            self.seen.lock().unwrap().push(session);
            cua_driver_core::protocol::ToolResult::text("moved")
        }
    }

    fn registry_state(seen: Arc<Mutex<Vec<String>>>) -> McpState {
        let mut registry = cua_driver_core::tool::ToolRegistry::new();
        registry.register(Box::new(CursorTool {
            def: cua_driver_core::tool::ToolDef {
                name: "move_cursor".into(),
                description: "test cursor".into(),
                input_schema: json!({"type": "object"}),
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
            seen,
        }));
        McpState {
            ctx: ServerContext::new(ServerConfig::default(), Some(TOKEN.into())),
            tools: Arc::new(registry),
            envelopes: None,
        }
    }

    async fn delete(state: &McpState, mcp: Option<&str>, agent: Option<&str>) -> StatusCode {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
        );
        if let Some(mcp) = mcp {
            headers.insert(SESSION_HEADER, HeaderValue::from_str(mcp).unwrap());
        }
        if let Some(agent) = agent {
            headers.insert(AGENT_SESSION_HEADER, HeaderValue::from_str(agent).unwrap());
        }
        handle(State(state.clone()), Method::DELETE, headers, Bytes::new())
            .await
            .status()
    }

    /// `DELETE /mcp` with only `X-Cua-Agent-Session` ends the run's driver
    /// session under the key its cursor was drawn with, so the in-guest
    /// overlay hides at once; with an `Mcp-Session-Id` it does not, and other
    /// runs keep their cursors.
    #[tokio::test]
    async fn deleting_a_run_ends_its_driver_session() {
        let ended: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = ended.clone();
        let _hook = cua_driver_core::session::register_scoped_session_end_hook(move |id| {
            seen.lock().unwrap().push(id.to_owned());
        });
        let cursors: Arc<Mutex<Vec<String>>> = Arc::default();
        let state = registry_state(cursors.clone());
        let session = move_cursor(&state, None, Some("run-finished-7f3a")).await;
        move_cursor(&state, None, Some("run-keepgoing-7f3a")).await;
        let cursors = cursors.lock().unwrap().clone();
        let (finished, keepgoing) = (cursors[0].clone(), cursors[1].clone());
        assert!(
            finished.ends_with(":agent-run-finished-7f3a") && finished != "agent-run-finished-7f3a",
            "the registry keys the cursor runtime-privately: {finished}"
        );

        assert_eq!(
            delete(&state, Some(&session), Some("run-keepgoing-7f3a")).await,
            StatusCode::NO_CONTENT
        );
        assert!(!ended.lock().unwrap().contains(&keepgoing));

        assert_eq!(
            delete(&state, None, Some("run-finished-7f3a")).await,
            StatusCode::NO_CONTENT
        );
        let ended = ended.lock().unwrap().clone();
        assert!(ended.contains(&finished), "{ended:?}");
        assert!(!ended.contains(&keepgoing), "{ended:?}");
        cua_driver_core::session::end_session(&keepgoing);
    }

    /// A plain MCP client (no agent header) owns its driver session through
    /// its MCP session: deleting that session ends it and hides its cursor.
    #[tokio::test]
    async fn deleting_a_plain_mcp_session_ends_its_driver_session() {
        let ended: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = ended.clone();
        let _hook = cua_driver_core::session::register_scoped_session_end_hook(move |id| {
            seen.lock().unwrap().push(id.to_owned());
        });
        let cursors: Arc<Mutex<Vec<String>>> = Arc::default();
        let state = registry_state(cursors.clone());
        let session = move_cursor(&state, None, None).await;
        let other = move_cursor(&state, None, None).await;
        let cursors = cursors.lock().unwrap().clone();
        assert_eq!(
            delete(&state, Some(&session), None).await,
            StatusCode::NO_CONTENT
        );
        let ended = ended.lock().unwrap().clone();
        assert!(ended.contains(&cursors[0]), "{ended:?}");
        assert!(!ended.contains(&cursors[1]), "{ended:?}");
        cua_driver_core::session::end_trusted_adapter_transport(&other);
    }
}
