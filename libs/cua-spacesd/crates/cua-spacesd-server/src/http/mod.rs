// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Plain-HTTP routes served next to gRPC on the driver port. Every path must
//! be one of `cua_proto::metadata::*_PATH` (see the route/spec test).

pub mod files;
pub mod mcp;
pub mod mcp_envelope;
pub mod multipart;
pub mod share;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
#[cfg(target_os = "windows")]
use axum::Json;

use crate::context::ServerContext;

/// Non-Windows keeps the original unauthenticated liveness response. Windows
/// serves Siren's authenticated readiness JSON from the live desktop probe.
pub async fn health(State(ctx): State<ServerContext>, headers: HeaderMap) -> Response {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = headers;
        return if ctx.shutdown_token().is_cancelled() {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::NO_CONTENT
        }
        .into_response();
    }

    #[cfg(target_os = "windows")]
    {
        if !ctx.check_bearer(&headers) {
            return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
        }
        let interactive_session = !ctx.shutdown_token().is_cancelled()
            && platform_windows::diagnostics::siren_interactive_session_ready();
        return windows_health_response(interactive_session).into_response();
    }
}

#[cfg(target_os = "windows")]
fn windows_health_response(interactive_session: bool) -> (StatusCode, Json<serde_json::Value>) {
    if interactive_session {
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ok",
                "interactive_session": true,
                "platform": "windows"
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "interactive_session": false,
                "platform": "windows"
            })),
        )
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn health_payload_never_promotes_not_ready_to_interactive() {
        let (status, Json(body)) = windows_health_response(true);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            serde_json::json!({
                "status": "ok",
                "interactive_session": true,
                "platform": "windows"
            })
        );

        let (status, Json(body)) = windows_health_response(false);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body,
            serde_json::json!({
                "status": "not_ready",
                "interactive_session": false,
                "platform": "windows"
            })
        );
    }

    #[tokio::test]
    async fn windows_health_reuses_bearer_authentication() {
        let ctx = ServerContext::new(
            crate::config::ServerConfig::default(),
            Some("0123456789abcdef0123456789abcdef".into()),
        );
        let response = health(State(ctx), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
