// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The state volume's health on the wire (CUA-1297): `/readyz` writes a
//! probe file and fails while the volume cannot be written, a device, team
//! or pairing code write that cannot be stored is refused (never silently
//! dropped), `/metrics` counts it, and liveness (`/healthz`) does not
//! depend on the disk.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use base64::Engine as _;
use cua_relay::devices::{device_id, register_message, session_message};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

async fn start(issuer: &FakeIssuer, state: std::path::PathBuf) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(state),
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    base
}

/// Registers a new device of `sub` with a fresh sign-in.
async fn enroll(issuer: &FakeIssuer, base: &str, sub: &str) -> reqwest::Response {
    issuer.set_auth_time(sub, cua_relay::assertion::now_secs() as i64);
    let token = issuer.token(sub, Some(&format!("{sub}@example.com")), "cua-relay", 600);
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let id = device_id(pair.public_key().as_ref());
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let ts = cua_relay::assertion::now_secs();
    let sig = b64.encode(
        pair.sign(&rng, register_message(&id, ts).as_bytes())
            .unwrap()
            .as_ref(),
    );
    reqwest::Client::new()
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&json!({
            "public_key": b64.encode(pair.public_key().as_ref()),
            "name": "laptop", "ts": ts, "sig": sig, "bootstrap": true,
        }))
        .send()
        .await
        .unwrap()
}

/// Enrolls a device of `sub` and opens a session on it: (account token,
/// device session).
async fn signed_in(issuer: &FakeIssuer, base: &str, sub: &str) -> (String, String) {
    issuer.set_auth_time(sub, cua_relay::assertion::now_secs() as i64);
    let token = issuer.token(sub, Some(&format!("{sub}@example.com")), "cua-relay", 600);
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let id = device_id(pair.public_key().as_ref());
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let sign = |m: &str| b64.encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let ts = cua_relay::assertion::now_secs();
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&json!({
            "public_key": b64.encode(pair.public_key().as_ref()),
            "name": "laptop", "ts": ts, "sig": sign(&register_message(&id, ts)), "bootstrap": true,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let r = http
        .post(format!("{base}/v1/devices/session"))
        .bearer_auth(&token)
        .json(&json!({"device_id": id, "ts": ts, "sig": sign(&session_message(&id, ts))}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let v: Value = r.json().await.unwrap();
    (token, v["session"].as_str().unwrap().to_owned())
}

/// `POST <path>` as a signed-in device; the status.
async fn post_as(base: &str, who: &(String, String), path: &str, body: Value) -> StatusCode {
    reqwest::Client::new()
        .post(format!("{base}{path}"))
        .bearer_auth(&who.0)
        .header("x-cua-device-session", &who.1)
        .json(&body)
        .send()
        .await
        .unwrap()
        .status()
}

async fn readyz(base: &str) -> (StatusCode, Value) {
    let r = reqwest::get(format!("{base}/readyz")).await.unwrap();
    (r.status(), r.json().await.unwrap())
}

#[tokio::test]
async fn a_volume_that_cannot_be_written_fails_readiness_and_refuses_device_writes() {
    let issuer = FakeIssuer::new(ISSUER);
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    std::fs::create_dir(&state_dir).unwrap();
    let base = start(&issuer, state_dir.join("relay.json")).await;

    let (s, v) = readyz(&base).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ready"], true);
    assert_eq!(v["probe"]["ok"], true);
    assert!(v["disk"]["available_bytes"].as_u64().unwrap() > 0, "{v}");
    let r = enroll(&issuer, &base, "ada").await;
    assert!(r.status().is_success(), "{}", r.status());
    let cy = signed_in(&issuer, &base, "cy").await;
    // Teams are an optional hosted-relay feature (`hosted`).
    let teams = cfg!(feature = "hosted");
    let team = json!({"name": "Ops"});
    let code = json!({"ttl_secs": 600});
    if teams {
        assert_eq!(
            post_as(&base, &cy, "/v1/teams", team.clone()).await,
            StatusCode::CREATED
        );
    }
    assert!(post_as(&base, &cy, "/v1/pairing-codes", code.clone())
        .await
        .is_success());

    // The volume goes read-only (or full).
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    // The probe result is cached for a second.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (s, v) = readyz(&base).await;
    let refused = enroll(&issuer, &base, "bob").await;
    let refused_status = refused.status();
    let refused_body: Value = refused.json().await.unwrap_or(Value::Null);
    let team_refused = post_as(&base, &cy, "/v1/teams", team.clone()).await;
    let code_refused = post_as(&base, &cy, "/v1/pairing-codes", code.clone()).await;
    let live = reqwest::get(format!("{base}/healthz"))
        .await
        .unwrap()
        .status();
    let metrics = reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let (_, after) = readyz(&base).await;
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    if v["probe"]["ok"] == true {
        // Root ignores directory permissions; nothing to check then.
        return;
    }
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    assert_eq!(v["ready"], false);
    assert_eq!(
        live,
        StatusCode::NO_CONTENT,
        "liveness does not touch the disk"
    );
    // The new device was not enrolled: the caller is told, not misled.
    assert_eq!(
        refused_status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{refused_body}"
    );
    assert_eq!(after["stores"]["devices"]["ok"], false, "{after}");
    // A team or a pairing code that cannot be stored is refused too.
    if teams {
        assert_eq!(team_refused, StatusCode::INTERNAL_SERVER_ERROR);
    }
    assert_eq!(code_refused, StatusCode::INTERNAL_SERVER_ERROR);
    let stores: &[&str] = if teams {
        &["devices", "teams", "pairing"]
    } else {
        &["devices", "pairing"]
    };
    for &store in stores {
        assert!(
            after["failing"].as_array().unwrap().contains(&json!(store)),
            "{store}: {after}"
        );
        assert!(
            metrics.contains(&format!(
                "cua_relay_state_persist_failures_total{{store=\"{store}\"}} 1"
            )),
            "{store}: {metrics}"
        );
    }
    assert!(metrics.contains("cua_relay_state_probe_ok 0"), "{metrics}");

    // Writable again: the next write succeeds and readiness recovers.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let r = enroll(&issuer, &base, "bob").await;
    assert!(r.status().is_success(), "{}", r.status());
    if teams {
        assert_eq!(
            post_as(&base, &cy, "/v1/teams", team).await,
            StatusCode::CREATED
        );
    }
    assert!(post_as(&base, &cy, "/v1/pairing-codes", code)
        .await
        .is_success());
    let (s, v) = readyz(&base).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let info: Value = reqwest::get(format!("{base}/v1/info"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["state_writable"], true);
}
