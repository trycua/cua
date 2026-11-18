// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The other ways a loopback forward can hold a descriptor after its user
//! is gone (CUA-2159 follow-up; `forward_fds.rs` covers the probe loop):
//! a client that hangs up while its stream is still being dialled (the
//! connection must not wait out the dial), a burst of clients past the
//! forward's in-flight bound, and the UDP socket and flows of a forward
//! that carried datagrams (they used to outlive the forward by 180 s).
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
    cfg.connect_timeout = Duration::from_secs(30);
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
async fn clients_that_hang_up_and_udp_flows_do_not_outlive_the_forward() {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let machine = node(&coord, NodeKind::Machine, dir.path(), "m").await;
    let client = node(&coord, NodeKind::Client, dir.path(), "c").await;
    let key = machine.node_key();
    wait_peer(&client, &key).await;
    coord.set_relay(false);

    {
        let fwd = client.forward(key, "spacesd", None).await.unwrap();
        probe(fwd.local, Duration::from_millis(50)).await;
        udp_burst(fwd.local, 2).await;
    }
    let base = settle(open_fds(), Duration::from_secs(3)).await;

    // 1. Clients hang up while the (30 s) dial to the silent peer is still
    //    running: their sockets close within moments, not after the dial.
    let fwd = client.forward(key, "spacesd", None).await.unwrap();
    for _ in 0..CYCLES {
        probe(fwd.local, Duration::from_millis(10)).await;
    }
    let after = settle(base + 2, Duration::from_secs(3)).await;
    assert!(
        after <= base + 2 + SLACK,
        "hung-up clients kept their sockets while the dial ran: {base} before {CYCLES} probes, {after} after"
    );

    // 2. More simultaneous clients than the forward serves: the rest are
    //    refused at once, so the count stays bounded.
    let mut held = Vec::new();
    for _ in 0..(CYCLES * 20) {
        if let Ok(c) = TcpStream::connect(fwd.local).await {
            held.push(c);
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let ours = held.len();
    // The forward's side of each accepted connection is in this process too.
    let peak = open_fds();
    assert!(
        peak < base + ours + 300,
        "a burst of {ours} clients opened {} descriptors on the forward's side",
        peak - base - ours
    );
    drop(held);
    drop(fwd);

    // 3. Datagrams through a forward: its UDP socket and flows end with it.
    for _ in 0..CYCLES / 2 {
        let fwd = client.forward(key, "spacesd", None).await.unwrap();
        udp_burst(fwd.local, 3).await;
        drop(fwd);
    }
    let after_udp = settle(base, Duration::from_secs(3)).await;
    assert!(
        after_udp <= base + SLACK,
        "forwards that carried datagrams kept descriptors: {base} before, {after_udp} after"
    );
    machine.shutdown();
    client.shutdown();
}

/// Sends datagrams to the forward's UDP port from `flows` source ports.
async fn udp_burst(addr: std::net::SocketAddr, flows: usize) {
    for _ in 0..flows {
        let s = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let _ = s.send_to(b"hello", addr).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
