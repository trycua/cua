// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The mesh coordinator on its own: a static machine and a client node
//! (real `cua-mesh` nodes) on loopback, pairing by proof, a stream between
//! them, the registry surviving a coordinator restart, `/v1/info`, and the
//! admin counters.

use std::sync::Arc;
use std::time::Duration;

use cua_mesh::{Mesh, MeshConfig, NodeKind};
use cua_relay::server::{Relay, RelayConfig};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

async fn serve(relay: &Relay, port: u16) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = relay.clone();
    let task = tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    (port, task)
}

fn config(dir: &std::path::Path) -> RelayConfig {
    RelayConfig {
        tokens: vec!["reg".into()],
        admin_token: Some("admin".into()),
        state_file: Some(dir.join("state.json")),
        ..RelayConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn static_pairing_streams_and_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let relay = Relay::new(config(dir.path()));
    let (port, task) = serve(&relay, 0).await;
    let url = format!("http://127.0.0.1:{port}");
    let info: serde_json::Value = reqwest::get(format!("{url}/v1/info"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["mesh"]["version"], cua_mesh::VERSION);

    // A static machine (registration token) that accepts the env token
    // "env-1", serving an echo service.
    let mut m = MeshConfig::new(
        url.clone(),
        NodeKind::Machine,
        Some(dir.path().join("m.json")),
        cua_mesh::static_auth(vec![
            ("authorization".into(), "Bearer reg".into()),
            ("x-cua-machine-id".into(), "machine-0001".into()),
        ]),
    );
    m.approved_path = Some(dir.path().join("approved.json"));
    m.pair_verifier = Some(Arc::new(|client, ts, proof| {
        cua_mesh::proto::verify_pair_proof(
            "env-1",
            "machine-0001",
            client,
            ts,
            proof,
            cua_mesh::proto::now_secs(),
        )
    }));
    let machine = Mesh::start(m).await.unwrap();
    machine.wait_ready(Duration::from_secs(10)).await.unwrap();
    let echo = machine.clone();
    tokio::spawn(async move {
        while let Some(i) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(i.stream);
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });

    let client = Mesh::start(MeshConfig::new(
        url.clone(),
        NodeKind::Client,
        Some(dir.path().join("c.json")),
        cua_mesh::static_auth(Vec::new()),
    ))
    .await
    .unwrap();
    client.wait_ready(Duration::from_secs(10)).await.unwrap();
    assert!(client.pair("machine-0001", "wrong").await.is_err());
    client.pair("machine-0001", "env-1").await.unwrap();
    let peer = client
        .wait_machine("machine-0001", Duration::from_secs(10))
        .await
        .expect("paired");
    let mut s = client
        .open_stream(&peer.node_key, "echo", None)
        .await
        .unwrap();
    s.write_all(b"through the coordinator").await.unwrap();
    let mut buf = [0u8; 23];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"through the coordinator");

    // Admin counters.
    let http = reqwest::Client::new();
    let unauth = http
        .get(format!("{url}/relay/v1/mesh"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);
    let status: cua_relay::mesh::MeshStatus = http
        .get(format!("{url}/relay/v1/mesh"))
        .bearer_auth("admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (
            status.nodes,
            status.machines,
            status.connected,
            status.pairings
        ),
        (2, 1, 2, 1)
    );

    // The coordinator restarts on the same port with the same state file:
    // the pairing survives, and the stream keeps working meanwhile.
    relay.mesh().disconnect_all();
    task.abort();
    drop(relay);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let relay = Relay::new(config(dir.path()));
    let (_, _task) = serve(&relay, port).await;
    client.wait_ready(Duration::from_secs(15)).await.unwrap();
    machine.wait_ready(Duration::from_secs(15)).await.unwrap();
    assert!(
        client.machine("machine-0001").is_some(),
        "pairing persisted"
    );
    s.write_all(b"after the restart------").await.unwrap();
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"after the restart------");
}
