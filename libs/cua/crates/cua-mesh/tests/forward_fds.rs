// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A loopback forward to a peer that is in the map but does not answer (an
//! offline machine): the daemon probes it every few seconds, and each probe
//! is a local connection the forward accepted. None of them may outlive the
//! probe. Before the fix every probe's connection waited, in a queue, for
//! the dial before it (each up to 20 s), and dropping the forward did not
//! end them: a daemon running for a day held thousands of CLOSED loopback
//! sockets and hit "Too many open files".
//!
//! One test per binary: it counts this process's descriptors.

#![cfg(unix)]

use std::time::Duration;

use cua_mesh::testing::FakeCoordinator;
use cua_mesh::{Mesh, MeshConfig, Mode, NodeKind, PublicKey, static_auth};
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpStream;

/// Probe cycles: enough that a leak of one descriptor each stands out.
const CYCLES: usize = 40;
/// Descriptors the runtime may open or close on its own meanwhile.
const SLACK: usize = 6;

fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0)
}

async fn node(coord: &FakeCoordinator, kind: NodeKind, dir: &std::path::Path, name: &str) -> Mesh {
    let mut headers = Vec::new();
    if kind == NodeKind::Machine {
        headers.push(("x-cua-machine-id".to_owned(), format!("machine-{name}")));
    }
    let mut cfg = MeshConfig::new(
        coord.url.clone(),
        kind,
        Some(dir.join(format!("{name}.json"))),
        static_auth(headers),
    );
    cfg.name = name.to_owned();
    cfg.stun = vec![coord.stun.to_string()];
    // Only the relay carries packets, so turning it off silences the peer.
    cfg.relay_only = true;
    cfg.connect_timeout = Duration::from_secs(1);
    let m = Mesh::start(cfg).await.expect("start");
    m.wait_ready(Duration::from_secs(10)).await.expect("ready");
    m
}

async fn wait_peer(m: &Mesh, key: &PublicKey) {
    for _ in 0..200 {
        if m.peers().iter().any(|p| &p.node_key == key) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("peer never appeared");
}

/// One probe: connect, send a request, give up after `wait`.
async fn probe(addr: std::net::SocketAddr, wait: Duration) {
    let mut c = TcpStream::connect(addr).await.expect("connect");
    let _ = c
        .write_all(b"GET /v1/info HTTP/1.1\r\nhost: probe\r\n\r\n")
        .await;
    tokio::time::sleep(wait).await;
}

/// Polls until the count is back within `SLACK` of `base`, or `within`.
async fn settle(base: usize, within: Duration) -> usize {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let n = open_fds();
        if n <= base + SLACK || tokio::time::Instant::now() >= deadline {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probes_through_a_forward_to_a_silent_peer_keep_descriptors_flat() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m").await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c").await;
    let key = machine.node_key();
    wait_peer(&client, &key).await;
    // The machine stays in the map but nothing reaches it any more.
    coord.set_relay(false);

    // Warm up (lazy runtime descriptors), then measure.
    {
        let fwd = client.forward(key, "spacesd", None).await.unwrap();
        probe(fwd.local, Duration::from_millis(50)).await;
    }
    let base = settle(open_fds(), Duration::from_secs(3)).await;

    // 1. The daemon's loop: open a forward, probe it, the probe gives up,
    //    forget the forward. Forgetting it ends the connection it accepted.
    for _ in 0..CYCLES {
        let fwd = client.forward(key, "spacesd", None).await.unwrap();
        probe(fwd.local, Duration::from_millis(50)).await;
        drop(fwd);
    }
    let after_forget = settle(base, Duration::from_secs(2)).await;
    assert!(
        after_forget <= base + SLACK,
        "forgotten forwards kept their connections: {base} descriptors before {CYCLES} probes, {after_forget} after"
    );

    // 2. Probes against one forward that stays: callers do not queue behind
    //    one another's dials, so each ends within about one dial timeout.
    let fwd = client.forward(key, "spacesd", None).await.unwrap();
    let base = open_fds();
    let mut probes = Vec::new();
    for _ in 0..CYCLES {
        probes.push(tokio::spawn(probe(fwd.local, Duration::from_millis(20))));
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for p in probes {
        p.await.unwrap();
    }
    // Serialized dials would hold these for CYCLES x 1 s.
    let after_burst = settle(base, Duration::from_secs(4)).await;
    assert!(
        after_burst <= base + SLACK,
        "probes queued behind a silent peer: {base} descriptors before {CYCLES} probes, {after_burst} after"
    );
    drop(fwd);
    machine.shutdown();
    client.shutdown();
}
