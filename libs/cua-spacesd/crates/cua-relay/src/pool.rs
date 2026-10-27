// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account's machine pool (account mode): who coordinates its batch
//! work, and the job ledger that outlives any one machine.
//!
//! - **Lease.** One enrolled device of the account at a time coordinates
//!   (`POST /v1/pool/lease`). Each grant to a new holder takes the next
//!   `epoch` (a counter that only grows, written to disk before it is
//!   handed out). A holder renews; a lease not renewed within its TTL, on
//!   this relay's monotonic clock, can be granted to another device.
//! - **Log.** The coordinator's state changes, appended only under the
//!   current epoch (`POST /v1/pool/log`): an append of any other epoch is
//!   refused (`fenced`), so a holder that was paused or cut off past its
//!   lease cannot write once another holds it, whatever the clocks say.
//!   An append names the head it extends (`from`); a retry of entries
//!   already stored is answered with the head (appends are idempotent).
//!   The log never holds two terminal entries for one job. A snapshot
//!   (`PUT /v1/pool/snapshot`) truncates it. Entries are hash-chained.
//! - **Mailbox.** Clients' requests to the holder (`POST /v1/pool/rpc`)
//!   and its answers, in memory: a request waits for the holder's answer
//!   or times out.
//! - **Keys.** What jobs are and produce never reaches this relay in the
//!   clear: the devices seal it end to end (`cua-spaces-cluster`'s
//!   `pool::seal`). The relay keeps each device's certified pool key
//!   (`POST /v1/pool/member`, checked against its enrolled device key) and
//!   the account's keyring (`/v1/pool/keys`: the data keys wrapped to each
//!   device, which it cannot open), and checks only what it can see: a
//!   keyring is wrapped by and to the account's devices, a keyring update
//!   names the version it replaces, and every log entry, snapshot, request
//!   and answer is sealed (`not_sealed` otherwise) under the newest key
//!   generation. Once a device that held the keys is revoked, everything
//!   is refused (`rekey`) until a device still there makes the next
//!   generation without it, so nothing written later is readable by it.
//!
//! - **Disk.** Each account's pool keeps at most
//!   [`StorageLimits::pool_account_quota_bytes`](crate::storage::StorageLimits)
//!   on the state volume (log, terminal job keys, snapshot), and the log at
//!   most [`LOG_MAX_BYTES`] between snapshots; past either an append is
//!   refused (`507`, `snapshot_needed` or `quota`) until the holder writes
//!   a snapshot. Pool writes also leave the volume's reserve free
//!   (`disk_reserve`, retry later), so devices and sign-ins keep working
//!   when pools fill it. A failed write is cut back (no torn line stays)
//!   and answered `storage`; startup drops a torn tail and leftover temp
//!   files.
//!
//! What the relay reads of the log: positions, epochs, job ids, the kind
//! of each change, which entry ends a job, the run of a submit, the
//! machine and attempt of a dispatch, and sizes. See
//! `docs/content/docs/spaces/guides/relay-security.mdx`.
//!
//! Every call needs the account token of an enrolled device of the
//! account; the device is the contender's identity. See
//! `rfcs/pool-self-coordination.md`.

// Handlers short-circuit with ready-made responses.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::devices::AuditEvent;
use crate::oidc::Identity;
use crate::server::Relay;
use crate::storage::{store, Refusal, Storage};

/// Shortest and longest lease a holder may ask for.
const TTL_MIN: Duration = Duration::from_secs(2);
const TTL_MAX: Duration = Duration::from_secs(120);
/// Default lease.
const TTL_DEFAULT: Duration = Duration::from_secs(15);
/// Entries kept between snapshots before an append is refused (the
/// coordinator snapshots every 20 000).
const LOG_MAX_ENTRIES: usize = 200_000;
/// Bytes of log kept between snapshots (on disk, and about as much in
/// memory) before an append is refused with `snapshot_needed`.
pub const LOG_MAX_BYTES: u64 = 128 << 20;
/// Largest append, request and answer bodies: one submit of the most jobs
/// a submit takes (100 000), sealed, fits.
const BODY_LIMIT: usize = 64 << 20;
/// Largest snapshot body (base64 of the sealed, compressed database).
const SNAPSHOT_BODY_LIMIT: usize = 64 << 20;
/// Largest keyring body (up to 4096 wraps).
const KEYS_BODY_LIMIT: usize = 4 << 20;
/// Bodies of the other calls (lease, release, take, member).
const SMALL_BODY_LIMIT: usize = 64 << 10;
/// Longest job key the log keeps (`<pool>-<id>` is far shorter).
const JOB_MAX: usize = 256;
/// How long a client refused for space waits before it retries.
const RETRY_AFTER_SECS: u64 = 5;
/// Requests waiting for the holder, per account.
const MAILBOX_MAX: usize = 4096;
/// How long a client waits for the holder's answer.
const RPC_WAIT: Duration = Duration::from_secs(60);
/// Terminal job keys remembered (oldest forgotten first): about 50 MiB of
/// memory and 20 MiB of disk per account at most.
const TERMINAL_MAX: usize = 500_000;

/// One entry of an account's pool log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Position (from 1).
    pub seq: u64,
    /// The epoch it was appended under.
    pub epoch: u64,
    /// What it records (the coordinator's word).
    pub kind: String,
    /// The job it ends or changes (empty: none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub job: String,
    /// It ends `job` (at most one such entry per job).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terminal: bool,
    /// The run it belongs to (empty: none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run: String,
    /// The machine an attempt went to (dispatches).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node: String,
    /// The attempt a dispatch starts.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attempt: u32,
    /// The coordinator's payload, sealed by the devices.
    #[serde(default)]
    pub data: Value,
    /// SHA-256 (hex) of the previous entry's hash and this entry.
    #[serde(default)]
    pub hash: String,
}

/// An entry as a holder appends it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NewEntry {
    /// See [`Entry::kind`].
    pub kind: String,
    /// See [`Entry::job`].
    #[serde(default)]
    pub job: String,
    /// See [`Entry::terminal`].
    #[serde(default)]
    pub terminal: bool,
    /// See [`Entry::run`].
    #[serde(default)]
    pub run: String,
    /// See [`Entry::node`].
    #[serde(default)]
    pub node: String,
    /// See [`Entry::attempt`].
    #[serde(default)]
    pub attempt: u32,
    /// See [`Entry::data`].
    #[serde(default)]
    pub data: Value,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn chain(prev: &str, e: &Entry) -> String {
    let body = json!({
        "seq": e.seq, "epoch": e.epoch, "kind": e.kind, "job": e.job,
        "terminal": e.terminal, "run": e.run, "node": e.node, "attempt": e.attempt,
        "data": e.data,
    });
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    ctx.update(prev.as_bytes());
    ctx.update(body.to_string().as_bytes());
    hex::encode(ctx.finish().as_ref())
}

/// The durable part of an account's lease.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LeaseFile {
    epoch: u64,
    #[serde(default)]
    holder: Option<Holder>,
    #[serde(default)]
    snapshot_seq: u64,
    #[serde(default)]
    snapshot_hash: String,
}

/// Who holds (or contends for) the lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    /// The enrolled device.
    pub device: String,
    /// Its display name (what the machine calls itself).
    #[serde(default)]
    pub name: String,
    /// Its relay machine id, when it is one (the coordinator reaches its
    /// own runner without the relay).
    #[serde(default)]
    pub machine: String,
    /// Higher is preferred: machines that provide Spaces over clients.
    #[serde(default)]
    pub priority: u32,
}

struct Waiting {
    id: u64,
    method: String,
    params: Value,
    caller: Value,
    reply: tokio::sync::oneshot::Sender<(u16, Value)>,
}

/// A device's certified pool key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MemberKey {
    /// X25519, base64.
    pool_key: String,
    /// The device key's signature of [`member_message`].
    sig: String,
}

/// The data keys wrapped to one device (opaque here but for who and what).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wrap {
    /// The recipient device.
    pub device: String,
    /// Its pool key.
    pub key: String,
    /// The device that wrapped them.
    pub from: String,
    /// HPKE encapsulated key.
    pub enc: String,
    /// HPKE ciphertext.
    pub ct: String,
}

/// The account's keyring: the newest generation and its wraps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyring {
    /// The newest key generation.
    pub gen: u64,
    /// One per device that holds the keys.
    pub wraps: Vec<Wrap>,
}

impl Keyring {
    fn holds<'a>(&'a self, members: &'a [Member]) -> impl Iterator<Item = &'a Wrap> + 'a {
        self.wraps.iter().filter(move |w| {
            members
                .iter()
                .any(|m| m.device == w.device && m.pool_key == w.key)
        })
    }

    /// A wrap for a device that is no member (with that key) any more.
    fn stale(&self, members: &[Member]) -> bool {
        self.holds(members).count() != self.wraps.len()
    }
}

/// What is stored of the account's keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct KeysFile {
    #[serde(default)]
    version: u64,
    #[serde(default)]
    keyring: Option<Keyring>,
    #[serde(default)]
    members: BTreeMap<String, MemberKey>,
}

/// A device that may hold the keys: enrolled (not revoked) with a
/// certified pool key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Member {
    /// `dev_…`.
    pub device: String,
    /// Its P-256 device key (base64url).
    pub public_key: String,
    /// Its pool key.
    pub pool_key: String,
    /// The device key's signature of the pool key.
    pub sig: String,
    /// Its name.
    pub name: String,
}

/// What a device signs to certify its pool key (as `cua-spaces-cluster`'s
/// `pool::seal::member_message`).
pub fn member_message(account: &str, device: &str, pool_key: &str) -> String {
    format!("cua-pool-key/v1\n{account}\n{device}\n{pool_key}")
}

/// The generation of sealed data: an object of exactly `v` (1), `g`, `n`
/// and `c`.
pub fn sealed_gen(v: &Value) -> Option<u64> {
    let o = v.as_object()?;
    if o.len() != 4
        || o.get("v")?.as_u64()? != 1
        || !o.get("n")?.is_string()
        || !o.get("c")?.is_string()
    {
        return None;
    }
    o.get("g")?.as_u64()
}

/// One account's pool.
struct Account {
    dir: Option<PathBuf>,
    storage: Arc<Storage>,
    lease: LeaseFile,
    expires: Instant,
    contenders: HashMap<String, (Holder, Instant)>,
    entries: VecDeque<Entry>,
    /// The size of each entry's line in the log file.
    sizes: VecDeque<u32>,
    /// Their sum: the log since the snapshot.
    bytes: u64,
    head: u64,
    head_hash: String,
    terminal: HashSet<Arc<str>>,
    terminal_order: VecDeque<Arc<str>>,
    snapshot: Option<Vec<u8>>,
    /// Bytes of `log.jsonl` and `terminal.txt` on disk.
    log_file_bytes: u64,
    terminal_file_bytes: u64,
    /// The files still hold what a snapshot dropped (their rewrite failed
    /// or is due): rewritten at the next append.
    rewrite_due: bool,
    mailbox: VecDeque<Waiting>,
    /// Requests the holder took and has not answered, by id.
    taken: HashMap<u64, tokio::sync::oneshot::Sender<(u16, Value)>>,
    next_rpc: u64,
    keys: KeysFile,
}

/// Cuts `path` back to `len` bytes (a torn or untrusted tail).
fn truncate(path: &Path, len: u64) -> std::io::Result<()> {
    let f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.set_len(len)?;
    f.sync_all()
}

impl Account {
    fn open(dir: Option<PathBuf>, ttl: Duration, storage: Arc<Storage>) -> std::io::Result<Self> {
        let mut a = Self {
            dir,
            storage,
            lease: LeaseFile::default(),
            // A relay that starts again treats the lease as held for a
            // full lease time: the holder renews it, nobody else gets it
            // sooner than they could have before the restart.
            expires: Instant::now() + ttl,
            contenders: HashMap::new(),
            entries: VecDeque::new(),
            sizes: VecDeque::new(),
            bytes: 0,
            head: 0,
            head_hash: String::new(),
            terminal: HashSet::new(),
            terminal_order: VecDeque::new(),
            snapshot: None,
            log_file_bytes: 0,
            terminal_file_bytes: 0,
            rewrite_due: false,
            mailbox: VecDeque::new(),
            taken: HashMap::new(),
            next_rpc: 1,
            keys: KeysFile::default(),
        };
        let Some(dir) = a.dir.clone() else {
            return Ok(a);
        };
        std::fs::create_dir_all(&dir)?;
        crate::storage::sweep_temps(&dir, 0)?;
        if let Ok(raw) = std::fs::read(dir.join("lease.json")) {
            a.lease = serde_json::from_slice(&raw)
                .map_err(|e| std::io::Error::other(format!("pool lease: {e}")))?;
        }
        a.head = a.lease.snapshot_seq;
        a.head_hash = a.lease.snapshot_hash.clone();
        let terminal_path = dir.join("terminal.txt");
        if let Ok(raw) = std::fs::read(&terminal_path) {
            // A torn last key (a crash mid-write) was never acknowledged.
            let good = raw.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            if good < raw.len() {
                truncate(&terminal_path, good as u64)?;
            }
            for k in String::from_utf8_lossy(&raw[..good])
                .lines()
                .filter(|l| !l.is_empty())
            {
                a.remember_terminal(k);
            }
            a.terminal_file_bytes = good as u64;
        }
        let log_path = dir.join("log.jsonl");
        if let Ok(file) = std::fs::File::open(&log_path) {
            let mut reader = std::io::BufReader::new(file);
            let mut line = Vec::new();
            let mut good = 0u64;
            let mut end = 0u64;
            loop {
                line.clear();
                let n = reader.read_until(b'\n', &mut line)?;
                if n == 0 {
                    break;
                }
                end += n as u64;
                let body = line.strip_suffix(b"\n");
                // A torn last line (a crash mid-write) was never
                // acknowledged, and nothing after a gap can be trusted:
                // drop it and everything after it, from the file too, so
                // the next append does not follow a broken line.
                let Some(e) = body.and_then(|b| serde_json::from_slice::<Entry>(b).ok()) else {
                    break;
                };
                if e.seq <= a.head {
                    // Before the snapshot (a rewrite that did not happen).
                    a.rewrite_due = true;
                    good = end;
                    continue;
                }
                if e.seq != a.head + 1 {
                    break;
                }
                a.head = e.seq;
                a.head_hash = e.hash.clone();
                a.bytes += n as u64;
                a.sizes.push_back(n as u32);
                a.entries.push_back(e);
                good = end;
            }
            let len = std::fs::metadata(&log_path)?.len();
            if good < len {
                tracing::warn!(
                    path = %log_path.display(),
                    dropped_bytes = len - good,
                    "pool: dropped a torn or broken log tail"
                );
                truncate(&log_path, good)?;
            }
            a.log_file_bytes = good;
        }
        if let Ok(raw) = std::fs::read(dir.join("snapshot.bin")) {
            a.snapshot = Some(raw);
        }
        if let Ok(raw) = std::fs::read(dir.join("keys.json")) {
            a.keys = serde_json::from_slice(&raw)
                .map_err(|e| std::io::Error::other(format!("pool keys: {e}")))?;
        }
        Ok(a)
    }

    /// Bytes this pool keeps on disk (log, terminal keys, snapshot).
    fn disk_bytes(&self) -> u64 {
        self.log_file_bytes
            + self.terminal_file_bytes
            + self.snapshot.as_ref().map_or(0, |s| s.len() as u64)
    }

    /// Reports the outcome of a write to the pool store.
    fn record<T>(&self, r: std::io::Result<T>) -> std::io::Result<T> {
        if self.dir.is_some() {
            self.storage.record(store::POOL, &r);
        }
        r
    }

    fn save_keys(&self) -> std::io::Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let r = serde_json::to_vec(&self.keys)
            .map_err(std::io::Error::other)
            .and_then(|raw| crate::assertion::write_private(&dir.join("keys.json"), &raw));
        self.record(r)
    }

    /// The members among `devices` (the account's enrolled devices: id ->
    /// (P-256 key, name)).
    fn members(&self, devices: &HashMap<String, (String, String)>) -> Vec<Member> {
        self.keys
            .members
            .iter()
            .filter_map(|(id, k)| {
                let (public_key, name) = devices.get(id)?;
                Some(Member {
                    device: id.clone(),
                    public_key: public_key.clone(),
                    pool_key: k.pool_key.clone(),
                    sig: k.sig.clone(),
                    name: name.clone(),
                })
            })
            .collect()
    }

    /// Whether data sealed under generation `g` (`None`: not sealed) may
    /// be stored now.
    fn key_gate(&self, g: Option<u64>, members: &[Member]) -> Result<(), Response> {
        let Some(g) = g else {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "not_sealed",
                "pool data must be sealed by the account's devices",
            ));
        };
        let rekey = |m: String| {
            (
                StatusCode::CONFLICT,
                Json(json!({"code": "rekey", "error": m,
                            "gen": self.keys.keyring.as_ref().map_or(0, |k| k.gen)})),
            )
                .into_response()
        };
        let Some(ring) = &self.keys.keyring else {
            return Err(rekey("the account has no pool keys yet".into()));
        };
        if ring.stale(members) {
            return Err(rekey(
                "a device that held the pool's keys left: a device still here makes new ones"
                    .into(),
            ));
        }
        if g != ring.gen {
            return Err(rekey(format!(
                "sealed under key generation {g}; the account's keys are at {}",
                ring.gen
            )));
        }
        Ok(())
    }

    /// Drops the log and snapshot (nobody can read them any more).
    fn reset_log(&mut self) -> std::io::Result<()> {
        self.entries.clear();
        self.sizes.clear();
        self.bytes = 0;
        self.head = 0;
        self.head_hash.clear();
        self.terminal.clear();
        self.terminal_order.clear();
        self.snapshot = None;
        self.lease.snapshot_seq = 0;
        self.lease.snapshot_hash.clear();
        self.save_lease()?;
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_file(dir.join("snapshot.bin"));
        }
        self.rewrite_files()
    }

    fn save_lease(&self) -> std::io::Result<()> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let r = serde_json::to_vec(&self.lease)
            .map_err(std::io::Error::other)
            .and_then(|raw| crate::assertion::write_private(&dir.join("lease.json"), &raw));
        self.record(r)
    }

    fn holder_live(&self, now: Instant) -> bool {
        self.lease.holder.is_some() && now < self.expires
    }

    fn remember_terminal(&mut self, key: &str) {
        let key: Arc<str> = Arc::from(key);
        if !self.terminal.insert(key.clone()) {
            return;
        }
        self.terminal_order.push_back(key);
        while self.terminal_order.len() > TERMINAL_MAX {
            if let Some(old) = self.terminal_order.pop_front() {
                self.terminal.remove(&old);
            }
        }
    }

    /// Appends `lines` (the entries' log lines) and the terminal keys of
    /// `new`, fsynced. On a failure both files are cut back to where they
    /// were, so no torn line stays and a retry starts clean.
    fn append_files(&mut self, lines: &[u8], new: &[Entry]) -> std::io::Result<()> {
        let Some(dir) = self.dir.clone() else {
            return Ok(());
        };
        let mut terminal = Vec::new();
        for e in new.iter().filter(|e| e.terminal && !e.job.is_empty()) {
            terminal.extend_from_slice(e.job.as_bytes());
            terminal.push(b'\n');
        }
        let (log_before, terminal_before) = (self.log_file_bytes, self.terminal_file_bytes);
        let append = |name: &str, bytes: &[u8], before: u64| -> std::io::Result<()> {
            let path = dir.join(name);
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            let written = f.write_all(bytes).and_then(|()| f.sync_data());
            if written.is_err() {
                let _ = f.set_len(before);
            }
            written
        };
        let mut r = append("log.jsonl", lines, log_before);
        if r.is_ok() && !terminal.is_empty() {
            r = append("terminal.txt", &terminal, terminal_before);
            if r.is_err() {
                let _ = truncate(&dir.join("log.jsonl"), log_before);
            }
        }
        if r.is_ok() {
            self.log_file_bytes += lines.len() as u64;
            self.terminal_file_bytes += terminal.len() as u64;
        }
        self.record(r)
    }

    /// Rewrites the log and the terminal keys with only what is kept
    /// (streamed, atomic). On a failure the old files stay (still
    /// readable: what a snapshot dropped is skipped on load) and the
    /// rewrite is tried again later.
    fn rewrite_files(&mut self) -> std::io::Result<()> {
        let Some(dir) = self.dir.clone() else {
            return Ok(());
        };
        let mut log_bytes = 0u64;
        let mut terminal_bytes = 0u64;
        let r = crate::assertion::write_private_with(&dir.join("log.jsonl"), |out| {
            let mut counted = Counted(out, &mut log_bytes);
            for e in &self.entries {
                serde_json::to_writer(&mut counted, e).map_err(std::io::Error::other)?;
                counted.write_all(b"\n")?;
            }
            Ok(())
        })
        .and_then(|()| {
            crate::assertion::write_private_with(&dir.join("terminal.txt"), |out| {
                let mut counted = Counted(out, &mut terminal_bytes);
                for k in &self.terminal_order {
                    counted.write_all(k.as_bytes())?;
                    counted.write_all(b"\n")?;
                }
                Ok(())
            })
        });
        match &r {
            Ok(()) => {
                self.log_file_bytes = log_bytes;
                self.terminal_file_bytes = terminal_bytes;
                self.rewrite_due = false;
            }
            Err(_) => self.rewrite_due = true,
        }
        self.record(r)
    }
}

/// A writer that counts what goes through it.
struct Counted<'a>(&'a mut dyn std::io::Write, &'a mut u64);

impl std::io::Write for Counted<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.0.write(buf)?;
        *self.1 += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// Every account's pool on this relay.
pub struct PoolState {
    dir: Option<PathBuf>,
    storage: Arc<Storage>,
    accounts: Mutex<HashMap<String, Arc<Mutex<Account>>>>,
    /// Wakes holders waiting for mailbox requests.
    notify: Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
}

impl PoolState {
    /// Pools persisted under `dir` (in memory when `None`).
    pub fn open(dir: Option<PathBuf>) -> std::io::Result<Self> {
        Self::open_with(dir, Arc::default())
    }

    /// [`PoolState::open`], reporting writes to `storage` and held to its
    /// limits.
    pub fn open_with(dir: Option<PathBuf>, storage: Arc<Storage>) -> std::io::Result<Self> {
        if let Some(d) = &dir {
            std::fs::create_dir_all(d)?;
            crate::storage::sweep_temps(d, 1)?;
        }
        Ok(Self {
            dir,
            storage,
            accounts: Mutex::default(),
            notify: Mutex::default(),
        })
    }

    fn account(&self, id: &str) -> Result<Arc<Mutex<Account>>, String> {
        let mut all = self.accounts.lock().expect("pool accounts");
        if let Some(a) = all.get(id) {
            return Ok(a.clone());
        }
        let dir = self.dir.as_ref().map(|d| {
            let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
            ctx.update(id.as_bytes());
            d.join(&hex::encode(ctx.finish().as_ref())[..32])
        });
        let a = Arc::new(Mutex::new(
            Account::open(dir, TTL_DEFAULT, self.storage.clone()).map_err(|e| e.to_string())?,
        ));
        all.insert(id.to_string(), a.clone());
        Ok(a)
    }

    /// Account deletion: drops the account's in-memory pool and removes its
    /// on-disk directory. True when either existed. Idempotent.
    pub fn erase_account(&self, id: &str) -> std::io::Result<bool> {
        let mut all = self.accounts.lock().expect("pool accounts");
        let mut existed = all.remove(id).is_some();
        self.notify.lock().expect("pool notify").remove(id);
        if let Some(d) = &self.dir {
            let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
            ctx.update(id.as_bytes());
            let dir = d.join(&hex::encode(ctx.finish().as_ref())[..32]);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => existed = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(existed)
    }

    fn notifier(&self, id: &str) -> Arc<tokio::sync::Notify> {
        self.notify
            .lock()
            .expect("pool notify")
            .entry(id.to_string())
            .or_default()
            .clone()
    }
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into(), "code": code}))).into_response()
}

/// A pool write refused for space: `507` with its code, retryable.
fn refused(r: &Refusal) -> Response {
    (
        StatusCode::INSUFFICIENT_STORAGE,
        [(
            axum::http::header::RETRY_AFTER,
            RETRY_AFTER_SECS.to_string(),
        )],
        Json(
            json!({"code": r.code(), "error": r.message(), "retryable": true,
                    "retry_after_ms": RETRY_AFTER_SECS * 1000}),
        ),
    )
        .into_response()
}

pub(crate) fn routes() -> Router<Relay> {
    let limit = DefaultBodyLimit::max;
    Router::new()
        .route("/v1/pool", get(status))
        .route("/v1/pool/lease", post(lease).layer(limit(SMALL_BODY_LIMIT)))
        .route(
            "/v1/pool/release",
            post(release).layer(limit(SMALL_BODY_LIMIT)),
        )
        .route(
            "/v1/pool/log",
            get(read_log).post(append).layer(limit(BODY_LIMIT)),
        )
        .route(
            "/v1/pool/snapshot",
            get(read_snapshot)
                .put(put_snapshot)
                .layer(limit(SNAPSHOT_BODY_LIMIT)),
        )
        .route("/v1/pool/rpc", post(rpc).layer(limit(BODY_LIMIT)))
        .route(
            "/v1/pool/rpc/take",
            post(take).layer(limit(SMALL_BODY_LIMIT)),
        )
        .route("/v1/pool/rpc/reply", post(reply).layer(limit(BODY_LIMIT)))
        .route(
            "/v1/pool/member",
            post(member).layer(limit(SMALL_BODY_LIMIT)),
        )
        .route(
            "/v1/pool/keys",
            get(read_keys).put(put_keys).layer(limit(KEYS_BODY_LIMIT)),
        )
}

/// The caller: an enrolled device of an account.
async fn caller(relay: &Relay, headers: &HeaderMap) -> Result<(Identity, String), Response> {
    let who = crate::device_api::account(relay, headers).await?;
    let device = crate::device_api::enrolled(relay, headers, &who)?;
    Ok((who, device))
}

/// The account's enrolled devices (not revoked): id -> (P-256 key, name).
fn enrolled_devices(relay: &Relay, account: &str) -> HashMap<String, (String, String)> {
    let now = crate::assertion::now_secs();
    relay
        .devices
        .list(account)
        .into_iter()
        .filter(|d| {
            matches!(
                d.state(now),
                crate::devices::DeviceState::Enrolled | crate::devices::DeviceState::Expired
            )
        })
        .map(|d| (d.id, (d.public_key, d.name)))
        .collect()
}

fn pool_of(relay: &Relay, who: &Identity) -> Result<Arc<Mutex<Account>>, Response> {
    relay
        .pool
        .account(&who.account)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e))
}

fn ttl_of(ms: Option<u64>) -> Duration {
    ms.map(Duration::from_millis)
        .unwrap_or(TTL_DEFAULT)
        .clamp(TTL_MIN, TTL_MAX)
}

fn status_view(a: &Account, now: Instant) -> Value {
    let contenders: Vec<Value> = a
        .contenders
        .values()
        .filter(|(_, seen)| now.duration_since(*seen) < TTL_MAX)
        .map(|(h, seen)| {
            json!({"device": h.device, "name": h.name, "machine": h.machine,
                   "priority": h.priority, "seen_ms_ago": now.duration_since(*seen).as_millis() as u64})
        })
        .collect();
    json!({
        "epoch": a.lease.epoch,
        "holder": if a.holder_live(now) { json!(a.lease.holder) } else { Value::Null },
        "expires_in_ms": a.expires.saturating_duration_since(now).as_millis() as u64,
        "head": a.head,
        "snapshot_seq": a.lease.snapshot_seq,
        "contenders": contenders,
        "waiting_requests": a.mailbox.len(),
    })
}

async fn status(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let (who, _) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let a = pool.lock().expect("pool");
    let mut v = status_view(&a, Instant::now());
    // The account as machines see it (`acct` of the relay's assertions).
    v["account"] = json!(who.account);
    Json(v).into_response()
}

#[derive(Deserialize)]
struct LeaseBody {
    #[serde(default)]
    name: String,
    #[serde(default)]
    machine: String,
    #[serde(default)]
    priority: u32,
    /// The epoch the caller holds (renew), or 0 (acquire).
    #[serde(default)]
    epoch: u64,
    #[serde(default)]
    ttl_ms: Option<u64>,
}

/// Acquires or renews the account's lease.
async fn lease(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<LeaseBody>,
) -> Response {
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let ttl = ttl_of(body.ttl_ms);
    let me = Holder {
        device: device.clone(),
        name: body.name.chars().take(128).collect(),
        machine: body.machine.chars().take(64).collect(),
        priority: body.priority.min(100),
    };
    let relay2 = relay.clone();
    let account = who.account.clone();
    tokio::task::spawn_blocking(move || {
        let mut a = pool.lock().expect("pool");
        let now = Instant::now();
        a.contenders.insert(device.clone(), (me.clone(), now));
        a.contenders
            .retain(|_, (_, seen)| now.duration_since(*seen) < TTL_MAX);
        let mine = a.lease.holder.as_ref().is_some_and(|h| h.device == device);
        if mine && body.epoch == a.lease.epoch && body.epoch != 0 {
            // Renew. Tell a holder to yield to a waiting contender it
            // ranks below (a laptop to a machine that provides Spaces).
            a.expires = now + ttl;
            if let Some(h) = a.lease.holder.as_mut() {
                h.name = me.name.clone();
                h.priority = me.priority;
            }
            let yield_to = a
                .contenders
                .values()
                .filter(|(h, seen)| {
                    h.device != device
                        && h.priority > me.priority
                        && now.duration_since(*seen) < ttl
                })
                .map(|(h, _)| h.name.clone())
                .next();
            return Json(json!({
                "granted": true, "epoch": a.lease.epoch, "ttl_ms": ttl.as_millis() as u64,
                "head": a.head, "snapshot_seq": a.lease.snapshot_seq,
                "yield_to": yield_to,
            }))
            .into_response();
        }
        if a.holder_live(now) && !(mine && body.epoch == 0) {
            let h = a.lease.holder.clone();
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "granted": false, "code": if mine { "lease_lost" } else { "held" },
                    "epoch": a.lease.epoch, "holder": h,
                    "expires_in_ms": a.expires.saturating_duration_since(now).as_millis() as u64,
                })),
            )
                .into_response();
        }
        // Vacant or expired, but a contender this one ranks below is waiting
        // (a laptop while a machine that provides Spaces asks): it goes
        // first.
        if let Some((better, _)) = a.contenders.values().find(|(h, seen)| {
            h.device != device && h.priority > me.priority && now.duration_since(*seen) < ttl
        }) {
            let better = better.name.clone();
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "granted": false, "code": "held",
                    "epoch": a.lease.epoch, "waiting_for": better,
                    "expires_in_ms": 0,
                })),
            )
                .into_response();
        }
        // Vacant, expired, or this device asking afresh: the next epoch.
        let previous = a.lease.holder.clone();
        a.lease.epoch += 1;
        a.lease.holder = Some(me.clone());
        if let Err(e) = a.save_lease() {
            a.lease.epoch -= 1;
            a.lease.holder = previous;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e.to_string());
        }
        a.expires = now + ttl;
        // Requests the previous holder took and never answered go back to
        // the mailbox's callers as "retry".
        for (_, tx) in a.taken.drain() {
            let _ = tx.send((
                503,
                json!({"error": "the coordinator changed; retry", "code": "retry"}),
            ));
        }
        let epoch = a.lease.epoch;
        let head = a.head;
        let snapshot_seq = a.lease.snapshot_seq;
        drop(a);
        relay2.devices.audit(
            &account,
            AuditEvent::new("pool_coordinator")
                .device(Some(&device))
                .subject(me.name.clone())
                .detail(match previous {
                    Some(p) if p.device != device => format!("epoch {epoch}, after {}", p.name),
                    _ => format!("epoch {epoch}"),
                }),
        );
        Json(json!({
            "granted": true, "epoch": epoch, "ttl_ms": ttl.as_millis() as u64,
            "head": head, "snapshot_seq": snapshot_seq, "yield_to": Value::Null,
        }))
        .into_response()
    })
    .await
    .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))
}

#[derive(Deserialize)]
struct ReleaseBody {
    epoch: u64,
}

async fn release(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<ReleaseBody>,
) -> Response {
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let mut a = pool.lock().expect("pool");
    let mine = a.lease.holder.as_ref().is_some_and(|h| h.device == device);
    if mine && a.lease.epoch == body.epoch {
        a.expires = Instant::now();
        a.contenders.remove(&device);
        for (_, tx) in a.taken.drain() {
            let _ = tx.send((
                503,
                json!({"error": "the coordinator changed; retry", "code": "retry"}),
            ));
        }
    }
    Json(json!({"released": mine})).into_response()
}

#[derive(Deserialize)]
struct AppendBody {
    epoch: u64,
    /// The head these entries extend.
    from: u64,
    entries: Vec<NewEntry>,
}

/// Appends entries under the current epoch.
async fn append(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<AppendBody>,
) -> Response {
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let devices = enrolled_devices(&relay, &who.account);
    tokio::task::spawn_blocking(move || append_now(&pool, &device, &devices, body))
        .await
        .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))
}

fn append_now(
    pool: &Mutex<Account>,
    device: &str,
    devices: &HashMap<String, (String, String)>,
    body: AppendBody,
) -> Response {
    let mut a = pool.lock().expect("pool");
    let holder_ok = a.lease.holder.as_ref().is_some_and(|h| h.device == device);
    if body.epoch != a.lease.epoch || !holder_ok {
        return (
            StatusCode::CONFLICT,
            Json(json!({"code": "fenced", "error": "this epoch is over: another device coordinates now",
                        "epoch": a.lease.epoch})),
        )
            .into_response();
    }
    let n = body.entries.len() as u64;
    if body.from < a.head {
        // A retry of entries already stored (their reply was lost)?
        let first = a.lease.snapshot_seq + 1;
        let same = body.from + n <= a.head
            && body.from + 1 >= first
            && body.entries.iter().enumerate().all(|(i, ne)| {
                let seq = body.from + 1 + i as u64;
                a.entries.get((seq - first) as usize).is_some_and(|e| {
                    e.epoch == body.epoch
                        && e.kind == ne.kind
                        && e.job == ne.job
                        && e.terminal == ne.terminal
                        && e.run == ne.run
                        && e.node == ne.node
                        && e.attempt == ne.attempt
                        && e.data == ne.data
                })
            });
        if same {
            return Json(json!({"head": body.from + n, "retried": true})).into_response();
        }
        return (
            StatusCode::CONFLICT,
            Json(json!({"code": "conflict", "error": "the log moved on", "head": a.head})),
        )
            .into_response();
    }
    if body.from > a.head {
        return (
            StatusCode::CONFLICT,
            Json(json!({"code": "conflict", "error": "a gap", "head": a.head})),
        )
            .into_response();
    }
    if a.entries.len() + body.entries.len() > LOG_MAX_ENTRIES || a.bytes > LOG_MAX_BYTES {
        return (
            StatusCode::INSUFFICIENT_STORAGE,
            Json(json!({"code": "snapshot_needed", "retryable": true,
                        "error": "the log is full: write a snapshot, then retry"})),
        )
            .into_response();
    }
    if let Some(e) = body.entries.iter().find(|e| e.job.len() > JOB_MAX) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("job key of {} bytes (at most {JOB_MAX})", e.job.len()),
        );
    }
    let members = a.members(devices);
    for e in &body.entries {
        if let Err(r) = a.key_gate(sealed_gen(&e.data), &members) {
            return r;
        }
    }
    let mut ended: HashSet<&str> = HashSet::new();
    for e in &body.entries {
        if e.terminal
            && !e.job.is_empty()
            && (a.terminal.contains(e.job.as_str()) || !ended.insert(&e.job))
        {
            tracing::error!(job = %e.job, "pool: refused a second terminal entry for a job");
            return (
                StatusCode::CONFLICT,
                Json(json!({"code": "duplicate_terminal", "job": e.job,
                            "error": format!("job {} already ended", e.job)})),
            )
                .into_response();
        }
    }
    let mut prev = a.head_hash.clone();
    let mut new = Vec::with_capacity(body.entries.len());
    for (i, ne) in body.entries.into_iter().enumerate() {
        let mut e = Entry {
            seq: body.from + 1 + i as u64,
            epoch: body.epoch,
            kind: ne.kind,
            job: ne.job,
            terminal: ne.terminal,
            run: ne.run,
            node: ne.node.chars().take(128).collect(),
            attempt: ne.attempt,
            data: ne.data,
            hash: String::new(),
        };
        e.hash = chain(&prev, &e);
        prev = e.hash.clone();
        new.push(e);
    }
    let mut lines = Vec::new();
    let mut sizes = Vec::with_capacity(new.len());
    for e in &new {
        let before = lines.len();
        if let Err(err) = serde_json::to_writer(&mut lines, e) {
            return error(StatusCode::BAD_REQUEST, "invalid", err.to_string());
        }
        lines.push(b'\n');
        sizes.push((lines.len() - before) as u32);
    }
    if a.rewrite_due {
        // What the last snapshot dropped is still on disk: drop it first.
        let _ = a.rewrite_files();
    }
    if a.dir.is_some() {
        let incoming = lines.len() as u64
            + new
                .iter()
                .filter(|e| e.terminal)
                .map(|e| e.job.len() as u64 + 1)
                .sum::<u64>();
        if let Err(r) = a.storage.admit_pool(a.disk_bytes(), incoming, false) {
            return refused(&r);
        }
    }
    if let Err(err) = a.append_files(&lines, &new) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            format!("the append was not stored ({err}); retry"),
        );
    }
    for (e, size) in new.into_iter().zip(sizes) {
        if e.terminal && !e.job.is_empty() {
            a.remember_terminal(&e.job);
        }
        a.bytes += u64::from(size);
        a.sizes.push_back(size);
        a.head = e.seq;
        a.head_hash = e.hash.clone();
        a.entries.push_back(e);
    }
    Json(json!({"head": a.head, "hash": a.head_hash})).into_response()
}

#[derive(Deserialize)]
struct ReadQuery {
    #[serde(default)]
    after: u64,
    #[serde(default)]
    limit: Option<usize>,
}

async fn read_log(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Query(q): Query<ReadQuery>,
) -> Response {
    let (who, _) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let a = pool.lock().expect("pool");
    if q.after < a.lease.snapshot_seq {
        return (
            StatusCode::GONE,
            Json(
                json!({"code": "snapshot", "error": "read the snapshot first",
                        "snapshot_seq": a.lease.snapshot_seq}),
            ),
        )
            .into_response();
    }
    let limit = q.limit.unwrap_or(5000).clamp(1, 20_000);
    let first = a.lease.snapshot_seq + 1;
    let skip = (q.after + 1).saturating_sub(first) as usize;
    let entries: Vec<&Entry> = a.entries.iter().skip(skip).take(limit).collect();
    Json(
        json!({"entries": entries, "head": a.head, "epoch": a.lease.epoch,
                "snapshot_seq": a.lease.snapshot_seq}),
    )
    .into_response()
}

#[derive(Deserialize, Serialize)]
struct SnapshotBody {
    epoch: u64,
    seq: u64,
    /// Base64 of the coordinator's snapshot.
    data: String,
}

async fn put_snapshot(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<SnapshotBody>,
) -> Response {
    use base64::Engine as _;
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(body.data.as_bytes()) else {
        return error(StatusCode::BAD_REQUEST, "invalid", "data is not base64");
    };
    let devices = enrolled_devices(&relay, &who.account);
    tokio::task::spawn_blocking(move || {
        let mut a = pool.lock().expect("pool");
        let holder_ok = a.lease.holder.as_ref().is_some_and(|h| h.device == device);
        if body.epoch != a.lease.epoch || !holder_ok {
            return (
                StatusCode::CONFLICT,
                Json(json!({"code": "fenced", "epoch": a.lease.epoch})),
            )
                .into_response();
        }
        let members = a.members(&devices);
        let g = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| sealed_gen(&v));
        if let Err(r) = a.key_gate(g, &members) {
            return r;
        }
        if body.seq > a.head || body.seq < a.lease.snapshot_seq {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid",
                "snapshot seq outside the log",
            );
        }
        let first = a.lease.snapshot_seq + 1;
        let cut = (body.seq + 1).saturating_sub(first) as usize;
        let hash = if body.seq == a.lease.snapshot_seq {
            a.lease.snapshot_hash.clone()
        } else {
            a.entries[cut - 1].hash.clone()
        };
        if let Some(dir) = a.dir.clone() {
            if let Err(r) = a
                .storage
                .admit_pool(a.disk_bytes(), bytes.len() as u64, true)
            {
                return refused(&r);
            }
            let written = a.record(crate::assertion::write_private(
                &dir.join("snapshot.bin"),
                &bytes,
            ));
            if let Err(e) = written {
                return error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e.to_string());
            }
        }
        let old = a.lease.clone();
        a.lease.snapshot_seq = body.seq;
        a.lease.snapshot_hash = hash;
        if let Err(e) = a.save_lease() {
            a.lease = old;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e.to_string());
        }
        a.snapshot = Some(bytes);
        a.entries.drain(..cut);
        let dropped: u64 = a.sizes.drain(..cut).map(u64::from).sum();
        a.bytes -= dropped;
        // A failed rewrite is reported and tried again at the next append;
        // the snapshot itself is stored.
        let _ = a.rewrite_files();
        Json(json!({"snapshot_seq": a.lease.snapshot_seq})).into_response()
    })
    .await
    .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))
}

async fn read_snapshot(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    use base64::Engine as _;
    let (who, _) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let a = pool.lock().expect("pool");
    Json(json!({
        "seq": a.lease.snapshot_seq,
        "hash": a.lease.snapshot_hash,
        "data": a.snapshot.as_ref().map(|b| base64::engine::general_purpose::STANDARD.encode(b)),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct RpcBody {
    method: String,
    /// `{"sealed": <sealed data>}`.
    #[serde(default)]
    params: Value,
    /// What this relay may read: a submit's run, job count and `on`.
    #[serde(default)]
    meta: Value,
}

/// A client's request to the holder: waits for its answer.
async fn rpc(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<RpcBody>,
) -> Response {
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let devices = enrolled_devices(&relay, &who.account);
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut a = pool.lock().expect("pool");
        let members = a.members(&devices);
        if let Err(r) = a.key_gate(sealed_gen(&body.params["sealed"]), &members) {
            return r;
        }
        if !a.holder_live(Instant::now()) {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "no_coordinator",
                "none of your machines coordinates right now",
            );
        }
        if a.mailbox.len() >= MAILBOX_MAX {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "busy",
                "too many requests waiting",
            );
        }
        let id = a.next_rpc;
        a.next_rpc += 1;
        let method: String = body.method.chars().take(64).collect();
        if method == "submit" {
            let run = body
                .meta
                .get("run")
                .and_then(Value::as_str)
                .unwrap_or_default();
            relay.devices.audit(
                &who.account,
                AuditEvent::new("pool_run")
                    .device(Some(&device))
                    .subject(run.chars().take(80).collect::<String>())
                    .detail(format!(
                        "{} job(s), on {}",
                        body.meta.get("count").and_then(Value::as_u64).unwrap_or(0),
                        body.meta
                            .get("on")
                            .and_then(Value::as_str)
                            .filter(|o| !o.is_empty())
                            .unwrap_or("auto")
                    )),
            );
        }
        a.mailbox.push_back(Waiting {
            id,
            method,
            params: body.params,
            caller: json!({"account": who.account, "user": who.user, "email": who.email,
                           "device": device}),
            reply: tx,
        });
    }
    relay.pool.notifier(&who.account).notify_waiters();
    match tokio::time::timeout(RPC_WAIT, rx).await {
        Ok(Ok((code, v))) => (
            StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(v),
        )
            .into_response(),
        Ok(Err(_)) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "retry",
            "the coordinator changed; retry",
        ),
        Err(_) => error(
            StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            "the coordinating machine did not answer in time",
        ),
    }
}

#[derive(Deserialize)]
struct TakeBody {
    epoch: u64,
    #[serde(default)]
    wait_ms: u64,
}

/// The holder takes waiting requests (a long poll).
async fn take(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<TakeBody>,
) -> Response {
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let notify = relay.pool.notifier(&who.account);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(body.wait_ms.min(25_000));
    loop {
        let notified = notify.notified();
        {
            let mut a = pool.lock().expect("pool");
            let holder_ok = a.lease.holder.as_ref().is_some_and(|h| h.device == device);
            if body.epoch != a.lease.epoch || !holder_ok {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({"code": "fenced", "epoch": a.lease.epoch})),
                )
                    .into_response();
            }
            if !a.mailbox.is_empty() {
                let mut out = Vec::new();
                while out.len() < 64 {
                    let Some(w) = a.mailbox.pop_front() else {
                        break;
                    };
                    if w.reply.is_closed() {
                        continue;
                    }
                    out.push(json!({"id": w.id, "method": w.method, "params": w.params,
                                    "caller": w.caller}));
                    a.taken.insert(w.id, w.reply);
                }
                if !out.is_empty() {
                    return Json(json!({"requests": out})).into_response();
                }
            }
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return Json(json!({"requests": []})).into_response();
        }
    }
}

#[derive(Deserialize)]
struct ReplyBody {
    epoch: u64,
    replies: Vec<OneReply>,
}

#[derive(Deserialize)]
struct OneReply {
    id: u64,
    #[serde(default = "ok")]
    status: u16,
    #[serde(default)]
    body: Value,
}

fn ok() -> u16 {
    200
}

async fn reply(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<ReplyBody>,
) -> Response {
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let devices = enrolled_devices(&relay, &who.account);
    let mut a = pool.lock().expect("pool");
    let holder_ok = a.lease.holder.as_ref().is_some_and(|h| h.device == device);
    if body.epoch != a.lease.epoch || !holder_ok {
        return (
            StatusCode::CONFLICT,
            Json(json!({"code": "fenced", "epoch": a.lease.epoch})),
        )
            .into_response();
    }
    let members = a.members(&devices);
    for r in &body.replies {
        if let Err(resp) = a.key_gate(sealed_gen(&r.body["sealed"]), &members) {
            return resp;
        }
    }
    let mut delivered = 0;
    for r in body.replies {
        if let Some(tx) = a.taken.remove(&r.id) {
            delivered += usize::from(tx.send((r.status, r.body)).is_ok());
        }
    }
    Json(json!({"delivered": delivered})).into_response()
}

#[derive(Deserialize)]
struct MemberBody {
    pool_key: String,
    sig: String,
}

/// Registers the calling device's pool key, signed by its device key.
async fn member(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<MemberBody>,
) -> Response {
    use base64::Engine as _;
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let Some(record) = relay.devices.get(&device) else {
        return error(
            StatusCode::FORBIDDEN,
            "device_not_enrolled",
            "unknown device",
        );
    };
    let key_ok = base64::engine::general_purpose::STANDARD
        .decode(body.pool_key.trim())
        .is_ok_and(|k| k.len() == 32);
    let sig_ok = crate::devices::decode_public_key(&record.public_key).is_ok_and(|pk| {
        crate::devices::verify(
            &pk,
            &member_message(&who.account, &device, &body.pool_key),
            &body.sig,
        )
    });
    if !key_ok || !sig_ok {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "the pool key must be 32 bytes, signed by this device's key",
        );
    }
    let mut a = pool.lock().expect("pool");
    let new = MemberKey {
        pool_key: body.pool_key,
        sig: body.sig,
    };
    if a.keys.members.get(&device) != Some(&new) {
        let old = a.keys.members.insert(device.clone(), new);
        if let Err(e) = a.save_keys() {
            match old {
                Some(o) => a.keys.members.insert(device.clone(), o),
                None => a.keys.members.remove(&device),
            };
            return error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e.to_string());
        }
        drop(a);
        relay.devices.audit(
            &who.account,
            AuditEvent::new("pool_key")
                .device(Some(&device))
                .subject(record.name),
        );
    }
    Json(json!({"registered": true})).into_response()
}

/// The account's keyring, its version and members.
async fn read_keys(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let (who, _) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let devices = enrolled_devices(&relay, &who.account);
    let a = pool.lock().expect("pool");
    let members = a.members(&devices);
    let stale = a.keys.keyring.as_ref().is_some_and(|k| k.stale(&members));
    Json(json!({
        "keyring": a.keys.keyring, "version": a.keys.version,
        "members": members, "stale": stale,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct PutKeysBody {
    /// The version this replaces (0: none yet).
    version: u64,
    keyring: Keyring,
    /// A new chain: only when no member holds the keys (the log, sealed
    /// under keys nobody has, is dropped).
    #[serde(default)]
    reset: bool,
}

/// Whether `new` may replace `old`, written by `writer`.
fn check_keyring(
    old: Option<&Keyring>,
    new: &Keyring,
    reset: bool,
    writer: &str,
    members: &[Member],
) -> Result<(), String> {
    let member = |d: &str, k: &str| members.iter().any(|m| m.device == d && m.pool_key == k);
    if !members.iter().any(|m| m.device == writer) {
        return Err("this device has registered no pool key".into());
    }
    if new.gen == 0 || new.wraps.is_empty() || new.wraps.len() > 4096 {
        return Err("a keyring needs a generation and at least one wrap".into());
    }
    if let Some(w) = new.wraps.iter().find(|w| !member(&w.device, &w.key)) {
        return Err(format!(
            "wrapped to {}, which is not one of the account's devices with that pool key",
            w.device
        ));
    }
    let Some(old) = old else {
        return if reset {
            Err("nothing to reset".into())
        } else if new.wraps.iter().all(|w| w.from == writer) {
            Ok(())
        } else {
            Err("a new keyring is wrapped by the device that sends it".into())
        };
    };
    let added: Vec<&Wrap> = new
        .wraps
        .iter()
        .filter(|w| !old.wraps.contains(w))
        .collect();
    if added.iter().any(|w| w.from != writer) {
        return Err("new wraps are made by the device that sends them".into());
    }
    if reset {
        if old.holds(members).next().is_some() {
            return Err("a device still holds the keys: it shares them instead".into());
        }
        return if new.gen > old.gen {
            Ok(())
        } else {
            Err("a new chain starts after the old generation".into())
        };
    }
    if new.gen == old.gen + 1 {
        return Ok(());
    }
    if new.gen != old.gen {
        return Err(format!("generation {} after {}", new.gen, old.gen));
    }
    if old.stale(members) {
        return Err("a device that held the keys left: make the next generation".into());
    }
    if old.wraps.iter().any(|w| !new.wraps.contains(w)) {
        return Err("within a generation, wraps are only added".into());
    }
    Ok(())
}

/// Replaces the keyring if it is still the version the device saw.
async fn put_keys(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<PutKeysBody>,
) -> Response {
    let (who, device) = match caller(&relay, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pool = match pool_of(&relay, &who) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let devices = enrolled_devices(&relay, &who.account);
    let mut a = pool.lock().expect("pool");
    if body.version != a.keys.version {
        return (
            StatusCode::CONFLICT,
            Json(
                json!({"code": "keyring_changed", "error": "the keyring changed meanwhile",
                        "version": a.keys.version}),
            ),
        )
            .into_response();
    }
    let members = a.members(&devices);
    if let Err(e) = check_keyring(
        a.keys.keyring.as_ref(),
        &body.keyring,
        body.reset,
        &device,
        &members,
    ) {
        return error(StatusCode::BAD_REQUEST, "invalid_keyring", e);
    }
    let old = a.keys.clone();
    a.keys.keyring = Some(body.keyring.clone());
    a.keys.version += 1;
    if let Err(e) = a.save_keys() {
        a.keys = old;
        return error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e.to_string());
    }
    if body.reset {
        if let Err(e) = a.reset_log() {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                format!("the keyring is stored but the old log was not dropped ({e}); retry"),
            );
        }
    }
    let version = a.keys.version;
    drop(a);
    relay.devices.audit(
        &who.account,
        AuditEvent::new("pool_keys")
            .device(Some(&device))
            .detail(format!(
                "generation {} for {} device(s){}",
                body.keyring.gen,
                body.keyring.wraps.len(),
                if body.reset {
                    ", new chain (log dropped)"
                } else {
                    ""
                }
            )),
    );
    Json(json!({"version": version})).into_response()
}

/// For tests and `/v1/pool` diagnostics: the account's lease.
pub fn describe(state: &PoolState, account: &str) -> Option<BTreeMap<String, Value>> {
    let a = state.account(account).ok()?;
    let a = a.lock().expect("pool");
    let v = status_view(&a, Instant::now());
    v.as_object()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}
