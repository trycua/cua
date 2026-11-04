//! The reconciler: a pool's slots, each one Space and at most one runner.
//!
//! A slot goes `creating` (its Space is being created) → `starting` (the
//! runner is installed, registered with a JIT configuration and started)
//! → `idle` (online, waiting for a job) → `busy` (GitHub gave it a job) →
//! `ending` (its logs are copied out, its Space deleted and its runner
//! removed), and is then forgotten. A Space is never reused.
//!
//! Each [`Pool::tick`] (every few seconds under [`Pool::serve`]):
//!
//! 1. reads the Spaces and GitHub's runners (at most every 10 s);
//! 2. ends slots whose runner exited (the job is done), crashed, never
//!    started, never came online (`register_timeout`), whose Space is gone
//!    (the runner is removed at GitHub), that idled past `idle_timeout`
//!    (only when GitHub agrees the runner is not busy) or ran a job past
//!    `job_timeout`;
//! 3. at start and every 5 minutes removes orphans both ways: Spaces
//!    labelled `cua-ci=<pool>` that no slot owns (one whose runner is
//!    still registered is taken back instead), and runners named
//!    `<prefix>-*` that no slot owns;
//! 4. asks for new slots to keep `warm` idle runners, never more than
//!    `max_runners` Spaces (for macOS on one computer, never more than the
//!    two macOS VMs Apple allows, less the ones other Spaces use).
//!
//! New slots are provisioned concurrently ([`Pool::provision`]). A create
//! that fails backs off exponentially (5 s doubling, at most 5 minutes); a
//! computer at its capacity limit is not a failure: the pool waits 30 s and
//! asks again.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::TarballSource;
use crate::config::{
    GuestOs, HostResources, MACOS_VMS_PER_HOST, PoolConfig, SPACE_LABEL_POOL, SPACE_LABEL_SLOT,
};
use crate::egress::{self, EgressPolicy};
use crate::events::{Event, EventKind, EventLog, Summary};
use crate::github::{DeleteOutcome, JitRequest, Runner, RunnerApi};
use crate::guest::{self, GuestState};
use crate::paths::CiPaths;
use crate::perf::StartMode;
use crate::perf::runtime::{JobClaim, PerfRuntime, SlotPerf};
use crate::perf::timers::Timers;
use crate::perf::warm::Ledger;
use crate::spaces::{SpaceRow, SpaceSpec, Spaces, SpacesError};
use crate::{Error, Result};

/// How often [`Pool::serve`] ticks.
pub const TICK: Duration = Duration::from_secs(3);
/// GitHub's runner list is read at most this often.
const RUNNERS_TTL_MS: u64 = 10_000;
/// Orphans are looked for this often.
const SWEEP_MS: u64 = 5 * 60_000;
/// Copied logs past their retention are pruned this often.
const PRUNE_MS: u64 = 3_600_000;
/// The first backoff after a failure; it doubles up to [`BACKOFF_MAX_MS`].
const BACKOFF_BASE_MS: u64 = 5_000;
/// The longest backoff.
pub const BACKOFF_MAX_MS: u64 = 5 * 60_000;
/// A computer at its capacity limit is asked again after this long.
const CAPACITY_RETRY_MS: u64 = 30_000;
/// A started wrapper that has not written its pid by now never started.
const START_GRACE_MS: u64 = 120_000;
/// A guest that has not answered this many status checks in a row is lost.
const GUEST_UNREACHABLE_CHECKS: u32 = 20;
/// How long a Space create may take.
const CREATE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Guest commands.
const SH_TIMEOUT: Duration = Duration::from_secs(120);
/// A Space that cannot be deleted is tried again each tick this many times.
const MAX_DELETE_TRIES: u32 = 100;
/// Unpacking the runner.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// The time, in Unix ms (a manual clock in tests).
pub trait Clock: Send + Sync {
    /// Now.
    fn now_ms(&self) -> u64;
}

/// The system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        crate::unix_ms()
    }
}

/// What a pool runs on.
pub struct PoolDeps {
    /// Cua Spaces.
    pub spaces: Arc<dyn Spaces>,
    /// GitHub.
    pub github: Arc<dyn RunnerApi>,
    /// Runner tarballs for images without the runner.
    pub tarballs: Arc<dyn TarballSource>,
    /// The time.
    pub clock: Arc<dyn Clock>,
    /// This computer's cores and memory (sizes macOS runners on it).
    pub host: Option<HostResources>,
}

/// Where a slot is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Its Space is being created.
    Creating,
    /// The runner is being installed, registered and started.
    Starting,
    /// The runner waits for a job.
    Idle,
    /// The runner runs a job.
    Busy,
    /// It is being torn down.
    Ending,
    /// A prepared VM waits for a job, not registered with GitHub (the
    /// warm pool: a claim registers it).
    Warm,
}

impl Phase {
    /// `creating`, `starting`, `idle`, `busy`, `ending`.
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Creating => "creating",
            Phase::Starting => "starting",
            Phase::Idle => "idle",
            Phase::Busy => "busy",
            Phase::Ending => "ending",
            Phase::Warm => "warm",
        }
    }
}

/// One Space and its runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    /// Slot id (8 hex digits).
    pub id: String,
    /// The Space's and the runner's name (`<prefix>-<id>`).
    pub name: String,
    /// The Space, once created.
    #[serde(default)]
    pub space_id: Option<String>,
    /// The runner, once registered.
    #[serde(default)]
    pub runner_id: Option<u64>,
    /// Where it is.
    pub phase: Phase,
    /// Created (Unix ms).
    pub created_ms: u64,
    /// Space ready.
    #[serde(default)]
    pub ready_ms: Option<u64>,
    /// Runner started.
    #[serde(default)]
    pub registered_ms: Option<u64>,
    /// First seen online at GitHub.
    #[serde(default)]
    pub online_ms: Option<u64>,
    /// Job started (seen busy).
    #[serde(default)]
    pub busy_ms: Option<u64>,
    /// The guest user's home.
    #[serde(default)]
    pub home: Option<String>,
    /// `host` (enforced), `off`, or why it is not enforced.
    #[serde(default)]
    pub egress: Option<String>,
    /// Why it is ending.
    #[serde(default)]
    pub ending: Option<String>,
    /// The performance core's bookkeeping (golden image, start mode, the
    /// job it was claimed for).
    #[serde(default)]
    pub perf: SlotPerf,
}

/// What survives a restart: `state.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolState {
    /// The slots.
    #[serde(default)]
    pub slots: Vec<Slot>,
    /// Failures in a row.
    #[serde(default)]
    pub failures: u32,
    /// No create before this (Unix ms).
    #[serde(default)]
    pub next_create_ms: u64,
    /// The month's spend of a metered pool.
    #[serde(default)]
    pub ledger: Ledger,
}

impl PoolState {
    /// Reads `state.json` (empty when there is none).
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
}

/// Runtime-only bookkeeping.
#[derive(Default)]
struct Runtime {
    runners: Option<(u64, Vec<Runner>)>,
    last_sweep_ms: Option<u64>,
    last_prune_ms: u64,
    guest_errors: HashMap<String, u32>,
    capacity_note: Option<u32>,
    last_list_error: Option<String>,
    /// The actions runner version last installed (the bench's
    /// `runner_version`).
    runner_version: Option<String>,
}

/// A pool and its reconciler.
pub struct Pool {
    cfg: PoolConfig,
    paths: CiPaths,
    deps: PoolDeps,
    events: EventLog,
    state: std::sync::Mutex<PoolState>,
    rt: std::sync::Mutex<Runtime>,
    group: tokio::sync::OnceCell<u64>,
    seq: AtomicU64,
    /// The bench's phase events (always on: they are the event log).
    timers: Timers,
    /// The performance core, when the pool has one.
    perf: Option<Arc<PerfRuntime>>,
    /// Jobs come from the hosted service and GitHub is not reachable from
    /// here: the runner list is never read.
    hosted: bool,
}

impl Pool {
    /// Opens a pool: reads its state. Slots a previous process was still
    /// creating or starting are ended at the first tick.
    pub fn open(cfg: PoolConfig, paths: CiPaths, deps: PoolDeps) -> Result<Self> {
        cfg.validate()?;
        let mut st = PoolState::load(&paths.state(&cfg.name));
        for s in &mut st.slots {
            if matches!(s.phase, Phase::Creating | Phase::Starting) {
                s.phase = Phase::Ending;
                s.ending = Some("interrupted".into());
            }
        }
        let events = EventLog::new(paths.events(&cfg.name));
        let timers = Timers::new(events.clone());
        let pool = Self {
            cfg,
            paths,
            deps,
            events,
            state: std::sync::Mutex::new(st),
            rt: std::sync::Mutex::new(Runtime::default()),
            group: tokio::sync::OnceCell::new(),
            seq: AtomicU64::new(0),
            timers,
            perf: None,
            hosted: false,
        };
        pool.save()?;
        Ok(pool)
    }

    /// The configuration.
    pub fn config(&self) -> &PoolConfig {
        &self.cfg
    }

    /// The event log.
    pub fn events(&self) -> &EventLog {
        &self.events
    }

    /// The slots now.
    pub fn slots(&self) -> Vec<Slot> {
        self.state.lock().expect("state").slots.clone()
    }

    /// The persisted state now.
    pub fn state(&self) -> PoolState {
        self.state.lock().expect("state").clone()
    }

    fn now(&self) -> u64 {
        self.deps.clock.now_ms()
    }

    fn record(&self, e: Event) {
        self.events.record(&e);
    }

    fn save(&self) -> Result<()> {
        let st = self.state.lock().expect("state").clone();
        let path = self.paths.state(&self.cfg.name);
        cua_home::guard_write(&path)?;
        let body =
            serde_json::to_vec_pretty(&st).map_err(|e| Error::Io(std::io::Error::other(e)))?;
        cua_home::write_private(&path, &body)?;
        Ok(())
    }

    fn save_logged(&self) {
        if let Err(e) = self.save() {
            tracing::warn!("saving the pool state: {e}");
        }
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Slot)) -> Option<Slot> {
        let out = {
            let mut st = self.state.lock().expect("state");
            st.slots.iter_mut().find(|s| s.id == id).map(|s| {
                f(s);
                s.clone()
            })
        };
        self.save_logged();
        out
    }

    fn slot(&self, id: &str) -> Option<Slot> {
        self.state
            .lock()
            .expect("state")
            .slots
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    fn remove_slot(&self, id: &str) {
        self.state
            .lock()
            .expect("state")
            .slots
            .retain(|s| s.id != id);
        self.rt.lock().expect("rt").guest_errors.remove(id);
        self.save_logged();
    }

    fn new_slot_id(&self) -> String {
        use rand::Rng as _;
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let r: u32 = rand::rng().random();
        format!("{:08x}", r ^ (n as u32).rotate_left(16))
    }

    async fn runners(&self, force: bool) -> Option<Vec<Runner>> {
        if self.hosted {
            return None;
        }
        let now = self.now();
        if !force
            && let Some((at, r)) = &self.rt.lock().expect("rt").runners
            && now.saturating_sub(*at) < RUNNERS_TTL_MS
        {
            return Some(r.clone());
        }
        match self.deps.github.list_runners().await {
            Ok(r) => {
                let mut rt = self.rt.lock().expect("rt");
                rt.runners = Some((now, r.clone()));
                rt.last_list_error = None;
                Some(r)
            }
            Err(e) => {
                let msg = e.to_string();
                let new = {
                    let mut rt = self.rt.lock().expect("rt");
                    let new = rt.last_list_error.as_deref() != Some(&msg);
                    rt.last_list_error = Some(msg.clone());
                    new
                };
                if new {
                    self.record(Event::new(now, EventKind::Error).detail(msg));
                }
                self.rt
                    .lock()
                    .expect("rt")
                    .runners
                    .as_ref()
                    .map(|(_, r)| r.clone())
            }
        }
    }

    fn ours(&self, row: &SpaceRow) -> bool {
        row.label(SPACE_LABEL_POOL) == Some(self.cfg.name.as_str())
    }

    fn runner_is_ours(&self, r: &Runner) -> bool {
        r.name.starts_with(&format!("{}-", self.cfg.prefix()))
    }

    /// One reconcile pass. Returns the slots to provision (what
    /// [`Pool::serve`] starts concurrently).
    pub async fn tick(&self) -> Vec<String> {
        // Slots a previous pass or process left half torn down.
        let pending: Vec<Slot> = self
            .slots()
            .into_iter()
            .filter(|s| s.phase == Phase::Ending)
            .collect();
        for s in pending {
            let why = s.ending.clone().unwrap_or_else(|| "interrupted".into());
            self.finish(&s.id, &why, None).await;
        }

        let spaces = match self.deps.spaces.list().await {
            Ok(l) => Some(l),
            Err(e) => {
                self.record(
                    Event::new(self.now(), EventKind::Error).detail(format!("listing Spaces: {e}")),
                );
                None
            }
        };
        let runners = self.runners(false).await;

        // Spaces that went away under a live slot.
        if let Some(list) = &spaces {
            let live: Vec<Slot> = self
                .slots()
                .into_iter()
                .filter(|s| matches!(s.phase, Phase::Idle | Phase::Busy | Phase::Warm))
                .collect();
            for s in live {
                let present = s
                    .space_id
                    .as_ref()
                    .is_some_and(|id| list.iter().any(|r| &r.id == id));
                if !present {
                    self.record(
                        Event::new(self.now(), EventKind::SpaceLost)
                            .slot(&s.id)
                            .space(s.space_id.as_deref())
                            .runner(Some(&s.name), s.runner_id),
                    );
                    self.finish(&s.id, "space_lost", None).await;
                }
            }
        }

        // Each runner's process and GitHub's view of it.
        let live: Vec<Slot> = self
            .slots()
            .into_iter()
            .filter(|s| matches!(s.phase, Phase::Idle | Phase::Busy))
            .collect();
        for s in live {
            self.check_slot(&s, runners.as_deref()).await;
        }
        // Warm VMs: still there, still answering, not too old.
        self.check_warm().await;

        // Orphans, at start and every few minutes.
        let now = self.now();
        let sweep = {
            let rt = self.rt.lock().expect("rt");
            rt.last_sweep_ms
                .is_none_or(|t| now.saturating_sub(t) >= SWEEP_MS)
        };
        if sweep && let Some(list) = &spaces {
            self.sweep(list, false).await;
            self.rt.lock().expect("rt").last_sweep_ms = Some(now);
        }

        // Old copied logs.
        let prune = {
            let rt = self.rt.lock().expect("rt");
            now.saturating_sub(rt.last_prune_ms) >= PRUNE_MS
        };
        if prune {
            self.prune_logs(now);
            self.rt.lock().expect("rt").last_prune_ms = now;
        }

        let Some(list) = &spaces else { return vec![] };
        let local_macos = self.cfg.guest_os() == GuestOs::Macos && self.cfg.on.trim() == "local";
        let lume = if local_macos {
            self.deps.spaces.macos_vms_running().await
        } else {
            None
        };
        if self.claim_mode() {
            return self.scale_up_warm(list, lume).await;
        }
        self.scale_up(list, lume)
    }

    /// The most VMs this pool may run now: `max_runners`, and for macOS on
    /// this Mac Apple's two less the macOS VMs that are not the pool's
    /// (other Spaces, Lume's own, another tool's). `lume`: every macOS VM
    /// running on this Mac, Spaces or not.
    pub(crate) fn cap_for(&self, spaces: &[SpaceRow], lume: Option<u32>) -> u32 {
        let now = self.now();
        let mut cap = self.cfg.effective_max();
        if self.cfg.guest_os() == GuestOs::Macos && self.cfg.on.trim() == "local" {
            // Other macOS VMs: other Spaces, and VMs that are not Spaces at
            // all (Lume's own, another tool's), whichever count is higher.
            let other_spaces = spaces
                .iter()
                .filter(|r| r.is_local_macos_vm() && !self.ours(r))
                .count() as u32;
            let ours = spaces
                .iter()
                .filter(|r| r.is_local_macos_vm() && self.ours(r))
                .count() as u32;
            let others = other_spaces.max(lume.unwrap_or(0).saturating_sub(ours));
            let budget = MACOS_VMS_PER_HOST.saturating_sub(others);
            if budget < cap {
                let changed = {
                    let mut rt = self.rt.lock().expect("rt");
                    let changed = rt.capacity_note != Some(others);
                    rt.capacity_note = Some(others);
                    changed
                };
                if changed {
                    self.record(Event::new(now, EventKind::CapacityWait).detail(format!(
                        "{others} other macOS VM{} run on this Mac; Apple allows {MACOS_VMS_PER_HOST}, \
                         so the pool uses at most {budget}",
                        if others == 1 { "" } else { "s" }
                    )));
                }
            } else {
                self.rt.lock().expect("rt").capacity_note = None;
            }
            cap = cap.min(budget);
        }
        cap
    }

    /// `lume`: every macOS VM running on this Mac, Spaces or not.
    fn scale_up(&self, spaces: &[SpaceRow], lume: Option<u32>) -> Vec<String> {
        let now = self.now();
        let cap = self.cap_for(spaces, lume);
        let warm = self.cfg.effective_warm();
        let mut ids = Vec::new();
        {
            let mut st = self.state.lock().expect("state");
            if now < st.next_create_ms {
                return vec![];
            }
            let live: Vec<&Slot> = st
                .slots
                .iter()
                .filter(|s| s.phase != Phase::Ending)
                .collect();
            let total = live.len() as u32;
            let waiting = live
                .iter()
                .filter(|s| matches!(s.phase, Phase::Creating | Phase::Starting | Phase::Idle))
                .count() as u32;
            let want = warm.saturating_sub(waiting).min(cap.saturating_sub(total));
            for _ in 0..want {
                let id = self.new_slot_id();
                st.slots.push(Slot {
                    name: format!("{}-{id}", self.cfg.prefix()),
                    id: id.clone(),
                    space_id: None,
                    runner_id: None,
                    phase: Phase::Creating,
                    created_ms: now,
                    ready_ms: None,
                    registered_ms: None,
                    online_ms: None,
                    busy_ms: None,
                    home: None,
                    egress: None,
                    ending: None,
                    perf: SlotPerf::default(),
                });
                ids.push(id);
            }
        }
        if !ids.is_empty() {
            self.save_logged();
            for id in &ids {
                let name = self.slot(id).map(|s| s.name).unwrap_or_default();
                self.record(
                    Event::new(now, EventKind::SlotCreated)
                        .slot(id)
                        .runner(Some(&name), None),
                );
            }
        }
        ids
    }

    async fn check_slot(&self, s: &Slot, runners: Option<&[Runner]>) {
        let Some(space) = s.space_id.clone() else {
            return;
        };
        let home = match &s.home {
            Some(h) => h.clone(),
            None => match self.probe(&space).await {
                Ok(p) => {
                    self.update(&s.id, |x| x.home = Some(p.home.clone()));
                    p.home
                }
                Err(_) => {
                    self.guest_error(s).await;
                    return;
                }
            },
        };
        let out = self
            .deps
            .spaces
            .sh(
                &space,
                &guest::status_script(&home),
                Duration::from_secs(30),
            )
            .await;
        let (state, assigned) = match out {
            Ok(o) if o.ok() => {
                self.rt.lock().expect("rt").guest_errors.remove(&s.id);
                (
                    guest::parse_status(&o.stdout),
                    guest::parse_assigned(&o.stdout),
                )
            }
            _ => {
                self.guest_error(s).await;
                return;
            }
        };
        let now = self.now();
        let since_start = now.saturating_sub(s.registered_ms.unwrap_or(s.created_ms));
        match state {
            GuestState::Exited(code) => {
                let why = if code == 0 {
                    "job_done"
                } else {
                    "runner_exited"
                };
                self.finish(&s.id, why, Some(code)).await;
                return;
            }
            GuestState::Gone => {
                self.finish(&s.id, "runner_crashed", None).await;
                return;
            }
            GuestState::Starting if since_start > START_GRACE_MS => {
                self.finish(&s.id, "runner_never_started", None).await;
                return;
            }
            GuestState::Starting | GuestState::Running => {}
        }
        let runner = runners.and_then(|rs| {
            s.runner_id
                .and_then(|id| rs.iter().find(|r| r.id == id))
                .cloned()
        });
        let mut s = s.clone();
        // The runner's own console log says when it was given the job: more
        // exact, and sooner, than GitHub's runner list (read every 10 s).
        if s.phase == Phase::Idle
            && let Some(a) = assigned
            && let Some(at) = crate::perf::timers::parse_iso_ms(&a)
        {
            let host_at = (at as i64 - s.perf.guest_offset_ms.unwrap_or(0)).max(0) as u64;
            s = self.mark_busy(&s, host_at.min(now));
        }
        if let Some(r) = &runner {
            if r.status == "online" && s.online_ms.is_none() {
                s = self.update(&s.id, |x| x.online_ms = Some(now)).unwrap_or(s);
            }
            if r.busy && s.phase == Phase::Idle {
                s = self.mark_busy(&s, now);
            }
        }
        match s.phase {
            Phase::Idle => {
                // Only on GitHub's word: an unreachable API is no reason
                // to replace a runner.
                if runners.is_some()
                    && s.online_ms.is_none()
                    && since_start > self.cfg.register_timeout().as_millis() as u64
                {
                    self.finish(&s.id, "register_timeout", None).await;
                } else if since_start > self.cfg.idle_timeout().as_millis() as u64 {
                    // Only a runner GitHub agrees is idle: a job may have
                    // just been assigned.
                    let id = s.runner_id.unwrap_or_default();
                    match self.deps.github.delete_runner(id).await {
                        Ok(DeleteOutcome::Busy) => {
                            self.mark_busy(&s, now);
                        }
                        Ok(_) => {
                            self.update(&s.id, |x| x.runner_id = None);
                            self.finish(&s.id, "idle_recycled", None).await;
                        }
                        Err(e) => self.record(
                            Event::new(now, EventKind::Error)
                                .slot(&s.id)
                                .detail(format!("removing idle runner {}: {e}", s.name)),
                        ),
                    }
                }
            }
            Phase::Busy => {
                let busy_for = now.saturating_sub(s.busy_ms.unwrap_or(now));
                if busy_for > self.cfg.job_timeout().as_millis() as u64 {
                    self.finish(&s.id, "job_timeout", None).await;
                }
            }
            _ => {}
        }
    }

    fn mark_busy(&self, s: &Slot, now: u64) -> Slot {
        let wait = now.saturating_sub(s.registered_ms.unwrap_or(now));
        self.timers.job_assigned(
            Event::new(now, EventKind::JobAssigned)
                .slot(&s.id)
                .space(s.space_id.as_deref())
                .runner(Some(&s.name), s.runner_id)
                .ms(wait),
            &s.id,
            s.perf.job.as_ref().map(|j| j.fork),
        );
        self.update(&s.id, |x| {
            x.phase = Phase::Busy;
            x.busy_ms = Some(now);
        })
        .unwrap_or_else(|| s.clone())
    }

    async fn guest_error(&self, s: &Slot) {
        let n = {
            let mut rt = self.rt.lock().expect("rt");
            let n = rt.guest_errors.entry(s.id.clone()).or_insert(0);
            *n += 1;
            *n
        };
        if n >= GUEST_UNREACHABLE_CHECKS {
            self.finish(&s.id, "guest_unreachable", None).await;
        }
    }

    async fn probe(&self, space: &str) -> Result<guest::Probe> {
        let o = self.deps.spaces.sh(space, guest::PROBE, SH_TIMEOUT).await?;
        if !o.ok() {
            return Err(Error::Guest(format!(
                "probing the Space failed: {}",
                o.stderr.trim()
            )));
        }
        guest::parse_probe(&o.stdout).map_err(Error::Guest)
    }

    /// Removes orphans: labelled Spaces no slot owns (taking back the ones
    /// whose runner is still registered), and runners of the pool no slot
    /// owns. `all`: everything of the pool, slots or not (`remove`).
    async fn sweep(&self, spaces: &[SpaceRow], all: bool) {
        let now = self.now();
        let runners = self.runners(true).await;
        let slots = self.slots();
        let owned_slot = |row: &SpaceRow| {
            row.label(SPACE_LABEL_SLOT)
                .is_some_and(|l| slots.iter().any(|s| s.id == l))
        };
        let mut adopted_runners: Vec<u64> = Vec::new();
        for row in spaces.iter().filter(|r| self.ours(r)) {
            if !all && owned_slot(row) {
                continue;
            }
            // A Space whose runner is still registered lost its slot (the
            // state file): take it back rather than kill a job.
            let runner = runners
                .as_ref()
                .and_then(|rs| rs.iter().find(|r| r.name == row.name))
                .cloned();
            if !all && let (Some(r), Some(slot_id)) = (runner, row.label(SPACE_LABEL_SLOT)) {
                let slot = Slot {
                    id: slot_id.to_string(),
                    name: row.name.clone(),
                    space_id: Some(row.id.clone()),
                    runner_id: Some(r.id),
                    phase: if r.busy { Phase::Busy } else { Phase::Idle },
                    created_ms: now,
                    ready_ms: Some(now),
                    registered_ms: Some(now),
                    online_ms: (r.status == "online").then_some(now),
                    busy_ms: r.busy.then_some(now),
                    home: None,
                    egress: None,
                    ending: None,
                    perf: SlotPerf::default(),
                };
                adopted_runners.push(r.id);
                self.state.lock().expect("state").slots.push(slot);
                self.save_logged();
                self.record(
                    Event::new(now, EventKind::Adopted)
                        .slot(slot_id)
                        .space(Some(&row.id))
                        .runner(Some(&row.name), Some(r.id)),
                );
                continue;
            }
            match self.deps.spaces.delete(&row.id).await {
                Ok(()) | Err(SpacesError::NotFound(_)) => self.record(
                    Event::new(now, EventKind::OrphanSpaceDeleted)
                        .space(Some(&row.id))
                        .runner(Some(&row.name), None),
                ),
                Err(e) => self.record(
                    Event::new(now, EventKind::Error)
                        .space(Some(&row.id))
                        .detail(format!("deleting orphan Space {}: {e}", row.name)),
                ),
            }
        }
        let Some(runners) = runners else { return };
        let slots = self.slots();
        for r in runners.iter().filter(|r| self.runner_is_ours(r)) {
            let owned =
                slots.iter().any(|s| s.runner_id == Some(r.id)) || adopted_runners.contains(&r.id);
            if owned && !all {
                continue;
            }
            match self.deps.github.delete_runner(r.id).await {
                Ok(DeleteOutcome::Deleted) => self.record(
                    Event::new(now, EventKind::OrphanRunnerDeleted)
                        .runner(Some(&r.name), Some(r.id)),
                ),
                Ok(DeleteOutcome::NotFound) => {}
                Ok(DeleteOutcome::Busy) => self.record(
                    Event::new(now, EventKind::Error)
                        .runner(Some(&r.name), Some(r.id))
                        .detail(format!(
                            "runner {} has no Space but GitHub still lists it as busy; removing \
                             it again later",
                            r.name
                        )),
                ),
                Err(e) => self.record(
                    Event::new(now, EventKind::Error)
                        .runner(Some(&r.name), Some(r.id))
                        .detail(format!("removing orphan runner {}: {e}", r.name)),
                ),
            }
        }
    }

    fn prune_logs(&self, now: u64) {
        let keep = Duration::from_secs(u64::from(self.cfg.log_retention_days) * 86_400);
        let dir = self.paths.logs(&self.cfg.name);
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|t| {
                    now.saturating_sub(t.as_millis() as u64) > keep.as_millis() as u64
                });
            if old && cua_home::guard_write(&e.path()).is_ok() {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    fn failure(&self, slot: &str, why: String) {
        let now = self.now();
        let delay = {
            let mut st = self.state.lock().expect("state");
            st.failures = st.failures.saturating_add(1);
            let shift = st.failures.saturating_sub(1).min(16);
            let delay = BACKOFF_BASE_MS
                .saturating_mul(1 << shift)
                .min(BACKOFF_MAX_MS);
            st.next_create_ms = now + delay;
            delay
        };
        self.save_logged();
        self.record(
            Event::new(now, EventKind::CreateFailed)
                .slot(slot)
                .detail(why)
                .ms(delay),
        );
    }

    /// Creates a slot's Space and starts its runner. On failure the slot
    /// is torn down and the pool backs off.
    pub async fn provision(&self, id: &str) {
        self.provision_with(id, None).await;
    }

    /// The Space cannot enforce the egress policy: a fork pull request is
    /// given back and a pool with `require_egress` fails, both before any
    /// Space exists (`None`; the slot is already gone). Otherwise the pool
    /// says so and goes on without a policy (the note for the slot's events).
    fn egress_unenforced(&self, id: &str, job: Option<&JobClaim>, why: &str) -> Option<String> {
        if job.is_some_and(|j| j.require_egress) {
            // A fork pull request never runs unprotected: the job is given
            // back (the claim says why), and the pool does not back off for a
            // policy.
            self.remove_slot(id);
            self.job_unserved(job, "egress_unenforceable");
            return None;
        }
        if self.cfg.require_egress {
            let why = format!(
                "require_egress is set and this Space cannot enforce an egress policy: {why}"
            );
            self.remove_slot(id);
            self.failure(id, why);
            self.job_unserved(job, "egress_unenforceable");
            return None;
        }
        self.record(
            Event::new(self.now(), EventKind::EgressUnenforced)
                .slot(id)
                .detail(why.to_string()),
        );
        Some(format!("not enforced: {why}"))
    }

    /// [`Pool::provision`] for a job that is waiting (a claim with no warm
    /// VM free): the runner is registered for `job` as soon as the VM is
    /// ready. Returns whether the slot is on its way (`false`: it failed
    /// and was torn down).
    pub(crate) async fn provision_with(&self, id: &str, job: Option<&JobClaim>) -> bool {
        let Some(slot) = self.slot(id) else {
            return false;
        };
        let (cpus, memory_mb) = self.cfg.space_size(self.deps.host);
        let policy = egress::pool_policy(
            self.cfg.egress,
            &self.cfg.egress_deny,
            &self.cfg.egress_allow,
        );
        let mut spec = SpaceSpec {
            name: slot.name.clone(),
            image: self.cfg.image.clone(),
            on: self.cfg.on.clone(),
            cpus,
            memory_mb,
            disk_gb: self.cfg.disk_gb,
            labels: BTreeMap::from([
                (SPACE_LABEL_POOL.to_string(), self.cfg.name.clone()),
                (SPACE_LABEL_SLOT.to_string(), slot.id.clone()),
            ]),
            egress: policy.clone(),
            timeout: CREATE_TIMEOUT,
            create_id: format!("cua-ci:{}:{}", self.cfg.name, slot.id),
            mounts: self.slot_mounts(id),
        };
        let mut egress_note = if policy.is_some() {
            "host".to_string()
        } else {
            "off".to_string()
        };
        // A fork of the golden checkpoint when there is one (the checkpoint
        // sets the size); a cold create from the image otherwise.
        let mut fork = self.fork_choice(id);
        let sized = (spec.cpus, spec.memory_mb, spec.disk_gb, spec.image.clone());
        // A Space started from a checkpoint keeps its saved state, so it takes
        // no egress policy. On macOS nothing can enforce one anyway (the same
        // "not enforced" path as a cold create). A Linux fork with a policy to
        // keep starts cold instead, so the policy holds.
        let guest_macos = self.cfg.guest_os() == GuestOs::Macos;
        if spec.egress.is_some() && fork.is_some() && !guest_macos {
            fork = None;
        }
        if let Some(f) = &fork {
            spec.image = f.image.clone();
            spec.cpus = None;
            spec.memory_mb = None;
            spec.disk_gb = None;
        }
        if spec.egress.is_some() && spec.image.starts_with("checkpoint:") {
            let why = "a Space started from a checkpoint keeps its saved state and takes no \
                       egress policy";
            match self.egress_unenforced(id, job, why) {
                Some(note) => egress_note = note,
                None => return false,
            }
            spec.egress = None;
        }
        let mut created = self.deps.spaces.create(&spec).await;
        if let (Some(f), Err(e)) = (&fork, &created)
            && !matches!(
                e,
                SpacesError::LimitExceeded(_) | SpacesError::EgressUnsupported(_)
            )
        {
            // A fork that cannot start is not a reason to fail the job:
            // start it cold and have the golden image rebuilt.
            self.golden_fork_failed(id, f, &e.to_string());
            (spec.cpus, spec.memory_mb, spec.disk_gb, spec.image) = sized.clone();
            created = self.deps.spaces.create(&spec).await;
        }
        if let Err(SpacesError::EgressUnsupported(why)) = &created {
            match self.egress_unenforced(id, job, why) {
                Some(note) => egress_note = note,
                None => return false,
            }
            spec.egress = None;
            created = self.deps.spaces.create(&spec).await;
        }
        let mode = if created.is_ok() && spec.image.starts_with("checkpoint:") {
            StartMode::Fork
        } else {
            StartMode::ColdCreate
        };
        let row = match created {
            Ok(row) => row,
            Err(SpacesError::LimitExceeded(m)) => {
                let now = self.now();
                self.remove_slot(id);
                {
                    let mut st = self.state.lock().expect("state");
                    st.next_create_ms = st.next_create_ms.max(now + CAPACITY_RETRY_MS);
                }
                self.save_logged();
                self.record(Event::new(now, EventKind::CapacityWait).slot(id).detail(m));
                self.job_unserved(job, "capacity");
                return false;
            }
            Err(e) => {
                self.remove_slot(id);
                self.failure(id, format!("creating Space {}: {e}", slot.name));
                self.timers.infra_failure(
                    self.now(),
                    None,
                    &format!("creating Space {}: {e}", slot.name),
                );
                self.job_unserved(job, "create_failed");
                return false;
            }
        };
        let now = self.now();
        let applied = spec.egress.clone();
        let image_key = fork
            .as_ref()
            .filter(|_| mode == StartMode::Fork)
            .map(|f| f.key.clone());
        let variant = fork
            .as_ref()
            .filter(|_| mode == StartMode::Fork)
            .map(|f| f.variant);
        self.update(id, |s| {
            s.space_id = Some(row.id.clone());
            s.ready_ms = Some(now);
            s.phase = Phase::Starting;
            s.egress = Some(egress_note.clone());
            s.perf.start_mode = Some(mode);
            s.perf.image_key = image_key.clone();
            s.perf.variant = variant;
        });
        self.record(
            Event::new(now, EventKind::SpaceReady)
                .slot(id)
                .space(Some(&row.id))
                .ms(now.saturating_sub(slot.created_ms)),
        );
        if mode == StartMode::Fork {
            // A restored VM wakes up in the past with its old network lease.
            let fix =
                crate::perf::golden::fork_fixup_script(self.now() / 1000, self.cfg.guest_os());
            if let Err(e) = self.deps.spaces.sh(&row.id, &fix, SH_TIMEOUT).await {
                self.failure(id, format!("preparing the fork {}: {e}", slot.name));
                self.finish(id, "start_failed", None).await;
                return false;
            }
        }
        match self.start_runner_for(id, &row, applied, job).await {
            Ok(()) => {
                self.state.lock().expect("state").failures = 0;
                self.save_logged();
                if mode == StartMode::Fork {
                    self.golden_fork_ok();
                }
                true
            }
            Err(e) => {
                self.failure(id, format!("starting the runner in {}: {e}", slot.name));
                self.timers.infra_failure(
                    self.now(),
                    Some(id),
                    &format!("starting the runner in {}: {e}", slot.name),
                );
                self.finish(id, "start_failed", None).await;
                false
            }
        }
    }

    async fn start_runner_for(
        &self,
        id: &str,
        row: &SpaceRow,
        applied: Option<EgressPolicy>,
        job: Option<&JobClaim>,
    ) -> Result<()> {
        let space = row.id.as_str();
        let probe = self.probe(space).await?;
        let home = probe.home.clone();
        let host_now = self.now() as i64;
        self.update(id, |s| {
            s.home = Some(home.clone());
            s.perf.guest_offset_ms = probe.clock_ms.map(|c| c as i64 - host_now);
        });

        // DNS: allow exactly the resolver the guest uses when the policy
        // blocks it (before any job code runs).
        if let Some(policy) = applied {
            let allow = egress::resolver_allowances(&probe.resolv, &policy);
            if !allow.is_empty() {
                let mut p = policy.clone();
                p.allow.extend(allow.iter().cloned());
                self.deps.spaces.set_egress(space, &p).await?;
            }
            self.record(
                Event::new(self.now(), EventKind::EgressApplied)
                    .slot(id)
                    .space(Some(space))
                    .detail(if allow.is_empty() {
                        "private networks denied".to_string()
                    } else {
                        format!(
                            "private networks denied; resolver allowed: {}",
                            allow.join(", ")
                        )
                    }),
            );
        }

        if !probe.runner_present {
            let t0 = self.now();
            let platform = guest::platform(&probe.os, &probe.arch).ok_or_else(|| {
                Error::Guest(format!("no actions runner for {} {}", probe.os, probe.arch))
            })?;
            let tarball = self.deps.tarballs.tarball(platform).await?;
            self.rt.lock().expect("rt").runner_version = Some(tarball.version.clone());
            self.deps
                .spaces
                .upload_file(space, &tarball.path, &guest::tarball_path(&home))
                .await?;
            let o = self
                .deps
                .spaces
                .sh(space, &guest::install_script(&home), INSTALL_TIMEOUT)
                .await?;
            if !o.ok() {
                return Err(Error::Guest(format!(
                    "unpacking the runner failed: {}",
                    o.stderr.trim()
                )));
            }
            self.record(
                Event::new(self.now(), EventKind::RunnerInstalled)
                    .slot(id)
                    .space(Some(space))
                    .ms(self.now().saturating_sub(t0))
                    .detail(format!("actions/runner {} ({platform})", tarball.version)),
            );
        }

        self.deps
            .spaces
            .write_file(
                space,
                &guest::wrapper_path(&home),
                guest::wrapper_script().as_bytes(),
                0o755,
            )
            .await?;
        let o = self
            .deps
            .spaces
            .sh(space, &guest::prepare_script(&home), SH_TIMEOUT)
            .await?;
        if !o.ok() {
            return Err(Error::Guest(format!(
                "preparing the runner failed: {}",
                o.stderr.trim()
            )));
        }

        // Warm pool: the VM is ready and prepared; a claim registers it.
        if self.claim_mode() && job.is_none() {
            self.warm_ready(id);
            return Ok(());
        }
        self.register_runner(id, space, &home, &probe.arch, job)
            .await
    }

    /// The runner version in the VMs: the one installed last, else the
    /// golden image's.
    fn runner_version(&self) -> Option<String> {
        self.rt
            .lock()
            .expect("rt")
            .runner_version
            .clone()
            .or_else(|| {
                let p = self.perf.as_ref()?;
                let g = p.golden_spec.lock().ok()?;
                g.as_ref().map(|g| g.runner_version.clone())
            })
    }

    /// Registers the runner and starts it: the part of a start that needs
    /// GitHub. With a job (a claim) the job's files (caches, mirror,
    /// `job.env`) are delivered while the JIT configuration is minted.
    pub(crate) async fn register_runner(
        &self,
        id: &str,
        space: &str,
        home: &str,
        arch: &str,
        job: Option<&JobClaim>,
    ) -> Result<()> {
        if job.is_some() {
            let version = self.runner_version();
            self.timers.vm_ready(self.now(), id, version.as_deref());
        }
        let name = self.slot(id).map(|s| s.name).unwrap_or_default();
        let jit = match job {
            Some(j) => {
                let (delivered, jit) = tokio::join!(
                    self.deliver_job_files(id, space, home, j),
                    self.mint_jit(id, &name, arch, Some(j))
                );
                if let Err(e) = delivered {
                    // The job still runs: without caches and mirror it only
                    // takes longer. Say so.
                    self.record(
                        Event::new(self.now(), EventKind::Error)
                            .slot(id)
                            .detail(format!("delivering the job's caches failed: {e}")),
                    );
                }
                jit?
            }
            None => self.mint_jit(id, &name, arch, None).await?,
        };
        // The account turned the egress policy off for this job: the VM's
        // policy is lifted before any job code runs.
        if job.is_some_and(|j| j.egress_off)
            && self.slot(id).and_then(|s| s.egress).as_deref() == Some("host")
        {
            self.deps
                .spaces
                .set_egress(space, &EgressPolicy::default())
                .await?;
        }
        // Kept before the configuration goes in: a crash from here on still
        // knows which runner to remove.
        self.update(id, |s| s.runner_id = Some(jit.runner_id));
        self.deps
            .spaces
            .write_file(
                space,
                &guest::jit_path(home),
                jit.encoded.expose().as_bytes(),
                0o600,
            )
            .await?;
        self.deps
            .spaces
            .spawn(
                space,
                "/bin/bash",
                &[guest::wrapper_path(home)],
                &guest::runner_dir(home),
                &guest::process_tag(id),
            )
            .await?;
        let now = self.now();
        self.update(id, |s| {
            s.phase = Phase::Idle;
            s.registered_ms = Some(now);
        });
        self.timers.runner_registered(
            Event::new(now, EventKind::RunnerRegistered)
                .slot(id)
                .space(Some(space))
                .runner(Some(&jit.runner_name), Some(jit.runner_id)),
            id,
        );
        Ok(())
    }

    /// A JIT configuration: from the performance core's source (the hosted
    /// service) for a claimed job, else from GitHub with the pool's group.
    async fn mint_jit(
        &self,
        id: &str,
        name: &str,
        arch: &str,
        job: Option<&JobClaim>,
    ) -> Result<crate::github::JitConfig> {
        if let Some(h) = job.and_then(|j| j.jit.as_ref()) {
            return h.0.jit(job.expect("job"), name).await;
        }
        if let (Some(j), Some(src)) = (job, self.perf.as_ref().and_then(|p| p.jit.clone())) {
            return src.jit(j, name).await;
        }
        let group = *self
            .group
            .get_or_try_init(|| {
                self.deps
                    .github
                    .runner_group_id(self.cfg.runner_group.as_deref())
            })
            .await?;
        let labels = match job {
            Some(j) if !j.labels.is_empty() => j.labels.clone(),
            _ => crate::config::runner_labels(self.cfg.guest_os(), arch, &self.cfg.labels),
        };
        let _ = id;
        self.deps
            .github
            .generate_jit(&JitRequest {
                name: job
                    .and_then(|j| j.runner_name.clone())
                    .unwrap_or_else(|| name.to_string()),
                runner_group_id: group,
                labels,
                work_folder: guest::WORK_FOLDER.into(),
            })
            .await
    }

    /// Ends a slot: copies its runner logs out, deletes its Space, removes
    /// its runner at GitHub, forgets it.
    async fn finish(&self, id: &str, reason: &str, exit: Option<i32>) {
        // A teardown that could not delete the Space is retried here on the
        // next tick, without redoing the log copy and the job's events.
        let retry = self
            .slot(id)
            .is_some_and(|p| p.phase == Phase::Ending && p.perf.delete_failed > 0);
        let Some(mut s) = self.update(id, |s| {
            s.phase = Phase::Ending;
            s.ending.get_or_insert_with(|| reason.to_string());
        }) else {
            return;
        };
        let reason = s.ending.clone().unwrap_or_else(|| reason.to_string());
        let reason = reason.as_str();
        let now = self.now();
        let mut summary = guest::JobSummary::default();
        let mut ran_job = s.busy_ms.is_some() || s.perf.job.is_some();
        if !retry {
            if let (Some(space), Some(home)) = (&s.space_id, &s.home)
                && reason != "space_lost"
            {
                summary = self.copy_logs(&s, space, home).await;
                // The guest's marks and the cache write-back, while the VM
                // is still there.
                self.job_end(&s, space, home, reason, &summary).await;
            }
            ran_job = ran_job || summary.job.is_some();
            let s_busy = s.busy_ms.is_some();
            s = self
                .update(id, |x| {
                    x.perf.runner_result = summary.result.clone();
                    x.perf.exit_code = exit;
                    x.perf.ran = s_busy || summary.job.is_some();
                })
                .unwrap_or(s);
            if ran_job
                && matches!(
                    reason,
                    "job_done"
                        | "runner_exited"
                        | "job_timeout"
                        | "runner_crashed"
                        | "space_lost"
                        | "guest_unreachable"
                )
            {
                let mut e = Event::new(now, EventKind::JobFinished)
                    .slot(id)
                    .space(s.space_id.as_deref())
                    .runner(Some(&s.name), s.runner_id)
                    .ms(now.saturating_sub(s.busy_ms.or(s.registered_ms).unwrap_or(now)))
                    .reason(reason);
                e.exit_code = exit;
                e.job = summary.job.clone();
                self.timers.job_finished(e, id, summary.result.as_deref());
            } else if s.perf.job.is_some() {
                // A claimed job that never ran: the service requeues it.
                self.job_released(&s, reason);
            }
        }
        let mut deleted = true;
        if s.space_id.is_none() {
            // A create that never returned: its Space, if any, carries the
            // slot's label and the sweep finds it.
            self.rt.lock().expect("rt").last_sweep_ms = None;
        }
        // The Space and the runner go at once: both are calls on the
        // teardown path.
        let (space_res, runner_res) = tokio::join!(
            async {
                match &s.space_id {
                    Some(space) => Some(self.deps.spaces.delete(space).await),
                    None => None,
                }
            },
            async {
                match s.runner_id {
                    Some(rid) => Some(self.deps.github.delete_runner(rid).await),
                    None => None,
                }
            }
        );
        if let (Some(space), Some(Err(e))) = (&s.space_id, &space_res)
            && !matches!(e, SpacesError::NotFound(_))
        {
            deleted = false;
            let tries = s.perf.delete_failed + 1;
            if tries == 1 || tries % 20 == 0 {
                self.record(
                    Event::new(now, EventKind::Error)
                        .slot(id)
                        .space(Some(space))
                        .detail(format!(
                            "deleting Space {}: {e} (try {tries}; retried every tick)",
                            s.name
                        )),
                );
            }
            self.update(id, |x| x.perf.delete_failed = tries);
        }
        if let (Some(rid), Some(Err(e))) = (s.runner_id, &runner_res) {
            self.record(
                Event::new(now, EventKind::Error)
                    .slot(id)
                    .runner(Some(&s.name), Some(rid))
                    .detail(format!(
                        "removing runner {}: {e} (the next sweep retries)",
                        s.name
                    )),
            );
        }
        if !deleted {
            if s.perf.delete_failed + 1 < MAX_DELETE_TRIES {
                // Keep the slot: the next tick tries again, and the VM is
                // never forgotten while it may still run.
                return;
            }
            self.record(
                Event::new(self.now(), EventKind::Error)
                    .slot(id)
                    .detail(format!(
                        "giving up deleting Space {} after {MAX_DELETE_TRIES} tries; the sweep \
                         will find it by its label",
                        s.name
                    )),
            );
            self.rt.lock().expect("rt").last_sweep_ms = None;
        }
        self.remove_slot(id);
        self.after_teardown(&s, reason, ran_job || s.perf.ran).await;
        let mut e = Event::new(self.now(), EventKind::SlotDeleted)
            .slot(id)
            .space(s.space_id.as_deref())
            .runner(Some(&s.name), s.runner_id)
            .reason(reason)
            .ms(self.now().saturating_sub(s.created_ms));
        e.exit_code = exit;
        self.record(e);
    }

    async fn copy_logs(&self, s: &Slot, space: &str, home: &str) -> guest::JobSummary {
        let dir = self.paths.slot_logs(&self.cfg.name, &s.id);
        if cua_home::guard_write(&dir).is_err() || std::fs::create_dir_all(&dir).is_err() {
            return Default::default();
        }
        let console = guest::console_log(home);
        let diag = guest::diag_dir(home);
        let copy = async {
            // Both at once: the log copy is on the teardown path.
            tokio::join!(
                self.deps.spaces.download(space, &console, &dir),
                self.deps.spaces.download(space, &diag, &dir)
            )
        };
        match tokio::time::timeout(Duration::from_secs(120), copy).await {
            Ok((Ok(()), _)) | Ok((_, Ok(()))) => self.record(
                Event::new(self.now(), EventKind::LogsCopied)
                    .slot(&s.id)
                    .detail(dir.display().to_string()),
            ),
            Ok((Err(e), _)) => self.record(
                Event::new(self.now(), EventKind::Error)
                    .slot(&s.id)
                    .detail(format!("copying runner logs: {e}")),
            ),
            Err(_) => self.record(
                Event::new(self.now(), EventKind::Error)
                    .slot(&s.id)
                    .detail("copying runner logs timed out".to_string()),
            ),
        }
        std::fs::read_to_string(dir.join("cua-run.log"))
            .map(|t| guest::parse_console(&t))
            .unwrap_or_default()
    }

    /// A tick, then every slot it asked for provisioned (one after the
    /// other). `cua ci serve --once` and the tests.
    pub async fn tick_and_provision(&self) {
        for id in self.tick().await {
            self.provision(&id).await;
        }
    }

    /// Runs the reconciler until `shutdown`. Slots are provisioned
    /// concurrently; idle runners are kept on shutdown (the next start
    /// picks them up from the state file).
    pub async fn serve(
        self: Arc<Self>,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<()> {
        self.record(
            Event::new(self.now(), EventKind::ServeStarted).detail(format!(
                "{} {} on {}, at most {} runner(s), {} warm",
                self.cfg.scope,
                self.cfg.image,
                self.cfg.on,
                self.cfg.effective_max(),
                self.cfg.effective_warm()
            )),
        );
        let mut tasks = tokio::task::JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            for id in self.tick().await {
                let pool = self.clone();
                tasks.spawn(async move { pool.provision(&id).await });
            }
            // The golden image: rebuilt in the background when it is due.
            if let Some(job) = self.clone().golden_task() {
                tasks.spawn(job);
            }
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tokio::time::sleep(self.tick_interval()) => {}
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
        tasks.abort_all();
        self.record(Event::new(self.now(), EventKind::ServeStopped));
        Ok(())
    }

    /// Deletes every Space and runner of the pool (busy ones too).
    pub async fn remove_all(&self) -> Result<RemoveReport> {
        let mut report = RemoveReport::default();
        for s in self.slots() {
            if s.phase == Phase::Busy {
                report.busy_stopped += 1;
            }
            self.finish(&s.id, "pool_removed", None).await;
            report.slots += 1;
        }
        let spaces = self.deps.spaces.list().await?;
        let before = self.events.read().len();
        self.sweep(&spaces, true).await;
        report.orphans = self.events.read()[before..]
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    EventKind::OrphanSpaceDeleted | EventKind::OrphanRunnerDeleted
                )
            })
            .count();
        Ok(report)
    }
}

/// What [`Pool::remove_all`] removed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveReport {
    /// Slots torn down.
    pub slots: usize,
    /// Of those, running a job.
    pub busy_stopped: usize,
    /// Spaces and runners without a slot.
    pub orphans: usize,
}

/// A pool's status: its slots, the last week's counts and timings, and
/// what its egress policy does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolStatus {
    /// Pool.
    pub pool: String,
    /// `org:NAME` or `repo:OWNER/NAME`.
    pub scope: String,
    /// Image.
    pub image: String,
    /// Where Spaces run.
    pub on: String,
    /// The most runners at once.
    pub max_runners: u32,
    /// Idle runners kept.
    pub warm: u32,
    /// The labels a job's `runs-on` uses (for a Linux pool on several
    /// computers, each runner adds its architecture, ARM64 or X64).
    pub labels: Vec<String>,
    /// The slots now.
    pub slots: Vec<Slot>,
    /// Slots by phase.
    pub counts: BTreeMap<String, u32>,
    /// The last 7 days.
    pub last_7_days: Summary,
    /// The egress policy, in words.
    pub egress: String,
    /// Backing off until (Unix ms), after failures in a row.
    pub backoff_until_ms: Option<u64>,
}

/// The status of `cfg`'s pool from its files.
pub fn status(cfg: &PoolConfig, paths: &CiPaths, now_ms: u64) -> PoolStatus {
    let st = PoolState::load(&paths.state(&cfg.name));
    let events = EventLog::new(paths.events(&cfg.name)).read();
    let summary = crate::events::summarize(&events, now_ms.saturating_sub(7 * 86_400_000));
    let mut counts = BTreeMap::new();
    for s in &st.slots {
        *counts.entry(s.phase.as_str().to_string()).or_insert(0) += 1;
    }
    let egress = match cfg.egress {
        crate::config::EgressMode::Off => "off".to_string(),
        crate::config::EgressMode::DenyPrivate => match (&summary.egress_note, cfg.guest_os()) {
            (Some(n), _) => format!("not enforced: {n}"),
            (None, GuestOs::Macos) => "not enforced: macOS VMs (Lume) refuse egress policies; see \
                                       the docs for a host firewall"
                .to_string(),
            (None, GuestOs::Linux) => {
                "private, tailnet, link-local and unique-local networks denied (enforced by the host)"
                    .to_string()
            }
        },
    };
    // The architecture label: macOS VMs are Apple silicon; a Linux Space
    // on this computer has its architecture; elsewhere it is the host's.
    let arch = match cfg.guest_os() {
        GuestOs::Macos => Some("ARM64"),
        GuestOs::Linux if cfg.on.trim() == "local" => {
            crate::config::arch_label(std::env::consts::ARCH)
        }
        GuestOs::Linux => None,
    };
    let mut labels = vec![
        "self-hosted".to_string(),
        cfg.guest_os().label().to_string(),
    ];
    labels.extend(arch.map(str::to_string));
    labels.push(crate::config::POOL_LABEL.into());
    for l in &cfg.labels {
        if !labels.iter().any(|x| x.eq_ignore_ascii_case(l)) {
            labels.push(l.clone());
        }
    }
    PoolStatus {
        pool: cfg.name.clone(),
        scope: cfg.scope.to_string(),
        image: cfg.image.clone(),
        on: cfg.on.clone(),
        max_runners: cfg.effective_max(),
        warm: cfg.effective_warm(),
        labels,
        slots: st.slots,
        counts,
        last_7_days: summary,
        egress,
        backoff_until_ms: (st.failures > 0 && st.next_create_ms > now_ms)
            .then_some(st.next_create_ms),
    }
}

mod glue;

pub use glue::{Capacity, ClaimError, Claimed};

#[cfg(test)]
mod tests;
