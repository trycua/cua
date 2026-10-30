// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The coordinator (`cua cluster serve`): a durable job queue and a
//! scheduler that places jobs on the cluster's hosts.
//!
//! **Hot path.** Placement runs from memory ([`cua_cluster::placement`]): each
//! dispatch round takes queued jobs, ranks the hosts that fit, and sends
//! each host one `RunHostJobs` batch; the host accepts or rejects each job
//! from its own free resources. The round's dispatches are written to the
//! database in one transaction before the batches go out. One task per
//! host long-polls its change feed (`ListHostJobs`), which is also the
//! host's heartbeat, and applies each batch of changes in one transaction.
//! No task or thread per job.
//!
//! **Semantics.** A job runs at least once: a host that stops answering
//! for `node_down_after` is marked down and its jobs go back to the queue
//! with a new attempt (and a fence, delivered as a cancel when the host
//! answers again), so a partitioned host may still finish the old attempt
//! of a job. Not of a gang member: its host stops it once it stops hearing
//! from its coordinator, and the gang is placed again only after that
//! ([`gang`]).
//! A job completes exactly once: a completion is a conditional update that
//! only the current attempt's first terminal result passes; every other
//! result is counted in `duplicates_suppressed` and dropped. Leases use
//! this process's monotonic clock only, so a host's clock skew changes no
//! decision (it is reported).
//!
//! **Restarts.** The queue, each job's attempt and host, and each host's
//! feed position are in the database: a coordinator that starts again
//! loads the jobs in flight and reconciles them from the hosts' feeds,
//! looking a job up on its host (`GetHostJob`) when its lease runs out.

pub mod durable;
pub mod gang;
mod gang_sched;
pub mod journal;
pub mod metrics;
pub mod requests;
pub mod sessions;
pub mod store;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use cua_proto::env::v1 as pb;
use serde::Serialize;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use cua_cluster::link::{FailoverLink, GrpcLink, LinkError, NodeLink, Target};
use cua_cluster::model::{HostJobState, JobSpec, now_ms};
use cua_cluster::placement::{self, Candidate, Need, Policy};
use journal::{Journal, JournalError, LocalJournal, Recorder};
use metrics::Metrics;
use store::{Change, NewJob, NodeRow, Op, OpOut, Store, state};

/// How the coordinator runs.
#[derive(Clone, Debug)]
pub struct Config {
    /// Where `cluster.db` lives (`~/.cua/cluster`).
    pub dir: PathBuf,
    /// The cluster's name, for people.
    pub name: String,
    /// Spread or pack.
    pub policy: Policy,
    /// How long a host feed call waits for a change (and so how often an
    /// idle host is heard from).
    pub poll_wait: Duration,
    /// A host not heard from this long is down: its jobs run elsewhere.
    /// Longer than a feed call's own deadline (its wait plus 10 s), so a
    /// host that is only slow to answer, its CPU taken by the Spaces it
    /// runs, is not declared down while its answer is on the way.
    pub node_down_after: Duration,
    /// A dispatch not acknowledged this long is looked up on its host.
    pub ack_grace: Duration,
    /// A job running this long past its timeout is looked up on its host.
    pub run_grace: Duration,
    /// Jobs per `RunHostJobs` call.
    pub dispatch_batch: usize,
    /// Attempts a job gets when the submitter does not say.
    pub default_max_attempts: u32,
    /// How often the re-driver looks at the live sessions.
    pub session_tick: Duration,
    /// Runs a durable Space's `.cua/setup` after each placement (the first
    /// copy and every re-placement from scratch). `None`: no setup step.
    pub space_setup: Option<durable::SetupHook>,
}

impl Config {
    /// Defaults for a cluster in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            name: "cluster".into(),
            policy: Policy::Spread,
            poll_wait: Duration::from_secs(2),
            node_down_after: Duration::from_secs(30),
            ack_grace: Duration::from_secs(15),
            run_grace: Duration::from_secs(15 * 60),
            dispatch_batch: 256,
            default_max_attempts: 5,
            session_tick: Duration::from_secs(2),
            space_setup: None,
        }
    }
}

/// Makes the link to a host (a provider node in this process, or a test
/// double); `None` falls back to the gRPC link its row describes.
pub type LinkFactory = Arc<dyn Fn(&NodeRow) -> Option<Arc<dyn NodeLink>> + Send + Sync>;

/// A coordinator error, for the API.
#[derive(Debug, thiserror::Error)]
pub enum CoordError {
    /// Bad input.
    #[error("{0}")]
    Invalid(String),
    /// Unknown thing.
    #[error("{0}")]
    NotFound(String),
    /// Not allowed (an invite that does not work).
    #[error("{0}")]
    Denied(String),
    /// Nothing can take it now.
    #[error("{0}")]
    Unavailable(String),
    /// It clashes with what exists (a run id reused with another spec).
    #[error("{0}")]
    Conflict(String),
    /// The database.
    #[error("{0}")]
    Store(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Health {
    Unknown,
    Up,
    Down,
}

struct NodeRt {
    row: NodeRow,
    link: Arc<dyn NodeLink>,
    health: Health,
    status: Option<pb::HostNodeStatus>,
    last_ok: Option<Instant>,
    first_fail: Option<Instant>,
    pending_cpu: u64,
    pending_mem: u64,
    pending_jobs: u32,
    /// Rejected work since the last status: treat as full until it says
    /// otherwise.
    full: bool,
    skew_ms: i64,
    rtt_ms: u32,
    /// Attempts to cancel when it answers: job -> attempt.
    fences: HashMap<i64, u32>,
    error: String,
    stop: CancellationToken,
}

impl NodeRt {
    fn placeable(&self) -> bool {
        self.health == Health::Up
            && (self.row.state == "ready" || self.row.state == "pinned")
            && self.status.is_some()
            && !self.full
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Dispatched,
    Running,
}

struct Queued {
    run: String,
    spec: Arc<JobSpec>,
    attempt: u32,
    retries: u32,
    max_attempts: u32,
    ready_at: Instant,
    /// Not placed before this (a failed attempt's backoff).
    not_before: Instant,
    submitted_ms: i64,
}

/// A gang run as the scheduler keeps it (its row is in the database).
struct GangRt {
    spec: Arc<JobSpec>,
    min: u32,
    max: u32,
    retries: u32,
    policy: gang::GangPolicy,
    state: String,
    attempt: u32,
    /// Members of the current attempt.
    size: u32,
    failures: u32,
    /// Infrastructure failures that count against `policy.infra_retries`.
    infra_used: u32,
    ready_at: Instant,
    /// When the current attempt was placed (or this coordinator took it
    /// over): its start barrier's clock.
    placed_at: Instant,
    /// The current attempt's start barrier was released.
    released: bool,
    /// The attempt the barrier timeout or the watchdog already stopped.
    watched_out: u32,
    /// Every host that ran one of its members (placed there again first).
    hosts: Vec<String>,
    /// A placement is being recorded.
    placing: bool,
    /// Why it is not placed, for people.
    waiting: String,
    /// Members no computer can take now (what the elastic pool adds
    /// machines for); 0 once it is placed.
    short: u32,
}

impl GangRt {
    fn from_row(r: &gang::GangRow) -> Self {
        let wait = (r.ready_ms - now_ms()).max(0) as u64;
        Self {
            spec: Arc::new(r.spec.clone()),
            min: r.min,
            max: r.max,
            retries: r.retries,
            policy: r.policy.clone(),
            state: r.state.clone(),
            attempt: r.attempt,
            size: r.size,
            failures: r.failures,
            infra_used: r.infra_used,
            ready_at: Instant::now() + Duration::from_millis(wait),
            placed_at: Instant::now(),
            released: false,
            watched_out: 0,
            hosts: r.hosts.clone(),
            placing: false,
            waiting: String::new(),
            short: 0,
        }
    }
}

/// A gang placement planned in memory, recorded before it is sent.
struct GangPlan {
    run: String,
    attempt: u32,
    members: Vec<gang::GangMember>,
    need: Need,
}

struct Flight {
    node: String,
    attempt: u32,
    run: String,
    spec: Arc<JobSpec>,
    phase: Phase,
    /// When to look the job up on its host (the lease).
    deadline: Instant,
    /// When its host accepted it.
    started: Option<Instant>,
    ready_at: Instant,
    submitted_ms: i64,
    retries: u32,
    max_attempts: u32,
    user_cancel: bool,
    reconciling: bool,
}

#[derive(Default)]
struct Sched {
    nodes: BTreeMap<String, NodeRt>,
    queue: VecDeque<i64>,
    queued: HashMap<i64, Queued>,
    max_loaded: i64,
    inflight: HashMap<i64, Flight>,
    run_limits: HashMap<String, u32>,
    run_inflight: HashMap<String, u32>,
    /// Gang runs that have not ended.
    gangs: BTreeMap<String, GangRt>,
    /// Members of those gangs: job -> run.
    gang_jobs: HashMap<i64, String>,
    /// Gangs whose members changed: look at them again.
    gang_dirty: HashSet<String>,
    /// What running members last reported (start barrier, progress).
    progress: HashMap<i64, Progress>,
    /// Start barriers to release with each host's next feed call: host ->
    /// job -> attempt.
    releases: HashMap<String, BTreeMap<i64, u32>>,
}

/// A member's start barrier and progress as its host last reported them.
#[derive(Clone, Debug)]
struct Progress {
    attempt: u32,
    ready: bool,
    released: bool,
    /// Host clock (ms; 0: no progress yet).
    last_progress_ms: i64,
    step: i64,
    /// The host's clock when it reported, and when that was here: the
    /// stall age needs no clock agreement.
    host_now_ms: i64,
    heard: Instant,
}

impl Progress {
    /// How long since its last progress (None: none yet).
    fn quiet_for(&self) -> Option<Duration> {
        (self.last_progress_ms > 0).then(|| {
            Duration::from_millis((self.host_now_ms - self.last_progress_ms).max(0) as u64)
                + self.heard.elapsed()
        })
    }
}

impl Sched {
    fn requeue_front(&mut self, id: i64, f: &Flight, retry: bool) {
        self.requeue_later(id, f, retry, Duration::ZERO);
    }

    /// Back to the queue, placed again no sooner than `delay` from now.
    fn requeue_later(&mut self, id: i64, f: &Flight, retry: bool, delay: Duration) {
        if let Some(n) = self.run_inflight.get_mut(&f.run) {
            *n = n.saturating_sub(1);
        }
        let now = Instant::now();
        self.queued.insert(
            id,
            Queued {
                run: f.run.clone(),
                spec: f.spec.clone(),
                attempt: f.attempt,
                retries: f.retries + u32::from(retry),
                max_attempts: f.max_attempts,
                ready_at: now,
                not_before: now + delay,
                submitted_ms: f.submitted_ms,
            },
        );
        self.queue.push_front(id);
    }

    fn is_member(&self, id: i64) -> bool {
        self.gang_jobs.contains_key(&id)
    }

    fn land(&mut self, f: &Flight) {
        if let Some(n) = self.run_inflight.get_mut(&f.run) {
            *n = n.saturating_sub(1);
        }
    }
}

struct WriteReq {
    ops: Vec<Op>,
    done: tokio::sync::oneshot::Sender<Result<Vec<OpOut>, String>>,
}

/// One host as `cua cluster status` shows it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct NodeView {
    /// Name.
    pub name: String,
    /// `direct`, `relay` or `provider`.
    pub via: String,
    /// How it is reached now.
    pub address: String,
    /// `ready`, `draining`, `down` or `unknown`.
    pub health: String,
    /// OS.
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// Kinds it creates.
    pub kinds: Vec<String>,
    /// Runtimes it offers.
    pub runtimes: Vec<String>,
    /// CPU (thousandths).
    pub millicpus_total: u32,
    /// Free CPU.
    pub millicpus_free: u32,
    /// Memory (MiB).
    pub memory_total_mb: u64,
    /// Free memory.
    pub memory_free_mb: u64,
    /// Spaces running there (provided and jobs).
    pub spaces: u32,
    /// Jobs it runs (as it reports).
    pub jobs_running: u32,
    /// Most jobs it runs at once (0: CPU and memory decide).
    pub jobs_max: u32,
    /// Jobs this coordinator has there.
    pub jobs_assigned: u32,
    /// Seconds since it was last heard from.
    pub last_seen_secs: Option<u64>,
    /// Its clock minus ours (ms).
    pub clock_skew_ms: i64,
    /// Round trip of its last answer (ms).
    pub rtt_ms: u32,
    /// Its runner's epoch.
    pub epoch: u64,
    /// Why it cannot take work, or its last error.
    pub problem: String,
    /// Its accelerators (`apple:M2 Max:38-core GPU:96 GB`, `nvidia:L4:23 GB`).
    pub accelerators: Vec<String>,
    /// It is dedicated: it offers its own Space to jobs (`kind = "machine"`).
    pub dedicated: bool,
    /// What an hour of it costs (US dollars; 0 for machines you own).
    pub usd_per_hour: f64,
    /// Its addresses for gang peers (LAN, tailnet, public).
    pub addresses: Vec<String>,
}

/// Where an interactive Space went.
#[derive(Clone, Debug, Serialize)]
pub struct PlacedSpace {
    /// The host.
    pub node: String,
    /// `direct:<host address>:<port>` (add it with this and the token), or
    /// `relay:<machine>` (the account reaches it through the relay).
    pub id: String,
    /// The Space's cua-spacesd token.
    pub token: String,
    /// The Space on the host (`local:<name>`).
    pub local_space: String,
    /// Its kind.
    pub kind: String,
}

/// What `place_space` needs.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct SpaceRequest {
    /// Image.
    #[serde(default)]
    pub image: String,
    /// Kind.
    #[serde(default)]
    pub kind: String,
    /// Runtime.
    #[serde(default)]
    pub runtime: String,
    /// Name.
    #[serde(default)]
    pub name: String,
    /// vCPUs (0: 2).
    #[serde(default)]
    pub cpus: u32,
    /// Memory MiB (0: 4096).
    #[serde(default)]
    pub memory_mb: u64,
    /// No desktop (`CreateHostSpaceRequest.headless`).
    #[serde(default)]
    pub headless: bool,
}

/// The coordinator.
pub struct Coordinator {
    cfg: Config,
    store: Store,
    cluster_id: String,
    metrics: Metrics,
    sched: Mutex<Sched>,
    wake: Notify,
    links: Option<LinkFactory>,
    writes: tokio::sync::mpsc::UnboundedSender<WriteReq>,
    db_error: Mutex<Option<String>>,
    /// Where ops go before they take effect (a pool log, or nowhere).
    rec: Recorder,
    /// Another machine took over: this coordinator stopped for good.
    fenced: std::sync::atomic::AtomicBool,
    /// Pool log position of the last snapshot written.
    snapshot_seq: std::sync::atomic::AtomicU64,
    /// The highest job id in this coordinator's database: a job of the
    /// pool above it was submitted by a newer holder.
    max_job: std::sync::atomic::AtomicI64,
    /// One lock per durable Space name.
    durable_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Durable Space setups running now: (name, generation).
    setups: Mutex<std::collections::HashSet<(String, u32)>>,
    /// One lock per session name.
    session_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Per session: (driver job, its output size, when it last grew).
    session_watch: Mutex<HashMap<String, (i64, u64, i64)>>,
    stop: CancellationToken,
    me: Weak<Coordinator>,
}

/// Pool log entries between snapshots.
const SNAPSHOT_EVERY: u64 = 20_000;

/// In-memory queue refills below this many jobs ...
const QUEUE_LOW: usize = 10_000;
/// ... up to this many.
const QUEUE_HIGH: usize = 20_000;
/// Most jobs placed per round.
const ROUND_MAX: usize = 4096;
/// Lookups of jobs on hosts at once.
const RECONCILE_AT_ONCE: usize = 64;

fn host_job_id(cluster: &str, id: i64) -> String {
    format!("{cluster}-{id}")
}

impl Coordinator {
    /// The job a host job id names, when it is one of this cluster's.
    fn job_of(&self, host_job_id: &str) -> Option<i64> {
        host_job_id
            .strip_prefix(&self.cluster_id)
            .and_then(|r| r.strip_prefix('-'))
            .and_then(|r| r.parse::<i64>().ok())
    }
}

impl Coordinator {
    /// Opens the cluster in `cfg.dir` and starts scheduling: hosts, the
    /// queue and the jobs in flight come back from the database.
    pub async fn start(cfg: Config, links: Option<LinkFactory>) -> Result<Arc<Self>, CoordError> {
        Self::start_with(cfg, links, Arc::new(LocalJournal)).await
    }

    /// [`Coordinator::start`] recording every state change in `journal`
    /// before it takes effect (a pool log: the database must already be at
    /// the log's head, see [`journal::rebuild`]).
    pub async fn start_with(
        cfg: Config,
        links: Option<LinkFactory>,
        journal: Arc<dyn Journal>,
    ) -> Result<Arc<Self>, CoordError> {
        let store = Store::open(&cfg.dir.join("cluster.db")).map_err(CoordError::Store)?;
        let (existing_id, nodes, inflight, fences, limits, seq, max_job) = store
            .call(|c| {
                durable::init(c).map_err(|e| e.to_string())?;
                sessions::init(c).map_err(|e| e.to_string())?;
                Ok((
                    store::meta_get(c, "cluster_id")?,
                    store::nodes(c)?,
                    store::in_flight(c)?,
                    store::fences(c)?,
                    store::run_limits(c)?,
                    store::meta_get(c, journal::SEQ_KEY)?
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0),
                    store::max_job_id(c)?,
                ))
            })
            .await
            .map_err(CoordError::Store)?;
        let (live_gangs, live_members) = store
            .call(|c| Ok((gang::live_gangs(c)?, gang::live_members(c)?)))
            .await
            .map_err(CoordError::Store)?;
        let new_id = existing_id.is_none();
        let cluster_id = existing_id.unwrap_or_else(|| format!("{:08x}", rand::random::<u32>()));
        let (writes, rx) = tokio::sync::mpsc::unbounded_channel();
        let rec = Recorder {
            journal,
            pool: cluster_id.clone(),
            handle: tokio::runtime::Handle::current(),
        };
        let me = Arc::new_cyclic(|me| Self {
            cfg,
            store,
            cluster_id: cluster_id.clone(),
            metrics: Metrics::default(),
            sched: Mutex::new(Sched::default()),
            wake: Notify::new(),
            links,
            writes,
            db_error: Mutex::new(None),
            rec,
            fenced: std::sync::atomic::AtomicBool::new(false),
            snapshot_seq: std::sync::atomic::AtomicU64::new(seq),
            max_job: std::sync::atomic::AtomicI64::new(max_job),
            durable_locks: Mutex::default(),
            setups: Mutex::default(),
            session_locks: Mutex::default(),
            session_watch: Mutex::default(),
            stop: CancellationToken::new(),
            me: me.clone(),
        });
        tokio::spawn(me.clone().writer(rx));
        if new_id {
            me.persist_ops(vec![Op::Meta {
                k: "cluster_id".into(),
                v: cluster_id,
            }])
            .await
            .map_err(CoordError::Store)?;
        }
        let mut orphans = Vec::new();
        {
            let mut g = me.sched.lock().expect("sched");
            g.run_limits = limits.into_iter().collect();
            for r in &live_gangs {
                g.gangs.insert(r.run.clone(), GangRt::from_row(r));
                // Look at each again: members may have ended meanwhile.
                g.gang_dirty.insert(r.run.clone());
            }
            g.gang_jobs = live_members.into_iter().collect();
            for row in nodes {
                let rt = me.node_rt(row);
                g.nodes.insert(rt.row.name.clone(), rt);
            }
            for (node, id, attempt) in fences {
                if let Some(n) = g.nodes.get_mut(&node) {
                    n.fences.insert(id, attempt);
                }
            }
            let now = Instant::now();
            for j in inflight {
                if !g.nodes.contains_key(&j.node) {
                    orphans.push(if g.is_member(j.id) {
                        member_end(
                            j.id,
                            j.attempt,
                            state::ERROR,
                            format!("its host {} was removed", j.node),
                        )
                    } else {
                        Change::Requeue {
                            id: j.id,
                            attempt: j.attempt,
                            error: format!("its host {} was removed", j.node),
                            retry: true,
                        }
                    });
                    continue;
                }
                *g.run_inflight.entry(j.run.clone()).or_default() += 1;
                let phase = if j.state == state::DISPATCHED {
                    Phase::Dispatched
                } else {
                    Phase::Running
                };
                let deadline = if phase == Phase::Running {
                    now + Duration::from_secs(u64::from(j.spec.timeout_secs)) + me.cfg.run_grace
                } else {
                    now + me.cfg.ack_grace
                };
                g.inflight.insert(
                    j.id,
                    Flight {
                        node: j.node,
                        attempt: j.attempt,
                        run: j.run,
                        started: (phase == Phase::Running).then_some(now),
                        spec: Arc::new(j.spec),
                        phase,
                        deadline,
                        ready_at: now,
                        submitted_ms: j.submitted_ms,
                        retries: j.retries,
                        max_attempts: j.max_attempts,
                        user_cancel: j.state == state::CANCELING,
                        reconciling: false,
                    },
                );
            }
            let names: Vec<String> = g.nodes.keys().cloned().collect();
            for name in names {
                me.spawn_poller(&mut g, &name);
            }
        }
        if !orphans.is_empty() {
            me.persist(orphans).await;
        }
        tokio::spawn(me.clone().dispatcher());
        tokio::spawn(me.clone().leases());
        tokio::spawn(me.clone().durable_loop());
        tokio::spawn(me.clone().session_loop());
        Ok(me)
    }

    /// Stops every task (the database stays consistent at any point).
    pub fn stop(&self) {
        self.stop.cancel();
        let g = self.sched.lock().expect("sched");
        for n in g.nodes.values() {
            n.stop.cancel();
        }
    }

    /// This cluster's id (the prefix of its job ids on hosts).
    pub fn cluster_id(&self) -> &str {
        &self.cluster_id
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The database.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The counters.
    pub fn metrics(&self) -> metrics::Snapshot {
        self.metrics.snapshot()
    }

    /// Records a cluster operation in the cluster's hash-chained audit log
    /// (`<cluster dir>/spaces-audit.jsonl`, the format of a host's Spaces
    /// audit): who did what to which host, run or job.
    pub async fn audit(&self, action: &str, who: &str, subject: &str, detail: &str) {
        let (dir, a, w, s, d) = (
            self.cfg.dir.clone(),
            action.to_string(),
            who.to_string(),
            subject.to_string(),
            detail.to_string(),
        );
        let r =
            tokio::task::spawn_blocking(move || cua_host::provided::audit(&dir, &a, &w, &s, &d))
                .await;
        if let Ok(Err(e)) = r {
            tracing::warn!("cluster audit: {e}");
        }
    }

    /// The most recent audit events, newest first.
    pub async fn audit_log(&self, limit: usize) -> cua_host::provided::SpacesAuditReport {
        let dir = self.cfg.dir.clone();
        tokio::task::spawn_blocking(move || cua_host::provided::read_audit(&dir, limit))
            .await
            .unwrap_or_else(|e| cua_host::provided::SpacesAuditReport {
                recent: vec![],
                error: Some(e.to_string()),
            })
    }

    /// The last database write failure, while writes are failing.
    pub fn db_error(&self) -> Option<String> {
        self.db_error.lock().expect("db error").clone()
    }

    fn arc(&self) -> Arc<Self> {
        self.me.upgrade().expect("coordinator alive")
    }

    fn node_rt(&self, row: NodeRow) -> NodeRt {
        let link = self
            .links
            .as_ref()
            .and_then(|f| f(&row))
            .unwrap_or_else(|| default_link(&row));
        NodeRt {
            row,
            link,
            health: Health::Unknown,
            status: None,
            last_ok: None,
            first_fail: None,
            pending_cpu: 0,
            pending_mem: 0,
            pending_jobs: 0,
            full: false,
            skew_ms: 0,
            rtt_ms: 0,
            fences: HashMap::new(),
            error: String::new(),
            stop: self.stop.child_token(),
        }
    }

    fn spawn_poller(&self, g: &mut Sched, name: &str) {
        let Some(n) = g.nodes.get(name) else { return };
        let stop = n.stop.clone();
        let me = self.arc();
        let name = name.to_string();
        tokio::spawn(async move { me.poll(name, stop).await });
    }

    // ---- durable writes -------------------------------------------------

    /// Records `changes` in order (one transaction per group); waits until
    /// they are on disk (and in the pool log, for a pool), retrying a
    /// failing database or log (writes then stall, which stalls
    /// dispatching: nothing proceeds without being recorded). Per change,
    /// whether it took effect; nothing does once this coordinator is
    /// fenced.
    async fn persist(&self, changes: Vec<Change>) -> Vec<bool> {
        if changes.is_empty() {
            return vec![];
        }
        let n = changes.len();
        let ops = changes
            .into_iter()
            .map(|change| Op::Change { change })
            .collect();
        match self.persist_ops(ops).await {
            Ok(outs) => outs.iter().map(OpOut::applied).collect(),
            Err(_) => vec![false; n],
        }
    }

    /// Records `ops` (see [`Coordinator::persist`]).
    pub(crate) async fn persist_ops(&self, ops: Vec<Op>) -> Result<Vec<OpOut>, String> {
        if ops.is_empty() {
            return Ok(vec![]);
        }
        if self.is_fenced() {
            return Err("another machine coordinates now".into());
        }
        let (done, rx) = tokio::sync::oneshot::channel();
        if self.writes.send(WriteReq { ops, done }).is_err() {
            return Err("the coordinator stopped".into());
        }
        rx.await
            .unwrap_or_else(|_| Err("the coordinator stopped".into()))
    }

    /// Whether another machine took over (this coordinator stopped).
    pub fn is_fenced(&self) -> bool {
        self.fenced.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Resolves when this coordinator stops (fenced or stopped).
    pub async fn stopped(&self) {
        self.stop.cancelled().await
    }

    fn fence(&self, why: &str) {
        if !self.fenced.swap(true, std::sync::atomic::Ordering::SeqCst) {
            tracing::warn!("this machine no longer coordinates the pool: {why}");
        }
        self.stop();
    }

    async fn writer(self: Arc<Self>, mut rx: tokio::sync::mpsc::UnboundedReceiver<WriteReq>) {
        loop {
            // A stopped coordinator records nothing more (as if its process
            // ended): a new one owns the database.
            let first = tokio::select! {
                _ = self.stop.cancelled() => return,
                r = rx.recv() => match r {
                    Some(r) => r,
                    None => return,
                },
            };
            let mut group = vec![first];
            while group.len() < 64 {
                match rx.try_recv() {
                    Ok(r) => group.push(r),
                    Err(_) => break,
                }
            }
            let all: Arc<Vec<Op>> =
                Arc::new(group.iter().flat_map(|r| r.ops.iter().cloned()).collect());
            let mut backoff = Duration::from_millis(100);
            let results = loop {
                if self.stop.is_cancelled() {
                    for r in group {
                        let _ = r.done.send(Err("the coordinator stopped".into()));
                    }
                    return;
                }
                let batch = all.clone();
                let rec = self.rec.clone();
                match self
                    .store
                    .call(move |c| journal::record(c, &rec, &batch))
                    .await
                {
                    Ok(Ok(r)) => {
                        *self.db_error.lock().expect("db error") = None;
                        break r;
                    }
                    Ok(Err(JournalError::Fenced(e))) => {
                        self.fence(&e);
                        for r in group {
                            let _ = r.done.send(Err(format!("fenced: {e}")));
                        }
                        return;
                    }
                    Ok(Err(JournalError::Conflict(e))) => {
                        // The log is not where this database thinks: never
                        // write past what it does not know.
                        self.fence(&format!("the pool log moved on ({e})"));
                        for r in group {
                            let _ = r.done.send(Err(format!("conflict: {e}")));
                        }
                        return;
                    }
                    Ok(Err(JournalError::Full(e))) => {
                        // Nothing was appended: compact the log, then send
                        // the same group again.
                        tracing::warn!("the pool log is full ({e}): writing a snapshot first");
                        *self.db_error.lock().expect("db error") = Some(e);
                        if !self.maybe_snapshot(true).await {
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(Duration::from_secs(5));
                        }
                    }
                    Ok(Err(JournalError::Unavailable(e) | JournalError::Rekey(e))) => {
                        tracing::warn!("the pool log is out of reach (retrying): {e}");
                        *self.db_error.lock().expect("db error") = Some(e);
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(5));
                    }
                    Err(e) => {
                        tracing::warn!("cluster database write failed (retrying): {e}");
                        *self.db_error.lock().expect("db error") = Some(e);
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(5));
                    }
                }
            };
            let mut it = results.into_iter();
            for r in group {
                let mine: Vec<OpOut> = it.by_ref().take(r.ops.len()).collect();
                let _ = r.done.send(Ok(mine));
            }
            self.maybe_snapshot(false).await;
        }
    }

    /// Writes a snapshot to the pool log every [`SNAPSHOT_EVERY`] entries,
    /// so the log a machine taking over replays stays short, or now
    /// (`force`: the log refused an append until one is written), waiting
    /// for it. Returns whether one was written (or, not forced, sent).
    async fn maybe_snapshot(&self, force: bool) -> bool {
        use std::sync::atomic::Ordering;
        if !self.rec.journal.replicated() {
            return false;
        }
        let Ok(seq) = self
            .store
            .call(|c| {
                Ok(store::meta_get(c, journal::SEQ_KEY)?
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0))
            })
            .await
        else {
            return false;
        };
        let last = self.snapshot_seq.load(Ordering::SeqCst);
        if !force && seq < last + SNAPSHOT_EVERY {
            return false;
        }
        let dir = self.cfg.dir.clone();
        let Ok((seq, bytes)) = self
            .store
            .call(move |c| {
                let seq = store::meta_get(c, journal::SEQ_KEY)?
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                Ok((seq, journal::snapshot_bytes(c, &dir)?))
            })
            .await
        else {
            return false;
        };
        let journal = self.rec.journal.clone();
        if force {
            return match journal.snapshot(seq, bytes).await {
                Ok(()) => {
                    self.snapshot_seq.fetch_max(seq, Ordering::SeqCst);
                    true
                }
                Err(e) => {
                    tracing::warn!("writing a pool snapshot at {seq}: {e}");
                    false
                }
            };
        }
        self.snapshot_seq.store(seq, Ordering::SeqCst);
        let me = self.me.clone();
        tokio::spawn(async move {
            if let Err(e) = journal.snapshot(seq, bytes).await {
                tracing::warn!("writing a pool snapshot at {seq}: {e}");
                // Not written: try again after the next append.
                if let Some(c) = me.upgrade() {
                    let _ = c.snapshot_seq.compare_exchange(
                        seq,
                        last,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                }
            }
        });
        true
    }

    // ---- submitting -------------------------------------------------------

    /// Submits jobs to run `run` (at most `concurrency` of its jobs in
    /// flight at once; 0: no limit). A key submitted before returns the
    /// job it made (`true`) instead of a new one.
    pub async fn submit(
        &self,
        run: &str,
        concurrency: u32,
        jobs: Vec<NewJob>,
    ) -> Result<Vec<(i64, bool)>, CoordError> {
        if run.trim().is_empty() {
            return Err(CoordError::Invalid("a run needs an id".into()));
        }
        for j in &jobs {
            if j.spec.argv.is_empty() {
                return Err(CoordError::Invalid(format!("job {} has no command", j.key)));
            }
        }
        let n = jobs.len();
        // The limit goes in before the jobs do: once they are queued the
        // scheduler may place them, and a run whose limit is not known yet
        // is placed without one (16 of a limit-3 run at once on a slow host).
        if concurrency > 0 {
            self.sched
                .lock()
                .expect("sched")
                .run_limits
                .entry(run.to_string())
                .or_insert(concurrency);
        }
        let outs = self
            .persist_ops(vec![Op::Submit {
                run: run.to_string(),
                concurrency,
                now_ms: now_ms(),
                jobs,
            }])
            .await
            .map_err(CoordError::Store)?;
        let out = match outs.into_iter().next() {
            Some(OpOut::Submitted(v)) => v,
            _ => return Err(CoordError::Store("the submit was not recorded".into())),
        };
        if let Some(top) = out.iter().map(|(id, _)| *id).max() {
            self.max_job
                .fetch_max(top, std::sync::atomic::Ordering::SeqCst);
        }
        let fresh = out.iter().filter(|(_, existed)| !existed).count();
        self.metrics
            .submitted
            .fetch_add(fresh as u64, std::sync::atomic::Ordering::Relaxed);
        tracing::debug!(run, jobs = n, fresh, "submitted");
        self.wake.notify_one();
        Ok(out)
    }

    /// Cancels a job: a queued one ends now; one on a host is stopped
    /// there and ends `canceled` (unless it ended first).
    pub async fn cancel_job(&self, id: i64) -> Result<String, CoordError> {
        let (change, cancel) = {
            let mut g = self.sched.lock().expect("sched");
            if g.queued.remove(&id).is_some() {
                (
                    Change::EndQueued {
                        id,
                        state: state::CANCELED.into(),
                        error: "canceled by its owner".into(),
                        at_ms: now_ms(),
                    },
                    None,
                )
            } else if let Some(f) = g.inflight.get_mut(&id) {
                f.user_cancel = true;
                let (node, attempt) = (f.node.clone(), f.attempt);
                let link = g.nodes.get(&node).map(|n| n.link.clone());
                (Change::Canceling { id }, link.map(|l| (l, attempt)))
            } else {
                // Queued in the database only (not loaded yet), or ended.
                (
                    Change::EndQueued {
                        id,
                        state: state::CANCELED.into(),
                        error: "canceled by its owner".into(),
                        at_ms: now_ms(),
                    },
                    None,
                )
            }
        };
        let applied = self.persist(vec![change]).await;
        if let Some((link, attempt)) = cancel {
            let job_id = host_job_id(&self.cluster_id, id);
            let _ = link
                .cancel(pb::CancelHostJobRequest {
                    job_id,
                    attempt,
                    reason: "canceled by its owner".into(),
                })
                .await;
            return Ok("canceling".into());
        }
        Ok(if applied.first() == Some(&true) {
            "canceled".into()
        } else {
            "already ended".into()
        })
    }

    /// Cancels every job of `run` that has not ended (a gang run ends
    /// `canceled` once its members stopped).
    pub async fn cancel_run(&self, run: &str) -> Result<u64, CoordError> {
        self.cancel_gang(run).await;
        let run_id = run.to_string();
        let ids: Vec<i64> = self
            .store
            .call(move |c| {
                Ok(store::jobs(c, &run_id, "", 0, usize::MAX >> 1, false)?
                    .into_iter()
                    .filter(|j| !state::is_terminal(&j.state))
                    .map(|j| j.id)
                    .collect())
            })
            .await
            .map_err(CoordError::Store)?;
        let n = ids.len() as u64;
        // One write for the whole run: queued jobs end now, jobs on hosts
        // are marked canceling and their hosts told.
        let mut changes = Vec::with_capacity(ids.len());
        let mut on_hosts = Vec::new();
        {
            let mut g = self.sched.lock().expect("sched");
            let at = now_ms();
            for id in ids {
                if let Some(f) = g.inflight.get_mut(&id) {
                    f.user_cancel = true;
                    let (node, attempt) = (f.node.clone(), f.attempt);
                    changes.push(Change::Canceling { id });
                    if let Some(link) = g.nodes.get(&node).map(|n| n.link.clone()) {
                        on_hosts.push((link, id, attempt));
                    }
                } else {
                    g.queued.remove(&id);
                    changes.push(Change::EndQueued {
                        id,
                        state: state::CANCELED.into(),
                        error: "canceled by its owner".into(),
                        at_ms: at,
                    });
                }
            }
        }
        for chunk in changes.chunks(2000) {
            self.persist(chunk.to_vec()).await;
        }
        let cluster = self.cluster_id.clone();
        futures_cancel(on_hosts, &cluster).await;
        Ok(n)
    }

    // ---- dispatching ------------------------------------------------------

    async fn refill(&self) {
        let (want, after) = {
            let g = self.sched.lock().expect("sched");
            if g.queue.len() >= QUEUE_LOW {
                return;
            }
            (QUEUE_HIGH - g.queue.len(), g.max_loaded)
        };
        let rows = match self
            .store
            .call(move |c| store::queued_after(c, after, want))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("loading queued jobs: {e}");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut g = self.sched.lock().expect("sched");
        for r in rows {
            g.max_loaded = g.max_loaded.max(r.id);
            if g.inflight.contains_key(&r.id) || g.queued.contains_key(&r.id) {
                continue;
            }
            g.queued.insert(
                r.id,
                Queued {
                    run: r.run,
                    spec: Arc::new(r.spec),
                    attempt: r.attempt,
                    retries: r.retries,
                    max_attempts: r.max_attempts,
                    ready_at: now,
                    not_before: now,
                    submitted_ms: r.submitted_ms,
                },
            );
            g.queue.push_back(r.id);
        }
    }

    async fn dispatcher(self: Arc<Self>) {
        loop {
            if self.stop.is_cancelled() {
                return;
            }
            self.refill().await;
            self.gang_tick().await;
            let (plan, ended, gangs) = self.plan();
            if !ended.is_empty() {
                self.persist(ended).await;
            }
            for gp in gangs {
                self.launch_gang(gp).await;
            }
            if plan.is_empty() {
                tokio::select! {
                    _ = self.stop.cancelled() => return,
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
                continue;
            }
            let at = now_ms();
            let changes: Vec<Change> = plan
                .iter()
                .flat_map(|(node, _, jobs)| {
                    jobs.iter().map(move |(id, spec)| Change::Dispatched {
                        id: *id,
                        node: node.clone(),
                        attempt: spec.attempt,
                        at_ms: at,
                    })
                })
                .collect();
            let n = changes.len();
            let applied = self.persist(changes).await;
            // A job the database did not move (canceled meanwhile) is not
            // sent.
            let mut k = 0;
            let mut skipped = HashSet::new();
            for (_, _, jobs) in &plan {
                for (id, _) in jobs {
                    if applied.get(k) != Some(&true) {
                        skipped.insert(*id);
                    }
                    k += 1;
                }
            }
            if !skipped.is_empty() {
                let mut g = self.sched.lock().expect("sched");
                for id in &skipped {
                    if let Some(f) = g.inflight.remove(id) {
                        g.land(&f);
                        if let Some(nrt) = g.nodes.get_mut(&f.node) {
                            nrt.pending_cpu =
                                nrt.pending_cpu.saturating_sub(u64::from(f.spec.millicpus));
                            nrt.pending_mem = nrt.pending_mem.saturating_sub(f.spec.memory_mb);
                            nrt.pending_jobs = nrt.pending_jobs.saturating_sub(1);
                        }
                    }
                }
            }
            self.metrics.dispatched.fetch_add(
                (n - skipped.len()) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            for (node, link, jobs) in plan {
                let jobs: Vec<(i64, JobSpec)> = jobs
                    .into_iter()
                    .filter(|(id, _)| !skipped.contains(id))
                    .collect();
                if jobs.is_empty() {
                    continue;
                }
                let me = self.clone();
                tokio::spawn(async move { me.send(node, link, jobs).await });
            }
            tokio::task::yield_now().await;
        }
    }

    /// Places queued jobs on hosts, in memory. Returns the batches and the
    /// changes of jobs that ended in the queue (out of attempts).
    #[allow(clippy::type_complexity)]
    fn plan(
        &self,
    ) -> (
        Vec<(String, Arc<dyn NodeLink>, Vec<(i64, JobSpec)>)>,
        Vec<Change>,
        Vec<GangPlan>,
    ) {
        let mut g = self.sched.lock().expect("sched");
        let names: Vec<String> = g
            .nodes
            .iter()
            .filter(|(_, n)| n.placeable())
            .map(|(k, _)| k.clone())
            .collect();
        let at = Instant::now();
        let gangs_ready = g
            .gangs
            .values()
            .any(|r| r.state == gang::gstate::QUEUED && !r.placing && r.ready_at <= at);
        if names.is_empty() {
            for r in g.gangs.values_mut() {
                if r.state == gang::gstate::QUEUED {
                    r.waiting = "none of your computers can take work now".into();
                    r.short = r.min.max(1);
                }
            }
            return (vec![], vec![], vec![]);
        }
        if g.queue.is_empty() && !gangs_ready {
            return (vec![], vec![], vec![]);
        }
        let mut cands: Vec<Candidate> = names
            .iter()
            .map(|k| {
                let n = &g.nodes[k];
                Candidate {
                    name: k.clone(),
                    ready: true,
                    os: n.row.os.clone(),
                    status: n.status.clone(),
                    pending_millicpus: n.pending_cpu,
                    pending_memory_mb: n.pending_mem,
                    pending_jobs: n.pending_jobs,
                    elastic: n.row.via == "provider",
                    pinned: n.row.state == "pinned",
                    provider: n.row.provider.clone(),
                    ..Default::default()
                }
            })
            .collect();
        // Only machines that take just the work naming them (this one, when
        // it does not provide Spaces): work that names no machine (`auto`)
        // has nothing else to go to, so it runs here rather than wait for
        // ever.
        if cands.iter().all(|c| c.pinned) {
            for c in &mut cands {
                c.pinned = false;
            }
        }
        // Gangs first: they need several hosts at once.
        let gang_plans = if gangs_ready {
            self.plan_gangs(&mut g, &mut cands)
        } else {
            vec![]
        };
        let mut out: BTreeMap<String, Vec<(i64, JobSpec)>> = BTreeMap::new();
        let mut ended = Vec::new();
        let mut deferred = Vec::new();
        let (mut placed, mut scanned, mut misses, mut skipped) = (0usize, 0usize, 0usize, 0usize);
        let now = Instant::now();
        let mut run_counts: HashMap<String, u32> = HashMap::new();
        // Shapes (what a job needs) that found no host this round: every
        // later job of the same shape is passed over at once, so jobs that
        // fit elsewhere (another OS, a pinned host, the elastic capacity)
        // behind thousands that do not are still placed.
        let mut missed: std::collections::HashSet<NeedKey> = std::collections::HashSet::new();
        while let Some(id) = g.queue.pop_front() {
            let Some(q) = g.queued.get(&id) else {
                continue;
            };
            if q.not_before > now {
                // A failed attempt's pause.
                deferred.push(id);
                continue;
            }
            let (retries, max_attempts, run) = (q.retries, q.max_attempts, q.run.clone());
            let need = need_of(&q.spec);
            let key = need_key(&need);
            if retries < max_attempts && missed.contains(&key) {
                deferred.push(id);
                skipped += 1;
                if skipped >= SKIP_MAX {
                    break;
                }
                continue;
            }
            if placed >= ROUND_MAX || scanned >= 2 * ROUND_MAX || misses >= 32 {
                g.queue.push_front(id);
                break;
            }
            scanned += 1;
            if retries >= max_attempts {
                let q = g.queued.remove(&id).expect("queued");
                ended.push(Change::EndQueued {
                    id,
                    state: state::ERROR.into(),
                    error: format!(
                        "gave up after {} attempts were lost (hosts went down or could not run it)",
                        q.retries
                    ),
                    at_ms: now_ms(),
                });
                Metrics::inc(&self.metrics.ended_other);
                continue;
            }
            if let Some(limit) = g.run_limits.get(&run).copied() {
                let cur = g.run_inflight.get(&run).copied().unwrap_or(0)
                    + run_counts.get(&run).copied().unwrap_or(0);
                if cur >= limit {
                    deferred.push(id);
                    continue;
                }
            }
            let Some(&best) = placement::rank(&cands, &need, self.cfg.policy).first() else {
                deferred.push(id);
                misses += 1;
                missed.insert(key);
                continue;
            };
            misses = 0;
            let c = &mut cands[best];
            c.pending_millicpus += u64::from(need.millicpus);
            c.pending_memory_mb += need.memory_mb;
            c.pending_jobs += 1;
            let node = c.name.clone();
            let q = g.queued.remove(&id).expect("queued");
            let attempt = q.attempt + 1;
            let mut spec = (*q.spec).clone();
            spec.job_id = host_job_id(&self.cluster_id, id);
            spec.attempt = attempt;
            *run_counts.entry(q.run.clone()).or_default() += 1;
            g.inflight.insert(
                id,
                Flight {
                    node: node.clone(),
                    attempt,
                    run: q.run,
                    spec: q.spec,
                    phase: Phase::Dispatched,
                    // The lease starts once the batch is sent (`send`).
                    deadline: now + Duration::from_secs(3600),
                    started: None,
                    ready_at: q.ready_at,
                    submitted_ms: q.submitted_ms,
                    retries: q.retries,
                    max_attempts: q.max_attempts,
                    user_cancel: false,
                    reconciling: false,
                },
            );
            out.entry(node).or_default().push((id, spec));
            placed += 1;
        }
        for id in deferred.into_iter().rev() {
            g.queue.push_front(id);
        }
        for (run, n) in run_counts {
            *g.run_inflight.entry(run).or_default() += n;
        }
        for c in &cands {
            if let Some(n) = g.nodes.get_mut(&c.name) {
                n.pending_cpu = c.pending_millicpus;
                n.pending_mem = c.pending_memory_mb;
                n.pending_jobs = c.pending_jobs;
            }
        }
        let plan = out
            .into_iter()
            .map(|(node, jobs)| {
                let link = g.nodes[&node].link.clone();
                (node, link, jobs)
            })
            .collect();
        (plan, ended, gang_plans)
    }

    async fn send(
        self: Arc<Self>,
        node: String,
        link: Arc<dyn NodeLink>,
        jobs: Vec<(i64, JobSpec)>,
    ) {
        for chunk in jobs.chunks(self.cfg.dispatch_batch.max(1)) {
            if self.stop.is_cancelled() {
                return;
            }
            let req = pb::RunHostJobsRequest {
                jobs: chunk.iter().map(|(_, s)| s.to_pb()).collect(),
                scheduler: self.cluster_id.clone(),
            };
            let started = Instant::now();
            let r = link.run(req).await;
            self.metrics.dispatch_call(started.elapsed());
            let mut changes = Vec::new();
            let mut cancels = Vec::new();
            {
                let mut g = self.sched.lock().expect("sched");
                if let Some(n) = g.nodes.get_mut(&node) {
                    for (_, s) in chunk {
                        n.pending_cpu = n.pending_cpu.saturating_sub(u64::from(s.millicpus));
                        n.pending_mem = n.pending_mem.saturating_sub(s.memory_mb);
                        n.pending_jobs = n.pending_jobs.saturating_sub(1);
                    }
                }
                // The lease of each job sent starts now; an acknowledgement
                // below replaces it with the run's.
                let lease = Instant::now() + self.cfg.ack_grace;
                for (id, s) in chunk {
                    if let Some(f) = g.inflight.get_mut(id)
                        && f.attempt == s.attempt
                        && f.phase == Phase::Dispatched
                    {
                        f.deadline = lease;
                    }
                }
                match r {
                    Ok(resp) => {
                        if let Some(st) = resp.node.clone() {
                            self.heard(&mut g, &node, st, Some(started));
                        }
                        let by_id: HashMap<&str, &pb::HostJobAck> =
                            resp.acks.iter().map(|a| (a.job_id.as_str(), a)).collect();
                        for (id, spec) in chunk {
                            match by_id.get(spec.job_id.as_str()) {
                                Some(ack) => {
                                    self.on_ack(&mut g, &node, *id, ack, &mut changes, &mut cancels)
                                }
                                None => {
                                    // No answer for it: the lease looks it up.
                                }
                            }
                        }
                    }
                    Err(LinkError::Refused(e)) => {
                        // The host refuses the call itself (an old host, a
                        // revoked token): nothing was accepted.
                        if let Some(n) = g.nodes.get_mut(&node) {
                            n.error = e.clone();
                            n.full = true;
                        }
                        for (id, spec) in chunk {
                            if let Some(f) = g.inflight.remove(id) {
                                if f.attempt == spec.attempt {
                                    self.give_up(
                                        &mut g,
                                        *id,
                                        f,
                                        format!("{node} refused: {e}"),
                                        false,
                                        false,
                                        &mut changes,
                                    );
                                } else {
                                    g.inflight.insert(*id, f);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Unknown whether it arrived (most likely not: the
                        // host is unreachable). Run the jobs elsewhere and
                        // fence these attempts on the host, delivered when
                        // it answers again; send it nothing until then.
                        tracing::debug!(%node, "dispatch: {e}");
                        if let Some(n) = g.nodes.get_mut(&node) {
                            n.error = e.to_string();
                            n.full = true;
                        }
                        for (id, spec) in chunk {
                            let Some(f) = g.inflight.remove(id) else {
                                continue;
                            };
                            if f.attempt != spec.attempt || f.node != node {
                                g.inflight.insert(*id, f);
                                continue;
                            }
                            let attempt = f.attempt;
                            self.give_up(
                                &mut g,
                                *id,
                                f,
                                format!("{node}: {e}"),
                                false,
                                false,
                                &mut changes,
                            );
                            changes.push(Change::FenceAdd {
                                node: node.clone(),
                                id: *id,
                                attempt,
                            });
                            if let Some(n) = g.nodes.get_mut(&node) {
                                n.fences.insert(*id, attempt);
                            }
                        }
                    }
                }
            }
            self.finish(changes, cancels).await;
        }
    }

    fn mark_running(&self, f: &mut Flight) {
        let now = Instant::now();
        f.phase = Phase::Running;
        f.started = Some(now);
        f.deadline = now + Duration::from_secs(u64::from(f.spec.timeout_secs)) + self.cfg.run_grace;
    }

    fn on_ack(
        &self,
        g: &mut Sched,
        node: &str,
        id: i64,
        ack: &pb::HostJobAck,
        changes: &mut Vec<Change>,
        cancels: &mut Vec<(String, i64, u32)>,
    ) {
        let Some(f) = g.inflight.get(&id) else { return };
        if f.node != node || f.attempt != ack.attempt {
            return;
        }
        match ack.outcome.as_str() {
            "accepted" | "duplicate" => {
                Metrics::inc(&self.metrics.accepted);
                if let Some(job) = &ack.job
                    && HostJobState::parse(&job.state).is_some_and(|s| s.is_terminal())
                {
                    self.on_record(g, node, job, changes, cancels);
                    return;
                }
                let f = g.inflight.get_mut(&id).expect("flight");
                if f.phase == Phase::Dispatched {
                    self.metrics.queue_wait(f.ready_at.elapsed());
                    self.mark_running(f);
                    changes.push(Change::Running {
                        id,
                        attempt: f.attempt,
                        at_ms: now_ms(),
                    });
                }
            }
            "rejected" => {
                Metrics::inc(&self.metrics.rejected);
                if let Some(n) = g.nodes.get_mut(node) {
                    n.full = true;
                }
                let f = g.inflight.remove(&id).expect("flight");
                self.give_up(
                    g,
                    id,
                    f,
                    format!("{}{node}: {}", gang::NO_ROOM, ack.reason),
                    false,
                    false,
                    changes,
                );
            }
            _ if g.is_member(id) => {
                let f = g.inflight.remove(&id).expect("flight");
                self.give_up(
                    g,
                    id,
                    f,
                    format!("{node}: {}", ack.reason),
                    false,
                    false,
                    changes,
                );
            }
            _ => {
                // Stale: the host saw a newer attempt (a restored
                // database): go past it.
                let mut f = g.inflight.remove(&id).expect("flight");
                let seen = ack.job.as_ref().map_or(f.attempt, |j| j.attempt);
                changes.push(Change::Requeue {
                    id,
                    attempt: f.attempt,
                    error: format!("{node}: {}", ack.reason),
                    retry: false,
                });
                f.attempt = f.attempt.max(seen);
                g.requeue_front(id, &f, false);
            }
        }
    }

    /// A host's record of a job: moves the job on, or counts it as a
    /// duplicate (an ended job, an attempt that is not current).
    fn on_record(
        &self,
        g: &mut Sched,
        node: &str,
        h: &pb::HostJob,
        changes: &mut Vec<Change>,
        cancels: &mut Vec<(String, i64, u32)>,
    ) {
        let Some(id) = h
            .job_id
            .strip_prefix(&self.cluster_id)
            .and_then(|r| r.strip_prefix('-'))
            .and_then(|r| r.parse::<i64>().ok())
        else {
            return;
        };
        let Some(st) = HostJobState::parse(&h.state) else {
            return;
        };
        let current = g
            .inflight
            .get(&id)
            .is_some_and(|f| f.node == node && f.attempt == h.attempt);
        if !current {
            // The newest attempt this coordinator knows of the job.
            let known = g
                .inflight
                .get(&id)
                .map(|f| f.attempt)
                .or_else(|| g.queued.get(&id).map(|q| q.attempt));
            let newer_job =
                known.is_none() && id > self.max_job.load(std::sync::atomic::Ordering::SeqCst);
            if st == HostJobState::Running && (known.is_some_and(|k| h.attempt > k) || newer_job) {
                // A newer attempt than any this coordinator dispatched, or
                // a job submitted after its view (found by the simulation:
                // a stale holder canceled a newer holder's new jobs over and
                // over): a newer holder runs the job now, and this
                // coordinator is stale (its next write is refused). Leave
                // it running.
                Metrics::inc(&self.metrics.stale_view);
            } else if st == HostJobState::Running {
                // An attempt that is no longer current still runs there.
                cancels.push((node.to_string(), id, h.attempt));
            } else if matches!(st, HostJobState::Succeeded | HostJobState::Failed) {
                // A result that would have been a completion: an attempt
                // that is no longer current, or a job that already ended.
                Metrics::inc(&self.metrics.duplicates_suppressed);
            }
            return;
        }
        let f = g.inflight.get_mut(&id).expect("flight");
        match st {
            HostJobState::Running => {
                if f.phase == Phase::Dispatched {
                    self.metrics.queue_wait(f.ready_at.elapsed());
                    Metrics::inc(&self.metrics.accepted);
                    self.mark_running(f);
                    changes.push(Change::Running {
                        id,
                        attempt: f.attempt,
                        at_ms: now_ms(),
                    });
                }
            }
            HostJobState::Succeeded | HostJobState::Failed => {
                let f = g.inflight.remove(&id).expect("flight");
                if st == HostJobState::Failed
                    && !g.is_member(id)
                    && !f.user_cancel
                    && f.retries < f.spec.retries
                    && f.retries + 1 < f.max_attempts
                {
                    // It may fail this many times: again, after a pause.
                    Metrics::inc(&self.metrics.retries);
                    let why = if h.timed_out {
                        "ran out of time".to_string()
                    } else {
                        format!("exited {}", h.exit_code.unwrap_or(-1))
                    };
                    changes.push(Change::Requeue {
                        id,
                        attempt: f.attempt,
                        error: format!("{node}: {why}; running it again"),
                        retry: true,
                    });
                    let delay = gang::backoff(f.retries + 1);
                    g.requeue_later(id, &f, true, delay);
                    return;
                }
                g.land(&f);
                changes.push(complete(
                    id,
                    h,
                    if st == HostJobState::Succeeded {
                        state::SUCCEEDED
                    } else {
                        state::FAILED
                    },
                ));
                self.metrics.ended(Duration::from_millis(
                    (now_ms() - f.submitted_ms).max(0) as u64
                ));
                Metrics::inc(if st == HostJobState::Succeeded {
                    &self.metrics.succeeded
                } else {
                    &self.metrics.failed
                });
            }
            HostJobState::Error | HostJobState::Canceled => {
                let f = g.inflight.remove(&id).expect("flight");
                if g.is_member(id) {
                    // Its gang decides what happens next.
                    g.land(&f);
                    changes.push(complete(
                        id,
                        h,
                        if f.user_cancel || st == HostJobState::Canceled {
                            state::CANCELED
                        } else {
                            state::ERROR
                        },
                    ));
                    Metrics::inc(&self.metrics.ended_other);
                    return;
                }
                if f.user_cancel {
                    g.land(&f);
                    changes.push(complete(id, h, state::CANCELED));
                    Metrics::inc(&self.metrics.ended_other);
                    self.metrics.ended(Duration::from_millis(
                        (now_ms() - f.submitted_ms).max(0) as u64
                    ));
                } else if st == HostJobState::Error && f.retries + 1 >= f.max_attempts {
                    g.land(&f);
                    let mut c = complete(id, h, state::ERROR);
                    if let Change::Complete { error, .. } = &mut c {
                        *error = format!("gave up after {} attempts: {}", f.retries + 1, h.error);
                    }
                    changes.push(c);
                    Metrics::inc(&self.metrics.ended_other);
                    self.metrics.ended(Duration::from_millis(
                        (now_ms() - f.submitted_ms).max(0) as u64
                    ));
                } else {
                    Metrics::inc(&self.metrics.retries);
                    changes.push(Change::Requeue {
                        id,
                        attempt: f.attempt,
                        error: format!("{node}: {}", h.error),
                        retry: st == HostJobState::Error,
                    });
                    g.requeue_front(id, &f, st == HostJobState::Error);
                }
            }
        }
    }

    /// Records changes, counts completions the database refused as
    /// duplicates, sends cancels, and wakes the dispatcher.
    async fn finish(&self, changes: Vec<Change>, cancels: Vec<(String, i64, u32)>) {
        let kinds: Vec<bool> = changes
            .iter()
            .map(|c| matches!(c, Change::Complete { .. }))
            .collect();
        let ended: Vec<i64> = changes
            .iter()
            .filter_map(|c| match c {
                Change::Complete { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        let applied = self.persist(changes).await;
        for (is_complete, ok) in kinds.into_iter().zip(applied) {
            if is_complete && !ok {
                Metrics::inc(&self.metrics.duplicates_suppressed);
            }
        }
        self.gangs_changed(&ended);
        for (node, id, attempt) in cancels {
            let link = {
                let g = self.sched.lock().expect("sched");
                g.nodes.get(&node).map(|n| n.link.clone())
            };
            if let Some(link) = link {
                let job_id = host_job_id(&self.cluster_id, id);
                Metrics::inc(&self.metrics.fences);
                tokio::spawn(async move {
                    let _ = link
                        .cancel(pb::CancelHostJobRequest {
                            job_id,
                            attempt,
                            reason: "this attempt was superseded".into(),
                        })
                        .await;
                });
            }
        }
        self.wake.notify_one();
    }

    // ---- hosts ------------------------------------------------------------

    /// A fresh status from `node`. `sent` is when a call that answers at
    /// once was made (its round trip is measured, and the host's clock read
    /// at its midpoint); `None` for a long poll, whose host clock is read
    /// as it answers.
    fn heard(&self, g: &mut Sched, node: &str, st: pb::HostNodeStatus, sent: Option<Instant>) {
        let Some(n) = g.nodes.get_mut(node) else {
            return;
        };
        match sent {
            Some(t) => {
                let rtt = t.elapsed();
                n.rtt_ms = rtt.as_millis() as u32;
                n.skew_ms = st.host_time_ms - (now_ms() - (rtt.as_millis() as i64) / 2);
            }
            None => n.skew_ms = st.host_time_ms - now_ms(),
        }
        n.last_ok = Some(Instant::now());
        n.first_fail = None;
        n.full = false;
        n.error.clear();
        if n.health != Health::Up {
            tracing::info!(%node, "host is up");
        }
        n.health = Health::Up;
        n.status = Some(st);
    }

    async fn poll(self: Arc<Self>, node: String, stop: CancellationToken) {
        let mut fails = 0u32;
        loop {
            if stop.is_cancelled() {
                return;
            }
            let (link, cursor, wait) = {
                let g = self.sched.lock().expect("sched");
                let Some(n) = g.nodes.get(&node) else { return };
                let wait = if n.health == Health::Up && n.fences.is_empty() {
                    self.cfg.poll_wait
                } else {
                    Duration::ZERO
                };
                (n.link.clone(), n.row.cursor, wait)
            };
            let release: Vec<pb::HostJobRef> = self
                .sched
                .lock()
                .expect("sched")
                .releases
                .remove(&node)
                .unwrap_or_default()
                .into_iter()
                .map(|(id, attempt)| pb::HostJobRef {
                    job_id: host_job_id(&self.cluster_id, id),
                    attempt,
                })
                .collect();
            // A barrier to release answers at once (the wait would hold it).
            let wait = if release.is_empty() {
                wait
            } else {
                Duration::ZERO
            };
            let started = Instant::now();
            let req = pb::ListHostJobsRequest {
                after_seq: cursor,
                limit: 1000,
                wait_ms: wait.as_millis() as u32,
                without_output: false,
                release,
            };
            let r = tokio::select! {
                _ = stop.cancelled() => return,
                r = link.list(req) => r,
            };
            match r {
                Ok(resp) => {
                    fails = 0;
                    let more = resp.more;
                    self.on_feed(&node, resp).await;
                    if !more {
                        // Answered at once (a feed with news): no busy loop.
                        if started.elapsed() < Duration::from_millis(20) {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    }
                }
                Err(e) => {
                    fails += 1;
                    self.on_unreachable(&node, e).await;
                    let backoff = Duration::from_millis(250 * u64::from(fails.min(8)));
                    tokio::select! {
                        _ = stop.cancelled() => return,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                }
            }
        }
    }

    async fn on_feed(&self, node: &str, resp: pb::ListHostJobsResponse) {
        let mut changes = Vec::new();
        let mut cancels = Vec::new();
        let fences: Vec<(i64, u32)>;
        let link;
        let mut reset = false;
        'feed: {
            let mut g = self.sched.lock().expect("sched");
            let Some(st) = resp.node.clone() else { return };
            let Some(n) = g.nodes.get_mut(node) else {
                return;
            };
            link = n.link.clone();
            let ledger_changed = !n.row.ledger_id.is_empty() && n.row.ledger_id != st.ledger_id;
            let fresh = n.row.ledger_id.is_empty();
            if ledger_changed || fresh {
                if ledger_changed {
                    tracing::warn!(%node, "the host's job ledger is new (its disk was wiped?): reconciling its jobs");
                }
                n.row.ledger_id = st.ledger_id.clone();
                n.row.cursor = 0;
                changes.push(Change::Cursor {
                    node: node.to_string(),
                    seq: 0,
                    ledger_id: st.ledger_id.clone(),
                });
                if ledger_changed {
                    // Every job there: look it up now.
                    let now = Instant::now();
                    for f in g.inflight.values_mut().filter(|f| f.node == node) {
                        f.deadline = now;
                    }
                    self.heard(&mut g, node, st, None);
                    reset = true;
                    fences = vec![];
                    break 'feed;
                }
            }
            let host_now_ms = st.host_time_ms;
            self.heard(&mut g, node, st, None);
            for h in &resp.jobs {
                self.on_record(&mut g, node, h, &mut changes, &mut cancels);
            }
            let heard = Instant::now();
            for p in &resp.progress {
                let Some(id) = self.job_of(&p.job_id) else {
                    continue;
                };
                if g.inflight
                    .get(&id)
                    .is_some_and(|f| f.node == node && f.attempt == p.attempt)
                {
                    g.progress.insert(
                        id,
                        Progress {
                            attempt: p.attempt,
                            ready: p.ready,
                            released: p.released,
                            last_progress_ms: p.last_progress_ms,
                            step: p.step,
                            host_now_ms,
                            heard,
                        },
                    );
                }
            }
            let n = g.nodes.get_mut(node).expect("node");
            if resp.next_seq < n.row.cursor {
                // The host's numbering is behind our cursor (a ledger from
                // before numbering was kept): follow it back.
                tracing::warn!(%node, cursor = n.row.cursor, host = resp.next_seq, "the host's job feed is behind our cursor: resetting it");
                n.row.cursor = resp.next_seq;
                changes.push(Change::Cursor {
                    node: node.to_string(),
                    seq: resp.next_seq,
                    ledger_id: n.row.ledger_id.clone(),
                });
            } else if resp.next_seq > n.row.cursor {
                n.row.cursor = resp.next_seq;
                changes.push(Change::Cursor {
                    node: node.to_string(),
                    seq: resp.next_seq,
                    ledger_id: n.row.ledger_id.clone(),
                });
            }
            fences = n.fences.iter().map(|(k, v)| (*k, *v)).collect();
        }
        self.finish(changes, cancels).await;
        if reset {
            return;
        }
        // Deliver the fences of attempts moved off it while it was gone.
        for (id, attempt) in fences {
            Metrics::inc(&self.metrics.fences);
            let r = link
                .cancel(pb::CancelHostJobRequest {
                    job_id: host_job_id(&self.cluster_id, id),
                    attempt,
                    reason: "this attempt ran elsewhere while this host was unreachable".into(),
                })
                .await;
            if r.is_ok() {
                self.sched
                    .lock()
                    .expect("sched")
                    .nodes
                    .get_mut(node)
                    .map(|n| n.fences.remove(&id));
                self.persist(vec![Change::FenceDone {
                    node: node.to_string(),
                    id,
                }])
                .await;
            }
        }
    }

    async fn on_unreachable(&self, node: &str, e: LinkError) {
        let mut changes = Vec::new();
        {
            let mut g = self.sched.lock().expect("sched");
            let down_after = self.cfg.node_down_after;
            let Some(n) = g.nodes.get_mut(node) else {
                return;
            };
            n.error = e.to_string();
            let now = Instant::now();
            let since = *n.first_fail.get_or_insert(now);
            let silent = now - n.last_ok.unwrap_or(since);
            if n.health == Health::Down || silent < down_after {
                return;
            }
            tracing::warn!(%node, "host down (no answer for {silent:?}): its jobs run elsewhere");
            n.health = Health::Down;
            Metrics::inc(&self.metrics.node_down);
            let ids: Vec<i64> = g
                .inflight
                .iter()
                .filter(|(_, f)| f.node == node)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                let f = g.inflight.remove(&id).expect("flight");
                if f.user_cancel {
                    g.land(&f);
                    changes.push(Change::Complete {
                        id,
                        attempt: f.attempt,
                        state: state::CANCELED.into(),
                        exit_code: None,
                        timed_out: false,
                        error: format!("canceled; {node} went down"),
                        stdout: vec![],
                        stderr: vec![],
                        result: vec![],
                        space: String::new(),
                        create_ms: 0,
                        run_ms: 0,
                        delete_ms: 0,
                        at_ms: now_ms(),
                        ended_ms: 0,
                    });
                    continue;
                }
                Metrics::inc(&self.metrics.retries);
                let attempt = f.attempt;
                self.give_up(
                    &mut g,
                    id,
                    f,
                    format!("{node} went down"),
                    true,
                    true,
                    &mut changes,
                );
                changes.push(Change::FenceAdd {
                    node: node.to_string(),
                    id,
                    attempt,
                });
                if let Some(n) = g.nodes.get_mut(node) {
                    n.fences.insert(id, attempt);
                }
            }
        }
        self.finish(changes, vec![]).await;
    }

    // ---- leases -------------------------------------------------------------

    async fn leases(self: Arc<Self>) {
        let gate = Arc::new(tokio::sync::Semaphore::new(RECONCILE_AT_ONCE));
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
            self.gang_watch().await;
            let due: Vec<(i64, String, u32, Arc<dyn NodeLink>)> = {
                let mut g = self.sched.lock().expect("sched");
                let now = Instant::now();
                let up: HashMap<String, Arc<dyn NodeLink>> = g
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.health == Health::Up)
                    .map(|(k, n)| (k.clone(), n.link.clone()))
                    .collect();
                g.inflight
                    .iter_mut()
                    .filter(|(_, f)| !f.reconciling && f.deadline <= now)
                    .filter_map(|(id, f)| {
                        let link = up.get(&f.node)?.clone();
                        f.reconciling = true;
                        Some((*id, f.node.clone(), f.attempt, link))
                    })
                    .take(1024)
                    .collect()
            };
            for (id, node, attempt, link) in due {
                let Ok(permit) = gate.clone().acquire_owned().await else {
                    return;
                };
                let me = self.clone();
                tokio::spawn(async move {
                    me.reconcile(id, node, attempt, link).await;
                    drop(permit);
                });
            }
        }
    }

    /// A lease ran out: what does the host say about the job?
    async fn reconcile(&self, id: i64, node: String, attempt: u32, link: Arc<dyn NodeLink>) {
        Metrics::inc(&self.metrics.reconciles);
        let job_id = host_job_id(&self.cluster_id, id);
        let r = link.get(&job_id).await;
        let mut changes = Vec::new();
        let mut cancels = Vec::new();
        let mut fence = false;
        {
            let mut g = self.sched.lock().expect("sched");
            match g.inflight.get_mut(&id) {
                Some(f) if f.node == node && f.attempt == attempt => f.reconciling = false,
                _ => return,
            }
            match r {
                Ok(resp) => {
                    if let Some(st) = resp.node {
                        self.heard(&mut g, &node, st, None);
                    }
                    let job = resp.job.unwrap_or_default();
                    let st = HostJobState::parse(&job.state);
                    if job.attempt == attempt && st.is_some_and(|s| s.is_terminal()) {
                        self.on_record(&mut g, &node, &job, &mut changes, &mut cancels);
                    } else if job.attempt == attempt {
                        let f = g.inflight.get_mut(&id).expect("flight");
                        let limit = Duration::from_secs(u64::from(f.spec.timeout_secs))
                            + self.cfg.run_grace;
                        let started = f.started;
                        if f.phase == Phase::Dispatched {
                            self.on_record(&mut g, &node, &job, &mut changes, &mut cancels);
                        } else if let Some(t) = started.filter(|t| t.elapsed() < limit) {
                            // Still within its time: renew the lease.
                            f.deadline = t + limit;
                        } else {
                            // Running far past its timeout: stuck. Stop it
                            // and run it again.
                            let f = g.inflight.remove(&id).expect("flight");
                            cancels.push((node.clone(), id, attempt));
                            Metrics::inc(&self.metrics.retries);
                            self.give_up(
                                &mut g,
                                id,
                                f,
                                format!("{node}: ran far past its timeout"),
                                true,
                                true,
                                &mut changes,
                            );
                        }
                    } else {
                        // The host never got this attempt (it has an older
                        // one, or a fence).
                        fence = true;
                    }
                }
                Err(LinkError::NotFound(_)) => fence = true,
                Err(LinkError::Unreachable(_)) => {
                    if let Some(f) = g.inflight.get_mut(&id) {
                        f.deadline = Instant::now() + Duration::from_secs(2);
                    }
                }
                Err(LinkError::Refused(e)) => {
                    let f = g.inflight.remove(&id).expect("flight");
                    self.give_up(
                        &mut g,
                        id,
                        f,
                        format!("{node}: {e}"),
                        false,
                        false,
                        &mut changes,
                    );
                }
            }
        }
        if fence {
            // Fence the attempt on the host first, so a late delivery of
            // it is refused there, then run the job again.
            let fenced = link
                .cancel(pb::CancelHostJobRequest {
                    job_id: job_id.clone(),
                    attempt,
                    reason: "this attempt never arrived; it runs again".into(),
                })
                .await;
            Metrics::inc(&self.metrics.fences);
            let mut g = self.sched.lock().expect("sched");
            if let Some(f) = g.inflight.get(&id)
                && f.node == node
                && f.attempt == attempt
            {
                match fenced {
                    Ok(c)
                        if c.job.as_ref().is_some_and(|j| {
                            j.attempt == attempt
                                && j.state != HostJobState::Canceled.as_str()
                                && j.accepted_at_ms > 0
                        }) =>
                    {
                        // It did arrive meanwhile: the feed reports it.
                        let f = g.inflight.get_mut(&id).expect("flight");
                        f.deadline = Instant::now() + self.cfg.ack_grace;
                    }
                    Ok(_) => {
                        let f = g.inflight.remove(&id).expect("flight");
                        self.give_up(
                            &mut g,
                            id,
                            f,
                            format!("{node} never received it"),
                            false,
                            false,
                            &mut changes,
                        );
                    }
                    Err(_) => {
                        let f = g.inflight.get_mut(&id).expect("flight");
                        f.deadline = Instant::now() + Duration::from_secs(2);
                    }
                }
            }
        }
        self.finish(changes, cancels).await;
    }

    // ---- membership -------------------------------------------------------

    /// Adds (or updates) a host and starts hearing from it.
    pub async fn add_node(&self, row: NodeRow) -> Result<(), CoordError> {
        self.persist_ops(vec![Op::Node { row: row.clone() }])
            .await
            .map_err(CoordError::Store)?;
        let mut g = self.sched.lock().expect("sched");
        let keep = g.nodes.remove(&row.name);
        let mut rt = self.node_rt(row.clone());
        if let Some(old) = keep {
            old.stop.cancel();
            rt.fences = old.fences;
            if old.row.ledger_id == row.ledger_id || row.ledger_id.is_empty() {
                rt.row.cursor = old.row.cursor;
                rt.row.ledger_id = old.row.ledger_id;
            }
        }
        g.nodes.insert(row.name.clone(), rt);
        self.spawn_poller(&mut g, &row.name);
        drop(g);
        self.wake.notify_one();
        Ok(())
    }

    /// Stops (or resumes) sending new jobs to a host; its jobs finish.
    pub async fn drain(&self, name: &str, drain: bool) -> Result<(), CoordError> {
        let state = if drain { "draining" } else { "ready" };
        let found = self.sched.lock().expect("sched").nodes.contains_key(name);
        if found {
            self.persist_ops(vec![Op::NodeState {
                name: name.to_string(),
                state: state.to_string(),
            }])
            .await
            .map_err(CoordError::Store)?;
        }
        if !found {
            return Err(CoordError::NotFound(format!(
                "no host {name} in the cluster"
            )));
        }
        if let Some(n) = self.sched.lock().expect("sched").nodes.get_mut(name) {
            n.row.state = state.into();
        }
        self.wake.notify_one();
        Ok(())
    }

    /// Removes a host: its jobs in flight run elsewhere (and are fenced
    /// there, best effort).
    pub async fn remove_node(&self, name: &str) -> Result<(), CoordError> {
        let mut changes = Vec::new();
        let (rt, flights) = {
            let mut g = self.sched.lock().expect("sched");
            let Some(rt) = g.nodes.remove(name) else {
                return Err(CoordError::NotFound(format!(
                    "no host {name} in the cluster"
                )));
            };
            rt.stop.cancel();
            let ids: Vec<i64> = g
                .inflight
                .iter()
                .filter(|(_, f)| f.node == name)
                .map(|(id, _)| *id)
                .collect();
            let mut flights = Vec::new();
            for id in ids {
                let f = g.inflight.remove(&id).expect("flight");
                flights.push((id, f.attempt));
                self.give_up(
                    &mut g,
                    id,
                    f,
                    format!("{name} was removed from the cluster"),
                    false,
                    true,
                    &mut changes,
                );
            }
            (rt, flights)
        };
        self.finish(changes, vec![]).await;
        for (id, attempt) in flights {
            let _ = rt
                .link
                .cancel(pb::CancelHostJobRequest {
                    job_id: host_job_id(&self.cluster_id, id),
                    attempt,
                    reason: "this host was removed from the cluster".into(),
                })
                .await;
        }
        self.persist_ops(vec![Op::NodeDelete {
            name: name.to_string(),
        }])
        .await
        .map_err(CoordError::Store)?;
        self.wake.notify_one();
        Ok(())
    }

    /// Every host now.
    pub fn nodes(&self) -> Vec<NodeView> {
        let g = self.sched.lock().expect("sched");
        let mut assigned: HashMap<&str, u32> = HashMap::new();
        for f in g.inflight.values() {
            *assigned.entry(f.node.as_str()).or_default() += 1;
        }
        g.nodes
            .values()
            .map(|n| {
                let s = n.status.clone().unwrap_or_default();
                let health = match (n.health, n.row.state.as_str()) {
                    (Health::Down, _) => "down",
                    (Health::Unknown, _) => "unknown",
                    (Health::Up, "draining") => "draining",
                    (Health::Up, "pinned") => "own runs only",
                    (Health::Up, _) => "ready",
                };
                NodeView {
                    name: n.row.name.clone(),
                    via: n.row.via.clone(),
                    address: n.link.describe(),
                    health: health.into(),
                    os: n.row.os.clone(),
                    arch: s.arch.clone(),
                    kinds: s.kinds.clone(),
                    runtimes: s.runtimes.clone(),
                    millicpus_total: s.millicpus_total,
                    millicpus_free: s.millicpus_free,
                    memory_total_mb: s.memory_total_mb,
                    memory_free_mb: s.memory_free_mb,
                    spaces: s.running.values().sum(),
                    jobs_running: s.jobs_running,
                    jobs_max: s.jobs_max,
                    jobs_assigned: assigned.get(n.row.name.as_str()).copied().unwrap_or(0),
                    last_seen_secs: n.last_ok.map(|t| t.elapsed().as_secs()),
                    clock_skew_ms: n.skew_ms,
                    rtt_ms: n.rtt_ms,
                    epoch: s.epoch,
                    accelerators: s.accelerators.clone(),
                    dedicated: s.dedicated,
                    usd_per_hour: s.usd_per_hour,
                    addresses: s.addresses.clone(),
                    problem: if n.health == Health::Down {
                        // Say which way failed: a relay-only host is
                        // unreachable while its relay is.
                        format!(
                            "no answer {} for {} s: {}",
                            n.link.describe(),
                            n.first_fail.map_or(0, |t| t.elapsed().as_secs()),
                            n.error
                        )
                    } else if !s.problem.is_empty() {
                        s.problem.clone()
                    } else {
                        n.error.clone()
                    },
                }
            })
            .collect()
    }

    /// Every host's row.
    pub fn node_rows(&self) -> Vec<NodeRow> {
        let g = self.sched.lock().expect("sched");
        g.nodes.values().map(|n| n.row.clone()).collect()
    }

    /// Whether host `name` is here.
    pub fn has_node(&self, name: &str) -> bool {
        self.sched.lock().expect("sched").nodes.contains_key(name)
    }

    /// Whether a job is running on host `name` now.
    pub fn node_busy(&self, name: &str) -> bool {
        let g = self.sched.lock().expect("sched");
        g.inflight.values().any(|f| f.node == name)
    }

    /// Jobs waiting in memory and in flight.
    pub fn depth(&self) -> (usize, usize) {
        let g = self.sched.lock().expect("sched");
        (g.queued.len(), g.inflight.len())
    }

    /// The jobs that have waited at least `min_age` for a machine, by
    /// shape: what the elastic pool adds machines for (`pending`). Jobs
    /// pinned to one machine are left out (no new machine helps them).
    pub fn pending_demand(&self, min_age: Duration) -> Vec<serde_json::Value> {
        let g = self.sched.lock().expect("sched");
        // (image, kind, runtime, millicpus, MiB, os, arch, accelerator, cloud)
        type Shape = (
            String,
            String,
            String,
            u32,
            u64,
            String,
            String,
            String,
            String,
        );
        let mut by: std::collections::BTreeMap<Shape, (u32, u32)> = Default::default();
        // Gangs (`cua run`, compat runs) that wait for computers: each member
        // no computer can take is one more machine's worth of demand.
        for r in g.gangs.values() {
            if r.state != gang::gstate::QUEUED || r.placing || r.short == 0 {
                continue;
            }
            let cloud = cua_cluster::placement::cloud_pin(&r.spec.on).unwrap_or_default();
            if (!r.spec.on.is_empty() && cloud.is_empty()) || r.ready_at.elapsed() < min_age {
                continue;
            }
            let s = &r.spec;
            let e = by
                .entry((
                    s.image.clone(),
                    s.kind.clone(),
                    s.runtime.clone(),
                    s.millicpus,
                    s.memory_mb,
                    s.os.clone(),
                    s.arch.clone(),
                    s.accelerator.clone(),
                    cloud.to_string(),
                ))
                .or_default();
            e.0 += r.short;
            e.1 = e.1.max(s.timeout_secs);
        }
        for q in g.queued.values() {
            let cloud = cua_cluster::placement::cloud_pin(&q.spec.on).unwrap_or_default();
            if (!q.spec.on.is_empty() && cloud.is_empty()) || q.ready_at.elapsed() < min_age {
                continue;
            }
            let s = &q.spec;
            let e = by
                .entry((
                    s.image.clone(),
                    s.kind.clone(),
                    s.runtime.clone(),
                    s.millicpus,
                    s.memory_mb,
                    s.os.clone(),
                    s.arch.clone(),
                    s.accelerator.clone(),
                    cloud.to_string(),
                ))
                .or_default();
            e.0 += 1;
            e.1 = e.1.max(s.timeout_secs);
        }
        by.into_iter()
            .map(
                |(
                    (image, kind, runtime, millicpus, memory_mb, os, arch, accelerator, provider),
                    (count, timeout),
                )| {
                    serde_json::json!({
                        "image": image, "kind": kind, "runtime": runtime,
                        "millicpus": millicpus, "memory_mb": memory_mb, "os": os,
                        "arch": arch, "accelerator": accelerator, "count": count,
                        "timeout_secs": timeout, "provider": provider,
                    })
                },
            )
            .collect()
    }

    /// Creates an interactive Space on the best host that reaches it
    /// directly (`--on cluster`): the hosts are tried best first until one
    /// creates it.
    pub async fn place_space(&self, req: SpaceRequest) -> Result<PlacedSpace, CoordError> {
        self.place_space_excluding(req, &[]).await
    }

    /// [`Coordinator::place_space`] on any host but the `exclude`d ones.
    pub async fn place_space_excluding(
        &self,
        req: SpaceRequest,
        exclude: &[String],
    ) -> Result<PlacedSpace, CoordError> {
        let need = Need {
            private_image: cua_cluster::placement::image_is_private(&req.image),
            kind: cua_cluster::model::normalize_auto(&req.kind),
            runtime: cua_cluster::model::normalize_auto(&req.runtime),
            host_os: if req.image.to_ascii_lowercase().contains("macos") {
                "macos".into()
            } else {
                String::new()
            },
            arch: String::new(),
            on: String::new(),
            shape: cua_cluster::placement::warm_key(
                &req.image,
                &req.kind,
                &req.runtime,
                req.cpus,
                req.memory_mb,
            ),
            millicpus: if req.cpus == 0 { 2000 } else { req.cpus * 1000 },
            memory_mb: if req.memory_mb == 0 {
                4096
            } else {
                req.memory_mb
            },
            accelerator: String::new(),
            services: false,
        };
        // How the Space is reached: at its host's direct address, or
        // through the relay as a machine of its own (the account's).
        type Ways = Vec<(Arc<dyn NodeLink>, String)>;
        let (cands, links): (Vec<Candidate>, Ways) = {
            let g = self.sched.lock().expect("sched");
            g.nodes
                .values()
                .filter(|n| {
                    (!n.row.direct.is_empty() || !n.row.relay_machine.is_empty())
                        && !exclude.contains(&n.row.name)
                })
                .map(|n| {
                    (
                        Candidate {
                            name: n.row.name.clone(),
                            ready: n.health == Health::Up && n.row.state == "ready",
                            os: n.row.os.clone(),
                            status: n.status.clone(),
                            pending_millicpus: n.pending_cpu,
                            pending_memory_mb: n.pending_mem,
                            pending_jobs: n.pending_jobs,
                            elastic: false,
                            pinned: n.row.state == "pinned",
                            provider: n.row.provider.clone(),
                            ..Default::default()
                        },
                        (n.link.clone(), n.row.direct.clone()),
                    )
                })
                .unzip()
        };
        let order = placement::rank(&cands, &need, self.cfg.policy);
        if cands.is_empty() {
            let relayed = {
                let g = self.sched.lock().expect("sched");
                g.nodes.values().filter(|n| n.row.via != "provider").count()
            };
            return Err(CoordError::Unavailable(if relayed > 0 {
                format!(
                    "none of the {relayed} machine(s) here can hold a Space someone else \
                     reaches (this machine's own runner only runs jobs)"
                )
            } else {
                "none of your machines provides Spaces".into()
            }));
        }
        if order.is_empty() {
            return Err(CoordError::Unavailable(format!(
                "no host can take this Space: {}",
                placement::why_nothing_fits(&cands, &need)
            )));
        }
        let mut errors = Vec::new();
        for i in order {
            let (link, direct) = &links[i];
            let via_relay = direct.is_empty();
            let r = link
                .create_space(pb::CreateHostSpaceRequest {
                    image: req.image.clone(),
                    kind: need.kind.clone(),
                    runtime: need.runtime.clone(),
                    name: req.name.clone(),
                    cpus: req.cpus,
                    memory_mb: req.memory_mb,
                    disk_gb: 0,
                    attach: None,
                    direct: !via_relay,
                    create_key: String::new(),
                    fork_source: String::new(),
                    network: String::new(),
                    headless: req.headless,
                    // A cluster create carries no egress policy.
                    egress: None,
                    ..Default::default()
                })
                .await;
            match r {
                Ok(space) if via_relay && !space.relay_machine.is_empty() => {
                    return Ok(PlacedSpace {
                        node: cands[i].name.clone(),
                        id: format!("relay:{}", space.relay_machine),
                        token: String::new(),
                        local_space: space.local_space,
                        kind: space.kind,
                    });
                }
                Ok(_) if via_relay => {
                    errors.push(format!("{}: created no Space on the relay", cands[i].name))
                }
                Ok(space) if space.direct_port > 0 => {
                    let host = direct.rsplit_once(':').map_or(direct.as_str(), |(h, _)| h);
                    return Ok(PlacedSpace {
                        node: cands[i].name.clone(),
                        id: format!("direct:{host}:{}", space.direct_port),
                        token: space.direct_token,
                        local_space: space.local_space,
                        kind: space.kind,
                    });
                }
                Ok(_) => errors.push(format!("{}: created no direct port", cands[i].name)),
                Err(e) => errors.push(format!("{}: {e}", cands[i].name)),
            }
        }
        Err(CoordError::Unavailable(errors.join("; ")))
    }
}

/// Tells hosts to stop jobs, a few at a time.
async fn futures_cancel(jobs: Vec<(Arc<dyn NodeLink>, i64, u32)>, cluster: &str) {
    let gate = Arc::new(tokio::sync::Semaphore::new(32));
    let mut tasks = Vec::new();
    for (link, id, attempt) in jobs {
        let Ok(permit) = gate.clone().acquire_owned().await else {
            break;
        };
        let job_id = host_job_id(cluster, id);
        tasks.push(tokio::spawn(async move {
            let _ = link
                .cancel(pb::CancelHostJobRequest {
                    job_id,
                    attempt,
                    reason: "canceled by its owner".into(),
                })
                .await;
            drop(permit);
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
}

fn complete(id: i64, h: &pb::HostJob, state: &str) -> Change {
    Change::Complete {
        id,
        attempt: h.attempt,
        state: state.to_string(),
        exit_code: h.exit_code,
        timed_out: h.timed_out,
        error: h.error.clone(),
        stdout: h.stdout.clone(),
        stderr: h.stderr.clone(),
        result: h.result.clone(),
        space: h.space.clone(),
        create_ms: h.create_ms,
        run_ms: h.run_ms,
        delete_ms: h.delete_ms,
        at_ms: now_ms(),
        ended_ms: command_ended_ms(h),
    }
}

/// When a job's command ended by its host's clock (0: unknown). The host
/// stamps its record after deleting the job's Space, which takes seconds
/// for a VM, so that time is taken off.
fn command_ended_ms(h: &pb::HostJob) -> i64 {
    if h.finished_at_ms <= 0 {
        return 0;
    }
    (h.finished_at_ms - i64::from(h.delete_ms)).max(1)
}

fn default_link(row: &NodeRow) -> Arc<dyn NodeLink> {
    let mut ways: Vec<Arc<dyn NodeLink>> = Vec::new();
    if !row.direct.is_empty() {
        ways.push(Arc::new(GrpcLink::new(
            Target::Direct(row.direct.clone()),
            row.token.clone(),
        )));
    }
    if !row.relay_url.is_empty() && !row.relay_machine.is_empty() {
        // End to end encrypted and direct when possible; a relay or host
        // without the mesh falls through to the relay's HTTPS path.
        ways.push(Arc::new(GrpcLink::new(
            Target::Mesh {
                url: row.relay_url.clone(),
                machine: row.relay_machine.clone(),
            },
            row.token.clone(),
        )));
        ways.push(Arc::new(GrpcLink::new(
            Target::Relay {
                url: row.relay_url.clone(),
                machine: row.relay_machine.clone(),
            },
            row.token.clone(),
        )));
    }
    if ways.is_empty() {
        ways.push(Arc::new(GrpcLink::new(
            Target::Direct("127.0.0.1:1".into()),
            row.token.clone(),
        )));
    }
    if ways.len() == 1 {
        return ways.pop().expect("one way");
    }
    Arc::new(FailoverLink::new(ways))
}

/// The stored form of a secret token.
pub fn hash_token(token: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(token.trim().as_bytes()))
}

/// A host name as the cluster keeps it: letters, digits, `-` and `.`.
pub fn sanitize_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "host".into() } else { s }
}

/// What placement depends on, for passing over jobs of a shape that found
/// no host in a round.
type NeedKey = (
    String,
    String,
    String,
    String,
    String,
    u32,
    u64,
    bool,
    String,
    bool,
);

fn need_key(n: &Need) -> NeedKey {
    (
        n.kind.clone(),
        n.runtime.clone(),
        n.host_os.clone(),
        n.arch.clone(),
        n.on.clone(),
        n.millicpus,
        n.memory_mb,
        n.private_image,
        n.accelerator.clone(),
        n.services,
    )
}

/// What a job needs of its host.
fn need_of(spec: &JobSpec) -> Need {
    Need {
        private_image: cua_cluster::placement::image_is_private(&spec.image),
        kind: spec.kind.clone(),
        runtime: spec.runtime.clone(),
        host_os: spec.os.clone(),
        arch: spec.arch.clone(),
        on: spec.on.clone(),
        shape: cua_cluster::placement::warm_key(
            &spec.image,
            &spec.kind,
            &spec.runtime,
            spec.millicpus.div_ceil(1000),
            spec.memory_mb,
        ),
        millicpus: spec.millicpus,
        memory_mb: spec.memory_mb,
        accelerator: spec.accelerator.clone(),
        services: !spec.services.is_empty(),
    }
}

/// A gang member's end without a record from its host.
fn member_end(id: i64, attempt: u32, state: &str, error: String) -> Change {
    Change::Complete {
        id,
        attempt,
        state: state.to_string(),
        exit_code: None,
        timed_out: false,
        error,
        stdout: vec![],
        stderr: vec![],
        result: vec![],
        space: String::new(),
        create_ms: 0,
        run_ms: 0,
        delete_ms: 0,
        at_ms: now_ms(),
        ended_ms: 0,
    }
}

/// Jobs a dispatch round may pass over (their shape found no host) before
/// it stops: a bound on the work per round with a very long queue.
const SKIP_MAX: usize = 200_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_tidy() {
        assert_eq!(sanitize_name("Mac mini (spare)"), "mac-mini--spare");
        assert_eq!(sanitize_name("  "), "host");
        assert_eq!(sanitize_name("ip-172-31-1-5"), "ip-172-31-1-5");
    }
}
