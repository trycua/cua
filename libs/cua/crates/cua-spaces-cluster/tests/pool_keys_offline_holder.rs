// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A device signed in while the account's devices that hold the pool keys
//! are offline (an asleep laptop, a box that was switched off): a pool
//! request fails within seconds, not minutes, and says which devices hold
//! the keys and the ways on. It never starts new keys on its own while they
//! are enrolled. Once one of them is online, it shares them and the request
//! goes through; once the person revoked them all, this device makes new
//! keys and the request goes through.
//!
//! The relay is a stand-in over HTTP with the relay's keyring rules
//! ([`MemPool`]), so the agent's real client code runs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use cua_cluster::link::NodeLink;
use cua_spaces_cluster::coordinator::LinkFactory;
use cua_spaces_cluster::coordinator::store::NodeRow;
use cua_spaces_cluster::pool::keys::{DeviceSigner, KeyDirectory, PoolKeys};
use cua_spaces_cluster::pool::memlog::{MemKeys, MemPool};
use cua_spaces_cluster::pool::relay::{PoolAuth, RelayPool};
use cua_spaces_cluster::pool::seal::{Keyring, Member, PoolSecret, device_id};
use cua_spaces_cluster::pool::{AgentConfig, Host, KEYS_NOT_SHARED, PoolAgent, PoolMachine};
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

/// The new device: signed in to the stand-in relay, provides no Spaces.
struct NewDevice {
    relay: RelayPool,
    signer: Arc<Signer>,
    secret: PoolSecret,
}

#[async_trait]
impl Host for NewDevice {
    async fn relay(&self) -> Option<RelayPool> {
        Some(self.relay.clone())
    }
    fn name(&self) -> String {
        "new-box".into()
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
async fn a_new_device_fails_fast_naming_offline_holders_then_gets_the_keys() {
    let pool = MemPool::new();
    let fake = Fake {
        keys: pool.keys(ACCOUNT),
        devices: Arc::default(),
        rpcs: Arc::default(),
    };
    let base = serve(fake.clone()).await;
    let enroll = |s: &Signer, name: &str| {
        fake.devices
            .lock()
            .unwrap()
            .insert(s.device(), (s.public_key(), name.to_string()));
    };

    // The first device made the keys, then went to sleep 3 days ago.
    let laptop = Signer::new();
    enroll(&laptop, "ada-laptop");
    let laptop_keys = PoolKeys::new(
        ACCOUNT,
        laptop.clone(),
        PoolSecret::generate(),
        Arc::new(RelayPool::new(
            &base,
            ACCOUNT,
            Arc::new(Auth(laptop.device())),
        )),
    );
    laptop_keys.sync().await.unwrap();
    pool.saw(&laptop.device(), now() - 3 * 86_400);

    // A new device signs in and runs something on the pool.
    let new = Signer::new();
    enroll(&new, "new-box");
    let relay = RelayPool::new(&base, ACCOUNT, Arc::new(Auth(new.device())));
    let host = Arc::new(NewDevice {
        relay: relay.clone(),
        signer: new.clone(),
        secret: PoolSecret::generate(),
    });
    let dir = tempfile::tempdir().unwrap();
    let cfg = AgentConfig {
        keys_grace: Duration::from_secs(3),
        ..AgentConfig::default()
    };
    let agent = PoolAgent::start(dir.path().join("pool"), host, cfg)
        .await
        .unwrap();
    let submit = json!({"run": "p-1", "command": ["echo", "hi"], "on": "cloud:hetzner"});
    let t = Instant::now();
    let (status, body) = agent.request("pool", "submit", submit.clone()).await;
    let took = t.elapsed();
    // Fast: the grace, not the 60 s a request waits for a coordinator.
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["code"], KEYS_NOT_SHARED, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("ada-laptop"), "{msg}");
    assert!(msg.contains("last seen 3 days ago"), "{msg}");
    assert!(msg.contains("cua devices revoke"), "{msg}");
    assert_eq!(body["holders"][0]["device"], laptop.device(), "{body}");
    assert!(
        fake.rpcs.lock().unwrap().is_empty(),
        "nothing went out unsealed"
    );
    // It made no new keys of its own: the laptop still holds generation 1.
    let ring = pool.keyring().unwrap();
    assert_eq!(ring.generation, 1);
    assert!(ring.wraps.iter().all(|w| w.device == laptop.device()));
    // What `cua computers` shows: who holds the keys.
    let keys = agent.keys_for(&relay).await.unwrap();
    assert_eq!(keys.view().holders[0].name, "ada-laptop");
    assert!(keys.view().waiting.is_some());

    // The laptop wakes up: its upkeep shares the keys with the new device,
    // and the request goes through (sealed) to the coordinator.
    laptop_keys.sync().await.unwrap();
    let (status, body) = agent.request("pool", "submit", submit.clone()).await;
    // The stand-in's mailbox answers unsealed, which the agent refuses.
    assert_eq!(status, 502, "{body}");
    let rpc = fake.rpcs.lock().unwrap().pop().unwrap();
    assert!(rpc["params"]["sealed"].is_object(), "{rpc}");
    assert_eq!(pool.keyring().unwrap().generation, 1, "same keys, shared");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn once_the_offline_holders_are_revoked_the_new_device_makes_new_keys() {
    let pool = MemPool::new();
    let fake = Fake {
        keys: pool.keys(ACCOUNT),
        devices: Arc::default(),
        rpcs: Arc::default(),
    };
    let base = serve(fake.clone()).await;
    let old = Signer::new();
    fake.devices
        .lock()
        .unwrap()
        .insert(old.device(), (old.public_key(), "old-box".into()));
    PoolKeys::new(
        ACCOUNT,
        old.clone(),
        PoolSecret::generate(),
        Arc::new(RelayPool::new(&base, ACCOUNT, Arc::new(Auth(old.device())))),
    )
    .sync()
    .await
    .unwrap();

    let new = Signer::new();
    fake.devices
        .lock()
        .unwrap()
        .insert(new.device(), (new.public_key(), "new-box".into()));
    let keys = PoolKeys::new(
        ACCOUNT,
        new.clone(),
        PoolSecret::generate(),
        Arc::new(RelayPool::new(&base, ACCOUNT, Arc::new(Auth(new.device())))),
    );
    let e = keys.sync().await.unwrap_err();
    assert!(e.to_string().contains("old-box"), "{e}");
    assert_eq!(keys.view().holders.len(), 1);
    // The person revokes the box they no longer have (`cua devices
    // revoke`): no device holds the keys any more, so this one starts new
    // ones, wrapped only to the devices still there.
    pool.revoke(&old.device());
    fake.devices.lock().unwrap().remove(&old.device());
    let b = keys.sync().await.unwrap();
    assert_eq!(b.generation(), 2);
    let ring = pool.keyring().unwrap();
    assert!(ring.wraps.iter().all(|w| w.device == new.device()));
    assert_eq!(keys.view().holders[0].name, "new-box");
}
