//! The phase timeline of a job and the structured events the bench reads.
//!
//! The schema is `ci-contract/EVENTS.md` (owned by the bench): kinds
//! `job_queued`, `vm_claimed`, `vm_ready`, `runner_registered`,
//! `job_assigned`, `job_started`, `checkout_done`, `cache_restore`,
//! `job_finished`, `teardown_done`, plus the correctness events
//! (`cache_read`, `cache_write`, `secret_scan`, `resource_sweep`,
//! `infra_failure`). All go to the pool's `events.jsonl` through
//! [`crate::events::EventLog`], with absolute `ts_ms` from the host's
//! clock: phases are differences of two events, never stored durations.
//!
//! Some moments happen in the guest (the first step began, the job ended,
//! a cache restored). The guest's job hooks ([`super::boot`]) append them
//! to `~/.cua-ci/timers.jsonl`; [`parse_guest_marks`] reads that file and
//! shifts the guest's clock onto the host's with the offset measured at
//! the probe, so one clock stamps every event. The runner's own worker log
//! carries each step's start and finish ([`parse_step_telemetry`]), which
//! is where `checkout_done` gets its duration.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;

use super::StartMode;
use crate::config::GuestOs;
use crate::events::{Event, EventKind, EventLog};

/// One job's moments (Unix ms, host clock) as the controller learned them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobTimeline {
    /// `<run id>/<job id>` once known.
    pub job_key: Option<String>,
    /// The slot.
    pub slot: String,
    /// Class and architecture.
    pub class: Option<GuestOs>,
    /// `arm64` or `x64`.
    pub arch: Option<String>,
    /// How the VM started.
    pub start_mode: Option<StartMode>,
    /// The job runs a fork's pull request.
    pub fork_pr: Option<bool>,
    /// From the job's `CUA_CI_BENCH_RUN_ID`.
    pub bench_run_id: Option<String>,
    /// GitHub's `created_at` of the job.
    pub queued_at_ms: Option<u64>,
    /// The controller first heard of it.
    pub queued_ms: Option<u64>,
    /// A VM was taken.
    pub claimed_ms: Option<u64>,
    /// The VM answered.
    pub ready_ms: Option<u64>,
    /// The runner was registered.
    pub registered_ms: Option<u64>,
    /// GitHub handed it the job.
    pub assigned_ms: Option<u64>,
    /// The first step began.
    pub started_ms: Option<u64>,
    /// The job ended (the job-completed hook, or the runner's exit).
    pub finished_ms: Option<u64>,
    /// The VM was gone.
    pub teardown_ms: Option<u64>,
    /// `actions/checkout`'s duration and whether the mirror served it.
    pub checkout: Option<(u64, bool)>,
    /// Cache restore durations, ms.
    pub restore_ms: Vec<u64>,
    /// Restores that found something, that found nothing, and the bytes
    /// restored.
    pub cache_hits: u32,
    /// See `cache_hits`.
    pub cache_misses: u32,
    /// See `cache_hits`.
    pub cache_bytes: u64,
}

/// The derived timers, each `None` when an endpoint was not observed
/// (the bench prints "not measured", never zero).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Phases {
    /// `job_assigned` minus GitHub's `created_at`, else minus the
    /// controller's receipt.
    pub queue_wait: Option<u64>,
    /// `vm_claimed` minus the controller's receipt.
    pub vm_claim: Option<u64>,
    /// `vm_ready` minus `vm_claimed`.
    pub vm_boot: Option<u64>,
    /// `runner_registered` minus `vm_ready`.
    pub runner_register: Option<u64>,
    /// `job_assigned` minus `runner_registered`.
    pub assign: Option<u64>,
    /// `job_started` minus `job_assigned`.
    pub job_start: Option<u64>,
    /// The headline: `job_started` minus the queue time.
    pub start_latency: Option<u64>,
    /// `actions/checkout`.
    pub checkout: Option<u64>,
    /// Sum of the cache restores.
    pub cache_restore_total: Option<u64>,
    /// `job_finished` minus `job_started`.
    pub job: Option<u64>,
    /// `teardown_done` minus `job_finished`.
    pub teardown: Option<u64>,
    /// `teardown_done` minus the queue time.
    pub wall: Option<u64>,
}

fn diff(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    Some(a?.saturating_sub(b?))
}

impl JobTimeline {
    /// The queue time: GitHub's when known, else the controller's.
    pub fn queue_origin(&self) -> Option<u64> {
        self.queued_at_ms.or(self.queued_ms)
    }

    /// The derived timers.
    pub fn phases(&self) -> Phases {
        let origin = self.queue_origin();
        Phases {
            queue_wait: diff(self.assigned_ms, origin),
            vm_claim: diff(self.claimed_ms, self.queued_ms),
            vm_boot: diff(self.ready_ms, self.claimed_ms),
            runner_register: diff(self.registered_ms, self.ready_ms),
            assign: diff(self.assigned_ms, self.registered_ms),
            job_start: diff(self.started_ms, self.assigned_ms),
            start_latency: diff(self.started_ms, origin),
            checkout: self.checkout.map(|c| c.0),
            cache_restore_total: (!self.restore_ms.is_empty())
                .then(|| self.restore_ms.iter().sum()),
            job: diff(self.finished_ms, self.started_ms),
            teardown: diff(self.teardown_ms, self.finished_ms),
            wall: diff(self.teardown_ms, origin),
        }
    }
}

/// The timelines of the jobs in flight, and the log they write to.
pub struct Timers {
    log: EventLog,
    jobs: Mutex<HashMap<String, JobTimeline>>,
}

impl Timers {
    /// Writing to `log`.
    pub fn new(log: EventLog) -> Self {
        Self {
            log,
            jobs: Mutex::new(HashMap::new()),
        }
    }

    /// A copy of a slot's timeline.
    pub fn timeline(&self, slot: &str) -> Option<JobTimeline> {
        self.jobs.lock().expect("timers").get(slot).cloned()
    }

    /// Forgets a slot (after `teardown_done`).
    pub fn forget(&self, slot: &str) -> Option<JobTimeline> {
        self.jobs.lock().expect("timers").remove(slot)
    }

    /// Updates a slot's timeline and returns it (created when missing).
    fn with(&self, slot: &str, f: impl FnOnce(&mut JobTimeline)) -> JobTimeline {
        let mut jobs = self.jobs.lock().expect("timers");
        let t = jobs.entry(slot.to_string()).or_insert_with(|| JobTimeline {
            slot: slot.to_string(),
            ..Default::default()
        });
        f(t);
        t.clone()
    }

    /// The common fields of the schema on `e`.
    fn common(t: &JobTimeline, mut e: Event) -> Event {
        if !t.slot.is_empty() {
            e.slot = Some(t.slot.clone());
        }
        if let Some(k) = &t.job_key {
            e = e.with("job_key", k.as_str());
        }
        if let Some(b) = &t.bench_run_id {
            e = e.with("bench_run_id", b.as_str());
        }
        if let Some(c) = t.class {
            e = e.with("class", c.as_str());
        }
        if let Some(a) = &t.arch {
            e = e.with("arch", a.as_str());
        }
        e
    }

    /// `job_queued`: the controller learned of a job. The slot is not
    /// known yet, so the timeline is created by [`Timers::vm_claimed`]
    /// from this call's values (`key` names the pending job).
    pub fn job_queued(&self, now: u64, key: &str, repo: &str, queued_at_ms: Option<u64>) {
        let mut e = Event::new(now, EventKind::JobQueued).with("job_key", key);
        if !repo.is_empty() {
            e = e.with("repo", repo);
        }
        if let Some(q) = queued_at_ms {
            e = e.with("queued_at_ms", q);
        }
        self.log.record(&e);
    }

    /// `vm_claimed`: a VM was taken for a job.
    #[allow(clippy::too_many_arguments)]
    pub fn vm_claimed(
        &self,
        now: u64,
        slot: &str,
        job_key: Option<&str>,
        class: GuestOs,
        arch: &str,
        mode: StartMode,
        queued_ms: Option<u64>,
        queued_at_ms: Option<u64>,
    ) {
        let t = self.with(slot, |t| {
            t.job_key = job_key.map(str::to_string);
            t.class = Some(class);
            t.arch = Some(arch.to_string());
            t.start_mode = Some(mode);
            t.queued_ms = queued_ms.or(Some(now));
            t.queued_at_ms = queued_at_ms;
            t.claimed_ms = Some(now);
        });
        self.log.record(&Self::common(
            &t,
            Event::new(now, EventKind::VmClaimed).with("start_mode", mode.as_str()),
        ));
    }

    /// `vm_ready`.
    pub fn vm_ready(&self, now: u64, slot: &str, runner_version: Option<&str>) {
        let t = self.with(slot, |t| t.ready_ms = Some(now));
        let mut e = Event::new(now, EventKind::VmReady);
        if let Some(v) = runner_version {
            e = e.with("runner_version", v);
        }
        self.log.record(&Self::common(&t, e));
    }

    /// `runner_registered`.
    pub fn runner_registered(&self, e: Event, slot: &str) {
        let t = self.with(slot, |t| t.registered_ms = Some(e.ts_ms));
        self.log.record(&Self::common(&t, e));
    }

    /// `job_assigned`: `base` carries the time, the idle wait (`ms`), the
    /// runner and the Space.
    pub fn job_assigned(&self, base: Event, slot: &str, fork_pr: Option<bool>) {
        let t = self.with(slot, |t| {
            t.assigned_ms = Some(base.ts_ms);
            if fork_pr.is_some() {
                t.fork_pr = fork_pr;
            }
        });
        let mut e = base;
        if let Some(f) = t.fork_pr {
            e = e.with("fork_pr", f);
        }
        self.log.record(&Self::common(&t, e));
    }

    /// `job_started` and the guest's other moments, from the hooks.
    pub fn guest_marks(&self, slot: &str, marks: &GuestMarks) {
        let t = self.with(slot, |t| {
            if let Some(s) = marks.job_start_ms {
                t.started_ms.get_or_insert(s);
            }
            if let Some(f) = marks.job_end_ms {
                t.finished_ms.get_or_insert(f);
            }
            if let Some(b) = &marks.bench_run_id {
                t.bench_run_id.get_or_insert_with(|| b.clone());
            }
        });
        if let Some(s) = marks.job_start_ms {
            self.log
                .record(&Self::common(&t, Event::new(s, EventKind::JobStarted)));
        }
        for r in &marks.restores {
            self.cache_restore(slot, r);
        }
    }

    /// `checkout_done`.
    pub fn checkout_done(&self, now: u64, slot: &str, ms: u64, mirror: bool) {
        let t = self.with(slot, |t| t.checkout = Some((ms, mirror)));
        self.log.record(&Self::common(
            &t,
            Event::new(now, EventKind::CheckoutDone)
                .ms(ms)
                .with("mirror", mirror),
        ));
    }

    /// `cache_restore`, one per store.
    pub fn cache_restore(&self, slot: &str, r: &RestoreMark) {
        let t = self.with(slot, |t| {
            t.restore_ms.push(r.ms);
            if r.hit {
                t.cache_hits += 1;
                t.cache_bytes += r.bytes;
            } else {
                t.cache_misses += 1;
            }
        });
        self.log.record(&Self::common(
            &t,
            Event::new(r.at_ms, EventKind::CacheRestore)
                .ms(r.ms)
                .with("store", r.store.as_str())
                .with("hit", r.hit)
                .with("bytes", r.bytes)
                .with("scope", r.scope.as_str()),
        ));
    }

    /// `job_finished`: `base` carries the time, the exit code, the job's
    /// name, the reason and the Space; `ms` becomes the job's duration when
    /// both ends are known (the guest's marks), and the end is the
    /// job-completed hook's mark when there is one. `raw_result` is the
    /// runner's own word (`Succeeded`).
    pub fn job_finished(&self, base: Event, slot: &str, raw_result: Option<&str>) {
        let t = self.with(slot, |t| {
            t.finished_ms.get_or_insert(base.ts_ms);
        });
        let mut e = base;
        e.ts_ms = t.finished_ms.unwrap_or(e.ts_ms);
        if let Some(ms) = t.phases().job {
            e = e.ms(ms);
        }
        // The bench wants success, failure or cancelled; the runner's own
        // word stays beside it.
        e.result = raw_result.map(|r| normalize_result(r).to_string());
        if let Some(r) = raw_result {
            e = e.with("runner_result", r);
        }
        self.log.record(&Self::common(&t, e));
    }

    /// `teardown_done`; returns the finished timeline and forgets it.
    pub fn teardown_done(&self, now: u64, slot: &str, reason: &str) -> JobTimeline {
        let t = self.with(slot, |t| t.teardown_ms = Some(now));
        let ms = t.phases().teardown.unwrap_or(0);
        self.log.record(&Self::common(
            &t,
            Event::new(now, EventKind::TeardownDone)
                .ms(ms)
                .reason(reason),
        ));
        self.forget(slot);
        t
    }

    /// `infra_failure`: a failure that was not the job's own.
    pub fn infra_failure(&self, now: u64, slot: Option<&str>, detail: &str) {
        let mut e = Event::new(now, EventKind::InfraFailure).detail(detail);
        if let Some(s) = slot {
            e = Self::common(&self.timeline(s).unwrap_or_default(), e).slot(s);
        }
        self.log.record(&e);
    }

    /// `secret_scan`: what the scan of the VM after the job found (names only).
    pub fn secret_scan(&self, now: u64, slot: &str, found: u32, detail: &str) {
        let t = self.timeline(slot).unwrap_or_default();
        self.log.record(&Self::common(
            &t,
            Event::new(now, EventKind::SecretScan)
                .slot(slot)
                .with("found", found)
                .detail(detail),
        ));
    }

    /// `resource_sweep`: what the sweep after a teardown found leaked.
    pub fn resource_sweep(&self, now: u64, leaked_vms: u32, leaked_cloud: u32, detail: &str) {
        self.log.record(
            &Event::new(now, EventKind::ResourceSweep)
                .with("leaked_vms", leaked_vms)
                .with("leaked_cloud", leaked_cloud)
                .detail(detail),
        );
    }
}

/// `Succeeded` and friends as the bench's words.
pub fn normalize_result(r: &str) -> &'static str {
    match r.trim().to_ascii_lowercase().as_str() {
        "succeeded" | "success" | "succeededwithissues" => "success",
        "canceled" | "cancelled" | "abandoned" => "cancelled",
        _ => "failure",
    }
}

/// One cache restore the guest hook did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreMark {
    /// When it finished (host clock).
    pub at_ms: u64,
    /// `pnpm`, `cargo`, `swiftpm`, `sccache`.
    pub store: String,
    /// How long it took.
    pub ms: u64,
    /// Bytes copied.
    pub bytes: u64,
    /// Something was there to restore.
    pub hit: bool,
    /// `repo`, `default_branch` or `fork_readonly`.
    pub scope: String,
}

/// What the guest's hooks recorded, on the host's clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestMarks {
    /// The first step began.
    pub job_start_ms: Option<u64>,
    /// The job-completed hook ran.
    pub job_end_ms: Option<u64>,
    /// `GITHUB_EVENT_NAME` the hook saw.
    pub event: Option<String>,
    /// `GITHUB_REPOSITORY`.
    pub repo: Option<String>,
    /// `GITHUB_REF`.
    pub git_ref: Option<String>,
    /// The job's `CUA_CI_BENCH_RUN_ID`.
    pub bench_run_id: Option<String>,
    /// The repository mirror was set up for `actions/checkout`.
    pub mirror_ready: Option<bool>,
    /// The cache restores.
    pub restores: Vec<RestoreMark>,
}

#[derive(Deserialize)]
struct RawMark {
    k: String,
    #[serde(default)]
    ms: u64,
    #[serde(default)]
    at: u64,
    #[serde(default)]
    store: String,
    #[serde(default)]
    bytes: u64,
    #[serde(default)]
    hit: bool,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    repo: String,
    #[serde(default, rename = "ref")]
    git_ref: String,
    #[serde(default)]
    bench: String,
    #[serde(default)]
    used: Option<bool>,
}

/// Reads the guest's `timers.jsonl`. `guest_minus_host_ms` is the guest
/// clock's offset measured at the probe (guest time minus host time); the
/// marks are shifted by it. Lines that do not parse are skipped.
pub fn parse_guest_marks(text: &str, guest_minus_host_ms: i64) -> GuestMarks {
    let shift = |t: u64| (t as i64 - guest_minus_host_ms).max(0) as u64;
    let mut m = GuestMarks::default();
    for line in text.lines() {
        let Ok(r) = serde_json::from_str::<RawMark>(line) else {
            continue;
        };
        match r.k.as_str() {
            "job_start" => {
                m.job_start_ms = Some(shift(r.at));
                m.event = Some(r.event).filter(|s| !s.is_empty());
                m.repo = Some(r.repo).filter(|s| !s.is_empty());
                m.git_ref = Some(r.git_ref).filter(|s| !s.is_empty());
                m.bench_run_id = Some(r.bench).filter(|s| !s.is_empty());
            }
            "job_end" => m.job_end_ms = Some(shift(r.at)),
            "mirror" => m.mirror_ready = r.used,
            "cache_restore" => m.restores.push(RestoreMark {
                at_ms: shift(r.at),
                store: r.store,
                ms: r.ms,
                bytes: r.bytes,
                hit: r.hit,
                scope: r.scope,
            }),
            _ => {}
        }
    }
    m
}

/// One workflow step as the runner's worker log reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepTiming {
    /// The step's display name.
    pub name: String,
    /// `actions/checkout`, ... (empty for a `run:` step).
    pub action: String,
    /// Start, Unix ms (the runner's own UTC stamps, host and guest agree
    /// after the clock fixup; use [`StepTiming::ms`] for durations).
    pub start_ms: u64,
    /// Finish.
    pub finish_ms: u64,
    /// `Succeeded`, `Failed`, ...
    pub result: String,
}

impl StepTiming {
    /// The step's duration.
    pub fn ms(&self) -> u64 {
        self.finish_ms.saturating_sub(self.start_ms)
    }

    /// The step is `actions/checkout`.
    pub fn is_checkout(&self) -> bool {
        self.action.starts_with("actions/checkout")
    }
}

/// Reads the step telemetry lines of a runner worker log
/// (`Publish step telemetry for current step {...}` with `displayName`,
/// `action`, `startTime`, `finishTime` and `result`). Lines it cannot read
/// are skipped; the durations are differences of two stamps of one clock.
pub fn parse_step_telemetry(log: &str) -> Vec<StepTiming> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default, rename = "displayName")]
        display_name: String,
        #[serde(default)]
        action: String,
        #[serde(default, rename = "startTime")]
        start: String,
        #[serde(default, rename = "finishTime")]
        finish: String,
        #[serde(default)]
        result: String,
    }
    let mut out = Vec::new();
    for line in log.lines() {
        let Some(i) = line.find("Publish step telemetry for current step") else {
            continue;
        };
        let rest = &line[i..];
        let Some(j) = rest.find('{') else { continue };
        let Ok(r) = serde_json::from_str::<Raw>(rest[j..].trim()) else {
            continue;
        };
        let (Some(s), Some(f)) = (parse_iso_ms(&r.start), parse_iso_ms(&r.finish)) else {
            continue;
        };
        out.push(StepTiming {
            name: r.display_name,
            action: r.action,
            start_ms: s,
            finish_ms: f,
            result: r.result,
        });
    }
    out
}

/// `2026-10-28T12:00:01.123Z` (fraction and `Z` optional) as Unix ms.
pub fn parse_iso_ms(s: &str) -> Option<u64> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s.split_once(['T', ' '])?;
    let mut d = date.split('-');
    let (y, mo, da): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':');
    let (h, mi, se): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    let frac_ms: i64 = format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()?;
    // Days from civil (Howard Hinnant).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let ms = ((days * 24 + h) * 60 + mi) * 60 * 1000 + se * 1000 + frac_ms;
    u64::try_from(ms).ok()
}

#[cfg(test)]
mod tests;
