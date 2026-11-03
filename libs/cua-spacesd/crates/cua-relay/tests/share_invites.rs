// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Share by email is a pending invite (CUA-1764): a new email on a
//! machine's allowlist or viewers is passed to the Spaces API once, which
//! emails the recipient; `/internal/v1/shared` lists what is shared with an
//! account, and counts shares that wait for its email to be verified.
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use cua_relay::devices::{register_message, session_message};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::org_access::OrgAccessConfig;
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";
const SESSION: &str = "x-cua-device-session";
const INTERNAL: &str = "internal-test-token";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// Enrolls a device for `sub` and returns (token, session).
async fn enroll(base: &str, issuer: &FakeIssuer, sub: &str, email: &str) -> (String, String) {
    issuer.set_auth_time(sub, cua_relay::assertion::now_secs() as i64);
    let token = issuer.token(sub, Some(email), "cua-relay", 600);
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let id = cua_relay::devices::device_id(pair.public_key().as_ref());
    let sign = |m: &str| b64().encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let http = reqwest::Client::new();
    let ts = cua_relay::assertion::now_secs();
    let r = http
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&json!({
            "public_key": b64().encode(pair.public_key().as_ref()),
            "name": "laptop",
            "ts": ts,
            "sig": sign(&register_message(&id, ts)),
            "bootstrap": true,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "register {}", r.status());
    let r = http
        .post(format!("{base}/v1/devices/session"))
        .bearer_auth(&token)
        .json(&json!({"device_id": id, "ts": ts, "sig": sign(&session_message(&id, ts))}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "session {}", r.status());
    let v: Value = r.json().await.unwrap();
    (token, v["session"].as_str().unwrap().to_owned())
}

async fn call(
    method: reqwest::Method,
    url: String,
    (token, session): &(String, String),
    body: Value,
) -> (StatusCode, Value) {
    let r = reqwest::Client::new()
        .request(method, url)
        .bearer_auth(token)
        .header(SESSION, session)
        .json(&body)
        .send()
        .await
        .unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

/// A fake Spaces API that records share invites.
async fn fake_api(invites: Arc<Mutex<Vec<Value>>>) -> String {
    let app = axum::Router::new().route(
        "/internal/v1/share-invites",
        axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
            let invites = invites.clone();
            async move {
                invites.lock().unwrap().push(v);
                axum::http::StatusCode::ACCEPTED
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    url
}

async fn start_relay(issuer: &FakeIssuer, dir: &std::path::Path, api: String) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(dir.join("machines.json")),
        org_access: OrgAccessConfig {
            api_url: Some(api),
            client_id: "cua-relay-svc".into(),
            internal_token: Some(INTERNAL.into()),
            ..Default::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    base
}

async fn wait_for(invites: &Arc<Mutex<Vec<Value>>>, n: usize) {
    for _ in 0..200 {
        if invites.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn shared(base: &str, query: &str) -> (StatusCode, Value) {
    let r = reqwest::Client::new()
        .get(format!("{base}/internal/v1/shared?{query}"))
        .bearer_auth(INTERNAL)
        .send()
        .await
        .unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn email_share_is_a_pending_invite_with_one_notification() {
    let issuer = FakeIssuer::new(ISSUER);
    let dir = tempfile::tempdir().unwrap();
    let invites: Arc<Mutex<Vec<Value>>> = Arc::default();
    let base = start_relay(&issuer, dir.path(), fake_api(invites.clone()).await).await;
    let ada = enroll(&base, &issuer, "ada", "ada@aigis.test").await;
    let id = "space-adasspace001";

    let (s, v) = call(
        reqwest::Method::POST,
        format!("{base}/v1/machines"),
        &ada,
        json!({"id": id, "name": "Design review"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    // Ada shares by email (mixed case), view only.
    let (s, v) = call(
        reqwest::Method::PATCH,
        format!("{base}/v1/machines/{id}"),
        &ada,
        json!({"viewers": ["Bob@Example.test"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    wait_for(&invites, 1).await;
    {
        let got = invites.lock().unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["email"], "bob@example.test");
        assert_eq!(got[0]["role"], "viewer");
        assert_eq!(got[0]["owner_sub"], "ada");
        assert_eq!(got[0]["owner_email"], "ada@aigis.test");
        assert_eq!(got[0]["machine_id"], id);
        assert_eq!(got[0]["machine_name"], "Design review");
    }

    // Changing the role of the same address, or renaming, sends nothing new;
    // an account id sends nothing at all.
    let (s, _) = call(
        reqwest::Method::PATCH,
        format!("{base}/v1/machines/{id}"),
        &ada,
        json!({"viewers": [], "allow": ["bob@example.test", "acct-123"]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(invites.lock().unwrap().len(), 1);

    // Bob's email isn't verified: nothing listed, one share waits.
    let (s, v) = shared(&base, "sub=bob&email=bob@example.test&email_verified=false").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({"shared": [], "pending": 1}));

    // Verified: listed with its role, nothing pending.
    let (_, v) = shared(&base, "sub=bob&email=BOB@example.test&email_verified=true").await;
    assert_eq!(v["pending"], 0);
    let list = v["shared"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{v}");
    assert_eq!(list[0]["id"], id);
    assert_eq!(list[0]["role"], "shared");
    assert_eq!(list[0]["owner"]["id"], "ada");

    // The owner's own machines are not "shared with you"; the token is required.
    let (_, v) = shared(&base, "sub=ada&email=ada@aigis.test&email_verified=true").await;
    assert_eq!(v, json!({"shared": [], "pending": 0}));
    let r = reqwest::Client::new()
        .get(format!("{base}/internal/v1/shared?sub=bob"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}
