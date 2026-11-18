//! A pool's event log (`$CUA_HOME/ci/<pool>/events.jsonl`, one JSON
//! object per line) and the numbers `cua ci status` shows from it.
//!
//! Every slot's life is in it: `slot_created`, `space_ready` (with the
//! create-to-ready time), `runner_installed`, `runner_registered`, `job_assigned`
//! (with how long the runner waited for it), `job_started` (first step), `job_finished` (with the
//! job's duration, name and result), `slot_deleted` (with why), and the
//! pool's own: `create_failed`, `capacity_wait`, `egress_unenforced`,
//! orphans removed, errors. No event carries a secret.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The log is rotated (one old file kept) past this size.
const ROTATE_BYTES: u64 = 16 * 1024 * 1024;

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// `cua ci serve` started.
    ServeStarted,
    /// `cua ci serve` stopped.
    ServeStopped,
    /// A slot's Space is being created.
    SlotCreated,
    /// Its Space is ready (`ms`: create to ready).
    SpaceReady,
    /// The runner was copied in and unpacked (`ms`).
    RunnerInstalled,
    /// The egress policy is on (`detail`: the resolver allowed).
    EgressApplied,
    /// The Space runs without an egress policy (`detail`: the runtime's
    /// reason).
    EgressUnenforced,
    /// The runner was registered and started (`runner`, `runner_id`).
    /// (`runner_registered` in the bench schema, `EVENTS.md`.)
    RunnerRegistered,
    /// GitHub gave the runner a job (`ms`: how long it waited idle).
    JobAssigned,
    /// The job's first workflow step began (the runner's `Set up job`
    /// finished): read back from the guest's job-start hook.
    JobStarted,
    /// The runner's one job ended (`ms`: the job's duration).
    JobFinished,
    /// A slot ended and its Space was deleted (`reason`, `ms`: lifetime).
    SlotDeleted,
    /// Creating or starting a slot failed (`detail`, `ms`: backoff).
    CreateFailed,
    /// The computer had no room (two macOS VMs, a Space limit): waiting.
    CapacityWait,
    /// A slot's Space disappeared.
    SpaceLost,
    /// A labelled Space no slot owns was deleted.
    OrphanSpaceDeleted,
    /// A runner of the pool no slot owns was removed.
    OrphanRunnerDeleted,
    /// A Space and runner found without a slot were taken back.
    Adopted,
    /// Runner logs were copied out (`detail`: where).
    LogsCopied,
    /// Something failed (`detail`).
    Error,
    /// The service or daemon first learned of a job (`queued_at_ms`).
    JobQueued,
    /// A VM was taken for a job (`start_mode`: warm_pool, fork, cold_create).
    VmClaimed,
    /// The VM answers (spacesd reachable, guest agent up).
    VmReady,
    /// `actions/checkout` finished (`ms`, `mirror`).
    CheckoutDone,
    /// One cache store was restored (`store`, `hit`, `bytes`, `ms`, `scope`).
    CacheRestore,
    /// A cache read (`store`, `key_hash`, `hit`, `source_repo_hash`,
    /// `reader_repo_hash`).
    CacheRead,
    /// A cache write (`store`, `key_hash`, `scope`, `bytes`).
    CacheWrite,
    /// The VM was deleted and nothing of the job remains (`reason`, `ms`).
    TeardownDone,
    /// The post-job scan of the VM for secrets (`found`, `detail`).
    SecretScan,
    /// The post-teardown sweep (`leaked_vms`, `leaked_cloud`, `detail`).
    ResourceSweep,
    /// A failure that was not the job's own (`detail`).
    InfraFailure,
    /// A warm VM became ready, was claimed, or was reaped (`reason`).
    WarmPool,
    /// The golden image was built, verified, refreshed or invalidated
    /// (`detail`, `ms`).
    Golden,
}

/// One line of the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// When (Unix ms).
    pub ts_ms: u64,
    /// What.
    pub kind: EventKind,
    /// The slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    /// The Space id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// The runner's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    /// The runner's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<u64>,
    /// A duration in ms (see the kind).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// Why (slot_deleted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// More, for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The runner's exit code (job_finished).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The job's name (job_finished).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    /// The job's result (job_finished).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// More typed fields of the bench schema (`EVENTS.md`): `job_key`,
    /// `class`, `arch`, `fork_pr`, `start_mode`, `store`, `hit`, `bytes`,
    /// `scope`, `mirror`, `key_hash`, ... They sit beside the fields above
    /// in the JSON line.
    #[serde(
        flatten,
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Event {
    /// An event of `kind` at `ts_ms`.
    pub fn new(ts_ms: u64, kind: EventKind) -> Self {
        Self {
            ts_ms,
            kind,
            slot: None,
            space: None,
            runner: None,
            runner_id: None,
            ms: None,
            reason: None,
            detail: None,
            exit_code: None,
            job: None,
            result: None,
            extra: Default::default(),
        }
    }

    /// With one more field of the bench schema.
    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.extra.insert(key.to_string(), value.into());
        self
    }

    /// With the slot.
    pub fn slot(mut self, s: &str) -> Self {
        self.slot = Some(s.to_string());
        self
    }

    /// With the Space.
    pub fn space(mut self, s: Option<&str>) -> Self {
        self.space = s.map(str::to_string);
        self
    }

    /// With the runner.
    pub fn runner(mut self, name: Option<&str>, id: Option<u64>) -> Self {
        self.runner = name.map(str::to_string);
        self.runner_id = id;
        self
    }

    /// With a duration.
    pub fn ms(mut self, ms: u64) -> Self {
        self.ms = Some(ms);
        self
    }

    /// With why.
    pub fn reason(mut self, r: &str) -> Self {
        self.reason = Some(r.to_string());
        self
    }

    /// With more.
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
}

/// The append-only log.
#[derive(Clone, Debug)]
pub struct EventLog {
    path: PathBuf,
}

impl EventLog {
    /// At `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Its path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one event (and mirrors it to the tracing log). A write
    /// failure is reported there, never fatal.
    pub fn record(&self, e: &Event) {
        let line = match serde_json::to_string(e) {
            Ok(l) => l,
            Err(_) => return,
        };
        match e.kind {
            EventKind::Error | EventKind::CreateFailed | EventKind::EgressUnenforced => {
                tracing::warn!(target: "cua_ci", "{line}")
            }
            _ => tracing::info!(target: "cua_ci", "{line}"),
        }
        if let Err(err) = self.append(&line) {
            tracing::warn!("event log {}: {err}", self.path.display());
        }
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        cua_home::guard_write(&self.path)?;
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
            let old = self.path.with_extension("jsonl.1");
            let _ = std::fs::rename(&self.path, old);
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(format!("{line}\n").as_bytes())
    }

    /// Every event (the rotated file first); unreadable lines are skipped.
    pub fn read(&self) -> Vec<Event> {
        let mut out = Vec::new();
        for p in [self.path.with_extension("jsonl.1"), self.path.clone()] {
            if let Ok(text) = std::fs::read_to_string(&p) {
                out.extend(text.lines().filter_map(|l| serde_json::from_str(l).ok()));
            }
        }
        out
    }

    /// The last `n` events.
    pub fn tail(&self, n: usize) -> Vec<Event> {
        let all = self.read();
        let skip = all.len().saturating_sub(n);
        all.into_iter().skip(skip).collect()
    }
}

/// Percentiles of a duration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// Samples.
    pub count: usize,
    /// Median, ms.
    pub p50_ms: Option<u64>,
    /// 95th percentile, ms.
    pub p95_ms: Option<u64>,
    /// Largest, ms.
    pub max_ms: Option<u64>,
}

impl Stats {
    /// Of `samples` (nearest-rank percentiles).
    pub fn of(mut samples: Vec<u64>) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        samples.sort_unstable();
        let rank = |p: f64| {
            let i = ((p * samples.len() as f64).ceil() as usize).clamp(1, samples.len()) - 1;
            samples[i]
        };
        Self {
            count: samples.len(),
            p50_ms: Some(rank(0.50)),
            p95_ms: Some(rank(0.95)),
            max_ms: samples.last().copied(),
        }
    }
}

/// The counts and timings of a window of events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Slots created.
    pub slots_created: usize,
    /// Jobs that started.
    pub jobs_started: usize,
    /// Jobs that finished.
    pub jobs_finished: usize,
    /// Jobs whose result was not `Succeeded` (from the runner's log).
    pub jobs_not_succeeded: usize,
    /// Create or start failures.
    pub failures: usize,
    /// Waits for a free macOS VM (or another limit).
    pub capacity_waits: usize,
    /// Jobs stopped at `job_timeout`.
    pub job_timeouts: usize,
    /// Idle runners replaced at `idle_timeout`.
    pub idle_recycled: usize,
    /// Spaces lost mid-life (crashed, deleted elsewhere).
    pub spaces_lost: usize,
    /// Orphan Spaces and runners removed.
    pub orphans_removed: usize,
    /// Create to ready.
    pub create_to_ready: Stats,
    /// Runner ready to job started (idle wait).
    pub idle_wait: Stats,
    /// Job durations.
    pub job_duration: Stats,
    /// The last failure, with when.
    pub last_error: Option<(u64, String)>,
    /// The last egress note (`egress_unenforced`'s reason).
    pub egress_note: Option<String>,
}

/// Summarizes the events at or after `since_ms`.
pub fn summarize(events: &[Event], since_ms: u64) -> Summary {
    let mut s = Summary::default();
    let (mut ready, mut wait, mut job) = (vec![], vec![], vec![]);
    for e in events.iter().filter(|e| e.ts_ms >= since_ms) {
        match e.kind {
            EventKind::SlotCreated => s.slots_created += 1,
            EventKind::SpaceReady => ready.extend(e.ms),
            EventKind::JobAssigned => {
                s.jobs_started += 1;
                wait.extend(e.ms);
            }
            EventKind::JobFinished => {
                s.jobs_finished += 1;
                job.extend(e.ms);
                if e.result
                    .as_deref()
                    .is_some_and(|r| !matches!(r, "Succeeded" | "success"))
                {
                    s.jobs_not_succeeded += 1;
                }
            }
            EventKind::CreateFailed | EventKind::Error => {
                s.failures += 1;
                s.last_error = Some((e.ts_ms, e.detail.clone().unwrap_or_default()));
            }
            EventKind::CapacityWait => s.capacity_waits += 1,
            EventKind::SpaceLost => s.spaces_lost += 1,
            EventKind::OrphanSpaceDeleted | EventKind::OrphanRunnerDeleted => {
                s.orphans_removed += 1
            }
            EventKind::EgressUnenforced => s.egress_note = e.detail.clone(),
            EventKind::EgressApplied => s.egress_note = None,
            EventKind::SlotDeleted => match e.reason.as_deref() {
                Some("job_timeout") => s.job_timeouts += 1,
                Some("idle_recycled") => s.idle_recycled += 1,
                _ => {}
            },
            _ => {}
        }
    }
    s.create_to_ready = Stats::of(ready);
    s.idle_wait = Stats::of(wait);
    s.job_duration = Stats::of(job);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_nearest_rank() {
        let s = Stats::of((1..=100).collect());
        assert_eq!(
            (s.p50_ms, s.p95_ms, s.max_ms),
            (Some(50), Some(95), Some(100))
        );
        assert_eq!(Stats::of(vec![7]).p95_ms, Some(7));
        assert_eq!(Stats::of(vec![]), Stats::default());
    }

    #[test]
    fn log_round_trip_and_summary() {
        let d = tempfile::tempdir().unwrap();
        let log = EventLog::new(d.path().join("p/events.jsonl"));
        log.record(&Event::new(1, EventKind::SlotCreated).slot("a"));
        log.record(&Event::new(2, EventKind::SpaceReady).slot("a").ms(19_200));
        log.record(&Event::new(3, EventKind::JobAssigned).slot("a").ms(3_000));
        let mut fin = Event::new(4, EventKind::JobFinished).slot("a").ms(240_000);
        fin.result = Some("Failed".into());
        log.record(&fin);
        log.record(
            &Event::new(5, EventKind::CreateFailed)
                .detail("boom")
                .ms(5_000),
        );
        log.record(&Event::new(6, EventKind::SlotDeleted).reason("job_timeout"));
        std::fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap()
            .write_all(b"not json\n")
            .unwrap();
        let all = log.read();
        assert_eq!(all.len(), 6);
        assert_eq!(log.tail(2)[1].kind, EventKind::SlotDeleted);
        let s = summarize(&all, 0);
        assert_eq!(s.slots_created, 1);
        assert_eq!(s.create_to_ready.p50_ms, Some(19_200));
        assert_eq!(s.idle_wait.count, 1);
        assert_eq!(s.job_duration.p95_ms, Some(240_000));
        assert_eq!(s.jobs_not_succeeded, 1);
        assert_eq!(s.failures, 1);
        assert_eq!(s.job_timeouts, 1);
        assert_eq!(s.last_error, Some((5, "boom".into())));
        assert_eq!(summarize(&all, 5).slots_created, 0);
        // Lines are compact JSON with snake_case kinds and no empty fields.
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(
            text.starts_with(r#"{"ts_ms":1,"kind":"slot_created","slot":"a"}"#),
            "{text}"
        );
    }
}
