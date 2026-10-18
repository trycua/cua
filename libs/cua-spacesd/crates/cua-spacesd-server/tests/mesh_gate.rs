// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The machine side of the mesh is off unless allowed (`CUA_MESH`, or the
//! app's channel with the Beta switch on): while it is off the machine
//! never asks its relay about the mesh, and it starts asking once the
//! policy allows it, without a restart.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cua_mesh::Policy;
use cua_spacesd_server::mesh_host::{run, MeshHostConfig};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

/// A relay without a mesh that counts `/v1/info` requests.
async fn counting_relay() -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut tcp, _)) = listener.accept().await else {
                continue;
            };
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = tcp.read(&mut buf).await.unwrap_or(0);
                if String::from_utf8_lossy(&buf[..n]).starts_with("GET /v1/info") {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
                let body = r#"{"account_auth":true,"static_tokens":false}"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tcp.write_all(reply.as_bytes()).await;
            });
        }
    });
    (url, count)
}

fn config(relay: &str, dir: &std::path::Path, on: Arc<AtomicBool>) -> MeshHostConfig {
    MeshHostConfig {
        relay_url: relay.to_owned(),
        relay_token: "machine-token".into(),
        relay_token_file: None,
        machine_id: "mesh-gate-test-0001".into(),
        dir: dir.join("mesh"),
        local: "127.0.0.1:9".parse().unwrap(),
        quic: None,
        account: Arc::default(),
        env_token: Arc::new(|| None),
        name: "test".into(),
        policy: Arc::new(move || {
            if on.load(Ordering::SeqCst) {
                Policy::Prefer
            } else {
                Policy::Off
            }
        }),
        recheck: Duration::from_millis(50),
    }
}

async fn wait_for(count: &AtomicUsize, at_least: usize) -> bool {
    for _ in 0..100 {
        if count.load(Ordering::SeqCst) >= at_least {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn an_off_machine_never_asks_its_relay_about_the_mesh() {
    let (relay, count) = counting_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let on = Arc::new(AtomicBool::new(false));
    let stop = CancellationToken::new();
    let task = tokio::spawn(run(config(&relay, dir.path(), on.clone()), stop.clone()));

    // Off: 20 rechecks later, still nothing asked and no node state written.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "an off machine asked the relay"
    );
    assert!(
        !dir.path().join("mesh").exists(),
        "an off machine made a node"
    );

    // Allowed (CUA_MESH=prefer, or the Beta switch): the next recheck asks,
    // with no restart. This relay has no mesh, so it asks once and waits.
    on.store(true, Ordering::SeqCst);
    assert!(wait_for(&count, 1).await, "an allowed machine never asked");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "a relay without a mesh is asked again later, not hammered"
    );

    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("stops on shutdown")
        .unwrap();
}

#[tokio::test]
async fn the_default_policy_is_off() {
    // No CUA_MESH and no app channel: the machine stays on the relay path.
    let dir = tempfile::tempdir().unwrap();
    if std::env::var(cua_mesh::policy::ENV).is_err() {
        assert_eq!(Policy::current(Some(dir.path())), Policy::Off);
        std::fs::write(dir.path().join(cua_mesh::policy::CHANNEL_FILE), "beta").unwrap();
        // The Beta switch is off in this release.
        assert_eq!(Policy::current(Some(dir.path())), Policy::Off);
    }
}
