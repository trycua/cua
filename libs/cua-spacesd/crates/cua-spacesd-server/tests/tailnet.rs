// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Tailnet identity on a real listener: the peer address the authenticator
//! sees is the TCP peer, never a header a client sends.

mod common;

use std::sync::Arc;

use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_spacesd_server::tailnet::{TailnetAuth, WhoIs, PEER_HEADER};

use common::*;

#[tokio::test]
async fn a_client_cannot_claim_a_tailnet_address() {
    let driver = target().await;
    let ctx = driver.local.as_ref().expect("in process").ctx.clone();
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("host.json");
    std::fs::write(
        &policy,
        r#"{"owner_email":"alice@example.com","tailnet_identity":true}"#,
    )
    .unwrap();
    ctx.auth()
        .set_external(Arc::new(TailnetAuth::with_lookup(policy, |peer| {
            (peer.ip().to_string() == "100.101.1.1").then(|| WhoIs {
                login: "alice@example.com".into(),
                name: "Alice".into(),
                tags: vec![],
            })
        })));
    // No token, and a forged peer header claiming Alice's tailnet node.
    let forged = SpacesdClient::connect(
        ConnectOptions::parse(&driver.url)
            .unwrap()
            .transport(TransportPreference::Native)
            .probe(false)
            .header(PEER_HEADER, "100.101.1.1:41641"),
    )
    .await
    .unwrap();
    let err = forged
        .system()
        .health(cua_proto::env::v1::HealthRequest {})
        .await
        .expect_err("a forged peer header must not authenticate");
    assert_eq!(err.code(), tonic::Code::Unauthenticated, "{err}");
    // The env token still works next to tailnet identity.
    let c = driver.client(TransportPreference::Native).await;
    let (code, out) = run_sh(&c, "echo token-still-works").await;
    assert_eq!(
        (code, out.as_slice()),
        (Some(0), &b"token-still-works\n"[..])
    );
}
