// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Many machines, some of them offline: one client reaches all of them at
//! once (as the Spaces list does after an app launch or a daemon restart).
//! An offline machine is in the network map, marked offline, as a computer
//! that has been switched off for a while is. Machines that answer must
//! answer within their budget, whatever the offline ones do, and the
//! offline ones must fail fast instead of waiting out a connect timeout.
//!
//! Sizes: `CUA_MESH_SCALE_ONLINE` (default 15) and
//! `CUA_MESH_SCALE_OFFLINE` (default 5). Each run prints one `SCALE` line
//! per scenario with the slowest online and offline answers.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_mesh::dialer::Dialer;
use cua_mesh::testing::FakeCoordinator;
use cua_mesh::{Mesh, MeshConfig, Mode, NodeKind, static_auth};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Every machine that is online answers within this.
const ONLINE_BUDGET: Duration = Duration::from_secs(2);
/// Every offline machine is reported unreachable within this.
const OFFLINE_BUDGET: Duration = Duration::from_secs(3);
/// No single answer takes longer than this (it would only time out).
const CAP: Duration = Duration::from_secs(60);

fn size(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

async fn machine(coord: &FakeCoordinator, dir: &std::path::Path, name: &str) -> Mesh {
    let headers = vec![("x-cua-machine-id".to_owned(), format!("machine-{name}"))];
    let mut cfg = MeshConfig::new(
        coord.url.clone(),
        NodeKind::Machine,
        Some(dir.join(format!("{name}.json"))),
        static_auth(headers),
    );
    cfg.name = name.to_owned();
    cfg.stun = vec![coord.stun.to_string()];
    // Port 0: many machines in one process.
    cfg.media_udp_port = 0;
    cfg.approved_path = Some(dir.join(format!("{name}-approved.json")));
    let m = Mesh::start(cfg).await.expect("start");
    m.wait_ready(Duration::from_secs(10)).await.expect("ready");
    m
}

/// Echoes every stream, whatever its service.
fn serve_echo(m: &Mesh) {
    let m = m.clone();
    tokio::spawn(async move {
        while let Some(incoming) = m.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(incoming.stream);
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
    });
}

struct Fleet {
    _dir: tempfile::TempDir,
    coord: FakeCoordinator,
    online: Vec<Mesh>,
    offline: Vec<String>,
}

/// `online` machines serving echo, plus `offline` machines that joined
/// once and were switched off (in the map, offline).
async fn fleet(online: usize, offline: usize) -> Fleet {
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let mut up = Vec::new();
    for i in 0..online {
        let m = machine(&coord, dir.path(), &format!("on{i}")).await;
        serve_echo(&m);
        up.push(m);
    }
    let mut down = Vec::new();
    for i in 0..offline {
        let name = format!("off{i}");
        let m = machine(&coord, dir.path(), &name).await;
        m.shutdown();
        down.push(format!("machine-{name}"));
    }
    // The coordinator has noticed each one went away.
    for _ in 0..200 {
        if coord.connected() == online {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        coord.connected(),
        online,
        "offline machines still connected"
    );
    Fleet {
        _dir: dir,
        coord,
        online: up,
        offline: down,
    }
}

async fn wait_map(client: &Mesh, machines: usize, offline: &[String]) {
    for _ in 0..400 {
        let peers = client.peers();
        let all = peers.iter().filter(|p| p.kind == NodeKind::Machine).count() == machines;
        let marked = offline
            .iter()
            .all(|id| client.machine(id).is_some_and(|p| !p.online));
        if all && marked {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the client's map never listed every machine with its state");
}

async fn echo_once<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    s: &mut S,
) -> std::io::Result<()> {
    s.write_all(b"ping").await?;
    s.flush().await?;
    let mut got = [0u8; 4];
    s.read_exact(&mut got).await?;
    assert_eq!(&got, b"ping");
    Ok(())
}

#[derive(Default)]
struct Timings {
    online: Vec<Duration>,
    offline: Vec<Duration>,
    online_failed: Vec<String>,
    offline_answered: usize,
}

impl Timings {
    fn report_and_check(&self, scenario: &str) {
        let max = |v: &[Duration]| v.iter().max().copied().unwrap_or_default();
        eprintln!(
            "SCALE {scenario}: online n={} max={:?}  offline n={} max={:?}",
            self.online.len(),
            max(&self.online),
            self.offline.len(),
            max(&self.offline)
        );
        assert!(
            self.online_failed.is_empty(),
            "{scenario}: online machines failed: {:?}",
            self.online_failed
        );
        assert_eq!(
            self.offline_answered, 0,
            "{scenario}: an offline machine answered"
        );
        assert!(
            max(&self.online) <= ONLINE_BUDGET,
            "{scenario}: slowest online machine took {:?} (budget {ONLINE_BUDGET:?})",
            max(&self.online)
        );
        assert!(
            max(&self.offline) <= OFFLINE_BUDGET,
            "{scenario}: slowest offline machine took {:?} (budget {OFFLINE_BUDGET:?})",
            max(&self.offline)
        );
    }
}

/// Streams to every machine at once, straight on the node.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_machines_do_not_hold_up_streams_to_the_rest() {
    let f = fleet(
        size("CUA_MESH_SCALE_ONLINE", 15),
        size("CUA_MESH_SCALE_OFFLINE", 5),
    )
    .await;
    let client = Dialer::start(
        &f.coord.url,
        static_auth(vec![]),
        None,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    let total = f.online.len() + f.offline.len();
    wait_map(client.mesh(), total, &f.offline).await;

    let mesh = client.mesh().clone();
    let start = Instant::now();
    let mut tasks = Vec::new();
    // Offline ones first: the worst order for a shared connect lock.
    for id in f.offline.iter() {
        let key = mesh.machine(id).unwrap().node_key;
        let mesh = mesh.clone();
        tasks.push(tokio::spawn(async move {
            let r = tokio::time::timeout(CAP, async {
                let mut s = mesh.open_stream(&key, "echo", None).await?;
                echo_once(&mut s)
                    .await
                    .map_err(|e| cua_mesh::Error::Io(e.to_string()))
            })
            .await;
            (
                false,
                r.map(|r| r.is_ok()).unwrap_or(false),
                start.elapsed(),
            )
        }));
    }
    for m in &f.online {
        let key = m.node_key();
        let mesh = mesh.clone();
        tasks.push(tokio::spawn(async move {
            let r = tokio::time::timeout(CAP, async {
                let mut s = mesh.open_stream(&key, "echo", None).await?;
                echo_once(&mut s)
                    .await
                    .map_err(|e| cua_mesh::Error::Io(e.to_string()))
            })
            .await;
            (true, r.map(|r| r.is_ok()).unwrap_or(false), start.elapsed())
        }));
    }
    let mut t = Timings::default();
    for task in tasks {
        let (online, ok, took) = task.await.unwrap();
        match (online, ok) {
            (true, true) => t.online.push(took),
            (true, false) => t.online_failed.push(format!("{took:?}")),
            (false, true) => t.offline_answered += 1,
            (false, false) => t.offline.push(took),
        }
    }
    t.report_and_check("open_stream");
}

/// The Spaces path: a route (a loopback forward) per machine, then a
/// request through it, for every machine at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_machines_do_not_hold_up_routes_to_the_rest() {
    let f = fleet(
        size("CUA_MESH_SCALE_ONLINE", 15),
        size("CUA_MESH_SCALE_OFFLINE", 5),
    )
    .await;
    let client = Arc::new(
        Dialer::start(
            &f.coord.url,
            static_auth(vec![]),
            None,
            Duration::from_secs(10),
        )
        .await
        .unwrap(),
    );
    let total = f.online.len() + f.offline.len();
    wait_map(client.mesh(), total, &f.offline).await;

    let start = Instant::now();
    let mut tasks = Vec::new();
    let ids = f.offline.iter().map(|id| (false, id.clone())).chain(
        f.online
            .iter()
            .enumerate()
            .map(|(i, _)| (true, format!("machine-on{i}"))),
    );
    for (online, id) in ids {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let r = tokio::time::timeout(CAP, async {
                let route = client
                    .route(&id, None, Duration::from_secs(3))
                    .await
                    .map_err(|e| e.to_string())?;
                let mut s = tokio::net::TcpStream::connect(route.local)
                    .await
                    .map_err(|e| e.to_string())?;
                echo_once(&mut s).await.map_err(|e| e.to_string())
            })
            .await;
            (online, matches!(r, Ok(Ok(()))), start.elapsed())
        }));
    }
    let mut t = Timings::default();
    for task in tasks {
        let (online, ok, took) = task.await.unwrap();
        match (online, ok) {
            (true, true) => t.online.push(took),
            (true, false) => t.online_failed.push(format!("{took:?}")),
            (false, true) => t.offline_answered += 1,
            (false, false) => t.offline.push(took),
        }
    }
    t.report_and_check("route");
}

/// Computers that accept a connection and then never answer (a hung
/// driver, a machine going to sleep mid-request): the rest answer within
/// budget all the same, and each hung one is bounded by its caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unresponsive_machines_do_not_hold_up_the_rest() {
    let online = size("CUA_MESH_SCALE_ONLINE", 15);
    let hung = size("CUA_MESH_SCALE_HUNG", 5);
    let dir = tempfile::tempdir().unwrap();
    let coord = FakeCoordinator::start(Mode::Account).await;
    let mut up = Vec::new();
    for i in 0..online {
        let m = machine(&coord, dir.path(), &format!("on{i}")).await;
        serve_echo(&m);
        up.push(m);
    }
    // Their streams are never accepted, so never answered.
    let mut dark = Vec::new();
    for i in 0..hung {
        dark.push(machine(&coord, dir.path(), &format!("hung{i}")).await);
    }
    let client = Dialer::start(
        &coord.url,
        static_auth(vec![]),
        None,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    wait_map(client.mesh(), online + hung, &[]).await;

    let mesh = client.mesh().clone();
    let start = Instant::now();
    let mut tasks = Vec::new();
    let targets = dark
        .iter()
        .map(|m| (false, m.node_key()))
        .chain(up.iter().map(|m| (true, m.node_key())));
    for (awake, key) in targets {
        let mesh = mesh.clone();
        tasks.push(tokio::spawn(async move {
            // The caller's own deadline, as every Spaces call has one.
            let r = tokio::time::timeout(OFFLINE_BUDGET - Duration::from_millis(500), async {
                let mut s = mesh.open_stream(&key, "echo", None).await?;
                echo_once(&mut s)
                    .await
                    .map_err(|e| cua_mesh::Error::Io(e.to_string()))
            })
            .await;
            (awake, matches!(r, Ok(Ok(()))), start.elapsed())
        }));
    }
    let mut t = Timings::default();
    for task in tasks {
        let (awake, ok, took) = task.await.unwrap();
        match (awake, ok) {
            (true, true) => t.online.push(took),
            (true, false) => t.online_failed.push(format!("{took:?}")),
            (false, true) => t.offline_answered += 1,
            (false, false) => t.offline.push(took),
        }
    }
    t.report_and_check("unresponsive");
}
