// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay keeps serving while its disk stalls (CUA-4413).
//!
//! On 23 Nov 2026 a ~80 s I/O pause on the relay's node stopped the whole
//! relay: state writes fsynced under locks that request handlers take, every
//! tokio worker ended up blocked, and outbound calls (Spaces API, OIDC
//! discovery, PostHog) timed out together. These tests hold a state writer
//! as a hung fsync would and check that requests, lookups and outbound
//! calls carry on, and that the held changes land once the disk answers.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::oidc::testing::FakeIssuer;
use crate::oidc::{OidcConfig, OidcValidator};
use crate::server::{Relay, RelayConfig};

const ISSUER: &str = "https://auth.test/realms/cua";

/// What must answer while the disk is stalled.
const SNAPPY: Duration = Duration::from_secs(5);
/// When a test's stall is lifted even if the relay under test is stuck on it.
const WATCHDOG: Duration = Duration::from_secs(15);

async fn serve(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    base
}

// Two workers and more stalled requests than workers: if a stalled write
// held a worker (or a lock a worker waits on), nothing below could answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_and_outbound_calls_proceed_while_the_state_disk_is_stalled() {
    let dir = tempfile::tempdir().unwrap();
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(dir.path().join("relay.json")),
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    {
        let relay = relay.clone();
        tokio::spawn(async move {
            let _ = relay.serve(listener, std::future::pending()).await;
        });
    }
    // An upstream standing in for the Spaces API, the issuer or PostHog.
    let upstream =
        serve(axum::Router::new().route("/ping", axum::routing::get(|| async { "pong" }))).await;
    let http = reqwest::Client::new();
    let ada = issuer.token("ada-sub", Some("ada@example.com"), "cua-relay", 300);

    // One machine registered before the stall.
    let first = http
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&ada)
        .json(&json!({"id": "machine-0000", "name": "before"}))
        .send()
        .await
        .unwrap();
    assert!(first.status().is_success(), "{}", first.status());

    // The disk stops answering: the directory's writer holds its next write.
    // A watchdog lifts the stall after a while, so a relay that blocks on it
    // fails the timing checks below instead of hanging the test.
    let stall = relay.directory.file.stall();
    {
        let relay = relay.clone();
        std::thread::spawn(move || {
            std::thread::sleep(WATCHDOG);
            relay.directory.file.unstall();
        });
    }
    let mut held = Vec::new();
    for i in 1..=6 {
        let (http, base, ada) = (http.clone(), base.clone(), ada.clone());
        held.push(tokio::spawn(async move {
            http.post(format!("{base}/v1/machines"))
                .bearer_auth(&ada)
                .json(&json!({"id": format!("machine-000{i}"), "name": format!("m{i}")}))
                .send()
                .await
                .unwrap()
                .status()
        }));
    }
    // Let the registrations reach the stalled writer.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        held.iter().all(|h| !h.is_finished()),
        "a registration answers only once its change is on disk"
    );

    // Requests that read the same store answer at once ...
    let during = std::time::Instant::now();
    let listed = tokio::time::timeout(
        SNAPPY,
        http.get(format!("{base}/v1/machines"))
            .bearer_auth(&ada)
            .send(),
    )
    .await
    .expect("the machine list answers during the stall")
    .unwrap();
    assert!(listed.status().is_success());
    let listed: Value = listed.json().await.unwrap();
    assert!(listed.to_string().contains("machine-0000"), "{listed}");
    let health = tokio::time::timeout(SNAPPY, http.get(format!("{base}/healthz")).send())
        .await
        .expect("health answers during the stall")
        .unwrap();
    assert!(health.status().is_success());
    // ... the store's lock is free (the write holds none) ...
    assert!(relay.directory.get("machine-0000").is_some());
    // ... and outbound calls go out and come back.
    for _ in 0..3 {
        let pong = tokio::time::timeout(SNAPPY, http.get(format!("{upstream}/ping")).send())
            .await
            .expect("an outbound call completes during the stall")
            .unwrap();
        assert_eq!(pong.text().await.unwrap(), "pong");
    }
    // Timers still fire on time.
    let started = std::time::Instant::now();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        during.elapsed() < SNAPPY,
        "the relay answered only after the disk did ({:?})",
        during.elapsed()
    );
    // The writer is reported behind.
    assert!(
        relay
            .storage
            .writer_behind(crate::storage::store::DIRECTORY)
            > Duration::ZERO
    );
    assert!(relay.storage.metrics().lines().any(|l| l
        .starts_with("cua_relay_state_writer_behind_seconds{store=\"directory\"}")
        && !l.ends_with(" 0.000")));

    // The disk answers again: every held registration completes, in one or
    // two coalesced writes, and all of them are on disk.
    drop(stall);
    for h in held {
        let status = tokio::time::timeout(SNAPPY, h).await.unwrap().unwrap();
        assert!(status.is_success(), "{status}");
    }
    let raw = std::fs::read_to_string(dir.path().join("relay.json")).unwrap();
    for i in 0..=6 {
        assert!(raw.contains(&format!("machine-000{i}")), "{raw}");
    }
    assert_eq!(
        relay
            .storage
            .writer_behind(crate::storage::store::DIRECTORY),
        Duration::ZERO
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_sessions_open_and_are_checked_while_their_journal_is_stalled() {
    use crate::devices::tests::TestKey;
    use crate::devices::{register_message, session_message, Registration};
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        crate::devices::DeviceStore::open(dir.path().join("devices.json"), Default::default())
            .unwrap(),
    );
    let key = TestKey::new();
    let ts = crate::assertion::now_secs();
    store
        .register(&Registration {
            account: "acct",
            user: "acct",
            public_key: &key.public(),
            name: "laptop",
            platform: "macos",
            ts,
            sig: &key.sign(&register_message(&key.id(), ts)),
            bootstrap: true,
            auth_time: Some(ts),
            ..Registration::default()
        })
        .await
        .unwrap();
    // The session journal and the audit log stop answering.
    let journal = store.stall_sessions();
    let audit = store.stall_audit();
    let mut tokens = Vec::new();
    for _ in 0..4 {
        let ts = crate::assertion::now_secs();
        let opened = tokio::time::timeout(
            SNAPPY,
            store.open_session(
                "acct",
                &key.id(),
                ts,
                &key.sign(&session_message(&key.id(), ts)),
            ),
        )
        .await
        .expect("a session opens while its journal write waits")
        .unwrap();
        tokens.push(opened.0);
    }
    for t in &tokens {
        assert_eq!(store.session_device("acct", t), Some(key.id()));
    }
    drop((journal, audit));
    store.flush();
    let lines = std::fs::read_to_string(dir.path().join("devices.json.sessions.jsonl"))
        .unwrap()
        .lines()
        .count();
    assert_eq!(lines, tokens.len());
}
