// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `GET /internal/v1/devices/{id}` (CUA-2339): the read-only device lookup
//! in-cluster services call. It needs the internal token, is
//! absent when none is configured, refuses proxied requests, and reports
//! the device's account, key and enrollment state.
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::sync::Arc;

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
const INTERNAL: &str = "internal-secret-for-device-lookup";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

async fn start(issuer: &FakeIssuer, internal: Option<&str>) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        org_access: OrgAccessConfig {
            internal_token: internal.map(str::to_owned),
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

struct Device {
    pair: ring::signature::EcdsaKeyPair,
    id: String,
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

    /// Enrolls by a fresh sign-in and opens a session.
    async fn enroll(&self, base: &str, token: &str) -> String {
        let http = reqwest::Client::new();
        let ts = cua_relay::assertion::now_secs();
        let r = http
            .post(format!("{base}/v1/devices"))
            .bearer_auth(token)
            .json(&json!({
                "public_key": b64().encode(self.pair.public_key().as_ref()),
                "name": "laptop",
                "ts": ts,
                "sig": self.sign(&register_message(&self.id, ts)),
                "bootstrap": true,
            }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "{}", r.status());
        let r = http
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
        assert!(r.status().is_success(), "{}", r.status());
        let v: Value = r.json().await.unwrap();
        v["session"].as_str().unwrap().to_owned()
    }
}

async fn lookup(base: &str, id: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut req = reqwest::Client::new().get(format!("{base}/internal/v1/devices/{id}"));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let r = req.send().await.unwrap();
    (r.status(), r.text().await.unwrap())
}

fn token(issuer: &FakeIssuer, sub: &str) -> String {
    issuer.set_auth_time(sub, cua_relay::assertion::now_secs() as i64);
    issuer.token(sub, None, "cua-relay", 300)
}

#[tokio::test]
async fn reports_an_enrolled_device_with_its_key_and_account() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, Some(INTERNAL)).await;
    let device = Device::new();
    device.enroll(&base, &token(&issuer, "ada")).await;

    let (status, body) = lookup(&base, &device.id, Some(INTERNAL)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["id"], device.id);
    assert_eq!(v["account"], "ada");
    assert_eq!(v["state"], "enrolled");
    assert_eq!(v["enrolled"], true);
    assert_eq!(v["revoked"], false);
    let public = device.pair.public_key().as_ref();
    assert_eq!(
        v["public_jwk"],
        json!({
            "kty": "EC", "crv": "P-256",
            "x": b64().encode(&public[1..33]),
            "y": b64().encode(&public[33..65]),
        })
    );

    // Unknown ids and malformed ids are a plain 404.
    for id in ["dev_000000000000000000000000", "not-a-device"] {
        let (status, body) = lookup(&base, id, Some(INTERNAL)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("device_not_found"), "{body}");
    }
}

#[tokio::test]
async fn a_revoked_device_reads_as_revoked() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, Some(INTERNAL)).await;
    let device = Device::new();
    let ada = token(&issuer, "ada");
    let session = device.enroll(&base, &ada).await;
    let r = reqwest::Client::new()
        .delete(format!("{base}/v1/devices/{}", device.id))
        .bearer_auth(&ada)
        .header("x-cua-device-session", session)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let (status, body) = lookup(&base, &device.id, Some(INTERNAL)).await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["state"], "revoked");
    assert_eq!(v["enrolled"], false);
    assert_eq!(v["revoked"], true);
}

#[tokio::test]
async fn needs_the_internal_token_and_a_direct_request() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, Some(INTERNAL)).await;
    let device = Device::new();
    let ada = token(&issuer, "ada");
    device.enroll(&base, &ada).await;

    // No token, a wrong token, an account token: refused.
    assert_eq!(
        lookup(&base, &device.id, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        lookup(&base, &device.id, Some("internal-secret-for-device-lookuX"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        lookup(&base, &device.id, Some(&ada)).await.0,
        StatusCode::UNAUTHORIZED
    );
    // Through a proxy (the ingress): as if the route did not exist.
    let r = reqwest::Client::new()
        .get(format!("{base}/internal/v1/devices/{}", device.id))
        .bearer_auth(INTERNAL)
        .header("X-Forwarded-For", "203.0.113.9")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert_eq!(r.text().await.unwrap(), "not found");
}

#[tokio::test]
async fn is_absent_without_an_internal_token() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, None).await;
    let device = Device::new();
    device.enroll(&base, &token(&issuer, "ada")).await;
    let (status, body) = lookup(&base, &device.id, Some(INTERNAL)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, "not found");
}
