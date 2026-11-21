// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A mesh node that reconnects without changing anything costs no registry
//! write. relay.cua.ai rewrote its whole registry (11k nodes) on every
//! reconnect, disconnect and endpoint report, about three times a second
//! under the reconnect churn of 20 Nov 2026, and each write serialized
//! megabytes from a different thread. Only `last_seen` changes then; it is
//! written at most every `LAST_SEEN_SAVE_EVERY` and at shutdown.

use std::time::Duration;

use cua_mesh::{Mesh, MeshConfig, NodeKind};
use cua_relay::server::{Relay, RelayConfig};

async fn mesh_writes(url: &str) -> u64 {
    let text = reqwest::get(format!("{url}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    text.lines()
        .find_map(|l| {
            l.strip_prefix("cua_relay_state_writes_total{store=\"mesh\"} ")?
                .trim()
                .parse()
                .ok()
        })
        .expect("mesh writes on /metrics")
}

async fn connected(url: &str, n: u64) {
    for _ in 0..200 {
        let s: serde_json::Value = reqwest::Client::new()
            .get(format!("{url}/relay/v1/mesh"))
            .bearer_auth("admin")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if s["connected"] == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("mesh never had {n} nodes connected");
}

fn last_seen(registry: &std::path::Path) -> u64 {
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(registry).unwrap()).unwrap();
    raw["nodes"][0]["last_seen"].as_u64().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnects_that_change_nothing_do_not_rewrite_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let relay = Relay::new(RelayConfig {
        tokens: vec!["reg".into()],
        admin_token: Some("admin".into()),
        state_file: Some(dir.path().join("state.json")),
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = relay.clone();
    let server = tokio::spawn(async move {
        let _ = serving
            .serve(listener, async {
                let _ = stopped.await;
            })
            .await;
    });

    // A static machine: its node is registered (and written) once.
    let key = dir.path().join("m.json");
    let start = || {
        Mesh::start(MeshConfig::new(
            url.clone(),
            NodeKind::Machine,
            Some(key.clone()),
            cua_mesh::static_auth(vec![
                ("authorization".into(), "Bearer reg".into()),
                ("x-cua-machine-id".into(), "machine-0001".into()),
            ]),
        ))
    };
    let node = start().await.expect("mesh node starts");
    node.wait_ready(Duration::from_secs(10)).await.unwrap();
    node.shutdown();
    connected(&url, 0).await;
    // Past the 200 ms coalescing of a write that is due.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let registered = mesh_writes(&url).await;
    assert!(registered >= 1, "a new node is written");
    let registry = dir.path().join("state.mesh.json");
    let first_seen = last_seen(&registry);

    // The same node comes and goes (a flapping client, a restart loop):
    // nothing on disk changes but `last_seen`.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    for _ in 0..6 {
        let node = start().await.expect("mesh node starts");
        node.wait_ready(Duration::from_secs(10)).await.unwrap();
        node.shutdown();
        connected(&url, 0).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    let after = mesh_writes(&url).await;
    assert_eq!(
        after - registered,
        0,
        "six reconnects of a known node rewrote the registry {} times",
        after - registered
    );

    // A clean shutdown still keeps the newest `last_seen`.
    let _ = stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), server).await;
    assert!(
        last_seen(&registry) > first_seen,
        "last_seen written at shutdown"
    );
}
