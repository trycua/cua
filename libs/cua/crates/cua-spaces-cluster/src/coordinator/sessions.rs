// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Durable agent sessions: a long-running agent the cluster keeps alive.
//! When the process that drives it (the *driver*) or the machine it runs
//! on dies, the coordinator starts it again, and the new driver picks up
//! where the old one stopped (the driver reads its own journal; this
//! module only decides *when* and *which epoch*).
//!
//! # Model
//!
//! - A **session** is a named row in the coordinator's database (the
//!   `sessions` table, journaled to the pool log like every other change,
//!   so a machine taking over the pool keeps every session).
//! - One **drive** is one job: `session:<name>:<epoch>` is its idempotency
//!   key, `max_attempts = 1`, no job-level retries. The session layer owns
//!   re-driving, so there is exactly one place that counts attempts.
//! - The **epoch** is the fencing token. It only grows; the coordinator
//!   issues it, one per drive, under a compare-and-set on the row's
//!   revision (in the database transaction, not just in memory). Resources
//!   (the Space, the driver's log) fence on it, see the launch contract
//!   below. With a durable Space attached (`spec.space`) the epoch is
//!   `max(counter + 1, the Space lease's token)`, so both stay monotonic.
//!
//! # States
//!
//! `pending` -> `running` -> `done` | `backoff` -> `running` ... |
//! `failed` (`exhausted` or `permanent`) | `stopped`. Terminal: `done`,
//! `failed`, `stopped`.
//!
//! Driver exit codes: `0` finished (`done`); `78` permanent failure
//! (`failed`/`permanent`, never resumed); `75` fenced (a newer epoch
//! exists elsewhere: the session is driven again with a higher epoch, which
//! counts as an attempt); anything else, a lost host or a timeout is a
//! crash: resume after a backoff (2 s, 4 s, ... 60 s), up to
//! `max_attempts` drives in total, then `failed`/`exhausted` with the last
//! failure in `error`.
//!
//! # Launch contract (the driver's environment)
//!
//! `CUA_SESSION`, `CUA_SESSION_EPOCH`, `CUA_SESSION_ATTEMPT`,
//! `CUA_SESSION_RESUME` (`1` from the second epoch on), `CUA_SESSION_SPACE`
//! (the durable Space's name or empty), `CUA_SESSION_STATE_DIR` (the
//! session's `state_dir` or empty); with a Space also
//! `CUA_SESSION_SPACE_ADDR` (how to reach it, `direct:host:port`),
//! `CUA_SESSION_SPACE_TOKEN` (its cua-spacesd token) and
//! `CUA_SESSION_SPACE_LEASE` (the lease's fencing token).
//! [`ENV_CONTRACT`] lists them; a test keeps it and the emitted names equal.
//!
//! # Guarantees
//!
//! - **At most one resumer per epoch.** The loop, a request and a second
//!   coordinator all go through a per-session lock and a revision CAS in
//!   the same transaction as the write, so of two racing resumes one is
//!   refused and no epoch is issued twice.
//! - **A stale driver is cut off, not trusted.** The previous driver job is
//!   cancelled before the next epoch is issued; one on a partitioned host
//!   is fenced there (the job layer's fence list) when the host answers
//!   again; and anything it writes after the new driver started carries a
//!   lower epoch than the floor the new driver raised, so it is refused.
//! - **Survives the coordinator.** Every transition is an op in the pool
//!   log. A `running` session whose job the database does not know is
//!   submitted again under the same key (idempotent), and a backoff is an
//!   absolute time, so a restart mid-backoff keeps the attempts.
//!
//! # Limits
//!
//! - The coordinator can only see that the driver *job* ended or its host
//!   was lost (after `node_down_after`), not that the agent is wedged while
//!   its process lives; a driver that hangs forever is only stopped by the
//!   job `timeout_secs` (7 days by default) or `stop`.
//! - Fencing of the *resources* (Space, log) is done by the driver and
//!   those resources on the new epoch; this module issues and orders the
//!   epochs but does not itself enforce them on a Space.
//! - A partitioned host keeps running its old driver until it can be told
//!   (the job layer cancels it when the host answers, or the host fences
//!   itself); until then two drivers can run, and only the epoch floor on
//!   their shared resources keeps the old one from writing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

mod telemetry;

use super::store::{self, NewJob, state as jstate};
use super::{CoordError, Coordinator, sanitize_name};
use cua_cluster::model::{JobSpec, now_ms};

/// Session states.
pub mod sstate {
    /// Created; no drive has started yet (or none could).
    pub const PENDING: &str = "pending";
    /// A driver job exists for the current epoch.
    pub const RUNNING: &str = "running";
    /// The driver ended badly; the next drive waits for its time.
    pub const BACKOFF: &str = "backoff";
    /// The driver finished (exit 0).
    pub const DONE: &str = "done";
    /// Gave up: see the reason.
    pub const FAILED: &str = "failed";
    /// Stopped by its owner.
    pub const STOPPED: &str = "stopped";

    /// Whether `s` is an ended state.
    pub fn is_terminal(s: &str) -> bool {
        matches!(s, DONE | FAILED | STOPPED)
    }
}

/// Why a session is `failed`.
pub mod reason {
    /// The driver crashed `max_attempts` times.
    pub const EXHAUSTED: &str = "exhausted";
    /// The driver said it cannot succeed (exit 78), or the session cannot
    /// be driven (its Space is gone).
    pub const PERMANENT: &str = "permanent";
}

/// A driver that cannot succeed and must not be started again.
pub const EXIT_PERMANENT: i32 = 78;
/// A driver that found a newer epoch: it stopped, ignore it.
pub const EXIT_FENCED: i32 = 75;

/// The environment every driver job gets, by name.
pub const ENV_CONTRACT: &[&str] = &[
    "CUA_SESSION",
    "CUA_SESSION_EPOCH",
    "CUA_SESSION_ATTEMPT",
    "CUA_SESSION_RESUME",
    "CUA_SESSION_SPACE",
    "CUA_SESSION_STATE_DIR",
    "CUA_SESSION_HEARTBEAT_SECS",
];

/// What a driver with an attached durable Space also gets.
pub const ENV_SPACE: &[&str] = &[
    "CUA_SESSION_SPACE_ADDR",
    "CUA_SESSION_SPACE_TOKEN",
    "CUA_SESSION_SPACE_LEASE",
];

/// Seconds a driver may run (7 days).
const DRIVER_TIMEOUT_SECS: u32 = 7 * 24 * 3600;
/// Entries of history kept.
const HISTORY_MAX: usize = 50;
/// The lease on the attached Space.
const LEASE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// What a session runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSpec {
    /// The driver command.
    pub argv: Vec<String>,
    /// Its environment (never shown back; the launch contract is added).
    pub env: BTreeMap<String, String>,
    /// Image of the driver's Space (empty: the canonical Linux image).
    pub image: String,
    /// `container`, `vm`, `microvm` or empty.
    pub kind: String,
    /// A runtime or empty.
    pub runtime: String,
    /// vCPUs of the driver's Space (0: the default).
    pub cpus: f64,
    /// Memory in MiB (0: the default).
    pub memory_mb: u64,
    /// Seconds one drive may run (0: 7 days).
    pub timeout_secs: u32,
    /// The machine the driver must run on (empty or `auto`: any).
    pub on: String,
    /// Host OS it needs.
    pub os: String,
    /// Host CPU architecture it needs.
    pub arch: String,
    /// Accelerator it needs.
    pub accelerator: String,
    /// Files unpacked in the driver's work directory (a gzip tar, base64).
    pub bundle: String,
    /// A durable Space the session works in: leased on every drive.
    pub space: String,
    /// Where the driver keeps its journal (`CUA_SESSION_STATE_DIR`).
    pub state_dir: String,
    /// What the session is, for people (`pi`, `claude-code`, ...).
    pub label: String,
    /// Seconds a running driver may write no output before it counts as
    /// wedged and is cancelled and resumed (`None`: 120; `0`: never). The
    /// driver is told to print a heartbeat line every quarter of it
    /// (`CUA_SESSION_HEARTBEAT_SECS`).
    pub stall_secs: Option<u32>,
}

/// Seconds without output after which a driver is wedged, by default.
pub const DEFAULT_STALL_SECS: u32 = 120;
/// A driver that has written nothing yet (installing, starting) gets at
/// least this long.
const STARTUP_GRACE_SECS: u64 = 600;

impl SessionSpec {
    /// The stall limit in seconds (0: off).
    pub fn stall(&self) -> u32 {
        self.stall_secs.unwrap_or(DEFAULT_STALL_SECS)
    }

    /// How often the driver should print a heartbeat (0: not asked to).
    pub fn heartbeat_secs(&self) -> u32 {
        match self.stall() {
            0 => 0,
            n => (n / 4).max(1),
        }
    }
}

/// One thing that happened to a session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hist {
    /// The epoch it concerns.
    pub epoch: u64,
    /// Which drive (1-based).
    pub attempt: u32,
    /// When (Unix ms).
    pub at_ms: i64,
    /// `started`, `done`, `crashed`, `fenced`, `permanent`, `exhausted`,
    /// `stopped`.
    pub event: String,
    /// The machine, when known.
    pub node: String,
    /// The driver job.
    pub job: i64,
    /// The driver's exit code, when it exited.
    pub exit_code: Option<i32>,
    /// More.
    pub detail: String,
    /// Why a drive ended badly, as a fixed word (`process_exit`, `stall`,
    /// `host_down`, `driver_lost`, `fenced`); empty otherwise. Telemetry
    /// reads it; it never holds free text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cause: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Row {
    name: String,
    spec: SessionSpec,
    state: String,
    reason: String,
    epoch: u64,
    attempts: u32,
    max_attempts: u32,
    driver_job: i64,
    node: String,
    error: String,
    next_attempt_ms: i64,
    /// The Space lease's fencing token of the current epoch.
    lease_token: u64,
    /// Bumped by every write: the compare-and-set.
    rev: u64,
    history: Vec<Hist>,
    created_ms: i64,
    updated_ms: i64,
    /// What [`Row::note`] recorded since this row was loaded: telemetry
    /// reads it once the row is saved. Never stored.
    #[serde(skip)]
    fresh: Vec<Hist>,
}

/// What `spec` shows back: no environment values, no bundle.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SpecView {
    /// The driver command.
    pub argv: Vec<String>,
    /// Names of the environment variables set (values are never shown).
    pub env_keys: Vec<String>,
    /// Image.
    pub image: String,
    /// Kind.
    pub kind: String,
    /// Machine it is pinned to.
    pub on: String,
    /// Attached durable Space.
    pub space: String,
    /// Journal directory.
    pub state_dir: String,
    /// What it is.
    pub label: String,
    /// Size of the bundle.
    pub bundle_bytes: usize,
    /// Seconds without output before the driver counts as wedged (0: off).
    pub stall_secs: u32,
}

/// A session as anyone with the admin token sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionView {
    /// Its name.
    pub name: String,
    /// `pending`, `running`, `backoff`, `done`, `failed` or `stopped`.
    pub state: String,
    /// Why it failed: `exhausted` or `permanent`; empty otherwise.
    pub reason: String,
    /// The current (newest) epoch.
    pub epoch: u64,
    /// Drives so far.
    pub attempts: u32,
    /// Drives allowed in total.
    pub max_attempts: u32,
    /// Seconds until the next drive, in `backoff`.
    pub next_attempt_in_secs: Option<i64>,
    /// The driver job of the current epoch.
    pub driver_job: Option<i64>,
    /// The machine the driver runs (or last ran) on.
    pub node: String,
    /// The last failure (or why it cannot start).
    pub error: String,
    /// Seconds since the running driver last wrote output (stall watch on).
    pub last_output_age_secs: Option<i64>,
    /// What it runs (no secrets).
    pub spec: SpecView,
    /// What happened, oldest first.
    pub history: Vec<Hist>,
    /// Created (Unix ms).
    pub created_ms: i64,
    /// Last changed.
    pub updated_ms: i64,
}

impl Row {
    fn view(&self) -> SessionView {
        let now = now_ms();
        SessionView {
            name: self.name.clone(),
            state: self.state.clone(),
            reason: self.reason.clone(),
            epoch: self.epoch,
            attempts: self.attempts,
            max_attempts: self.max_attempts,
            next_attempt_in_secs: (self.state == sstate::BACKOFF)
                .then(|| ((self.next_attempt_ms - now).max(0) + 999) / 1000),
            driver_job: (self.driver_job > 0).then_some(self.driver_job),
            node: self.node.clone(),
            error: self.error.clone(),
            last_output_age_secs: None,
            spec: SpecView {
                argv: self.spec.argv.clone(),
                env_keys: self.spec.env.keys().cloned().collect(),
                image: self.spec.image.clone(),
                kind: self.spec.kind.clone(),
                on: self.spec.on.clone(),
                space: self.spec.space.clone(),
                state_dir: self.spec.state_dir.clone(),
                label: self.spec.label.clone(),
                bundle_bytes: self.spec.bundle.len(),
                stall_secs: self.spec.stall(),
            },
            history: self.history.clone(),
            created_ms: self.created_ms,
            updated_ms: self.updated_ms,
        }
    }

    fn note(&mut self, event: &str, exit_code: Option<i32>, detail: impl Into<String>) {
        self.note_cause(event, exit_code, detail, "");
    }

    fn note_cause(
        &mut self,
        event: &str,
        exit_code: Option<i32>,
        detail: impl Into<String>,
        cause: &str,
    ) {
        let h = Hist {
            epoch: self.epoch,
            attempt: self.attempts,
            at_ms: now_ms(),
            event: event.into(),
            node: self.node.clone(),
            job: self.driver_job,
            exit_code,
            detail: detail.into(),
            cause: cause.into(),
        };
        self.fresh.push(h.clone());
        self.history.push(h);
        if self.history.len() > HISTORY_MAX {
            let cut = self.history.len() - HISTORY_MAX;
            self.history.drain(..cut);
        }
    }
}

pub(crate) fn init(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions(
           name TEXT PRIMARY KEY, rev INTEGER NOT NULL, state TEXT NOT NULL,
           row TEXT NOT NULL);",
    )
}

fn load(c: &Connection, name: &str) -> Result<Option<Row>, String> {
    let json: Option<String> = c
        .query_row("SELECT row FROM sessions WHERE name=?1", [name], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    json.map(|j| serde_json::from_str(&j).map_err(|e| format!("session row: {e}")))
        .transpose()
}

fn list(c: &Connection, live_only: bool) -> Result<Vec<Row>, String> {
    let q = if live_only {
        "SELECT row FROM sessions WHERE state IN ('pending','running','backoff') ORDER BY name"
    } else {
        "SELECT row FROM sessions ORDER BY name"
    };
    let mut st = c.prepare(q).map_err(|e| e.to_string())?;
    let rows: Vec<String> = st
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    rows.iter()
        .map(|j| serde_json::from_str(j).map_err(|e| format!("session row: {e}")))
        .collect()
}

/// [`store::Op::SessionSave`](super::store::Op::SessionSave): writes the
/// row when the stored revision is `expect_rev` (0: no row yet); whether
/// it did.
pub(crate) fn save_json(
    c: &Connection,
    row: &serde_json::Value,
    expect_rev: Option<u64>,
) -> Result<bool, String> {
    let r: Row = serde_json::from_value(row.clone()).map_err(|e| format!("session row: {e}"))?;
    if let Some(want) = expect_rev {
        let have = load(c, &r.name)?.map_or(0, |s| s.rev);
        if have != want {
            return Ok(false);
        }
    }
    c.execute(
        "INSERT OR REPLACE INTO sessions(name, rev, state, row) VALUES(?1,?2,?3,?4)",
        params![
            r.name,
            r.rev as i64,
            r.state,
            serde_json::to_string(&r).map_err(|e| e.to_string())?
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}

/// [`store::Op::SessionRemove`](super::store::Op::SessionRemove).
pub(crate) fn remove_named(c: &Connection, name: &str) -> Result<bool, String> {
    c.execute("DELETE FROM sessions WHERE name=?1", [name])
        .map(|n| n > 0)
        .map_err(|e| e.to_string())
}

/// The driver of `r`'s epoch ended badly (`event`: `crashed`, `fenced` or
/// `stalled`): back off for the next drive, or give up at the cap.
fn mark_crashed(r: &mut Row, event: &str, code: Option<i32>, why: String, cause: &str) {
    r.note_cause(event, code, why.clone(), cause);
    if r.attempts >= r.max_attempts {
        r.state = sstate::FAILED.into();
        r.reason = reason::EXHAUSTED.into();
        r.error = format!(
            "gave up after {} of {} drives; last: {why}",
            r.attempts, r.max_attempts
        );
        r.note("exhausted", code, r.error.clone());
    } else {
        r.state = sstate::BACKOFF.into();
        r.error = why;
        r.next_attempt_ms = now_ms() + super::gang::backoff(r.attempts).as_millis() as i64;
    }
}

/// The last `n` characters of `s`, trimmed, on one line.
fn tail(s: &str, n: usize) -> String {
    let t = s.trim();
    let skip = t.chars().count().saturating_sub(n);
    t.chars()
        .skip(skip)
        .collect::<String>()
        .replace('\n', " | ")
}

impl Coordinator {
    async fn session_lock(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let m = {
            let mut g = self.session_locks.lock().expect("session locks");
            g.entry(name.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        m.lock_owned().await
    }

    async fn session_load(&self, name: &str) -> Result<Option<Row>, CoordError> {
        let n = name.to_string();
        self.store
            .call(move |c| load(c, &n))
            .await
            .map_err(CoordError::Store)
    }

    /// Writes `r` when the stored revision is still `expect`: the new row,
    /// or `None` when another writer got there first.
    async fn session_save(&self, mut r: Row, expect: u64) -> Result<Option<Row>, CoordError> {
        r.rev = expect + 1;
        r.updated_ms = now_ms();
        let row = serde_json::to_value(&r).map_err(|e| CoordError::Store(e.to_string()))?;
        let outs = self
            .persist_ops(vec![store::Op::SessionSave {
                row,
                expect_rev: Some(expect),
            }])
            .await
            .map_err(CoordError::Store)?;
        let applied = outs.first().is_some_and(|o| o.applied());
        if applied && !r.fresh.is_empty() {
            let machines = self.nodes().len() as u32;
            telemetry::session_changed(&r, machines);
        }
        Ok(applied.then_some(r))
    }

    /// A session with its driver's machine filled in.
    async fn session_view(&self, r: &Row) -> SessionView {
        let mut v = r.view();
        if r.driver_job > 0
            && !sstate::is_terminal(&r.state)
            && let Ok(Some(j)) = {
                let id = r.driver_job;
                self.store.call(move |c| store::job(c, id)).await
            }
            && !j.node.is_empty()
        {
            v.node = j.node;
        }
        if r.state == sstate::RUNNING
            && let Some((job, _, at)) = self
                .session_watch
                .lock()
                .expect("session watch")
                .get(&r.name)
                .copied()
            && job == r.driver_job
        {
            v.last_output_age_secs = Some((now_ms() - at).max(0) / 1000);
        }
        v
    }

    /// Creates (or returns) the session `name`, and starts its first drive
    /// at once (when it cannot yet, the loop does within seconds).
    /// `max_attempts` 0: the cluster's default (5).
    pub async fn session_start(
        &self,
        name: &str,
        spec: SessionSpec,
        max_attempts: u32,
    ) -> Result<SessionView, CoordError> {
        let clean = sanitize_name(name);
        if clean != name.trim() || clean.is_empty() || clean.len() > 48 {
            return Err(CoordError::Invalid(format!(
                "a session is named with lower-case letters, digits, `-` and `.` (48 at most), \
                 like {clean:?}"
            )));
        }
        if spec.argv.is_empty() || spec.argv[0].trim().is_empty() {
            return Err(CoordError::Invalid(
                "a session needs a driver command".into(),
            ));
        }
        let _g = self.session_lock(&clean).await;
        if let Some(r) = self.session_load(&clean).await? {
            if r.spec != spec {
                return Err(CoordError::Conflict(format!(
                    "session {clean} already exists with another driver; stop and delete it first"
                )));
            }
            return Ok(self.session_view(&r).await);
        }
        let now = now_ms();
        let row = Row {
            name: clean.clone(),
            spec,
            state: sstate::PENDING.into(),
            max_attempts: if max_attempts == 0 {
                self.cfg.default_max_attempts.max(1)
            } else {
                max_attempts.min(100)
            },
            created_ms: now,
            ..Default::default()
        };
        let Some(row) = self.session_save(row, 0).await? else {
            return Err(CoordError::Conflict(format!(
                "session {clean} was created at the same time"
            )));
        };
        self.audit("session", "coordinator", &clean, "start").await;
        let row = self.session_drive(row).await?;
        Ok(self.session_view(&row).await)
    }

    /// One session.
    pub async fn session_get(&self, name: &str) -> Result<SessionView, CoordError> {
        match self.session_load(name).await? {
            Some(r) => Ok(self.session_view(&r).await),
            None => Err(CoordError::NotFound(format!("no session {name}"))),
        }
    }

    /// Every session.
    pub async fn session_list(&self) -> Result<Vec<SessionView>, CoordError> {
        let rows = self
            .store
            .call(|c| list(c, false))
            .await
            .map_err(CoordError::Store)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(self.session_view(r).await);
        }
        Ok(out)
    }

    /// Stops the session: its driver is cancelled and it is never driven
    /// again. Stopping an ended session returns it as it is.
    pub async fn session_stop(&self, name: &str) -> Result<SessionView, CoordError> {
        let _g = self.session_lock(name).await;
        let mut r = self
            .session_load(name)
            .await?
            .ok_or_else(|| CoordError::NotFound(format!("no session {name}")))?;
        if sstate::is_terminal(&r.state) {
            return Ok(r.view());
        }
        self.session_cancel_driver(&r).await;
        let rev = r.rev;
        r.state = sstate::STOPPED.into();
        r.next_attempt_ms = 0;
        r.note("stopped", None, "stopped by its owner");
        let Some(r) = self.session_save(r, rev).await? else {
            return Err(CoordError::Conflict(format!("{name} changed; try again")));
        };
        self.session_release(&r).await;
        self.audit("session", "coordinator", name, "stop").await;
        Ok(r.view())
    }

    /// Deletes the session (stopping it first when it still runs).
    pub async fn session_delete(&self, name: &str) -> Result<(), CoordError> {
        let _g = self.session_lock(name).await;
        let r = self
            .session_load(name)
            .await?
            .ok_or_else(|| CoordError::NotFound(format!("no session {name}")))?;
        if !sstate::is_terminal(&r.state) {
            self.session_cancel_driver(&r).await;
            self.session_release(&r).await;
        }
        self.persist_ops(vec![store::Op::SessionRemove {
            name: name.to_string(),
        }])
        .await
        .map_err(CoordError::Store)?;
        self.audit("session", "coordinator", name, "delete").await;
        Ok(())
    }

    /// The output of the session's newest driver from byte `from`: from its
    /// machine while it can be reached, else the tail kept with the job.
    pub async fn session_output(
        &self,
        name: &str,
        from: u64,
    ) -> Result<serde_json::Value, CoordError> {
        use base64::Engine as _;
        let r = self
            .session_load(name)
            .await?
            .ok_or_else(|| CoordError::NotFound(format!("no session {name}")))?;
        if r.driver_job <= 0 {
            return Ok(serde_json::json!({
                "name": name, "epoch": r.epoch, "job": 0, "host": "", "state": r.state,
                "from": from, "next": from, "size": 0, "data_b64": "", "text": "",
                "done": sstate::is_terminal(&r.state),
            }));
        }
        let id = r.driver_job;
        let job = self
            .store
            .call(move |c| store::job(c, id))
            .await
            .map_err(CoordError::Store)?;
        let ended = job.as_ref().is_some_and(|j| jstate::is_terminal(&j.state));
        // What the coordinator kept of an ended job: used when its machine
        // is gone, forgot the job, or kept no output for it.
        let kept = |j: store::JobRow| {
            let all = format!("{}{}", j.stdout, j.stderr).into_bytes();
            let start = (from as usize).min(all.len());
            let chunk = all[start..].to_vec();
            serde_json::json!({
                "name": name, "epoch": r.epoch, "job": id, "host": j.node,
                "state": r.state, "from": from, "next": all.len(), "size": all.len(),
                "data_b64": base64::engine::general_purpose::STANDARD.encode(&chunk),
                "text": String::from_utf8_lossy(&chunk),
                "done": sstate::is_terminal(&r.state),
            })
        };
        match self.job_output(id, from).await {
            Ok((host, o)) => {
                if o.output_size == 0
                    && ended
                    && let Some(j) = job.filter(|j| !j.stdout.is_empty() || !j.stderr.is_empty())
                {
                    return Ok(kept(j));
                }
                let job_done = o
                    .job
                    .as_ref()
                    .and_then(|j| cua_cluster::model::HostJobState::parse(&j.state))
                    .is_some_and(|s| s.is_terminal());
                let next = from + o.output_chunk.len() as u64;
                Ok(serde_json::json!({
                    "name": name, "epoch": r.epoch, "job": id, "host": host,
                    "state": r.state, "from": from, "next": next, "size": o.output_size,
                    "data_b64": base64::engine::general_purpose::STANDARD.encode(&o.output_chunk),
                    "text": String::from_utf8_lossy(&o.output_chunk),
                    "done": sstate::is_terminal(&r.state) && job_done && next >= o.output_size,
                }))
            }
            Err(e) => match job.filter(|_| ended) {
                Some(j) => Ok(kept(j)),
                None => Err(e),
            },
        }
    }

    /// Looks at session `name` once and acts: starts the first drive,
    /// notices an ended driver, or resumes after a backoff. The re-driver
    /// loop calls it for every live session; any number of callers at once
    /// issue one epoch between them.
    pub async fn session_check(&self, name: &str) -> Result<Option<SessionView>, CoordError> {
        let _g = self.session_lock(name).await;
        let Some(r) = self.session_load(name).await? else {
            return Ok(None);
        };
        let r = self.session_step(r).await?;
        Ok(Some(self.session_view(&r).await))
    }

    async fn session_step(&self, r: Row) -> Result<Row, CoordError> {
        match r.state.as_str() {
            sstate::PENDING => self.session_drive(r).await,
            sstate::BACKOFF if now_ms() >= r.next_attempt_ms => self.session_drive(r).await,
            sstate::RUNNING => {
                if r.driver_job <= 0 {
                    // Saved before its job was submitted (this coordinator
                    // or the one before stopped between the two).
                    return self.session_submit_driver(r).await;
                }
                let id = r.driver_job;
                let job = self
                    .store
                    .call(move |c| store::job(c, id))
                    .await
                    .map_err(CoordError::Store)?;
                match job {
                    None => {
                        let mut r = r;
                        r.driver_job = 0;
                        self.session_submit_driver(r).await
                    }
                    Some(j) if jstate::is_terminal(&j.state) => self.session_ended(r, j).await,
                    Some(j) => self.session_watch(r, j).await,
                }
            }
            _ => Ok(r),
        }
    }

    /// Watches a running driver's output: when it has not grown for the
    /// session's stall limit, the driver is wedged (a frozen process, a
    /// hung tool): cancel it (fenced on its host) and resume after a
    /// backoff like any crash. A host that cannot be reached is not a stall:
    /// the job layer decides that after `node_down_after`.
    async fn session_watch(&self, r: Row, j: store::JobRow) -> Result<Row, CoordError> {
        let stall = r.spec.stall() as u64;
        if stall == 0 || j.state != jstate::RUNNING {
            return Ok(r);
        }
        let now = now_ms();
        // Only the size is wanted: a read past the end returns it alone.
        let Ok((_, o)) = self.job_output(j.id, u64::MAX).await else {
            return Ok(r);
        };
        let (changed, size) = {
            let mut w = self.session_watch.lock().expect("session watch");
            let e = w
                .entry(r.name.clone())
                .or_insert((j.id, o.output_size, now));
            if e.0 != j.id || o.output_size > e.1 {
                *e = (j.id, o.output_size, now);
            }
            (e.2, e.1)
        };
        let limit = if size == 0 {
            stall.max(STARTUP_GRACE_SECS)
        } else {
            stall
        };
        if now - changed < (limit * 1000) as i64 {
            return Ok(r);
        }
        let rev = r.rev;
        let name = r.name.clone();
        let mut n = r.clone();
        n.node = j.node.clone();
        let why = format!(
            "epoch {}: driver job {} wrote no output for {} s (limit {limit} s): wedged, cancelled",
            n.epoch,
            j.id,
            (now - changed) / 1000
        );
        if let Err(e) = self.cancel_job(j.id).await {
            tracing::warn!(session = %name, job = j.id, "cancelling a wedged driver: {e}");
        }
        mark_crashed(&mut n, "stalled", None, why, "stall");
        let state = n.state.clone();
        let Some(n) = self.session_save(n, rev).await? else {
            return self.session_reload(&name).await;
        };
        if sstate::is_terminal(&state) {
            self.session_release(&n).await;
        }
        self.audit(
            "session",
            "coordinator",
            &name,
            &format!("epoch {} stalled: {state}", n.epoch),
        )
        .await;
        Ok(n)
    }

    /// The driver job of the current epoch ended: done, failed for good,
    /// or crashed (backoff, or out of attempts).
    async fn session_ended(&self, mut r: Row, j: store::JobRow) -> Result<Row, CoordError> {
        let rev = r.rev;
        let name = r.name.clone();
        r.node = j.node.clone();
        let code = j.exit_code;
        let why = {
            let mut w = format!("epoch {}: driver job {} {}", r.epoch, j.id, j.state);
            if let Some(c) = code {
                w.push_str(&format!(", exit {c}"));
            }
            if j.timed_out {
                w.push_str(", ran out of time");
            }
            if !j.error.is_empty() {
                w.push_str(&format!(" ({})", tail(&j.error, 200)));
            }
            let t = tail(&j.stderr, 300);
            if !t.is_empty() {
                w.push_str(&format!(": {t}"));
            }
            w
        };
        if j.state == jstate::SUCCEEDED {
            r.state = sstate::DONE.into();
            r.error.clear();
            r.note("done", code, "the driver finished");
        } else if code == Some(EXIT_PERMANENT) {
            r.state = sstate::FAILED.into();
            r.reason = reason::PERMANENT.into();
            r.error = format!("permanent failure, not resumed. {why}");
            r.note("permanent", code, why);
        } else {
            let fenced = code == Some(EXIT_FENCED);
            let cause = if fenced {
                "fenced"
            } else if j.error.ends_with(" went down") {
                "host_down"
            } else if code.is_some() {
                "process_exit"
            } else {
                "driver_lost"
            };
            mark_crashed(
                &mut r,
                if fenced { "fenced" } else { "crashed" },
                code,
                why,
                cause,
            );
        }
        let state = r.state.clone();
        let Some(r) = self.session_save(r, rev).await? else {
            return self.session_reload(&name).await;
        };
        if sstate::is_terminal(&state) {
            self.session_release(&r).await;
        }
        self.audit(
            "session",
            "coordinator",
            &r.name,
            &format!("epoch {} ended: {state}", r.epoch),
        )
        .await;
        Ok(r)
    }

    /// The row after losing a compare-and-set.
    async fn session_reload(&self, name: &str) -> Result<Row, CoordError> {
        self.session_load(name)
            .await?
            .ok_or_else(|| CoordError::NotFound(format!("no session {name}")))
    }

    /// Issues the next epoch and starts its driver (`pending` or `backoff`
    /// -> `running`).
    async fn session_drive(&self, r: Row) -> Result<Row, CoordError> {
        // The previous driver goes first: never two epochs at once.
        self.session_cancel_driver(&r).await;
        let mut lease = None;
        if !r.spec.space.is_empty() {
            // A Space its host lost since the last look is placed again first.
            self.durable_recheck(&r.spec.space).await;
            // Its .cua/setup gates the drive (looked at before the lease, so
            // waiting takes no fencing token).
            if let Ok(d) = self.durable_get(&r.spec.space).await {
                if d.setup == super::durable::setup::FAILED {
                    let why = format!(
                        "the .cua/setup of its durable Space {} failed: {}",
                        r.spec.space, d.setup_note
                    );
                    return self.session_fail_permanent(r, why).await;
                }
                if d.setup == super::durable::setup::PENDING {
                    return self
                        .session_wait(r, "its durable Space is running .cua/setup")
                        .await;
                }
            }
            match self
                .durable_acquire(&r.spec.space, &format!("session:{}", r.name), LEASE_TTL)
                .await
            {
                Ok(c)
                    if c.state == super::durable::state::RUNNING
                        && c.setup == super::durable::setup::FAILED =>
                {
                    let _ = self.durable_release(&r.spec.space, c.lease_token).await;
                    let why = format!(
                        "the .cua/setup of its durable Space {} failed: {}",
                        r.spec.space, c.setup_note
                    );
                    return self.session_fail_permanent(r, why).await;
                }
                Ok(c)
                    if c.state == super::durable::state::RUNNING
                        && c.setup == super::durable::setup::PENDING =>
                {
                    let _ = self.durable_release(&r.spec.space, c.lease_token).await;
                    return self
                        .session_wait(r, "its durable Space is running .cua/setup")
                        .await;
                }
                Ok(c) if c.state == super::durable::state::RUNNING => lease = Some(c),
                Ok(c) => {
                    // Nothing to drive yet: do not hold the lease meanwhile.
                    let _ = self.durable_release(&r.spec.space, c.lease_token).await;
                    return self
                        .session_wait(r, "its durable Space is not placed yet")
                        .await;
                }
                Err(CoordError::NotFound(e)) => {
                    return self.session_fail_permanent(r, e).await;
                }
                Err(e) => return self.session_wait(r, &e.to_string()).await,
            }
        }
        let rev = r.rev;
        let name = r.name.clone();
        let mut next = r;
        next.epoch = (next.epoch + 1).max(lease.as_ref().map_or(0, |c| c.lease_token));
        next.lease_token = lease.as_ref().map_or(0, |c| c.lease_token);
        next.attempts += 1;
        next.state = sstate::RUNNING.into();
        next.driver_job = 0;
        next.node.clear();
        next.error.clear();
        next.next_attempt_ms = 0;
        next.note("started", None, "");
        let Some(next) = self.session_save(next, rev).await? else {
            // Another resumer won this epoch.
            return self.session_reload(&name).await;
        };
        self.audit(
            "session",
            "coordinator",
            &next.name,
            &format!("epoch {} (drive {})", next.epoch, next.attempts),
        )
        .await;
        self.session_submit_driver_with(next, lease).await
    }

    /// Recorded when it changes, not on every try.
    async fn session_wait(&self, r: Row, why: &str) -> Result<Row, CoordError> {
        if r.error == why {
            return Ok(r);
        }
        let rev = r.rev;
        let mut n = r.clone();
        n.error = why.to_string();
        Ok(self.session_save(n, rev).await?.unwrap_or(r))
    }

    async fn session_fail_permanent(&self, r: Row, why: String) -> Result<Row, CoordError> {
        let rev = r.rev;
        let mut n = r.clone();
        n.state = sstate::FAILED.into();
        n.reason = reason::PERMANENT.into();
        n.error = why.clone();
        n.note("permanent", None, why);
        Ok(self.session_save(n, rev).await?.unwrap_or(r))
    }

    async fn session_submit_driver(&self, r: Row) -> Result<Row, CoordError> {
        let lease = if r.spec.space.is_empty() {
            None
        } else {
            match self.durable_connection(&r.spec.space, r.lease_token).await {
                Ok(c) => Some(c),
                Err(e) => return self.session_wait(r, &e.to_string()).await,
            }
        };
        self.session_submit_driver_with(r, lease).await
    }

    /// Submits the driver job of the row's epoch (idempotent per epoch) and
    /// records its id.
    async fn session_submit_driver_with(
        &self,
        r: Row,
        lease: Option<super::durable::DurableConnection>,
    ) -> Result<Row, CoordError> {
        let run = format!("session-{}", r.name);
        let key = format!("session:{}:{}", r.name, r.epoch);
        let spec = driver_job(&r, &run, lease.as_ref())?;
        let ids = self
            .submit(
                &run,
                0,
                vec![NewJob {
                    key,
                    spec,
                    max_attempts: 1,
                }],
            )
            .await?;
        let Some((id, _)) = ids.first().copied() else {
            return Err(CoordError::Store("the driver job was not recorded".into()));
        };
        let rev = r.rev;
        let mut n = r.clone();
        n.driver_job = id;
        Ok(self.session_save(n, rev).await?.unwrap_or(r))
    }

    /// Cancels the driver job of the row, if one still runs.
    async fn session_cancel_driver(&self, r: &Row) {
        if r.driver_job <= 0 {
            return;
        }
        let id = r.driver_job;
        if let Ok(Some(j)) = self.store.call(move |c| store::job(c, id)).await
            && !jstate::is_terminal(&j.state)
            && let Err(e) = self.cancel_job(id).await
        {
            tracing::warn!(session = %r.name, job = id, "cancelling the driver: {e}");
        }
    }

    /// Gives the attached Space's lease back.
    async fn session_release(&self, r: &Row) {
        if !r.spec.space.is_empty() && r.lease_token > 0 {
            let _ = self.durable_release(&r.spec.space, r.lease_token).await;
        }
    }

    /// The re-driver: every `session_tick`, looks at each live session.
    pub(crate) async fn session_loop(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => return,
                _ = tokio::time::sleep(self.cfg.session_tick) => {}
            }
            let rows = match self.store.call(|c| list(c, true)).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("sessions: {e}");
                    continue;
                }
            };
            for row in rows {
                if let Err(e) = self.session_check(&row.name).await {
                    tracing::warn!(session = %row.name, "re-driving: {e}");
                }
            }
        }
    }
}

/// The driver job of the row's current epoch.
fn driver_job(
    r: &Row,
    run: &str,
    lease: Option<&super::durable::DurableConnection>,
) -> Result<JobSpec, CoordError> {
    use base64::Engine as _;
    let s = &r.spec;
    let bundle = if s.bundle.trim().is_empty() {
        vec![]
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(s.bundle.trim())
            .map_err(|e| CoordError::Invalid(format!("bundle: not base64 ({e})")))?
    };
    let mut env = s.env.clone();
    for (k, v) in [
        ("CUA_SESSION", r.name.clone()),
        ("CUA_SESSION_EPOCH", r.epoch.to_string()),
        ("CUA_SESSION_ATTEMPT", r.attempts.to_string()),
        ("CUA_SESSION_RESUME", u8::from(r.epoch > 1).to_string()),
        ("CUA_SESSION_SPACE", s.space.clone()),
        ("CUA_SESSION_STATE_DIR", s.state_dir.clone()),
        ("CUA_SESSION_HEARTBEAT_SECS", s.heartbeat_secs().to_string()),
    ] {
        env.insert(k.into(), v);
    }
    if let Some(c) = lease {
        env.insert("CUA_SESSION_SPACE_ADDR".into(), c.id.clone());
        env.insert("CUA_SESSION_SPACE_TOKEN".into(), c.token.clone());
        env.insert("CUA_SESSION_SPACE_LEASE".into(), c.lease_token.to_string());
    }
    let spec = JobSpec {
        job_id: String::new(),
        attempt: 0,
        image: s.image.clone(),
        kind: cua_cluster::model::normalize_auto(&s.kind),
        runtime: s.runtime.clone(),
        millicpus: if s.cpus > 0.0 {
            (s.cpus * 1000.0).round() as u32
        } else {
            0
        },
        memory_mb: s.memory_mb,
        argv: s.argv.clone(),
        env,
        timeout_secs: if s.timeout_secs == 0 {
            DRIVER_TIMEOUT_SECS
        } else {
            s.timeout_secs
        },
        result_path: String::new(),
        max_output_bytes: 0,
        os: String::new(),
        arch: String::new(),
        on: String::new(),
        bundle,
        run_id: run.to_string(),
        accelerator: s.accelerator.clone(),
        retries: 0,
        services: vec![],
    };
    let mut out = JobSpec::from_pb(&spec.to_pb());
    out.os = s.os.trim().to_ascii_lowercase();
    out.arch = if s.arch.trim().is_empty() {
        String::new()
    } else {
        cua_cluster::model::normalize_arch(&s.arch)
    };
    out.on = cua_cluster::model::normalize_auto(&s.on);
    out.accelerator = cua_cluster::model::normalize_accelerator(&spec.accelerator)
        .map_err(CoordError::Invalid)?;
    out.run_id = run.to_string();
    out.retries = 0;
    if out.kind == cua_cluster::model::KIND_PROCESS {
        return Err(CoordError::Invalid(cua_cluster::model::process_refused()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(rev: u64, epoch: u64) -> serde_json::Value {
        serde_json::to_value(Row {
            name: "s".into(),
            state: sstate::RUNNING.into(),
            epoch,
            rev,
            ..Default::default()
        })
        .unwrap()
    }

    /// The compare-and-set is in the database transaction: of two writers
    /// that read the same revision, one wins and the other is refused, so a
    /// stale resumer cannot overwrite the epoch the winner issued.
    #[test]
    fn a_stale_writer_is_refused_by_the_revision() {
        let c = Connection::open_in_memory().unwrap();
        init(&c).unwrap();
        // Created once: a second creator is refused.
        assert!(save_json(&c, &row(1, 0), Some(0)).unwrap());
        assert!(!save_json(&c, &row(1, 0), Some(0)).unwrap());
        // Two resumers read rev 1; the first issues epoch 1.
        assert!(save_json(&c, &row(2, 1), Some(1)).unwrap());
        assert!(!save_json(&c, &row(2, 7), Some(1)).unwrap());
        assert_eq!(load(&c, "s").unwrap().unwrap().epoch, 1);
        // A write without an expectation (an old log entry) still applies.
        assert!(save_json(&c, &row(9, 2), None).unwrap());
        assert!(remove_named(&c, "s").unwrap());
        assert!(load(&c, "s").unwrap().is_none());
    }

    #[test]
    fn history_is_bounded_and_the_view_hides_secrets() {
        let mut r = Row::default();
        r.spec.env.insert("TOKEN".into(), "hunter2".into());
        for i in 0..(HISTORY_MAX + 10) {
            r.epoch = i as u64;
            r.note("crashed", Some(1), "x");
        }
        assert_eq!(r.history.len(), HISTORY_MAX);
        assert_eq!(r.history[0].epoch, 10);
        let json = serde_json::to_string(&r.view()).unwrap();
        assert!(!json.contains("hunter2") && json.contains("TOKEN"));
    }
}
