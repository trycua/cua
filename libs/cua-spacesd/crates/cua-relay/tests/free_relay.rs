// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay without its optional hosted-relay features (the default
//! build): every account is Free, `/v1/usage` answers 404 like a relay that
//! meters nothing, the Teams routes answer 410 "not available on this
//! relay", a team pairing code finds no team, and the hosted Spaces API's
//! `/internal/v1/*` routes are not served.
#![cfg(not(feature = "hosted"))]

use std::sync::Arc;

use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

async fn start(issuer: &FakeIssuer) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    base
}

async fn send(
    method: reqwest::Method,
    url: String,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let r = reqwest::Client::new()
        .request(method, url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn teams_routes_answer_410_not_available() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer).await;
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    for (method, path) in [
        (reqwest::Method::GET, "/v1/teams"),
        (reqwest::Method::POST, "/v1/teams"),
        (reqwest::Method::POST, "/v1/teams/join"),
        (reqwest::Method::GET, "/v1/teams/t1"),
        (reqwest::Method::DELETE, "/v1/teams/t1/members/bob"),
        (reqwest::Method::POST, "/v1/teams/t1/machines"),
        (reqwest::Method::GET, "/join/team/abcd"),
    ] {
        let (s, v) = send(
            method.clone(),
            format!("{base}{path}"),
            &ada,
            json!({"name": "T"}),
        )
        .await;
        assert_eq!(s, StatusCode::GONE, "{method} {path}: {v}");
        assert_eq!(v["error"], "gone", "{method} {path}: {v}");
        assert_eq!(
            v["detail"], "Teams are not available on this relay",
            "{method} {path}: {v}"
        );
    }
}

#[tokio::test]
async fn usage_answers_404_metering_not_enabled() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer).await;
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 300);
    let (s, v) = send(
        reqwest::Method::GET,
        format!("{base}/v1/usage"),
        &ada,
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    assert_eq!(v["error"], "metering is not enabled on this relay");
}

#[tokio::test]
async fn internal_routes_are_not_served() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer).await;
    for (method, path) in [
        (reqwest::Method::POST, "/internal/v1/membership-changed"),
        (reqwest::Method::GET, "/internal/v1/records/machine-0001"),
        (reqwest::Method::GET, "/internal/v1/shared?sub=ada"),
        (reqwest::Method::DELETE, "/internal/v1/accounts/ada"),
    ] {
        let (s, v) = send(
            method.clone(),
            format!("{base}{path}"),
            "any",
            json!({"org": "o"}),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{method} {path}: {v}");
    }
}

#[tokio::test]
async fn metrics_carry_no_team_or_plan_counters() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer).await;
    let text = reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.contains("cua_relay_state_writes_total"), "{text}");
    assert!(!text.contains("cua_relay_org_"), "{text}");
    assert!(!text.contains("cua_relay_plan"), "{text}");
}
