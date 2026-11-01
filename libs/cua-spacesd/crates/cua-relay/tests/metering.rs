// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Allowance metering end to end (CUA-1360): a held stream through the relay
//! becomes a live session; past the allowance the relay flags responses with
//! the upgrade link and keeps forwarding; `/v1/usage` and the admin view
//! report it.
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_relay::clock::Clock;
use cua_relay::meter::{parse_allowances, MeterConfig, ALLOWANCE_HEADER};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";
const UPGRADE: &str = "https://buy.example/founding";
/// How long a stream stays open before its session is live. Tests make that
/// time pass by advancing the meter's [`Clock`], never by sleeping, so a
/// slow runner can neither miss it (a held stream not live yet) nor
/// overshoot it (a short call counted as a session).
const LIVE_AFTER: Duration = Duration::from_secs(10);

/// Held requests: the machine's handler holds each one open (an arrival
/// adds a permit to `arrived`; the response waits for one on `release`).
/// A held request that ends without being released (refused, failed) is
/// recorded in `ended`, so a test waiting for arrivals fails instead of
/// waiting forever.
#[derive(Clone)]
struct Gate {
    arrived: Arc<Semaphore>,
    release: Arc<Semaphore>,
    ended: Arc<(Mutex<Vec<String>>, Notify)>,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            arrived: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
            ended: Arc::default(),
        }
    }
}

impl Gate {
    /// The machine side: holds the request until released.
    async fn hold(&self) {
        self.arrived.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }

    /// The client side: sends `req` as a held request.
    fn send(&self, req: reqwest::RequestBuilder) -> JoinHandle<reqwest::Response> {
        let ended = self.ended.clone();
        tokio::spawn(async move {
            let r = req.send().await;
            let how = match &r {
                Ok(r) => r.status().to_string(),
                Err(e) => e.to_string(),
            };
            ended.0.lock().unwrap().push(how);
            ended.1.notify_one();
            r.unwrap()
        })
    }

    /// Waits until `n` held requests are inside the machine's handler, so
    /// the relay has opened (and metered) each of their streams.
    async fn arrivals(&self, n: u32) {
        tokio::select! {
            p = self.arrived.acquire_many(n) => p.unwrap().forget(),
            () = self.ended.1.notified() => panic!(
                "a held request ended before reaching the machine: {:?}",
                self.ended.0.lock().unwrap()
            ),
        }
    }
}

#[tokio::test]
async fn past_the_allowance_the_session_continues_with_an_upgrade_prompt() {
    let issuer = FakeIssuer::new(ISSUER);
    let clock = Clock::default();
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        admin_token: Some("admin-secret".into()),
        meter: MeterConfig {
            // One live session allowed: a second one is past it.
            allowances: parse_allowances("free=1/20").unwrap(),
            upgrade_url: UPGRADE.into(),
            live_after: LIVE_AFTER,
            clock: clock.clone(),
            ..MeterConfig::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });

    // The machine's server: `/slow` holds the stream open until released.
    let gate = Gate::default();
    let held_gate = gate.clone();
    let app = axum::Router::new()
        .route(
            "/slow",
            axum::routing::get(move || async move {
                held_gate.hold().await;
                "slow"
            }),
        )
        .fallback(|| async { "ok" });
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(local, app).await;
    });

    let http = reqwest::Client::new();
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 600);
    let stop = CancellationToken::new();
    let mut ids = Vec::new();
    for n in 0..2 {
        let id = format!("meter{n}{}", uuid::Uuid::new_v4().simple());
        let reg: serde_json::Value = http
            .post(format!("{base}/v1/machines"))
            .bearer_auth(&ada)
            .json(&serde_json::json!({"id": id, "name": "metered"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut join = cua_relay::client::JoinConfig::new(
            base.replace("http://", "ws://"),
            reg["machine_token"].as_str().unwrap().to_owned(),
            id.clone(),
            local_addr,
        );
        join.heartbeat = Duration::from_secs(2);
        tokio::spawn(cua_relay::client::run(join, stop.clone()));
        for _ in 0..200 {
            if relay.machine_ids().contains(&id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(relay.machine_ids().contains(&id), "machine joined");
        ids.push(id);
    }
    let usage = |token: String| {
        let (http, base) = (http.clone(), base.clone());
        async move {
            http.get(format!("{base}/v1/usage"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()
        }
    };

    // Short calls are not sessions: no prompt.
    let r = http
        .get(format!("{base}/m/{}/probe", ids[0]))
        .bearer_auth(&ada)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    assert!(r.headers().get(ALLOWANCE_HEADER).is_none());
    let v = usage(ada.clone()).await;
    assert_eq!(v["live_sessions"], 0);
    assert_eq!(v["live_sessions_limit"], 1);
    assert_eq!(v["hours_limit"], 20);
    assert_eq!(v["soft_limit_reached"], false);

    // Two held streams on two machines: two live sessions, one over.
    let held: Vec<_> = ids
        .iter()
        .map(|id| gate.send(http.get(format!("{base}/m/{id}/slow")).bearer_auth(&ada)))
        .collect();
    // Both streams are open at the relay, then stay open past `live_after`.
    gate.arrivals(2).await;
    clock.advance(LIVE_AFTER);
    relay
        .meter()
        .tick(Duration::ZERO, cua_relay::assertion::now_secs());
    let v = usage(ada.clone()).await;
    assert_eq!(v["live_sessions"], 2, "{v}");
    assert_eq!(v["soft_limit_reached"], true);
    assert_eq!(v["reasons"], serde_json::json!(["sessions"]));
    assert_eq!(v["upgrade_url"], UPGRADE);

    // Still forwarded (no enforcement), with the prompt header.
    let r = http
        .get(format!("{base}/m/{}/probe", ids[0]))
        .bearer_auth(&ada)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    assert_eq!(r.text().await.unwrap(), "ok");
    gate.release.add_permits(2);
    for h in held {
        let r = h.await.unwrap();
        assert!(r.status().is_success());
    }
    let r = http
        .get(format!("{base}/m/{}/probe", ids[1]))
        .bearer_auth(&ada)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.headers().get(ALLOWANCE_HEADER).unwrap().to_str().unwrap(),
        format!("exceeded; reasons=sessions; upgrade={UPGRADE}")
    );

    // Bob's own usage is untouched; machine tokens have none.
    assert_eq!(usage(bob).await["live_sessions"], 0);

    // The admin view lists Ada, and needs the admin token.
    let admin = |token: &str| {
        http.get(format!("{base}/relay/v1/usage"))
            .bearer_auth(token)
            .send()
    };
    assert_eq!(admin("wrong").await.unwrap().status(), 401);
    let all: serde_json::Value = admin("admin-secret").await.unwrap().json().await.unwrap();
    let rows = all["users"].as_array().unwrap();
    let ada_row = rows.iter().find(|r| r["user"] == "ada").expect("ada row");
    assert_eq!(ada_row["email"], "ada@example.com");
    assert_eq!(ada_row["soft_limit_reached"], true);
    assert_eq!(all["allowances"]["free"]["live_sessions"], 1);
    stop.cancel();
}

#[tokio::test]
async fn a_ticketed_viewer_websocket_is_metered_as_the_recent_account() {
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    // The machine answers `/media` with a WebSocket it holds open.
    let app = axum::Router::new()
        .route(
            "/media",
            axum::routing::get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                ws.on_upgrade(
                    |mut socket| async move { while let Some(Ok(_)) = socket.recv().await {} },
                )
            }),
        )
        .fallback(|| async { "ok" });
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(local, app).await;
    });
    let http = reqwest::Client::new();
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let id = format!("viewer{}", uuid::Uuid::new_v4().simple());
    let reg: serde_json::Value = http
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&ada)
        .json(&serde_json::json!({"id": id, "name": "viewed"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        reg["machine_token"].as_str().unwrap().to_owned(),
        id.clone(),
        local_addr,
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
    // Ada opens the session (an account call), then the ticketed media
    // WebSocket the machine minted for her.
    let r = http
        .get(format!("{base}/m/{id}/probe"))
        .bearer_auth(&ada)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let url = format!(
        "{}/m/{id}/media?ticket=t0",
        base.replace("http://", "ws://")
    );
    let (_socket, _) = tokio_tungstenite::connect_async(url).await.expect("ws");
    let v: serde_json::Value = http
        .get(format!("{base}/v1/usage"))
        .bearer_auth(&ada)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["live_sessions"], 1, "{v}");
    stop.cancel();
}

#[tokio::test]
async fn a_long_grpc_stream_is_refused_past_an_enforced_cap_and_short_calls_never_are() {
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        meter: MeterConfig {
            allowances: parse_allowances("free=1/20").unwrap(),
            enforce_plans: cua_relay::meter::parse_plan_set("free"),
            upgrade_url: UPGRADE.into(),
            live_after: LIVE_AFTER,
            ..MeterConfig::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    // The machine answers every call at once, except the held terminal
    // stream, which stays open until released.
    let gate = Gate::default();
    let held_gate = gate.clone();
    let app = axum::Router::new()
        .route(
            "/cua.env.v1.ProcessService/StartProcess",
            axum::routing::post(move || async move {
                held_gate.hold().await;
                "ok"
            }),
        )
        .fallback(|| async { "ok" });
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(local, app).await;
    });

    let http = reqwest::Client::new();
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let stop = CancellationToken::new();
    let mut ids = Vec::new();
    for n in 0..2 {
        let id = format!("grpc{n}{}", uuid::Uuid::new_v4().simple());
        let reg: serde_json::Value = http
            .post(format!("{base}/v1/machines"))
            .bearer_auth(&ada)
            .json(&serde_json::json!({"id": id, "name": "grpc"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut join = cua_relay::client::JoinConfig::new(
            base.replace("http://", "ws://"),
            reg["machine_token"].as_str().unwrap().to_owned(),
            id.clone(),
            local_addr,
        );
        join.heartbeat = Duration::from_secs(2);
        tokio::spawn(cua_relay::client::run(join, stop.clone()));
        for _ in 0..200 {
            if relay.machine_ids().contains(&id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(relay.machine_ids().contains(&id), "machine joined");
        ids.push(id);
    }
    let request = |id: &str, method: &str| {
        http.post(format!("{base}/m/{id}/cua.env.v1.{method}"))
            .bearer_auth(&ada)
            .header("content-type", "application/grpc")
            .body(vec![0u8; 5])
    };
    let call = |id: &str, method: &str| {
        let req = request(id, method);
        async move { req.send().await.unwrap() }
    };

    // A terminal stream on machine 0 holds the one allowed session.
    let held = gate.send(request(&ids[0], "ProcessService/StartProcess"));
    gate.arrivals(1).await;

    // A second machine's stream is refused: ResourceExhausted, plain sentence.
    let r = call(&ids[1], "ProcessService/StartProcess").await;
    assert_eq!(r.headers()["grpc-status"], "8");
    assert_eq!(r.headers()["x-cua-relay-error"], "plan_limit_sessions");
    assert_eq!(r.headers()["x-cua-relay-limit"], "1");
    let message = r.headers()["grpc-message"].to_str().unwrap().to_owned();
    assert!(message.contains("live%20sessions") || message.contains("live sessions"));
    // Watches hold sessions too.
    let r = call(&ids[1], "WindowsService/WatchWindows").await;
    assert_eq!(r.headers()["grpc-status"], "8");

    // Short unary calls are never refused, even past the cap.
    let r = call(&ids[1], "ProcessService/ListProcesses").await;
    assert!(r.headers().get("grpc-status").is_none());
    assert!(r.status().is_success());

    // The machine already held takes more streams.
    let r = call(&ids[0], "ProcessService/ConnectProcess").await;
    assert!(r.headers().get("grpc-status").is_none());
    assert!(r.status().is_success());

    // The live stream was never cut.
    gate.release.add_permits(1);
    assert!(held.await.unwrap().status().is_success());
    stop.cancel();
}
