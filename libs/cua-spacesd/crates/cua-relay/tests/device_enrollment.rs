// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Account-mode trust model: a machine token only serves its own machine;
//! client devices must be enrolled (second factor) to list or reach
//! machines, with a migration grace period; every access is audited.

use std::sync::Arc;

use base64::Engine as _;
use cua_relay::devices::{register_message, session_message, DevicePolicy};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

fn now() -> u64 {
    cua_relay::assertion::now_secs()
}

async fn start(issuer: &FakeIssuer, grace_secs: u64) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_policy: DevicePolicy {
            grace_secs,
            ..DevicePolicy::default()
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

struct Device {
    pair: ring::signature::EcdsaKeyPair,
    id: String,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

impl Device {
    fn new() -> Self {
        let rng = ring::rand::SystemRandom::new();
        let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
        let id = cua_relay::devices::device_id(pair.public_key().as_ref());
        Self { pair, id }
    }

    fn sign(&self, message: &str) -> String {
        let rng = ring::rand::SystemRandom::new();
        b64().encode(self.pair.sign(&rng, message.as_bytes()).unwrap().as_ref())
    }

    async fn register(&self, base: &str, token: &str, bootstrap: bool) -> Value {
        self.register_on(base, token, bootstrap, None).await
    }

    /// Registers as a device of the machine `machine_id` reports.
    async fn register_on(
        &self,
        base: &str,
        token: &str,
        bootstrap: bool,
        machine_id: Option<&str>,
    ) -> Value {
        let ts = now();
        let r = http()
            .post(format!("{base}/v1/devices"))
            .bearer_auth(token)
            .json(&json!({
                "public_key": b64().encode(self.pair.public_key().as_ref()),
                "name": "laptop",
                "ts": ts,
                "sig": self.sign(&register_message(&self.id, ts)),
                "bootstrap": bootstrap,
                "machine_id": machine_id,
            }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "{}", r.status());
        r.json().await.unwrap()
    }

    async fn session(&self, base: &str, token: &str) -> Result<String, StatusCode> {
        let ts = now();
        let r = http()
            .post(format!("{base}/v1/devices/session"))
            .bearer_auth(token)
            .json(&json!({
                "device_id": self.id,
                "ts": ts,
                "sig": self.sign(&session_message(&self.id, ts)),
            }))
            .send()
            .await
            .unwrap();
        if !r.status().is_success() {
            return Err(r.status());
        }
        let v: Value = r.json().await.unwrap();
        Ok(v["session"].as_str().unwrap().to_owned())
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

async fn get(base: &str, path: &str, token: &str, session: Option<&str>) -> reqwest::Response {
    let mut req = http().get(format!("{base}{path}")).bearer_auth(token);
    if let Some(s) = session {
        req = req.header("x-cua-device-session", s);
    }
    req.send().await.unwrap()
}

async fn register_machine(base: &str, token: &str, id: &str, allow: &[&str]) -> Value {
    let r = http()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(token)
        .json(&json!({"id": id, "name": id, "allow": allow}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    r.json().await.unwrap()
}

#[tokio::test]
async fn a_machine_token_only_serves_its_own_machine() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 14 * 86_400).await;
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let a = register_machine(&base, &ada, "machine-aaaa", &[]).await;
    register_machine(&base, &ada, "machine-bbbb", &[]).await;
    let token = a["machine_token"].as_str().unwrap().to_owned();

    // Lists only itself; cannot read the account's other machines.
    let list: Value = get(&base, "/v1/machines", &token, None)
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<_> = list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["machine-aaaa"]);
    assert_eq!(
        get(&base, "/v1/machines/machine-bbbb", &token, None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // Cannot reach another machine as a client (not an account token).
    let r = get(&base, "/m/machine-bbbb/v1/anything", &token, None).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // Cannot widen who reaches it, rename it, register or mint tokens.
    let r = http()
        .patch(format!("{base}/v1/machines/machine-aaaa"))
        .bearer_auth(&token)
        .json(&json!({"allow": ["mallory@example.com"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = http()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&token)
        .json(&json!({"id": "machine-cccc"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = http()
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&json!({"public_key": "x", "ts": now(), "sig": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // Its kill switch works, and it can undo its own stop...
    let stop = |t: String| {
        let base = base.clone();
        async move {
            http()
                .post(format!("{base}/v1/machines/machine-aaaa/stop-sharing"))
                .bearer_auth(t)
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    let start_sharing = |t: String| {
        let base = base.clone();
        async move {
            http()
                .post(format!("{base}/v1/machines/machine-aaaa/start-sharing"))
                .bearer_auth(t)
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    assert!(stop(token.clone()).await.is_success());
    assert!(start_sharing(token.clone()).await.is_success());
    // ...but not a stop the owner made from the account.
    assert!(stop(ada.clone()).await.is_success());
    assert_eq!(start_sharing(token.clone()).await, StatusCode::FORBIDDEN);
    assert!(start_sharing(ada.clone()).await.is_success());
}

#[tokio::test]
async fn a_machine_registered_without_proof_starts_unconfirmed() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 0).await;
    issuer.set_auth_time("ada", now() as i64);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let owner = Device::new();
    owner.register(&base, &ada, true).await;
    let owner_session = owner.session(&base, &ada).await.unwrap();

    // A stolen account session, with no enrolled device and no MFA, adds a
    // machine (hosting still needs only the account): it starts unconfirmed
    // (S5) instead of blending in as one of the owner's own.
    let r = register_machine(&base, &ada, "machine-rogue", &[]).await;
    assert_eq!(r["machine"]["confirmed"], false);
    let got: Value = get(
        &base,
        "/v1/machines/machine-rogue",
        &ada,
        Some(&owner_session),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(got["confirmed"], false);
    let audit: Value = get(&base, "/v1/audit", &ada, Some(&owner_session))
        .await
        .json()
        .await
        .unwrap();
    assert!(audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "machine_unconfirmed" && e["machine"] == "machine-rogue"));

    // A machine registered with an enrolled device's session starts
    // confirmed; so does one registered from a sign-in that proves MFA with
    // no device session at all.
    let r = http()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &owner_session)
        .json(&json!({"id": "machine-witnessed"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["machine"]["confirmed"], true);

    issuer.set_amr("ada", &["otp"]);
    let mfa = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let r = register_machine(&base, &mfa, "machine-mfa-witnessed", &[]).await;
    assert_eq!(r["machine"]["confirmed"], true);
    issuer.set_amr("ada", &[]);

    // Someone shared with as an editor cannot confirm the owner's rogue
    // machine.
    issuer.set_auth_time("bob", now() as i64);
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 300);
    let bob_device = Device::new();
    bob_device.register(&base, &bob, true).await;
    let bob_session = bob_device.session(&base, &bob).await.unwrap();
    http()
        .patch(format!("{base}/v1/machines/machine-rogue"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &owner_session)
        .json(&json!({"allow": ["bob@example.com"]}))
        .send()
        .await
        .unwrap();
    let r = http()
        .post(format!("{base}/v1/machines/machine-rogue/confirm"))
        .bearer_auth(&bob)
        .header("x-cua-device-session", &bob_session)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // The owner, from an enrolled device, confirms it.
    let r = http()
        .post(format!("{base}/v1/machines/machine-rogue/confirm"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &owner_session)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["confirmed"], true);
    let audit: Value = get(&base, "/v1/audit", &ada, Some(&owner_session))
        .await
        .json()
        .await
        .unwrap();
    assert!(audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "machine_confirmed" && e["machine"] == "machine-rogue"));
}

#[tokio::test]
async fn clients_need_an_enrolled_device_after_the_grace_period() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 0).await;
    // A long-lived session: signed in an hour ago.
    issuer.set_auth_time("ada", now() as i64 - 3600);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    // Hosting needs no enrolled device: a host only registers itself.
    let reg = register_machine(&base, &ada, "machine-aaaa", &[]).await;
    let machine_token = reg["machine_token"].as_str().unwrap().to_owned();

    // The account token alone lists nothing and reaches nothing.
    let r = get(&base, "/v1/machines", &ada, None).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(r.text().await.unwrap().contains("not enrolled"));
    let r = get(&base, "/m/machine-aaaa/v1/anything", &ada, None).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    // Re-registering (token rotation) needs the machine token or a device.
    let r = http()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&ada)
        .json(&json!({"id": "machine-aaaa", "allow": ["mallory@example.com"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = http()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&ada)
        .header(
            "x-cua-machine-authorization",
            format!("Bearer {machine_token}"),
        )
        .json(&json!({"id": "machine-aaaa"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());

    // A stale session cannot bootstrap the first device.
    let first = Device::new();
    let r = first.register(&base, &ada, true).await;
    assert_eq!(r["device"]["state"], "pending");
    assert!(first.session(&base, &ada).await.is_err());
    // A fresh interactive sign-in can.
    issuer.set_auth_time("ada", now() as i64);
    let fresh = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let r = first.register(&base, &fresh, true).await;
    assert_eq!(r["device"]["state"], "enrolled");
    let s1 = first.session(&base, &fresh).await.unwrap();
    let r = get(&base, "/v1/machines", &fresh, Some(&s1)).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().get("x-cua-device-enrollment").is_none());
    // Past the gate: the machine is just not connected.
    let r = get(&base, "/m/machine-aaaa/v1/anything", &fresh, Some(&s1)).await;
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
    // The session is bound to its account.
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 300);
    assert_eq!(
        get(&base, "/v1/machines", &bob, Some(&s1)).await.status(),
        StatusCode::FORBIDDEN
    );

    // A new device on a long-lived session shows a code; an enrolled
    // device confirms it.
    let second = Device::new();
    let r = second.register(&base, &ada, true).await;
    assert_eq!(r["device"]["state"], "pending");
    let code = r["code"].as_str().unwrap().to_owned();
    assert_eq!(
        second.session(&base, &fresh).await,
        Err(StatusCode::FORBIDDEN)
    );
    let approve = |session: Option<String>, body: Value| {
        let base = base.clone();
        let fresh = fresh.clone();
        async move {
            let mut req = http()
                .post(format!("{base}/v1/devices/approve"))
                .bearer_auth(fresh)
                .json(&body);
            if let Some(s) = session {
                req = req.header("x-cua-device-session", s);
            }
            req.send().await.unwrap().status()
        }
    };
    // Not without an enrolled device's session.
    assert_eq!(
        approve(None, json!({"code": code})).await,
        StatusCode::FORBIDDEN
    );
    assert!(approve(Some(s1.clone()), json!({"code": code}))
        .await
        .is_success());
    let s2 = second.session(&base, &fresh).await.unwrap();
    assert_eq!(
        get(&base, "/v1/machines", &fresh, Some(&s2)).await.status(),
        StatusCode::OK
    );

    // Devices are listed, renamed and revoked from an enrolled device.
    let list: Value = get(&base, "/v1/devices", &fresh, Some(&s1))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(list["devices"].as_array().unwrap().len(), 2);
    let r = http()
        .patch(format!("{base}/v1/devices/{}", second.id))
        .bearer_auth(&fresh)
        .header("x-cua-device-session", &s1)
        .json(&json!({"name": "work phone"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let r = http()
        .delete(format!("{base}/v1/devices/{}", second.id))
        .bearer_auth(&fresh)
        .header("x-cua-device-session", &s1)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    assert_eq!(
        get(&base, "/v1/machines", &fresh, Some(&s2)).await.status(),
        StatusCode::FORBIDDEN
    );

    // The audit log shows who did what, from which device.
    let audit: Value = get(&base, "/v1/audit", &fresh, Some(&s1))
        .await
        .json()
        .await
        .unwrap();
    let kinds: Vec<_> = audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect();
    for kind in [
        "machine_registered",
        "machine_reregistered",
        "device_registered",
        "device_enrolled",
        "device_session",
        "machine_access",
        "device_renamed",
        "device_revoked",
    ] {
        assert!(
            kinds.iter().any(|k| k == kind),
            "{kind} missing in {kinds:?}"
        );
    }
    let access = audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "machine_access")
        .unwrap();
    assert_eq!(access["device"], first.id.as_str());
    assert_eq!(access["machine"], "machine-aaaa");
}

#[tokio::test]
async fn shares_are_audited_for_the_owner_and_revocable() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 0).await;
    issuer.set_auth_time("ada", now() as i64);
    issuer.set_auth_time("bob", now() as i64);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 300);
    let ada_dev = Device::new();
    ada_dev.register(&base, &ada, true).await;
    let sa = ada_dev.session(&base, &ada).await.unwrap();
    let bob_dev = Device::new();
    bob_dev.register(&base, &bob, true).await;
    let sb = bob_dev.session(&base, &bob).await.unwrap();
    register_machine(&base, &ada, "machine-aaaa", &["bob@example.com"]).await;

    // Bob (enrolled on his own account) reaches the shared machine.
    let r = get(&base, "/m/machine-aaaa/v1/anything", &bob, Some(&sb)).await;
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
    // Ada revokes the share.
    let r = http()
        .patch(format!("{base}/v1/machines/machine-aaaa"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &sa)
        .json(&json!({"allow": []}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let r = get(&base, "/m/machine-aaaa/v1/anything", &bob, Some(&sb)).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    let audit: Value = get(&base, "/v1/audit", &ada, Some(&sa))
        .await
        .json()
        .await
        .unwrap();
    let events = audit["events"].as_array().unwrap();
    let find = |kind: &str| events.iter().find(|e| e["kind"] == kind).cloned();
    assert_eq!(find("share_added").unwrap()["subject"], "bob@example.com");
    assert_eq!(find("shared_access").unwrap()["subject"], "bob@example.com");
    let removed = find("share_removed").unwrap();
    assert_eq!(removed["subject"], "bob@example.com");
    assert_eq!(removed["device"], ada_dev.id.as_str());
    // Bob's own log records his access; Ada's log is not his.
    let bob_audit: Value = get(&base, "/v1/audit", &bob, Some(&sb))
        .await
        .json()
        .await
        .unwrap();
    let bob_kinds: Vec<_> = bob_audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(bob_kinds.contains(&"machine_access".to_string()));
    assert!(!bob_kinds.contains(&"share_added".to_string()));
}

#[tokio::test]
async fn the_audit_log_is_hash_chained_and_exportable_as_jsonl() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 0).await;
    issuer.set_auth_time("ada", now() as i64);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let dev = Device::new();
    dev.register(&base, &ada, true).await;
    let session = dev.session(&base, &ada).await.unwrap();
    register_machine(&base, &ada, "machine-aaaa", &[]).await;

    let audit: Value = get(&base, "/v1/audit", &ada, Some(&session))
        .await
        .json()
        .await
        .unwrap();
    let events = audit["events"].as_array().unwrap();
    assert!(events.len() >= 2, "{events:?}");
    // Every relay-visible event is chained: a real 64-hex-char hash and
    // mac, and the first entry's prev is the genesis sentinel.
    assert_eq!(
        events[0]["prev"],
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
    for w in events.windows(2) {
        assert_eq!(w[1]["prev"], w[0]["hash"]);
        assert_ne!(w[0]["hash"].as_str().unwrap(), "");
        assert_ne!(w[0]["mac"].as_str().unwrap(), "");
    }

    // The jsonl export carries the same chained entries, one per line.
    let r = http()
        .get(format!("{base}/v1/audit?format=jsonl"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &session)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    assert_eq!(
        r.headers().get("content-type").unwrap(),
        "application/x-ndjson; charset=utf-8"
    );
    let body = r.text().await.unwrap();
    let lines: Vec<Value> = body
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), events.len());
    assert_eq!(
        lines.last().unwrap()["hash"],
        events.last().unwrap()["hash"]
    );
}

#[tokio::test]
async fn unenrolled_devices_keep_access_during_the_grace_period_but_are_flagged() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 14 * 86_400).await;
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    register_machine(&base, &ada, "machine-aaaa", &[]).await;
    let r = get(&base, "/v1/machines", &ada, None).await;
    assert_eq!(r.status(), StatusCode::OK);
    let flag = r
        .headers()
        .get("x-cua-device-enrollment")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(flag.starts_with("required; enforce-after="), "{flag}");
    let r = get(&base, "/m/machine-aaaa/v1/anything", &ada, None).await;
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
    let audit: Value = get(&base, "/v1/audit", &ada, None)
        .await
        .json()
        .await
        .unwrap();
    assert!(audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "unenrolled_access"));
    // During the grace period the first device may bootstrap without a
    // fresh sign-in.
    let d = Device::new();
    let r = d.register(&base, &ada, true).await;
    assert_eq!(r["device"]["enrolled_by"], "bootstrap:grace");
}

#[tokio::test]
async fn a_fresh_sign_in_enrolls_this_device_and_replaces_its_old_key() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 0).await;
    issuer.set_auth_time("ada", now() as i64);
    let fresh = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    // The unsigned build of the app enrolled this Mac with its key.
    let unsigned = Device::new();
    let r = unsigned
        .register_on(&base, &fresh, true, Some("mac-hw-uuid-hash"))
        .await;
    assert_eq!(r["device"]["state"], "enrolled");
    let s_old = unsigned.session(&base, &fresh).await.unwrap();

    // The signed build keeps its own key. Signed in an hour ago, it waits
    // for an approval, like any device on a long-lived session.
    issuer.set_auth_time("ada", now() as i64 - 3600);
    let stale = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let signed = Device::new();
    let r = signed
        .register_on(&base, &stale, true, Some("mac-hw-uuid-hash"))
        .await;
    assert_eq!(r["device"]["state"], "pending");
    assert!(r["code"].is_string());

    // Signing in again is enough: it enrolls at once and replaces the old
    // key of the same Mac instead of listing it twice.
    issuer.set_auth_time("ada", now() as i64);
    let again = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let r = signed
        .register_on(&base, &again, true, Some("mac-hw-uuid-hash"))
        .await;
    assert_eq!(r["device"]["state"], "enrolled");
    assert_eq!(r["device"]["enrolled_by"], "bootstrap:fresh-sign-in");
    assert_eq!(r["superseded"], json!([unsigned.id]));
    assert!(r["code"].is_null());
    let s_new = signed.session(&base, &again).await.unwrap();
    // The old key's session ended; it cannot open another.
    assert_eq!(
        get(&base, "/v1/machines", &again, Some(&s_old))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        unsigned.session(&base, &again).await,
        Err(StatusCode::FORBIDDEN)
    );
    let list: Value = get(&base, "/v1/devices", &again, Some(&s_new))
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<_> = list["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, std::slice::from_ref(&signed.id));

    // A brand-new machine, from a session that does not prove a second
    // factor: a verified email is no longer enough on its own once the
    // account has a strong enrolled device (S4). It waits for approval.
    let laptop = Device::new();
    let r = laptop
        .register_on(&base, &again, true, Some("linux-machine-id-hash"))
        .await;
    assert_eq!(r["device"]["state"], "pending");
    assert!(r["code"].is_string());

    // The same sign-in, now proving MFA (Keycloak `amr`), enrolls it at
    // once and keeps both machines.
    issuer.set_amr("ada", &["otp"]);
    let mfa_signed_in = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let r = laptop
        .register_on(&base, &mfa_signed_in, true, Some("linux-machine-id-hash"))
        .await;
    assert_eq!(r["device"]["state"], "enrolled");
    assert_eq!(r["superseded"], json!([]));
    issuer.set_amr("ada", &[]);

    // The audit log says how each device got in.
    let audit: Value = get(&base, "/v1/audit", &again, Some(&s_new))
        .await
        .json()
        .await
        .unwrap();
    let events = audit["events"].as_array().unwrap();
    let by_sign_in: Vec<_> = events
        .iter()
        .filter(|e| e["kind"] == "device_enrolled" && e["detail"] == "enrolled by fresh sign-in")
        .map(|e| e["device"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        by_sign_in,
        [unsigned.id.clone(), signed.id.clone(), laptop.id.clone()]
    );
    let rekeyed = events
        .iter()
        .find(|e| e["kind"] == "device_rekeyed")
        .unwrap();
    assert_eq!(rekeyed["device"], signed.id.as_str());
    assert_eq!(rekeyed["subject"], unsigned.id.as_str());
}

#[tokio::test]
async fn an_unverified_email_needs_an_approval_for_further_devices() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 0).await;
    issuer.set_auth_time("eve", now() as i64);
    let unverified = issuer.token_with("eve", Some("eve@example.com"), false, "cua-relay", 300);
    // The account's first device still enrolls by a fresh sign-in.
    let first = Device::new();
    let r = first.register(&base, &unverified, true).await;
    assert_eq!(r["device"]["state"], "enrolled");
    // Further devices need an approval until the email is verified.
    let second = Device::new();
    let r = second.register(&base, &unverified, true).await;
    assert_eq!(r["device"]["state"], "pending");
    assert!(r["code"].is_string());
    let s1 = first.session(&base, &unverified).await.unwrap();
    // Approving by the device id works as well as by the code; an unknown
    // code says how to approve instead.
    let approve = |body: Value| {
        let base = base.clone();
        let token = unverified.clone();
        let s1 = s1.clone();
        async move {
            http()
                .post(format!("{base}/v1/devices/approve"))
                .bearer_auth(token)
                .header("x-cua-device-session", s1)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };
    let r = approve(json!({"code": "ZZZZ-ZZZZ"})).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["code"], "code_not_found");
    assert!(v["error"].as_str().unwrap().contains("approve by id"));
    let r = approve(json!({"code": second.id})).await;
    assert!(r.status().is_success(), "{}", r.status());
    assert!(second.session(&base, &unverified).await.is_ok());
}
