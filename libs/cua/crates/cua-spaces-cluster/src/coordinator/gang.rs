// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Gangs: a run whose jobs run together, one per host, as ranks
//! `0..n-1` of one program (data-parallel training, `nodes=(min, max)`).
//!
//! - **All or nothing.** A gang is placed only when at least `min`
//!   distinct hosts can each take a member now, and on as many as `max`.
//!   Every member of a placement is dispatched with the same gang attempt
//!   and environment: `CUA_RUN_ID`, `CUA_ATTEMPT`, `CUA_RANK`,
//!   `CUA_WORLD_SIZE` and `CUA_PEERS` (`ip:port` by rank, see [`peers`]).
//!   Each member is a job of its own (a new one per attempt), so a host's
//!   ledger, fencing and the coordinator's exactly-once completion apply
//!   to members unchanged.
//! - **Fail together.** When a member ends without success (failed, error,
//!   timed out, canceled by its host, or its host was lost) the others of
//!   its attempt are canceled at once. Only once every member of the
//!   attempt has ended is the gang queued again as the next attempt (at
//!   the largest size that fits), so two attempts of one rank never run at
//!   once.
//! - **Two budgets.** A *code failure* (a member exited non-zero, ran out
//!   of its own time, or stalled) uses `retries`, after a backoff with full
//!   jitter ([`Backoff`]). An *infrastructure failure* (a lost or
//!   restarted host, a member that never started, a host that fenced
//!   itself, a start-barrier timeout) uses `infra_retries` instead
//!   (default 10), which refills once a member of an attempt ran 10
//!   minutes. A member that never started because its host had no room
//!   uses neither.
//! - **Cut-off hosts.** A host that stops hearing from its coordinator
//!   stops its own members (`cua_cluster::node` self-fence); the gang of a
//!   lost host is placed again only once that host must have done so.
//! - **Start barrier** (`barrier_timeout_secs`). Members report ready (the
//!   cua jobs runtime does before the job's function runs); the
//!   coordinator releases them all once every member is ready, and ends
//!   the attempt (an infrastructure failure) when that takes longer.
//! - **Progress watchdog** (`stall_timeout_secs`). A member that reported
//!   progress (a heartbeat with a step) and then none for that long ends
//!   its attempt as stalled (a code failure).
//! - **Sticky.** A gang placed again prefers the hosts that ran it before:
//!   they hold its checkpoints (`CUA_JOB_DATA` is per run on each host).
//! - **One result.** Every member succeeded: the run succeeded, and its
//!   result is rank 0's.
//!
//! Every gang transition is an [`Op`](super::store::Op) recorded in the
//! pool log before it takes effect, like every other change.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::store::{StoreResult, state};
use cua_cluster::model::JobSpec;

/// Gang states.
pub mod gstate {
    /// Waiting for hosts (or for its backoff).
    pub const QUEUED: &str = "queued";
    /// Its members of the current attempt run.
    pub const RUNNING: &str = "running";
    /// Its owner canceled it; its members are stopping.
    pub const CANCELING: &str = "canceling";
    /// Ended: every member of an attempt succeeded.
    pub const SUCCEEDED: &str = "succeeded";
    /// Ended: it failed more times than it may retry.
    pub const FAILED: &str = "failed";
    /// Ended: canceled.
    pub const CANCELED: &str = "canceled";

    /// Whether `s` is an ended state.
    pub fn is_terminal(s: &str) -> bool {
        matches!(s, SUCCEEDED | FAILED | CANCELED)
    }
}

/// What a gang member that never started says first in its error: it does
/// not count as a failure of the gang.
pub const NOT_STARTED: &str = "not started: ";

/// What a member the coordinator stopped because another member of its
/// attempt ended badly says first (the reason of its cancel).
pub const STOPPED: &str = "stopped with its gang: ";

/// What a member that never started because its host had no room says
/// after [`NOT_STARTED`] (it uses no budget).
pub const NO_ROOM: &str = "no room on ";

/// What a member the progress watchdog stopped says first (a code failure).
pub const STALLED: &str = "stalled: ";

/// What the members of an attempt whose start barrier timed out say first
/// (an infrastructure failure).
pub const BARRIER_TIMEOUT: &str = "start barrier timed out: ";

/// A member that ran this long refills its gang's infrastructure budget.
pub const INFRA_REFILL_AFTER: std::time::Duration = std::time::Duration::from_secs(600);

/// The backoff before retrying after a code failure: before retry `n`
/// (1-based) a random pause in `[0, min(max, initial * coefficient^(n-1))]`
/// ("full jitter": gangs that failed together do not come back together).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backoff {
    /// The first retry's cap (ms).
    pub initial_ms: u64,
    /// Each retry's cap is this times the one before (>= 1).
    pub coefficient: f64,
    /// No pause is longer (ms).
    pub max_ms: u64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial_ms: 2_000,
            coefficient: 2.0,
            max_ms: 60_000,
        }
    }
}

impl Backoff {
    /// The cap of the pause before retry `n` (1-based).
    pub fn cap(&self, n: u32) -> std::time::Duration {
        let coef = if self.coefficient.is_finite() {
            self.coefficient.max(1.0)
        } else {
            1.0
        };
        let ms = self.initial_ms as f64 * coef.powi(n.saturating_sub(1).min(64) as i32);
        let ms = if ms.is_finite() { ms } else { f64::MAX };
        std::time::Duration::from_millis(ms.min(self.max_ms as f64).max(0.0) as u64)
    }

    /// The pause before retry `n`: uniform in `[0, cap(n)]`.
    pub fn delay(&self, n: u32) -> std::time::Duration {
        let cap = self.cap(n).as_millis() as u64;
        std::time::Duration::from_millis(if cap == 0 {
            0
        } else {
            rand::random_range(0..=cap)
        })
    }
}

/// How a gang retries and watches its members.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GangPolicy {
    /// Infrastructure failures it may retry after (refilled after a member
    /// ran [`INFRA_REFILL_AFTER`]).
    pub infra_retries: u32,
    /// The pause before a retry after a code failure.
    pub backoff: Backoff,
    /// Members wait at a start barrier at most this long (0: no barrier).
    pub barrier_timeout_secs: u32,
    /// A member that made no progress this long stalls its attempt (0: no
    /// watchdog). Armed by its first heartbeat with a step.
    pub stall_timeout_secs: u32,
}

/// Default infrastructure budget.
pub const INFRA_RETRIES: u32 = 10;

/// Default stall timeout (10 minutes).
pub const STALL_TIMEOUT_SECS: u32 = 600;

impl Default for GangPolicy {
    fn default() -> Self {
        Self {
            infra_retries: INFRA_RETRIES,
            backoff: Backoff::default(),
            barrier_timeout_secs: 0,
            stall_timeout_secs: STALL_TIMEOUT_SECS,
        }
    }
}

/// The first port of a gang's rendezvous range ([`port`]).
pub const PORT_BASE: u16 = 47_000;

/// Most hosts in one gang.
pub const MAX_NODES: u32 = 64;

/// What a gang run is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GangSpec {
    /// Fewest hosts it runs on.
    pub min: u32,
    /// Most hosts it runs on.
    pub max: u32,
    /// Code failures it may retry after (`retries` 3: at most 4 attempts
    /// that fail in its code).
    pub retries: u32,
    /// What every member runs (placement words included).
    pub spec: JobSpec,
    /// Infrastructure budget, backoff, start barrier and watchdog.
    #[serde(default)]
    pub policy: GangPolicy,
}

/// The fields in which two specs of one run differ (`spec.<field>` for
/// what the members run), for a submit that reuses a run id.
pub fn spec_diff(a: &GangSpec, b: &GangSpec) -> Vec<String> {
    let (va, vb) = (
        serde_json::to_value(a).unwrap_or_default(),
        serde_json::to_value(b).unwrap_or_default(),
    );
    let mut out = Vec::new();
    let keys = |v: &serde_json::Value| -> Vec<String> {
        v.as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut top = keys(&va);
    for k in keys(&vb) {
        if !top.contains(&k) {
            top.push(k);
        }
    }
    top.sort();
    for k in top {
        let (x, y) = (&va[&k], &vb[&k]);
        if x == y {
            continue;
        }
        if x.is_object() && y.is_object() {
            let mut sub = keys(x);
            for s in keys(y) {
                if !sub.contains(&s) {
                    sub.push(s);
                }
            }
            sub.sort();
            out.extend(
                sub.into_iter()
                    .filter(|s| x[s] != y[s])
                    .map(|s| format!("{k}.{s}")),
            );
        } else {
            out.push(k);
        }
    }
    out
}

/// A member as a placement records it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GangMember {
    /// Its rank.
    pub rank: u32,
    /// Its host.
    pub node: String,
    /// Its host's price when placed (US dollars an hour).
    #[serde(default)]
    pub usd_per_hour: f64,
    /// What it runs (with its `CUA_*` environment).
    pub spec: JobSpec,
}

/// A gang as stored.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GangRow {
    /// The run.
    pub run: String,
    /// Fewest hosts.
    pub min: u32,
    /// Most hosts.
    pub max: u32,
    /// Code failures it may retry after.
    pub retries: u32,
    /// What its members run.
    #[serde(skip)]
    pub spec: JobSpec,
    /// Infrastructure budget, backoff, barrier and watchdog.
    pub policy: GangPolicy,
    /// Infrastructure failures (lost machines, ...) so far.
    pub infra_failures: u32,
    /// Of those, the ones that count against `policy.infra_retries` (it
    /// refills after a member ran 10 minutes).
    pub infra_used: u32,
    /// Its state ([`gstate`]).
    pub state: String,
    /// The newest attempt (0: never placed).
    pub attempt: u32,
    /// Hosts of the newest attempt.
    pub size: u32,
    /// Attempts that failed in its code so far.
    pub failures: u32,
    /// Not placed again before this (Unix ms).
    pub ready_ms: i64,
    /// Why the last attempt ended, or why it failed.
    pub error: String,
    /// The job whose result is the run's (rank 0 of the attempt that
    /// succeeded).
    pub result_job: i64,
    /// Submitted (Unix ms).
    pub submitted_ms: i64,
    /// Ended (Unix ms; 0 while not).
    pub finished_ms: i64,
    /// Every host that ran one of its members.
    pub hosts: Vec<String>,
}

/// One member of one attempt, as `run` shows it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MemberRow {
    /// Its job.
    pub job: i64,
    /// The gang attempt.
    pub attempt: u32,
    /// Its rank.
    pub rank: u32,
    /// Its host.
    pub host: String,
    /// The job's state.
    pub state: String,
    /// Accepted by its host (Unix ms; 0: not yet).
    pub started_ms: i64,
    /// Ended (Unix ms; 0: not yet).
    pub finished_ms: i64,
    /// When its command ended, by its host's clock where the host said,
    /// else when the coordinator heard of it (Unix ms; 0: not yet). Orders
    /// the failures of an attempt's members (CUA-1826).
    pub ended_ms: i64,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// What went wrong.
    pub error: String,
    /// It failed because a peer went away (its ring or NCCL connection
    /// broke), as the last error in its output says: likely a
    /// consequence of another member's failure.
    pub peer_lost: bool,
    /// Its host's price (US dollars an hour).
    pub usd_per_hour: f64,
    /// Seconds it ran (so far).
    pub seconds: f64,
    /// `seconds` at `usd_per_hour` (an estimate).
    pub cost_usd: f64,
}

/// Creates the gang tables.
pub fn init(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS gangs(
           run TEXT PRIMARY KEY, min INTEGER NOT NULL, max INTEGER NOT NULL,
           retries INTEGER NOT NULL, spec TEXT NOT NULL, state TEXT NOT NULL,
           attempt INTEGER NOT NULL DEFAULT 0, size INTEGER NOT NULL DEFAULT 0,
           failures INTEGER NOT NULL DEFAULT 0, ready_ms INTEGER NOT NULL DEFAULT 0,
           error TEXT NOT NULL DEFAULT '', result_job INTEGER NOT NULL DEFAULT 0,
           submitted_ms INTEGER NOT NULL, finished_ms INTEGER NOT NULL DEFAULT 0,
           hosts TEXT NOT NULL DEFAULT '[]');
         CREATE TABLE IF NOT EXISTS gang_members(
           run TEXT NOT NULL, attempt INTEGER NOT NULL, rank INTEGER NOT NULL,
           job INTEGER NOT NULL, node TEXT NOT NULL, usd_per_hour REAL NOT NULL DEFAULT 0,
           PRIMARY KEY(run, attempt, rank));
         CREATE INDEX IF NOT EXISTS gang_members_job ON gang_members(job);",
    )?;
    // Columns added after the table first shipped.
    let have: Vec<String> = c
        .prepare("SELECT name FROM pragma_table_info('gangs')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for (col, def) in [
        ("policy", "TEXT NOT NULL DEFAULT '{}'"),
        ("infra_failures", "INTEGER NOT NULL DEFAULT 0"),
        ("infra_used", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !have.iter().any(|h| h == col) {
            c.execute_batch(&format!("ALTER TABLE gangs ADD COLUMN {col} {def};"))?;
        }
    }
    Ok(())
}

fn e(err: rusqlite::Error) -> String {
    format!("cluster database: {err}")
}

const GANG_COLS: &str = "run,min,max,retries,spec,state,attempt,size,failures,ready_ms,error,result_job,submitted_ms,finished_ms,hosts,policy,infra_failures,infra_used";

fn gang_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<GangRow> {
    let spec: String = r.get(4)?;
    let hosts: String = r.get(14)?;
    let policy: String = r.get(15)?;
    Ok(GangRow {
        run: r.get(0)?,
        min: r.get::<_, i64>(1)? as u32,
        max: r.get::<_, i64>(2)? as u32,
        retries: r.get::<_, i64>(3)? as u32,
        spec: serde_json::from_str(&spec).unwrap_or_default(),
        state: r.get(5)?,
        attempt: r.get::<_, i64>(6)? as u32,
        size: r.get::<_, i64>(7)? as u32,
        failures: r.get::<_, i64>(8)? as u32,
        ready_ms: r.get(9)?,
        error: r.get(10)?,
        result_job: r.get(11)?,
        submitted_ms: r.get(12)?,
        finished_ms: r.get(13)?,
        hosts: serde_json::from_str(&hosts).unwrap_or_default(),
        policy: serde_json::from_str(&policy).unwrap_or_default(),
        infra_failures: r.get::<_, i64>(16)? as u32,
        infra_used: r.get::<_, i64>(17)? as u32,
    })
}

/// One gang.
pub fn gang(c: &Connection, run: &str) -> StoreResult<Option<GangRow>> {
    c.query_row(
        &format!("SELECT {GANG_COLS} FROM gangs WHERE run=?1"),
        [run],
        gang_from,
    )
    .optional()
    .map_err(e)
}

/// Gangs that have not ended.
pub fn live_gangs(c: &Connection) -> StoreResult<Vec<GangRow>> {
    let mut st = c
        .prepare(&format!(
            "SELECT {GANG_COLS} FROM gangs WHERE state IN ('queued','running','canceling') ORDER BY submitted_ms"
        ))
        .map_err(e)?;
    st.query_map([], gang_from)
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)
}

/// Members (job ids) of live gangs whose jobs have not ended: job -> run.
pub fn live_members(c: &Connection) -> StoreResult<Vec<(i64, String)>> {
    let mut st = c
        .prepare(
            "SELECT m.job, m.run FROM gang_members m JOIN jobs j ON j.id=m.job
             WHERE j.state IN ('dispatched','running','canceling')",
        )
        .map_err(e)?;
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)
}

/// The members of `run` (every attempt, or only `attempt`), by attempt and
/// rank, with their jobs' states and a cost estimate (`now_ms` ends the
/// ones still running).
pub fn members(
    c: &Connection,
    run: &str,
    attempt: Option<u32>,
    now_ms: i64,
) -> StoreResult<Vec<MemberRow>> {
    let mut st = c
        .prepare_cached(
            "SELECT m.job, m.attempt, m.rank, m.node, m.usd_per_hour, j.state, j.started_ms,
                    j.finished_ms, j.exit_code, j.error, j.ended_ms, j.stdout, j.stderr
             FROM gang_members m JOIN jobs j ON j.id=m.job
             WHERE m.run=?1 AND (?2 < 0 OR m.attempt=?2) ORDER BY m.attempt, m.rank",
        )
        .map_err(e)?;
    st.query_map(params![run, attempt.map_or(-1, i64::from)], |r| {
        let usd: f64 = r.get(4)?;
        let started: i64 = r.get(6)?;
        let finished: i64 = r.get(7)?;
        let seconds = if started > 0 {
            ((if finished > 0 { finished } else { now_ms }) - started).max(0) as f64 / 1000.0
        } else {
            0.0
        };
        let ended: i64 = r.get(10)?;
        let error: String = r.get(9)?;
        let out = |i: usize| -> rusqlite::Result<Vec<u8>> {
            Ok(r.get::<_, Option<Vec<u8>>>(i)?.unwrap_or_default())
        };
        let peer_lost = peer_lost(&out(11)?, &out(12)?, &error);
        Ok(MemberRow {
            job: r.get(0)?,
            attempt: r.get::<_, i64>(1)? as u32,
            rank: r.get::<_, i64>(2)? as u32,
            host: r.get(3)?,
            usd_per_hour: usd,
            state: r.get(5)?,
            started_ms: started,
            finished_ms: finished,
            ended_ms: if ended > 0 { ended } else { finished },
            exit_code: r.get(8)?,
            error,
            peer_lost,
            seconds,
            cost_usd: seconds / 3600.0 * usd,
        })
    })
    .map_err(e)?
    .collect::<Result<_, _>>()
    .map_err(e)
}

/// The job key of a member: never one a submit makes (a run id has no
/// spaces).
pub fn member_key(run: &str, attempt: u32, rank: u32) -> String {
    format!("{run} gang a{attempt} r{rank}")
}

/// Records a gang run when new. Whether it was.
pub fn submit(c: &Connection, run: &str, g: &GangSpec, now_ms: i64) -> StoreResult<bool> {
    let spec = serde_json::to_string(&g.spec).map_err(|err| err.to_string())?;
    let policy = serde_json::to_string(&g.policy).map_err(|err| err.to_string())?;
    c.execute(
        "INSERT INTO gangs(run,min,max,retries,spec,state,submitted_ms,policy)
         VALUES(?1,?2,?3,?4,?5,'queued',?6,?7) ON CONFLICT(run) DO NOTHING",
        params![run, g.min, g.max, g.retries, spec, now_ms, policy],
    )
    .map(|n| n > 0)
    .map_err(e)
}

/// Places attempt `attempt` of a queued gang: its member jobs start
/// dispatched to their hosts. The new job ids by rank, or none when the
/// gang is not queued at the attempt before.
pub fn place(
    c: &Connection,
    run: &str,
    attempt: u32,
    members: &[GangMember],
    now_ms: i64,
) -> StoreResult<Vec<i64>> {
    let Some(g) = gang(c, run)? else {
        return Ok(vec![]);
    };
    if g.state != gstate::QUEUED || g.attempt + 1 != attempt || members.is_empty() {
        return Ok(vec![]);
    }
    let mut ids = Vec::with_capacity(members.len());
    let mut hosts = g.hosts.clone();
    for m in members {
        let spec = serde_json::to_string(&m.spec).map_err(|err| err.to_string())?;
        c.execute(
            "INSERT INTO jobs(key, run, spec, state, attempt, node, max_attempts, submitted_ms, dispatched_ms)
             VALUES(?1,?2,?3,'dispatched',1,?4,1,?5,?5)",
            params![member_key(run, attempt, m.rank), run, spec, m.node, now_ms],
        )
        .map_err(e)?;
        let id = c.last_insert_rowid();
        c.execute(
            "INSERT INTO gang_members(run, attempt, rank, job, node, usd_per_hour)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![run, attempt, m.rank, id, m.node, m.usd_per_hour],
        )
        .map_err(e)?;
        if !hosts.contains(&m.node) {
            hosts.push(m.node.clone());
        }
        ids.push(id);
    }
    c.execute(
        "UPDATE gangs SET state='running', attempt=?2, size=?3, hosts=?4 WHERE run=?1",
        params![
            run,
            attempt,
            members.len() as i64,
            serde_json::to_string(&hosts).unwrap_or_else(|_| "[]".into())
        ],
    )
    .map_err(e)?;
    Ok(ids)
}

/// Queues a running gang again after attempt `attempt` ended without
/// success (`failed`: a code failure; `infra`: an infrastructure failure;
/// `refill`: a member ran long enough to refill the infrastructure budget
/// first).
#[allow(clippy::too_many_arguments)]
pub fn requeue(
    c: &Connection,
    run: &str,
    attempt: u32,
    failed: bool,
    infra: bool,
    refill: bool,
    error: &str,
    ready_ms: i64,
) -> StoreResult<bool> {
    c.execute(
        "UPDATE gangs SET state='queued', failures=failures+?3, error=?4, ready_ms=?5,
           infra_failures=infra_failures+?6,
           infra_used=(CASE WHEN ?7 THEN 0 ELSE infra_used END)+?6
         WHERE run=?1 AND attempt=?2 AND state='running'",
        params![
            run,
            attempt,
            failed as i64,
            error,
            ready_ms,
            infra as i64,
            refill
        ],
    )
    .map(|n| n > 0)
    .map_err(e)
}

/// Ends a gang at attempt `attempt` (once).
pub fn end(
    c: &Connection,
    run: &str,
    attempt: u32,
    to: &str,
    error: &str,
    result_job: i64,
    at_ms: i64,
) -> StoreResult<bool> {
    c.execute(
        "UPDATE gangs SET state=?3, error=?4, result_job=?5, finished_ms=?6
         WHERE run=?1 AND attempt=?2 AND state IN ('queued','running','canceling')",
        params![run, attempt, to, error, result_job, at_ms],
    )
    .map(|n| n > 0)
    .map_err(e)
}

/// Its owner cancels a gang: a queued one ends now, a running one stops
/// its members.
pub fn cancel(c: &Connection, run: &str, at_ms: i64) -> StoreResult<bool> {
    let a = c
        .execute(
            "UPDATE gangs SET state='canceled', error='canceled by its owner', finished_ms=?2
             WHERE run=?1 AND state='queued'",
            params![run, at_ms],
        )
        .map_err(e)?;
    let b = c
        .execute(
            "UPDATE gangs SET state='canceling' WHERE run=?1 AND state='running'",
            params![run],
        )
        .map_err(e)?;
    Ok(a + b > 0)
}

/// The rendezvous port of a gang attempt: `47000 + (h % 500) * 2`, `h` a
/// stable hash of the run and the attempt (each rank listens on its
/// address at this port; the next one is free for a second channel).
pub fn port(run: &str, attempt: u32) -> u16 {
    use sha2::Digest as _;
    let d = sha2::Sha256::digest(format!("{run}/{attempt}").as_bytes());
    let h = u64::from_be_bytes(d[..8].try_into().expect("8 bytes"));
    PORT_BASE + ((h % 500) as u16) * 2
}

/// An address as hosts report it: `ip/prefix` (a bare `ip` is `/32`: no
/// subnet is assumed).
fn cidr(a: &str) -> Option<(std::net::Ipv4Addr, u8)> {
    let (ip, len) = match a.split_once('/') {
        Some((ip, len)) => (ip, len.parse::<u8>().ok()?.min(32)),
        None => (a, 32),
    };
    Some((ip.trim().parse().ok()?, len))
}

fn ip_of(a: &str) -> String {
    a.split('/').next().unwrap_or(a).trim().to_string()
}

fn in_net(ip: std::net::Ipv4Addr, net: std::net::Ipv4Addr, len: u8) -> bool {
    let mask = if len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(len))
    };
    u32::from(ip) & mask == u32::from(net) & mask
}

/// Whether two host addresses are on one subnet, each by its own
/// interface's netmask (each is in the other's network).
fn same_subnet(a: &str, b: &str) -> bool {
    let (Some((ia, la)), Some((ib, lb))) = (cidr(a), cidr(b)) else {
        return false;
    };
    ia != ib && la < 32 && lb < 32 && in_net(ib, ia, la) && in_net(ia, ib, lb)
}

fn tailnet(a: &str) -> bool {
    cidr(a).is_some_and(|(ip, _)| cua_cluster::sysinfo::is_tailnet(ip))
}

/// The address each member's peers reach it at, by rank, from each host's
/// `HostNodeStatus.addresses` (`ip/prefix`): one on the same subnet as an
/// address of every other member (by the interfaces' real netmasks), else
/// a tailnet (100.64.0.0/10) address when every member has one, else its
/// first address (`127.0.0.1` for a host that reports none). A shared
/// subnet does not prove the hosts reach each other (two sites can both
/// use 192.168.1.0/24): a host may route its peers another way
/// (`cua_cluster::peers::PeerRouter`, with `CUA_PEER_MACHINES`).
pub fn choose_addresses(addresses: &[Vec<String>]) -> Vec<String> {
    let all_tailnet = addresses.iter().all(|a| a.iter().any(|x| tailnet(x)));
    addresses
        .iter()
        .enumerate()
        .map(|(i, mine)| {
            let lan = mine.iter().find(|a| {
                addresses
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .all(|(_, other)| other.iter().any(|o| same_subnet(a, o)))
            });
            if let Some(a) = lan {
                return ip_of(a);
            }
            if all_tailnet && let Some(a) = mine.iter().find(|x| tailnet(x)) {
                return ip_of(a);
            }
            mine.first()
                .map(|a| ip_of(a))
                .unwrap_or_else(|| "127.0.0.1".into())
        })
        .collect()
}

/// `CUA_PEERS` of a gang attempt: `ip:port` by rank.
pub fn peers(run: &str, attempt: u32, addresses: &[Vec<String>]) -> String {
    let p = port(run, attempt);
    choose_addresses(addresses)
        .iter()
        .map(|a| format!("{a}:{p}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// What each member of a placement runs: the gang's spec with its rank's
/// `CUA_*` environment and the run id (the host keeps the run's data under
/// it).
///
/// `machines` are the members' machine ids by rank (`CUA_PEER_MACHINES`):
/// a host that routes rank traffic itself (a mesh forward) finds each
/// peer's machine there. `names` are their names in the pool by rank
/// (`CUA_PEER_NAMES`), for what members print about their peers.
pub fn member_specs(
    run: &str,
    attempt: u32,
    base: &JobSpec,
    addresses: &[Vec<String>],
    machines: &[String],
    names: &[String],
    barrier_timeout_secs: u32,
) -> Vec<JobSpec> {
    let n = addresses.len();
    let peers = peers(run, attempt, addresses);
    let machines = machines.join(",");
    let names = names.join(",");
    (0..n)
        .map(|rank| {
            let mut s = base.clone();
            s.run_id = run.to_string();
            let env: BTreeMap<&str, String> = [
                ("CUA_RUN_ID", run.to_string()),
                ("CUA_ATTEMPT", attempt.to_string()),
                ("CUA_RANK", rank.to_string()),
                ("CUA_WORLD_SIZE", n.to_string()),
                ("CUA_PEERS", peers.clone()),
                ("CUA_PEER_MACHINES", machines.clone()),
                ("CUA_PEER_NAMES", names.clone()),
            ]
            .into_iter()
            .collect();
            for (k, v) in env {
                s.env.insert(k.to_string(), v);
            }
            if barrier_timeout_secs > 0 {
                s.env.insert(
                    "CUA_BARRIER_TIMEOUT".into(),
                    barrier_timeout_secs.to_string(),
                );
            } else {
                s.env.remove("CUA_BARRIER_TIMEOUT");
            }
            s
        })
        .collect()
}

/// The pause before running a job (not a gang) again after `failures`
/// failures: 2 s, 4 s, 8 s, ... at most 60 s. A gang's follows its
/// [`GangPolicy::backoff`], with jitter.
pub fn backoff(failures: u32) -> std::time::Duration {
    Backoff::default().cap(failures.max(1))
}

/// What the members of an attempt say about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Members still run, and none ended badly.
    Running,
    /// Every member succeeded.
    Succeeded,
    /// A member ended badly while others still run: stop them.
    Stopping,
    /// Every member ended, at least one badly.
    Failed {
        /// A code failure (a member exited non-zero, ran out of its time,
        /// or stalled): it uses `retries`.
        code: bool,
        /// Otherwise an infrastructure failure (a lost host, a member that
        /// never started, a fenced host, a barrier timeout): it uses
        /// `infra_retries`. Neither: only members a full host turned away.
        infra: bool,
        /// Why (the first bad member's rank, host and error).
        why: String,
    },
}

/// Whether a member that ended badly (`state`, `error`) failed in its code.
pub fn code_failure(state: &str, error: &str) -> bool {
    state == state::FAILED || error.starts_with(STALLED)
}

/// Whether a member that ended badly uses no budget at all: it never
/// started because its host had no room.
fn free_failure(error: &str) -> bool {
    error
        .strip_prefix(NOT_STARTED)
        .is_some_and(|r| r.starts_with(NO_ROOM))
}

/// Whether a member ended because its host went down (the coordinator's
/// `<host> went down`, also after `canceled; `).
fn lost_host(error: &str) -> bool {
    error.ends_with(" went down")
}

/// What a peer's error says when another member went away first: an MLX
/// ring's or NCCL's broken connection.
const PEER_LOST: &[&str] = &[
    "connection to a peer was lost",
    "closed by the peer",
    "reset by peer",
    "connectionreseterror",
    "brokenpipeerror",
    "broken pipe",
    "remote process exited",
    "communicator was aborted",
    "peer closed",
];

/// Whether a member's failure is a broken connection to a peer, judged by
/// the last error its output raised (stderr, else stdout, else `error`):
/// a member whose own GPU timed out also logs its ring breaking, but
/// raised the GPU error.
pub fn peer_lost(stdout: &[u8], stderr: &[u8], error: &str) -> bool {
    let last_error = |text: &str| -> Option<String> {
        text.lines()
            .rev()
            .map(str::trim)
            .find(|l| raised(l))
            .map(str::to_ascii_lowercase)
    };
    let said = last_error(&String::from_utf8_lossy(stderr))
        .or_else(|| last_error(&String::from_utf8_lossy(stdout)))
        .unwrap_or_else(|| error.to_ascii_lowercase());
    PEER_LOST.iter().any(|p| said.contains(p))
}

/// Whether an output line is an error a program raised: a Python
/// exception (`RuntimeError: ...`, `torch.distributed.DistBackendError:
/// ...`) or a C++ one that ended it.
fn raised(line: &str) -> bool {
    if line.contains("uncaught exception") {
        return true;
    }
    let Some((name, _)) = line.split_once(": ") else {
        return false;
    };
    (name.ends_with("Error") || name.ends_with("Exception"))
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// A member of an attempt as [`verdict`] sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ended {
    /// Its rank.
    pub rank: u32,
    /// Its host.
    pub host: String,
    /// Its job's state.
    pub state: String,
    /// What went wrong.
    pub error: String,
    /// When it ended (Unix ms, its host's clock where known; 0: unknown).
    pub ended_ms: i64,
    /// Its failure is a broken connection to a peer ([`peer_lost`]).
    pub peer_lost: bool,
}

impl From<&MemberRow> for Ended {
    fn from(m: &MemberRow) -> Self {
        Self {
            rank: m.rank,
            host: m.host.clone(),
            state: m.state.clone(),
            error: m.error.clone(),
            ended_ms: m.ended_ms,
            peer_lost: m.peer_lost,
        }
    }
}

/// The order in which members that ended badly are blamed: the one that
/// failed first, by its host's clock; at the same time (or both unknown) a
/// lost host, then a member's own failure, then a peer whose connection
/// broke; then by rank (CUA-1826).
fn blame_order(m: &Ended) -> (i64, u8, u32) {
    let kind = if lost_host(&m.error) {
        0
    } else if m.peer_lost {
        2
    } else {
        1
    };
    let at = if m.ended_ms > 0 { m.ended_ms } else { i64::MAX };
    (at, kind, m.rank)
}

/// The member to blame among `bad` ones: [`blame_order`]'s first.
pub fn first_failure<'a>(bad: impl IntoIterator<Item = &'a Ended>) -> Option<&'a Ended> {
    bad.into_iter().min_by_key(|m| blame_order(m))
}

/// The verdict on an attempt from its members.
///
/// A member lost with its host explains the attempt: its peers' collectives
/// (an MLX ring, NCCL) break when it goes, so their own failures are a
/// consequence, not a code failure (CUA-1822). Otherwise the member that
/// failed first is named: a peer whose ring broke a second later is not
/// the cause (CUA-1826).
pub fn verdict(members: &[Ended]) -> Verdict {
    if members.is_empty() {
        return Verdict::Failed {
            code: false,
            infra: false,
            why: "no members".into(),
        };
    }
    let live = members.iter().any(|m| !state::is_terminal(&m.state));
    let bad: Vec<&Ended> = members
        .iter()
        .filter(|m| state::is_terminal(&m.state) && m.state != state::SUCCEEDED)
        .collect();
    if bad.is_empty() {
        return if live {
            Verdict::Running
        } else {
            Verdict::Succeeded
        };
    }
    if live {
        return Verdict::Stopping;
    }
    // The members that ended badly on their own (not stopped because of
    // another).
    let own: Vec<&Ended> = bad
        .iter()
        .copied()
        .filter(|m| !m.error.starts_with(STOPPED))
        .collect();
    if let Some(lost) = first_failure(own.iter().copied().filter(|m| lost_host(&m.error))) {
        return Verdict::Failed {
            code: false,
            infra: true,
            why: format!(
                "{} (rank {})",
                lost.error.strip_prefix("canceled; ").unwrap_or(&lost.error),
                lost.rank
            ),
        };
    }
    let code = own.iter().any(|m| code_failure(&m.state, &m.error));
    let infra = !code && own.iter().any(|m| !free_failure(&m.error));
    let first = first_failure(own.iter().copied().filter(|m| !free_failure(&m.error)))
        .or_else(|| own.first().copied())
        .or_else(|| bad.first().copied())
        .expect("a bad member");
    Verdict::Failed {
        code,
        infra,
        why: format!(
            "rank {} on {} {}{}",
            first.rank,
            first.host,
            first.state,
            if first.error.is_empty() {
                String::new()
            } else {
                format!(": {}", first.error)
            }
        ),
    }
}

/// One line on how a gang's attempts went, e.g. `attempt 4 · 1 code
/// failure, 2 lost machines`.
pub fn summary(g: &GangRow) -> String {
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if g.failures > 0 {
        parts.push(plural(g.failures, "code failure", "code failures"));
    }
    if g.infra_failures > 0 {
        parts.push(plural(g.infra_failures, "lost computer", "lost computers"));
    }
    if parts.is_empty() {
        format!("attempt {}", g.attempt)
    } else {
        format!("attempt {} · {}", g.attempt, parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn peers_share_a_lan_else_a_tailnet_else_their_first_address() {
        // Same LAN: the LAN addresses, whatever else they have.
        let lan = [
            v(&["192.168.1.10/24", "100.70.0.1/32"]),
            v(&["10.0.0.5/24", "192.168.1.11/24", "100.70.0.2/32"]),
        ];
        assert_eq!(choose_addresses(&lan), ["192.168.1.10", "192.168.1.11"]);
        // By the real netmask: an AWS /20 subnet shares beyond a /24 ...
        let aws = [v(&["172.31.17.4/20"]), v(&["172.31.30.9/20"])];
        assert_eq!(choose_addresses(&aws), ["172.31.17.4", "172.31.30.9"]);
        assert!(same_subnet("172.31.17.4/20", "172.31.30.9/20"));
        // ... and a /25 does not cover its whole /24.
        assert!(!same_subnet("192.168.1.10/25", "192.168.1.200/25"));
        // A bare address assumes no subnet.
        assert!(!same_subnet("192.168.1.10", "192.168.1.11"));
        // Different LANs, all on the tailnet: the tailnet.
        let tn = [
            v(&["192.168.1.10/24", "100.70.0.1/32"]),
            v(&["10.0.0.5/24", "100.80.0.2/32"]),
        ];
        assert_eq!(choose_addresses(&tn), ["100.70.0.1", "100.80.0.2"]);
        // Neither: each one's first address.
        let none = [
            v(&["192.168.1.10/24"]),
            v(&["10.0.0.5/24", "100.80.0.2/32"]),
            v(&[]),
        ];
        assert_eq!(
            choose_addresses(&none),
            ["192.168.1.10", "10.0.0.5", "127.0.0.1"]
        );
        // One member: its first address.
        assert_eq!(choose_addresses(&[v(&["10.1.1.1"])]), ["10.1.1.1"]);
    }

    #[test]
    fn ports_are_stable_even_and_in_range() {
        let p = port("r1", 1);
        assert_eq!(p, port("r1", 1));
        assert!((PORT_BASE..PORT_BASE + 1000).contains(&p));
        assert_eq!((p - PORT_BASE) % 2, 0);
        let spread: std::collections::HashSet<u16> = (1..50).map(|a| port("r1", a)).collect();
        assert!(spread.len() > 10);
    }

    #[test]
    fn members_get_their_rank_world_and_peers() {
        let base = JobSpec {
            argv: vec!["python".into()],
            ..Default::default()
        };
        let specs = member_specs(
            "train",
            3,
            &base,
            &[v(&["192.168.1.10/24"]), v(&["192.168.1.11/24"])],
            &["m-a".into(), "m-b".into()],
            &["studio".into(), "macbook".into()],
            900,
        );
        let p = port("train", 3);
        assert_eq!(specs.len(), 2);
        for (rank, s) in specs.iter().enumerate() {
            assert_eq!(s.env["CUA_RANK"], rank.to_string());
            assert_eq!(s.env["CUA_WORLD_SIZE"], "2");
            assert_eq!(s.env["CUA_ATTEMPT"], "3");
            assert_eq!(s.env["CUA_RUN_ID"], "train");
            assert_eq!(
                s.env["CUA_PEERS"],
                format!("192.168.1.10:{p},192.168.1.11:{p}")
            );
            assert_eq!(s.env["CUA_PEER_MACHINES"], "m-a,m-b");
            assert_eq!(s.env["CUA_PEER_NAMES"], "studio,macbook");
            assert_eq!(s.env["CUA_BARRIER_TIMEOUT"], "900");
            assert_eq!(s.run_id, "train");
        }
    }

    #[test]
    fn backoff_caps_follow_the_policy_and_delays_are_full_jitter() {
        let d = Backoff::default();
        let caps: Vec<u64> = (1..9).map(|n| d.cap(n).as_secs()).collect();
        assert_eq!(caps, [2, 4, 8, 16, 32, 60, 60, 60]);
        let plain: Vec<u64> = (1..9).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(plain, caps);
        let b = Backoff {
            initial_ms: 500,
            coefficient: 3.0,
            max_ms: 10_000,
        };
        let caps: Vec<u64> = (1..6).map(|n| b.cap(n).as_millis() as u64).collect();
        assert_eq!(caps, [500, 1500, 4500, 10_000, 10_000]);
        // Full jitter: within [0, cap], and spread out (not all at the cap).
        let delays: Vec<u64> = (0..200).map(|_| b.delay(3).as_millis() as u64).collect();
        assert!(delays.iter().all(|&ms| ms <= 4500));
        let distinct: std::collections::HashSet<u64> = delays.iter().copied().collect();
        assert!(distinct.len() > 50, "{distinct:?}");
        assert!(delays.iter().any(|&ms| ms < 1500) && delays.iter().any(|&ms| ms > 3000));
        // A coefficient below 1 or a huge exponent stays sane.
        let odd = Backoff {
            initial_ms: 1000,
            coefficient: 0.1,
            max_ms: 5000,
        };
        assert_eq!(odd.cap(5).as_millis(), 1000);
        assert_eq!(b.cap(1000).as_millis(), 10_000);
        assert_eq!(
            Backoff { initial_ms: 0, ..b }.delay(3),
            std::time::Duration::ZERO
        );
    }

    #[test]
    fn spec_diffs_name_the_fields() {
        let a = GangSpec {
            min: 1,
            max: 2,
            retries: 3,
            spec: JobSpec {
                argv: vec!["python".into()],
                ..Default::default()
            },
            policy: GangPolicy::default(),
        };
        assert!(spec_diff(&a, &a.clone()).is_empty());
        let mut b = a.clone();
        b.max = 3;
        b.spec.argv = vec!["python3".into()];
        b.policy.infra_retries = 2;
        assert_eq!(
            spec_diff(&a, &b),
            ["max", "policy.infra_retries", "spec.argv"]
        );
    }

    #[test]
    fn verdicts() {
        let m = |r: u32, st: &str, err: &str| Ended {
            rank: r,
            host: "h".into(),
            state: st.into(),
            error: err.into(),
            ..Default::default()
        };
        assert_eq!(
            verdict(&[m(0, "running", ""), m(1, "succeeded", "")]),
            Verdict::Running
        );
        assert_eq!(
            verdict(&[m(0, "succeeded", ""), m(1, "succeeded", "")]),
            Verdict::Succeeded
        );
        assert_eq!(
            verdict(&[m(0, "running", ""), m(1, "failed", "")]),
            Verdict::Stopping
        );
        // A lost host: an infrastructure failure, not a code one, and the
        // lost host is named.
        assert_eq!(
            verdict(&[m(0, "canceled", "x"), m(1, "error", "h went down")]),
            Verdict::Failed {
                code: false,
                infra: true,
                why: "h went down (rank 1)".into()
            }
        );
        // Never started: infrastructure (never delivered) ...
        assert!(matches!(
            verdict(&[
                m(0, "error", "not started: h never received it"),
                m(1, "canceled", "stopped with its gang: rank 0")
            ]),
            Verdict::Failed {
                code: false,
                infra: true,
                ..
            }
        ));
        // ... or nothing at all (a full host turned it away).
        assert!(matches!(
            verdict(&[
                m(0, "error", "not started: no room on h: full"),
                m(1, "canceled", "stopped with its gang: rank 0")
            ]),
            Verdict::Failed {
                code: false,
                infra: false,
                ..
            }
        ));
        // A stalled member is a code failure; a barrier timeout and a
        // self-fenced host are not.
        assert!(matches!(
            verdict(&[
                m(0, "canceled", "stalled: no progress for 600 s"),
                m(1, "canceled", "stopped with its gang: rank 0")
            ]),
            Verdict::Failed { code: true, .. }
        ));
        assert!(matches!(
            verdict(&[
                m(0, "canceled", "start barrier timed out: rank 1 not ready"),
                m(1, "canceled", "start barrier timed out: rank 1 not ready")
            ]),
            Verdict::Failed {
                code: false,
                infra: true,
                ..
            }
        ));
        assert!(matches!(
            verdict(&[m(
                0,
                "canceled",
                "fenced: no word from its scheduler for 60 s"
            )]),
            Verdict::Failed {
                code: false,
                infra: true,
                ..
            }
        ));
        // A peer that fails because another member's host went down (its
        // ring broke) is not a code failure: the lost host is the cause.
        assert_eq!(
            verdict(&[m(0, "error", "h went down"), m(1, "failed", "")]),
            Verdict::Failed {
                code: false,
                infra: true,
                why: "h went down (rank 0)".into()
            }
        );
        // As in a real run: the survivor's ring broke first (exit 1), then
        // the gang's cancel of the lost member landed as its host went down.
        let r = |r: u32, h: &str, st: &str, err: &str| Ended {
            rank: r,
            host: h.into(),
            state: st.into(),
            error: err.into(),
            ..Default::default()
        };
        assert_eq!(
            verdict(&[
                r(
                    0,
                    "aws-1",
                    "failed",
                    "exited with code 1: [ring] connection to a peer was lost"
                ),
                r(1, "aws-2", "canceled", "canceled; aws-2 went down")
            ]),
            Verdict::Failed {
                code: false,
                infra: true,
                why: "aws-2 went down (rank 1)".into()
            }
        );
        // The failing member is named, not the one stopped with it.
        assert_eq!(
            verdict(&[
                m(0, "canceled", "stopped with its gang: rank 1"),
                m(1, "failed", "")
            ]),
            Verdict::Failed {
                code: true,
                infra: false,
                why: "rank 1 on h failed".into()
            }
        );
    }

    #[test]
    fn the_member_that_failed_first_is_blamed_not_the_lowest_rank() {
        // As in run p-finetune-b8f9b48e (CUA-1826): rank 1's Metal GPU timed
        // out, and rank 0's ring broke 1.3 s later; both exited 1.
        const METAL: &str = "  File \"trainer.py\", line 331, in train\n    mx.eval(state)\nRuntimeError: [METAL] Command buffer execution failed: Caused GPU Timeout Error (00000002:kIOGPUCommandBufferCallbackErrorTimeout).\n[ring] Socket 4 was closed by the peer\n[ring] Too many send/recv errors. Failing pending tasks...\ncua: mlx_lm.lora exited with status 1\n";
        const RING: &str = "[ring] Socket 4 was closed by the peer\n[ring] Too many send/recv errors. Failing pending tasks...\n    mx.eval(state)\nRuntimeError: [ring] connection to a peer was lost\ncua: mlx_lm.lora exited with status 1\n";
        assert!(!peer_lost(METAL.as_bytes(), b"", "exited with code 1"));
        assert!(peer_lost(RING.as_bytes(), b"", "exited with code 1"));
        // NCCL, and a C++ abort.
        assert!(peer_lost(
            b"",
            b"torch.distributed.DistBackendError: NCCL error: remote process exited or there was a network error",
            ""
        ));
        assert!(peer_lost(
            b"libc++abi: terminating due to uncaught exception of type std::runtime_error: [ring] connection to a peer was lost",
            b"",
            ""
        ));
        // Nothing raised: the error says it.
        assert!(!peer_lost(b"loss 1.2\n", b"", "exited with code 1"));

        let at = |rank: u32, host: &str, ms: i64, out: &str| Ended {
            rank,
            host: host.into(),
            state: "failed".into(),
            error: "exited with code 1: cua: mlx_lm.lora exited with status 1".into(),
            ended_ms: ms,
            peer_lost: peer_lost(out.as_bytes(), b"", ""),
        };
        let why = |v: Verdict| match v {
            Verdict::Failed { why, code, .. } => {
                assert!(code);
                why
            }
            other => panic!("{other:?}"),
        };
        // Rank 1 first, rank 0's ring breaks 200 ms later: rank 1.
        let t = 1_792_196_816_864;
        let members = [at(0, "macbook", t + 200, RING), at(1, "studio", t, METAL)];
        assert!(why(verdict(&members)).starts_with("rank 1 on studio failed"));
        // The other way round: rank 0.
        let members = [at(0, "macbook", t, METAL), at(1, "studio", t + 200, RING)];
        assert!(why(verdict(&members)).starts_with("rank 0 on macbook failed"));
        // By time, not by kind: a peer whose ring broke first is blamed
        // (its peer may have hung, not crashed).
        let members = [at(0, "macbook", t + 200, METAL), at(1, "studio", t, RING)];
        assert!(why(verdict(&members)).starts_with("rank 1 on studio"));
        // At the same time, or with no times: the own failure before the
        // broken ring, whatever the ranks.
        let members = [at(0, "macbook", t, RING), at(1, "studio", t, METAL)];
        assert!(why(verdict(&members)).starts_with("rank 1 on studio"));
        let members = [at(0, "macbook", 0, RING), at(1, "studio", 0, METAL)];
        assert!(why(verdict(&members)).starts_with("rank 1 on studio"));
        // A known time comes before an unknown one.
        let members = [at(0, "macbook", 0, METAL), at(1, "studio", t, RING)];
        assert!(why(verdict(&members)).starts_with("rank 1 on studio"));
        // A lost host still wins, even after a member's own failure.
        let mut lost = at(1, "studio", t + 30_000, "");
        lost.state = "error".into();
        lost.error = "studio went down".into();
        let members = [at(0, "macbook", t, METAL), lost];
        assert_eq!(
            verdict(&members),
            Verdict::Failed {
                code: false,
                infra: true,
                why: "studio went down (rank 1)".into()
            }
        );
        // The member stopped with its gang is never blamed, however early.
        let mut stopped = at(0, "macbook", t - 5_000, "");
        stopped.state = "canceled".into();
        stopped.error = format!("{STOPPED}rank 1 on studio failed");
        let members = [stopped, at(1, "studio", t, METAL)];
        assert!(why(verdict(&members)).starts_with("rank 1 on studio"));
    }
}
