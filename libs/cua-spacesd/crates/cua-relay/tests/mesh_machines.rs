// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Machine-to-machine peering (CUA-1783): an account's hosts see each
//! other in their network maps (a gang's rank traffic between the owner's
//! machines), marked as machines of that account; another account's
//! machines and the Spaces a host provides do not; a client device sees
//! exactly what it saw before.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use cua_mesh::{Mesh, MeshConfig, NodeKind, PeerInfo};
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
}

async fn enroll(base: &str, issuer: &FakeIssuer, sub: &str) -> Device {
    issuer.set_auth_time(sub, now() as i64);
    let token = issuer.token(sub, Some(&format!("{sub}@example.test")), "cua-relay", 600);
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
    }
}

impl Device {
    /// Registers machine `id` (a Space of `host` when given): its token.
    async fn register(&self, base: &str, id: &str, host: Option<&str>, allow: &[&str]) -> String {
        let mut body = json!({"id": id, "name": format!("{id} name"), "allow": allow});
        if let Some(h) = host {
            body["host"] = json!(h);
        }
        let r = reqwest::Client::new()
            .post(format!("{base}/v1/machines"))
            .bearer_auth(&self.token)
            .header(SESSION, &self.session)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status();
        let v: Value = r.json().await.unwrap();
        assert_eq!(status, StatusCode::CREATED, "{v}");
        v["machine_token"].as_str().unwrap().to_owned()
    }
}

async fn machine(base: &str, dir: &std::path::Path, id: &str, token: &str) -> Mesh {
    let mut cfg = MeshConfig::new(
        base.to_owned(),
        NodeKind::Machine,
        Some(dir.join(format!("{id}.json"))),
        cua_mesh::static_auth(vec![
            ("authorization".into(), format!("Bearer {token}")),
            ("x-cua-machine-id".into(), id.into()),
        ]),
    );
    cfg.name = id.into();
    let m = Mesh::start(cfg).await.unwrap();
    m.wait_ready(Duration::from_secs(10)).await.unwrap();
    m
}

/// `m`'s map once it has settled on `want` peers (by machine id or, for
/// clients, by name).
async fn settle(m: &Mesh, want: &[&str]) -> Vec<PeerInfo> {
    let label = |p: &PeerInfo| p.machine_id.clone().unwrap_or_else(|| p.name.clone());
    let mut want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
    want.sort();
    let mut last = Vec::new();
    for _ in 0..200 {
        let peers = m.peers();
        let mut got: Vec<String> = peers.iter().map(label).collect();
        got.sort();
        if got == want {
            // Stays that way (the coordinator recomputes every 2 s).
            tokio::time::sleep(Duration::from_millis(2500)).await;
            let peers = m.peers();
            let mut again: Vec<String> = peers.iter().map(label).collect();
            again.sort();
            assert_eq!(again, want, "the map changed after settling");
            return peers;
        }
        last = got;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("map {last:?}, wanted {want:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_accounts_machines_peer_with_each_other_and_nobody_elses() {
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
    let ada = enroll(&base, &issuer, "ada").await;
    let bob = enroll(&base, &issuer, "bob").await;
    let dir = state.path();

    // Ada: two hosts and a Space her first host provides. Bob: one host,
    // shared with Ada (sharing never makes machines peers).
    let (a1, a2, sp, b1) = (
        "ada-host-0001",
        "ada-host-0002",
        "space-ada00001",
        "bob-host-0001",
    );
    let t1 = ada.register(&base, a1, None, &[]).await;
    let t2 = ada.register(&base, a2, None, &[]).await;
    let b_tok = bob.register(&base, b1, None, &["ada@example.test"]).await;
    let m1 = machine(&base, dir, a1, &t1).await;
    // The Space registers once its host is on the relay's mesh.
    let ts = ada.register(&base, sp, Some(a1), &[]).await;
    let m2 = machine(&base, dir, a2, &t2).await;
    let space = machine(&base, dir, sp, &ts).await;
    let mb = machine(&base, dir, b1, &b_tok).await;

    // Ada's client device sees her machines and Bob's shared one, as
    // before machine peering.
    let mut cfg = MeshConfig::new(
        base.clone(),
        NodeKind::Client,
        Some(dir.join("client.json")),
        cua_mesh::static_auth(vec![
            ("authorization".into(), format!("Bearer {}", ada.token)),
            (SESSION.into(), ada.session.clone()),
        ]),
    );
    cfg.name = "ada laptop".into();
    let client = Mesh::start(cfg).await.unwrap();
    client.wait_ready(Duration::from_secs(10)).await.unwrap();
    let seen = settle(&client, &[a1, a2, sp, b1]).await;
    assert!(seen.iter().all(|p| p.grant.is_none()), "{seen:?}");

    // Each of Ada's hosts: the other host (as a machine of her account)
    // and the client (as its owner); never the Space or Bob's machine.
    for (m, other) in [(&m1, a2), (&m2, a1)] {
        let peers = settle(m, &[other, "ada laptop"]).await;
        let host = peers
            .iter()
            .find(|p| p.machine_id.as_deref() == Some(other))
            .unwrap();
        assert_eq!(host.kind, NodeKind::Machine);
        let g = host.grant.as_ref().expect("a machine peer carries a grant");
        assert_eq!(g.role, cua_mesh::proto::ROLE_MACHINE);
        assert_eq!((g.account.as_str(), g.user.as_str()), ("ada", "ada"));
        assert_eq!(g.name.as_deref(), Some(format!("{other} name").as_str()));
        let laptop = peers.iter().find(|p| p.kind == NodeKind::Client).unwrap();
        assert_eq!(laptop.grant.as_ref().unwrap().role, "owner");
    }
    // The Space sees only the client; Bob's host sees Ada's laptop (its
    // share) and no machine of either account.
    settle(&space, &["ada laptop"]).await;
    let bobs = settle(&mb, &["ada laptop"]).await;
    assert_eq!(bobs[0].grant.as_ref().unwrap().role, "shared");

    // A stream between Ada's hosts goes through.
    let echo = m2.clone();
    tokio::spawn(async move {
        while let Some(i) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(i.stream);
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let mut s = m1
        .open_stream(&m2.node_key(), &cua_mesh::proto::port_service(47000), None)
        .await
        .unwrap();
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    s.write_all(b"rank traffic").await.unwrap();
    let mut got = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(20), s.read_exact(&mut got))
        .await
        .expect("echo")
        .unwrap();
    assert_eq!(&got, b"rank traffic");
    // Bob's machine cannot open one to Ada's host: not in its map.
    assert!(mb
        .open_stream(&m1.node_key(), &cua_mesh::proto::port_service(47000), None)
        .await
        .is_err());
}
