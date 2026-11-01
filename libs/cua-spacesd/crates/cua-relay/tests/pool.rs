// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account's pool on the relay: one coordinating device at a time, an
//! epoch that only grows (across relay restarts too), appends fenced by
//! epoch, idempotent retries, at most one terminal entry per job, snapshots,
//! the mailbox between clients and the holder, and the keys: every entry,
//! snapshot, request and answer must be sealed under the account's newest
//! key generation, and a revoked device's keys stop working at once.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use cua_relay::devices::{register_message, session_message};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use cua_relay::storage::StorageLimits;
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

/// The default quota and no disk reserve: what the test machine's disk has
/// free (a CI runner's is often near full) must not refuse these tests'
/// writes. The reserve tests set their own limits.
async fn start(issuer: &FakeIssuer, state: Option<std::path::PathBuf>) -> String {
    let limits = StorageLimits {
        reserve_percent: 0,
        reserve_min_bytes: 0,
        ..StorageLimits::default()
    };
    start_with(issuer, state, limits).await
}

async fn start_with(
    issuer: &FakeIssuer,
    state: Option<std::path::PathBuf>,
    storage_limits: StorageLimits,
) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: state,
        storage_limits,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    base
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// An enrolled device of `sub` with a live session.
struct Device {
    token: String,
    session: String,
    id: String,
    pair: ring::signature::EcdsaKeyPair,
    pool_key: String,
}

/// Sealed data as the devices send it (the relay sees only this shape).
fn sealed(g: u64) -> Value {
    json!({"v": 1, "g": g, "n": "bm9uY2Vub25jZW5vbmNlbm9uY2Vub25j", "c": format!("Y2lwaGVy{}", rand::random::<u32>())})
}

/// Entries with sealed data under generation `g`.
fn entries(g: u64, list: Value) -> Value {
    let mut list = list;
    for e in list.as_array_mut().unwrap() {
        e["data"] = sealed(g);
    }
    list
}

async fn device(issuer: &FakeIssuer, base: &str, sub: &str) -> Device {
    issuer.set_auth_time(sub, cua_relay::assertion::now_secs() as i64);
    let token = issuer.token(sub, Some(&format!("{sub}@example.com")), "cua-relay", 600);
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let id = cua_relay::devices::device_id(pair.public_key().as_ref());
    let sign = |m: &str| b64().encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let ts = cua_relay::assertion::now_secs();
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&json!({
            "public_key": b64().encode(pair.public_key().as_ref()),
            "name": "m", "ts": ts, "sig": sign(&register_message(&id, ts)), "bootstrap": true,
            "machine_id": format!("machine-{}", uuid::Uuid::new_v4().simple()),
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/devices/session"))
        .bearer_auth(&token)
        .json(&json!({"device_id": id, "ts": ts, "sig": sign(&session_message(&id, ts))}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let v: Value = r.json().await.unwrap();
    let key: [u8; 32] = rand::random();
    Device {
        token,
        session: v["session"].as_str().unwrap().to_owned(),
        id,
        pair,
        pool_key: base64::engine::general_purpose::STANDARD.encode(key),
    }
}

/// `who` registers its pool key and makes generation 1 for `to`.
async fn keys(base: &str, who: &Device, to: &[&Device]) {
    for d in to.iter().copied().chain([who]) {
        let (s, v) = d.register(base).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    let (s, v) = who.put_keys(base, 0, 1, to, false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

impl Device {
    async fn account(&self, base: &str) -> String {
        let (_, v) = self.get(base, "/v1/pool").await;
        v["account"].as_str().unwrap().to_owned()
    }

    async fn register(&self, base: &str) -> (StatusCode, Value) {
        let account = self.account(base).await;
        let rng = ring::rand::SystemRandom::new();
        let m = cua_relay::pool::member_message(&account, &self.id, &self.pool_key);
        let sig = b64().encode(self.pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
        self.post(
            base,
            "/v1/pool/member",
            json!({"pool_key": self.pool_key, "sig": sig}),
        )
        .await
    }

    fn wrap_for(&self, to: &Device) -> Value {
        json!({"device": to.id, "key": to.pool_key, "from": self.id, "enc": "ZW5j", "ct": "Y3Q="})
    }

    async fn put_keys(
        &self,
        base: &str,
        version: u64,
        gen: u64,
        to: &[&Device],
        reset: bool,
    ) -> (StatusCode, Value) {
        let wraps: Vec<Value> = to.iter().map(|d| self.wrap_for(d)).collect();
        self.put(
            base,
            "/v1/pool/keys",
            json!({"version": version, "keyring": {"gen": gen, "wraps": wraps}, "reset": reset}),
        )
        .await
    }

    async fn post(&self, base: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let r = reqwest::Client::new()
            .post(format!("{base}{path}"))
            .bearer_auth(&self.token)
            .header("x-cua-device-session", &self.session)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn put(&self, base: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let r = reqwest::Client::new()
            .put(format!("{base}{path}"))
            .bearer_auth(&self.token)
            .header("x-cua-device-session", &self.session)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, base: &str, path: &str) -> (StatusCode, Value) {
        let r = reqwest::Client::new()
            .get(format!("{base}{path}"))
            .bearer_auth(&self.token)
            .header("x-cua-device-session", &self.session)
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn lease(
        &self,
        base: &str,
        epoch: u64,
        priority: u32,
        ttl_ms: u64,
    ) -> (StatusCode, Value) {
        self.post(
            base,
            "/v1/pool/lease",
            json!({"name": "n", "priority": priority, "epoch": epoch, "ttl_ms": ttl_ms}),
        )
        .await
    }

    async fn append(
        &self,
        base: &str,
        epoch: u64,
        from: u64,
        entries: Value,
    ) -> (StatusCode, Value) {
        self.post(
            base,
            "/v1/pool/log",
            json!({"epoch": epoch, "from": from, "entries": entries}),
        )
        .await
    }
}

#[tokio::test]
async fn one_holder_and_appends_are_fenced_by_epoch() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, None).await;
    let a = device(&issuer, &base, "ada").await;
    let b = device(&issuer, &base, "ada").await;
    keys(&base, &a, &[&a, &b]).await;

    let (s, v) = a.lease(&base, 0, 10, 2000).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let e1 = v["epoch"].as_u64().unwrap();
    assert_eq!(e1, 1);
    // Someone else is refused while the lease is live.
    let (s, v) = b.lease(&base, 0, 10, 2000).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "held");
    // Only the holder appends.
    let (s, v) = a
        .append(
            &base,
            e1,
            0,
            entries(1, json!([{"kind": "submit", "run": "r1"}])),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["head"], 1);
    let (s, v) = b
        .append(&base, e1, 1, entries(1, json!([{"kind": "x"}])))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "fenced");

    // The holder stops renewing (paused, cut off): after the TTL, b takes
    // the next epoch, and a's appends are refused whatever a believes.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let (s, v) = b.lease(&base, 0, 10, 2000).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let e2 = v["epoch"].as_u64().unwrap();
    assert_eq!(e2, e1 + 1);
    assert_eq!(v["head"], 1);
    let (s, v) = a
        .append(&base, e1, 1, entries(1, json!([{"kind": "late"}])))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "fenced");
    // a's renewal of the old epoch is refused too.
    let (s, v) = a.lease(&base, e1, 10, 2000).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (s, _) = b
        .append(
            &base,
            e2,
            1,
            entries(
                1,
                json!([{"kind": "dispatched", "job": "j1", "node": "studio", "attempt": 1}]),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn appends_are_idempotent_and_never_end_a_job_twice() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, None).await;
    let a = device(&issuer, &base, "ada").await;
    keys(&base, &a, &[&a]).await;
    let (_, v) = a.lease(&base, 0, 10, 10_000).await;
    let e = v["epoch"].as_u64().unwrap();
    let batch = entries(
        1,
        json!([
            {"kind": "dispatched", "job": "j1", "run": "r", "node": "studio", "attempt": 1},
            {"kind": "complete", "job": "j1", "terminal": true, "run": "r"},
        ]),
    );
    let (s, v) = a.append(&base, e, 0, batch.clone()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // The reply was lost; the same append again is answered, not doubled.
    let (s, v) = a.append(&base, e, 0, batch).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["retried"], true);
    assert_eq!(v["head"], 2);
    // A different append from an old head is a conflict.
    let (s, v) = a
        .append(&base, e, 0, entries(1, json!([{"kind": "other"}])))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "conflict");
    assert_eq!(v["head"], 2);
    // A second terminal entry for j1 is refused, in any batch.
    let (s, v) = a
        .append(
            &base,
            e,
            2,
            entries(
                1,
                json!([{"kind": "complete", "job": "j1", "terminal": true}]),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "duplicate_terminal");
    let (s, v) = a
        .append(
            &base,
            e,
            2,
            entries(
                1,
                json!([
                    {"kind": "complete", "job": "j2", "terminal": true},
                    {"kind": "end", "job": "j2", "terminal": true},
                ]),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["code"], "duplicate_terminal");
    // Nothing of the refused batch was stored.
    let (s, v) = a.get(&base, "/v1/pool/log?after=0").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    let stored = v["entries"].as_array().unwrap();
    assert_ne!(stored[1]["hash"], stored[0]["hash"]);
    assert_eq!(stored[0]["node"], "studio");
    assert_eq!(stored[0]["attempt"], 1);
}

#[tokio::test]
async fn the_epoch_and_log_survive_a_relay_restart_and_snapshots_truncate() {
    let issuer = FakeIssuer::new(ISSUER);
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("relay.json");
    let base = start(&issuer, Some(state.clone())).await;
    let a = device(&issuer, &base, "ada").await;
    keys(&base, &a, &[&a]).await;
    let (_, v) = a.lease(&base, 0, 10, 2000).await;
    let e = v["epoch"].as_u64().unwrap();
    let (s, _) = a
        .append(
            &base,
            e,
            0,
            entries(
                1,
                json!([
                    {"kind": "submit", "run": "r"},
                    {"kind": "complete", "job": "j1", "terminal": true},
                    {"kind": "dispatched", "job": "j2"},
                ]),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // A snapshot in the clear is refused; a sealed one is kept.
    let (s, v) = a
        .put(
            &base,
            "/v1/pool/snapshot",
            json!({"epoch": e, "seq": 2, "data": base64::engine::general_purpose::STANDARD.encode(b"state at 2")}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["code"], "not_sealed");
    let snap = serde_json::to_vec(&sealed(1)).unwrap();
    let (s, v) = a
        .put(
            &base,
            "/v1/pool/snapshot",
            json!({"epoch": e, "seq": 2, "data": base64::engine::general_purpose::STANDARD.encode(&snap)}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = a.get(&base, "/v1/pool/log?after=0").await;
    assert_eq!(s, StatusCode::GONE, "{v}");

    // A second relay process on the same state (a restart): the epoch,
    // the log after the snapshot, the snapshot and the ended jobs are kept.
    let base2 = start(&issuer, Some(state)).await;
    let b = device(&issuer, &base2, "ada").await;
    // The keys survived the restart: b is added to them.
    let (s, v) = b.register(&base2).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v) = b.get(&base2, "/v1/pool/keys").await;
    assert_eq!(v["version"], 1, "{v}");
    let (_, v) = b.get(&base2, "/v1/pool/log?after=2").await;
    assert_eq!(v["entries"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["entries"][0]["job"], "j2");
    let (_, v) = b.get(&base2, "/v1/pool/snapshot").await;
    assert_eq!(v["seq"], 2);
    // The restarted relay treats the lease as held for a full TTL.
    let (s, _) = b.lease(&base2, 0, 10, 2000).await;
    assert_eq!(s, StatusCode::CONFLICT);
    tokio::time::sleep(Duration::from_millis(15_200)).await;
    let (s, v) = b.lease(&base2, 0, 10, 2000).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["epoch"].as_u64().unwrap(), e + 1);
    let (s, v) = b
        .append(
            &base2,
            e + 1,
            3,
            entries(
                1,
                json!([{"kind": "complete", "job": "j1", "terminal": true}]),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "duplicate_terminal");
}

#[tokio::test]
async fn a_holder_of_lower_priority_is_told_to_yield() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, None).await;
    let laptop = device(&issuer, &base, "ada").await;
    let spare = device(&issuer, &base, "ada").await;
    let (_, v) = laptop.lease(&base, 0, 1, 5000).await;
    let e = v["epoch"].as_u64().unwrap();
    let (s, _) = spare.lease(&base, 0, 10, 5000).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, v) = laptop.lease(&base, e, 1, 5000).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["yield_to"], "n");
    let (_, v) = laptop
        .post(&base, "/v1/pool/release", json!({"epoch": e}))
        .await;
    assert_eq!(v["released"], true);
    // The laptop asking again right away does not get it back: the waiting
    // machine that provides Spaces goes first.
    let (s, v) = laptop.lease(&base, 0, 1, 5000).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["waiting_for"], "n");
    let (s, v) = spare.lease(&base, 0, 10, 5000).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["epoch"].as_u64().unwrap(), e + 1);
}

#[tokio::test]
async fn the_mailbox_carries_requests_to_the_holder_and_answers_back() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, None).await;
    let holder = device(&issuer, &base, "ada").await;
    let client = device(&issuer, &base, "ada").await;
    let other = device(&issuer, &base, "bob").await;
    keys(&base, &holder, &[&holder, &client]).await;
    keys(&base, &other, &[&other]).await;

    // A request in the clear is refused.
    let (s, v) = client
        .post(
            &base,
            "/v1/pool/rpc",
            json!({"method": "submit", "params": {"run": "r9", "command": ["echo", "secret"]}}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["code"], "not_sealed");
    // No holder: the client is told at once.
    let (s, v) = client
        .post(
            &base,
            "/v1/pool/rpc",
            json!({"method": "runs", "params": {"sealed": sealed(1)}}),
        )
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    assert_eq!(v["code"], "no_coordinator");

    let (_, v) = holder.lease(&base, 0, 10, 10_000).await;
    let e = v["epoch"].as_u64().unwrap();
    let base2 = base.clone();
    let serve = tokio::spawn(async move {
        let (s, v) = holder
            .post(
                &base2,
                "/v1/pool/rpc/take",
                json!({"epoch": e, "wait_ms": 5000}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let req = v["requests"][0].clone();
        assert_eq!(req["method"], "submit");
        assert_eq!(req["caller"]["account"], "ada");
        assert_eq!(req["params"]["sealed"]["g"], 1);
        // An answer in the clear is refused; a sealed one goes back.
        let (s, v) = holder
            .post(
                &base2,
                "/v1/pool/rpc/reply",
                json!({"epoch": e, "replies": [{"id": req["id"], "body": {"accepted": 3}}]}),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        let (s, _) = holder
            .post(
                &base2,
                "/v1/pool/rpc/reply",
                json!({"epoch": e, "replies": [{"id": req["id"], "body": {"sealed": sealed(1)}}]}),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
    });
    let (s, v) = client
        .post(
            &base,
            "/v1/pool/rpc",
            json!({"method": "submit", "params": {"sealed": sealed(1)},
                   "meta": {"run": "r9", "count": 3, "on": "auto"}}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["sealed"]["g"], 1);
    serve.await.unwrap();

    // Another account's pool is separate: no holder there.
    let (s, _) = other
        .post(
            &base,
            "/v1/pool/rpc",
            json!({"method": "runs", "params": {"sealed": sealed(1)}}),
        )
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    let (s, v) = other.get(&base, "/v1/pool").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["epoch"], 0);
}

#[tokio::test]
async fn an_unenrolled_caller_is_refused() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, None).await;
    let token = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/pool/lease"))
        .bearer_auth(&token)
        .json(&json!({"epoch": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_revoked_device_stops_the_old_keys_until_new_ones_leave_it_out() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, None).await;
    let a = device(&issuer, &base, "ada").await;
    let b = device(&issuer, &base, "ada").await;
    let c = device(&issuer, &base, "ada").await;
    // A pool key not signed by the device's own key is refused.
    let (s, v) = a
        .post(
            &base,
            "/v1/pool/member",
            json!({"pool_key": a.pool_key, "sig": "AAAA"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    keys(&base, &a, &[&a, &b]).await;
    let (_, v) = a.lease(&base, 0, 10, 30_000).await;
    let e = v["epoch"].as_u64().unwrap();
    // Plain data, and data sealed under a generation that is not the
    // newest, are refused.
    let (s, v) = a
        .append(
            &base,
            e,
            0,
            json!([{"kind": "submit", "data": {"command": "echo secret"}}]),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["code"], "not_sealed");
    let (s, v) = a
        .append(&base, e, 0, entries(2, json!([{"kind": "submit"}])))
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "rekey");
    let (s, _) = a
        .append(&base, e, 0, entries(1, json!([{"kind": "submit"}])))
        .await;
    assert_eq!(s, StatusCode::OK);
    // Keyring updates: only the version they replace, only to devices of
    // the account that registered, and within a generation only added.
    let (s, v) = a.put_keys(&base, 0, 1, &[&a, &b], false).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "keyring_changed");
    let (s, v) = a.put_keys(&base, 1, 1, &[&a, &b, &c], false).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "c registered no pool key: {v}");
    let (s, v) = c.register(&base).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = a.put_keys(&base, 1, 1, &[&a], false).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a wrap was taken away: {v}");
    let (s, v) = a.put_keys(&base, 1, 1, &[&a, &b, &c], false).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // b is revoked: the old keys stop at once, for appends, requests and
    // snapshots alike, and new ones may not include b.
    let r = reqwest::Client::new()
        .delete(format!("{base}/v1/devices/{}", b.id))
        .bearer_auth(&a.token)
        .header("x-cua-device-session", &a.session)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let (_, v) = a.get(&base, "/v1/pool/keys").await;
    assert_eq!(v["stale"], true, "{v}");
    assert_eq!(v["members"].as_array().unwrap().len(), 2, "{v}");
    let (s, v) = a
        .append(
            &base,
            e,
            1,
            entries(1, json!([{"kind": "dispatched", "job": "j1"}])),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "rekey");
    let (s, v) = c
        .post(
            &base,
            "/v1/pool/rpc",
            json!({"method": "runs", "params": {"sealed": sealed(1)}}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "rekey");
    let (s, v) = a.put_keys(&base, 2, 2, &[&a, &b, &c], false).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "wrapped to a revoked device: {v}"
    );
    // No new chain while devices still hold the keys.
    let (s, v) = a.put_keys(&base, 2, 2, &[&a, &c], true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = a.put_keys(&base, 2, 2, &[&a, &c], false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = a
        .append(
            &base,
            e,
            1,
            entries(1, json!([{"kind": "dispatched", "job": "j1"}])),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "rekey");
    let (s, v) = a
        .append(
            &base,
            e,
            1,
            entries(2, json!([{"kind": "dispatched", "job": "j1"}])),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn keys_nobody_holds_start_a_new_chain_and_drop_the_log() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, None).await;
    let a = device(&issuer, &base, "ada").await;
    let b = device(&issuer, &base, "ada").await;
    keys(&base, &a, &[&a]).await;
    let (_, v) = a.lease(&base, 0, 10, 2_000).await;
    let e = v["epoch"].as_u64().unwrap();
    let (s, _) = a
        .append(
            &base,
            e,
            0,
            entries(1, json!([{"kind": "submit", "run": "r"}])),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // a, the only device with the keys, is revoked.
    let (s, v) = b.register(&base).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let r = reqwest::Client::new()
        .delete(format!("{base}/v1/devices/{}", a.id))
        .bearer_auth(&b.token)
        .header("x-cua-device-session", &b.session)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    // b starts a new chain; the log, sealed under keys nobody has, goes.
    let (s, v) = b.put_keys(&base, 1, 2, &[&b], true).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v) = b.get(&base, "/v1/pool/log?after=0").await;
    assert_eq!(v["head"], 0, "{v}");
    assert_eq!(v["entries"].as_array().unwrap().len(), 0);
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    let (s, v) = b.lease(&base, 0, 10, 2_000).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let e2 = v["epoch"].as_u64().unwrap();
    let (s, v) = b
        .append(
            &base,
            e2,
            0,
            entries(2, json!([{"kind": "submit", "run": "r2"}])),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

/// The one account pool directory under `state`.
fn pool_dir(state: &std::path::Path) -> std::path::PathBuf {
    let pools = state.with_extension("pool");
    let mut dirs: Vec<_> = std::fs::read_dir(&pools)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.pop().unwrap()
}

/// No space reserved and no quota to speak of, unless a test sets them.
fn roomy() -> StorageLimits {
    StorageLimits {
        reserve_percent: 0,
        reserve_min_bytes: 0,
        pool_account_quota_bytes: 1 << 30,
    }
}

#[tokio::test]
async fn a_torn_log_tail_and_temp_files_are_dropped_at_restart_and_appends_go_on() {
    let issuer = FakeIssuer::new(ISSUER);
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("relay.json");
    let base = start_with(&issuer, Some(state.clone()), roomy()).await;
    let a = device(&issuer, &base, "ada").await;
    keys(&base, &a, &[&a]).await;
    let (_, v) = a.lease(&base, 0, 10, 60_000).await;
    let e = v["epoch"].as_u64().unwrap();
    let (s, v) = a
        .append(
            &base,
            e,
            0,
            entries(
                1,
                json!([{"kind": "submit", "run": "r"}, {"kind": "dispatched", "job": "j1"}]),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // A crash mid-append left half a line, and another mid-rewrite a temp.
    let pool = pool_dir(&state);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(pool.join("log.jsonl"))
        .unwrap();
    std::io::Write::write_all(&mut f, b"{\"seq\":3,\"epoch\":1,\"ki").unwrap();
    drop(f);
    let temp = pool.join(format!(".log.jsonl.{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temp, vec![b'x'; 1 << 20]).unwrap();

    // The relay restarts: the device's session and lease carry over.
    let base2 = start_with(&issuer, Some(state.clone()), roomy()).await;
    assert!(!temp.exists(), "the temp file is removed at startup");
    let (s, v) = a.lease(&base2, e, 10, 60_000).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = a
        .append(
            &base2,
            e,
            2,
            entries(
                1,
                json!([{"kind": "complete", "job": "j1", "terminal": true}]),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "the torn tail was cut: {v}");
    assert_eq!(v["head"], 3);
    // And again: nothing acknowledged is lost behind a broken line.
    let base3 = start_with(&issuer, Some(state.clone()), roomy()).await;
    let (_, v) = a.get(&base3, "/v1/pool/log?after=0").await;
    let seqs: Vec<u64> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [1, 2, 3], "{v}");
}

#[tokio::test]
async fn the_account_quota_refuses_appends_until_a_snapshot() {
    let issuer = FakeIssuer::new(ISSUER);
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("relay.json");
    let base = start_with(
        &issuer,
        Some(state),
        StorageLimits {
            pool_account_quota_bytes: 8 << 10,
            ..roomy()
        },
    )
    .await;
    let a = device(&issuer, &base, "ada").await;
    keys(&base, &a, &[&a]).await;
    let (_, v) = a.lease(&base, 0, 10, 60_000).await;
    let e = v["epoch"].as_u64().unwrap();
    let mut head = 0;
    let refusal = loop {
        let (s, v) = a
            .append(
                &base,
                e,
                head,
                entries(
                    1,
                    json!([{"kind": "dispatched", "job": format!("j{head}")}]),
                ),
            )
            .await;
        if s != StatusCode::OK {
            break (s, v);
        }
        head = v["head"].as_u64().unwrap();
        assert!(head < 1000, "the quota never refused");
    };
    assert_eq!(refusal.0, StatusCode::INSUFFICIENT_STORAGE, "{}", refusal.1);
    assert_eq!(refusal.1["code"], "quota");
    assert_eq!(refusal.1["retryable"], true);
    // A snapshot at the head compacts the log: appends go on.
    let snap = serde_json::to_vec(&sealed(1)).unwrap();
    let (s, v) = a
        .put(
            &base,
            "/v1/pool/snapshot",
            json!({"epoch": e, "seq": head, "data": base64::engine::general_purpose::STANDARD.encode(&snap)}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = a
        .append(
            &base,
            e,
            head,
            entries(1, json!([{"kind": "dispatched", "job": "after"}])),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

// Free space (the reserve) is measured with statvfs, on Unix only.
#[cfg(unix)]
#[tokio::test]
async fn devices_still_enroll_while_the_reserve_refuses_pool_appends() {
    let issuer = FakeIssuer::new(ISSUER);
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("relay.json");
    // A reserve of the whole volume: every pool write is over budget.
    let base = start_with(
        &issuer,
        Some(state),
        StorageLimits {
            reserve_percent: 100,
            ..roomy()
        },
    )
    .await;
    let a = device(&issuer, &base, "ada").await;
    keys(&base, &a, &[&a]).await;
    let (_, v) = a.lease(&base, 0, 10, 60_000).await;
    let e = v["epoch"].as_u64().unwrap();
    let (s, v) = a
        .append(
            &base,
            e,
            0,
            entries(1, json!([{"kind": "submit", "run": "r"}])),
        )
        .await;
    assert_eq!(s, StatusCode::INSUFFICIENT_STORAGE, "{v}");
    assert_eq!(v["code"], "disk_reserve");
    // Device and session writes are not held to the reserve.
    let b = device(&issuer, &base, "ada").await;
    let (s, _) = b.get(&base, "/v1/pool").await;
    assert_eq!(s, StatusCode::OK);
    let r = reqwest::get(format!("{base}/readyz")).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["disk"]["pool_writes_refused"], true, "{v}");
    assert_eq!(v["pool_writes_refused"]["disk_reserve"], 1, "{v}");
    let metrics = reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains("cua_relay_pool_writes_refused_total{reason=\"disk_reserve\"} 1"),
        "{metrics}"
    );
}
