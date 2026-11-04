// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Connect/disconnect churn leaves nothing behind (the 3 Nov 2026 OOM kill
//! of relay.cua.ai): machine tunnels that come, go and replace each other,
//! and mesh nodes that reconnect and fight over one node key. After the
//! churn every per-connection table is empty again, and the relay's live
//! heap is back near where it started.
//!
//! Each round also opens proxied client streams (some aborted, some left
//! for the machine's departure to end), so a leak in the stream counters
//! (`global_streams`, the per-machine count, their guards) shows too: the
//! open-stream gauge must be back at zero after every round.
//!
//! The heap is measured with jemalloc's `stats.allocated` (bytes in live
//! allocations), which this test binary uses as its allocator on Linux, so
//! allocator fragmentation does not blur a real leak. On other platforms
//! only the tables are checked.

use std::time::Duration;

use cua_mesh::{Mesh, MeshConfig, NodeKind};
use cua_relay::server::{Relay, RelayConfig};
use tokio_util::sync::CancellationToken;

#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const TOKEN: &str = "reg";

#[cfg(target_os = "linux")]
fn live_heap() -> Option<u64> {
    use tikv_jemalloc_ctl::{epoch, stats};
    epoch::advance().ok()?;
    Some(stats::allocated::read().ok()? as u64)
}
#[cfg(not(target_os = "linux"))]
fn live_heap() -> Option<u64> {
    None
}

async fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..400 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn open_streams(url: &str) -> u64 {
    let text = reqwest::get(format!("{url}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    text.lines()
        .find_map(|l| l.strip_prefix("cua_relay_client_streams ")?.trim().parse().ok())
        .expect("cua_relay_client_streams in /metrics")
}

/// One round: `n` machines join (a third of them twice, so the second
/// session replaces the first), `n` mesh clients connect (pairs on one key,
/// so each replaces the other), then everything leaves.
async fn round(
    relay: &Relay,
    url: &str,
    local: std::net::SocketAddr,
    dir: &std::path::Path,
    n: usize,
    r: usize,
) {
    let mut stops = Vec::new();
    for i in 0..n {
        let id = format!("churn{i:04}{}", "0".repeat(20));
        let mut join = cua_relay::client::JoinConfig::new(
            url.replace("http://", "ws://"),
            TOKEN.into(),
            id,
            local,
        );
        join.heartbeat = Duration::from_secs(1);
        let s = CancellationToken::new();
        tokio::spawn(cua_relay::client::run(join.clone(), s.clone()));
        stops.push(s);
        if i % 3 == 0 {
            let s = CancellationToken::new();
            tokio::spawn(cua_relay::client::run(join, s.clone()));
            stops.push(s);
        }
    }
    let mut nodes = Vec::new();
    for i in 0..n {
        let key = dir.join(format!("r{r}-node{}.json", i / 2));
        let mut cfg = MeshConfig::new(
            url.to_owned(),
            NodeKind::Client,
            Some(key),
            cua_mesh::static_auth(Vec::new()),
        );
        cfg.relay_only = true;
        // A node that does not start would hollow the round out: fail.
        nodes.push(Mesh::start(cfg).await.expect("mesh node starts"));
    }
    wait_until("machines online", || relay.machine_ids().len() == n).await;
    // Proxied streams through the budget's counters: the backend holds each
    // one open, half are dropped by the client mid-flight, the rest end when
    // their machine leaves.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let streams: Vec<_> = (0..n.min(12))
        .map(|i| {
            let client = client.clone();
            let url = format!("{url}/m/churn{i:04}{}/stream", "0".repeat(20));
            tokio::spawn(async move {
                client
                    .get(&url)
                    .header("x-cua-env-authorization", "Bearer t")
                    .send()
                    .await
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        open_streams(url).await > 0,
        "round {r}: proxied streams are open"
    );
    for (i, s) in streams.iter().enumerate() {
        if i % 2 == 0 {
            s.abort();
        }
    }
    for s in stops {
        s.cancel();
    }
    for node in nodes {
        node.shutdown();
    }
    wait_until("machines gone", || relay.machine_ids().is_empty()).await;
    for s in streams {
        s.abort();
    }
    for _ in 0..400 {
        if open_streams(url).await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(open_streams(url).await, 0, "round {r}: stream slots leaked");
    wait_until("mesh nodes gone", || {
        let s = cua_relay::mesh::status(relay);
        s.connected == 0 && s.nodes == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_disconnect_churn_leaves_no_state_and_no_heap_behind() {
    let dir = tempfile::tempdir().unwrap();
    let relay = Relay::new(RelayConfig {
        tokens: vec![TOKEN.into()],
        require_client_credentials: false,
        max_machines_per_token: 1000,
        state_file: Some(dir.path().join("state.json")),
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = backend.local_addr().unwrap();
    // Holds every connection open and silent, so proxied streams stay open.
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((conn, _)) = backend.accept().await {
            held.push(conn);
        }
    });

    // Two warm-up rounds: lazily built tables, thread-local caches and the
    // runtime's pools reach their working size.
    const N: usize = 24;
    for r in 0..2 {
        round(&relay, &url, local, dir.path(), N, r).await;
    }
    let before = live_heap();
    for r in 2..12 {
        round(&relay, &url, local, dir.path(), N, r).await;
    }
    // Let tasks that are finishing (writers, watchdogs) wind down.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = live_heap();
    if let (Some(before), Some(after)) = (before, after) {
        let grew = after.saturating_sub(before);
        eprintln!("CHURN live heap before {before} after {after} (+{grew} bytes over 10 rounds)");
        // Ten rounds of 32 machine sessions (8 replaced), 24 mesh
        // connections (12 replaced) and 12 proxied streams: about 680
        // connections, so a leak of 2 KiB per connection crosses this.
        // (The silent backend's held sockets add a few hundred bytes each in
        // the test itself, well inside the margin.)
        assert!(grew < 1 << 20, "live heap grew {grew} bytes over the churn");
    }
}
