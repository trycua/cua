// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Connection churn against a running relay, to watch its memory: machine
//! tunnels that come and go (and replace each other), mesh nodes that
//! reconnect and fight over one node key, and a steady stream of rejected
//! and offline-machine HTTP requests. The mix prod saw before the 3 Nov 2026
//! OOM kill.
//!
//! ```text
//! cargo run --release --example churn -- http://127.0.0.1:18080 reg 600
//! ```
//!
//! Arguments: relay URL, static registration token, seconds to run.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cua_mesh::{Mesh, MeshConfig, NodeKind};
use tokio_util::sync::CancellationToken;

const MACHINES: usize = 40;
const FIGHTING_PAIRS: usize = 8;
const FRESH_CLIENTS: usize = 8;
const HTTP_WORKERS: usize = 8;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:18080".into());
    let token = args.next().unwrap_or_else(|| "reg".into());
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(600);
    let dir = tempfile::tempdir().unwrap();
    let stop = CancellationToken::new();
    let counts: Arc<[AtomicU64; 4]> = Arc::new(Default::default());

    // A local spacesd stand-in the tunnels splice into.
    let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = backend.local_addr().unwrap();
    tokio::spawn(async move { while backend.accept().await.is_ok() {} });

    // Machine tunnels: up for 1-4 s, down, up again; every third cycle a
    // second session with the same id joins first ("replacing previous
    // session").
    for i in 0..MACHINES {
        let (url, token, stop, counts) = (url.clone(), token.clone(), stop.clone(), counts.clone());
        tokio::spawn(async move {
            let id = format!("churn{i:04}{}", "0".repeat(20));
            let mut n = 0u64;
            while !stop.is_cancelled() {
                n += 1;
                let mut join = cua_relay::client::JoinConfig::new(
                    url.replace("http://", "ws://"),
                    token.clone(),
                    id.clone(),
                    local,
                );
                join.heartbeat = Duration::from_secs(2);
                let s1 = CancellationToken::new();
                tokio::spawn(cua_relay::client::run(join.clone(), s1.clone()));
                let s2 = CancellationToken::new();
                if n.is_multiple_of(3) {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    tokio::spawn(cua_relay::client::run(join, s2.clone()));
                }
                tokio::time::sleep(Duration::from_millis(1000 + rand::random::<u64>() % 3000))
                    .await;
                s1.cancel();
                s2.cancel();
                counts[0].fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
    }

    // Mesh nodes. Pairs of client processes sharing one key file: each
    // connect replaces the other's ("mesh node replaced its older
    // connection"). Both restart every few seconds.
    for p in 0..FIGHTING_PAIRS {
        for side in 0..2 {
            let (url, stop, counts) = (url.clone(), stop.clone(), counts.clone());
            let key = dir.path().join(format!("pair{p}.json"));
            tokio::spawn(async move {
                while !stop.is_cancelled() {
                    let cfg = MeshConfig::new(
                        url.clone(),
                        NodeKind::Client,
                        Some(key.clone()),
                        cua_mesh::static_auth(Vec::new()),
                    );
                    if let Ok(node) = Mesh::start(cfg).await {
                        let _ = node.wait_ready(Duration::from_secs(5)).await;
                        tokio::time::sleep(Duration::from_millis(
                            1500 + side * 700 + rand::random::<u64>() % 2000,
                        ))
                        .await;
                        node.shutdown();
                        counts[1].fetch_add(1, Ordering::Relaxed);
                    }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            });
        }
    }
    // Fresh client nodes: a new key every time.
    for _ in 0..FRESH_CLIENTS {
        let (url, stop, counts) = (url.clone(), stop.clone(), counts.clone());
        tokio::spawn(async move {
            while !stop.is_cancelled() {
                let cfg = MeshConfig::new(
                    url.clone(),
                    NodeKind::Client,
                    None,
                    cua_mesh::static_auth(Vec::new()),
                );
                if let Ok(node) = Mesh::start(cfg).await {
                    let _ = node.wait_ready(Duration::from_secs(5)).await;
                    tokio::time::sleep(Duration::from_millis(500 + rand::random::<u64>() % 1500))
                        .await;
                    node.shutdown();
                    counts[2].fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }

    // Scale: machines with many paired clients online (prod had 1310 mesh
    // connections). Every connect or disconnect changes the paired
    // machine's netmap, so each recomputation re-encodes big maps.
    let paired_machines: usize = std::env::var("CHURN_PAIRED_MACHINES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let clients_per: usize = std::env::var("CHURN_CLIENTS_PER_MACHINE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for m in 0..paired_machines {
        let machine_id = format!("mesh{m:04}{}", "0".repeat(20));
        let env = format!("env-{m}");
        let mut cfg = MeshConfig::new(
            url.clone(),
            NodeKind::Machine,
            Some(dir.path().join(format!("m{m}.json"))),
            cua_mesh::static_auth(vec![
                ("authorization".into(), format!("Bearer {token}")),
                ("x-cua-machine-id".into(), machine_id.clone()),
            ]),
        );
        cfg.relay_only = true;
        cfg.approved_path = Some(dir.path().join(format!("approved{m}.json")));
        let (env_v, id_v) = (env.clone(), machine_id.clone());
        cfg.pair_verifier = Some(Arc::new(move |client, ts, proof| {
            cua_mesh::proto::verify_pair_proof(
                &env_v,
                &id_v,
                client,
                ts,
                proof,
                cua_mesh::proto::now_secs(),
            )
        }));
        let machine = Mesh::start(cfg).await.unwrap();
        let _ = machine.wait_ready(Duration::from_secs(10)).await;
        std::mem::forget(machine);
        for c in 0..clients_per {
            let (url, stop, counts, machine_id, env) = (
                url.clone(),
                stop.clone(),
                counts.clone(),
                machine_id.clone(),
                env.clone(),
            );
            let key = dir.path().join(format!("m{m}c{c}.json"));
            // One in ten clients comes and goes; the rest stay.
            let cycles = c.is_multiple_of(10);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(rand::random::<u64>() % 20_000)).await;
                while !stop.is_cancelled() {
                    let mut cfg = MeshConfig::new(
                        url.clone(),
                        NodeKind::Client,
                        Some(key.clone()),
                        cua_mesh::static_auth(Vec::new()),
                    );
                    cfg.relay_only = true;
                    let Ok(node) = Mesh::start(cfg).await else {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    };
                    let _ = node.wait_ready(Duration::from_secs(10)).await;
                    let _ = node.pair(&machine_id, &env).await;
                    if cycles {
                        tokio::time::sleep(Duration::from_millis(
                            3000 + rand::random::<u64>() % 5000,
                        ))
                        .await;
                        node.shutdown();
                        counts[2].fetch_add(1, Ordering::Relaxed);
                    } else {
                        stop.cancelled().await;
                        node.shutdown();
                        return;
                    }
                }
            });
        }
    }

    // HTTP: rejected tokens and requests for machines that are offline.
    let http = reqwest::Client::new();
    for w in 0..HTTP_WORKERS {
        let (url, stop, http, counts) = (url.clone(), stop.clone(), http.clone(), counts.clone());
        tokio::spawn(async move {
            let mut n = 0u64;
            while !stop.is_cancelled() {
                n += 1;
                let req = if n.is_multiple_of(2) {
                    http.get(format!("{url}/v1/machines"))
                        .bearer_auth(format!("bad-{w}-{n}"))
                } else {
                    http.get(format!("{url}/m/offline{n:08}{}/x", "0".repeat(16)))
                };
                let _ = req.send().await;
                counts[3].fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        tokio::time::sleep(Duration::from_secs(10)).await;
        println!(
            "CHURN t={}s machine_cycles={} pair_cycles={} fresh_cycles={} http={}",
            start.elapsed().as_secs(),
            counts[0].load(Ordering::Relaxed),
            counts[1].load(Ordering::Relaxed),
            counts[2].load(Ordering::Relaxed),
            counts[3].load(Ordering::Relaxed),
        );
    }
    stop.cancel();
    tokio::time::sleep(Duration::from_secs(2)).await;
}
