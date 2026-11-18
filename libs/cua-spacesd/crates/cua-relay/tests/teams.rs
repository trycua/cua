// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teams end to end over HTTP with device enrollment on: the owner creates
//! a team, shares a machine with it and invites a member by link; the
//! member sees the machine at once but opens it only after an admin
//! approved their device; members cannot manage the team; a macOS Space
//! joins like any other; removing the member cuts them off; every step is in the team's
//! audit log.
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::HeaderMap;
use base64::Engine as _;
use cua_relay::assertion::ASSERTION_HEADER;
use cua_relay::devices::{register_message, session_message};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";
const SESSION: &str = "x-cua-device-session";

fn now() -> u64 {
    cua_relay::assertion::now_secs()
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// An enrolled device of one account, with a live session.
struct Device {
    token: String,
    session: String,
    id: String,
}

async fn enroll(base: &str, issuer: &FakeIssuer, sub: &str, email: &str) -> Device {
    issuer.set_auth_time(sub, now() as i64);
    let token = issuer.token(sub, Some(email), "cua-relay", 600);
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let id = cua_relay::devices::device_id(pair.public_key().as_ref());
    let sign = |m: &str| b64().encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let http = reqwest::Client::new();
    let ts = now();
    let r = http
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&json!({
            "public_key": b64().encode(pair.public_key().as_ref()),
            "name": format!("{sub} laptop"),
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
    Device {
        token,
        session: v["session"].as_str().unwrap().to_owned(),
        id,
    }
}

impl Device {
    async fn call(
        &self,
        method: reqwest::Method,
        url: String,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = reqwest::Client::new()
            .request(method, url)
            .bearer_auth(&self.token)
            .header(SESSION, &self.session);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await.unwrap();
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
    async fn get(&self, url: String) -> (StatusCode, Value) {
        self.call(reqwest::Method::GET, url, None).await
    }
    async fn post(&self, url: String, body: Value) -> (StatusCode, Value) {
        self.call(reqwest::Method::POST, url, Some(body)).await
    }
}

#[tokio::test]
async fn a_team_shares_machines_with_approved_member_devices() {
    let issuer = FakeIssuer::new(ISSUER);
    let state = tempfile::tempdir().unwrap();
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(state.path().join("machines.json")),
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });

    // The machine's local server records the assertions it is handed.
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

    let ada = enroll(&base, &issuer, "ada", "ada@aigis.test").await;
    let bob = enroll(&base, &issuer, "bob", "bob@aigis.test").await;
    let eve = enroll(&base, &issuer, "eve", "eve@else.test").await;

    // Ada registers a Linux Space and a macOS one; both join.
    let linux = "space-teamlinux001";
    let (s, reg) = ada
        .post(
            format!("{base}/v1/machines"),
            json!({"id": linux, "name": "build box"}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{reg}");
    let (s, _) = ada
        .post(
            format!("{base}/v1/machines"),
            json!({"id": "space-teammac0001", "name": "mac", "meta": {"cua.space.os": "macos"}}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        reg["machine_token"].as_str().unwrap().to_owned(),
        linux.to_owned(),
        local_addr,
    );
    join.heartbeat = Duration::from_secs(2);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&linux.to_owned()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // Create the team, add the machines (Macs may be team machines).
    let (s, team) = ada
        .post(format!("{base}/v1/teams"), json!({"name": "AIGIS"}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{team}");
    let id = team["id"].as_str().unwrap().to_owned();
    assert_eq!(team["role"], "owner");
    let (s, v) = ada
        .post(
            format!("{base}/v1/teams/{id}/machines"),
            json!({"machine": linux, "os": "linux"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = ada
        .post(
            format!("{base}/v1/teams/{id}/machines"),
            json!({"machine": "space-teammac0001"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Not in the team: Eve neither sees the team nor reaches the machine.
    let (s, _) = eve.get(format!("{base}/v1/teams/{id}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = eve.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // A link invite; Bob joins with the pasted link.
    let (s, inv) = ada
        .post(format!("{base}/v1/teams/{id}/invites"), json!({}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{inv}");
    let link = inv["link"].as_str().unwrap().to_owned();
    assert!(link.contains("/join/team/"), "{link}");
    let page = reqwest::get(&link).await.unwrap().text().await.unwrap();
    assert!(page.contains("cua team join"), "{page}");
    let (s, joined) = bob
        .post(format!("{base}/v1/teams/join"), json!({"code": link}))
        .await;
    assert_eq!(s, StatusCode::OK, "{joined}");
    assert_eq!(joined["role"], "member");
    assert_eq!(joined["this_device"], "pending");

    // Bob (and his agents) see the team machine right away...
    let (s, list) = bob.get(format!("{base}/v1/machines")).await;
    assert_eq!(s, StatusCode::OK);
    let m = list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == linux)
        .cloned()
        .expect("team machine listed for the member");
    assert_eq!(m["team"], id.as_str());
    assert_eq!(m["role"], "shared");
    // ...but open it only once an admin approved this device.
    let (s, v) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(v.to_string().contains("waiting for a team admin"), "{v}");

    // A member cannot approve, invite or add machines.
    let (s, _) = bob
        .post(
            format!("{base}/v1/teams/{id}/devices/{}/approve", bob.id),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = bob
        .post(format!("{base}/v1/teams/{id}/invites"), json!({}))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // The owner sees the pending device and approves it.
    let (_, t) = ada.get(format!("{base}/v1/teams/{id}")).await;
    assert!(
        t["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["device"] == bob.id.as_str() && d["state"] == "pending"),
        "{t}"
    );
    let (s, d) = ada
        .post(
            format!("{base}/v1/teams/{id}/devices/{}/approve", bob.id),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let (s, _) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        seen.lock().unwrap().last().is_some(),
        "the machine got an assertion"
    );

    // An email invite is accepted when the invited (verified) email lists
    // its machines from an enrolled device: clients without team commands
    // join too.
    let carol = enroll(&base, &issuer, "carol", "carol@aigis.test").await;
    let (s, _) = ada
        .post(
            format!("{base}/v1/teams/{id}/invites"),
            json!({"email": "Carol@aigis.test"}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, list) = carol.get(format!("{base}/v1/machines")).await;
    assert!(
        list["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == linux),
        "{list}"
    );
    let (_, t) = carol.get(format!("{base}/v1/teams/{id}")).await;
    assert_eq!(t["role"], "member");
    assert_eq!(t["this_device"], "pending");
    // Eve, not invited, joins nothing by listing.
    let (_, list) = eve.get(format!("{base}/v1/machines")).await;
    assert!(!list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == linux));

    // Removing Bob cuts him off.
    let (s, _) = ada
        .call(
            reqwest::Method::DELETE,
            format!("{base}/v1/teams/{id}/members/bob@aigis.test"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // The org audit log has the joins, approvals and opens.
    let (s, a) = ada
        .get(format!("{base}/v1/teams/{id}/audit?limit=200"))
        .await;
    assert_eq!(s, StatusCode::OK);
    let kinds: Vec<String> = a["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect();
    for k in [
        "team_created",
        "machine_added",
        "invite_created",
        "member_joined",
        "device_requested",
        "access_refused",
        "device_approved",
        "machine_opened",
        "member_removed",
    ] {
        assert!(kinds.contains(&k.to_owned()), "{k} missing from {kinds:?}");
    }

    // Teams survive a relay restart (same state directory).
    drop(relay);
    let reopened = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(state.path().join("machines.json")),
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base2 = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = reopened.serve(listener, std::future::pending()).await;
    });
    let ada2 = enroll(&base2, &issuer, "ada", "ada@aigis.test").await;
    let (s, t) = ada2.get(format!("{base2}/v1/teams/{id}")).await;
    assert_eq!(s, StatusCode::OK, "{t}");
    assert_eq!(t["machines"][0]["id"], linux);
    stop.cancel();
}
