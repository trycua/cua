//! Chaos test of the E2B gateway over mock Spaces: a seeded plan of client
//! operations (create, exec, upload, delete, double delete, delete during
//! create) with clients that give up or die mid-request, and injected
//! faults (the gateway killed and restarted, the Cua daemon down, a
//! network partition to it). After every seed the leak check asserts that
//! no Space exists that no record owns, and no record owns a Space that is
//! gone: nothing outlives its sandbox's E2B timeout.
//!
//! The generic scheduler, plan and inventory are `cua-chaos`; this file
//! only drives the E2B gateway with them. A failure prints its seed:
//! `CHAOS_SEEDS=<seed> cargo test -p cua-compat --features testing --test chaos`.
//! `CHAOS_SEEDS` is `a..b` or a comma list (default `0..6`, a bounded PR run).

use cua_chaos::{Inventory, Item, Plan, Profile, Source, Sources, StepKind, Wanted, collect};
use cua_compat::backend::mock::MockBackend;
use cua_compat::backend::{
    Backend, CheckpointInfo, CreateExtras, CreateSpec, Created, Identity, Liveness, Result,
};
use cua_compat::{Gateway, GatewayOptions, Provider};
use cua_spacesd_client::SpacesdClient;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::{Builder, Runtime};

/// The Cua daemon as the gateway sees it: the mock Spaces running on their
/// own runtime (so they outlive a killed gateway, as a real daemon does),
/// with a switch for "the daemon is down" and a stall for a partition.
struct Daemon {
    inner: Arc<MockBackend>,
    rt: tokio::runtime::Handle,
    down: AtomicBool,
    stall_ms: AtomicU64,
}

impl Daemon {
    async fn gate(&self) -> Result<()> {
        let stall = self.stall_ms.load(Ordering::Relaxed);
        if stall > 0 {
            tokio::time::sleep(Duration::from_millis(stall)).await;
            return Err(cua_compat::backend::BackendError::Timeout(
                "partitioned from the daemon".into(),
            ));
        }
        if self.down.load(Ordering::Relaxed) {
            return Err(cua_compat::backend::BackendError::Other(
                "the Cua daemon is not running".into(),
            ));
        }
        Ok(())
    }

    async fn on_daemon<T: Send + 'static>(
        &self,
        f: impl std::future::Future<Output = Result<T>> + Send + 'static,
    ) -> Result<T> {
        self.rt
            .spawn(f)
            .await
            .map_err(|e| cua_compat::backend::BackendError::Other(e.to_string()))?
    }
}

#[async_trait::async_trait]
impl Backend for Daemon {
    async fn create(&self, who: &Identity, spec: CreateSpec) -> Result<Created> {
        self.create_with(who, spec, CreateExtras::default()).await
    }
    async fn create_with(
        &self,
        who: &Identity,
        spec: CreateSpec,
        extras: CreateExtras,
    ) -> Result<Created> {
        self.gate().await?;
        let (b, w) = (self.inner.clone(), who.clone());
        // Once the daemon accepted the create it finishes it, whatever
        // happens to the caller.
        self.on_daemon(async move { b.create_with(&w, spec, extras).await })
            .await
    }
    async fn set_lifetime(
        &self,
        who: &Identity,
        name: &str,
        seconds: u64,
        on_expire: &str,
    ) -> Result<bool> {
        self.gate().await?;
        self.inner.set_lifetime(who, name, seconds, on_expire).await
    }
    async fn delete(&self, who: &Identity, name: &str) -> Result<bool> {
        self.gate().await?;
        let (b, w, n) = (self.inner.clone(), who.clone(), name.to_string());
        self.on_daemon(async move { b.delete(&w, &n).await }).await
    }
    async fn liveness(&self, who: &Identity, name: &str) -> Result<Liveness> {
        self.gate().await?;
        self.inner.liveness(who, name).await
    }
    async fn spacesd(&self, who: &Identity, name: &str) -> Result<SpacesdClient> {
        self.gate().await?;
        let (b, w, n) = (self.inner.clone(), who.clone(), name.to_string());
        self.on_daemon(async move { b.spacesd(&w, &n).await }).await
    }
    async fn forward(&self, who: &Identity, name: &str, port: u16) -> Result<String> {
        self.gate().await?;
        self.inner.forward(who, name, port).await
    }
    async fn suspend(&self, who: &Identity, name: &str) -> Result<()> {
        self.gate().await?;
        self.inner.suspend(who, name).await
    }
    async fn resume(&self, who: &Identity, name: &str) -> Result<()> {
        self.gate().await?;
        self.inner.resume(who, name).await
    }
    async fn restart(&self, who: &Identity, name: &str) -> Result<()> {
        self.gate().await?;
        self.inner.restart(who, name).await
    }
    async fn checkpoint(&self, who: &Identity, name: &str, checkpoint: &str) -> Result<()> {
        self.gate().await?;
        self.inner.checkpoint(who, name, checkpoint).await
    }
    async fn checkpoints(&self, who: &Identity) -> Result<Vec<CheckpointInfo>> {
        self.gate().await?;
        self.inner.checkpoints(who).await
    }
    async fn delete_checkpoint(&self, who: &Identity, checkpoint: &str) -> Result<bool> {
        self.gate().await?;
        self.inner.delete_checkpoint(who, checkpoint).await
    }
}

struct GwRun {
    rt: Runtime,
    gw: Gateway,
}

/// Which gateway the run drives. The scheduler, plan and leak check are the
/// same; only the HTTP calls of "create a sandbox" and "delete one" differ.
#[derive(Clone, Copy, PartialEq)]
enum Prov {
    E2b,
    Daytona,
}

struct World {
    prov: Prov,
    daemon_rt: Runtime,
    backend: Arc<MockBackend>,
    daemon: Arc<Daemon>,
    dir: tempfile::TempDir,
    gw: Mutex<Option<GwRun>>,
    addr: Arc<Mutex<SocketAddr>>,
    key: Mutex<String>,
}

impl World {
    fn new(prov: Prov) -> Arc<World> {
        let daemon_rt = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let backend = Arc::new(MockBackend::new());
        // A create takes a moment, so kills land mid-create.
        backend.create_delay_ms.store(150, Ordering::Relaxed);
        let daemon = Arc::new(Daemon {
            inner: backend.clone(),
            rt: daemon_rt.handle().clone(),
            down: AtomicBool::new(false),
            stall_ms: AtomicU64::new(0),
        });
        // The daemon's own reaper: ends Spaces whose lifetime passed, whether
        // or not any gateway is alive (but not while the daemon is down).
        {
            let (b, d) = (backend.clone(), daemon.clone());
            daemon_rt.spawn(async move {
                loop {
                    if !d.down.load(Ordering::Relaxed) {
                        b.reap_expired().await;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            });
        }
        let w = Arc::new(World {
            prov,
            daemon_rt,
            backend,
            daemon,
            dir: tempfile::tempdir().unwrap(),
            gw: Mutex::new(None),
            addr: Arc::new(Mutex::new("127.0.0.1:0".parse().unwrap())),
            key: Mutex::new(String::new()),
        });
        w.start_gateway();
        w
    }

    fn gateway_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("gw")
    }

    fn start_gateway(&self) {
        let mut g = self.gw.lock().unwrap();
        if g.is_some() {
            return;
        }
        let rt = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let mut o = GatewayOptions::new(match self.prov {
            Prov::E2b => Provider::E2b,
            Prov::Daytona => Provider::Daytona,
        });
        o.listen = *self.addr.lock().unwrap();
        o.host_port = 0;
        o.dir = Some(self.gateway_dir());
        o.create_default_user = false;
        let backend: Arc<dyn Backend> = self.daemon.clone();
        // The port may still be closing after a kill: retry briefly.
        let mut last = None;
        let mut gw = None;
        for _ in 0..50 {
            match rt.block_on(Gateway::start(o.clone(), Some(backend.clone()))) {
                Ok(x) => {
                    gw = Some(x);
                    break;
                }
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        let gw = gw.unwrap_or_else(|| panic!("gateway did not start: {last:?}"));
        *self.addr.lock().unwrap() = gw.api_addr;
        *self.key.lock().unwrap() = gw.api_key.clone();
        *g = Some(GwRun { rt, gw });
    }

    /// SIGKILL: every task of the gateway stops where it is.
    fn kill_gateway(&self) {
        if let Some(run) = self.gw.lock().unwrap().take() {
            drop(run.gw);
            run.rt.shutdown_background();
        }
    }

    fn inventory(&self, label: &str) -> Inventory {
        let mut inv = collect(
            "gateway",
            label,
            &Sources {
                sources: vec![Source::Json {
                    kind: "record:gateway".into(),
                    file: self
                        .gateway_dir()
                        .join("state.json")
                        .to_string_lossy()
                        .into(),
                    pointer: "/records".into(),
                    id_field: Some("id".into()),
                    owner_field: Some("cua_name".into()),
                }],
            },
        );
        let names = self.daemon_rt.block_on(self.backend.names());
        for n in names {
            inv.add(Item::new("space", &n));
        }
        inv
    }
}

struct Ctx {
    prov: Prov,
    http: reqwest::Client,
    addr: Arc<Mutex<SocketAddr>>,
    key: Arc<Mutex<String>>,
    /// (actor, slot) -> (sandbox id, envd token)
    slots: Mutex<HashMap<(u32, i64), (String, String)>>,
}

impl Ctx {
    fn url(&self, p: &str) -> String {
        let base = match self.prov {
            Prov::E2b => "",
            Prov::Daytona => "/api",
        };
        format!("http://{}{base}{p}", self.addr.lock().unwrap())
    }
    fn auth(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.prov {
            Prov::E2b => r.header("x-api-key", self.key()),
            Prov::Daytona => r.bearer_auth(self.key()),
        }
    }
    fn coll(&self) -> &'static str {
        match self.prov {
            Prov::E2b => "/sandboxes",
            Prov::Daytona => "/sandbox",
        }
    }
    fn key(&self) -> String {
        self.key.lock().unwrap().clone()
    }
}

fn envelope(v: &Value) -> Vec<u8> {
    let p = v.to_string().into_bytes();
    let mut b = vec![0u8];
    b.extend_from_slice(&(p.len() as u32).to_be_bytes());
    b.extend_from_slice(&p);
    b
}

async fn create(c: &Ctx, actor: u32, slot: i64, timeout_s: i64) {
    let body = match c.prov {
        Prov::E2b => json!({"templateID": "base", "timeout": timeout_s}),
        Prov::Daytona => json!({}),
    };
    let Ok(r) = c
        .auth(c.http.post(c.url(c.coll())))
        .json(&body)
        .send()
        .await
    else {
        return;
    };
    if matches!(r.status().as_u16(), 200 | 201)
        && let Ok(v) = r.json::<Value>().await
    {
        let id = v["sandboxID"].as_str().or(v["id"].as_str()).unwrap_or("");
        c.slots.lock().unwrap().insert(
            (actor, slot),
            (
                id.to_string(),
                v["envdAccessToken"].as_str().unwrap_or("").to_string(),
            ),
        );
    }
}

async fn delete_id(c: &Ctx, id: &str) {
    let _ = c
        .auth(c.http.delete(c.url(&format!("{}/{id}", c.coll()))))
        .send()
        .await;
}

async fn op(c: Arc<Ctx>, step: cua_chaos::Step) {
    let arg = |k: &str| step.args.get(k).and_then(Value::as_i64);
    let slot = arg("slot").unwrap_or(0);
    let actor = step.actor;
    let held = c.slots.lock().unwrap().get(&(actor, slot)).cloned();
    // A client that gives up (its SDK timeout) or dies (SIGKILL) drops the
    // request where it is: both are a dropped future at the HTTP level.
    let give_up = arg("kill_after_ms").or(arg("abandon_after_ms"));
    let body = async {
        match step.name.as_str() {
            "create" => create(&c, actor, slot, arg("timeout_s").unwrap_or(3)).await,
            "exec" => {
                if let Some((id, tok)) = &held
                    && let Ok(r) = c
                        .http
                        .post(c.url("/process.Process/Start"))
                        .header("e2b-sandbox-id", id)
                        .header("x-access-token", tok)
                        .header("content-type", "application/connect+json")
                        .body(envelope(
                            &json!({"process": {"cmd": "echo", "args": ["hi"]}}),
                        ))
                        .send()
                        .await
                {
                    let _ = r.bytes().await;
                }
            }
            "upload" => {
                if let Some((id, tok)) = &held {
                    let kib = arg("kib").unwrap_or(1) as usize;
                    let _ = c
                        .http
                        .post(c.url("/files?path=/tmp/chaos.bin"))
                        .header("e2b-sandbox-id", id)
                        .header("x-access-token", tok)
                        .body(vec![7u8; kib * 1024])
                        .send()
                        .await;
                }
            }
            "delete" => {
                if let Some((id, _)) = held {
                    delete_id(&c, &id).await;
                    c.slots.lock().unwrap().remove(&(actor, slot));
                }
            }
            "double_delete" => {
                if let Some((id, _)) = held {
                    tokio::join!(delete_id(&c, &id), delete_id(&c, &id));
                    c.slots.lock().unwrap().remove(&(actor, slot));
                }
            }
            "delete_during_create" => {
                let c2 = c.clone();
                let creating = tokio::spawn(async move { create(&c2, actor, slot, 3).await });
                tokio::time::sleep(Duration::from_millis(60)).await;
                if let Ok(r) = c.auth(c.http.get(c.url(c.coll()))).send().await
                    && let Ok(v) = r.json::<Value>().await
                {
                    let list = match &v {
                        Value::Array(l) => l.clone(),
                        o => o["items"].as_array().cloned().unwrap_or_default(),
                    };
                    for s in list {
                        if let Some(id) = s["sandboxID"].as_str().or(s["id"].as_str()) {
                            delete_id(&c, id).await;
                        }
                    }
                }
                let _ = creating.await;
            }
            _ => {}
        }
    };
    match give_up {
        Some(ms) => {
            let _ = tokio::time::timeout(Duration::from_millis(ms.max(1) as u64), body).await;
        }
        None => body.await,
    }
}

const PROFILE: &str = r#"{
  "name": "e2b-hermetic", "actors": 6, "ops_per_actor": 8, "gap_ms": [10, 120],
  "faults": 4, "duration_ms": 1500,
  "ops": [
    {"name": "create", "weight": 6, "args": [
      {"name": "slot", "lo": 0, "hi": 3},
      {"name": "timeout_s", "lo": 2, "hi": 4},
      {"name": "kill_after_ms", "lo": 20, "hi": 400, "permille": 250},
      {"name": "abandon_after_ms", "lo": 20, "hi": 400, "permille": 250}]},
    {"name": "exec", "weight": 3, "args": [{"name": "slot", "lo": 0, "hi": 3},
      {"name": "kill_after_ms", "lo": 5, "hi": 200, "permille": 200}]},
    {"name": "upload", "weight": 2, "args": [{"name": "slot", "lo": 0, "hi": 3},
      {"name": "kib", "lo": 1, "hi": 2048},
      {"name": "kill_after_ms", "lo": 5, "hi": 200, "permille": 300}]},
    {"name": "delete", "weight": 3, "args": [{"name": "slot", "lo": 0, "hi": 3}]},
    {"name": "double_delete", "weight": 1, "args": [{"name": "slot", "lo": 0, "hi": 3}]},
    {"name": "delete_during_create", "weight": 1, "args": [{"name": "slot", "lo": 0, "hi": 3}]}
  ],
  "fault_menu": [
    {"name": "gateway_kill", "weight": 4, "heal": "gateway_start", "heal_after_ms": 400},
    {"name": "daemon_kill", "weight": 3, "heal": "daemon_start", "heal_after_ms": 500},
    {"name": "partition", "weight": 2, "heal": "partition_heal", "heal_after_ms": 600}
  ]
}"#;

fn seeds() -> Vec<u64> {
    let s = std::env::var("CHAOS_SEEDS").unwrap_or_else(|_| "0..6".into());
    if let Some((a, b)) = s.split_once("..") {
        return (a.parse().unwrap()..b.parse().unwrap()).collect();
    }
    s.split(',').map(|x| x.parse().unwrap()).collect()
}

struct Outcome {
    killed: usize,
    faults: Vec<String>,
    leaks: Result<()>,
}

fn run_seed(prov: Prov, seed: u64, profile_json: &str) -> std::result::Result<Outcome, String> {
    let profile: Profile = serde_json::from_str(profile_json).unwrap();
    let plan: Plan = profile.plan(seed);
    let world = World::new(prov);
    let client_rt = Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let ctx = Arc::new(Ctx {
        prov,
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap(),
        addr: world.addr.clone(),
        key: Arc::new(Mutex::new(world.key.lock().unwrap().clone())),
        slots: Mutex::default(),
    });
    let baseline = world.inventory("before");
    let start = Instant::now();
    let mut ops = Vec::new();
    let mut faults = Vec::new();
    let mut killed = 0;
    for step in &plan.steps {
        if let Some(wait) = Duration::from_millis(step.at_ms).checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
        match step.kind {
            StepKind::Op => {
                if step.args.contains_key("kill_after_ms") {
                    killed += 1;
                }
                ops.push(client_rt.spawn(op(ctx.clone(), step.clone())));
            }
            StepKind::Fault => {
                faults.push(step.name.clone());
                match step.name.as_str() {
                    "gateway_kill" => world.kill_gateway(),
                    "gateway_start" => {
                        world.start_gateway();
                        *ctx.key.lock().unwrap() = world.key.lock().unwrap().clone();
                    }
                    "daemon_kill" => world.daemon.down.store(true, Ordering::Relaxed),
                    "daemon_start" => world.daemon.down.store(false, Ordering::Relaxed),
                    "partition" => world.daemon.stall_ms.store(800, Ordering::Relaxed),
                    "partition_heal" => world.daemon.stall_ms.store(0, Ordering::Relaxed),
                    other => return Err(format!("unknown fault {other}")),
                }
            }
        }
    }
    client_rt.block_on(async {
        for o in ops {
            let _ = tokio::time::timeout(Duration::from_secs(30), o).await;
        }
    });
    // Heal everything, then the client's own cleanup: delete what it knows.
    world.daemon.down.store(false, Ordering::Relaxed);
    world.daemon.stall_ms.store(0, Ordering::Relaxed);
    world.start_gateway();
    *ctx.key.lock().unwrap() = world.key.lock().unwrap().clone();
    let known: Vec<String> = ctx
        .slots
        .lock()
        .unwrap()
        .values()
        .map(|(id, _)| id.clone())
        .collect();
    client_rt.block_on(async {
        for id in known {
            delete_id(&ctx, &id).await;
        }
    });
    if prov == Prov::Daytona {
        // Daytona sandboxes outlive their client by design (auto-stop is in
        // minutes): the user deletes what the list shows, and then nothing
        // may remain on either side.
        client_rt.block_on(async {
            if let Ok(r) = ctx.auth(ctx.http.get(ctx.url(ctx.coll()))).send().await
                && let Ok(v) = r.json::<Value>().await
            {
                let list = match &v {
                    Value::Array(l) => l.clone(),
                    o => o["items"].as_array().cloned().unwrap_or_default(),
                };
                for s in list {
                    if let Some(id) = s["id"].as_str() {
                        delete_id(&ctx, id).await;
                    }
                }
            }
        });
    }
    // Everything left must go by itself: the E2B timeouts are seconds.
    let deadline = Instant::now() + Duration::from_secs(25);
    let wanted = Wanted::default();
    let report = loop {
        let r = world.inventory("after").leaks(&baseline, &wanted);
        if r.is_clean() || Instant::now() > deadline {
            break r;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    // Say what happened to each leaked Space: was a delete ever attempted?
    let deleted: Vec<String> = world
        .daemon_rt
        .block_on(async { world.backend.deleted.lock().await.clone() });
    let mut report = report;
    for l in &mut report.leaks {
        if l.item.kind == "space" {
            l.reason = format!(
                "{} (delete attempted: {})",
                l.reason,
                deleted.contains(&l.item.id)
            );
        }
    }
    let leaks = report
        .assert_clean(&format!(
            "seed {seed}: leak after the E2B gateway chaos run (faults {faults:?}); replay with CHAOS_SEEDS={seed}"
        ))
        .map_err(cua_compat::backend::BackendError::Other);
    world.kill_gateway();
    client_rt.shutdown_background();
    Ok(Outcome {
        killed,
        faults,
        leaks,
    })
}

fn campaign(prov: Prov, name: &str, profile_json: &str) {
    let mut failures = Vec::new();
    let (mut killed, mut faults) = (0, 0);
    let all = seeds();
    for seed in &all {
        let out = run_seed(prov, *seed, profile_json).unwrap();
        killed += out.killed;
        faults += out.faults.len();
        if let Err(e) = out.leaks {
            failures.push(e.to_string());
        }
    }
    eprintln!(
        "chaos {name}: {} seeds, {killed} clients killed or abandoned, {faults} faults injected, {} leaking seeds",
        all.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn e2b_gateway_leaks_nothing_under_seeded_chaos() {
    campaign(Prov::E2b, "e2b", PROFILE);
}

#[test]
fn daytona_gateway_leaks_nothing_under_seeded_chaos() {
    campaign(Prov::Daytona, "daytona", PROFILE);
}

/// A client dies and the gateway dies with it for good: the sandboxes still
/// end at their E2B timeout, because the Cua daemon holds the lifetime
/// (nothing of the gateway runs to enforce it).
#[test]
fn sandboxes_end_at_their_timeout_with_the_gateway_dead() {
    let world = World::new(Prov::E2b);
    let client_rt = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let ctx = Arc::new(Ctx {
        prov: Prov::E2b,
        http: reqwest::Client::new(),
        addr: world.addr.clone(),
        key: Arc::new(Mutex::new(world.key.lock().unwrap().clone())),
        slots: Mutex::default(),
    });
    client_rt.block_on(async {
        for i in 0..4 {
            create(&ctx, 1, i, 3).await;
        }
    });
    assert_eq!(world.daemon_rt.block_on(world.backend.names()).len(), 4);
    // Let the lifetime syncer push (wait for the daemon to hold all four; a
    // fixed sleep is too short on a loaded CI machine), then the gateway is
    // killed and never returns.
    let pushed = Instant::now() + Duration::from_secs(10);
    while Instant::now() < pushed
        && world
            .daemon_rt
            .block_on(world.backend.lifetimes.lock())
            .len()
            < 4
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    world.kill_gateway();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !world.daemon_rt.block_on(world.backend.names()).is_empty() {
        std::thread::sleep(Duration::from_millis(250));
    }
    let left = world.daemon_rt.block_on(world.backend.names());
    assert!(
        left.is_empty(),
        "alive after their timeout with the gateway dead: {left:?}"
    );
}

/// 64 clients create at once, each cancelling (SIGKILL or SDK give-up) at a
/// random moment, with deletes racing the creates and the gateway killed in
/// the middle of the burst.
const BURST: &str = r#"{
  "name": "burst", "actors": 64, "ops_per_actor": 2, "gap_ms": [0, 150],
  "faults": 2, "duration_ms": 800,
  "ops": [
    {"name": "create", "weight": 6, "args": [
      {"name": "slot", "lo": 0, "hi": 0},
      {"name": "timeout_s", "lo": 2, "hi": 4},
      {"name": "kill_after_ms", "lo": 5, "hi": 400, "permille": 400},
      {"name": "abandon_after_ms", "lo": 5, "hi": 400, "permille": 300}]},
    {"name": "delete", "weight": 2, "args": [{"name": "slot", "lo": 0, "hi": 0}]},
    {"name": "delete_during_create", "weight": 1, "args": [{"name": "slot", "lo": 0, "hi": 0}]}
  ],
  "fault_menu": [
    {"name": "gateway_kill", "weight": 3, "heal": "gateway_start", "heal_after_ms": 300},
    {"name": "daemon_kill", "weight": 1, "heal": "daemon_start", "heal_after_ms": 400}
  ]
}"#;

#[test]
fn a_64_way_burst_with_random_cancels_leaks_nothing() {
    campaign(Prov::E2b, "e2b-burst", BURST);
    campaign(Prov::Daytona, "daytona-burst", BURST);
}
