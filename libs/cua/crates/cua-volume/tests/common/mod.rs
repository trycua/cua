// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Shared by the replication simulations: a group of real nodes on a
//! simulated network.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_volume::backend::{Backend, Condition};
use cua_volume::fs::FsBackend;
use cua_volume::replica::simnet::{Impair, SimNet};
use cua_volume::replica::{Config, Facts, Member, ReplicaNode, ReplicatedBackend};

pub struct M {
    #[allow(dead_code)]
    pub id: String,
    pub node: ReplicaNode,
    pub backend: Arc<ReplicatedBackend>,
    pub store: FsBackend,
    pub dir: tempfile::TempDir,
}

pub struct Sim {
    pub net: SimNet,
    pub m: HashMap<String, M>,
    pub members: Vec<Member>,
    pub cfg: Config,
}

pub fn member(id: &str, rank: u64) -> Member {
    Member {
        id: id.into(),
        name: id.to_string(),
        facts: Facts {
            rank,
            free_disk: 500 << 30,
            ..Default::default()
        },
        last_seen_ms: 0,
    }
}

impl Sim {
    pub async fn new(specs: &[(&str, u64)]) -> Sim {
        Sim::with(specs, Config::fast()).await
    }

    pub async fn with(specs: &[(&str, u64)], cfg: Config) -> Sim {
        // CUA_TEST_LOG=cua_volume=debug shows what the nodes decide.
        if let Ok(f) = std::env::var("CUA_TEST_LOG") {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(f)
                .with_test_writer()
                .try_init();
        }
        let net = SimNet::new();
        let members: Vec<Member> = specs.iter().map(|(i, r)| member(i, *r)).collect();
        let mut sim = Sim {
            net,
            m: HashMap::new(),
            members,
            cfg,
        };
        for (id, _) in specs {
            sim.boot(id, tempfile::tempdir().unwrap()).await;
        }
        sim
    }

    pub async fn boot(&mut self, id: &str, dir: tempfile::TempDir) {
        let (tr, mut rx) = self.net.join(id);
        let store = FsBackend::new(dir.path().join("data"));
        let node = ReplicaNode::open(
            self.cfg.clone(),
            id,
            id,
            dir.path(),
            store.clone(),
            Arc::new(tr),
            self.members.clone(),
        )
        .await
        .unwrap();
        let facts = self
            .members
            .iter()
            .find(|m| m.id == id)
            .unwrap()
            .facts
            .clone();
        node.set_facts(facts);
        let n2 = node.clone();
        tokio::spawn(async move {
            while let Some((_from, s)) = rx.recv().await {
                n2.accept(s).await;
            }
        });
        node.start();
        let backend = node.backend();
        self.m.insert(
            id.to_string(),
            M {
                id: id.to_string(),
                node,
                backend,
                store,
                dir,
            },
        );
    }

    pub fn get(&self, id: &str) -> &M {
        &self.m[id]
    }

    /// A process restart: shut down, keep the directory, boot again.
    pub async fn restart(&mut self, id: &str) {
        let old = self.m.remove(id).unwrap();
        old.node.shutdown().await;
        let dir = old.dir;
        drop(old.backend);
        self.boot(id, dir).await;
    }

    pub fn cut(&self, a: &str, b: &str) {
        self.net.set_both(
            a,
            b,
            Impair {
                sever: true,
                ..Default::default()
            },
        );
    }

    pub fn heal(&self, a: &str, b: &str) {
        self.net.heal(a, b);
    }

    pub async fn shutdown(&self) {
        for m in self.m.values() {
            m.node.shutdown().await;
        }
    }
}

pub async fn eventually<F, Fut>(what: &str, secs: u64, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if f().await {
            return;
        }
        if Instant::now() > end {
            panic!("timed out waiting for: {what}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A machine that needs an ordered write (a lease, a fence) keeps asking
/// until it can have one; that is what makes it take over from a dead owner.
pub async fn needs_authority(m: &M, key: &str) {
    eventually("an ordered write succeeds", 30, || async {
        m.backend
            .put(key, b"x".to_vec(), Condition::IfNoneMatch)
            .await
            .is_ok()
    })
    .await;
}

pub async fn head_etag(m: &M, key: &str) -> Option<String> {
    m.store.head(key).await.unwrap().map(|h| h.etag)
}

pub async fn heads(m: &M) -> Vec<(String, String)> {
    m.store
        .list("")
        .await
        .unwrap()
        .into_iter()
        .map(|h| (h.key, h.etag))
        .collect()
}

pub async fn converged(sim: &Sim, ids: &[&str]) -> bool {
    let first = heads(sim.get(ids[0])).await;
    for id in &ids[1..] {
        if heads(sim.get(id)).await != first {
            return false;
        }
    }
    true
}

pub async fn group_ready(sim: &Sim, owner: &str) {
    let dump = || async {
        for m in sim.m.values() {
            let s = m.node.status().await;
            eprintln!(
                "{}: owner={} epoch={} peers={:?}",
                m.id,
                s.owner,
                s.epoch,
                s.peers
                    .iter()
                    .map(|p| (p.id.clone(), p.health.clone(), p.connected))
                    .collect::<Vec<_>>()
            );
        }
    };
    let end = Instant::now() + Duration::from_secs(14);
    while Instant::now() < end {
        let mut ok = true;
        for m in sim.m.values() {
            if m.node.status().await.owner != owner {
                ok = false;
            }
        }
        if ok {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    dump().await;
    eventually("the group agrees on an owner", 5, || async {
        for m in sim.m.values() {
            let s = m.node.status().await;
            if s.owner != owner {
                return false;
            }
        }
        true
    })
    .await;
    // Everyone is connected to everyone.
    eventually("the machines are connected", 15, || async {
        for m in sim.m.values() {
            let s = m.node.status().await;
            if s.peers.iter().any(|p| !p.connected) {
                return false;
            }
        }
        true
    })
    .await;
}
