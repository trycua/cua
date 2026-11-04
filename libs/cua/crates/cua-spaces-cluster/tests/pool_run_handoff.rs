// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A run submitted before the relay is reachable (a daemon that just
//! started: the mesh is not connected yet) is registered on this machine's
//! own coordinator. When the relay comes up, the pool agent routes pool
//! requests through the relay's coordinator role; the run must still be
//! found, followed and cancelled, not "no run ...".

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use common::FakeExec;
use cua_cluster::link::{LocalLink, NodeLink};
use cua_cluster::node::{NodeConfig, NodeJobs};
use cua_spaces_cluster::coordinator::LinkFactory;
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::pool::keys::{DeviceSigner, KeyDirectory as _};
use cua_spaces_cluster::pool::memlog::{MemKeys, MemPool};
use cua_spaces_cluster::pool::relay::{PoolAuth, RelayPool};
use cua_spaces_cluster::pool::seal::{Keyring, Member, PoolSecret, device_id};
use cua_spaces_cluster::pool::{AgentConfig, Host, PoolAgent, PoolMachine};
use serde_json::{Value, json};

const ACCOUNT: &str = "acct";

struct Signer(ring::signature::EcdsaKeyPair, String);

impl Signer {
    fn new() -> Arc<Self> {
        use base64::Engine as _;
        use ring::signature::KeyPair as _;
        let rng = ring::rand::SystemRandom::new();
        let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
        let public =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pair.public_key().as_ref());
        Arc::new(Self(pair, public))
    }
}

impl DeviceSigner for Signer {
    fn device(&self) -> String {
        device_id(&self.1).unwrap()
    }
    fn public_key(&self) -> String {
        self.1.clone()
    }
    fn sign(&self, message: &str) -> Result<String, String> {
        use base64::Engine as _;
        let rng = ring::rand::SystemRandom::new();
        self.0
            .sign(&rng, message.as_bytes())
            .map(|s| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_ref()))
            .map_err(|_| "sign".into())
    }
}

/// The stand-in relay: who the devices are, the keyring rules, and what
/// reached the coordinator's mailbox.
#[derive(Clone)]
struct Fake {
    keys: MemKeys,
    /// `dev_…` -> (P-256 key, name).
    devices: Arc<Mutex<HashMap<String, (String, String)>>>,
    /// Sealed requests that reached the mailbox.
    rpcs: Arc<Mutex<Vec<Value>>>,
}

fn caller(h: &HeaderMap) -> String {
    h.get("x-test-device")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn refuse(status: StatusCode, code: &str, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"code": code, "error": msg})))
}

async fn serve(f: Fake) -> String {
    let app = Router::new()
        .route(
            "/v1/pool/member",
            post(
                |State(f): State<Fake>, h: HeaderMap, Json(b): Json<Value>| async move {
                    let device = caller(&h);
                    let Some((pk, _)) = f.devices.lock().unwrap().get(&device).cloned() else {
                        return refuse(StatusCode::FORBIDDEN, "device_not_enrolled", "unknown");
                    };
                    let m = Member {
                        device,
                        public_key: pk,
                        pool_key: b["pool_key"].as_str().unwrap_or_default().into(),
                        sig: b["sig"].as_str().unwrap_or_default().into(),
                        name: String::new(),
                    };
                    match f.keys.register(&m).await {
                        Ok(()) => (StatusCode::OK, Json(json!({"registered": true}))),
                        Err(e) => refuse(StatusCode::BAD_REQUEST, "invalid", &e.to_string()),
                    }
                },
            ),
        )
        .route(
            "/v1/pool/keys",
            get(|State(f): State<Fake>| async move {
                let mut st = f.keys.fetch().await.unwrap();
                let names = f.devices.lock().unwrap().clone();
                for m in &mut st.members {
                    m.name = names.get(&m.device).map(|d| d.1.clone()).unwrap_or_default();
                }
                Json(serde_json::to_value(st).unwrap())
            })
            .put(
                |State(f): State<Fake>, Json(b): Json<Value>| async move {
                    let ring: Keyring = serde_json::from_value(b["keyring"].clone()).unwrap();
                    match f
                        .keys
                        .put(
                            b["version"].as_u64().unwrap_or(0),
                            &ring,
                            b["reset"].as_bool().unwrap_or(false),
                        )
                        .await
                    {
                        Ok(v) => (StatusCode::OK, Json(json!({"version": v}))),
                        Err(e) => refuse(StatusCode::CONFLICT, "keyring_changed", &e.to_string()),
                    }
                },
            ),
        )
        .route(
            "/v1/devices",
            get(|State(f): State<Fake>| async move {
                let seen = f.keys.last_seen().await;
                let devices: Vec<Value> = f
                    .devices
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(id, (_, name))| json!({"id": id, "name": name, "last_seen": seen.get(id)}))
                    .collect();
                Json(json!({"devices": devices}))
            }),
        )
        // Nobody else coordinates; this test's devices never contend.
        .route(
            "/v1/pool/lease",
            post(|| async { refuse(StatusCode::CONFLICT, "held", "held") }),
        )
        // The coordinator's mailbox: what reached it, then a refusal the
        // agent passes on at once (no coordinator in this test).
        .route(
            "/v1/pool/rpc",
            post(|State(f): State<Fake>, Json(b): Json<Value>| async move {
                f.rpcs.lock().unwrap().push(b);
                refuse(StatusCode::FORBIDDEN, "test_reached", "reached the coordinator")
            }),
        )
        .with_state(f);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await });
    base
}

struct Auth(String);

#[async_trait]
impl PoolAuth for Auth {
    async fn headers(&self) -> Result<Vec<(String, String)>, String> {
        Ok(vec![("x-test-device".into(), self.0.clone())])
    }
}

/// This device: the relay appears later (`up`), as the mesh connects.
struct Dev {
    relay: RelayPool,
    up: AtomicBool,
    signer: Arc<Signer>,
    secret: PoolSecret,
    other: Arc<NodeJobs>,
}

#[async_trait]
impl Host for Dev {
    async fn relay(&self) -> Option<RelayPool> {
        self.up.load(Ordering::SeqCst).then(|| self.relay.clone())
    }
    fn name(&self) -> String {
        "laptop".into()
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
        None
    }
    fn elastic(&self) -> Option<(NodeRow, LinkFactory)> {
        None
    }
    async fn device_signer(&self, _relay: &RelayPool) -> Option<Arc<dyn DeviceSigner>> {
        Some(self.signer.clone())
    }
    fn pool_secret(&self) -> Result<PoolSecret, String> {
        Ok(self.secret.clone())
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_submitted_before_the_relay_connects_is_still_found_after() {
    let _ = now();
    let pool = MemPool::new();
    let fake = Fake {
        keys: pool.keys(ACCOUNT),
        devices: Arc::default(),
        rpcs: Arc::default(),
    };
    let base = serve(fake.clone()).await;
    let me = Signer::new();
    fake.devices
        .lock()
        .unwrap()
        .insert(me.device(), (me.public_key(), "laptop".into()));
    let dir = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let other = NodeJobs::open(
        NodeConfig {
            dir: dir.path().join("other"),
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
    .unwrap();
    let host = Arc::new(Dev {
        relay: RelayPool::new(&base, ACCOUNT, Arc::new(Auth(me.device()))),
        up: AtomicBool::new(false),
        signer: me,
        secret: PoolSecret::generate(),
        other,
    });
    let cfg = AgentConfig {
        lease_ttl: Duration::from_secs(3),
        discover_every: Duration::from_millis(200),
        keys_grace: Duration::from_secs(2),
        ..AgentConfig::default()
    };
    let agent = PoolAgent::start(dir.path().join("pool"), host.clone(), cfg)
        .await
        .unwrap();

    // Just after the daemon starts: no relay yet, so the pool is this
    // machine's own coordinator.
    let (s, v) = agent
        .request(
            "pool",
            "submit",
            json!({"run": "early-1", "command": ["echo", "hi"], "count": 2, "on": "studio"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // The mesh connects: the relay is there (and has a pool API).
    host.up.store(true, Ordering::SeqCst);
    for _ in 0..5 {
        let (s, v) = agent
            .request("pool", "run", json!({"run": "early-1"}))
            .await;
        assert_eq!(s, 200, "the run was lost at the handoff: {v}");
        assert_eq!(v["run"], "early-1", "{v}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let (s, v) = agent.request("pool", "runs", json!({})).await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["run"] == "early-1"),
        "{v}"
    );
    // And it still runs to the end, on the machine it was placed on.
    let mut done = Value::Null;
    for _ in 0..100 {
        let (s, v) = agent
            .request("pool", "run", json!({"run": "early-1"}))
            .await;
        assert_eq!(s, 200, "{v}");
        if v["done"] == true {
            done = v;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(done["done"], true, "the run never finished: {done}");
    let (s, v) = agent
        .request("pool", "cancel_run", json!({"run": "early-1"}))
        .await;
    assert_eq!(s, 200, "{v}");
}
