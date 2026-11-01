// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The coordinator's durable state: one SQLite database in WAL mode
//! (`<cluster dir>/cluster.db`). The job table is the queue; every state
//! change is a conditional update, so a job's completion is recorded at
//! most once (the first terminal result for its current attempt wins).
//!
//! Nothing here is on the placement path: the scheduler places from
//! memory and writes each round's dispatches, and each host's batch of
//! changes, in one transaction.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use cua_cluster::model::JobSpec;

/// Coordinator job states.
pub mod state {
    /// Waiting for a host.
    pub const QUEUED: &str = "queued";
    /// Sent to a host; not acknowledged yet.
    pub const DISPATCHED: &str = "dispatched";
    /// A host accepted it.
    pub const RUNNING: &str = "running";
    /// Canceled by its owner; the host is stopping it.
    pub const CANCELING: &str = "canceling";
    /// Ended: the command exited 0.
    pub const SUCCEEDED: &str = "succeeded";
    /// Ended: the command exited non-zero or ran out of time.
    pub const FAILED: &str = "failed";
    /// Ended: no host could run it within its attempts.
    pub const ERROR: &str = "error";
    /// Ended: canceled.
    pub const CANCELED: &str = "canceled";

    /// Whether `s` is an ended state.
    pub fn is_terminal(s: &str) -> bool {
        matches!(s, SUCCEEDED | FAILED | ERROR | CANCELED)
    }
}

/// A database error, as text.
pub type StoreResult<T> = Result<T, String>;

fn e(err: rusqlite::Error) -> String {
    format!("cluster database: {err}")
}

/// A host as stored. In a pool its row is replicated without its token.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeRow {
    /// Its name (unique).
    pub name: String,
    /// `host:port` of its direct listener (empty: none).
    pub direct: String,
    /// Relay URL (empty: none).
    pub relay_url: String,
    /// Its machine on the relay.
    pub relay_machine: String,
    /// Its env token.
    #[serde(skip)]
    pub token: String,
    /// `direct`, `relay` or `provider`.
    pub via: String,
    /// An elastic provider (`modal`), for a provider node.
    pub provider: String,
    /// `ready` or `draining`.
    pub state: String,
    /// Its OS.
    pub os: String,
    /// The change feed position applied.
    pub cursor: u64,
    /// The ledger the cursor belongs to.
    pub ledger_id: String,
    /// Added (Unix ms).
    pub added_ms: i64,
    /// Most jobs at once for a provider node.
    pub max_jobs: u32,
}

/// A job as the API shows it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct JobRow {
    /// Its id.
    pub id: i64,
    /// The submitter's idempotency key.
    pub key: String,
    /// Its run.
    pub run: String,
    /// What it runs.
    pub spec: JobSpec,
    /// Its state.
    pub state: String,
    /// The newest attempt.
    pub attempt: u32,
    /// The host of the newest attempt.
    pub node: String,
    /// Attempts allowed.
    pub max_attempts: u32,
    /// Submitted (Unix ms).
    pub submitted_ms: i64,
    /// Last dispatched.
    pub dispatched_ms: i64,
    /// Last accepted by a host.
    pub started_ms: i64,
    /// Ended.
    pub finished_ms: i64,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Ran out of time.
    pub timed_out: bool,
    /// What went wrong.
    pub error: String,
    /// Tail of stdout (lossy UTF-8).
    pub stdout: String,
    /// Tail of stderr.
    pub stderr: String,
    /// The result file, base64.
    pub result_b64: String,
    /// The Space it ran in.
    pub space: String,
    /// Space create, command, Space delete (ms).
    pub create_ms: u32,
    /// Command (ms).
    pub run_ms: u32,
    /// Space delete (ms).
    pub delete_ms: u32,
}

/// A queued or in-flight job loaded at start or refill.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// Id.
    pub id: i64,
    /// Run.
    pub run: String,
    /// Spec.
    pub spec: JobSpec,
    /// State.
    pub state: String,
    /// Newest attempt.
    pub attempt: u32,
    /// Its host.
    pub node: String,
    /// Attempts allowed.
    pub max_attempts: u32,
    /// Submitted.
    pub submitted_ms: i64,
    /// Attempts a host lost so far.
    pub retries: u32,
}

/// One change the scheduler records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// Queued -> dispatched to `node` as `attempt`.
    Dispatched {
        /// Job.
        id: i64,
        /// Host.
        node: String,
        /// The new attempt.
        attempt: u32,
        /// When.
        at_ms: i64,
    },
    /// Dispatched -> running (the host accepted it).
    Running {
        /// Job.
        id: i64,
        /// Attempt.
        attempt: u32,
        /// When.
        at_ms: i64,
    },
    /// Back to the queue (the attempt is kept; the next dispatch is a new
    /// one). `retry` counts it as a failed attempt.
    Requeue {
        /// Job.
        id: i64,
        /// The attempt that is given up.
        attempt: u32,
        /// Why.
        error: String,
        /// It counts as a failed attempt (a host lost it), not a refusal.
        retry: bool,
    },
    /// The first terminal result of the current attempt.
    Complete {
        /// Job.
        id: i64,
        /// Attempt.
        attempt: u32,
        /// Terminal state.
        state: String,
        /// Exit code.
        exit_code: Option<i32>,
        /// Ran out of time.
        timed_out: bool,
        /// Error.
        error: String,
        /// Output.
        #[serde(with = "b64")]
        stdout: Vec<u8>,
        /// Output.
        #[serde(with = "b64")]
        stderr: Vec<u8>,
        /// Result.
        #[serde(with = "b64")]
        result: Vec<u8>,
        /// Space.
        space: String,
        /// Timings.
        create_ms: u32,
        /// Timings.
        run_ms: u32,
        /// Timings.
        delete_ms: u32,
        /// When.
        at_ms: i64,
        /// When its command ended, by its host's clock (Unix ms; 0:
        /// unknown, e.g. its host was lost): orders the failures of a
        /// gang's members, whatever order the coordinator heard of them
        /// in (CUA-1826).
        #[serde(default)]
        ended_ms: i64,
    },
    /// A queued job ended without running (canceled).
    EndQueued {
        /// Job.
        id: i64,
        /// Terminal state.
        state: String,
        /// Why.
        error: String,
        /// When.
        at_ms: i64,
    },
    /// Running or dispatched -> canceling (its owner canceled it).
    Canceling {
        /// Job.
        id: i64,
    },
    /// A host's change feed position.
    Cursor {
        /// Host.
        node: String,
        /// Position.
        seq: u64,
        /// The ledger.
        ledger_id: String,
    },
    /// Remember to cancel `attempt` of job `id` on `node` when it answers.
    FenceAdd {
        /// Host.
        node: String,
        /// Job.
        id: i64,
        /// Attempt.
        attempt: u32,
    },
    /// The fence was delivered.
    FenceDone {
        /// Host.
        node: String,
        /// Job.
        id: i64,
    },
}

/// A job to submit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewJob {
    /// Idempotency key (unique across the cluster).
    pub key: String,
    /// What it runs.
    pub spec: JobSpec,
    /// Attempts allowed.
    pub max_attempts: u32,
}

/// Jobs of a run by state.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RunSummary {
    /// The run.
    pub run: String,
    /// Created (Unix ms).
    pub created_ms: i64,
    /// Jobs in flight at once (0: no limit).
    pub concurrency: u32,
    /// Jobs by state.
    pub counts: std::collections::BTreeMap<String, u64>,
    /// Jobs.
    pub total: u64,
    /// Every job ended.
    pub done: bool,
    /// First submit to last end, of the ended jobs (ms).
    pub span_ms: i64,
    /// Submit-to-end time of ended jobs: p50 (ms).
    pub e2e_p50_ms: i64,
    /// p99 (ms).
    pub e2e_p99_ms: i64,
    /// The command's own time: p50 (ms).
    pub run_p50_ms: i64,
    /// Space create time: p50 (ms).
    pub create_p50_ms: i64,
    /// Space create time: p99 (ms).
    pub create_p99_ms: i64,
    /// Attempts beyond the first, summed.
    pub retries: u64,
}

/// The database, shared by the scheduler's tasks.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Opens (creating) the database at `path`.
    pub fn open(path: &Path) -> StoreResult<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        }
        let conn = Connection::open(path).map_err(e)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Host env tokens live here.
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS nodes(
               name TEXT PRIMARY KEY, direct TEXT NOT NULL DEFAULT '',
               relay_url TEXT NOT NULL DEFAULT '', relay_machine TEXT NOT NULL DEFAULT '',
               token TEXT NOT NULL DEFAULT '', via TEXT NOT NULL,
               provider TEXT NOT NULL DEFAULT '', state TEXT NOT NULL,
               os TEXT NOT NULL DEFAULT '', cursor INTEGER NOT NULL DEFAULT 0,
               ledger_id TEXT NOT NULL DEFAULT '', added_ms INTEGER NOT NULL,
               max_jobs INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS runs(
               id TEXT PRIMARY KEY, created_ms INTEGER NOT NULL,
               concurrency INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS jobs(
               id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT NOT NULL UNIQUE,
               run TEXT NOT NULL DEFAULT '', spec TEXT NOT NULL, state TEXT NOT NULL,
               attempt INTEGER NOT NULL DEFAULT 0, node TEXT NOT NULL DEFAULT '',
               max_attempts INTEGER NOT NULL, submitted_ms INTEGER NOT NULL,
               dispatched_ms INTEGER NOT NULL DEFAULT 0, started_ms INTEGER NOT NULL DEFAULT 0,
               finished_ms INTEGER NOT NULL DEFAULT 0, exit_code INTEGER,
               timed_out INTEGER NOT NULL DEFAULT 0, error TEXT NOT NULL DEFAULT '',
               stdout BLOB, stderr BLOB, result BLOB, space TEXT NOT NULL DEFAULT '',
               create_ms INTEGER NOT NULL DEFAULT 0, run_ms INTEGER NOT NULL DEFAULT 0,
               delete_ms INTEGER NOT NULL DEFAULT 0, retries INTEGER NOT NULL DEFAULT 0,
               ended_ms INTEGER NOT NULL DEFAULT 0);
             CREATE INDEX IF NOT EXISTS jobs_state ON jobs(state, id);
             CREATE INDEX IF NOT EXISTS jobs_run ON jobs(run, id);
             CREATE TABLE IF NOT EXISTS fences(
               node TEXT NOT NULL, job INTEGER NOT NULL, attempt INTEGER NOT NULL,
               PRIMARY KEY(node, job));",
        )
        .map_err(e)?;
        // Columns added after the table first shipped.
        let have: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('jobs')")
            .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
            .map_err(e)?;
        if !have.iter().any(|h| h == "ended_ms") {
            conn.execute_batch("ALTER TABLE jobs ADD COLUMN ended_ms INTEGER NOT NULL DEFAULT 0;")
                .map_err(e)?;
        }
        // Durable Spaces' table too: a machine that takes over rebuilds it
        // from the pool log before the coordinator starts.
        super::durable::init(&conn).map_err(e)?;
        super::sessions::init(&conn).map_err(e)?;
        super::gang::init(&conn).map_err(e)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` on the connection off the async threads.
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> StoreResult<T> + Send + 'static,
    ) -> StoreResult<T> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut c = conn
                .lock()
                .map_err(|_| "cluster database lock".to_string())?;
            f(&mut c)
        })
        .await
        .map_err(|err| format!("cluster database task: {err}"))?
    }

    /// Runs `f` on the connection here (startup, tests).
    pub fn now<T>(&self, f: impl FnOnce(&mut Connection) -> StoreResult<T>) -> StoreResult<T> {
        let mut c = self
            .conn
            .lock()
            .map_err(|_| "cluster database lock".to_string())?;
        f(&mut c)
    }
}

/// `meta.k`.
pub fn meta_get(c: &Connection, k: &str) -> StoreResult<Option<String>> {
    c.query_row("SELECT v FROM meta WHERE k=?1", [k], |r| r.get(0))
        .optional()
        .map_err(e)
}

/// Sets `meta.k`.
pub fn meta_set(c: &Connection, k: &str, v: &str) -> StoreResult<()> {
    c.execute(
        "INSERT INTO meta(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
        params![k, v],
    )
    .map(|_| ())
    .map_err(e)
}

/// Adds `n` to the counter `meta.k`.
pub fn meta_add(c: &Connection, k: &str, n: u64) -> StoreResult<()> {
    let v: u64 = meta_get(c, k)?.and_then(|v| v.parse().ok()).unwrap_or(0);
    meta_set(c, k, &(v + n).to_string())
}

fn node_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        name: r.get(0)?,
        direct: r.get(1)?,
        relay_url: r.get(2)?,
        relay_machine: r.get(3)?,
        token: r.get(4)?,
        via: r.get(5)?,
        provider: r.get(6)?,
        state: r.get(7)?,
        os: r.get(8)?,
        cursor: r.get::<_, i64>(9)? as u64,
        ledger_id: r.get(10)?,
        added_ms: r.get(11)?,
        max_jobs: r.get::<_, i64>(12)? as u32,
    })
}

const NODE_COLS: &str = "name,direct,relay_url,relay_machine,token,via,provider,state,os,cursor,ledger_id,added_ms,max_jobs";

/// Every host.
pub fn nodes(c: &Connection) -> StoreResult<Vec<NodeRow>> {
    let mut st = c
        .prepare(&format!("SELECT {NODE_COLS} FROM nodes ORDER BY name"))
        .map_err(e)?;
    st.query_map([], node_from)
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)
}

/// One host.
pub fn node(c: &Connection, name: &str) -> StoreResult<Option<NodeRow>> {
    c.query_row(
        &format!("SELECT {NODE_COLS} FROM nodes WHERE name=?1"),
        [name],
        node_from,
    )
    .optional()
    .map_err(e)
}

/// Adds or replaces a host (its feed position is kept when its ledger is
/// the same).
pub fn upsert_node(c: &Connection, n: &NodeRow) -> StoreResult<()> {
    c.execute(
        "INSERT INTO nodes(name,direct,relay_url,relay_machine,token,via,provider,state,os,cursor,ledger_id,added_ms,max_jobs)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
         ON CONFLICT(name) DO UPDATE SET direct=excluded.direct, relay_url=excluded.relay_url,
           relay_machine=excluded.relay_machine, token=excluded.token, via=excluded.via,
           provider=excluded.provider, state=excluded.state, os=excluded.os,
           max_jobs=excluded.max_jobs",
        params![
            n.name,
            n.direct,
            n.relay_url,
            n.relay_machine,
            n.token,
            n.via,
            n.provider,
            n.state,
            n.os,
            n.cursor as i64,
            n.ledger_id,
            n.added_ms,
            n.max_jobs as i64
        ],
    )
    .map(|_| ())
    .map_err(e)
}

/// Sets a host's state.
pub fn set_node_state(c: &Connection, name: &str, state: &str) -> StoreResult<bool> {
    c.execute(
        "UPDATE nodes SET state=?2 WHERE name=?1",
        params![name, state],
    )
    .map(|n| n > 0)
    .map_err(e)
}

/// Removes a host and its fences.
pub fn delete_node(c: &mut Connection, name: &str) -> StoreResult<bool> {
    let tx = c.transaction().map_err(e)?;
    tx.execute("DELETE FROM fences WHERE node=?1", [name])
        .map_err(e)?;
    let n = tx
        .execute("DELETE FROM nodes WHERE name=?1", [name])
        .map_err(e)?;
    tx.commit().map_err(e)?;
    Ok(n > 0)
}

/// Fences not delivered yet: (node, job, attempt).
pub fn fences(c: &Connection) -> StoreResult<Vec<(String, i64, u32)>> {
    let mut st = c
        .prepare("SELECT node, job, attempt FROM fences")
        .map_err(e)?;
    st.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u32))
    })
    .map_err(e)?
    .collect::<Result<_, _>>()
    .map_err(e)
}

/// Records a run (its concurrency limit) when new.
pub fn ensure_run(c: &Connection, run: &str, concurrency: u32, now_ms: i64) -> StoreResult<()> {
    c.execute(
        "INSERT INTO runs(id, created_ms, concurrency) VALUES(?1,?2,?3) ON CONFLICT(id) DO NOTHING",
        params![run, now_ms, concurrency as i64],
    )
    .map(|_| ())
    .map_err(e)
}

/// Every run's concurrency limit.
pub fn run_limits(c: &Connection) -> StoreResult<Vec<(String, u32)>> {
    let mut st = c
        .prepare("SELECT id, concurrency FROM runs WHERE concurrency > 0")
        .map_err(e)?;
    st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u32)))
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)
}

/// Inserts jobs; a key seen before returns the existing job instead.
/// Returns (id, existed) per job.
pub fn submit(
    c: &mut Connection,
    run: &str,
    jobs: &[NewJob],
    now_ms: i64,
) -> StoreResult<Vec<(i64, bool)>> {
    let tx = c.transaction().map_err(e)?;
    let out = submit_in(&tx, run, jobs, now_ms)?;
    tx.commit().map_err(e)?;
    Ok(out)
}

/// [`submit`] inside the caller's transaction.
pub fn submit_in(
    tx: &Connection,
    run: &str,
    jobs: &[NewJob],
    now_ms: i64,
) -> StoreResult<Vec<(i64, bool)>> {
    let mut out = Vec::with_capacity(jobs.len());
    {
        let mut ins = tx
            .prepare_cached(
                "INSERT INTO jobs(key, run, spec, state, max_attempts, submitted_ms)
                 VALUES(?1,?2,?3,'queued',?4,?5) ON CONFLICT(key) DO NOTHING",
            )
            .map_err(e)?;
        let mut find = tx
            .prepare_cached("SELECT id FROM jobs WHERE key=?1")
            .map_err(e)?;
        for j in jobs {
            let spec = serde_json::to_string(&j.spec).map_err(|err| err.to_string())?;
            let n = ins
                .execute(params![j.key, run, spec, j.max_attempts as i64, now_ms])
                .map_err(e)?;
            if n == 1 {
                out.push((tx.last_insert_rowid(), false));
            } else {
                let id: i64 = find.query_row([&j.key], |r| r.get(0)).map_err(e)?;
                out.push((id, true));
            }
        }
    }
    Ok(out)
}

fn loaded_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Loaded> {
    let spec: String = r.get(2)?;
    Ok(Loaded {
        id: r.get(0)?,
        run: r.get(1)?,
        spec: serde_json::from_str(&spec).unwrap_or_default(),
        state: r.get(3)?,
        attempt: r.get::<_, i64>(4)? as u32,
        node: r.get(5)?,
        max_attempts: r.get::<_, i64>(6)? as u32,
        submitted_ms: r.get(7)?,
        retries: r.get::<_, i64>(8)? as u32,
    })
}

const LOADED_COLS: &str =
    "id, run, spec, state, attempt, node, max_attempts, submitted_ms, retries";

/// Queued jobs after `after_id`, oldest first.
pub fn queued_after(c: &Connection, after_id: i64, limit: usize) -> StoreResult<Vec<Loaded>> {
    let mut st = c
        .prepare_cached(&format!(
            "SELECT {LOADED_COLS} FROM jobs WHERE state='queued' AND id > ?1 ORDER BY id LIMIT ?2"
        ))
        .map_err(e)?;
    st.query_map(params![after_id, limit as i64], loaded_from)
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)
}

/// Jobs a host was given and that have not ended.
pub fn in_flight(c: &Connection) -> StoreResult<Vec<Loaded>> {
    let mut st = c
        .prepare(&format!(
            "SELECT {LOADED_COLS} FROM jobs WHERE state IN ('dispatched','running','canceling') ORDER BY id"
        ))
        .map_err(e)?;
    st.query_map([], loaded_from)
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)
}

/// Applies `changes` in one transaction. Returns, per change, whether it
/// took effect (a `Complete` that did not was a duplicate).
pub fn apply(c: &mut Connection, changes: &[Change]) -> StoreResult<Vec<bool>> {
    let tx = c.transaction().map_err(e)?;
    let out = changes
        .iter()
        .map(|ch| apply_change(&tx, ch))
        .collect::<StoreResult<Vec<bool>>>()?;
    tx.commit().map_err(e)?;
    Ok(out)
}

/// One change inside the caller's transaction: whether it took effect.
pub fn apply_change(tx: &Connection, ch: &Change) -> StoreResult<bool> {
    {
        let n = match ch {
            Change::Dispatched {
                id,
                node,
                attempt,
                at_ms,
            } => tx
                .prepare_cached(
                    "UPDATE jobs SET state='dispatched', node=?2, attempt=?3, dispatched_ms=?4
                     WHERE id=?1 AND state='queued'",
                )
                .and_then(|mut s| s.execute(params![id, node, *attempt as i64, at_ms])),
            Change::Running { id, attempt, at_ms } => tx
                .prepare_cached(
                    "UPDATE jobs SET state='running', started_ms=?3
                     WHERE id=?1 AND attempt=?2 AND state='dispatched'",
                )
                .and_then(|mut s| s.execute(params![id, *attempt as i64, at_ms])),
            Change::Requeue {
                id,
                attempt,
                error,
                retry,
            } => tx
                .prepare_cached(
                    "UPDATE jobs SET state='queued', node='', error=?3, retries=retries+?4
                     WHERE id=?1 AND attempt=?2 AND state IN ('dispatched','running','canceling')",
                )
                .and_then(|mut s| s.execute(params![id, *attempt as i64, error, *retry as i64])),
            Change::Complete {
                id,
                attempt,
                state,
                exit_code,
                timed_out,
                error,
                stdout,
                stderr,
                result,
                space,
                create_ms,
                run_ms,
                delete_ms,
                at_ms,
                ended_ms,
            } => tx
                .prepare_cached(
                    "UPDATE jobs SET state=?3, exit_code=?4, timed_out=?5, error=?6, stdout=?7,
                       stderr=?8, result=?9, space=?10, create_ms=?11, run_ms=?12, delete_ms=?13,
                       finished_ms=?14, ended_ms=?15
                     WHERE id=?1 AND attempt=?2 AND state IN ('dispatched','running','canceling')",
                )
                .and_then(|mut s| {
                    s.execute(params![
                        id,
                        *attempt as i64,
                        state,
                        exit_code,
                        *timed_out as i64,
                        error,
                        stdout,
                        stderr,
                        result,
                        space,
                        create_ms,
                        run_ms,
                        delete_ms,
                        at_ms,
                        ended_ms
                    ])
                }),
            Change::EndQueued {
                id,
                state,
                error,
                at_ms,
            } => tx
                .prepare_cached(
                    "UPDATE jobs SET state=?2, error=?3, finished_ms=?4 WHERE id=?1 AND state='queued'",
                )
                .and_then(|mut s| s.execute(params![id, state, error, at_ms])),
            Change::Canceling { id } => tx
                .prepare_cached(
                    "UPDATE jobs SET state='canceling' WHERE id=?1 AND state IN ('dispatched','running')",
                )
                .and_then(|mut s| s.execute(params![id])),
            Change::Cursor {
                node,
                seq,
                ledger_id,
            } => tx
                .prepare_cached("UPDATE nodes SET cursor=?2, ledger_id=?3 WHERE name=?1")
                .and_then(|mut s| s.execute(params![node, *seq as i64, ledger_id])),
            Change::FenceAdd { node, id, attempt } => tx
                .prepare_cached(
                    "INSERT INTO fences(node, job, attempt) VALUES(?1,?2,?3)
                     ON CONFLICT(node, job) DO UPDATE SET attempt=max(attempt, excluded.attempt)",
                )
                .and_then(|mut s| s.execute(params![node, id, *attempt as i64])),
            Change::FenceDone { node, id } => tx
                .prepare_cached("DELETE FROM fences WHERE node=?1 AND job=?2")
                .and_then(|mut s| s.execute(params![node, id])),
        }
        .map_err(e)?;
        Ok(n > 0)
    }
}

fn job_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    let spec: String = r.get(3)?;
    let blob = |i: usize| -> rusqlite::Result<Vec<u8>> {
        Ok(r.get::<_, Option<Vec<u8>>>(i)?.unwrap_or_default())
    };
    use base64::Engine as _;
    Ok(JobRow {
        id: r.get(0)?,
        key: r.get(1)?,
        run: r.get(2)?,
        spec: serde_json::from_str(&spec).unwrap_or_default(),
        state: r.get(4)?,
        attempt: r.get::<_, i64>(5)? as u32,
        node: r.get(6)?,
        max_attempts: r.get::<_, i64>(7)? as u32,
        submitted_ms: r.get(8)?,
        dispatched_ms: r.get(9)?,
        started_ms: r.get(10)?,
        finished_ms: r.get(11)?,
        exit_code: r.get(12)?,
        timed_out: r.get::<_, i64>(13)? != 0,
        error: r.get(14)?,
        stdout: String::from_utf8_lossy(&blob(15)?).into_owned(),
        stderr: String::from_utf8_lossy(&blob(16)?).into_owned(),
        result_b64: base64::engine::general_purpose::STANDARD.encode(blob(17)?),
        space: r.get(18)?,
        create_ms: r.get::<_, i64>(19)? as u32,
        run_ms: r.get::<_, i64>(20)? as u32,
        delete_ms: r.get::<_, i64>(21)? as u32,
    })
}

const JOB_COLS: &str = "id,key,run,spec,state,attempt,node,max_attempts,submitted_ms,dispatched_ms,started_ms,finished_ms,exit_code,timed_out,error,stdout,stderr,result,space,create_ms,run_ms,delete_ms";

/// One job.
pub fn job(c: &Connection, id: i64) -> StoreResult<Option<JobRow>> {
    c.query_row(
        &format!("SELECT {JOB_COLS} FROM jobs WHERE id=?1"),
        [id],
        job_from,
    )
    .optional()
    .map_err(e)
}

/// Jobs of `run` (empty: every run) in `state` (empty: any), after id
/// `after`, at most `limit`, with output only when `with_output`.
pub fn jobs(
    c: &Connection,
    run: &str,
    state: &str,
    after: i64,
    limit: usize,
    with_output: bool,
) -> StoreResult<Vec<JobRow>> {
    let cols = if with_output {
        JOB_COLS.to_string()
    } else {
        JOB_COLS.replace("stdout,stderr,result", "NULL,NULL,NULL")
    };
    let mut st = c
        .prepare(&format!(
            "SELECT {cols} FROM jobs WHERE (?1='' OR run=?1) AND (?2='' OR state=?2) AND id > ?3
             ORDER BY id LIMIT ?4"
        ))
        .map_err(e)?;
    st.query_map(params![run, state, after, limit as i64], job_from)
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)
}

/// The highest job id (0: none).
pub fn max_job_id(c: &Connection) -> StoreResult<i64> {
    c.query_row("SELECT COALESCE(MAX(id), 0) FROM jobs", [], |r| r.get(0))
        .map_err(e)
}

/// Jobs by state, across the cluster.
pub fn counts(c: &Connection) -> StoreResult<std::collections::BTreeMap<String, u64>> {
    let mut st = c
        .prepare("SELECT state, count(*) FROM jobs GROUP BY state")
        .map_err(e)?;
    st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
    })
    .map_err(e)?
    .collect::<Result<_, _>>()
    .map_err(e)
}

fn pct(sorted: &[i64], p: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// A run's summary (`None`: no such run).
pub fn run_summary(c: &Connection, run: &str) -> StoreResult<Option<RunSummary>> {
    let Some((created_ms, concurrency)) = c
        .query_row(
            "SELECT created_ms, concurrency FROM runs WHERE id=?1",
            [run],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u32)),
        )
        .optional()
        .map_err(e)?
    else {
        return Ok(None);
    };
    let mut s = RunSummary {
        run: run.to_string(),
        created_ms,
        concurrency,
        ..Default::default()
    };
    let mut st = c
        .prepare_cached(
            "SELECT state, submitted_ms, finished_ms, run_ms, create_ms, retries FROM jobs WHERE run=?1",
        )
        .map_err(e)?;
    let rows = st
        .query_map([run], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(e)?;
    let (mut e2e, mut runs, mut creates) = (Vec::new(), Vec::new(), Vec::new());
    let (mut first, mut last) = (i64::MAX, 0i64);
    for row in rows {
        let (state, sub, fin, run_ms, create_ms, retries) = row.map_err(e)?;
        s.total += 1;
        s.retries += retries as u64;
        first = first.min(sub);
        if state::is_terminal(&state) {
            last = last.max(fin);
            e2e.push(fin - sub);
            if run_ms > 0 {
                runs.push(run_ms);
            }
            if create_ms > 0 {
                creates.push(create_ms);
            }
        }
        *s.counts.entry(state).or_default() += 1;
    }
    let ended: u64 = s
        .counts
        .iter()
        .filter(|(k, _)| state::is_terminal(k))
        .map(|(_, v)| *v)
        .sum();
    s.done = s.total > 0 && ended == s.total;
    e2e.sort_unstable();
    runs.sort_unstable();
    creates.sort_unstable();
    s.e2e_p50_ms = pct(&e2e, 0.5);
    s.e2e_p99_ms = pct(&e2e, 0.99);
    s.run_p50_ms = pct(&runs, 0.5);
    s.create_p50_ms = pct(&creates, 0.5);
    s.create_p99_ms = pct(&creates, 0.99);
    if last > 0 && first != i64::MAX {
        s.span_ms = last - first;
    }
    Ok(Some(s))
}

/// Recent runs, newest first.
pub fn runs(c: &Connection, limit: usize) -> StoreResult<Vec<String>> {
    let mut st = c
        .prepare("SELECT id FROM runs ORDER BY created_ms DESC LIMIT ?1")
        .map_err(e)?;
    st.query_map([limit as i64], |r| r.get(0))
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)
}

/// Byte fields as base64 in the pool log.
mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let t = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(t.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

/// One state change of the coordinator as its writer records it: the unit
/// of the pool log (`rfcs/pool-self-coordination.md`). Applying the same
/// ops in the same order to the same state gives the same state, which is
/// what lets another machine take over from the log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// A job changes ([`Change`]).
    Change {
        /// The change.
        change: Change,
    },
    /// Jobs are submitted to a run (a key seen before is not added again).
    Submit {
        /// The run.
        run: String,
        /// Its concurrency limit (0: none).
        concurrency: u32,
        /// When.
        now_ms: i64,
        /// The jobs.
        jobs: Vec<NewJob>,
    },
    /// A host is added or changed.
    Node {
        /// Its row (never its token).
        row: NodeRow,
    },
    /// A host is drained (`draining`) or not (`ready`).
    NodeState {
        /// Host.
        name: String,
        /// `ready` or `draining`.
        state: String,
    },
    /// A host is removed (and its fences).
    NodeDelete {
        /// Host.
        name: String,
    },
    /// A setting of the coordinator (its pool id).
    Meta {
        /// Key.
        k: String,
        /// Value.
        v: String,
    },
    /// A durable Space's row is written.
    DurableSave {
        /// The row.
        row: serde_json::Value,
    },
    /// A durable Space is forgotten.
    DurableRemove {
        /// Its name.
        name: String,
    },
    /// A session's row is written (when the stored revision is
    /// `expect_rev`, 0: no row yet; `None`: whatever it is).
    SessionSave {
        /// The row.
        row: serde_json::Value,
        /// The compare-and-set.
        #[serde(default)]
        expect_rev: Option<u64>,
    },
    /// A session is forgotten.
    SessionRemove {
        /// Its name.
        name: String,
    },
    /// A gang run is submitted (once per run).
    GangSubmit {
        /// The run.
        run: String,
        /// When.
        now_ms: i64,
        /// What it is.
        gang: super::gang::GangSpec,
    },
    /// A queued gang is placed as `attempt`: its members' jobs start,
    /// dispatched to their hosts.
    GangPlace {
        /// The run.
        run: String,
        /// The new attempt.
        attempt: u32,
        /// When.
        now_ms: i64,
        /// Its members by rank.
        members: Vec<super::gang::GangMember>,
    },
    /// Every member of `attempt` ended, not all well: the gang waits for
    /// hosts again.
    GangRequeue {
        /// The run.
        run: String,
        /// The attempt that ended.
        attempt: u32,
        /// A code failure (it uses `retries`).
        failed: bool,
        /// An infrastructure failure (it uses `infra_retries`).
        #[serde(default)]
        infra: bool,
        /// A member ran long enough to refill the infrastructure budget.
        #[serde(default)]
        refill: bool,
        /// Why.
        error: String,
        /// Not placed again before this (Unix ms).
        ready_ms: i64,
    },
    /// A gang ends.
    GangEnd {
        /// The run.
        run: String,
        /// Its newest attempt.
        attempt: u32,
        /// `succeeded`, `failed` or `canceled`.
        state: String,
        /// Why.
        error: String,
        /// The job whose result is the run's (0: none).
        result_job: i64,
        /// When.
        at_ms: i64,
    },
    /// Its owner cancels a gang.
    GangCancel {
        /// The run.
        run: String,
        /// When.
        at_ms: i64,
    },
}

/// What applying an [`Op`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpOut {
    /// Whether it took effect.
    Applied(bool),
    /// Per submitted job: (id, the key existed already).
    Submitted(Vec<(i64, bool)>),
}

impl OpOut {
    /// Whether anything changed.
    pub fn took_effect(&self) -> bool {
        match self {
            OpOut::Applied(b) => *b,
            OpOut::Submitted(v) => v.iter().any(|(_, existed)| !existed),
        }
    }

    /// The flag of an [`OpOut::Applied`].
    pub fn applied(&self) -> bool {
        matches!(self, OpOut::Applied(true))
    }
}

impl Op {
    /// The word the pool log records it under.
    pub fn kind(&self) -> &'static str {
        match self {
            Op::Change { change } => match change {
                Change::Dispatched { .. } => "dispatched",
                Change::Running { .. } => "running",
                Change::Requeue { .. } => "requeue",
                Change::Complete { .. } => "complete",
                Change::EndQueued { .. } => "end_queued",
                Change::Canceling { .. } => "canceling",
                Change::Cursor { .. } => "cursor",
                Change::FenceAdd { .. } => "fence_add",
                Change::FenceDone { .. } => "fence_done",
            },
            Op::Submit { .. } => "submit",
            Op::Node { .. } => "node",
            Op::NodeState { .. } => "node_state",
            Op::NodeDelete { .. } => "node_delete",
            Op::Meta { .. } => "meta",
            Op::DurableSave { .. } => "durable_save",
            Op::DurableRemove { .. } => "durable_remove",
            Op::SessionSave { .. } => "session_save",
            Op::SessionRemove { .. } => "session_remove",
            Op::GangSubmit { .. } => "gang_submit",
            Op::GangPlace { .. } => "gang_place",
            Op::GangRequeue { .. } => "gang_requeue",
            Op::GangEnd { .. } => "gang_end",
            Op::GangCancel { .. } => "gang_cancel",
        }
    }

    /// Whether it goes to the pool log. A host's feed position does not: a
    /// coordinator that takes over reads every host's feed from the start.
    pub fn replicated(&self) -> bool {
        !matches!(
            self,
            Op::Change {
                change: Change::Cursor { .. }
            }
        )
    }

    /// The job it changes (the coordinator's id), if any.
    pub fn job(&self) -> Option<i64> {
        match self {
            Op::Change { change } => match change {
                Change::Dispatched { id, .. }
                | Change::Running { id, .. }
                | Change::Requeue { id, .. }
                | Change::Complete { id, .. }
                | Change::EndQueued { id, .. }
                | Change::Canceling { id }
                | Change::FenceAdd { id, .. }
                | Change::FenceDone { id, .. } => Some(*id),
                Change::Cursor { .. } => None,
            },
            _ => None,
        }
    }

    /// Whether it ends its job.
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Op::Change {
                change: Change::Complete { .. } | Change::EndQueued { .. }
            }
        )
    }
}

/// Applies `op` inside the caller's transaction.
pub fn apply_op(tx: &Connection, op: &Op) -> StoreResult<OpOut> {
    Ok(match op {
        Op::Change { change } => OpOut::Applied(apply_change(tx, change)?),
        Op::Submit {
            run,
            concurrency,
            now_ms,
            jobs,
        } => {
            ensure_run(tx, run, *concurrency, *now_ms)?;
            OpOut::Submitted(submit_in(tx, run, jobs, *now_ms)?)
        }
        Op::Node { row } => {
            let before = node(tx, &row.name)?;
            let mut row = row.clone();
            // The token is this machine's (direct hosts); a replicated row
            // keeps the one it had.
            if row.token.is_empty()
                && let Some(b) = &before
            {
                row.token = b.token.clone();
            }
            upsert_node(tx, &row)?;
            OpOut::Applied(before.is_none_or(|b| {
                let mut b = b;
                b.cursor = row.cursor;
                b.ledger_id = row.ledger_id.clone();
                b != row
            }))
        }
        Op::NodeState { name, state } => {
            let was = node(tx, name)?.map(|n| n.state);
            OpOut::Applied(was.is_some_and(|w| &w != state) && set_node_state(tx, name, state)?)
        }
        Op::NodeDelete { name } => {
            tx.execute("DELETE FROM fences WHERE node=?1", [name])
                .map_err(e)?;
            let n = tx
                .execute("DELETE FROM nodes WHERE name=?1", [name])
                .map_err(e)?;
            OpOut::Applied(n > 0)
        }
        Op::Meta { k, v } => {
            let was = meta_get(tx, k)?;
            meta_set(tx, k, v)?;
            OpOut::Applied(was.as_deref() != Some(v.as_str()))
        }
        Op::DurableSave { row } => OpOut::Applied(super::durable::save_json(tx, row)?),
        Op::DurableRemove { name } => OpOut::Applied(super::durable::remove_named(tx, name)?),
        Op::SessionSave { row, expect_rev } => {
            OpOut::Applied(super::sessions::save_json(tx, row, *expect_rev)?)
        }
        Op::SessionRemove { name } => OpOut::Applied(super::sessions::remove_named(tx, name)?),
        Op::GangSubmit { run, now_ms, gang } => {
            ensure_run(tx, run, 0, *now_ms)?;
            OpOut::Applied(super::gang::submit(tx, run, gang, *now_ms)?)
        }
        Op::GangPlace {
            run,
            attempt,
            now_ms,
            members,
        } => OpOut::Submitted(
            super::gang::place(tx, run, *attempt, members, *now_ms)?
                .into_iter()
                .map(|id| (id, false))
                .collect(),
        ),
        Op::GangRequeue {
            run,
            attempt,
            failed,
            infra,
            refill,
            error,
            ready_ms,
        } => OpOut::Applied(super::gang::requeue(
            tx, run, *attempt, *failed, *infra, *refill, error, *ready_ms,
        )?),
        Op::GangEnd {
            run,
            attempt,
            state,
            error,
            result_job,
            at_ms,
        } => OpOut::Applied(super::gang::end(
            tx,
            run,
            *attempt,
            state,
            error,
            *result_job,
            *at_ms,
        )?),
        Op::GangCancel { run, at_ms } => OpOut::Applied(super::gang::cancel(tx, run, *at_ms)?),
    })
}

/// Applies `ops` in one transaction (a coordinator without a pool log).
pub fn apply_ops(c: &mut Connection, ops: &[Op]) -> StoreResult<Vec<OpOut>> {
    let tx = c.transaction().map_err(e)?;
    let out = ops
        .iter()
        .map(|op| apply_op(&tx, op))
        .collect::<StoreResult<Vec<_>>>()?;
    tx.commit().map_err(e)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> JobSpec {
        JobSpec {
            argv: vec!["true".into()],
            millicpus: 1000,
            memory_mb: 512,
            timeout_secs: 60,
            max_output_bytes: 1024,
            ..Default::default()
        }
    }

    #[test]
    fn keys_make_submits_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("c.db")).unwrap();
        s.now(|c| {
            ensure_run(c, "r1", 0, 1)?;
            let jobs: Vec<NewJob> = (0..3)
                .map(|i| NewJob {
                    key: format!("r1/{i}"),
                    spec: spec(),
                    max_attempts: 3,
                })
                .collect();
            let a = submit(c, "r1", &jobs, 1)?;
            assert!(a.iter().all(|(_, existed)| !existed));
            let b = submit(c, "r1", &jobs, 2)?;
            assert_eq!(
                a.iter().map(|x| x.0).collect::<Vec<_>>(),
                b.iter().map(|x| x.0).collect::<Vec<_>>()
            );
            assert!(b.iter().all(|(_, existed)| *existed));
            assert_eq!(queued_after(c, 0, 10)?.len(), 3);
            Ok(())
        })
        .unwrap();
    }

    /// The first terminal result of the current attempt is recorded; a
    /// second one, or one for an older attempt, is not.
    #[test]
    fn a_completion_is_recorded_once_and_only_for_the_current_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("c.db")).unwrap();
        s.now(|c| {
            ensure_run(c, "r", 0, 1)?;
            let id = submit(
                c,
                "r",
                &[NewJob {
                    key: "k".into(),
                    spec: spec(),
                    max_attempts: 3,
                }],
                1,
            )?[0]
                .0;
            let done = |attempt: u32| Change::Complete {
                id,
                attempt,
                state: state::SUCCEEDED.into(),
                exit_code: Some(0),
                timed_out: false,
                error: String::new(),
                stdout: b"ok".to_vec(),
                stderr: vec![],
                result: vec![],
                space: String::new(),
                create_ms: 1,
                run_ms: 2,
                delete_ms: 3,
                at_ms: 9,
                ended_ms: 8,
            };
            let r = apply(
                c,
                &[
                    Change::Dispatched {
                        id,
                        node: "a".into(),
                        attempt: 1,
                        at_ms: 2,
                    },
                    Change::Requeue {
                        id,
                        attempt: 1,
                        error: "a went down".into(),
                        retry: true,
                    },
                    Change::Dispatched {
                        id,
                        node: "b".into(),
                        attempt: 2,
                        at_ms: 3,
                    },
                    Change::Running {
                        id,
                        attempt: 2,
                        at_ms: 4,
                    },
                    // The old attempt's late result: not recorded.
                    done(1),
                    done(2),
                    // A duplicate delivery of the result: not recorded.
                    done(2),
                ],
            )?;
            assert_eq!(r, vec![true, true, true, true, false, true, false]);
            let j = job(c, id)?.unwrap();
            assert_eq!(
                (
                    j.state.as_str(),
                    j.attempt,
                    j.node.as_str(),
                    j.stdout.as_str()
                ),
                ("succeeded", 2, "b", "ok")
            );
            let sum = run_summary(c, "r")?.unwrap();
            assert!(sum.done);
            assert_eq!(sum.retries, 1);
            Ok(())
        })
        .unwrap();
    }
}
