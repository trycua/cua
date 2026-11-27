// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Connection draining: on the stop signal the relay refuses new tunnels,
//! keeps the open ones up while client streams finish (up to a bound),
//! then closes them cleanly right before it exits.

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
    drain: Duration,
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
        drain,
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

/// A stand-in spacesd: answers every request 200 after `delay`.
async fn local_machine(delay: Duration) -> std::net::SocketAddr {
    local_machine_signalling(delay).await.0
}

/// [`local_machine`] that also signals when a request reaches it, so a test
/// can stop the relay once the stream is really in flight instead of after a
/// guessed delay (on a loaded runner the request may not have crossed the
/// tunnel yet, and the relay then has nothing to drain).
async fn local_machine_signalling(
    delay: Duration,
) -> (std::net::SocketAddr, Arc<tokio::sync::Notify>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let reached = Arc::new(tokio::sync::Notify::new());
    let signal = reached.clone();
    let app = axum::Router::new().fallback(move || {
        let signal = signal.clone();
        async move {
            signal.notify_one();
            tokio::time::sleep(delay).await;
            "machine says hi"
        }
    });
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, reached)
}

/// Waits until a request has reached the stand-in machine.
async fn wait_reached(reached: &tokio::sync::Notify) {
    tokio::time::timeout(Duration::from_secs(10), reached.notified())
        .await
        .expect("the request never reached the machine");
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

/// The stand-in is in `head` as a fixed answer; this one can be slow.
#[tokio::test]
async fn drain_lets_a_running_request_finish_and_refuses_new_tunnels() {
    let issuer = FakeIssuer::new(ISSUER);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let (relay, base, stop, task) = start(&issuer, Duration::from_secs(10)).await;
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    let token = register(&base, &ada, &id).await;
    let (local, reached) = local_machine_signalling(Duration::from_millis(1500)).await;
    let _machine = join(&base, &id, &token, local);
    wait_connected(&relay, &id).await;

    let request = tokio::spawn({
        let (base, id, ada) = (base.clone(), id.clone(), ada.clone());
        async move { get_machine(&base, &id, &ada).await }
    });
    wait_reached(&reached).await;
    let _ = stop.send(());
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Draining: the tunnel is still up for the running request, and a
    // second computer cannot join.
    assert!(relay.is_draining());
    assert_eq!(relay.machine_ids(), vec![id.clone()], "tunnel closed early");
    let other = format!("m{}", uuid::Uuid::new_v4().simple());
    let other_token = register(&base, &ada, &other).await;
    let _late = join(
        &base,
        &other,
        &other_token,
        local_machine(Duration::ZERO).await,
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !relay.machine_ids().contains(&other),
        "a tunnel was accepted while draining"
    );

    // The running request finishes; then the tunnels close and the relay ends.
    let status = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .expect("request cut by the drain")
        .unwrap();
    assert_eq!(status, reqwest::StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the relay did not end after the streams finished")
        .unwrap();
    assert!(relay.machine_ids().is_empty());
}

#[tokio::test]
async fn drain_is_bounded() {
    let issuer = FakeIssuer::new(ISSUER);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let (relay, base, stop, task) = start(&issuer, Duration::from_secs(1)).await;
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    let token = register(&base, &ada, &id).await;
    let (local, reached) = local_machine_signalling(Duration::from_secs(30)).await;
    let _machine = join(&base, &id, &token, local);
    wait_connected(&relay, &id).await;
    let _request = tokio::spawn({
        let (base, id, ada) = (base.clone(), id.clone(), ada.clone());
        async move { get_machine(&base, &id, &ada).await }
    });
    wait_reached(&reached).await;
    let started = Instant::now();
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .expect("a stuck stream held the relay past the drain bound")
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert!(relay.machine_ids().is_empty());
}

#[tokio::test]
async fn without_a_drain_the_tunnels_close_at_once() {
    let issuer = FakeIssuer::new(ISSUER);
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let (relay, base, stop, task) = start(&issuer, Duration::ZERO).await;
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    let token = register(&base, &ada, &id).await;
    let machine = join(&base, &id, &token, local_machine(Duration::ZERO).await);
    wait_connected(&relay, &id).await;
    let started = Instant::now();
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the open tunnel held the shutdown")
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(relay.machine_ids().is_empty(), "the tunnel was left open");
    machine.cancel();
}
