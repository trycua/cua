// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Plain-HTTP routes served next to gRPC on the driver port. Every path must
//! be one of `cua_proto::metadata::*_PATH` (see the route/spec test).

pub mod files;
pub mod mcp;
pub mod mcp_envelope;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::context::ServerContext;

/// `GET /health`: unauthenticated liveness. 204 while serving, 503 once
/// shutting down.
pub async fn health(State(ctx): State<ServerContext>) -> impl IntoResponse {
    if ctx.shutdown_token().is_cancelled() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::NO_CONTENT
    }
}
