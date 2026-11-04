// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A pool whose lease holder runs an older Cua (mixed versions: an old Mac
//! that provides Spaces and a new Linux box, or an old laptop that took the
//! lease while it had a request waiting). A new client's run that needs a
//! newer coordinator (job services, `--on <cloud>`) is never handed to the
//! older one, where it would run without its credentials or never place:
//! it is refused naming the machine to update. Runs that do not need it
//! still go through, and a newer holder takes them as before.
//!
//! The relay is a stand-in over HTTP that answers the pool's status and
//! records the requests that reach its mailbox, so the agent's real client
//! code runs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use cua_cluster::link::NodeLink;
use cua_spaces_cluster::coordinator::LinkFactory;
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::pool::relay::{PoolAuth, RelayPool};
use cua_spaces_cluster::pool::{
    AgentConfig, Host, POOL_LEVEL, PRIORITY_CLIENT, PRIORITY_PROVIDER, PoolAgent, PoolMachine,
    lease_priority,
};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Fake {
    /// (name, priority) of whoever holds the lease.
    holder: Arc<Mutex<Option<(String, u32)>>>,
    /// Requests that reached the mailbox (sealed or not).
    rpcs: Arc<Mutex<Vec<Value>>>,
}

async fn serve(f: Fake) -> String {
    let app = Router::new()
        .route(
            "/v1/pool",
            get(|State(f): State<Fake>| async move {
                let h = f.holder.lock().unwrap().clone();
                Json(json!({
                    "epoch": 7,
                    "holder": h.as_ref().map(|(n, p)| json!({"device": "dev_old", "name": n, "machine": "", "priority": p})),
                    "contenders": h.as_ref().map(|(n, p)| vec![json!({"device": "dev_old", "name": n, "machine": "", "priority": p, "seen_ms_ago": 100})]).unwrap_or_default(),
                    "head": 0,
                    "waiting_requests": 0,
                }))
            }),
        )
        .route(
            "/v1/pool/lease",
            post(|State(f): State<Fake>| async move {
                let h = f.holder.lock().unwrap().clone();
                (
                    axum::http::StatusCode::CONFLICT,
                    Json(json!({"granted": false, "code": "held", "holder": h.map(|(n, _)| n)})),
                )
            }),
        )
        .route(
            "/v1/pool/rpc",
            post(|State(f): State<Fake>, Json(b): Json<Value>| async move {
                f.rpcs.lock().unwrap().push(b);
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"code": "no_coordinator", "error": "nobody"})),
                )
            }),
        )
        .with_state(f);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

struct Auth;

#[async_trait]
impl PoolAuth for Auth {
    async fn headers(&self) -> Result<Vec<(String, String)>, String> {
        Ok(vec![])
    }
}

/// The new client: signed in, provides no Spaces.
struct NewClient(RelayPool);

#[async_trait]
impl Host for NewClient {
    async fn relay(&self) -> Option<RelayPool> {
        Some(self.0.clone())
    }
    fn name(&self) -> String {
        "new-linux".into()
    }
    fn machine_id(&self) -> String {
        String::new()
    }
    fn provides_spaces(&self) -> bool {
        false
    }
    async fn account_machines(&self) -> Result<Vec<PoolMachine>, String> {
        Ok(vec![])
    }
    async fn direct_machines(&self) -> Vec<PoolMachine> {
        vec![]
    }
    async fn local_runner(&self) -> Option<Arc<dyn NodeLink>> {
        None
    }
    fn elastic(&self) -> Option<(NodeRow, LinkFactory)> {
        None
    }
}

fn cfg() -> AgentConfig {
    AgentConfig {
        lease_ttl: Duration::from_secs(1),
        discover_every: Duration::from_millis(200),
        coordinator: |d| cua_spaces_cluster::coordinator::Config::new(d),
        keys_grace: Duration::from_millis(500),
        space_setup: None,
    }
}

async fn agent_with_holder(
    holder: Option<(&str, u32)>,
) -> (Arc<PoolAgent>, Fake, tempfile::TempDir) {
    let f = Fake::default();
    *f.holder.lock().unwrap() = holder.map(|(n, p)| (n.to_string(), p));
    let base = serve(f.clone()).await;
    let relay = RelayPool::new(&base, "acct", Arc::new(Auth));
    let dir = tempfile::tempdir().unwrap();
    let agent = PoolAgent::start(dir.path(), Arc::new(NewClient(relay)), cfg())
        .await
        .unwrap();
    (agent, f, dir)
}

fn compat_submit() -> Value {
    json!({
        "run": "p-python-mixed",
        "gang": true,
        "command": ["python", "e2b_app.py"],
        "on": "hetzner-1",
        "services": ["compat:e2b"],
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_machine_that_provides_spaces_is_named_and_never_gets_a_job_with_services() {
    // An old Mac (the first pool agent: priority 10) holds the lease.
    let (agent, f, _d) = agent_with_holder(Some(("old-mac", PRIORITY_PROVIDER))).await;
    let t = Instant::now();
    let (status, body) = agent.request("pool", "submit", compat_submit()).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["code"], "coordinator_too_old", "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(
        msg.contains("old-mac") && msg.contains("update Cua on old-mac"),
        "{msg}"
    );
    assert!(msg.contains("services"), "{msg}");
    // At once: an older machine that provides Spaces never hands over.
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(
        f.rpcs.lock().unwrap().is_empty(),
        "the job reached the older coordinator"
    );
    // A run pinned to a cloud too (the old one would never place it).
    let (status, body) = agent
        .request(
            "pool",
            "submit",
            json!({"run": "r2", "command": ["true"], "on": "cloud:hetzner"}),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("--on <cloud>"),
        "{body}"
    );
    assert!(f.rpcs.lock().unwrap().is_empty());
    agent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_laptop_that_does_not_hand_over_is_named_after_a_bounded_wait() {
    // An old laptop (priority 1) took the lease while it had a request
    // waiting. A newer machine contends; this one never lets go.
    let (agent, f, _d) = agent_with_holder(Some(("old-laptop", PRIORITY_CLIENT))).await;
    let t = Instant::now();
    let (status, body) = agent.request("pool", "submit", compat_submit()).await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("old-laptop"),
        "{body}"
    );
    let waited = t.elapsed();
    assert!(
        waited >= Duration::from_secs(2) && waited < Duration::from_secs(10),
        "{waited:?}"
    );
    assert!(f.rpcs.lock().unwrap().is_empty());
    agent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_newer_holder_takes_the_job_and_plain_runs_still_go_to_an_older_one() {
    // A newer holder: the check passes (this stand-in then has no keys for
    // the new device, so the request stops there, not at the check).
    let (agent, _f, _d) = agent_with_holder(Some(("new-box", lease_priority(true)))).await;
    let (status, body) = agent.request("pool", "submit", compat_submit()).await;
    assert_ne!(body["code"], "coordinator_too_old", "{body}");
    assert_ne!(status, 409, "{body}");
    agent.shutdown().await;

    // An older holder still gets runs that need nothing new.
    let (agent, _f, _d) = agent_with_holder(Some(("old-mac", PRIORITY_PROVIDER))).await;
    let (status, body) = agent
        .request(
            "pool",
            "submit",
            json!({"run": "r3", "command": ["true"], "on": "old-mac"}),
        )
        .await;
    assert_ne!(body["code"], "coordinator_too_old", "{body}");
    assert_ne!(status, 409, "{body}");
    agent.shutdown().await;
    assert_eq!(POOL_LEVEL, 1);
}
