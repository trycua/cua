// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Where the coordinator's state changes go before they take effect.
//!
//! - [`LocalJournal`]: nowhere else (a machine's own runs; machines
//!   reached without a relay). The database is the only record.
//! - A pool log (the relay's, [`crate::pool::relay::RelayLog`]): the
//!   writer appends each group of changes under its lease's epoch and only
//!   then commits them, so another machine that takes the lease rebuilds
//!   the same state from the log ([`rebuild`]).

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::store::{self, Op, OpOut, Store, StoreResult};

/// One entry as the pool log stores it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Position (from 1; 0 before it is stored).
    #[serde(default)]
    pub seq: u64,
    /// The op's word ([`Op::kind`]).
    pub kind: String,
    /// The job it changes, as hosts know it (`<pool>-<id>`; empty: none).
    #[serde(default)]
    pub job: String,
    /// It ends the job (the log refuses a second).
    #[serde(default)]
    pub terminal: bool,
    /// The run of a submit.
    #[serde(default)]
    pub run: String,
    /// The machine an attempt was dispatched to (`dispatched` only): the
    /// placement, kept readable on the relay like the job id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node: String,
    /// The attempt a `dispatched` entry starts.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attempt: u32,
    /// The op (sealed on the relay: see [`crate::pool::seal`]).
    pub data: serde_json::Value,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl LogEntry {
    /// The entry recording `op` of pool `pool`.
    pub fn of(op: &Op, pool: &str) -> Self {
        Self {
            seq: 0,
            kind: op.kind().into(),
            job: op
                .job()
                .map(|id| format!("{pool}-{id}"))
                .unwrap_or_default(),
            terminal: op.terminal(),
            run: match op {
                Op::Submit { run, .. } | Op::GangSubmit { run, .. } => run.clone(),
                _ => String::new(),
            },
            node: match op {
                Op::Change {
                    change: store::Change::Dispatched { node, .. },
                } => node.clone(),
                _ => String::new(),
            },
            attempt: match op {
                Op::Change {
                    change: store::Change::Dispatched { attempt, .. },
                } => *attempt,
                _ => 0,
            },
            data: serde_json::to_value(op).unwrap_or_default(),
        }
    }
}

/// Why an append did not happen.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    /// Another machine coordinates now (this epoch is over): stop.
    #[error("fenced: {0}")]
    Fenced(String),
    /// The log could not be reached or refused for now: try again.
    #[error("{0}")]
    Unavailable(String),
    /// The log refused data sealed under an older key generation (a device
    /// left and the keys changed): seal again with the new keys, then try
    /// again.
    #[error("rekey: {0}")]
    Rekey(String),
    /// The log has moved on from what this machine applied (a bug, or two
    /// writers): rebuild.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The log holds as much as it may until a snapshot truncates it (the
    /// relay's `snapshot_needed`, or the account's pool at its quota):
    /// write a snapshot, then try again. Nothing was appended.
    #[error("full: {0}")]
    Full(String),
}

/// Where the writer records ops before they take effect.
#[async_trait]
pub trait Journal: Send + Sync + 'static {
    /// Whether ops go anywhere (false: the database is the record).
    fn replicated(&self) -> bool;

    /// Appends `entries` after position `from`; returns the new head. A
    /// retry of an append that was stored returns the same head.
    async fn append(&self, from: u64, entries: Vec<LogEntry>) -> Result<u64, JournalError>;

    /// Stores a snapshot of the state at position `seq` (the log before it
    /// may then be dropped).
    async fn snapshot(&self, seq: u64, data: Vec<u8>) -> Result<(), JournalError>;
}

/// No log: the database is the record.
pub struct LocalJournal;

#[async_trait]
impl Journal for LocalJournal {
    fn replicated(&self) -> bool {
        false
    }

    async fn append(&self, from: u64, entries: Vec<LogEntry>) -> Result<u64, JournalError> {
        Ok(from + entries.len() as u64)
    }

    async fn snapshot(&self, _seq: u64, _data: Vec<u8>) -> Result<(), JournalError> {
        Ok(())
    }
}

/// What [`rebuild`] reads.
#[async_trait]
pub trait LogReader: Send + Sync {
    /// Entries after `after` (at most a page) and the head; `Ok(None)`
    /// when the log before `after` was dropped for a snapshot.
    async fn read(&self, after: u64) -> Result<Option<(Vec<LogEntry>, u64)>, JournalError>;

    /// The snapshot: (its position, its bytes).
    async fn snapshot(&self) -> Result<Option<(u64, Vec<u8>)>, JournalError>;
}

/// The meta key of the log position the database reflects.
pub const SEQ_KEY: &str = "pool_seq";
/// The meta key of the pool (log) the database belongs to.
pub const POOL_KEY: &str = "pool_log";

/// Brings the database at `path` to the head of the log `reader` reads
/// (`pool`: which log, so a database of another one is replaced): from
/// where it is, or from the snapshot when the log it needs was dropped.
/// Every entry must take effect again exactly as it did; one that does not
/// means the state diverged, and the rebuild fails.
pub async fn rebuild(
    path: &std::path::Path,
    pool: &str,
    reader: &dyn LogReader,
) -> Result<u64, String> {
    let mut store = Store::open(path)?;
    let (mine, at) = store
        .call(|c| {
            Ok((
                store::meta_get(c, POOL_KEY)?,
                store::meta_get(c, SEQ_KEY)?
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0),
            ))
        })
        .await?;
    let mut at = at;
    if mine.as_deref() != Some(pool) {
        drop(store);
        fresh(path)?;
        store = Store::open(path)?;
        at = 0;
    }
    let mut first = true;
    loop {
        match reader.read(at).await.map_err(|e| e.to_string())? {
            Some((entries, head)) => {
                if first && head < at {
                    // The log is shorter than what this database applied:
                    // a log that was reset. Start over.
                    drop(store);
                    fresh(path)?;
                    store = Store::open(path)?;
                    at = 0;
                    first = false;
                    continue;
                }
                first = false;
                if entries.is_empty() {
                    let p = pool.to_string();
                    store
                        .call(move |c| store::meta_set(c, POOL_KEY, &p))
                        .await?;
                    return Ok(at);
                }
                let p = pool.to_string();
                at = store.call(move |c| apply_entries(c, &entries, &p)).await?;
                if at >= head {
                    return Ok(at);
                }
            }
            None => {
                let Some((seq, data)) = reader.snapshot().await.map_err(|e| e.to_string())? else {
                    return Err("the pool log was cut for a snapshot that is missing".into());
                };
                drop(store);
                install_snapshot(path, &data)?;
                store = Store::open(path)?;
                let p = pool.to_string();
                store
                    .call(move |c| {
                        store::meta_set(c, SEQ_KEY, &seq.to_string())?;
                        store::meta_set(c, POOL_KEY, &p)
                    })
                    .await?;
                at = seq;
                first = false;
            }
        }
    }
}

fn apply_entries(
    c: &mut rusqlite::Connection,
    entries: &[LogEntry],
    pool: &str,
) -> StoreResult<u64> {
    let tx = c
        .transaction()
        .map_err(|e| format!("cluster database: {e}"))?;
    let mut at = store::meta_get(&tx, SEQ_KEY)?
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    for entry in entries {
        if entry.seq <= at {
            continue;
        }
        if entry.seq != at + 1 {
            return Err(format!("the pool log skips from {at} to {}", entry.seq));
        }
        let op: Op = serde_json::from_value(entry.data.clone())
            .map_err(|e| format!("pool log entry {}: {e}", entry.seq))?;
        let out = store::apply_op(&tx, &op)?;
        if !out.took_effect() {
            return Err(format!(
                "pool log entry {} ({}) did not take effect again: the state diverged",
                entry.seq, entry.kind
            ));
        }
        at = entry.seq;
    }
    store::meta_set(&tx, SEQ_KEY, &at.to_string())?;
    store::meta_set(&tx, POOL_KEY, pool)?;
    tx.commit().map_err(|e| format!("cluster database: {e}"))?;
    Ok(at)
}

/// Removes the database at `path` (and its WAL files).
fn fresh(path: &std::path::Path) -> Result<(), String> {
    for suffix in ["", "-wal", "-shm"] {
        let p = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", p.display())),
        }
    }
    Ok(())
}

/// Replaces the database at `path` with a snapshot ([`snapshot_bytes`]).
fn install_snapshot(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    use std::io::Read as _;
    fresh(path)?;
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut raw)
        .map_err(|e| format!("pool snapshot: {e}"))?;
    let tmp = path.with_extension("snapshot.tmp");
    std::fs::write(&tmp, &raw).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Bytes of ended jobs' result files a snapshot keeps (the newest first):
/// results are large (a model adapter), a snapshot must stay under the
/// relay's limit, and nothing schedules on an old run's result.
pub const SNAPSHOT_RESULTS_BUDGET: u64 = 16 << 20;

/// A gzip of the database (`VACUUM INTO`), for a snapshot. Taken by the
/// writer between groups, so it is the state at the position it records,
/// except that results of ended jobs past [`SNAPSHOT_RESULTS_BUDGET`]
/// (oldest first) are left out: a coordinator that starts from it answers
/// `run_result` for those runs without a result.
pub fn snapshot_bytes(c: &rusqlite::Connection, dir: &std::path::Path) -> StoreResult<Vec<u8>> {
    use std::io::Write as _;
    let tmp = dir.join(format!("snapshot-{:016x}.db", rand::random::<u64>()));
    c.execute("VACUUM INTO ?1", [tmp.display().to_string()])
        .map_err(|e| format!("cluster database snapshot: {e}"))?;
    if let Err(e) = trim_results(&tmp, SNAPSHOT_RESULTS_BUDGET) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let raw = std::fs::read(&tmp).map_err(|e| format!("{}: {e}", tmp.display()));
    let _ = std::fs::remove_file(&tmp);
    let raw = raw?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw)
        .and_then(|_| gz.finish())
        .map_err(|e| format!("pool snapshot: {e}"))
}

/// Drops the result files of ended jobs in the database at `path` once the
/// newer ones add up to `budget` bytes, and compacts it.
fn trim_results(path: &std::path::Path, budget: u64) -> StoreResult<()> {
    let e = |e: rusqlite::Error| format!("pool snapshot: {e}");
    let c = rusqlite::Connection::open(path).map_err(e)?;
    let sizes: Vec<(i64, u64)> = {
        let mut q = c
            .prepare(
                "SELECT id, length(result) FROM jobs WHERE result IS NOT NULL \
                 AND finished_ms > 0 ORDER BY finished_ms DESC, id DESC",
            )
            .map_err(e)?;
        q.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64))
        })
        .map_err(e)?
        .collect::<rusqlite::Result<_>>()
        .map_err(e)?
    };
    let mut kept = 0u64;
    let mut dropped = 0;
    for (id, size) in sizes {
        if kept + size <= budget {
            kept += size;
            continue;
        }
        c.execute("UPDATE jobs SET result = NULL WHERE id = ?1", [id])
            .map_err(e)?;
        dropped += 1;
    }
    if dropped > 0 {
        c.execute_batch("VACUUM").map_err(e)?;
    }
    Ok(())
}

/// What the writer needs to record a group: the journal, the pool id the
/// log's job keys use, and the runtime to wait on the journal from the
/// database thread.
#[derive(Clone)]
pub struct Recorder {
    /// The journal.
    pub journal: Arc<dyn Journal>,
    /// The pool id (the prefix of job ids on hosts).
    pub pool: String,
    /// The runtime.
    pub handle: tokio::runtime::Handle,
}

/// The outcome of recording a group.
pub type Recorded = Result<Vec<OpOut>, JournalError>;

/// Applies `ops` in one transaction and, when the journal is replicated,
/// appends the ones that took effect before committing. Runs on the
/// database thread (it waits for the journal there).
pub fn record(c: &mut rusqlite::Connection, rec: &Recorder, ops: &[Op]) -> StoreResult<Recorded> {
    let tx = c
        .transaction()
        .map_err(|e| format!("cluster database: {e}"))?;
    let mut outs = Vec::with_capacity(ops.len());
    let mut entries = Vec::new();
    let replicated = rec.journal.replicated();
    for op in ops {
        let out = store::apply_op(&tx, op)?;
        if replicated && op.replicated() && out.took_effect() {
            entries.push(LogEntry::of(op, &rec.pool));
        }
        outs.push(out);
    }
    if !entries.is_empty() {
        let from = store::meta_get(&tx, SEQ_KEY)?
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let journal = rec.journal.clone();
        match rec.handle.block_on(journal.append(from, entries)) {
            Ok(head) => store::meta_set(&tx, SEQ_KEY, &head.to_string())?,
            // Nothing is committed: the transaction rolls back.
            Err(e) => return Ok(Err(e)),
        }
    }
    tx.commit().map_err(|e| format!("cluster database: {e}"))?;
    Ok(Ok(outs))
}

#[cfg(test)]
mod snapshot_tests {
    #[test]
    fn a_snapshot_keeps_the_newest_results_within_its_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE jobs (id INTEGER PRIMARY KEY, result BLOB, finished_ms INTEGER);
             INSERT INTO jobs VALUES (1, x'00000000000000000000', 100);
             INSERT INTO jobs VALUES (2, x'00000000000000000000', 300);
             INSERT INTO jobs VALUES (3, x'00000000000000000000', 200);
             INSERT INTO jobs VALUES (4, x'00000000000000000000', 0);
             INSERT INTO jobs VALUES (5, NULL, 400);",
        )
        .unwrap();
        drop(c);
        super::trim_results(&path, 25).unwrap();
        let c = rusqlite::Connection::open(&path).unwrap();
        let kept: Vec<i64> = c
            .prepare("SELECT id FROM jobs WHERE result IS NOT NULL ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        // The two newest ended results fit; the oldest goes; a job still
        // running keeps its (none yet, here a stand-in) result.
        assert_eq!(kept, [2, 3, 4]);
    }
}
