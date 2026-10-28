// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Account erasure (CUA-1950): `DELETE /internal/v1/accounts/{sub}` removes
//! or revokes everything the relay holds for one account and nothing of
//! anyone else's; it is idempotent, resumable, and the erasure survives a
//! restart (the stores reload from disk).
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use cua_mesh::{Mesh, MeshConfig, NodeKind};
use cua_relay::devices::{register_message, session_message};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::org_access::OrgAccessConfig;
use cua_relay::server::{Relay, RelayConfig};
use futures_util::{SinkExt as _, StreamExt as _};
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";
const INTERNAL: &str = "internal-secret";
const SESSION: &str = "x-cua-device-session";
/// The accounts' ids are distinctive so a leftover shows up in a file scan.
const ADA: &str = "ada-sub-7f3a";
const BOB: &str = "bob-sub-91c2";
const CY: &str = "cy-sub-5d10";
const ADA_EMAIL: &str = "ada-erase@example.test";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn now() -> u64 {
    cua_relay::assertion::now_secs()
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn email_of(sub: &str) -> String {
    match sub {
        ADA => ADA_EMAIL.to_owned(),
        _ => format!("{}@example.test", sub.split('-').next().unwrap()),
    }
}

/// A spacesd-shaped ticket for `principal`.
fn ticket(principal: &str, issued: Option<u64>) -> String {
    let exp = (std::time::SystemTime::now() + Duration::from_secs(600))
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut payload = json!({"s": "media", "r": "sess", "p": principal, "e": exp, "n": "abc"});
    if let Some(a) = issued {
        payload["a"] = a.into();
    }
    let body = b64().encode(serde_json::to_vec(&payload).unwrap());
    format!("v1.{body}.c2ln")
}

struct Env {
    relay: Relay,
    base: String,
    issuer: FakeIssuer,
    http: reqwest::Client,
    stop: CancellationToken,
    dir: tempfile::TempDir,
}

/// An enrolled device of one account.
#[derive(Clone)]
struct Acct {
    sub: String,
    token: String,
    session: String,
}

fn config(issuer: &FakeIssuer, dir: &std::path::Path, internal: Option<&str>) -> RelayConfig {
    RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(dir.join("machines.json")),
        org_access: OrgAccessConfig {
            internal_token: internal.map(str::to_owned),
            ..Default::default()
        },
        ..RelayConfig::default()
    }
}

async fn serve(relay: &Relay) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    base
}

impl Env {
    async fn start() -> Self {
        let issuer = FakeIssuer::new(ISSUER);
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::new(config(&issuer, dir.path(), Some(INTERNAL)));
        let base = serve(&relay).await;
        Self {
            relay,
            base,
            issuer,
            http: reqwest::Client::new(),
            stop: CancellationToken::new(),
            dir,
        }
    }

    async fn enroll(&self, sub: &str) -> Acct {
        enroll_on(&self.base, &self.issuer, sub).await
    }

    /// Registers machine `id` for `who` (allowing `allow`) and joins it with
    /// an echo `/media` WebSocket behind it. Returns the machine token.
    async fn machine(&self, who: &Acct, allow: &[&str]) -> (String, String) {
        let app = axum::Router::new()
            .route(
                "/media",
                axum::routing::get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                    ws.on_upgrade(|mut socket| async move {
                        while let Some(Ok(m)) = socket.recv().await {
                            if socket.send(m).await.is_err() {
                                break;
                            }
                        }
                    })
                }),
            )
            .fallback(|| async { "ok" });
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(local, app).await;
        });
        let id = format!("host-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let r = self
            .http
            .post(format!("{}/v1/machines", self.base))
            .bearer_auth(&who.token)
            .header(SESSION, &who.session)
            .json(&json!({"id": id, "name": format!("{} box", who.sub), "allow": allow}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::CREATED);
        let reg: Value = r.json().await.unwrap();
        let token = reg["machine_token"].as_str().unwrap().to_owned();
        let mut join = cua_relay::client::JoinConfig::new(
            self.base.replace("http://", "ws://"),
            token.clone(),
            id.clone(),
            local_addr,
        );
        join.heartbeat = Duration::from_secs(2);
        tokio::spawn(cua_relay::client::run(join, self.stop.clone()));
        for _ in 0..200 {
            if self.relay.machine_ids().contains(&id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(self.relay.machine_ids().contains(&id), "machine joined");
        (id, token)
    }

    async fn media(&self, id: &str, ticket: &str) -> Result<Ws, u16> {
        let url = format!(
            "{}/m/{id}/media?ticket={ticket}",
            self.base.replace("http://", "ws://")
        );
        match tokio_tungstenite::connect_async(url).await {
            Ok((ws, _)) => Ok(ws),
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => Err(r.status().as_u16()),
            Err(e) => panic!("media connect: {e}"),
        }
    }

    /// An account call to the machine (what mints a ticket on spacesd).
    async fn call(&self, id: &str, who: &Acct) -> StatusCode {
        self.http
            .get(format!("{}/m/{id}/probe", self.base))
            .bearer_auth(&who.token)
            .header(SESSION, &who.session)
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn erase(&self, sub: &str, emails: &[&str]) -> (StatusCode, Value) {
        erase_on(&self.base, sub, emails, Some(INTERNAL)).await
    }
}

async fn enroll_on(base: &str, issuer: &FakeIssuer, sub: &str) -> Acct {
    issuer.set_auth_time(sub, now() as i64);
    let token = issuer.token(sub, Some(&email_of(sub)), "cua-relay", 600);
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
    Acct {
        sub: sub.to_owned(),
        token,
        session: v["session"].as_str().unwrap().to_owned(),
    }
}

async fn erase_on(
    base: &str,
    sub: &str,
    emails: &[&str],
    token: Option<&str>,
) -> (StatusCode, Value) {
    let query: String = emails
        .iter()
        .map(|e| format!("email={}", e.replace('@', "%40")))
        .collect::<Vec<_>>()
        .join("&");
    let mut r = reqwest::Client::new().delete(format!(
        "{base}/internal/v1/accounts/{sub}{}{query}",
        if query.is_empty() { "" } else { "?" }
    ));
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    let r = r.send().await.unwrap();
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

impl Acct {
    async fn get(&self, base: &str, path: &str) -> (StatusCode, Value) {
        let r = reqwest::Client::new()
            .get(format!("{base}{path}"))
            .bearer_auth(&self.token)
            .header(SESSION, &self.session)
            .send()
            .await
            .unwrap();
        let status = r.status();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, base: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let r = reqwest::Client::new()
            .post(format!("{base}{path}"))
            .bearer_auth(&self.token)
            .header(SESSION, &self.session)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn machines(&self, base: &str) -> Vec<Value> {
        let (s, v) = self.get(base, "/v1/machines").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["machines"].as_array().unwrap().clone()
    }
}

async fn mesh_node(
    base: &str,
    dir: &std::path::Path,
    name: &str,
    auth: Vec<(String, String)>,
    kind: NodeKind,
) -> Mesh {
    let mut cfg = MeshConfig::new(
        base.to_owned(),
        kind,
        Some(dir.join(format!("mesh-{name}.json"))),
        cua_mesh::static_auth(auth),
    );
    cfg.name = name.into();
    let m = Mesh::start(cfg).await.unwrap();
    m.wait_ready(Duration::from_secs(10)).await.unwrap();
    m
}

async fn alive(ws: &mut Ws) -> bool {
    if ws.send(Message::Text("ping".into())).await.is_err() {
        return false;
    }
    matches!(
        tokio::time::timeout(Duration::from_secs(2), ws.next()).await,
        Ok(Some(Ok(Message::Text(_))))
    )
}

async fn closed(ws: &mut Ws, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let _ = ws.send(Message::Text("ping".into())).await;
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return false,
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return true,
            Ok(Some(Ok(_))) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// Every file under `dir` (recursively) whose content has `needle`.
fn files_with(dir: &std::path::Path, needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(raw) = std::fs::read(&p) {
                if String::from_utf8_lossy(&raw).contains(needle) {
                    hits.push(p.display().to_string());
                }
            }
        }
    }
    hits
}

#[tokio::test]
async fn the_endpoint_needs_the_internal_token() {
    let env = Env::start().await;
    let (s, _) = erase_on(&env.base, ADA, &[], None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = erase_on(&env.base, ADA, &[], Some("nope")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // An account token is not the internal token.
    let ada = env.enroll(ADA).await;
    let (s, _) = erase_on(&env.base, ADA, &[], Some(&ada.token)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // The device was not touched by the refused calls.
    assert_eq!(ada.get(&env.base, "/v1/devices").await.0, StatusCode::OK);
    // No internal token configured: the route does not exist.
    let issuer = FakeIssuer::new(ISSUER);
    let dir = tempfile::tempdir().unwrap();
    let relay = Relay::new(config(&issuer, dir.path(), None));
    let base = serve(&relay).await;
    let (s, _) = erase_on(&base, ADA, &[], Some(INTERNAL)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    env.stop.cancel();
}

#[tokio::test]
async fn an_unknown_account_is_a_200_with_zero_counts() {
    let env = Env::start().await;
    let bob = env.enroll(BOB).await;
    let (id, _) = env.machine(&bob, &[]).await;
    let (s, v) = env
        .erase(
            "nobody-at-all",
            &["nobody@example.test", "other@example.test"],
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["complete"], true);
    for k in [
        "machines",
        "shares_removed",
        "devices",
        "pairing_codes",
        "sessions_cut",
        "mesh_nodes",
        "teams",
        "grants",
    ] {
        assert_eq!(v[k], 0, "{k}: {v}");
    }
    for k in ["usage", "plan_cache", "pool"] {
        assert_eq!(v[k], false, "{k}: {v}");
    }
    assert_eq!(
        v["posthog_distinct_id"],
        cua_relay::meter::posthog_distinct_id("nobody-at-all").as_str()
    );
    // Bob is untouched.
    assert_eq!(bob.machines(&env.base).await.len(), 1);
    assert!(env.relay.machine_ids().contains(&id));
    // A bad id is a 400, not an erase.
    let (s, _) = erase_on(&env.base, "%20", &[], Some(INTERNAL)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    env.stop.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_erases_one_account_everywhere_and_nobody_elses_and_survives_a_restart() {
    let env = Env::start().await;
    let (ada, bob, cy) = (
        env.enroll(ADA).await,
        env.enroll(BOB).await,
        env.enroll(CY).await,
    );

    // Machines: Ada's A (shared with Bob), Bob's B (shared with Ada by her
    // email), Cy's C (shared with nobody).
    let (a, a_token) = env.machine(&ada, &[&email_of(BOB)]).await;
    let (b, b_token) = env.machine(&bob, &[ADA_EMAIL]).await;
    let (c, c_token) = env.machine(&cy, &[]).await;

    // Ada reaches Bob's machine and holds a live session and a ticket on it.
    assert_eq!(env.call(&b, &ada).await, StatusCode::OK);
    assert_eq!(env.call(&a, &ada).await, StatusCode::OK);
    assert_eq!(env.call(&a, &bob).await, StatusCode::OK);
    let mut ada_on_b = env.media(&b, &ticket(ADA, None)).await.expect("ada on B");
    let mut bob_on_b = env.media(&b, &ticket(BOB, None)).await.expect("bob on B");
    assert!(alive(&mut ada_on_b).await);
    let held = ticket(ADA, None);

    // Teams: Ada owns T1 and invited Cy, whose listing joins him; she solely
    // owns T4. Bob owns T2 and T3. Ada is in T2 and has an open email invite
    // to T3.
    let (s, t1) = ada
        .post(&env.base, "/v1/teams", json!({"name": "first crew"}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{t1}");
    let t1 = t1["id"].as_str().unwrap().to_owned();
    let (s, _) = ada
        .post(
            &env.base,
            &format!("/v1/teams/{t1}/invites"),
            json!({"email": email_of(CY)}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(cy.machines(&env.base).await.len(), 1);
    let (s, t4) = ada
        .post(&env.base, "/v1/teams", json!({"name": "solo crew"}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{t4}");
    let t4 = t4["id"].as_str().unwrap().to_owned();
    assert!(!files_with(env.dir.path(), &t4).is_empty());
    let (_, t2) = bob
        .post(&env.base, "/v1/teams", json!({"name": "bob team"}))
        .await;
    let t2 = t2["id"].as_str().unwrap().to_owned();
    let (s, _) = bob
        .post(
            &env.base,
            &format!("/v1/teams/{t2}/invites"),
            json!({"email": ADA_EMAIL}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, joined) = ada
        .post(&env.base, "/v1/teams/join", json!({"team": t2}))
        .await;
    assert!(s.is_success(), "{joined}");
    let (_, t3) = bob
        .post(&env.base, "/v1/teams", json!({"name": "other team"}))
        .await;
    let t3 = t3["id"].as_str().unwrap().to_owned();
    let (s, _) = bob
        .post(
            &env.base,
            &format!("/v1/teams/{t3}/invites"),
            json!({"email": ADA_EMAIL}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);

    // A pairing code, a pool lease and usage.
    let (s, code) = ada.post(&env.base, "/v1/pairing-codes", json!({})).await;
    assert_eq!(s, StatusCode::CREATED, "{code}");
    let (s, lease) = ada
        .post(
            &env.base,
            "/v1/pool/lease",
            json!({"name": "n", "priority": 1, "epoch": 0, "ttl_ms": 30000}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{lease}");
    assert_eq!(ada.get(&env.base, "/v1/usage").await.0, StatusCode::OK);
    assert_eq!(bob.get(&env.base, "/v1/usage").await.0, StatusCode::OK);
    let pool_dir = env.dir.path().join("machines.pool");
    assert!(
        std::fs::read_dir(&pool_dir).unwrap().count() >= 1,
        "ada's pool directory exists"
    );

    // Mesh: Ada's host and laptop, Bob's host.
    let m_a = mesh_node(
        &env.base,
        env.dir.path(),
        &a,
        vec![
            ("authorization".into(), format!("Bearer {a_token}")),
            ("x-cua-machine-id".into(), a.clone()),
        ],
        NodeKind::Machine,
    )
    .await;
    let m_b = mesh_node(
        &env.base,
        env.dir.path(),
        &b,
        vec![
            ("authorization".into(), format!("Bearer {b_token}")),
            ("x-cua-machine-id".into(), b.clone()),
        ],
        NodeKind::Machine,
    )
    .await;
    let m_ada = mesh_node(
        &env.base,
        env.dir.path(),
        "ada laptop",
        vec![
            ("authorization".into(), format!("Bearer {}", ada.token)),
            (SESSION.into(), ada.session.clone()),
        ],
        NodeKind::Client,
    )
    .await;
    let connected_before = env.relay.mesh().connected();
    assert!(connected_before >= 3, "{connected_before}");
    let _ = (&m_a, &m_b, &m_ada, &c_token);

    // What the others hold now.
    let (_, bob_audit_before) = bob.get(&env.base, "/v1/audit").await;
    let (_, cy_devices_before) = cy.get(&env.base, "/v1/devices").await;
    let cy_machines_before = cy.machines(&env.base).await;
    assert!(
        env.relay.meter().all().iter().any(|r| r.user == ADA),
        "ada is metered"
    );

    // ---- Erase.
    // No `email` given: the relay finds hers in its own records.
    let (s, v) = env.erase(ADA, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["complete"], true);
    assert_eq!(v["machines"], 1, "{v}");
    assert_eq!(v["shares_removed"], 1, "{v}");
    assert_eq!(v["devices"], 1, "{v}");
    assert_eq!(v["pairing_codes"], 1, "{v}");
    assert!(v["sessions_cut"].as_u64().unwrap() >= 2, "{v}");
    assert!(v["mesh_nodes"].as_u64().unwrap() >= 2, "{v}");
    assert_eq!(v["usage"], true, "{v}");
    assert_eq!(v["pool"], true, "{v}");
    assert_eq!(
        v["teams"], 4,
        "T1 passed on, T2 left, T3 invite scrubbed, T4 deleted: {v}"
    );
    assert!(v["grants"].as_u64().unwrap() >= 1, "{v}");
    assert_eq!(
        v["posthog_distinct_id"],
        cua_relay::meter::posthog_distinct_id(ADA).as_str()
    );
    assert!(!v.to_string().contains("ada-"), "counts only, no PII: {v}");

    // Tunnels and sessions: Ada's machine is gone, her stream on Bob's is cut,
    // Bob's is not, and her old tickets are refused.
    assert!(!env.relay.machine_ids().contains(&a));
    assert!(
        closed(&mut ada_on_b, Duration::from_secs(3)).await,
        "ada cut"
    );
    assert!(alive(&mut bob_on_b).await, "bob keeps his stream");
    assert_eq!(env.media(&b, &held).await.err(), Some(403), "held ticket");
    assert_eq!(
        env.media(&b, &ticket(ADA, Some(now() - 1))).await.err(),
        Some(403)
    );
    // Her device session is gone with her device.
    let (s, _) = ada.get(&env.base, "/v1/devices").await;
    assert!(!s.is_success(), "{s}");
    assert_eq!(env.call(&b, &ada).await, StatusCode::FORBIDDEN);

    // Others' data: Bob's machine lost only her share; her machine is gone
    // from Bob's list; Cy's data is intact; the teams are cleaned.
    let bob_machines = bob.machines(&env.base).await;
    assert_eq!(bob_machines.len(), 1, "{bob_machines:?}");
    assert_eq!(bob_machines[0]["id"], b.as_str());
    assert!(
        !bob_machines[0].to_string().contains("ada-"),
        "{}",
        bob_machines[0]
    );
    assert!(env.relay.machine_ids().contains(&b));
    assert!(env.relay.machine_ids().contains(&c));
    assert_eq!(cy.machines(&env.base).await, cy_machines_before);
    assert_eq!(cy.get(&env.base, "/v1/devices").await.1, cy_devices_before);
    assert_eq!(bob.get(&env.base, "/v1/devices").await.0, StatusCode::OK);
    // Bob's own audit chain is exactly as it was, and still verifies.
    // Bob's own audit chain no longer names her, keeps every event, and
    // still links entry to entry (the unit test checks the MACs).
    let (_, bob_audit) = bob.get(&env.base, "/v1/audit").await;
    let before = bob_audit_before["events"].as_array().unwrap();
    let after = bob_audit["events"].as_array().unwrap();
    assert!(
        before.iter().any(|e| e.to_string().contains("ada-")),
        "bob's log named her before"
    );
    assert!(!bob_audit.to_string().contains("ada-"), "{bob_audit}");
    assert_eq!(before.len(), after.len());
    for (b, a) in before.iter().zip(after) {
        assert_eq!(b["seq"], a["seq"]);
        assert_eq!(b["kind"], a["kind"]);
    }
    for w in after.windows(2) {
        assert_eq!(w[1]["prev"], w[0]["hash"], "re-chained");
    }
    let (s, t) = bob.get(&env.base, &format!("/v1/teams/{t2}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(t["members"].as_array().unwrap().len(), 1, "{t}");
    assert!(!t.to_string().contains("ada-"), "{t}");
    let (s, t) = bob.get(&env.base, &format!("/v1/teams/{t3}")).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!t.to_string().contains("ada-"), "invite scrubbed: {t}");
    let (s, t) = cy.get(&env.base, &format!("/v1/teams/{t1}")).await;
    assert_eq!(s, StatusCode::OK, "{t}");
    assert_eq!(
        t["role"], "owner",
        "ownership passed to the only other member: {t}"
    );
    assert_eq!(t["members"].as_array().unwrap().len(), 1, "{t}");
    assert!(!t.to_string().contains("ada-"), "{t}");
    assert!(
        files_with(env.dir.path(), &t4).is_empty(),
        "the team ada solely owned is gone"
    );
    assert!(env.relay.meter().all().iter().all(|r| r.user != ADA));
    assert!(env.relay.meter().all().iter().any(|r| r.user == BOB));
    assert!(
        env.relay.mesh().connected() < connected_before,
        "ada's mesh nodes were dropped"
    );
    // Idempotent: the same call again is the same 200 with zero counts.
    let (s, again) = env.erase(ADA, &[ADA_EMAIL]).await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(again["complete"], true);
    for k in [
        "machines",
        "shares_removed",
        "devices",
        "pairing_codes",
        "sessions_cut",
        "mesh_nodes",
        "teams",
        "grants",
    ] {
        assert_eq!(again[k], 0, "{k}: {again}");
    }
    for k in ["usage", "plan_cache", "pool"] {
        assert_eq!(again[k], false, "{k}: {again}");
    }

    // Nothing of hers is on the state volume.
    if let Some(f) = files_with(env.dir.path(), ADA).into_iter().next() {
        let raw = std::fs::read_to_string(&f).unwrap();
        let at = raw.find(ADA).unwrap();
        panic!(
            "{f} still holds the account id: ...{}...",
            &raw[at.saturating_sub(200)..(at + 120).min(raw.len())]
        );
    }
    let leftovers = files_with(env.dir.path(), ADA_EMAIL);
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Restart: everything reloads from disk without her.
    env.stop.cancel();
    env.relay.disconnect_all();
    let relay2 = Relay::new(config(&env.issuer, env.dir.path(), Some(INTERNAL)));
    let base2 = serve(&relay2).await;
    let bob_machines = bob.machines(&base2).await;
    assert_eq!(bob_machines.len(), 1);
    assert!(!bob_machines[0].to_string().contains("ada-"));
    assert_eq!(cy.machines(&base2).await.len(), 1);
    assert!(!ada.get(&base2, "/v1/devices").await.0.is_success());
    assert_eq!(bob.get(&base2, "/v1/audit").await.1, bob_audit);
    assert!(relay2.meter().all().iter().all(|r| r.user != ADA));
    let (s, t) = cy.get(&base2, &format!("/v1/teams/{t1}")).await;
    assert_eq!((s, t["role"].as_str()), (StatusCode::OK, Some("owner")));
    assert!(files_with(env.dir.path(), &t4).is_empty());
    let (s, again) = erase_on(&base2, ADA, &[ADA_EMAIL], Some(INTERNAL)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["devices"], 0);
    assert_eq!(again["machines"], 0);
    assert_eq!(again["teams"], 0);
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_write_answers_500_and_the_same_call_again_finishes() {
    let env = Env::start().await;
    let (ada, bob) = (env.enroll(ADA).await, env.enroll(BOB).await);
    let (a, _) = env.machine(&ada, &[]).await;
    let (s, code) = ada.post(&env.base, "/v1/pairing-codes", json!({})).await;
    assert_eq!(s, StatusCode::CREATED, "{code}");

    // The pairing file can no longer be replaced (a directory sits there).
    let pairing = env.dir.path().join("machines.pairing.json");
    assert!(pairing.exists());
    std::fs::remove_file(&pairing).unwrap();
    std::fs::create_dir(&pairing).unwrap();
    std::fs::write(pairing.join("keep"), b"x").unwrap();

    let (s, v) = env.erase(ADA, &[]).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{v}");
    assert_eq!(v["complete"], false);
    assert!(!v.to_string().contains(ADA), "{v}");
    // What ran before the failure stays done.
    assert!(!env.relay.machine_ids().contains(&a));
    assert!(!ada.get(&env.base, "/v1/devices").await.0.is_success());

    // Fixed; the same call finishes.
    std::fs::remove_dir_all(&pairing).unwrap();
    let (s, v) = env.erase(ADA, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["complete"], true);
    assert!(files_with(env.dir.path(), ADA).is_empty());
    assert_eq!(bob.get(&env.base, "/v1/devices").await.0, StatusCode::OK);
    env.stop.cancel();
}

#[tokio::test]
async fn internal_routes_refuse_requests_that_came_through_a_proxy() {
    let env = Env::start().await;
    let url = format!("{}/internal/v1/accounts/{ADA}", env.base);
    let http = reqwest::Client::new();
    // Direct, in-cluster style: works.
    let r = http
        .delete(&url)
        .bearer_auth(INTERNAL)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // Any proxy header: the same answer as an unknown route, even with the
    // right token.
    for (name, value) in [
        ("X-Forwarded-For", "203.0.113.9"),
        ("X-Forwarded-Host", "relay.example.com"),
        ("Forwarded", "for=203.0.113.9"),
        ("X-Real-IP", "203.0.113.9"),
    ] {
        let r = http
            .delete(&url)
            .bearer_auth(INTERNAL)
            .header(name, value)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{name}");
        assert_eq!(r.text().await.unwrap(), "not found", "{name}");
    }
    for (method, path) in [
        ("POST", "/internal/v1/membership-changed"),
        ("GET", "/internal/v1/records/host-x"),
        ("GET", "/internal/v1/shared?sub=x"),
    ] {
        let r = http
            .request(method.parse().unwrap(), format!("{}{path}", env.base))
            .bearer_auth(INTERNAL)
            .header("X-Forwarded-For", "203.0.113.9")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
    }
    env.stop.cancel();
}
