//! The host half of cluster jobs: [`NodeJobs`] accepts jobs from a
//! scheduler, runs each in a fresh Space through a [`JobExecutor`], and
//! keeps a durable [ledger](crate::ledger) keyed by job id.
//!
//! - **Idempotent delivery.** A job id and attempt delivered again returns
//!   the recorded job (`duplicate`) instead of running it twice here.
//! - **Fencing.** An attempt lower than the newest one seen for the job is
//!   refused (`stale`); a higher one cancels a lower one still running. A
//!   cancel of a job this host never saw records a tombstone, so a late
//!   delivery of that attempt is refused too.
//! - **Accept or reject by own resources.** A job is accepted only when its
//!   CPU and memory reservation fit what is free (and the job cap allows
//!   it); otherwise it is `rejected` and the scheduler tries another host.
//! - **Crash recovery.** Opening the runner again (the daemon restarted)
//!   ends every job the ledger says was running with `error` and deletes
//!   its Space.
//! - **Change feed.** [`NodeJobs::list`] returns what changed after a
//!   sequence number, holding the call (long poll) until something does;
//!   every answer carries [`NodeJobs::status`], the node's heartbeat, and
//!   each running job's start barrier and progress
//!   ([`JobExecutor::progress`]). A feed call releases the start barriers
//!   its scheduler names ([`NodeJobs::release`]).
//! - **Self-fence.** A gang member whose scheduler has not called the feed
//!   for [`NodeJobs::set_self_fence_after`] (default 60 s) is stopped here:
//!   a host cut off from its scheduler never runs a member next to the
//!   attempt that replaced it ([`FENCED`]).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_proto::env::v1 as pb;
use tokio_util::sync::CancellationToken;

use crate::ledger::{self, JobRecord, StateWord, Writer};
use crate::model::{HostJobState, JobSpec, MAX_BATCH, now_ms, space_name, tail};

/// How a job's command ended, as the executor saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecEnd {
    /// It exited with this code.
    Exited(i32),
    /// It ran out of time and was killed.
    TimedOut,
    /// The host could not run it (the Space did not start, ...).
    Error(String),
    /// It was canceled.
    Canceled,
}

/// What [`JobExecutor::run`] reports.
#[derive(Clone, Debug)]
pub struct ExecOutcome {
    /// How it ended.
    pub end: ExecEnd,
    /// stdout (the runner keeps the tail).
    pub stdout: Vec<u8>,
    /// stderr.
    pub stderr: Vec<u8>,
    /// The result file, when asked for and present.
    pub result: Vec<u8>,
    /// Why the result file is missing, when asked for.
    pub result_error: Option<String>,
    /// Space create time.
    pub create_ms: u32,
    /// Command time.
    pub run_ms: u32,
    /// Space delete time.
    pub delete_ms: u32,
}

impl ExecOutcome {
    /// An outcome with only an end.
    pub fn ended(end: ExecEnd) -> Self {
        Self {
            end,
            stdout: vec![],
            stderr: vec![],
            result: vec![],
            result_error: None,
            create_ms: 0,
            run_ms: 0,
            delete_ms: 0,
        }
    }
}

/// What kinds and runtimes an executor creates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offers {
    /// `container`, `vm`, `microvm`.
    pub kinds: Vec<String>,
    /// `gvisor`, `runc`, `qemu`, `lume`, `firecracker`, ...
    pub runtimes: Vec<String>,
}

/// Runs one job member in a Space: a fresh one (created, the command run,
/// the output and result file collected, deleted whatever happened) or a
/// dedicated host's own ([`crate::machine`]). Implemented over Spaces by
/// the host's daemon (`cua_spaces::host_jobs`), and by fakes in tests.
#[async_trait]
pub trait JobExecutor: Send + Sync + 'static {
    /// Runs `job` in a new Space named `space`. On `cancel` it stops the
    /// command, deletes the Space and returns [`ExecEnd::Canceled`].
    async fn run(&self, job: &JobSpec, space: &str, cancel: CancellationToken) -> ExecOutcome;

    /// Deletes Space `space` if it exists (one a crash left behind).
    async fn remove_space(&self, space: &str);

    /// What this executor creates. Asked each time the host reports its
    /// status or admits a job (so a runtime installed while it runs is
    /// offered): keep it cheap.
    fn offers(&self) -> Offers;

    /// The kind `spec` runs as here: its own, or, when it names none, the
    /// one this host picks for it (a dedicated host's own Space, a macOS VM
    /// for a macOS image, a container). Asked when the job is admitted;
    /// the executor then sees the job with this kind.
    fn kind_of(&self, spec: &JobSpec) -> String {
        if spec.kind.is_empty() {
            "container".into()
        } else {
            spec.kind.clone()
        }
    }

    /// Most members of `kind` this host runs at once now, when it is
    /// limited beyond CPU and memory (two macOS VMs per Mac, counting the
    /// ones it runs for anything else). Asked when a job is admitted: keep
    /// it cheap.
    fn kind_limit(&self, _kind: &str) -> Option<(u32, String)> {
        None
    }

    /// A job attempt's live output from byte `from` (a bounded chunk) and
    /// its size so far, when this executor keeps an output log.
    fn output(&self, _job_id: &str, _attempt: u32, _from: u64) -> Option<(Vec<u8>, u64)> {
        None
    }

    /// A running job attempt's start barrier and progress, when it reports
    /// them (members using the cua jobs runtime, or writing output). Asked
    /// on every feed call for each running job: keep it cheap.
    fn progress(&self, _job_id: &str, _attempt: u32) -> Option<pb::HostJobProgress> {
        None
    }

    /// Releases a running job attempt's start barrier (every member of its
    /// gang is ready). Called again until [`JobExecutor::progress`] says
    /// `released`: it must be idempotent.
    fn release(&self, _job_id: &str, _attempt: u32) {}

    /// How long stopping a job takes at most once canceled (a graceful
    /// signal's grace period).
    fn stop_grace(&self) -> Duration {
        Duration::ZERO
    }
}

/// What a gang member the host stopped itself (its scheduler went silent)
/// says first in its error.
pub const FENCED: &str = "fenced: ";

/// How long a host waits without a feed call from a gang member's
/// scheduler before it stops the member, unless `CUA_SELF_FENCE_AFTER_SECS`
/// says otherwise (0: never).
pub const SELF_FENCE_AFTER: Duration = Duration::from_secs(60);

fn self_fence_default() -> Duration {
    std::env::var("CUA_SELF_FENCE_AFTER_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(SELF_FENCE_AFTER, Duration::from_secs)
}

/// Whether a job is a gang member (the coordinator gives each its rank and
/// peers).
fn is_gang_member(spec: &JobSpec) -> bool {
    spec.env.contains_key("CUA_RANK") && spec.env.contains_key("CUA_PEERS")
}

/// The scheduler a job id belongs to on a shared host: `<owner>/` for an
/// account's jobs ([`NodeJobs::list_for`]), empty for the host's own.
fn owner_key(job_id: &str) -> &str {
    job_id.find('/').map_or("", |i| &job_id[..=i])
}

/// Capacity reserved outside the runner: the Spaces this host provides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct External {
    /// CPU reserved (thousandths).
    pub millicpus: u32,
    /// Memory reserved (MiB).
    pub memory_mb: u64,
    /// Running Spaces by kind.
    pub running: BTreeMap<String, u32>,
}

/// How a [`NodeJobs`] runs.
#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Ledger directory (`~/.cua/host/jobs`).
    pub dir: PathBuf,
    /// CPU offered to jobs and Spaces (thousandths).
    pub millicpus_total: u32,
    /// Memory offered (MiB).
    pub memory_total_mb: u64,
    /// Most jobs at once (0: CPU and memory decide).
    pub jobs_max: u32,
    /// Refuse a job while the machine has less memory than this actually
    /// available (MiB; 0: no check). What jobs reserve is a promise, not
    /// what they use: a host that overcommits its reservations
    /// (`CUA_HOST_JOBS_OVERCOMMIT`) keeps this floor so it never runs out.
    pub min_available_mb: u64,
    /// Ended jobs older than this are forgotten (and their fencing).
    pub retention: Duration,
    /// Ended jobs keep their output in memory this long (the ledger keeps
    /// it until the retention).
    pub output_retention: Duration,
    /// This start of the daemon.
    pub boot_id: String,
}

impl NodeConfig {
    /// This machine's capacity, with `CUA_HOST_JOBS_MILLICPUS`,
    /// `CUA_HOST_JOBS_MEMORY_MB` and `CUA_HOST_JOBS_MAX` overriding it.
    ///
    /// `CUA_HOST_JOBS_OVERCOMMIT` (a factor, default 1) lets reservations
    /// add up to more than the machine: a job reserves its memory limit,
    /// and a browser Space under a 1.5 GiB limit typically uses a third of
    /// it. With a factor above 1 the host also refuses jobs while less than
    /// an eighth of its memory (at least 1 GiB) is actually available.
    pub fn detect(dir: impl Into<PathBuf>) -> Self {
        let overcommit = std::env::var("CUA_HOST_JOBS_OVERCOMMIT")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|f| f.is_finite() && *f >= 1.0)
            .unwrap_or(1.0);
        let mut cfg = Self::detect_exact(dir);
        cfg.apply_overcommit(overcommit);
        cfg
    }

    /// Scales the reservable CPU and memory by `factor` (at least 1) and,
    /// when it is above 1, sets the available-memory floor.
    pub fn apply_overcommit(&mut self, factor: f64) {
        if factor <= 1.0 {
            return;
        }
        let total = crate::sysinfo::total_memory_mb().unwrap_or(self.memory_total_mb);
        self.min_available_mb = (total / 8).max(1024);
        self.memory_total_mb = (self.memory_total_mb as f64 * factor) as u64;
        self.millicpus_total =
            (f64::from(self.millicpus_total) * factor).min(f64::from(u32::MAX)) as u32;
    }

    fn detect_exact(dir: impl Into<PathBuf>) -> Self {
        let env_u64 = |k: &str| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
        };
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1);
        let mem = crate::sysinfo::total_memory_mb().unwrap_or(4096);
        // Keep some memory for the host itself.
        let reserve = (mem / 8).min(2048);
        Self {
            dir: dir.into(),
            millicpus_total: env_u64("CUA_HOST_JOBS_MILLICPUS")
                .map(|v| v as u32)
                .unwrap_or(cpus * 1000),
            memory_total_mb: env_u64("CUA_HOST_JOBS_MEMORY_MB").unwrap_or(mem - reserve),
            jobs_max: env_u64("CUA_HOST_JOBS_MAX").map(|v| v as u32).unwrap_or(0),
            min_available_mb: 0,
            retention: Duration::from_secs(24 * 3600),
            output_retention: Duration::from_secs(3600),
            boot_id: format!("{:016x}", rand::random::<u64>()),
        }
    }
}

type ExternalFn = Arc<dyn Fn() -> External + Send + Sync>;

/// Frees capacity held for nobody, given the CPU (thousandths) and memory
/// (MiB) a batch is short of; resolves once it is freed.
type ReclaimFn =
    Arc<dyn Fn(u32, u64) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync>;

struct RunHandle {
    attempt: u32,
    token: CancellationToken,
    /// A gang member: stopped here when its scheduler goes silent.
    gang: bool,
    started: Instant,
}

#[derive(Default)]
struct Inner {
    /// Live state of every job known (the source of fencing and capacity).
    jobs: HashMap<String, JobRecord>,
    /// Published (durable) records: the change feed.
    published: HashMap<String, Arc<JobRecord>>,
    /// Sequence number -> job, for the feed.
    feed: BTreeMap<u64, String>,
    /// The newest published sequence number.
    last_seq: u64,
    /// Running attempts, for cancels.
    runs: HashMap<String, RunHandle>,
    /// The last feed call of each scheduler ([`owner_key`]).
    contact: HashMap<String, Instant>,
    used_millicpus: u64,
    used_memory_mb: u64,
    running: u32,
    running_by_kind: BTreeMap<String, u32>,
    draining: bool,
    /// Jobs accepted in the last [`WARMUP`] and the memory each is expected
    /// to take once started: their Spaces may not have allocated it yet, so
    /// the available-memory floor counts it as already gone.
    warming: std::collections::VecDeque<(std::time::Instant, u64)>,
    /// `vm` jobs accepted in the last [`WARMUP`] and the VM memory each is
    /// expected to take ([`expected_vm_mb`]).
    vm_warming: std::collections::VecDeque<(std::time::Instant, u64)>,
}

/// Job Spaces a restarted runner removes at once.
const RECOVER_PARALLEL: usize = 32;

/// How often the host checks whether a gang member's scheduler went silent.
const SELF_FENCE_TICK: Duration = Duration::from_secs(1);

/// How long a newly accepted job counts against available memory before
/// its Space has started and shows up in what the machine reports.
const WARMUP: Duration = Duration::from_secs(90);

/// What one delivered job came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ack {
    /// Runs now.
    Accepted(JobRecord),
    /// Seen before: its record.
    Duplicate(JobRecord),
    /// No room or not allowed.
    Rejected(String),
    /// Older than an attempt seen, or fenced.
    Stale(String),
}

impl Ack {
    /// The wire form.
    pub fn to_pb(&self, job_id: &str, attempt: u32) -> pb::HostJobAck {
        let (outcome, reason, job) = match self {
            Ack::Accepted(r) => ("accepted", String::new(), Some(r.to_pb(false))),
            Ack::Duplicate(r) => ("duplicate", String::new(), Some(r.to_pb(true))),
            Ack::Rejected(why) => ("rejected", why.clone(), None),
            Ack::Stale(why) => ("stale", why.clone(), None),
        };
        pb::HostJobAck {
            job_id: job_id.to_string(),
            attempt,
            outcome: outcome.into(),
            reason,
            job,
        }
    }
}

/// Ready warm-pool members by [`crate::placement::warm_key`], as this
/// machine's warm pool reports them ([`set_warm_report`]).
type WarmFn = Arc<dyn Fn() -> std::collections::HashMap<String, u32> + Send + Sync>;

static WARM: Mutex<Option<WarmFn>> = Mutex::new(None);

/// Sets what [`NodeJobs::status`] reports as this machine's ready warm-pool
/// members (the daemon's warm pool calls it once).
pub fn set_warm_report(
    f: impl Fn() -> std::collections::HashMap<String, u32> + Send + Sync + 'static,
) {
    *WARM.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

fn warm_report() -> std::collections::HashMap<String, u32> {
    let f = WARM.lock().unwrap_or_else(|e| e.into_inner()).clone();
    f.map(|f| f()).unwrap_or_default()
}

/// The host's job runner.
pub struct NodeJobs {
    cfg: NodeConfig,
    exec: Arc<dyn JobExecutor>,
    inner: Arc<Mutex<Inner>>,
    writer: Writer,
    changed: tokio::sync::watch::Sender<u64>,
    meta: ledger::Meta,
    external: Mutex<Option<ExternalFn>>,
    reclaim: Mutex<Option<ReclaimFn>>,
    problem: Mutex<Option<String>>,
    /// Job Spaces a restart left that are still being removed.
    recovering: std::sync::atomic::AtomicUsize,
    /// Gang members stop after this long without a feed call (zero: never).
    self_fence_after: Mutex<Duration>,
}

impl std::fmt::Debug for NodeJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeJobs")
            .field("dir", &self.cfg.dir)
            .finish()
    }
}

impl NodeJobs {
    /// Opens the runner on `cfg.dir`'s ledger: jobs it says were running
    /// end with `error` (their Spaces are deleted in the background).
    pub fn open(cfg: NodeConfig, exec: Arc<dyn JobExecutor>) -> std::io::Result<Arc<Self>> {
        let loaded = ledger::load(&cfg.dir, cfg.retention)?;
        let inner = Arc::new(Mutex::new(Inner::default()));
        let (changed, _) = tokio::sync::watch::channel(loaded.last_seq);
        let mut crashed = Vec::new();
        {
            let mut g = inner.lock().expect("jobs");
            g.last_seq = loaded.last_seq;
            for r in loaded.records {
                if r.state() == HostJobState::Running {
                    crashed.push(r.clone());
                }
                g.feed.insert(r.seq, r.job_id.clone());
                g.published.insert(r.job_id.clone(), Arc::new(r.clone()));
                g.jobs.insert(r.job_id.clone(), r);
            }
        }
        let publish_inner = inner.clone();
        let publish_changed = changed.clone();
        let publish: ledger::Publish = Arc::new(move |records, _failure| {
            let mut last = 0;
            {
                let mut g = publish_inner.lock().expect("jobs");
                for r in records {
                    last = last.max(r.seq);
                    if let Some(old) = g.published.get(&r.job_id).map(|p| p.seq) {
                        g.feed.remove(&old);
                    }
                    if let Some(live) = g.jobs.get_mut(&r.job_id)
                        && live.attempt == r.attempt
                        && live.state == r.state
                    {
                        live.seq = r.seq;
                        // The published copy now serves the output; the
                        // live one drops it only here, so no reader ever
                        // sees an ended job without its output.
                        if live.state().is_terminal() {
                            live.stdout.clear();
                            live.stderr.clear();
                            live.result.clear();
                        }
                    }
                    g.feed.insert(r.seq, r.job_id.clone());
                    g.published.insert(r.job_id.clone(), Arc::new(r));
                }
                g.last_seq = g.last_seq.max(last);
            }
            publish_changed.send_replace(last);
        });
        let writer = Writer::start(&cfg.dir, loaded.last_seq, publish)?;
        let me = Arc::new(Self {
            cfg,
            exec,
            inner,
            writer,
            changed,
            meta: loaded.meta,
            external: Mutex::new(None),
            reclaim: Mutex::new(None),
            problem: Mutex::new(None),
            recovering: std::sync::atomic::AtomicUsize::new(crashed.len()),
            self_fence_after: Mutex::new(self_fence_default()),
        });
        if !crashed.is_empty() {
            tracing::warn!(
                jobs = crashed.len(),
                "jobs were running when the job runner stopped; ending them with error"
            );
            let me2 = me.clone();
            tokio::spawn(async move { me2.recover(crashed).await });
        }
        let me3 = Arc::downgrade(&me);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let Some(me) = me3.upgrade() else { return };
                me.sweep();
            }
        });
        let me4 = Arc::downgrade(&me);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SELF_FENCE_TICK).await;
                let Some(me) = me4.upgrade() else { return };
                me.self_fence();
            }
        });
        Ok(me)
    }

    /// Gang members stop after `after` without a feed call from their
    /// scheduler (zero: never). Default [`SELF_FENCE_AFTER`] or
    /// `CUA_SELF_FENCE_AFTER_SECS`.
    pub fn set_self_fence_after(&self, after: Duration) {
        *self.self_fence_after.lock().expect("fence") = after;
    }

    fn fence_after(&self) -> Duration {
        *self.self_fence_after.lock().expect("fence")
    }

    /// Stops the gang members whose scheduler has been silent too long:
    /// it has surely given up on them and may run their next attempt.
    fn self_fence(&self) {
        let after = self.fence_after();
        if after.is_zero() {
            return;
        }
        let mut g = self.inner.lock().expect("jobs");
        let due: Vec<(String, u64)> = g
            .runs
            .iter()
            .filter(|(_, r)| r.gang && !r.token.is_cancelled())
            .filter_map(|(id, r)| {
                // Silent since its last feed call, or since it started when
                // there was none since.
                let last = g
                    .contact
                    .get(owner_key(id))
                    .map_or(r.started, |c| (*c).max(r.started));
                let silent = last.elapsed();
                (silent >= after).then(|| (id.clone(), silent.as_secs()))
            })
            .collect();
        for (id, secs) in due {
            tracing::warn!(job = %id, "no word from its scheduler for {secs} s: stopping this gang member");
            if let Some(l) = g.jobs.get_mut(&id) {
                l.error = format!(
                    "{FENCED}no word from its scheduler for {secs} s; stopped so the attempt that replaces it runs alone"
                );
            }
            if let Some(r) = g.runs.get(&id) {
                r.token.cancel();
            }
        }
    }

    /// Releases the start barriers a scheduler names (`owner`: its account
    /// on a shared host): only attempts running here.
    pub fn release(&self, owner: Option<&str>, refs: &[pb::HostJobRef]) {
        if refs.is_empty() {
            return;
        }
        let live: Vec<(String, u32)> = {
            let g = self.inner.lock().expect("jobs");
            refs.iter()
                .map(|r| match owner {
                    Some(o) => (Self::owned_id(o, &r.job_id), r.attempt),
                    None => (r.job_id.clone(), r.attempt),
                })
                .filter(|(id, a)| g.runs.get(id).is_some_and(|run| run.attempt == *a))
                .collect()
        };
        for (id, attempt) in live {
            self.exec.release(&id, attempt);
        }
    }

    /// The start barrier and progress of the running jobs under `prefix`
    /// (ids without it).
    fn progress_of(&self, prefix: Option<&str>) -> Vec<pb::HostJobProgress> {
        let running: Vec<(String, u32)> = {
            let g = self.inner.lock().expect("jobs");
            g.runs
                .iter()
                .filter(|(id, _)| prefix.is_none_or(|p| id.starts_with(p)))
                .map(|(id, r)| (id.clone(), r.attempt))
                .collect()
        };
        let mut out: Vec<pb::HostJobProgress> = running
            .into_iter()
            .filter_map(|(id, attempt)| {
                let mut p = self.exec.progress(&id, attempt)?;
                p.job_id = match prefix {
                    Some(pre) => id.strip_prefix(pre).unwrap_or(&id).to_string(),
                    None => id,
                };
                p.attempt = attempt;
                Some(p)
            })
            .collect();
        out.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        out
    }

    /// Reports reservations made outside the runner (provided Spaces).
    pub fn set_external(&self, f: impl Fn() -> External + Send + Sync + 'static) {
        *self.external.lock().expect("external") = Some(Arc::new(f));
    }

    /// Sets what frees capacity held for nobody (idle warm-pool members,
    /// Spaces whose lifetime passed). A batch that would be refused for CPU
    /// or memory calls it first, with what it is short of, and then looks
    /// again: reclaiming comes before refusing.
    pub fn set_reclaim<F, Fut>(&self, f: F)
    where
        F: Fn(u32, u64) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        *self.reclaim.lock().expect("reclaim") = Some(Arc::new(move |c, m| Box::pin(f(c, m))));
    }

    /// What `specs` together are short of on this host: CPU (thousandths)
    /// and memory (MiB), counting reservations only (what a refusal would
    /// say), and the memory the available-memory floor wants beyond what
    /// the machine has.
    fn shortfall(&self, specs: &[JobSpec]) -> (u32, u64) {
        let ext = self.external();
        let g = self.inner.lock().expect("jobs");
        let want_cpu: u64 = specs
            .iter()
            .filter(|s| s.invalid().is_none())
            .map(|s| u64::from(s.millicpus))
            .sum();
        let want_mem: u64 = specs
            .iter()
            .filter(|s| s.invalid().is_none())
            .map(|s| s.memory_mb)
            .sum();
        let cpu = (g.used_millicpus + u64::from(ext.millicpus) + want_cpu)
            .saturating_sub(u64::from(self.cfg.millicpus_total));
        let mut mem =
            (g.used_memory_mb + ext.memory_mb + want_mem).saturating_sub(self.cfg.memory_total_mb);
        if self.cfg.min_available_mb > 0
            && let Some(avail) = crate::sysinfo::available_memory_mb()
        {
            let need = self.cfg.min_available_mb + self.expected_mb(want_mem);
            mem = mem.max(need.saturating_sub(avail));
        }
        (cpu.min(u64::from(u32::MAX)) as u32, mem)
    }

    /// Why this host takes no jobs (Spaces off), or `None`.
    pub fn set_problem(&self, problem: Option<String>) {
        *self.problem.lock().expect("problem") = problem;
    }

    /// Takes no new jobs while set.
    pub fn set_draining(&self, draining: bool) {
        self.inner.lock().expect("jobs").draining = draining;
    }

    fn external(&self) -> External {
        let f = self.external.lock().expect("external").clone();
        f.map(|f| f()).unwrap_or_default()
    }

    fn problem(&self) -> Option<String> {
        let left = self.recovering.load(std::sync::atomic::Ordering::SeqCst);
        if left > 0 {
            // New Spaces on top of the ones a restart left would take the
            // memory those still hold: none until they are gone.
            return Some(format!(
                "removing {left} Spaces of jobs the restart interrupted"
            ));
        }
        self.problem
            .lock()
            .expect("problem")
            .clone()
            .or_else(|| self.writer.last_error())
    }

    async fn recover(self: Arc<Self>, crashed: Vec<JobRecord>) {
        use futures_util::StreamExt as _;
        // Many at once: a busy host restarts with hundreds of job Spaces,
        // and each delete takes seconds under load.
        let removed: Vec<JobRecord> = futures_util::stream::iter(crashed)
            .map(|r| {
                let me = self.clone();
                async move {
                    if !r.space.is_empty() {
                        me.exec.remove_space(&r.space).await;
                    }
                    me.recovering
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    r
                }
            })
            .buffer_unordered(RECOVER_PARALLEL)
            .collect()
            .await;
        let mut ended = Vec::new();
        for mut r in removed {
            r.state = StateWord(HostJobState::Error);
            r.error = "the host's job runner restarted during the job".into();
            r.finished_at_ms = now_ms();
            {
                let mut g = self.inner.lock().expect("jobs");
                if let Some(live) = g.jobs.get_mut(&r.job_id)
                    && live.attempt == r.attempt
                    && live.state() == HostJobState::Running
                {
                    *live = r.clone();
                } else {
                    continue;
                }
            }
            ended.push(r);
        }
        if !ended.is_empty() {
            let _ = self.writer.append(ended, false).await;
        }
    }

    /// Forgets ended jobs past the retention and drops output held in
    /// memory past the output retention.
    fn sweep(&self) {
        let now = now_ms();
        let forget = now - self.cfg.retention.as_millis() as i64;
        let strip = now - self.cfg.output_retention.as_millis() as i64;
        let mut g = self.inner.lock().expect("jobs");
        let old: Vec<String> = g
            .jobs
            .values()
            .filter(|r| {
                r.state().is_terminal() && r.finished_at_ms > 0 && r.finished_at_ms < forget
            })
            .map(|r| r.job_id.clone())
            .collect();
        for id in old {
            g.jobs.remove(&id);
            if let Some(p) = g.published.remove(&id) {
                g.feed.remove(&p.seq);
            }
        }
        let heavy: Vec<String> = g
            .published
            .values()
            .filter(|r| r.finished_at_ms > 0 && r.finished_at_ms < strip && r.output_bytes() > 0)
            .map(|r| r.job_id.clone())
            .collect();
        for id in heavy {
            if let Some(p) = g.published.get_mut(&id) {
                let mut r = (**p).clone();
                r.stdout.clear();
                r.stderr.clear();
                r.result.clear();
                *p = Arc::new(r);
            }
        }
    }

    /// This host now, as a scheduler sees it.
    pub fn status(&self) -> pb::HostNodeStatus {
        let ext = self.external();
        let offers = self.exec.offers();
        let g = self.inner.lock().expect("jobs");
        let mut running = ext.running.clone();
        for (k, n) in &g.running_by_kind {
            *running.entry(k.clone()).or_default() += n;
        }
        let used_cpu = g.used_millicpus + u64::from(ext.millicpus);
        let used_mem = g.used_memory_mb + ext.memory_mb;
        let vm_warming = vm_warming_mb(&g);
        let mut st = pb::HostNodeStatus {
            boot_id: self.cfg.boot_id.clone(),
            epoch: self.meta.epoch,
            host_time_ms: now_ms(),
            millicpus_total: self.cfg.millicpus_total,
            millicpus_free: u64::from(self.cfg.millicpus_total).saturating_sub(used_cpu) as u32,
            memory_total_mb: self.cfg.memory_total_mb,
            memory_free_mb: self
                .cfg
                .memory_total_mb
                .saturating_sub(used_mem)
                .min(self.floor_free_mb(&g)),
            running: running.into_iter().collect(),
            runtimes: offers.runtimes,
            jobs_running: g.running,
            jobs_max: self.cfg.jobs_max,
            ledger_id: self.meta.ledger_id.clone(),
            ledger_seq: g.last_seq,
            draining: g.draining,
            problem: self.problem().unwrap_or_default(),
            arch: std::env::consts::ARCH.into(),
            warm: warm_report().into_iter().collect(),
            dedicated: offers.kinds.iter().any(|k| k == crate::model::KIND_MACHINE),
            job_services: true,
            kinds: offers.kinds,
            accelerators: crate::sysinfo::accelerators(),
            addresses: crate::sysinfo::addresses(),
            usd_per_hour: crate::sysinfo::usd_per_hour(),
            self_fence_after_ms: {
                let after = self.fence_after();
                if after.is_zero() {
                    0
                } else {
                    // Members are gone within this long of the last call:
                    // the wait, a tick of the check, and stopping them.
                    (after + SELF_FENCE_TICK + self.exec.stop_grace())
                        .as_millis()
                        .min(u128::from(u32::MAX)) as u32
                }
            },
            // The host's daemon adds the checkpoints it holds.
            ..Default::default()
        };
        crate::sysinfo::fill_measured(&mut st);
        // VMs of jobs accepted here that have not started yet.
        if let Some(m) = st.measured.as_mut() {
            m.vm_memory_free_mb = m.vm_memory_free_mb.saturating_sub(vm_warming);
        }
        st
    }

    /// A job's live output from byte `from` and its size so far (`None`:
    /// no such job, or its executor keeps no output log).
    pub fn output(&self, job_id: &str, from: u64) -> Option<(Vec<u8>, u64)> {
        let attempt = self.inner.lock().expect("jobs").jobs.get(job_id)?.attempt;
        self.exec.output(job_id, attempt, from)
    }

    /// [`NodeJobs::output`] for `owner`.
    pub fn output_for(&self, owner: &str, job_id: &str, from: u64) -> Option<(Vec<u8>, u64)> {
        self.output(&Self::owned_id(owner, job_id), from)
    }

    /// Why `spec` does not fit now, if it does not.
    /// The memory a job of `memory_mb` reserved is expected to use: its
    /// share of the machine's physical memory when reservations are
    /// overcommitted, else all of it.
    fn expected_mb(&self, memory_mb: u64) -> u64 {
        let physical = crate::sysinfo::total_memory_mb().unwrap_or(self.cfg.memory_total_mb);
        if self.cfg.memory_total_mb > physical {
            memory_mb.saturating_mul(physical) / self.cfg.memory_total_mb.max(1)
        } else {
            memory_mb
        }
    }

    /// What the available-memory floor still lets jobs reserve (in
    /// reservation units, overcommit included), so a scheduler sees the
    /// host as full when the floor would refuse its next job instead of
    /// sending work to be refused. Unlimited without a floor.
    fn floor_free_mb(&self, g: &Inner) -> u64 {
        if self.cfg.min_available_mb == 0 {
            return u64::MAX;
        }
        let Some(avail) = crate::sysinfo::available_memory_mb() else {
            return u64::MAX;
        };
        let warming: u64 = g
            .warming
            .iter()
            .filter(|(at, _)| at.elapsed() < WARMUP)
            .map(|(_, mb)| *mb)
            .sum();
        let spare = avail.saturating_sub(self.cfg.min_available_mb.saturating_add(warming));
        // Back from expected use to what jobs reserve.
        let per = self.expected_mb(1024).max(1);
        spare.saturating_mul(1024) / per
    }

    fn no_room(&self, g: &Inner, ext: &External, spec: &JobSpec) -> Option<String> {
        if g.draining {
            return Some("this host is draining".into());
        }
        if let Some(p) = self.problem() {
            return Some(p);
        }
        let offers = self.exec.offers();
        if spec.kind == crate::model::KIND_PROCESS {
            return Some(crate::model::process_refused());
        }
        if spec.kind == crate::model::KIND_MACHINE && !offers.kinds.contains(&spec.kind) {
            return Some(
                "this host is not dedicated, so it offers no computer Space of its own (a \
                 machine created for this work, `cua cloud add aws`, is)"
                    .into(),
            );
        }
        if !spec.kind.is_empty() && !offers.kinds.contains(&spec.kind) {
            return Some(format!(
                "this host does not create {} Spaces (it offers {})",
                spec.kind,
                offers.kinds.join(", ")
            ));
        }
        if !spec.runtime.is_empty() && !offers.runtimes.contains(&spec.runtime) {
            return Some(format!(
                "this host does not offer the {} runtime (it offers {})",
                spec.runtime,
                offers.runtimes.join(", ")
            ));
        }
        let kind = self.exec.kind_of(spec);
        if let Some((max, why)) = self.exec.kind_limit(&kind) {
            let running = g.running_by_kind.get(&kind).copied().unwrap_or(0);
            if running >= max {
                return Some(why);
            }
        }
        if self.cfg.jobs_max > 0 && g.running >= self.cfg.jobs_max {
            return Some(format!(
                "this host runs its most jobs ({})",
                self.cfg.jobs_max
            ));
        }
        let (want_cpu, want_mem) = reservation(spec);
        let cpu = g.used_millicpus + u64::from(ext.millicpus) + want_cpu;
        if cpu > u64::from(self.cfg.millicpus_total) {
            return Some(format!(
                "not enough CPU free ({} of {} millicpus reserved)",
                g.used_millicpus + u64::from(ext.millicpus),
                self.cfg.millicpus_total
            ));
        }
        if spec.kind == "vm"
            && let Some(v) = crate::sysinfo::vm_capacity()
        {
            // Measured: what the host's VMs use, snapshot memory shared by
            // copy-on-write VMs counted once, against its VM memory cap.
            let expected = expected_vm_mb(spec.memory_mb, v.typical_mb);
            let free = v.memory_free_mb.saturating_sub(vm_warming_mb(g));
            if free < expected {
                return Some(format!(
                    "not enough VM memory free ({free} MiB; a VM here uses about {expected} MiB)"
                ));
            }
        }
        let mem = g.used_memory_mb + ext.memory_mb + want_mem;
        if mem > self.cfg.memory_total_mb {
            return Some(format!(
                "not enough memory free ({} of {} MiB reserved)",
                g.used_memory_mb + ext.memory_mb,
                self.cfg.memory_total_mb
            ));
        }
        // The system file table is shared by everything on the machine: a
        // job started into a nearly full one fails with ENFILE, and so does
        // everything else (git, ssh).
        if let Some(why) = crate::sysinfo::file_table_shortfall() {
            return Some(why);
        }
        if self.cfg.min_available_mb > 0
            && let Some(avail) = crate::sysinfo::available_memory_mb()
        {
            let warming: u64 = g
                .warming
                .iter()
                .filter(|(at, _)| at.elapsed() < WARMUP)
                .map(|(_, mb)| *mb)
                .sum();
            let need = self
                .cfg
                .min_available_mb
                .saturating_add(warming)
                .saturating_add(self.expected_mb(spec.memory_mb));
            if avail < need {
                return Some(format!(
                    "the machine is low on memory ({avail} MiB available, {warming} MiB of it \
                     promised to Spaces still starting, keeps {} MiB)",
                    self.cfg.min_available_mb
                ));
            }
        }
        None
    }

    /// `RunHostJobs`: decides each job, makes the acceptances durable, then
    /// starts them.
    pub async fn run_batch(
        self: &Arc<Self>,
        specs: Vec<pb::HostJobSpec>,
        scheduler: &str,
    ) -> Vec<pb::HostJobAck> {
        let specs: Vec<JobSpec> = specs.iter().take(MAX_BATCH).map(JobSpec::from_pb).collect();
        // Reclaiming comes before refusing: what idle warm members and
        // expired or orphaned Spaces hold is freed, then the batch is judged.
        let reclaim = self.reclaim.lock().expect("reclaim").clone();
        if let Some(reclaim) = reclaim {
            let (cpu, mem) = self.shortfall(&specs);
            if cpu > 0 || mem > 0 {
                reclaim(cpu, mem).await;
            }
        }
        let ext = self.external();
        let mut acks: Vec<Option<Ack>> = vec![None; specs.len()];
        // Accepted now: (index, previous live record, superseded run).
        let mut accepted: Vec<(usize, Option<JobRecord>)> = Vec::new();
        // The kind each accepted job runs as here.
        let mut kinds: Vec<String> = vec![String::new(); specs.len()];
        let mut to_write = Vec::new();
        {
            let mut g = self.inner.lock().expect("jobs");
            for (i, spec) in specs.iter().enumerate() {
                if let Some(why) = spec.invalid() {
                    acks[i] = Some(Ack::Rejected(why));
                    continue;
                }
                let prev = g.jobs.get(&spec.job_id).cloned();
                if let Some(p) = &prev {
                    if spec.attempt < p.attempt {
                        acks[i] = Some(Ack::Stale(format!(
                            "attempt {} of {} is older than attempt {} seen here",
                            spec.attempt, spec.job_id, p.attempt
                        )));
                        continue;
                    }
                    if spec.attempt == p.attempt {
                        acks[i] = Some(
                            if p.state() == HostJobState::Canceled && p.accepted_at_ms == 0 {
                                Ack::Stale(format!(
                                    "attempt {} of {} was canceled before it arrived",
                                    spec.attempt, spec.job_id
                                ))
                            } else {
                                let full = g
                                    .published
                                    .get(&spec.job_id)
                                    .filter(|r| r.attempt == p.attempt && r.state == p.state)
                                    .map(|r| (**r).clone())
                                    .unwrap_or_else(|| p.clone());
                                Ack::Duplicate(full)
                            },
                        );
                        continue;
                    }
                }
                if let Some(why) = self.no_room(&g, &ext, spec) {
                    acks[i] = Some(Ack::Rejected(why));
                    continue;
                }
                kinds[i] = self.exec.kind_of(spec);
                // A newer attempt supersedes one still running here.
                if let Some(run) = g.runs.get(&spec.job_id)
                    && run.attempt < spec.attempt
                {
                    run.token.cancel();
                }
                let rec = JobRecord {
                    job_id: spec.job_id.clone(),
                    attempt: spec.attempt,
                    state: StateWord(HostJobState::Running),
                    space: space_name(&spec.job_id, spec.attempt),
                    accepted_at_ms: now_ms(),
                    millicpus: spec.millicpus,
                    memory_mb: spec.memory_mb,
                    kind: kinds[i].clone(),
                    scheduler: scheduler.to_string(),
                    ..Default::default()
                };
                let (cpu, mem) = reservation(spec);
                g.used_millicpus += cpu;
                g.used_memory_mb += mem;
                if spec.kind == "vm"
                    && let Some(v) = crate::sysinfo::vm_capacity()
                {
                    let now = std::time::Instant::now();
                    g.vm_warming.retain(|(at, _)| now - *at < WARMUP);
                    g.vm_warming
                        .push_back((now, expected_vm_mb(spec.memory_mb, v.typical_mb)));
                }
                if self.cfg.min_available_mb > 0 {
                    let now = std::time::Instant::now();
                    while g.warming.front().is_some_and(|(at, _)| now - *at >= WARMUP) {
                        g.warming.pop_front();
                    }
                    let mb = self.expected_mb(spec.memory_mb);
                    g.warming.push_back((now, mb));
                }
                g.running += 1;
                *g.running_by_kind.entry(kinds[i].clone()).or_default() += 1;
                g.jobs.insert(spec.job_id.clone(), rec.clone());
                to_write.push(rec);
                accepted.push((i, prev));
            }
        }
        if !to_write.is_empty() {
            match self.writer.append(to_write, true).await {
                Ok(_) => {
                    for (i, _) in &accepted {
                        let mut spec = specs[*i].clone();
                        // The executor sees the kind it runs as here.
                        spec.kind = kinds[*i].clone();
                        let token = CancellationToken::new();
                        let rec = {
                            let mut g = self.inner.lock().expect("jobs");
                            g.runs.insert(
                                spec.job_id.clone(),
                                RunHandle {
                                    attempt: spec.attempt,
                                    token: token.clone(),
                                    gang: is_gang_member(&spec),
                                    started: Instant::now(),
                                },
                            );
                            g.jobs.get(&spec.job_id).cloned().unwrap_or_default()
                        };
                        acks[*i] = Some(Ack::Accepted(rec));
                        let me = self.clone();
                        tokio::spawn(async move { me.execute(spec, token).await });
                    }
                }
                Err(e) => {
                    let mut g = self.inner.lock().expect("jobs");
                    for (i, prev) in accepted {
                        let spec = &specs[i];
                        release(&mut g, spec, &kinds[i]);
                        match prev {
                            Some(p) => {
                                g.jobs.insert(spec.job_id.clone(), p);
                            }
                            None => {
                                g.jobs.remove(&spec.job_id);
                            }
                        }
                        acks[i] = Some(Ack::Rejected(format!(
                            "this host could not record the job: {e}"
                        )));
                    }
                }
            }
        }
        specs
            .iter()
            .zip(acks)
            .map(|(s, a)| {
                a.unwrap_or_else(|| Ack::Rejected("not decided".into()))
                    .to_pb(&s.job_id, s.attempt)
            })
            .collect()
    }

    async fn execute(self: Arc<Self>, spec: JobSpec, token: CancellationToken) {
        let space = space_name(&spec.job_id, spec.attempt);
        let started = Instant::now();
        let outcome = self.exec.run(&spec, &space, token.clone()).await;
        tracing::debug!(job = %spec.job_id, attempt = spec.attempt, ms = started.elapsed().as_millis() as u64, end = ?outcome.end, "job ended");
        let max = spec.max_output_bytes as usize;
        let (stdout, t1) = tail(outcome.stdout, max);
        let (stderr, t2) = tail(outcome.stderr, max);
        let (result, t3) = tail(outcome.result, max);
        let record = {
            let mut g = self.inner.lock().expect("jobs");
            release(&mut g, &spec, &spec.kind);
            if g.runs
                .get(&spec.job_id)
                .is_some_and(|r| r.attempt == spec.attempt)
            {
                g.runs.remove(&spec.job_id);
            }
            let Some(live) = g.jobs.get_mut(&spec.job_id) else {
                return;
            };
            if live.attempt != spec.attempt || live.state().is_terminal() {
                // Superseded by a newer attempt (or already ended).
                return;
            }
            let (state, exit_code, timed_out, error) = match outcome.end {
                ExecEnd::Exited(0) => (HostJobState::Succeeded, Some(0), false, String::new()),
                ExecEnd::Exited(c) => (
                    HostJobState::Failed,
                    Some(c),
                    false,
                    exit_reason(c, &stdout, &stderr),
                ),
                ExecEnd::TimedOut => (
                    HostJobState::Failed,
                    None,
                    true,
                    format!("ran out of time ({} s)", spec.timeout_secs),
                ),
                ExecEnd::Error(e) => (HostJobState::Error, None, false, e),
                ExecEnd::Canceled => (HostJobState::Canceled, None, false, "canceled".into()),
            };
            let error = match (error.is_empty(), outcome.result_error) {
                (true, Some(r)) => r,
                (_, _) => error,
            };
            let error = if state == HostJobState::Canceled {
                live.error.clone().if_empty("canceled")
            } else {
                error
            };
            live.state = StateWord(state);
            live.exit_code = exit_code;
            live.timed_out = timed_out;
            live.stdout = stdout;
            live.stderr = stderr;
            live.result = result;
            live.error = error;
            live.truncated = t1 || t2 || t3;
            live.finished_at_ms = now_ms();
            live.create_ms = outcome.create_ms;
            live.run_ms = outcome.run_ms;
            live.delete_ms = outcome.delete_ms;
            // The live copy keeps its output until the ledger publishes
            // this record (the publish callback drops it then): until
            // then, `get` and a duplicate delivery read it from here.
            live.clone()
        };
        let _ = self.writer.append(vec![record], false).await;
    }

    /// `GetHostJob`.
    pub fn get(&self, job_id: &str) -> Option<JobRecord> {
        let g = self.inner.lock().expect("jobs");
        let live = g.jobs.get(job_id)?;
        Some(
            g.published
                .get(job_id)
                .filter(|p| p.attempt == live.attempt && p.state == live.state)
                .map(|p| (**p).clone())
                .unwrap_or_else(|| live.clone()),
        )
    }

    /// `CancelHostJob`: cancels `attempt` (0: whatever runs) and fences it.
    pub async fn cancel(&self, job_id: &str, attempt: u32, reason: &str) -> JobRecord {
        let reason = if reason.trim().is_empty() {
            "canceled".to_string()
        } else {
            reason.trim().to_string()
        };
        let tombstone = {
            let mut g = self.inner.lock().expect("jobs");
            if let Some(r) = g.jobs.get(job_id).cloned() {
                if r.state() == HostJobState::Running && (attempt == 0 || r.attempt <= attempt) {
                    if let Some(l) = g.jobs.get_mut(job_id) {
                        l.error = reason.clone();
                    }
                    if let Some(run) = g.runs.get(job_id) {
                        run.token.cancel();
                    }
                    // A newer attempt than the one running is fenced below.
                    if attempt == 0 || attempt == r.attempt {
                        return r;
                    }
                } else if attempt == 0 || r.attempt >= attempt {
                    return r;
                }
            }
            // Unknown, or only older attempts seen: record the fence.
            let rec = JobRecord {
                job_id: job_id.to_string(),
                attempt: attempt.max(1),
                state: StateWord(HostJobState::Canceled),
                error: reason,
                finished_at_ms: now_ms(),
                ..Default::default()
            };
            g.jobs.insert(job_id.to_string(), rec.clone());
            rec
        };
        let _ = self.writer.append(vec![tombstone.clone()], false).await;
        tombstone
    }

    /// `ListHostJobs`: changes after `after_seq`, holding the call up to
    /// `wait` while there are none.
    pub async fn list(
        &self,
        after_seq: u64,
        limit: u32,
        wait: Duration,
        with_output: bool,
    ) -> pb::ListHostJobsResponse {
        self.list_inner(after_seq, limit, wait, with_output, None)
            .await
    }

    async fn list_inner(
        &self,
        after_seq: u64,
        limit: u32,
        wait: Duration,
        with_output: bool,
        prefix: Option<&str>,
    ) -> pb::ListHostJobsResponse {
        let limit = if limit == 0 { 1000 } else { limit.min(5000) } as usize;
        let wait = wait.min(Duration::from_secs(30));
        // A feed call is its scheduler's sign of life (self-fence).
        self.inner
            .lock()
            .expect("jobs")
            .contact
            .insert(prefix.unwrap_or("").to_string(), Instant::now());
        let deadline = tokio::time::Instant::now() + wait;
        let mut rx = self.changed.subscribe();
        loop {
            rx.borrow_and_update();
            let (jobs, next, more) = {
                let g = self.inner.lock().expect("jobs");
                let mut jobs = Vec::new();
                // A cursor ahead of everything this ledger has (the ledger
                // numbered backwards in an older release): serve it from the
                // start and say where the numbering is, so the scheduler
                // moves its cursor back instead of waiting for news that
                // can never come.
                let newest = g.feed.keys().next_back().copied().unwrap_or(0);
                let after_seq = if after_seq > newest {
                    tracing::warn!(
                        after_seq,
                        newest,
                        "the scheduler's feed cursor is ahead of this ledger: resetting it"
                    );
                    0
                } else {
                    after_seq
                };
                let mut next = after_seq;
                let mut more = false;
                for (seq, id) in g.feed.range(after_seq.saturating_add(1)..) {
                    if jobs.len() >= limit {
                        more = true;
                        break;
                    }
                    if let Some(r) = g.published.get(id) {
                        match prefix {
                            None => jobs.push(r.to_pb(with_output)),
                            Some(p) => {
                                if let Some(rest) = r.job_id.strip_prefix(p) {
                                    let mut j = r.to_pb(with_output);
                                    j.job_id = rest.to_string();
                                    jobs.push(j);
                                }
                            }
                        }
                    }
                    next = *seq;
                }
                (jobs, next, more)
            };
            if !jobs.is_empty() || tokio::time::Instant::now() >= deadline {
                return pb::ListHostJobsResponse {
                    jobs,
                    next_seq: next,
                    more,
                    node: Some(self.status()),
                    progress: self.progress_of(prefix),
                };
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                return pb::ListHostJobsResponse {
                    jobs: vec![],
                    next_seq: next,
                    more: false,
                    node: Some(self.status()),
                    progress: self.progress_of(prefix),
                };
            }
        }
    }
}

/// One account's jobs on a host that several accounts share: the host keeps
/// them as `<owner>/<job id>`, so an account never sees, cancels or fences
/// another's jobs, whatever ids they pick. The host's daemon calls these with
/// the verified caller's account; a coordinator in the same process (a
/// [`crate::link::LocalLink`] made with an owner) uses the same names.
impl NodeJobs {
    fn owned_id(owner: &str, id: &str) -> String {
        format!("{owner}/{id}")
    }

    fn unown(owner: &str, mut j: pb::HostJob) -> pb::HostJob {
        if let Some(rest) = j.job_id.strip_prefix(&format!("{owner}/")) {
            j.job_id = rest.to_string();
        }
        j
    }

    /// [`NodeJobs::run_batch`] for `owner`.
    pub async fn run_batch_for(
        self: &Arc<Self>,
        owner: &str,
        mut specs: Vec<pb::HostJobSpec>,
        scheduler: &str,
    ) -> Vec<pb::HostJobAck> {
        for s in &mut specs {
            s.job_id = Self::owned_id(owner, &s.job_id);
            // A run's data on this host is the account's own too.
            if !s.run_id.is_empty() {
                s.run_id = Self::owned_id(owner, &s.run_id);
            }
        }
        let mut acks = self.run_batch(specs, &format!("{owner}/{scheduler}")).await;
        for a in &mut acks {
            if let Some(rest) = a.job_id.strip_prefix(&format!("{owner}/")) {
                a.job_id = rest.to_string();
            }
            a.job = a.job.take().map(|j| Self::unown(owner, j));
        }
        acks
    }

    /// [`NodeJobs::get`] for `owner`, as the wire record.
    pub fn get_for(&self, owner: &str, job_id: &str, with_output: bool) -> Option<pb::HostJob> {
        self.get(&Self::owned_id(owner, job_id))
            .map(|r| Self::unown(owner, r.to_pb(with_output)))
    }

    /// [`NodeJobs::list`] of `owner`'s jobs only.
    pub async fn list_for(
        &self,
        owner: &str,
        after_seq: u64,
        limit: u32,
        wait: Duration,
        with_output: bool,
    ) -> pb::ListHostJobsResponse {
        self.list_inner(
            after_seq,
            limit,
            wait,
            with_output,
            Some(&format!("{owner}/")),
        )
        .await
    }

    /// [`NodeJobs::cancel`] for `owner`.
    pub async fn cancel_for(
        &self,
        owner: &str,
        job_id: &str,
        attempt: u32,
        reason: &str,
    ) -> pb::HostJob {
        let r = self
            .cancel(&Self::owned_id(owner, job_id), attempt, reason)
            .await;
        Self::unown(owner, r.to_pb(false))
    }
}

/// What a job of `spec` reserves here, (millicpus, MiB): a `vm` Space's
/// vCPUs share host CPUs by the VM runtime's overcommit, and its memory is
/// measured ([`crate::sysinfo::vm_capacity`]) rather than reserved when a
/// VM runtime reports. The same when accepted and when released.
fn reservation(spec: &JobSpec) -> (u64, u64) {
    if spec.kind == "vm" && crate::sysinfo::vm_overcommit().is_some() {
        (u64::from(crate::sysinfo::vm_millicpus(spec.millicpus)), 0)
    } else {
        (u64::from(spec.millicpus), spec.memory_mb)
    }
}

/// What one more VM of `memory_mb` is expected to use on a host whose
/// running VMs use `typical_mb` each (0: none runs).
pub fn expected_vm_mb(memory_mb: u64, typical_mb: u64) -> u64 {
    if typical_mb == 0 {
        memory_mb
    } else {
        memory_mb.min(typical_mb)
    }
}

/// Memory of `vm` jobs accepted within [`WARMUP`] (their VMs may not have
/// started yet).
fn vm_warming_mb(g: &Inner) -> u64 {
    g.vm_warming
        .iter()
        .filter(|(at, _)| at.elapsed() < WARMUP)
        .map(|(_, mb)| *mb)
        .sum()
}

fn release(g: &mut Inner, spec: &JobSpec, kind: &str) {
    let (cpu, mem) = reservation(spec);
    g.used_millicpus = g.used_millicpus.saturating_sub(cpu);
    g.used_memory_mb = g.used_memory_mb.saturating_sub(mem);
    g.running = g.running.saturating_sub(1);
    if let Some(n) = g.running_by_kind.get_mut(kind) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            g.running_by_kind.remove(kind);
        }
    }
}

/// Why a command that exited with `code` failed: the code and the last
/// line it printed (usually the exception), rather than a result file
/// it never got to write.
pub(crate) fn exit_reason(code: i32, stdout: &[u8], stderr: &[u8]) -> String {
    let last = [stderr, stdout].iter().find_map(|b| {
        String::from_utf8_lossy(b)
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .map(|l| l.chars().take(240).collect::<String>())
    });
    match last {
        Some(l) => format!("exited with code {code}: {l}"),
        None => format!("exited with code {code}"),
    }
}

trait IfEmpty {
    fn if_empty(self, other: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, other: &str) -> String {
        if self.is_empty() {
            other.to_string()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod exit_reason_tests {
    #[test]
    fn a_failed_command_says_its_code_and_last_line() {
        let out = b"step 5\nTraceback (most recent call last):\nRuntimeError: [ring] lost\n\n";
        assert_eq!(
            super::exit_reason(1, out, b""),
            "exited with code 1: RuntimeError: [ring] lost"
        );
        assert_eq!(super::exit_reason(2, b"", b"  \n"), "exited with code 2");
        assert_eq!(
            super::exit_reason(3, b"out\n", b"err\n"),
            "exited with code 3: err"
        );
    }
}
