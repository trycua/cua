// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Deploys without downtime (CUA-1774): a stopping relay closes its machine
//! tunnels at once so they move to the next relay, and a relay that just
//! started holds requests for known machines until they are back instead
//! of failing them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

async fn start(
    issuer: &FakeIssuer,
    reconnect_grace: Duration,
) -> (
    Relay,
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        reconnect_grace,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let serving = relay.clone();
    let task = tokio::spawn(async move {
        let _ = serving
            .serve(listener, async move {
                let _ = stop_rx.await;
            })
            .await;
    });
    (relay, base, stop_tx, task)
}

/// A stand-in spacesd: answers every request 200.
async fn local_machine() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(|| async { "machine says hi" });
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

async fn register(base: &str, account: &str, id: &str) -> String {
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(account)
        .json(&json!({"id": id, "name": id}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let v: Value = r.json().await.unwrap();
    v["machine_token"].as_str().unwrap().to_owned()
}

fn join(base: &str, id: &str, token: &str, local: std::net::SocketAddr) -> CancellationToken {
    let mut config = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        token.to_owned(),
        id.to_owned(),
        local,
    );
    config.heartbeat = Duration::from_secs(1);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(config, stop.clone()));
    stop
}

async fn wait_connected(relay: &Relay, id: &str) {
    for _ in 0..200 {
        if relay.machine_ids().iter().any(|m| m == id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{id} did not connect");
}

async fn get_machine(base: &str, id: &str, account: &str) -> reqwest::StatusCode {
    reqwest::Client::new()
        .get(format!("{base}/m/{id}/hello"))
        .bearer_auth(account)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_new_relay_holds_requests_until_the_machine_is_back() {
    let issuer = FakeIssuer::new(ISSUER);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let (relay, base, _stop, _task) = start(&issuer, Duration::from_secs(10)).await;
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    let token = register(&base, &ada, &id).await;
    let local = local_machine().await;

    // The request arrives before the machine has reconnected...
    let request = tokio::spawn({
        let (base, id, ada) = (base.clone(), id.clone(), ada.clone());
        async move { get_machine(&base, &id, &ada).await }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !request.is_finished(),
        "answered before the machine was back"
    );
    // ...and is answered once it has.
    let _machine = join(&base, &id, &token, local);
    let status = tokio::time::timeout(Duration::from_secs(10), request)
        .await
        .expect("held past the machine's return")
        .unwrap();
    assert_eq!(status, reqwest::StatusCode::OK);
    wait_connected(&relay, &id).await;

    // An id the relay does not know is refused at once.
    let started = Instant::now();
    let status = get_machine(&base, "nosuchmachine", &ada).await;
    assert!(
        status.is_client_error() || status.is_server_error(),
        "{status}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn without_a_grace_an_offline_machine_fails_at_once() {
    let issuer = FakeIssuer::new(ISSUER);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let (_relay, base, _stop, _task) = start(&issuer, Duration::ZERO).await;
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    register(&base, &ada, &id).await;
    let started = Instant::now();
    assert_eq!(
        get_machine(&base, &id, &ada).await,
        reqwest::StatusCode::BAD_GATEWAY
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn a_stopping_relay_closes_machine_tunnels_at_once() {
    let issuer = FakeIssuer::new(ISSUER);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let (relay, base, stop, task) = start(&issuer, Duration::ZERO).await;
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    let token = register(&base, &ada, &id).await;
    let machine = join(&base, &id, &token, local_machine().await);
    wait_connected(&relay, &id).await;

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the open tunnel held the shutdown")
        .unwrap();
    assert!(relay.machine_ids().is_empty(), "the tunnel was left open");
    machine.cancel();
}
