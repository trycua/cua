// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! View-only shares: an account on a machine's `viewers` list reaches it
//! with an assertion whose role is `viewer`, never `shared`; the owner can
//! upgrade it to an editor (the allowlist) or revoke it, and the relay
//! refuses a revoked account at once.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::HeaderMap;
use cua_relay::assertion::ASSERTION_HEADER;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

/// The role inside an assertion (its payload is base64url JSON).
fn role_of(assertion: &str) -> String {
    use base64::Engine as _;
    let payload = assertion.split('.').nth(1).expect("payload");
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("base64url");
    let v: serde_json::Value = serde_json::from_slice(&raw).expect("json");
    v["role"].as_str().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn viewers_get_a_viewer_assertion_and_revocation_is_immediate() {
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        // This file is about viewer/editor roles, not device enrollment
        // (see tests/device_enrollment.rs for that).
        device_enrollment: false,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });

    // The machine's local server records the assertion it is handed.
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let record = seen.clone();
    let app = axum::Router::new().fallback(move |headers: HeaderMap| {
        let record = record.clone();
        async move {
            if let Some(a) = headers.get(ASSERTION_HEADER).and_then(|v| v.to_str().ok()) {
                record.lock().unwrap().push(a.to_owned());
            }
            "ok"
        }
    });
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(local, app).await;
    });

    let http = reqwest::Client::new();
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 600);
    let id = format!("share{}", uuid::Uuid::new_v4().simple());
    let reg: serde_json::Value = http
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&ada)
        .json(&serde_json::json!({"id": id, "name": "shared space"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let machine_token = reg["machine_token"].as_str().unwrap().to_owned();
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        machine_token,
        id.clone(),
        local_addr,
    );
    join.heartbeat = Duration::from_secs(2);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(relay.machine_ids().contains(&id), "machine joined");

    let patch = |body: serde_json::Value| {
        let (http, base, id, ada) = (http.clone(), base.clone(), id.clone(), ada.clone());
        async move {
            let r = http
                .patch(format!("{base}/v1/machines/{id}"))
                .bearer_auth(ada)
                .json(&body)
                .send()
                .await
                .unwrap();
            assert!(r.status().is_success(), "{}", r.status());
            r.json::<serde_json::Value>().await.unwrap()
        }
    };
    let call = |token: String| {
        let (http, base, id) = (http.clone(), base.clone(), id.clone());
        async move {
            http.get(format!("{base}/m/{id}/probe"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status()
        }
    };

    // Not shared: refused.
    assert_eq!(call(bob.clone()).await, reqwest::StatusCode::FORBIDDEN);
    // Viewer.
    let view = patch(serde_json::json!({"viewers": ["BOB@example.com"]})).await;
    assert_eq!(view["viewers"], serde_json::json!(["bob@example.com"]));
    assert_eq!(call(bob.clone()).await, reqwest::StatusCode::OK);
    assert_eq!(role_of(seen.lock().unwrap().last().unwrap()), "viewer");
    // Bob's own view of the machine says viewer and hides the lists.
    let as_bob: serde_json::Value = http
        .get(format!("{base}/v1/machines/{id}"))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(as_bob["role"], "viewer");
    assert!(as_bob.get("viewers").is_none());
    // Editor: move to the allowlist.
    patch(serde_json::json!({"viewers": [], "allow": ["bob@example.com"]})).await;
    assert_eq!(call(bob.clone()).await, reqwest::StatusCode::OK);
    assert_eq!(role_of(seen.lock().unwrap().last().unwrap()), "shared");
    // Revoked: refused on the next request.
    patch(serde_json::json!({"allow": []})).await;
    assert_eq!(call(bob.clone()).await, reqwest::StatusCode::FORBIDDEN);
    // The owner still gets through as the owner.
    assert_eq!(call(ada.clone()).await, reqwest::StatusCode::OK);
    assert_eq!(role_of(seen.lock().unwrap().last().unwrap()), "owner");
    stop.cancel();
}
