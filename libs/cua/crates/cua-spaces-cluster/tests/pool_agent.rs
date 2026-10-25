// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The pool agent's routing without a relay: a run on this machine never
//! asks for the relay and runs only on this machine's runner; a run on
//! your machines does not use this machine unless it provides Spaces.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use common::FakeExec;
use cua_cluster::link::{LocalLink, NodeLink};
use cua_cluster::node::{NodeConfig, NodeJobs};
use cua_spaces_cluster::coordinator::LinkFactory;
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::pool::relay::RelayPool;
use cua_spaces_cluster::pool::{AgentConfig, Host, PoolAgent, PoolMachine};
use serde_json::{Value, json};

fn runner(dir: &std::path::Path, exec: FakeExec) -> Arc<NodeJobs> {
    NodeJobs::open(
        NodeConfig {
            dir: dir.to_path_buf(),
            millicpus_total: 8000,
            memory_total_mb: 1 << 16,
            jobs_max: 0,
            min_available_mb: 0,
            retention: Duration::from_secs(3600),
            output_retention: Duration::from_secs(3600),
            boot_id: "b".into(),
        },
        Arc::new(exec),
    )
    .unwrap()
}

tokio::task_local! {
    /// Set while the test's own request is answered: the agent answers it
    /// on the caller's task, so a relay lookup from that request is told
    /// apart from the lease loop's (its own task, on its own timer).
    static IN_REQUEST: ();
}

struct FakeHost {
    /// Relay lookups made while answering the test's requests.
    relay_asked: AtomicU32,
    provides: AtomicBool,
    local: Arc<NodeJobs>,
    other: Arc<NodeJobs>,
}

#[async_trait]
impl Host for FakeHost {
    async fn relay(&self) -> Option<RelayPool> {
        if IN_REQUEST.try_with(|_| ()).is_ok() {
            self.relay_asked.fetch_add(1, Ordering::SeqCst);
        }
        None
    }
    fn name(&self) -> String {
        "laptop".into()
    }
    fn machine_id(&self) -> String {
        String::new()
    }
    fn provides_spaces(&self) -> bool {
        self.provides.load(Ordering::SeqCst)
    }
    async fn account_machines(&self) -> Result<Vec<PoolMachine>, String> {
        Ok(vec![])
    }
    async fn direct_machines(&self) -> Vec<PoolMachine> {
        vec![PoolMachine {
            row: NodeRow {
                name: "studio".into(),
                direct: "100.64.0.9:3211".into(),
                via: "direct".into(),
                os: "linux".into(),
                ..Default::default()
            },
            link: Arc::new(LocalLink::new(self.other.clone(), "studio", "linux").owned_by("local")),
        }]
    }
    async fn local_runner(&self) -> Option<Arc<dyn NodeLink>> {
        Some(Arc::new(
            LocalLink::new(self.local.clone(), "laptop", "linux").owned_by("local"),
        ))
    }
    fn elastic(&self) -> Option<(NodeRow, LinkFactory)> {
        None
    }
}

fn cfg() -> AgentConfig {
    AgentConfig {
        lease_ttl: Duration::from_secs(3),
        discover_every: Duration::from_millis(200),
        coordinator: |d| {
            let mut c = cua_spaces_cluster::coordinator::Config::new(d);
            c.poll_wait = Duration::from_millis(200);
            c
        },
        keys_grace: Duration::from_secs(2),
    }
}

/// `agent.request`, with the relay lookups it makes counted.
async fn request(agent: &PoolAgent, target: &str, method: &str, params: Value) -> (u16, Value) {
    IN_REQUEST
        .scope((), agent.request(target, method, params))
        .await
}

async fn wait_done(agent: &PoolAgent, target: &str, run: &str) -> Value {
    for _ in 0..300 {
        let (s, v) = request(agent, target, "run", json!({"run": run})).await;
        assert_eq!(s, 200, "{v}");
        if v["done"] == true {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{run} never finished");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_runs_stay_local_and_auto_skips_a_machine_that_does_not_provide() {
    let dir = tempfile::tempdir().unwrap();
    let (le, oe) = (FakeExec::new(), FakeExec::new());
    let host = Arc::new(FakeHost {
        relay_asked: AtomicU32::new(0),
        provides: AtomicBool::new(false),
        local: runner(&dir.path().join("local"), le.clone()),
        other: runner(&dir.path().join("other"), oe.clone()),
    });
    let agent = PoolAgent::start(dir.path().join("pool"), host.clone(), cfg())
        .await
        .unwrap();
    // The lease loop asks whether there is a relay on its own timer; a
    // local run's requests must not ask at all.
    let (s, v) = request(
        &agent,
        "local",
        "submit",
        json!({"run": "l-1", "command": ["echo", "hi"], "count": 6, "on": "local"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    wait_done(&agent, "local", "l-1").await;
    let (_, jobs) = request(&agent, "local", "jobs", json!({"run": "l-1"})).await;
    assert_eq!(
        host.relay_asked.load(Ordering::SeqCst),
        0,
        "a local run asked for the relay"
    );
    let jobs = jobs["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 6);
    assert!(
        jobs.iter()
            .all(|j| j["state"] == "succeeded" && j["node"] == "local"),
        "{jobs:?}"
    );
    assert_eq!(
        oe.seen.lock().unwrap().starts.len(),
        0,
        "a local run left the machine"
    );

    // `auto` without a relay: this machine (not providing Spaces) is not a
    // candidate; the direct machine takes them. (A pool request does look
    // for a relay: the count above is not vacuous.)
    let (s, v) = request(
        &agent,
        "pool",
        "submit",
        json!({"run": "p-1", "command": ["echo", "x"], "count": 5}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(host.relay_asked.load(Ordering::SeqCst) > 0);
    wait_done(&agent, "pool", "p-1").await;
    let (_, jobs) = agent.request("pool", "jobs", json!({"run": "p-1"})).await;
    assert!(
        jobs["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["node"] == "studio"),
        "{jobs}"
    );
    assert_eq!(
        le.seen.lock().unwrap().starts.len(),
        6,
        "only the local run ran here"
    );

    // Canceling a large run ends it at once: queued jobs in one write, the
    // ones on machines stopped there.
    let (s, v) = agent
        .request(
            "pool",
            "submit",
            json!({"run": "p-2", "command": ["sleep", "60000"], "count": 400, "cpus": 0.1, "memory_mb": 16}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    tokio::time::sleep(Duration::from_millis(800)).await;
    let t = std::time::Instant::now();
    let (s, v) = agent
        .request("pool", "cancel_run", json!({"run": "p-2"}))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["canceled"], 400);
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    let done = wait_done(&agent, "pool", "p-2").await;
    assert_eq!(done["counts"]["canceled"], 400, "{done}");
    agent.shutdown().await;
}
