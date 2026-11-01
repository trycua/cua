// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Team access from Keycloak Organizations and the Spaces API (CUA-1481),
//! against a fake Spaces API: with `org`, a member of the machine's
//! organization opens it only from an approved device, others never; the
//! relay fails closed when the API is down (personal machines unaffected);
//! a membership push cuts a removed member at once; `/v1/teams` is gone.
//! With `shadow`, `teams.json` still decides and disagreements are counted.
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Query;
use axum::http::StatusCode as AxStatus;
use base64::Engine as _;
use cua_relay::devices::{register_message, session_message};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::org_access::{Clock, OrgAccessConfig, TeamsSource};
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

/// The fake Spaces API's state.
#[derive(Default)]
struct Api {
    down: bool,
    /// (org, sub) members.
    members: HashSet<(String, String)>,
    /// (org, machine).
    machines: HashSet<(String, String)>,
    /// (org, sub, device) approvals.
    approved: HashSet<(String, String, String)>,
    access_calls: usize,
}

type Shared = Arc<Mutex<Api>>;

async fn fake_api(state: Shared) -> String {
    let access_state = state.clone();
    let list_state = state.clone();
    let app = axum::Router::new()
        .route(
            "/internal/v1/access",
            axum::routing::get(
                move |Query(q): Query<std::collections::HashMap<String, String>>| {
                    let state = access_state.clone();
                    async move {
                        let mut s = state.lock().unwrap();
                        if s.down {
                            return (AxStatus::SERVICE_UNAVAILABLE, axum::Json(json!({})));
                        }
                        s.access_calls += 1;
                        let org = q.get("org").cloned().unwrap_or_default();
                        let sub = q.get("sub").cloned().unwrap_or_default();
                        let machine = q.get("machine").cloned().unwrap_or_default();
                        let device = q.get("device").cloned().unwrap_or_default();
                        let member = s.members.contains(&(org.clone(), sub.clone()));
                        (
                            AxStatus::OK,
                            axum::Json(json!({
                                "member": member,
                                "role": if member { Some("member") } else { None },
                                "machine_in_org": s.machines.contains(&(org.clone(), machine)),
                                "device_approved": s.approved.contains(&(org, sub, device)),
                                "ttl": 60,
                            })),
                        )
                    }
                },
            ),
        )
        .route(
            "/internal/v1/orgs/{org}/machines",
            axum::routing::get(
                move |axum::extract::Path(org): axum::extract::Path<String>| {
                    let state = list_state.clone();
                    async move {
                        let s = state.lock().unwrap();
                        if s.down {
                            return (AxStatus::SERVICE_UNAVAILABLE, axum::Json(json!({})));
                        }
                        let machines: Vec<Value> = s
                            .machines
                            .iter()
                            .filter(|(o, _)| *o == org)
                            .map(|(_, m)| json!({"machine_id": m}))
                            .collect();
                        (AxStatus::OK, axum::Json(json!({ "machines": machines })))
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    url
}

const INTERNAL: &str = "internal-test-token";
/// Far longer than any test runs: a cached answer expires only by
/// `expire(&clock)`.
const TTL: Duration = Duration::from_secs(3600);

fn expire(clock: &Clock) {
    clock.advance(TTL + Duration::from_secs(1));
}

async fn start_relay(
    issuer: &FakeIssuer,
    state: &std::path::Path,
    source: TeamsSource,
    api: Option<String>,
    clock: &Clock,
) -> (Relay, String) {
    // No stale serving: the fail-closed tests want the first miss to refuse.
    start_relay_stale(issuer, state, source, api, clock, Duration::ZERO).await
}

async fn start_relay_stale(
    issuer: &FakeIssuer,
    state: &std::path::Path,
    source: TeamsSource,
    api: Option<String>,
    clock: &Clock,
    stale: Duration,
) -> (Relay, String) {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(state.join("machines.json")),
        org_access: OrgAccessConfig {
            source,
            api_url: api,
            client_id: "cua-relay-svc".into(),
            // Entries expire only when a test advances `clock`, never by
            // the wall clock, so a slow runner cannot expire one early.
            ttl: Some(TTL),
            clock: clock.clone(),
            stale_window: Some(stale),
            internal_token: Some(INTERNAL.into()),
            ..Default::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    (relay, base)
}

/// Registers machine `id` for `owner` and connects it to a local server.
async fn connect_machine(relay: &Relay, base: &str, owner: &Device, id: &str) -> CancellationToken {
    connect_machine_with(relay, base, owner, json!({"id": id, "name": id})).await
}

/// Registers the machine `body` describes for `owner` and connects it.
async fn connect_machine_with(
    relay: &Relay,
    base: &str,
    owner: &Device,
    body: Value,
) -> CancellationToken {
    let id = body["id"].as_str().unwrap().to_owned();
    let id = id.as_str();
    let app = axum::Router::new().fallback(|| async { "ok" });
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(local, app).await;
    });
    let (s, reg) = owner
        .post(format!("{base}/v1/machines"), body.clone())
        .await;
    assert_eq!(s, StatusCode::CREATED, "{reg}");
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        reg["machine_token"].as_str().unwrap().to_owned(),
        id.to_owned(),
        local_addr,
    );
    join.heartbeat = Duration::from_secs(2);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&id.to_owned()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    stop
}

async fn push(base: &str, token: Option<&str>, body: Value) -> StatusCode {
    let mut req = reqwest::Client::new()
        .post(format!("{base}/internal/v1/membership-changed"))
        .json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    req.send().await.unwrap().status()
}

async fn metric(base: &str, name: &str) -> u64 {
    let text = reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{name} ")))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("{name} missing from {text}"))
}

#[tokio::test]
async fn org_mode_uses_claims_and_the_api_and_fails_closed() {
    let issuer = FakeIssuer::new(ISSUER);
    let state = tempfile::tempdir().unwrap();
    let api: Shared = Arc::default();
    let api_url = fake_api(api.clone()).await;
    let clock = Clock::default();
    let (relay, base) = start_relay(
        &issuer,
        state.path(),
        TeamsSource::Org,
        Some(api_url),
        &clock,
    )
    .await;

    let org = json!({"aigis": {"id": "org-1", "groups": ["/owner"]}});
    issuer.set_orgs("ada", org.clone());
    issuer.set_orgs("bob", json!({"aigis": {"id": "org-1", "groups": []}}));
    let ada = enroll(&base, &issuer, "ada", "ada@aigis.test").await;
    let bob = enroll(&base, &issuer, "bob", "bob@aigis.test").await;
    let eve = enroll(&base, &issuer, "eve", "eve@else.test").await;
    let linux = "space-orglinux0001";
    let stop = connect_machine(&relay, &base, &ada, linux).await;
    {
        let mut a = api.lock().unwrap();
        a.machines.insert(("org-1".into(), linux.into()));
        a.members.insert(("org-1".into(), "ada".into()));
        a.members.insert(("org-1".into(), "bob".into()));
    }

    // The owner's own machine needs no API.
    let (s, _) = ada.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK);

    // Bob lists the org's machine, but his device is not approved yet.
    let (s, list) = bob.get(format!("{base}/v1/machines")).await;
    assert_eq!(s, StatusCode::OK);
    let m = list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == linux)
        .cloned()
        .expect("org machine listed for the member");
    assert_eq!(m["team"], "org-1");
    assert_eq!(m["role"], "shared");
    let (s, v) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert!(v.to_string().contains("approval"), "{v}");

    // Approved in the API: in once the cached "no" expires.
    api.lock()
        .unwrap()
        .approved
        .insert(("org-1".into(), "bob".into(), bob.id.clone()));
    expire(&clock);
    let (s, v) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // A second open within the TTL is served from the cache.
    let calls = api.lock().unwrap().access_calls;
    let (s, _) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(api.lock().unwrap().access_calls, calls);

    // Eve (no organization) neither sees nor opens it.
    let (_, list) = eve.get(format!("{base}/v1/machines")).await;
    assert!(!list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == linux));
    let (s, _) = eve.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // The relay's own teams API is retired.
    let (s, v) = ada.get(format!("{base}/v1/teams")).await;
    assert_eq!(s, StatusCode::GONE, "{v}");
    let (s, _) = ada
        .post(format!("{base}/v1/teams/team-1/machines"), json!({}))
        .await;
    assert_eq!(s, StatusCode::GONE);

    // The API goes down: once the cache expires, team access fails closed;
    // the owner's own machine still opens.
    api.lock().unwrap().down = true;
    expire(&clock);
    let (s, v) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    let (s, _) = ada.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK);
    assert!(metric(&base, "cua_relay_org_fail_closed_total").await >= 1);
    api.lock().unwrap().down = false;
    let (s, _) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK);

    // A membership push needs the internal token...
    assert_eq!(
        push(&base, None, json!({"org": "org-1", "sub": "bob"})).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        push(&base, Some("wrong"), json!({"org": "org-1", "sub": "bob"})).await,
        StatusCode::UNAUTHORIZED
    );
    // ...and cuts a removed member at once, without waiting for the TTL.
    api.lock()
        .unwrap()
        .members
        .remove(&("org-1".into(), "bob".into()));
    assert_eq!(
        push(&base, Some(INTERNAL), json!({"org": "org-1", "sub": "bob"})).await,
        StatusCode::NO_CONTENT
    );
    let (s, _) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    stop.cancel();
}

#[tokio::test]
async fn shadow_mode_keeps_teams_json_and_counts_mismatches() {
    let issuer = FakeIssuer::new(ISSUER);
    let state = tempfile::tempdir().unwrap();
    let api: Shared = Arc::default();
    let api_url = fake_api(api.clone()).await;
    let clock = Clock::default();
    let (relay, base) = start_relay(
        &issuer,
        state.path(),
        TeamsSource::Shadow,
        Some(api_url),
        &clock,
    )
    .await;
    let ada = enroll(&base, &issuer, "ada", "ada@aigis.test").await;
    let mut bob = enroll(&base, &issuer, "bob", "bob@aigis.test").await;
    let linux = "space-shadowlin001";
    let stop = connect_machine(&relay, &base, &ada, linux).await;

    // A teams.json team, as today: Bob joins and his device is approved.
    let (s, team) = ada
        .post(format!("{base}/v1/teams"), json!({"name": "AIGIS"}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{team}");
    let id = team["id"].as_str().unwrap().to_owned();
    let (s, _) = ada
        .post(
            format!("{base}/v1/teams/{id}/machines"),
            json!({"machine": linux, "os": "linux"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (_, inv) = ada
        .post(format!("{base}/v1/teams/{id}/invites"), json!({}))
        .await;
    let (s, _) = bob
        .post(
            format!("{base}/v1/teams/join"),
            json!({"code": inv["link"].as_str().unwrap()}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = ada
        .post(
            format!("{base}/v1/teams/{id}/devices/{}/approve", bob.id),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // Not migrated yet: teams.json lets Bob in, the org path disagrees.
    let (s, _) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK);
    let mismatches = metric(&base, "cua_relay_teams_shadow_mismatches_total").await;
    assert!(mismatches >= 1);

    // Migrated (org claim + API bindings): they agree, no new mismatch.
    {
        let mut a = api.lock().unwrap();
        a.machines.insert(("org-1".into(), linux.into()));
        // The machine's owner is in the org too (CUA-1403).
        a.members.insert(("org-1".into(), "ada".into()));
        a.members.insert(("org-1".into(), "bob".into()));
        a.approved
            .insert(("org-1".into(), "bob".into(), bob.id.clone()));
    }
    issuer.set_orgs("bob", json!({"aigis": {"id": "org-1"}}));
    bob.token = issuer.token("bob", Some("bob@aigis.test"), "cua-relay", 600);
    let (s, _) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = bob.get(format!("{base}/v1/machines")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        metric(&base, "cua_relay_teams_shadow_mismatches_total").await,
        mismatches
    );
    assert!(metric(&base, "cua_relay_teams_shadow_checks_total").await > mismatches);

    // The API down in shadow mode never changes the answer.
    api.lock().unwrap().down = true;
    expire(&clock);
    let (s, _) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK);
    // And the team routes still work.
    let (s, _) = ada.get(format!("{base}/v1/teams")).await;
    assert_eq!(s, StatusCode::OK);
    stop.cancel();
}

/// CUA-1403: a binding takes any machine id, so an org grants access to a
/// record only while the record's owner is a member of that same org.
/// Macs (macOS Spaces) are team machines like any other. The Spaces API
/// reads a record's owner from `/internal/v1/records/{id}`.
#[tokio::test]
async fn org_access_requires_the_owner_in_the_org_and_allows_macs() {
    let issuer = FakeIssuer::new(ISSUER);
    let state = tempfile::tempdir().unwrap();
    let api: Shared = Arc::default();
    let api_url = fake_api(api.clone()).await;
    let clock = Clock::default();
    let (relay, base) = start_relay(
        &issuer,
        state.path(),
        TeamsSource::Org,
        Some(api_url),
        &clock,
    )
    .await;

    issuer.set_orgs(
        "ada",
        json!({"aigis": {"id": "org-1", "groups": ["/owner"]}}),
    );
    issuer.set_orgs("bob", json!({"aigis": {"id": "org-1", "groups": []}}));
    let ada = enroll(&base, &issuer, "ada", "ada@aigis.test").await;
    let bob = enroll(&base, &issuer, "bob", "bob@aigis.test").await;
    let mallory = enroll(&base, &issuer, "mallory", "m@else.test").await;
    let own = "space-ownedbyada01";
    let mac = "space-adasmac0001";
    let foreign = "space-mallorys001";
    let stops = [
        connect_machine(&relay, &base, &ada, own).await,
        connect_machine_with(
            &relay,
            &base,
            &ada,
            json!({"id": mac, "name": "mac", "meta": {"cua.space.os": "macos"}}),
        )
        .await,
        connect_machine(&relay, &base, &mallory, foreign).await,
    ];
    {
        let mut a = api.lock().unwrap();
        for m in [own, mac, foreign] {
            // An org admin bound Mallory's record too (the API takes any id).
            a.machines.insert(("org-1".into(), m.into()));
        }
        a.members.insert(("org-1".into(), "ada".into()));
        a.members.insert(("org-1".into(), "bob".into()));
        a.approved
            .insert(("org-1".into(), "bob".into(), bob.id.clone()));
    }

    // Bob opens Ada's machines, the macOS Space included...
    let (s, v) = bob.get(format!("{base}/m/{own}/probe")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = bob.get(format!("{base}/m/{mac}/probe")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // ...but not Mallory's record bound to the org, nor sees it listed.
    let (s, v) = bob.get(format!("{base}/m/{foreign}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    let (_, list) = bob.get(format!("{base}/v1/machines")).await;
    let ids: Vec<&str> = list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert!(ids.contains(&own) && ids.contains(&mac), "{list}");
    assert!(!ids.contains(&foreign), "{list}");

    // The owner leaves the org: a push cuts every member at once.
    api.lock()
        .unwrap()
        .members
        .remove(&("org-1".into(), "ada".into()));
    assert_eq!(
        push(&base, Some(INTERNAL), json!({"org": "org-1", "sub": "ada"})).await,
        StatusCode::NO_CONTENT
    );
    let (s, _) = bob.get(format!("{base}/m/{own}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = bob.get(format!("{base}/m/{mac}/probe")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // The record lookup the API uses at bind time.
    let http = reqwest::Client::new();
    let r = http
        .get(format!("{base}/internal/v1/records/{mac}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = http
        .get(format!("{base}/internal/v1/records/{mac}"))
        .bearer_auth(INTERNAL)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = r.json().await.unwrap();
    assert_eq!(
        v,
        json!({"id": mac, "owner_sub": "ada", "platform": "macos", "kind": "host"})
    );
    let r = http
        .get(format!("{base}/internal/v1/records/{foreign}"))
        .bearer_auth(INTERNAL)
        .send()
        .await
        .unwrap();
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["owner_sub"], "mallory");
    assert_eq!(v["platform"], "unknown");
    let r = http
        .get(format!("{base}/internal/v1/records/space-nope000001"))
        .bearer_auth(INTERNAL)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"], "record_not_found");
    for s in stops {
        s.cancel();
    }
}

#[tokio::test]
async fn an_outage_keeps_a_previous_allow_for_the_stale_window_only() {
    let issuer = FakeIssuer::new(ISSUER);
    let state = tempfile::tempdir().unwrap();
    let api: Shared = Arc::default();
    let api_url = fake_api(api.clone()).await;
    let clock = Clock::default();
    let (relay, base) = start_relay_stale(
        &issuer,
        state.path(),
        TeamsSource::Org,
        Some(api_url),
        &clock,
        Duration::from_secs(2 * 3600),
    )
    .await;
    issuer.set_orgs(
        "ada",
        json!({"aigis": {"id": "org-1", "groups": ["/owner"]}}),
    );
    issuer.set_orgs("bob", json!({"aigis": {"id": "org-1", "groups": []}}));
    issuer.set_orgs("eve", json!({"aigis": {"id": "org-1", "groups": []}}));
    let ada = enroll(&base, &issuer, "ada", "ada@aigis.test").await;
    let bob = enroll(&base, &issuer, "bob", "bob@aigis.test").await;
    let eve = enroll(&base, &issuer, "eve", "eve@aigis.test").await;
    let linux = "space-orglinux0002";
    let stop = connect_machine(&relay, &base, &ada, linux).await;
    {
        let mut a = api.lock().unwrap();
        a.machines.insert(("org-1".into(), linux.into()));
        for who in ["ada", "bob"] {
            a.members.insert(("org-1".into(), who.into()));
        }
        a.approved
            .insert(("org-1".into(), "bob".into(), bob.id.clone()));
    }
    let (s, v) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // The API goes down. Past the TTL, Bob (allowed a moment ago) still
    // opens it; Eve, never allowed, is refused at once.
    api.lock().unwrap().down = true;
    expire(&clock);
    let (s, v) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(metric(&base, "cua_relay_org_stale_served_total").await >= 1);
    let (s, _) = eve.get(format!("{base}/m/{linux}/probe")).await;
    assert_ne!(s, StatusCode::OK);

    // Past the stale window it fails closed.
    clock.advance(Duration::from_secs(4000));
    let (s, v) = bob.get(format!("{base}/m/{linux}/probe")).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    stop.cancel();
}
