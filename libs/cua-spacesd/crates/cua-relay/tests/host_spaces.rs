// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spaces a host provides are machines of their own that name their host:
//! only an account that can use the host (its owner, or an editor) may
//! register one, from an enrolled device, and lists carry the host so
//! clients group the Spaces under it.

use std::sync::Arc;

use cua_relay::devices::DevicePolicy;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

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

async fn call(
    method: reqwest::Method,
    url: String,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut r = reqwest::Client::new()
        .request(method, url)
        .bearer_auth(token);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let r = r.send().await.unwrap();
    let status = r.status();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_space_on_a_host_names_it_and_only_its_editors_may_add_one() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 14 * 86_400).await;
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 600);
    let post = |token: &String, body: Value| {
        let (url, token) = (format!("{base}/v1/machines"), token.clone());
        async move { call(reqwest::Method::POST, url, &token, Some(body)).await }
    };

    let (s, _) = post(
        &ada,
        json!({"id": "hostmachine1", "name": "Mac mini (spare)"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) = post(
        &ada,
        json!({"id": "space-ada00001", "name": "space-ada00001", "host": "hostmachine1"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["machine"]["host"], "hostmachine1");
    let (_, list) = call(
        reqwest::Method::GET,
        format!("{base}/v1/machines"),
        &ada,
        None,
    )
    .await;
    let child = list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "space-ada00001")
        .unwrap()
        .clone();
    assert_eq!(child["host"], "hostmachine1");
    let host = list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "hostmachine1")
        .unwrap()
        .clone();
    assert!(host.get("host").is_none(), "a host names no host: {host}");

    // Bob cannot add Spaces to a host he cannot use...
    let (s, v) = post(
        &bob,
        json!({"id": "space-bob00001", "host": "hostmachine1"}),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    // ...nor to one shared with him to watch only...
    let (s, _) = call(
        reqwest::Method::PATCH,
        format!("{base}/v1/machines/hostmachine1"),
        &ada,
        Some(json!({"viewers": ["bob@example.com"]})),
    )
    .await;
    assert!(s.is_success());
    let (s, v) = post(
        &bob,
        json!({"id": "space-bob00001", "host": "hostmachine1"}),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert!(v["error"].as_str().unwrap().contains("watch only"), "{v}");
    // ...but can once he is an editor; the Space is his.
    call(
        reqwest::Method::PATCH,
        format!("{base}/v1/machines/hostmachine1"),
        &ada,
        Some(json!({"viewers": [], "allow": ["bob@example.com"]})),
    )
    .await;
    let (s, v) = post(
        &bob,
        json!({"id": "space-bob00001", "host": "hostmachine1"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["machine"]["owner"]["id"], "bob");
}

#[tokio::test]
async fn adding_a_space_to_a_host_needs_an_enrolled_device() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 0).await;
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    // Registering the host itself needs only the account (hosting never
    // enrolls the host as a client).
    let (s, _) = call(
        reqwest::Method::POST,
        format!("{base}/v1/machines"),
        &ada,
        Some(json!({"id": "hostmachine2"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) = call(
        reqwest::Method::POST,
        format!("{base}/v1/machines"),
        &ada,
        Some(json!({"id": "space-ada00002", "host": "hostmachine2"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("enrolled device"),
        "{v}"
    );
}

/// A machine registered with metadata (a Space in the owner's own cloud:
/// its provider and place) shows it in every listing; bad metadata is
/// refused before anything is registered.
#[tokio::test]
async fn machine_metadata_round_trips_and_is_checked() {
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(&issuer, 14 * 86_400).await;
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let meta = json!({"cua.cloud.provider": "aws", "cua.cloud.place": "AWS \u{b7} us-west-2"});
    let (s, v) = call(
        reqwest::Method::POST,
        format!("{base}/v1/machines"),
        &ada,
        Some(json!({"id": "cloud-00000000000000aa", "name": "box", "meta": meta})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["machine"]["meta"], meta);
    let (_, one) = call(
        reqwest::Method::GET,
        format!("{base}/v1/machines/cloud-00000000000000aa"),
        &ada,
        None,
    )
    .await;
    assert_eq!(one["meta"], meta);
    let (s, _) = call(
        reqwest::Method::POST,
        format!("{base}/v1/machines"),
        &ada,
        Some(json!({"id": "cloud-00000000000000bb", "meta": {"Bad Key": "x"}})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        reqwest::Method::GET,
        format!("{base}/v1/machines/cloud-00000000000000bb"),
        &ada,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "nothing was registered");
}
