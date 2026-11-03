// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Account isolation on a relay with both static tokens and accounts: an
//! account cannot take over an id connected with a static token, and an
//! unverified email never matches an allowlist.

use std::sync::Arc;
use std::time::Duration;

use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

async fn start(issuer: &FakeIssuer) -> (Relay, String) {
    let relay = Relay::new(RelayConfig {
        tokens: vec!["static-registration-token".into()],
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        // This file is about account/email isolation, not device
        // enrollment (see tests/device_enrollment.rs for that); leave
        // enrollment off so an unenrolled caller's request reaches the
        // directory check these tests assert on, instead of being refused
        // earlier for having no enrolled device.
        device_enrollment: false,
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

async fn post_machine(base: &str, token: &str, body: serde_json::Value) -> reqwest::StatusCode {
    reqwest::Client::new()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn an_account_cannot_register_an_id_connected_with_a_static_token() {
    let issuer = FakeIssuer::new(ISSUER);
    let (relay, base) = start(&issuer).await;
    // A self-hosted machine joins with the static token. Its local spacesd is
    // an unused port; only the registration matters here.
    let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = unused.local_addr().unwrap();
    drop(unused);
    let id = format!("static{}", uuid::Uuid::new_v4().simple());
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        "static-registration-token".into(),
        id.clone(),
        local,
    );
    join.heartbeat = Duration::from_secs(2);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(relay.machine_ids().contains(&id), "static machine joined");

    let eve = issuer.token("eve", Some("eve@example.com"), "cua-relay", 600);
    let status = post_machine(&base, &eve, serde_json::json!({ "id": id })).await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT);
    stop.cancel();
}

#[tokio::test]
async fn unverified_emails_do_not_match_allowlists() {
    let issuer = FakeIssuer::new(ISSUER);
    let (_relay, base) = start(&issuer).await;
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let id = format!("acct{}", uuid::Uuid::new_v4().simple());
    let status = post_machine(
        &base,
        &ada,
        serde_json::json!({ "id": id, "allow": ["bob@example.com"] }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CREATED);

    let get = |token: String| {
        let url = format!("{base}/v1/machines/{id}");
        async move {
            reqwest::Client::new()
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    // Someone else claiming Bob's address without verifying it sees nothing.
    let impostor = issuer.token_with("mallory", Some("bob@example.com"), false, "cua-relay", 600);
    assert_eq!(get(impostor).await, reqwest::StatusCode::NOT_FOUND);
    // Bob, verified, sees the shared machine.
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 600);
    assert_eq!(get(bob).await, reqwest::StatusCode::OK);
}
