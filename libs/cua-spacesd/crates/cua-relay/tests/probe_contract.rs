// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The probe contract (`x-cua-relay-route`, src/route_marker.rs): what
//! external monitoring matches instead of refusal wording (INC-27).
//!
//! Checked in every mode the hosted and self-hosted relays run in:
//! {phone support off, on} x {admin token unset, set} x {account sign-in
//! off, on}. A failure here means a probe in trycua/cloud
//! (blackbox `spaces_relay_*` modules, the status-page checker) would
//! page: change them in the same release, never after.

use std::sync::Arc;
use std::time::Duration;

use cua_push::{MockProvider, PushHub, Registry};
use cua_relay::client::JoinConfig;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::push_api::PushConfig;
use cua_relay::route_marker::{RouteMarker, HEADER};
use cua_relay::server::{Relay, RelayConfig};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";
const TOKEN: &str = "probe-contract-machine-token";
const MACHINE: &str = "probe-contract-machine";
/// The id the blackbox probe asks for: nobody registers it.
const NIL: &str = "00000000-0000-0000-0000-000000000000";

#[derive(Clone, Copy, Debug)]
struct Mode {
    push: bool,
    admin: bool,
    oidc: bool,
}

fn modes() -> Vec<Mode> {
    let mut all = Vec::new();
    for push in [false, true] {
        for admin in [false, true] {
            for oidc in [false, true] {
                all.push(Mode { push, admin, oidc });
            }
        }
    }
    all
}

struct Env {
    base: String,
    http: reqwest::Client,
    stop: CancellationToken,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.stop.cancel();
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// A relay in `mode` with one static-token computer joined. The computer's
/// `/health` answers 200 and tries to claim the relay's marker.
async fn start(mode: Mode) -> Env {
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        tokens: vec![TOKEN.into()],
        admin_token: mode.admin.then(|| "probe-contract-admin".into()),
        oidc: mode.oidc.then(|| {
            Arc::new(OidcValidator::with_jwks(
                OidcConfig::new(ISSUER),
                issuer.jwks(),
            ))
        }),
        push: mode.push.then(|| {
            PushConfig::new(Arc::new(PushHub::new(
                Registry::in_memory(),
                Arc::new(MockProvider::new()),
            )))
        }),
        ..RelayConfig::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    let mut tasks = vec![tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    })];

    let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    tasks.push(tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/health",
            axum::routing::get(|| async { ([(HEADER, "refused-auth")], "fine") }),
        );
        let _ = axum::serve(local, app).await;
    }));
    let config = JoinConfig::new(
        base.replace("http://", "ws://"),
        TOKEN.into(),
        MACHINE.into(),
        local_addr,
    );
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    tasks.push(tokio::spawn(async move {
        let _ = cua_relay::client::session(&config, &stopped).await;
    }));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !relay.machine_ids().contains(&MACHINE.to_string()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("computer never came online");
    Env {
        base,
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        stop,
        tasks,
    }
}

fn marker(r: &reqwest::Response) -> Option<String> {
    r.headers()
        .get(HEADER)
        .map(|v| v.to_str().unwrap().to_owned())
}

#[tokio::test]
async fn health_carries_ok_in_every_mode() {
    for mode in modes() {
        let env = start(mode).await;
        let r = env
            .http
            .get(format!("{}/healthz", env.base))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 204, "{mode:?}");
        assert_eq!(marker(&r).as_deref(), Some("ok"), "{mode:?}");

        let r = env
            .http
            .get(format!("{}/readyz", env.base))
            .send()
            .await
            .unwrap();
        let expected = if r.status() == 200 { "ok" } else { "not-ready" };
        assert_eq!(
            marker(&r).as_deref(),
            Some(expected),
            "{mode:?} {}",
            r.status()
        );
    }
}

/// The blackbox probe: an anonymous GET of an id nobody registered. The
/// refusal's wording differs by mode (that is what paged in INC-27); its
/// status and marker do not.
#[tokio::test]
async fn an_anonymous_unknown_id_is_refused_auth_in_every_mode() {
    let mut bodies = std::collections::BTreeSet::new();
    for mode in modes() {
        let env = start(mode).await;
        // With phone support off the machine's own `/health` is forwarded
        // without a credential (so an unknown id is `offline`, not refused).
        if !mode.push {
            let r = env
                .http
                .get(format!("{}/m/{NIL}/health", env.base))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 502, "{mode:?}");
            assert_eq!(marker(&r).as_deref(), Some("offline"), "{mode:?}");
        }
        let paths = if mode.push {
            &["healthz", "health", "cua.env.v1.SystemService/Health"][..]
        } else {
            &["healthz", "cua.env.v1.SystemService/Health"][..]
        };
        for path in paths {
            let r = env
                .http
                .get(format!("{}/m/{NIL}/{path}", env.base))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 401, "{mode:?} {path}");
            assert_eq!(
                marker(&r).as_deref(),
                Some("refused-auth"),
                "{mode:?} {path}"
            );
            bodies.insert(r.text().await.unwrap());
        }
        // A gRPC caller gets HTTP 200 with grpc-status 16; same marker.
        let r = env
            .http
            .post(format!(
                "{}/m/{NIL}/cua.env.v1.SystemService/Health",
                env.base
            ))
            .header("content-type", "application/grpc-web+proto")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{mode:?}");
        assert_eq!(
            r.headers().get("grpc-status").map(|v| v.to_str().unwrap()),
            Some("16"),
            "{mode:?}"
        );
        assert_eq!(marker(&r).as_deref(), Some("refused-auth"), "{mode:?}");
    }
    // The point of the contract: the text is free to vary.
    assert!(bodies.len() > 1, "{bodies:?}");
}

#[tokio::test]
async fn forwarded_and_offline_in_every_mode() {
    for mode in modes() {
        let env = start(mode).await;
        // The computer answered; its attempt to send the relay's marker is replaced.
        let r = env
            .http
            .get(format!("{}/m/{MACHINE}/health", env.base))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{mode:?}");
        let values: Vec<_> = r.headers().get_all(HEADER).iter().collect();
        assert_eq!(values.len(), 1, "{mode:?} {values:?}");
        assert_eq!(marker(&r).as_deref(), Some("forwarded"), "{mode:?}");

        if !mode.push || !mode.oidc {
            // With phone support and sign-in on, an unknown id is answered as
            // an account computer (refused-auth above), never as offline.
            let r = env
                .http
                .get(format!("{}/m/not-connected-here/health", env.base))
                .bearer_auth(TOKEN)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 502, "{mode:?}");
            assert_eq!(marker(&r).as_deref(), Some("offline"), "{mode:?}");
        }
    }
}

#[test]
fn every_value_is_a_valid_header_value() {
    for m in RouteMarker::ALL {
        let v = m.as_str();
        assert!(
            !v.is_empty() && v.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
            "{v}"
        );
    }
}

/// The probe port (`CUA_RELAY_HEALTH_LISTEN`) and STUN-only pods answer
/// `/healthz` from their own listener: same status, same marker.
#[tokio::test]
async fn the_probe_port_carries_ok() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(cua_relay::handover::serve_health(listener));
    let r = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{base}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(marker(&r).as_deref(), Some("ok"));
    task.abort();
}
