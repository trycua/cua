// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A fake OIDC issuer for end-to-end tests of the relay account mode. Keys
//! are generated at start; nothing is persisted. It approves every device
//! authorization immediately (so `cua auth login --no-browser` completes
//! without a browser) and mints tokens for a fixed user. Like Keycloak, a
//! completed sign-in (the device grant) sets `auth_time` to now and a
//! refresh keeps it.
//!
//! ```text
//! fake_oidc --listen 0.0.0.0:9000 --issuer http://issuer:9000 \
//!           --sub ada --email ada@example.com [--audience cua-relay]
//! ```
//!
//! Endpoints (advertised relative to the request's Host, so the same issuer
//! works from inside and outside a Docker network; `iss` stays `--issuer`):
//! `/.well-known/openid-configuration`, `/jwks`, `POST /device`,
//! `POST /token` (device_code and refresh_token grants), and
//! `GET /mint?sub=&email=&ttl=[&auth_time=now]` for scripts (`auth_time=now`
//! marks the token a fresh interactive sign-in, which bootstraps a device
//! enrollment).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cua_relay::oidc::testing::FakeIssuer;

struct Issuer {
    fake: FakeIssuer,
    sub: String,
    email: Option<String>,
    audience: String,
    ttl: i64,
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let listen: SocketAddr = arg(&args, "--listen")
        .unwrap_or_else(|| "127.0.0.1:9000".into())
        .parse()
        .expect("--listen");
    let issuer = arg(&args, "--issuer").unwrap_or_else(|| format!("http://{listen}"));
    let state = Arc::new(Issuer {
        fake: FakeIssuer::new(issuer.trim_end_matches('/')),
        sub: arg(&args, "--sub").unwrap_or_else(|| "ada".into()),
        email: arg(&args, "--email"),
        audience: arg(&args, "--audience").unwrap_or_else(|| "cua-relay".into()),
        ttl: arg(&args, "--ttl")
            .and_then(|t| t.parse().ok())
            .unwrap_or(300),
    });
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/jwks", get(jwks))
        .route("/device", post(device))
        .route("/token", post(token))
        .route("/mint", get(mint))
        .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(listen).await.expect("bind");
    eprintln!("fake OIDC issuer {issuer} on {listen}");
    axum::serve(listener, app).await.expect("serve");
}

fn base(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost");
    format!("http://{host}")
}

async fn discovery(State(s): State<Arc<Issuer>>, headers: HeaderMap) -> Response {
    let base = base(&headers);
    Json(serde_json::json!({
        "issuer": s.fake.issuer,
        "jwks_uri": format!("{base}/jwks"),
        "token_endpoint": format!("{base}/token"),
        "device_authorization_endpoint": format!("{base}/device"),
        "grant_types_supported": ["urn:ietf:params:oauth:grant-type:device_code", "refresh_token"],
    }))
    .into_response()
}

async fn jwks(State(s): State<Arc<Issuer>>) -> Response {
    Json(s.fake.jwks_json()).into_response()
}

async fn device(headers: HeaderMap) -> Response {
    let base = base(&headers);
    Json(serde_json::json!({
        "device_code": format!("dc-{}", uuid::Uuid::new_v4().simple()),
        "user_code": "FAKE-CODE",
        "verification_uri": format!("{base}/verify"),
        "expires_in": 60,
        "interval": 1,
    }))
    .into_response()
}

fn tokens(s: &Issuer, sub: &str, email: Option<&str>, ttl: i64) -> serde_json::Value {
    serde_json::json!({
        "access_token": s.fake.token(sub, email, &s.audience, ttl),
        "refresh_token": format!("rt-{sub}-{}", uuid::Uuid::new_v4().simple()),
        "expires_in": ttl,
        "token_type": "Bearer",
        "scope": "openid profile offline_access",
    })
}

async fn token(
    State(s): State<Arc<Issuer>>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Response {
    match form.get("grant_type").map(String::as_str) {
        Some("urn:ietf:params:oauth:grant-type:device_code") => {
            // An interactive sign-in just finished.
            s.fake
                .set_auth_time(&s.sub, cua_relay::assertion::now_secs() as i64);
            Json(tokens(&s, &s.sub, s.email.as_deref(), s.ttl)).into_response()
        }
        Some("refresh_token") => {
            Json(tokens(&s, &s.sub, s.email.as_deref(), s.ttl)).into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "unsupported_grant_type"})),
        )
            .into_response(),
    }
}

async fn mint(
    State(s): State<Arc<Issuer>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let sub = q.get("sub").cloned().unwrap_or_else(|| s.sub.clone());
    let email = q.get("email").cloned().or_else(|| s.email.clone());
    let ttl = q.get("ttl").and_then(|t| t.parse().ok()).unwrap_or(s.ttl);
    if q.get("auth_time").map(String::as_str) == Some("now") {
        s.fake
            .set_auth_time(&sub, cua_relay::assertion::now_secs() as i64);
    }
    Json(tokens(&s, &sub, email.as_deref(), ttl)).into_response()
}
