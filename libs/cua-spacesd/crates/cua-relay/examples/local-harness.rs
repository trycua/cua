// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A local relay with a fake identity provider, for end-to-end runs of the
//! phone app and the daemon on one machine. NEVER production: the
//! identity provider signs a token for anyone who asks, and the push
//! provider writes alerts to a file instead of sending them.
//!
//! ```text
//! cargo run -p cua-relay --example local-harness -- --dir /path/to/state
//! ```
//!
//! It listens on loopback ports only and writes `<dir>/harness.json`
//! (relay URL, identity provider URL, the accounts, the alerts file) once
//! it is up. Everything it keeps is under `--dir`. It stops on SIGINT or
//! SIGTERM.
//!
//! The identity provider (`idp`):
//!
//! - `GET  /realms/harness/.well-known/openid-configuration` and `/jwks`;
//! - `POST /token` with `{"sub": "ada", "email": "ada@harness.test",
//!   "ttl_secs": 43200}` answers `{"access_token": ...}` for any account
//!   (every token counts as a fresh sign-in, so a device enrolls with it);
//! - `POST /realms/harness/protocol/openid-connect/token` is the refresh
//!   grant (a refresh token is `refresh:<sub>`).
//!
//! The relay runs in account mode with device enrollment on, push on (the
//! alerts file is `<dir>/push-alerts.jsonl`) and the viewer's ticket
//! support, the same code as the hosted relay.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Form, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator, DEFAULT_AUDIENCE};
use cua_relay::push_api::PushConfig;
use cua_relay::server::{Relay, RelayConfig};
use serde_json::{json, Value};

struct Idp {
    issuer: FakeIssuer,
    base: String,
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn token(idp: &Idp, sub: &str, email: Option<&str>, ttl: i64) -> String {
    // Every token is a fresh sign-in: a device enrolls with it.
    idp.issuer.set_auth_time(sub, now_secs() as i64);
    let email = email
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{sub}@harness.test"));
    idp.issuer.token(sub, Some(&email), DEFAULT_AUDIENCE, ttl)
}

async fn mint(State(idp): State<Arc<Idp>>, Json(body): Json<Value>) -> Json<Value> {
    let sub = body["sub"].as_str().unwrap_or("ada");
    let ttl = body["ttl_secs"]
        .as_i64()
        .unwrap_or(43_200)
        .clamp(60, 7 * 86_400);
    let t = token(&idp, sub, body["email"].as_str(), ttl);
    Json(
        json!({"access_token": t, "token_type": "Bearer", "expires_in": ttl,
                "refresh_token": format!("refresh:{sub}")}),
    )
}

async fn refresh(
    State(idp): State<Arc<Idp>>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let sub = form
        .get("refresh_token")
        .and_then(|r| r.strip_prefix("refresh:"))
        .unwrap_or("ada")
        .to_string();
    let t = token(&idp, &sub, None, 43_200);
    Json(
        json!({"access_token": t, "token_type": "Bearer", "expires_in": 43_200,
                "refresh_token": format!("refresh:{sub}"), "scope": "openid profile offline_access"}),
    )
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dir = PathBuf::from(arg("--dir").ok_or("--dir <state directory> is required")?);
    std::fs::create_dir_all(&dir)?;
    let accounts: Vec<String> = arg("--accounts")
        .unwrap_or_else(|| "ada,bob".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // The identity provider.
    let idp_listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let idp_url = format!("http://{}", idp_listener.local_addr()?);
    let issuer_url = format!("{idp_url}/realms/harness");
    let idp = Arc::new(Idp {
        issuer: FakeIssuer::new(issuer_url.clone()),
        base: idp_url.clone(),
    });
    let jwks = idp.issuer.jwks();
    let jwks_json = idp.issuer.jwks_json();
    let discovery = json!({
        "issuer": issuer_url,
        "jwks_uri": format!("{issuer_url}/jwks"),
        "token_endpoint": format!("{issuer_url}/protocol/openid-connect/token"),
        "authorization_endpoint": format!("{issuer_url}/protocol/openid-connect/auth"),
        "device_authorization_endpoint": format!("{issuer_url}/protocol/openid-connect/auth/device"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
    });
    let app = Router::new()
        .route("/token", post(mint))
        .route(
            "/realms/harness/protocol/openid-connect/token",
            post(refresh),
        )
        .route(
            "/realms/harness/.well-known/openid-configuration",
            get({
                let d = discovery.clone();
                move || async move { Json(d) }
            }),
        )
        .route(
            "/realms/harness/jwks",
            get({
                let j = jwks_json.clone();
                move || async move { Json(j) }
            }),
        )
        .with_state(idp.clone());
    tokio::spawn(async move {
        let _ = axum::serve(idp_listener, app).await;
    });

    // The relay.
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let relay_url = format!("http://{}", listener.local_addr()?);
    let alerts = dir.join("push-alerts.jsonl");
    let hub = Arc::new(cua_push::PushHub::new(
        cua_push::Registry::open(dir.join("push.json"))?,
        Arc::new(cua_push::LogProvider::new(&alerts)),
    ));
    let relay = Relay::try_new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(issuer_url.clone()),
            jwks,
        ))),
        state_file: Some(dir.join("relay-state.json")),
        public_url: Some(relay_url.clone()),
        device_enrollment: true,
        device_policy: cua_relay::devices::DevicePolicy {
            // A token minted here is a fresh sign-in for a day.
            bootstrap_max_auth_age_secs: 86_400,
            ..Default::default()
        },
        push: Some(PushConfig::new(hub)),
        ..RelayConfig::default()
    })?;

    let info = json!({
        "pid": std::process::id(),
        "relay_url": relay_url,
        "idp_url": idp.base,
        "issuer": issuer_url,
        "audience": DEFAULT_AUDIENCE,
        "mint_url": format!("{}/token", idp.base),
        "accounts": accounts,
        "push_alerts_file": alerts,
        "dir": dir,
    });
    std::fs::write(dir.join("harness.json"), serde_json::to_vec_pretty(&info)?)?;
    println!("{}", serde_json::to_string(&info)?);
    relay.serve(listener, shutdown()).await?;
    let _ = std::fs::remove_file(dir.join("harness.json"));
    Ok(())
}
