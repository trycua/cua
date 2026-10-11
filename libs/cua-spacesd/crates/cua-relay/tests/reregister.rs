// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Re-registering an existing machine (host setup run again): the owner's
//! account token alone re-registers a machine whose host is gone, while a
//! live host needs its machine token or an enrolled device, and another
//! account never can. Each decision logs its path, and no secret.

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cua_relay::devices::DevicePolicy;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn logs() -> &'static Buffer {
    static LOGS: OnceLock<Buffer> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .init();
        buffer
    })
}

/// The re-register lines logged since the last call.
fn reregister_lines() -> Vec<String> {
    let mut buf = logs().0.lock().unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    buf.clear();
    text.lines()
        .filter(|l| l.contains("cua_relay::register"))
        .map(str::to_owned)
        .collect()
}

async fn start(issuer: &FakeIssuer) -> (Relay, String) {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        // Enrollment enforced, grace over: an account token alone has no
        // enrolled device behind it.
        device_policy: DevicePolicy {
            grace_secs: 0,
            ..DevicePolicy::default()
        },
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

async fn register(
    base: &str,
    account: &str,
    machine_token: Option<&str>,
    id: &str,
) -> reqwest::Response {
    let mut r = reqwest::Client::new()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(account)
        .json(&json!({"id": id, "name": id}));
    if let Some(t) = machine_token {
        r = r.header("x-cua-machine-authorization", format!("Bearer {t}"));
    }
    r.send().await.unwrap()
}

async fn token_of(r: reqwest::Response) -> String {
    assert!(r.status().is_success(), "{}", r.status());
    let v: Value = r.json().await.unwrap();
    v["machine_token"].as_str().unwrap().to_owned()
}

/// Connects `id` as a host with `token`; returns once the relay sees it.
async fn connect(relay: &Relay, base: &str, id: &str, token: &str) -> CancellationToken {
    let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = unused.local_addr().unwrap();
    drop(unused);
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        token.to_owned(),
        id.to_owned(),
        local,
    );
    join.heartbeat = Duration::from_secs(1);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&id.to_owned()) {
            return stop;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{id} did not connect");
}

#[tokio::test]
async fn reregistering_needs_proof_only_while_the_host_is_live() {
    logs();
    let issuer = FakeIssuer::new(ISSUER);
    let (relay, base) = start(&issuer).await;
    let ada = issuer.token("ada-sub-4c1f", Some("ada@example.com"), "cua-relay", 300);
    let eve = issuer.token("eve-sub-9d2e", Some("eve@example.com"), "cua-relay", 300);
    let id = format!("m{}", uuid::Uuid::new_v4().simple());
    let first = token_of(register(&base, &ada, None, &id).await).await;
    reregister_lines();

    // Stale record, same owner, host offline (setup died / reinstall lost
    // the token): the account token alone re-registers it and rotates the
    // token; the old one stops working.
    let second = token_of(register(&base, &ada, None, &id).await).await;
    assert_ne!(second, first);
    let lines = reregister_lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("path=\"stale_record\""), "{}", lines[0]);
    assert!(lines[0].contains("connected=false"), "{}", lines[0]);
    let r = register(&base, &ada, Some(&first), &id).await;
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "an unknown machine token is no proof, but the host is still offline"
    );
    let second = token_of(r).await;
    let lines = reregister_lines();
    assert!(lines[0].contains("path=\"stale_record\""), "{}", lines[0]);

    // Another account: refused, live or not.
    let r = register(&base, &eve, None, &id).await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert!(
        reregister_lines().is_empty(),
        "not the owner: no re-register path"
    );

    // The host connects and heartbeats: the account token alone is refused
    // (no enrolled device), so a stray session cannot knock it offline.
    let stop = connect(&relay, &base, &id, &second).await;
    let r = register(&base, &ada, None, &id).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let body: Value = r.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("needs its machine token or an enrolled device"),
        "{body}"
    );
    let lines = reregister_lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("path=\"refused_live_machine\""),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("connected=true"), "{}", lines[0]);
    assert!(relay.machine_ids().contains(&id), "still connected");
    // Another account cannot either.
    assert_eq!(
        register(&base, &eve, None, &id).await.status(),
        StatusCode::CONFLICT
    );

    // The host itself, with its machine token: allowed while live.
    let third = token_of(register(&base, &ada, Some(&second), &id).await).await;
    assert_ne!(third, second);
    let lines = reregister_lines();
    assert!(lines[0].contains("path=\"machine_token\""), "{}", lines[0]);
    stop.cancel();

    // No secret, id or identity in any re-register line.
    let all = lines.join("\n");
    for secret in [&ada, &eve, &first, &second, &third, &id] {
        assert!(!all.contains(secret.as_str()), "leaked {secret}");
    }
    for s in ["ada@example.com", "ada-sub-4c1f", "eve-sub-9d2e"] {
        assert!(!all.contains(s), "leaked {s}");
    }
}
