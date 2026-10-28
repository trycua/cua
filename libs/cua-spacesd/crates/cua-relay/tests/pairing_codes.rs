// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Adding a machine by link or QR: an enrolled device mints a one-time
//! pairing code, the new machine registers with it instead of an account
//! token, and the code is then spent. Codes can be revoked, never work for
//! another account's listing, and need an enrolled device to mint.

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

async fn start(issuer: &FakeIssuer) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_policy: DevicePolicy {
            grace_secs: 0,
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

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// An enrolled device of the account behind `token`; returns its session.
async fn enrolled_session(base: &str, token: &str) -> String {
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let id = cua_relay::devices::device_id(pair.public_key().as_ref());
    let sign = |m: &str| b64().encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let ts = now();
    let r = http()
        .post(format!("{base}/v1/devices"))
        .bearer_auth(token)
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
    assert!(r.status().is_success(), "{}", r.status());
    let r = http()
        .post(format!("{base}/v1/devices/session"))
        .bearer_auth(token)
        .json(&json!({"device_id": id, "ts": ts, "sig": sign(&session_message(&id, ts))}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let v: Value = r.json().await.unwrap();
    v["session"].as_str().unwrap().to_owned()
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

async fn mint(base: &str, token: &str, session: Option<&str>, body: Value) -> reqwest::Response {
    let mut r = http()
        .post(format!("{base}/v1/pairing-codes"))
        .bearer_auth(token)
        .json(&body);
    if let Some(s) = session {
        r = r.header("x-cua-device-session", s);
    }
    r.send().await.unwrap()
}

async fn register_with_code(base: &str, code: &str, id: &str) -> reqwest::Response {
    http()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(format!("cpc_{code}"))
        .json(&json!({"id": id, "name": "studio", "meta": {"cua.host.os": "linux"}}))
        .send()
        .await
        .unwrap()
}

async fn list_machines(base: &str, token: &str, session: &str) -> Vec<Value> {
    let r = http()
        .get(format!("{base}/v1/machines"))
        .bearer_auth(token)
        .header("x-cua-device-session", session)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = r.json().await.unwrap();
    v["machines"].as_array().unwrap().clone()
}

async fn codes(base: &str, token: &str, session: &str) -> Vec<Value> {
    let r = http()
        .get(format!("{base}/v1/pairing-codes"))
        .bearer_auth(token)
        .header("x-cua-device-session", session)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = r.json().await.unwrap();
    v["codes"].as_array().unwrap().clone()
}

#[tokio::test]
async fn a_pairing_code_adds_one_machine_to_the_account_once() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer).await;
    issuer.set_auth_time("ada", now() as i64);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let session = enrolled_session(&base, &ada).await;

    // Minting needs an enrolled device.
    let r = mint(&base, &ada, None, json!({})).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // A team code: only for a team the caller manages (and Teams are an
    // optional hosted-relay feature: without it no team is found).
    let r = mint(&base, &ada, Some(&session), json!({"team": "team-nope"})).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    #[cfg(feature = "hosted")]
    let team_id = {
        let team: Value = http()
            .post(format!("{base}/v1/teams"))
            .bearer_auth(&ada)
            .header("x-cua-device-session", &session)
            .json(&json!({"name": "AIGIS"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        team["id"].as_str().expect("team id").to_owned()
    };
    #[cfg(feature = "hosted")]
    let body = json!({"team": team_id});
    #[cfg(not(feature = "hosted"))]
    let body = json!({});
    let r = mint(&base, &ada, Some(&session), body).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let minted: Value = r.json().await.unwrap();
    let code = minted["code"].as_str().unwrap().to_owned();
    assert_eq!(code.len(), 14, "XXXX-XXXX-XXXX: {code}");
    assert_eq!(minted["state"], "active");
    assert!(minted["expires_at"].as_u64().unwrap() > now() + 25 * 60);

    // Listing never shows the code itself.
    let listed = codes(&base, &ada, &session).await;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].get("code").is_none());

    // The new machine registers with only the code (any case, no dashes).
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    let typed = code.replace('-', "").to_lowercase();
    let r = register_with_code(&base, &typed, &id).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let reg: Value = r.json().await.unwrap();
    assert!(reg["machine_token"].as_str().unwrap().starts_with("cmt_"));
    assert_eq!(reg["machine"]["owner"]["id"], "ada");
    assert_eq!(
        reg["machine"]["confirmed"], true,
        "minted by an enrolled device"
    );
    assert_eq!(reg["machine"]["meta"]["cua.host.os"], "linux");

    // It is in the account's machine list, and the code is spent.
    let machines = list_machines(&base, &ada, &session).await;
    assert!(machines.iter().any(|m| m["id"] == id.as_str()));
    // ... and on the team.
    #[cfg(feature = "hosted")]
    {
        let team: Value = http()
            .get(format!("{base}/v1/teams/{team_id}"))
            .bearer_auth(&ada)
            .header("x-cua-device-session", &session)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            team.to_string().contains(&id),
            "machine on the team: {team}"
        );
    }
    let listed = codes(&base, &ada, &session).await;
    assert_eq!(listed[0]["state"], "used");
    assert_eq!(listed[0]["machine"], id.as_str());
    let again = format!("m{}", uuid::Uuid::new_v4().simple());
    let r = register_with_code(&base, &code, &again).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Revoking the device that made a code voids the code.
    let second: Value = mint(&base, &ada, Some(&session), json!({}))
        .await
        .json()
        .await
        .unwrap();
    let devices: Value = http()
        .get(format!("{base}/v1/devices"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let dev = devices["devices"][0]["id"].as_str().unwrap().to_owned();
    let audit_before = http()
        .get(format!("{base}/v1/audit"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &session)
        .send()
        .await
        .unwrap();
    let audit: Value = audit_before.json().await.unwrap();
    let r = http()
        .delete(format!("{base}/v1/devices/{dev}"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &session)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let third = format!("m{}", uuid::Uuid::new_v4().simple());
    let r = register_with_code(&base, second["code"].as_str().unwrap(), &third).await;
    assert_eq!(
        r.status(),
        StatusCode::UNAUTHORIZED,
        "minting device revoked"
    );

    // The registration is audited with the minting device.
    let kinds: Vec<_> = audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        kinds.contains(&"pairing_code_created".to_owned()),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&"machine_registered".to_owned()),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn revoked_mistyped_and_foreign_codes_add_nothing() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer).await;
    issuer.set_auth_time("ada", now() as i64);
    issuer.set_auth_time("eve", now() as i64);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let eve = issuer.token("eve", Some("eve@example.com"), "cua-relay", 300);
    let ada_s = enrolled_session(&base, &ada).await;
    let eve_s = enrolled_session(&base, &eve).await;

    let minted: Value = mint(&base, &ada, Some(&ada_s), json!({"ttl_secs": 600}))
        .await
        .json()
        .await
        .unwrap();
    let code = minted["code"].as_str().unwrap().to_owned();
    let pid = minted["id"].as_str().unwrap().to_owned();

    // Another account can neither see nor revoke it.
    assert!(codes(&base, &eve, &eve_s).await.is_empty());
    let r = http()
        .delete(format!("{base}/v1/pairing-codes/{pid}"))
        .bearer_auth(&eve)
        .header("x-cua-device-session", &eve_s)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    // A mistyped code is refused and does not spend the real one.
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    let mut wrong = code.clone();
    wrong.replace_range(0..1, if code.starts_with('A') { "B" } else { "A" });
    assert_eq!(
        register_with_code(&base, &wrong, &id).await.status(),
        StatusCode::UNAUTHORIZED
    );
    // A failed registration (bad machine id) does not spend it either.
    assert_eq!(
        register_with_code(&base, &code, "BAD ID").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(codes(&base, &ada, &ada_s).await[0]["state"], "active");

    // Revoked by its owner: refused.
    let r = http()
        .delete(format!("{base}/v1/pairing-codes/{pid}"))
        .bearer_auth(&ada)
        .header("x-cua-device-session", &ada_s)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let r = register_with_code(&base, &code, &id).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let body: Value = r.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("not valid"),
        "{body}"
    );
    assert_eq!(codes(&base, &ada, &ada_s).await[0]["state"], "revoked");
    assert!(list_machines(&base, &ada, &ada_s).await.is_empty());

    // A code is no account token anywhere else.
    let r = http()
        .get(format!("{base}/v1/machines"))
        .bearer_auth(format!("cpc_{code}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}
