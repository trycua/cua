// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account's billing as the Spaces apps show it: the SDK's status from a
//! loopback account API, through the app core's billing line. The SDK side
//! alone is `cua-sdk/tests/fleet_billing.rs`.

use cua_daemon::{Runtime, RuntimeConfig};
use cua_sdk::Cua;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A loopback account API answering every request with `body`.
async fn account_api(body: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = s.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes()).await;
        }
    });
    url
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_status_carries_the_credit_and_the_billing_page() {
    let url = account_api(
        r#"{"billing_enabled": true, "payment_method_present": false, "card": null,
            "plan": "none", "payg_available": true,
            "credit": {"balance_usd_cents": 742, "signup_grant_usd_cents": 100, "signup_grant_unused": false},
            "billing_url": "https://run.cua.ai/billing"}"#,
    )
    .await;
    let dirs = tempfile::tempdir().unwrap();
    let account = cua_auth::account::AccountApi::from_lookup(&|k| match k {
        "CUA_FLEET_BASE_URL" => Some(url.clone()),
        "FLEETS_TOKEN" => Some("account-token".into()),
        _ => None,
    });
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        account,
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    let s = Cua::from_runtime(runtime)
        .fleet()
        .unwrap()
        .billing_status()
        .await
        .unwrap();
    assert_eq!(s.credit.as_ref().map(|c| c.balance_usd_cents), Some(742));
    assert_eq!(s.billing_url.as_deref(), Some("https://run.cua.ai/billing"));
    let app = cua_spaces_ffi::app_billing_status(s);
    assert_eq!(
        cua_spaces_app_core::billing::billing_line(&app),
        "$7.42 credit left"
    );
}
