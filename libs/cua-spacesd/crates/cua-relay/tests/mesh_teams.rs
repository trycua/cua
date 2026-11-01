// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A team machine on the mesh (CUA-1426, CUA-1354): a member's device
//! gets the machine in its network map only once a team admin approved it,
//! as on the HTTPS path, and loses it when the machine leaves the team.
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use cua_mesh::{Mesh, MeshConfig, NodeKind};
use cua_relay::devices::{register_message, session_message};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};

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
    async fn post(&self, url: String, body: Value) -> (StatusCode, Value) {
        self.call(reqwest::Method::POST, url, Some(body)).await
    }
}

fn client(base: &str, dir: &std::path::Path, who: &Device) -> MeshConfig {
    let mut cfg = MeshConfig::new(
        base.to_owned(),
        NodeKind::Client,
        Some(dir.join("client.json")),
        cua_mesh::static_auth(vec![
            ("authorization".into(), format!("Bearer {}", who.token)),
            (SESSION.into(), who.session.clone()),
        ]),
    );
    cfg.name = "laptop".into();
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_machines_reach_approved_member_devices_over_the_mesh() {
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

    let ada = enroll(&base, &issuer, "ada", "ada@aigis.test").await;
    let bob = enroll(&base, &issuer, "bob", "bob@aigis.test").await;

    // Ada's machine joins the mesh with its machine token.
    let machine = "space-meshteam0001";
    let (s, reg) = ada
        .post(
            format!("{base}/v1/machines"),
            json!({"id": machine, "name": "build box"}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{reg}");
    let mut m = MeshConfig::new(
        base.clone(),
        NodeKind::Machine,
        Some(state.path().join("m.json")),
        cua_mesh::static_auth(vec![
            (
                "authorization".into(),
                format!("Bearer {}", reg["machine_token"].as_str().unwrap()),
            ),
            ("x-cua-machine-id".into(), machine.into()),
        ]),
    );
    m.name = "build box".into();
    let m = Mesh::start(m).await.unwrap();
    m.wait_ready(Duration::from_secs(10)).await.unwrap();

    // A team with the machine; Bob joins by link.
    let (s, team) = ada
        .post(format!("{base}/v1/teams"), json!({"name": "AIGIS"}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{team}");
    let id = team["id"].as_str().unwrap().to_owned();
    let (s, v) = ada
        .post(
            format!("{base}/v1/teams/{id}/machines"),
            json!({"machine": machine, "os": "linux"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, inv) = ada
        .post(format!("{base}/v1/teams/{id}/invites"), json!({}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{inv}");
    let (s, joined) = bob
        .post(
            format!("{base}/v1/teams/join"),
            json!({"code": inv["link"].as_str().unwrap()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{joined}");
    assert_eq!(joined["this_device"], "pending");

    // Pending: the machine is not in Bob's map.
    let dir = tempfile::tempdir().unwrap();
    let bob_mesh = Mesh::start(client(&base, dir.path(), &bob)).await.unwrap();
    bob_mesh.wait_ready(Duration::from_secs(10)).await.unwrap();
    assert!(bob_mesh
        .wait_machine(machine, Duration::from_secs(3))
        .await
        .is_none());

    // Approved: it appears, and the machine sees Bob with the team role.
    let (s, v) = ada
        .post(
            format!("{base}/v1/teams/{id}/devices/{}/approve", bob.id),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    bob_mesh
        .wait_machine(machine, Duration::from_secs(10))
        .await
        .expect("the team machine is in the approved member's map");
    let mut granted = None;
    for _ in 0..200 {
        granted = m
            .peers()
            .into_iter()
            .find(|p| p.node_key == bob_mesh.node_key())
            .and_then(|p| p.grant);
        if granted.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let grant = granted.expect("the machine's map grants Bob");
    assert_eq!(grant.account, "bob");
    assert_eq!(grant.role, "shared");

    // The machine leaves the team: Bob's map drops it.
    let r = reqwest::Client::new()
        .delete(format!("{base}/v1/teams/{id}/machines/{machine}"))
        .bearer_auth(&ada.token)
        .header(SESSION, &ada.session)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    for _ in 0..200 {
        if bob_mesh.machine(machine).is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the machine stayed in Bob's map after leaving the team");
}
