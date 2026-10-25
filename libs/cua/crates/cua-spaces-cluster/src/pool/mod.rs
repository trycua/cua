// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The pool agent: what every signed-in machine's `cua daemon` runs so
//! that all of the account's machines act as one, with no coordinator for
//! anyone to set up (`rfcs/pool-self-coordination.md`).
//!
//! - **This machine's own runs** (`cua sb run` with `on` omitted or
//!   `local`) go to a coordinator in this daemon with a local ledger and
//!   only this machine's job runner. Nothing about them leaves the machine.
//! - **Pool runs** (`--on auto`, or a machine's name) go to the machine
//!   that holds the account's lease on the relay. A machine that provides
//!   Spaces contends for the lease; a client contends only when it has a
//!   request waiting and nobody holds it, and yields to a provider. The
//!   holder rebuilds the coordinator's state from the relay's log
//!   ([`crate::coordinator::journal::rebuild`]), records every change there
//!   under its epoch, places on the account's machines that provide
//!   Spaces, and answers the account's devices through the relay's
//!   mailbox. When it stops renewing (asleep, gone, cut off), another
//!   machine takes the lease and goes on from the log.
//! - **Without a relay pool** (machines added by their Tailscale or LAN
//!   address, or a relay without the pool API), pool runs are coordinated
//!   here, attended: they pause while this machine sleeps.

pub mod keys;
pub mod memlog;
pub mod relay;
pub mod seal;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::coordinator::requests::{self, Caller};
use crate::coordinator::store::NodeRow;
use crate::coordinator::{Config, Coordinator, LinkFactory, journal};
use cua_cluster::link::NodeLink;
use keys::{DeviceSigner, KeyError, PoolKeys, SealedLog};
use relay::{PoolError, RelayPool};
use seal::{Envelope, PoolSecret};

/// The name of this machine's own runner in its coordinators (what
/// `on="local"` pins a job to).
pub const LOCAL: &str = "local";

/// The `code` of a pool request refused because this device's pool keys
/// are not shared yet (its `holders` name the devices that hold them).
pub const KEYS_NOT_SHARED: &str = "keys_not_shared";

/// Base lease priority of a machine that provides Spaces (always-on hosts).
pub const PRIORITY_PROVIDER: u32 = 10;
/// Base lease priority of a client (a laptop).
pub const PRIORITY_CLIENT: u32 = 1;

/// What this coordinator carries beyond the first pool agent, added to its
/// lease priority so the relay (which only compares priorities) prefers
/// it, and every machine can read it off the pool's status:
///
/// - 0 (priority 1 or 10): the first pool agent.
/// - 1: it runs jobs with host services (`compat:e2b`, ...) on every kind
///   of host, a dedicated one included, and places `--on <cloud>` jobs on
///   that cloud's hosts by their relay machine meta.
///
/// A client's level stays below `PRIORITY_PROVIDER - PRIORITY_CLIENT` so a
/// client never outranks a machine that provides Spaces. Older agents only
/// test `priority >= PRIORITY_PROVIDER` and `priority < PRIORITY_PROVIDER`,
/// so a higher level reads to them as the same role (the wire is unchanged).
pub const POOL_LEVEL: u32 = 1;

/// This machine's lease priority: its role's base plus [`POOL_LEVEL`].
pub fn lease_priority(provider: bool) -> u32 {
    if provider {
        PRIORITY_PROVIDER + POOL_LEVEL
    } else {
        PRIORITY_CLIENT + POOL_LEVEL
    }
}

/// The level ([`POOL_LEVEL`]) a lease priority says, and whether it is a
/// machine that provides Spaces.
pub fn level_of(priority: u32) -> (u32, bool) {
    if priority >= PRIORITY_PROVIDER {
        (priority - PRIORITY_PROVIDER, true)
    } else {
        (priority.saturating_sub(PRIORITY_CLIENT), false)
    }
}

/// The level a pool request needs of whoever coordinates it (0: any):
/// a submit whose jobs ask for host services, or that is pinned to a
/// cloud (`cloud:<provider>`), needs level 1; an older coordinator would
/// run it without its services (no credentials) or never place it.
pub fn level_needed(method: &str, params: &Value) -> Option<&'static str> {
    if method != "submit" {
        return None;
    }
    let has_services = |v: &Value| v["services"].as_array().is_some_and(|a| !a.is_empty());
    let jobs = params["jobs"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if has_services(params) || jobs.iter().any(has_services) {
        return Some("jobs with services (cua run --compat)");
    }
    let cloud = |v: &Value| v["on"].as_str().is_some_and(|o| o.starts_with("cloud:"));
    if cloud(params) || jobs.iter().any(cloud) {
        return Some("jobs pinned to a cloud (--on <cloud>)");
    }
    None
}

/// Who coordinates, from the relay's pool status (`null`: nobody):
/// `{name, device, this_machine, provides_spaces, level, older}`.
pub fn coordinator_view(status: &Value) -> Value {
    let h = &status["holder"];
    if h.is_null() {
        return Value::Null;
    }
    let (level, provider) = level_of(h["priority"].as_u64().unwrap_or(0) as u32);
    json!({
        "name": h["name"],
        "device": h["device"],
        "this_machine": status.get("this_machine_coordinates").is_some(),
        "provides_spaces": provider,
        "level": level,
        "older": level < POOL_LEVEL,
    })
}

/// Whether the relay's pool `status` shows a contender (not `me`, seen
/// within `ttl_ms`) that ranks above `priority` and is at least as new: the
/// one a holder at `priority` hands the lease to.
pub fn newer_contender_waits(status: &Value, me: &str, priority: u32, ttl_ms: u64) -> bool {
    let (mine, _) = level_of(priority);
    status["contenders"].as_array().is_some_and(|cs| {
        cs.iter().any(|c| {
            let p = c["priority"].as_u64().unwrap_or(0) as u32;
            c["name"].as_str() != Some(me)
                && p > priority
                && level_of(p).0 >= mine
                && c["seen_ms_ago"].as_u64().unwrap_or(u64::MAX) < ttl_ms
        })
    })
}

/// The refusal when the machine that coordinates the pool runs an older
/// Cua than a request needs.
pub fn too_old_message(holder: &str, provider: bool, what: &str) -> String {
    let stop = if provider {
        " (or stop Cua there: `cua daemon stop`; a newer machine then coordinates)".to_string()
    } else {
        String::new()
    };
    format!(
        "your machine {holder} coordinates your pool and runs an older Cua that cannot run \
         {what}; update Cua on {holder} to run this job{stop}. Update: curl -fsSL \
         https://cua.ai/install.sh | sh"
    )
}

/// A machine a coordinator places on, and how to reach it.
#[derive(Clone)]
pub struct PoolMachine {
    /// Its row (`relay_machine`: its id on the relay; `name`: suggested).
    pub row: NodeRow,
    /// How to reach its job runner.
    pub link: Arc<dyn NodeLink>,
}

/// What the agent needs from the machine it runs on (the daemon).
#[async_trait]
pub trait Host: Send + Sync + 'static {
    /// The signed-in account's pool on its relay, when signed in.
    async fn relay(&self) -> Option<RelayPool>;
    /// This machine's name, for people.
    fn name(&self) -> String;
    /// This machine's id on the relay (empty: not set up as a host).
    fn machine_id(&self) -> String;
    /// Whether this machine provides Spaces now.
    fn provides_spaces(&self) -> bool;
    /// The account's machines that provide Spaces (this one included when
    /// it does, reached in-process), each with a link: what the pool
    /// places on. `Err` when they could not be listed (the relay).
    async fn account_machines(&self) -> Result<Vec<PoolMachine>, String>;
    /// Machines added by their direct (Tailscale or LAN) address that
    /// provide Spaces.
    async fn direct_machines(&self) -> Vec<PoolMachine>;
    /// This machine's own job runner.
    async fn local_runner(&self) -> Option<Arc<dyn NodeLink>>;
    /// Elastic capacity (Modal) when the user connected it here: its row
    /// and the factory that answers for it.
    fn elastic(&self) -> Option<(NodeRow, LinkFactory)>;
    /// This device's enrolled device key on the relay of `relay` (it
    /// certifies the pool key), when there is one.
    async fn device_signer(&self, _relay: &RelayPool) -> Option<Arc<dyn DeviceSigner>> {
        None
    }
    /// This device's pool key, made and stored on first use.
    fn pool_secret(&self) -> Result<PoolSecret, String> {
        Err("this machine keeps no pool key".into())
    }
}

/// How the agent behaves.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// Lease time to live (renewed every third).
    pub lease_ttl: Duration,
    /// How often the account's machines are listed again.
    pub discover_every: Duration,
    /// The coordinators' tuning.
    pub coordinator: fn(&Path) -> Config,
    /// How long a pool request waits for this device's keys while
    /// devices that hold them are enrolled (one that is online shares
    /// them within seconds) before it says which devices hold them.
    pub keys_grace: Duration,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            lease_ttl: Duration::from_secs(15),
            discover_every: Duration::from_secs(10),
            coordinator: |d| Config::new(d),
            keys_grace: Duration::from_secs(20),
        }
    }
}

type Links = Arc<std::sync::Mutex<HashMap<String, Arc<dyn NodeLink>>>>;

fn factory(links: &Links, elastic: Option<LinkFactory>) -> LinkFactory {
    let links = links.clone();
    Arc::new(move |row: &NodeRow| {
        if let Some(l) = links.lock().expect("links").get(&row.name) {
            return Some(l.clone());
        }
        elastic.as_ref().and_then(|f| f(row))
    })
}

struct Holding {
    epoch: u64,
    relay: RelayPool,
    coord: Arc<Coordinator>,
    stop: CancellationToken,
}

/// Requests that name one run (`run` in the params).
const RUN_KEYED: [&str; 5] = ["run", "job_output", "run_result", "cancel_run", "audit"];

/// The pool agent of one machine.
pub struct PoolAgent {
    dir: PathBuf,
    host: Arc<dyn Host>,
    cfg: AgentConfig,
    own: Arc<Coordinator>,
    own_links: Links,
    holding: tokio::sync::Mutex<Option<Holding>>,
    /// A request here waits for someone to coordinate the pool.
    wanted: AtomicBool,
    /// The relay has no pool API: pool runs are coordinated here.
    no_relay_pool: AtomicBool,
    /// The account's pool keys, per pool (relay and account).
    keys: tokio::sync::Mutex<HashMap<String, Arc<PoolKeys>>>,
    changed: tokio::sync::Notify,
    stop: CancellationToken,
}

impl PoolAgent {
    /// Starts the agent with its state under `dir` (`<cua home>/pool`).
    pub async fn start(
        dir: impl Into<PathBuf>,
        host: Arc<dyn Host>,
        cfg: AgentConfig,
    ) -> Result<Arc<Self>, String> {
        let dir = dir.into();
        let own_links: Links = Arc::default();
        let elastic = host.elastic();
        let own = Coordinator::start(
            (cfg.coordinator)(&dir.join("own")),
            Some(factory(&own_links, elastic.as_ref().map(|e| e.1.clone()))),
        )
        .await
        .map_err(|e| e.to_string())?;
        let me = Arc::new(Self {
            dir,
            host,
            cfg,
            own,
            own_links,
            holding: tokio::sync::Mutex::new(None),
            wanted: AtomicBool::new(false),
            no_relay_pool: AtomicBool::new(false),
            keys: tokio::sync::Mutex::default(),
            changed: tokio::sync::Notify::new(),
            stop: CancellationToken::new(),
        });
        me.discover_own().await;
        if let Some((row, _)) = elastic {
            let _ = me.own.add_node(row).await;
        }
        tokio::spawn(me.clone().own_loop());
        tokio::spawn(me.clone().lease_loop());
        Ok(me)
    }

    /// Stops the agent (releasing the lease when it holds it).
    pub async fn shutdown(&self) {
        self.stop.cancel();
        self.drop_holding("this machine's daemon stops", true).await;
        self.own.stop();
    }

    /// This machine's own coordinator.
    pub fn own(&self) -> &Arc<Coordinator> {
        &self.own
    }

    /// The pool coordinator, while this machine holds the lease.
    pub async fn holder(&self) -> Option<(u64, Arc<Coordinator>)> {
        self.holding
            .lock()
            .await
            .as_ref()
            .map(|h| (h.epoch, h.coord.clone()))
    }

    // ---- requests ---------------------------------------------------------

    /// Answers a request from this machine (the CLI, MCP, SDK, app):
    /// `target` `local` is this machine's own coordinator; `pool` is
    /// whoever coordinates the account's machines (this machine, or another
    /// through the relay's mailbox), or this machine when there is no pool
    /// on a relay.
    pub async fn request(&self, target: &str, method: &str, params: Value) -> (u16, Value) {
        let caller = Caller::local();
        if target == LOCAL {
            return requests::answer(&self.own, method, params, &caller).await;
        }
        // A run registered on this machine's own coordinator (submitted
        // while the daemon had no relay yet, before the coordinator role
        // settled) stays followed here: whoever coordinates the pool now
        // does not know it. Run ids are unique, so asking this machine
        // first is never ambiguous; a run it does not have goes on.
        if RUN_KEYED.contains(&method)
            && let Some(run) = params["run"].as_str().filter(|r| !r.is_empty())
            && requests::answer(&self.own, "run", json!({ "run": run }), &caller)
                .await
                .0
                == 200
        {
            return requests::answer(&self.own, method, params, &caller).await;
        }
        let (status, body) = self.request_pool(method, params.clone(), &caller).await;
        // `runs`: add the ones still on this machine's coordinator (the
        // pool's answer failing does not hide them).
        if method == "runs" {
            let (s, mine) = requests::answer(&self.own, method, params, &caller).await;
            if s == 200
                && let Some(extra) = mine["runs"].as_array()
            {
                let mut all = if status == 200 {
                    body["runs"].as_array().cloned().unwrap_or_default()
                } else {
                    Vec::new()
                };
                let before = all.len();
                for r in extra {
                    if !all.iter().any(|x| x["run"] == r["run"]) {
                        all.push(r.clone());
                    }
                }
                if status == 200 || all.len() > before {
                    return (200, json!({ "runs": all }));
                }
            }
        }
        (status, body)
    }

    async fn request_pool(&self, method: &str, params: Value, caller: &Caller) -> (u16, Value) {
        let caller = caller.clone();
        let deadline = Instant::now() + Duration::from_secs(60);
        let keys_deadline = Instant::now() + self.cfg.keys_grace;
        let mut checked = false;
        loop {
            if let Some((epoch, coord)) = self.holder().await {
                let (status, body) =
                    requests::answer(&coord, method, params.clone(), &caller).await;
                // Another machine took over while this one answered: ask
                // whoever coordinates now (requests are idempotent).
                if status >= 500 && coord.is_fenced() && Instant::now() < deadline {
                    self.drop_epoch(epoch, "another machine coordinates now")
                        .await;
                    continue;
                }
                return (status, body);
            }
            let relay = match self.host.relay().await {
                Some(r) if !self.no_relay_pool.load(Ordering::SeqCst) => r,
                _ => return requests::answer(&self.own, method, params, &caller).await,
            };
            if let Some(what) = level_needed(method, &params)
                && !checked
            {
                checked = true;
                if let Err(e) = self.check_holder_level(&relay, what).await {
                    return (409, json!({"error": e, "code": "coordinator_too_old"}));
                }
                // This machine may coordinate now.
                continue;
            }
            let Some(keys) = self.keys_for(&relay).await else {
                return (
                    503,
                    json!({"error": "this device has no pool key yet: sign in again (`cua auth login`)"}),
                );
            };
            let sealed = match seal_request(&keys, method, &params).await {
                Ok(s) => s,
                Err(KeyError::NotShared { holders }) => {
                    // A device that holds the keys and is online shares
                    // them within seconds; past that, waiting longer does
                    // not help: say who holds them (not a retryable 503).
                    if Instant::now() >= keys_deadline {
                        return (
                            403,
                            json!({
                                "error": keys::not_shared_message(&holders),
                                "code": KEYS_NOT_SHARED,
                                "holders": holders,
                            }),
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                Err(e) => {
                    if Instant::now() >= deadline {
                        return (503, json!({"error": e.to_string()}));
                    }
                    tracing::debug!("pool keys: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            match relay
                .rpc(method, sealed.params, request_meta(method, &params))
                .await
            {
                Ok((status, body)) => {
                    return open_reply(&keys, method, &sealed.rid, status, body).await;
                }
                Err(PoolError::Refused { code, .. }) if code == "rekey" => {
                    if let Err(e) = keys.sync().await {
                        tracing::debug!("pool keys: {e}");
                    }
                    continue;
                }
                Err(PoolError::Unsupported(_)) => {
                    tracing::info!(
                        "the relay does not coordinate pools: this machine coordinates its pool runs"
                    );
                    self.no_relay_pool.store(true, Ordering::SeqCst);
                    self.discover_own().await;
                }
                Err(PoolError::Refused {
                    code,
                    message,
                    body,
                }) => {
                    if !matches!(
                        code.as_str(),
                        "no_coordinator" | "retry" | "timeout" | "busy"
                    ) {
                        return (
                            if code == "401" { 401 } else { 403 },
                            json!({"error": message, "detail": body}),
                        );
                    }
                    if Instant::now() >= deadline {
                        return (
                            503,
                            json!({"error": "none of your machines could coordinate this in time; is one that provides Spaces online?"}),
                        );
                    }
                    if code == "no_coordinator" {
                        // Nobody coordinates: this machine will.
                        self.wanted.store(true, Ordering::SeqCst);
                        self.changed.notify_waiters();
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(PoolError::Unavailable(e)) => {
                    if Instant::now() >= deadline {
                        return (503, json!({"error": e}));
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// The pool's state on the relay: who coordinates, contenders, log
    /// head (`None`: no relay pool).
    pub async fn pool_status(&self) -> Option<Value> {
        let relay = self.host.relay().await?;
        let mut v = relay.status().await.ok()?;
        if let Some((epoch, _)) = self.holder().await {
            v["this_machine_coordinates"] = json!(epoch);
        }
        v["coordinator"] = coordinator_view(&v);
        if let Some(k) = self.keys_for(&relay).await {
            v["keys"] = json!(k.view());
        }
        Some(v)
    }

    /// The account's pool keys on this device (`None`: not signed in, or
    /// this device has no device key or pool key).
    pub async fn keys_for(&self, relay: &RelayPool) -> Option<Arc<PoolKeys>> {
        let signer = self.host.device_signer(relay).await?;
        let pool = relay.key();
        let mut all = self.keys.lock().await;
        if let Some(k) = all.get(&pool)
            && k.device() == signer.device()
        {
            return Some(k.clone());
        }
        let secret = match self.host.pool_secret() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("this machine's pool key: {e}");
                return None;
            }
        };
        let account = pool.rsplit_once('#').map(|(_, a)| a).unwrap_or_default();
        let k = PoolKeys::new(account, signer, secret, Arc::new(relay.clone()));
        all.insert(pool, k.clone());
        Some(k)
    }

    // ---- this machine's own coordinator ----------------------------------

    async fn discover_own(&self) {
        let mut machines = Vec::new();
        if let Some(link) = self.host.local_runner().await {
            let provides = self.host.provides_spaces();
            machines.push(PoolMachine {
                row: NodeRow {
                    name: LOCAL.into(),
                    via: "local".into(),
                    // Jobs that name it only, unless it provides Spaces.
                    state: if provides { "ready" } else { "pinned" }.into(),
                    os: std::env::consts::OS.into(),
                    ..Default::default()
                },
                link,
            });
        }
        machines.extend(self.host.direct_machines().await);
        if self.no_relay_pool.load(Ordering::SeqCst)
            && let Ok(more) = self.host.account_machines().await
        {
            let mine = self.host.machine_id();
            machines.extend(more.into_iter().filter(|m| m.row.relay_machine != mine));
        }
        apply_machines(&self.own, &self.own_links, machines, true).await;
    }

    async fn own_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => return,
                _ = tokio::time::sleep(self.cfg.discover_every * 3) => {}
            }
            self.discover_own().await;
        }
    }

    // ---- the lease ----------------------------------------------------------

    async fn lease_loop(self: Arc<Self>) {
        let tick = self.cfg.lease_ttl / 3;
        let mut last_ok = Instant::now();
        let mut backoff = Duration::ZERO;
        let mut ticks = 0u64;
        loop {
            ticks += 1;
            tokio::select! {
                _ = self.stop.cancelled() => return,
                _ = tokio::time::sleep(tick + backoff) => {}
                _ = self.changed.notified() => {}
            }
            let Some(relay) = self.host.relay().await else {
                self.drop_holding("signed out", false).await;
                continue;
            };
            self.drop_holding_of_another_account(&relay).await;
            if self.no_relay_pool.load(Ordering::SeqCst) {
                continue;
            }
            let priority = lease_priority(self.host.provides_spaces());
            // Every machine keeps the account's keys in step (and shares
            // them with new devices, or makes new ones when a device left).
            let keys = self.keys_for(&relay).await;
            if let Some(k) = &keys
                && (k.cached().is_none() || ticks.is_multiple_of(2))
                && let Err(e) = k.sync().await
            {
                tracing::debug!("pool keys: {e}");
            }
            let held = self.holding.lock().await.as_ref().map(|h| h.epoch);
            let name = self.host.name();
            let machine = self.host.machine_id();
            match held {
                Some(epoch) => {
                    match relay
                        .lease(epoch, &name, &machine, priority, self.cfg.lease_ttl)
                        .await
                    {
                        Ok(g) if g.epoch == epoch => {
                            last_ok = Instant::now();
                            if let Some(to) = &g.yield_to
                                && self.should_yield(&relay, priority).await
                            {
                                self.drop_holding(
                                    &format!(
                                        "{to} ranks above this machine and coordinates from now"
                                    ),
                                    true,
                                )
                                .await;
                            }
                        }
                        Ok(g) => {
                            self.drop_holding("the lease ran out and was granted again", false)
                                .await;
                            self.become_holder(relay, g.epoch).await;
                        }
                        Err(PoolError::Unavailable(e)) => {
                            // Keep coordinating while the relay is out of
                            // reach (appends stall meanwhile), and stop once
                            // the lease is surely over.
                            if last_ok.elapsed() >= self.cfg.lease_ttl {
                                self.drop_holding(&format!("cannot renew the lease: {e}"), false)
                                    .await;
                            }
                        }
                        Err(e) => self.drop_holding(&e.to_string(), false).await,
                    }
                }
                None => {
                    let want = priority >= PRIORITY_PROVIDER || self.wanted.load(Ordering::SeqCst);
                    // Only a machine that has the keys can read the log.
                    if !want || keys.as_ref().and_then(|k| k.cached()).is_none() {
                        continue;
                    }
                    match relay
                        .lease(0, &name, &machine, priority, self.cfg.lease_ttl)
                        .await
                    {
                        Ok(g) => {
                            last_ok = Instant::now();
                            backoff = Duration::ZERO;
                            self.wanted.store(false, Ordering::SeqCst);
                            self.become_holder(relay, g.epoch).await;
                        }
                        Err(PoolError::Unsupported(_)) => {
                            self.no_relay_pool.store(true, Ordering::SeqCst);
                        }
                        Err(PoolError::Refused { code, .. }) if code == "held" => {}
                        Err(e) => {
                            tracing::debug!("pool lease: {e}");
                            backoff = (backoff * 2)
                                .clamp(Duration::from_secs(1), Duration::from_secs(30));
                        }
                    }
                }
            }
        }
    }

    /// Whether this holder (at `priority`) hands the lease to a contender
    /// the relay says ranks above it: only to one at least as new (an older
    /// machine that provides Spaces never takes the pool from a newer
    /// laptop: it would drop what the newer one carries).
    async fn should_yield(&self, relay: &RelayPool, priority: u32) -> bool {
        match relay.status().await {
            Ok(st) => newer_contender_waits(
                &st,
                &self.host.name(),
                priority,
                self.cfg.lease_ttl.as_millis() as u64,
            ),
            Err(_) => false,
        }
    }

    /// Before a request that needs a newer coordinator (`what`, see
    /// [`level_needed`]) goes to the machine that coordinates: `Err` with
    /// the refusal when that machine is older. An older laptop hands the
    /// pool over to this machine (it contends meanwhile, bounded); an
    /// older machine that provides Spaces never does, so that is refused at
    /// once, naming it.
    async fn check_holder_level(&self, relay: &RelayPool, what: &str) -> Result<(), String> {
        let deadline = Instant::now() + self.cfg.lease_ttl * 3;
        loop {
            if self.holder().await.is_some() {
                return Ok(());
            }
            let Ok(st) = relay.status().await else {
                // The relay answers the request itself if it can.
                return Ok(());
            };
            let holder = &st["holder"];
            if holder.is_null() {
                return Ok(());
            }
            let (level, provider) = level_of(holder["priority"].as_u64().unwrap_or(0) as u32);
            if level >= POOL_LEVEL {
                // A newer machine took over (this one contended meanwhile).
                if Instant::now() > deadline - self.cfg.lease_ttl * 3 {
                    self.wanted.store(false, Ordering::SeqCst);
                }
                return Ok(());
            }
            let name = holder["name"].as_str().unwrap_or("(unnamed)").to_string();
            if provider || Instant::now() >= deadline {
                self.wanted.store(false, Ordering::SeqCst);
                return Err(too_old_message(&name, provider, what));
            }
            // Contend: the relay tells the older laptop to hand over.
            self.wanted.store(true, Ordering::SeqCst);
            self.changed.notify_waiters();
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn become_holder(self: &Arc<Self>, relay: RelayPool, epoch: u64) {
        let dir = self.dir.join("pool").join(relay_key(&relay.key()));
        let Some(keys) = self.keys_for(&relay).await else {
            let _ = relay.release(epoch).await;
            return;
        };
        if let Err(e) = keys.sync().await {
            tracing::warn!("taking over the pool: its keys: {e}");
            let _ = relay.release(epoch).await;
            return;
        }
        let log = SealedLog::new(relay.log(epoch), keys.clone());
        let started = Instant::now();
        let at = match journal::rebuild(&dir.join("cluster.db"), &relay.key(), &log).await {
            Ok(at) => at,
            Err(e) => {
                tracing::error!("taking over the pool: rebuilding from the log failed: {e}");
                let _ = relay.release(epoch).await;
                return;
            }
        };
        let links: Links = Arc::default();
        let elastic = self.host.elastic();
        // Reach the machines before the coordinator looks up its jobs on them,
        // within a third of the lease (this loop renews it): machines that
        // take longer are added by the next discovery round.
        let machines = tokio::time::timeout(self.cfg.lease_ttl / 3, self.host.account_machines())
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        let coord = match Coordinator::start_with(
            (self.cfg.coordinator)(&dir),
            Some(factory(&links, elastic.as_ref().map(|e| e.1.clone()))),
            Arc::new(log),
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("taking over the pool: {e}");
                let _ = relay.release(epoch).await;
                return;
            }
        };
        apply_machines(&coord, &links, machines, false).await;
        if let Some((row, _)) = elastic {
            let _ = coord.add_node(row).await;
        }
        tracing::info!(
            epoch,
            log = at,
            took_ms = started.elapsed().as_millis() as u64,
            "this machine coordinates the pool"
        );
        let stop = self.stop.child_token();
        *self.holding.lock().await = Some(Holding {
            epoch,
            relay: relay.clone(),
            coord: coord.clone(),
            stop: stop.clone(),
        });
        tokio::spawn(self.clone().serve_mailbox(
            relay.clone(),
            keys,
            epoch,
            coord.clone(),
            stop.clone(),
        ));
        tokio::spawn(
            self.clone()
                .discover_pool(coord.clone(), links, stop.clone()),
        );
        // A fenced coordinator stops itself: let go of it.
        let me = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = stop.cancelled() => {}
                _ = coord.stopped() => {
                    if coord.is_fenced() {
                        me.drop_epoch(epoch, "another machine coordinates now").await;
                    }
                }
            }
        });
    }

    async fn drop_epoch(&self, epoch: u64, why: &str) {
        let mut g = self.holding.lock().await;
        if g.as_ref().is_some_and(|h| h.epoch == epoch) {
            let h = g.take().expect("holding");
            drop(g);
            tracing::info!(epoch, "this machine stops coordinating the pool: {why}");
            h.stop.cancel();
            h.coord.stop();
        }
    }

    /// Another account signed in than the one this machine coordinates
    /// the pool of: its coordinator and job state are the old account's, so
    /// it stops at once (the lease is not released: `relay` is the new
    /// account's pool, and the old lease ends by itself). Returns whether
    /// it did.
    async fn drop_holding_of_another_account(&self, relay: &RelayPool) -> bool {
        let other = self
            .holding
            .lock()
            .await
            .as_ref()
            .is_some_and(|h| h.relay.key() != relay.key());
        if other {
            self.drop_holding("another account signed in", false).await;
        }
        other
    }

    async fn drop_holding(&self, why: &str, release: bool) {
        let h = self.holding.lock().await.take();
        if let Some(h) = h {
            tracing::info!(
                epoch = h.epoch,
                "this machine stops coordinating the pool: {why}"
            );
            h.stop.cancel();
            h.coord.stop();
            if release {
                let _ = h.relay.release(h.epoch).await;
            }
        }
    }

    async fn discover_pool(
        self: Arc<Self>,
        coord: Arc<Coordinator>,
        links: Links,
        stop: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(self.cfg.discover_every) => {}
            }
            match self.host.account_machines().await {
                Ok(m) => apply_machines(&coord, &links, m, false).await,
                Err(e) => tracing::debug!("listing the account's machines: {e}"),
            }
        }
    }

    async fn serve_mailbox(
        self: Arc<Self>,
        relay: RelayPool,
        keys: Arc<PoolKeys>,
        epoch: u64,
        coord: Arc<Coordinator>,
        stop: CancellationToken,
    ) {
        let mut fails = 0u32;
        loop {
            if stop.is_cancelled() {
                return;
            }
            let taken = tokio::select! {
                _ = stop.cancelled() => return,
                r = relay.take(epoch, Duration::from_secs(20)) => r,
            };
            match taken {
                Ok(reqs) => {
                    fails = 0;
                    for r in reqs {
                        let (relay, coord, keys) = (relay.clone(), coord.clone(), keys.clone());
                        tokio::spawn(async move {
                            let caller: Caller =
                                serde_json::from_value(r["caller"].clone()).unwrap_or_default();
                            let method = r["method"].as_str().unwrap_or_default().to_string();
                            let (rid, status, body) = match open_request(
                                &keys,
                                &method,
                                &caller.device,
                                &r["params"],
                            )
                            .await
                            {
                                Ok((rid, params)) => {
                                    let (status, body) =
                                        requests::answer(&coord, &method, params, &caller).await;
                                    (rid, status, body)
                                }
                                Err(e) => (
                                    String::new(),
                                    400,
                                    json!({"error": format!("the request could not be opened: {e}")}),
                                ),
                            };
                            if status >= 500 && coord.is_fenced() {
                                // Taken over meanwhile: the relay tells the
                                // client to retry with the next holder.
                                return;
                            }
                            for _ in 0..3 {
                                let sealed = match seal_reply(
                                    &keys,
                                    &method,
                                    &caller.device,
                                    &rid,
                                    status,
                                    &body,
                                )
                                .await
                                {
                                    Ok(s) => s,
                                    Err(e) => {
                                        tracing::warn!("sealing a pool answer: {e}");
                                        return;
                                    }
                                };
                                match relay
                                    .reply(
                                        epoch,
                                        vec![json!({"id": r["id"], "status": status, "body": sealed})],
                                    )
                                    .await
                                {
                                    Err(PoolError::Refused { code, .. }) if code == "rekey" => {
                                        let _ = keys.sync().await;
                                    }
                                    _ => return,
                                }
                            }
                        });
                    }
                }
                Err(PoolError::Refused { code, .. }) if code == "fenced" => {
                    self.drop_epoch(epoch, "another machine coordinates now")
                        .await;
                    return;
                }
                Err(e) => {
                    fails += 1;
                    tracing::debug!("pool mailbox: {e}");
                    tokio::time::sleep(Duration::from_millis(500 * u64::from(fails.min(10)))).await;
                }
            }
        }
    }
}

/// A request sealed for the coordinator, and the id its answer must name.
struct SealedRequest {
    params: Value,
    rid: String,
}

fn rpc_aad(fields: &[&str]) -> Vec<u8> {
    seal::aad("cua-pool-rpc/v1", fields)
}

/// Seals `params` of `method` from this device for whichever of the
/// account's devices coordinates (bound to the method and this device, so
/// the relay cannot move it to another; it adds the device as the caller).
async fn seal_request(
    keys: &PoolKeys,
    method: &str,
    params: &Value,
) -> Result<SealedRequest, KeyError> {
    let rid = hex::encode(rand::random::<[u8; 12]>());
    let device = keys.device();
    let plain = serde_json::to_vec(&json!({"rid": rid, "params": params})).unwrap_or_default();
    let env = keys
        .seal(
            &rpc_aad(&["request", keys.account(), method, &device]),
            &plain,
        )
        .await?;
    Ok(SealedRequest {
        params: json!({"sealed": env.to_value()}),
        rid,
    })
}

/// What the relay may read of a request: a submit's run, job count and
/// placement (`on`), for the account's audit. Nothing of what runs.
fn request_meta(method: &str, params: &Value) -> Value {
    if method != "submit" {
        return Value::Null;
    }
    let count = params["count"]
        .as_u64()
        .or_else(|| params["jobs"].as_array().map(|j| j.len() as u64))
        .unwrap_or(0);
    json!({"run": params["run"], "count": count, "on": params["on"]})
}

/// The coordinator side: opens a request the relay delivered from
/// `device`: `(rid, params)`.
async fn open_request(
    keys: &PoolKeys,
    method: &str,
    device: &str,
    params: &Value,
) -> Result<(String, Value), String> {
    let env = Envelope::parse(&params["sealed"]).ok_or("it is not sealed")?;
    let plain = keys
        .open(&rpc_aad(&["request", keys.account(), method, device]), &env)
        .await
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
    Ok((
        v["rid"].as_str().unwrap_or_default().to_string(),
        v["params"].clone(),
    ))
}

fn reply_aad(account: &str, method: &str, device: &str, rid: &str, status: u16) -> Vec<u8> {
    rpc_aad(&["reply", account, method, device, rid, &status.to_string()])
}

/// The coordinator side: seals an answer for the device that asked.
async fn seal_reply(
    keys: &PoolKeys,
    method: &str,
    device: &str,
    rid: &str,
    status: u16,
    body: &Value,
) -> Result<Value, KeyError> {
    let plain = serde_json::to_vec(body).unwrap_or_default();
    let env = keys
        .seal(
            &reply_aad(keys.account(), method, device, rid, status),
            &plain,
        )
        .await?;
    Ok(json!({"sealed": env.to_value()}))
}

/// Opens the coordinator's answer to this device's request `rid`.
async fn open_reply(
    keys: &PoolKeys,
    method: &str,
    rid: &str,
    status: u16,
    body: Value,
) -> (u16, Value) {
    let Some(env) = Envelope::parse(&body["sealed"]) else {
        return (
            502,
            json!({"error": "the answer did not come sealed from the coordinating machine"}),
        );
    };
    match keys
        .open(
            &reply_aad(keys.account(), method, &keys.device(), rid, status),
            &env,
        )
        .await
    {
        Ok(plain) => match serde_json::from_slice(&plain) {
            Ok(v) => (status, v),
            Err(e) => (502, json!({"error": format!("the answer: {e}")})),
        },
        Err(e) => (
            502,
            json!({"error": format!("the answer could not be opened: {e}")}),
        ),
    }
}

/// A file name for a relay URL.
fn relay_key(url: &str) -> String {
    use sha2::Digest;
    hex::encode(&sha2::Sha256::digest(url.trim_end_matches('/').as_bytes())[..8])
}

/// Brings a coordinator's machines in line with `machines`: new ones are
/// added (named after themselves, uniquely; a machine keeps the name it
/// had, matched by its relay id), changed ones updated, and ones no longer
/// listed drained (their jobs finish; nothing new goes there) and, still
/// unlisted the next time, removed (relay machines). `own`: also
/// keeps this machine's runner as listed.
async fn apply_machines(coord: &Coordinator, links: &Links, machines: Vec<PoolMachine>, own: bool) {
    let existing = coord.node_rows();
    let mut seen = Vec::new();
    for m in machines {
        let mut row = m.row;
        let prior = existing.iter().find(|r| {
            (!row.relay_machine.is_empty() && r.relay_machine == row.relay_machine)
                || (row.relay_machine.is_empty() && r.name == row.name)
        });
        row.name = match prior {
            Some(p) => p.name.clone(),
            None => {
                let wanted = crate::coordinator::sanitize_name(&row.name);
                let mut name = wanted.clone();
                let mut k = 2;
                while existing.iter().any(|r| r.name == name) || seen.contains(&name) {
                    name = format!("{wanted}-{k}");
                    k += 1;
                }
                name
            }
        };
        if row.state.is_empty() {
            row.state = "ready".into();
        }
        if row.added_ms == 0 {
            row.added_ms = prior.map_or_else(cua_cluster::model::now_ms, |p| p.added_ms);
        }
        seen.push(row.name.clone());
        let changed_link = {
            let mut l = links.lock().expect("links");
            let same = l
                .get(&row.name)
                .is_some_and(|old| old.describe() == m.link.describe());
            if !same {
                l.insert(row.name.clone(), m.link.clone());
            }
            !same
        };
        let unchanged = prior.is_some_and(|p| {
            p.via == row.via
                && p.provider == row.provider
                && p.os == row.os
                && p.state == row.state
                && p.relay_machine == row.relay_machine
        });
        if (changed_link || !unchanged || !coord.has_node(&row.name))
            && let Err(e) = coord.add_node(row).await
        {
            tracing::warn!("adding a machine to the pool: {e}");
        }
    }
    for r in existing {
        if seen.contains(&r.name) || r.via == "provider" || (own && r.name == LOCAL) {
            continue;
        }
        if r.state == "ready" {
            let _ = coord.drain(&r.name, true).await;
        } else if r.via == "relay" && r.state == "draining" {
            // Gone from the account at two listings in a row (removed,
            // or no longer providing Spaces): it leaves the pool, so its
            // name is free again and `cua machines` stops showing it.
            let _ = coord.remove_node(&r.name).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_cluster::link::LocalLink;
    use cua_cluster::node::{ExecEnd, ExecOutcome, JobExecutor, NodeConfig, NodeJobs, Offers};

    struct Idle;

    #[async_trait]
    impl JobExecutor for Idle {
        async fn run(
            &self,
            _job: &cua_cluster::model::JobSpec,
            _space: &str,
            _cancel: CancellationToken,
        ) -> ExecOutcome {
            ExecOutcome::ended(ExecEnd::Exited(0))
        }
        async fn remove_space(&self, _space: &str) {}
        fn offers(&self) -> Offers {
            Offers {
                kinds: vec!["container".into()],
                runtimes: vec![],
            }
        }
    }

    fn machine(dir: &Path, name: &str, id: &str) -> PoolMachine {
        let jobs = NodeJobs::open(
            NodeConfig {
                dir: dir.join(id),
                millicpus_total: 2000,
                memory_total_mb: 4096,
                jobs_max: 0,
                min_available_mb: 0,
                retention: Duration::from_secs(60),
                output_retention: Duration::from_secs(60),
                boot_id: "b".into(),
            },
            Arc::new(Idle),
        )
        .unwrap();
        PoolMachine {
            row: NodeRow {
                name: name.into(),
                relay_machine: id.into(),
                via: "relay".into(),
                os: "linux".into(),
                ..Default::default()
            },
            link: Arc::new(LocalLink::new(jobs, name, "linux")),
        }
    }

    struct NoHost;

    #[async_trait]
    impl Host for NoHost {
        async fn relay(&self) -> Option<RelayPool> {
            None
        }
        fn name(&self) -> String {
            "mini".into()
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

    struct NoAuth;

    #[async_trait]
    impl relay::PoolAuth for NoAuth {
        async fn headers(&self) -> Result<Vec<(String, String)>, String> {
            Err("not used".into())
        }
    }

    /// A machine that coordinates the pool of the account that was signed
    /// in stops when another account signs in, even if the new pool grants
    /// the same epoch number; the same account keeps it.
    #[tokio::test]
    async fn the_coordinator_of_the_previous_account_stops_when_another_signs_in() {
        let dir = tempfile::tempdir().unwrap();
        let agent = PoolAgent::start(
            dir.path().join("pool"),
            Arc::new(NoHost),
            AgentConfig::default(),
        )
        .await
        .unwrap();
        let auth: Arc<dyn relay::PoolAuth> = Arc::new(NoAuth);
        let pool_a = RelayPool::new("http://relay", "acct-a", auth.clone());
        let pool_b = RelayPool::new("http://relay", "acct-b", auth);
        let coord = Coordinator::start(crate::coordinator::Config::new(dir.path().join("c")), None)
            .await
            .unwrap();
        *agent.holding.lock().await = Some(Holding {
            epoch: 1,
            relay: pool_a.clone(),
            coord,
            stop: CancellationToken::new(),
        });
        assert!(!agent.drop_holding_of_another_account(&pool_a).await);
        assert!(agent.holder().await.is_some(), "the same account keeps it");
        assert!(agent.drop_holding_of_another_account(&pool_b).await);
        assert!(
            agent.holder().await.is_none(),
            "the other account's is gone"
        );
        agent.shutdown().await;
    }

    /// A machine removed from the account is drained at the first listing
    /// without it and leaves the pool at the next, so its name is free for
    /// a new machine (`cua cloud add aws --down`, then `cua cloud add aws`).
    #[tokio::test]
    async fn a_machine_gone_from_the_account_leaves_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let links: Links = Arc::default();
        let coord = Coordinator::start(
            crate::coordinator::Config::new(dir.path().join("c")),
            Some(factory(&links, None)),
        )
        .await
        .unwrap();
        apply_machines(
            &coord,
            &links,
            vec![machine(dir.path(), "aws-1", "m1")],
            false,
        )
        .await;
        assert!(coord.has_node("aws-1"));
        apply_machines(&coord, &links, vec![], false).await;
        let rows = coord.node_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, "draining");
        apply_machines(&coord, &links, vec![], false).await;
        assert!(coord.node_rows().is_empty(), "{:?}", coord.node_rows());
        // A new machine with the same name gets it.
        apply_machines(
            &coord,
            &links,
            vec![machine(dir.path(), "aws-1", "m2")],
            false,
        )
        .await;
        let rows = coord.node_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].name.as_str(), rows[0].relay_machine.as_str()),
            ("aws-1", "m2")
        );
        // A machine listed again after a drain is ready again, not removed.
        apply_machines(&coord, &links, vec![], false).await;
        apply_machines(
            &coord,
            &links,
            vec![machine(dir.path(), "aws-1", "m2")],
            false,
        )
        .await;
        assert_eq!(coord.node_rows()[0].state, "ready");
    }

    #[test]
    fn lease_levels_read_as_the_same_role_to_older_agents() {
        // Older agents compare against PRIORITY_PROVIDER only.
        assert!(lease_priority(true) >= PRIORITY_PROVIDER);
        assert!(lease_priority(false) < PRIORITY_PROVIDER);
        assert_eq!(level_of(lease_priority(true)), (POOL_LEVEL, true));
        assert_eq!(level_of(lease_priority(false)), (POOL_LEVEL, false));
        // The first pool agent's priorities are level 0.
        assert_eq!(level_of(PRIORITY_PROVIDER), (0, true));
        assert_eq!(level_of(PRIORITY_CLIENT), (0, false));
        // A newer laptop never outranks an older machine that provides Spaces.
        assert!(lease_priority(false) < PRIORITY_PROVIDER);
    }

    #[test]
    fn submits_with_services_or_a_cloud_pin_need_a_newer_coordinator() {
        assert!(level_needed("submit", &json!({"services": ["compat:e2b"]})).is_some());
        assert!(
            level_needed(
                "submit",
                &json!({"jobs": [{"services": ["compat:daytona"]}]})
            )
            .is_some()
        );
        assert!(level_needed("submit", &json!({"on": "cloud:hetzner"})).is_some());
        assert!(level_needed("submit", &json!({"command": ["true"], "on": "studio"})).is_none());
        assert!(level_needed("submit", &json!({"services": []})).is_none());
        assert!(level_needed("runs", &json!({"services": ["compat:e2b"]})).is_none());
    }

    #[test]
    fn a_holder_hands_over_only_to_a_newer_or_equal_machine_that_ranks_above() {
        let st = |name: &str, p: u32| json!({"contenders": [{"name": name, "priority": p, "seen_ms_ago": 100}]});
        let laptop = lease_priority(false);
        // A newer laptop does not hand the pool to an older provider.
        assert!(!newer_contender_waits(
            &st("old-box", PRIORITY_PROVIDER),
            "me",
            laptop,
            15_000
        ));
        // It does to a provider as new as it.
        assert!(newer_contender_waits(
            &st("box", lease_priority(true)),
            "me",
            laptop,
            15_000
        ));
        // Never to itself, a lower rank, or one not seen lately.
        assert!(!newer_contender_waits(
            &st("me", lease_priority(true)),
            "me",
            laptop,
            15_000
        ));
        assert!(!newer_contender_waits(
            &st("box", PRIORITY_CLIENT),
            "me",
            laptop,
            15_000
        ));
        let stale = json!({"contenders": [{"name": "box", "priority": 11, "seen_ms_ago": 20_000}]});
        assert!(!newer_contender_waits(&stale, "me", laptop, 15_000));
    }

    #[test]
    fn the_coordinator_view_says_who_and_whether_it_is_older() {
        let v = coordinator_view(&json!({
            "holder": {"name": "ip-172-31-43-135", "device": "dev_x", "priority": 1}
        }));
        assert_eq!(v["name"], "ip-172-31-43-135");
        assert_eq!(v["older"], true);
        assert_eq!(v["provides_spaces"], false);
        let v = coordinator_view(&json!({
            "holder": {"name": "box", "priority": lease_priority(true)},
            "this_machine_coordinates": 3
        }));
        assert_eq!(v["older"], false);
        assert_eq!(v["this_machine"], true);
        assert!(coordinator_view(&json!({"holder": null})).is_null());
        let m = too_old_message("old-mac", true, "jobs with services (cua run --compat)");
        assert!(
            m.contains("old-mac") && m.contains("update Cua on old-mac"),
            "{m}"
        );
    }
}
