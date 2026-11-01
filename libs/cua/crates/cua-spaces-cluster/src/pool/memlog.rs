// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! An in-memory stand-in for the relay's pool (lease, log and keyring)
//! with the same rules, for tests and the deterministic simulation: epochs
//! only grow; an append of another epoch is refused; an append names the
//! head it extends (a retry of stored entries returns the head); a job ends
//! at most once. Once the account has a keyring ([`MemPool::keys`]), an
//! append must be sealed under its newest generation, and is refused
//! (`rekey`) while a device that holds the keys has left. With
//! [`MemPool::limit_log`], an append past that many entries since the last
//! snapshot is refused ([`JournalError::Full`]) until the holder stores a
//! snapshot, as the relay does past its log and quota limits.
//! [`MemPool::history`] keeps every grant, append and key change for
//! invariant checks.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::coordinator::journal::{Journal, JournalError, LogEntry, LogReader};
use crate::pool::keys::{KeyDirectory, KeyError, KeyState};
use crate::pool::seal::{Envelope, Keyring, Member};

/// One thing that happened to the pool, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `holder` was granted `epoch`.
    Granted {
        /// Epoch.
        epoch: u64,
        /// Who.
        holder: String,
    },
    /// Entries `from+1 ..= head` were appended under `epoch`.
    Appended {
        /// Epoch.
        epoch: u64,
        /// The head before.
        from: u64,
        /// The head after.
        head: u64,
    },
    /// An append under `epoch` was refused (another epoch is current).
    Fenced {
        /// The refused epoch.
        epoch: u64,
        /// The current one.
        current: u64,
    },
    /// The keyring was stored with generation `generation`, wrapped to
    /// `to`.
    Keyring {
        /// Generation.
        generation: u64,
        /// The devices it is wrapped to.
        to: Vec<String>,
    },
    /// `device` left the account (revoked).
    Revoked {
        /// The device.
        device: String,
    },
    /// An append sealed under an older generation (or not sealed) was
    /// refused.
    Rekey {
        /// The generation it was sealed under (0: not sealed).
        sealed: u64,
        /// The keyring's.
        current: u64,
    },
}

#[derive(Default)]
struct State {
    epoch: u64,
    holder: Option<String>,
    expired: bool,
    entries: Vec<LogEntry>,
    epochs: Vec<u64>,
    ended: HashSet<String>,
    history: Vec<Event>,
    /// Appends that fail with "unavailable" before reaching the log, and
    /// ones that are stored but whose answer is lost (the network).
    drop_requests: u32,
    drop_answers: u32,
    account: String,
    keyring: Option<Keyring>,
    keyring_version: u64,
    members: HashMap<String, Member>,
    revoked: HashSet<String>,
    /// When each device was last seen (the relay's device list).
    last_seen: HashMap<String, u64>,
    /// Entries past the last snapshot after which appends are refused.
    log_limit: Option<u64>,
    /// The newest snapshot's position.
    snapshot_seq: u64,
    /// Snapshots stored: (epoch, position).
    snapshots: Vec<(u64, u64)>,
}

impl State {
    fn members(&self) -> Vec<Member> {
        let mut m: Vec<Member> = self
            .members
            .values()
            .filter(|m| !self.revoked.contains(&m.device))
            .cloned()
            .collect();
        m.sort_by(|a, b| a.device.cmp(&b.device));
        m
    }

    /// Whether `data` may be appended now: `Err((sealed, current))`.
    fn key_gate(&self, data: &serde_json::Value) -> Result<(), (u64, u64)> {
        let Some(ring) = &self.keyring else {
            return Ok(());
        };
        let sealed = Envelope::parse(data).map_or(0, |e| e.g);
        if sealed != ring.generation || ring.stale(&self.members()) {
            return Err((sealed, ring.generation));
        }
        Ok(())
    }
}

/// The in-memory pool.
#[derive(Clone, Default)]
pub struct MemPool {
    s: Arc<Mutex<State>>,
}

impl MemPool {
    /// An empty pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// `who` acquires the lease if it is vacant or expired: its new epoch.
    pub fn acquire(&self, who: &str) -> Option<u64> {
        let mut s = self.s.lock().expect("pool");
        if s.holder.is_some() && !s.expired && s.holder.as_deref() != Some(who) {
            return None;
        }
        s.epoch += 1;
        s.holder = Some(who.to_string());
        s.expired = false;
        let epoch = s.epoch;
        s.history.push(Event::Granted {
            epoch,
            holder: who.to_string(),
        });
        Some(epoch)
    }

    /// The lease runs out (its holder stopped renewing, or a clock jumped).
    pub fn expire(&self) {
        self.s.lock().expect("pool").expired = true;
    }

    /// The current epoch and holder.
    pub fn lease(&self) -> (u64, Option<String>) {
        let s = self.s.lock().expect("pool");
        (s.epoch, s.holder.clone())
    }

    /// Refuses appends ([`JournalError::Full`]) once `entries` entries
    /// follow the last snapshot, until a snapshot is stored.
    pub fn limit_log(&self, entries: u64) {
        self.s.lock().expect("pool").log_limit = Some(entries);
    }

    /// The snapshots stored: (epoch, position).
    pub fn snapshots(&self) -> Vec<(u64, u64)> {
        self.s.lock().expect("pool").snapshots.clone()
    }

    /// The next `n` appends fail before reaching the log.
    pub fn drop_requests(&self, n: u32) {
        self.s.lock().expect("pool").drop_requests += n;
    }

    /// The next `n` appends are stored but their answers are lost.
    pub fn drop_answers(&self, n: u32) {
        self.s.lock().expect("pool").drop_answers += n;
    }

    /// Every entry.
    pub fn entries(&self) -> Vec<LogEntry> {
        self.s.lock().expect("pool").entries.clone()
    }

    /// The epoch each entry was appended under.
    pub fn entry_epochs(&self) -> Vec<u64> {
        self.s.lock().expect("pool").epochs.clone()
    }

    /// Everything that happened.
    pub fn history(&self) -> Vec<Event> {
        self.s.lock().expect("pool").history.clone()
    }

    /// The log as written under `epoch`.
    pub fn log(&self, epoch: u64) -> MemLog {
        MemLog {
            pool: self.clone(),
            epoch,
        }
    }

    /// The account's keyring and members, as `account`'s devices see them
    /// (the relay's `/v1/pool/keys`).
    pub fn keys(&self, account: &str) -> MemKeys {
        self.s.lock().expect("pool").account = account.to_string();
        MemKeys { pool: self.clone() }
    }

    /// `device` leaves the account (revoked): it is no member any more,
    /// and appends are refused until the keys are made again without it.
    pub fn revoke(&self, device: &str) {
        let mut s = self.s.lock().expect("pool");
        s.revoked.insert(device.to_string());
        s.history.push(Event::Revoked {
            device: device.to_string(),
        });
    }

    /// The relay saw `device` at `secs` (Unix seconds).
    pub fn saw(&self, device: &str, secs: u64) {
        self.s
            .lock()
            .expect("pool")
            .last_seen
            .insert(device.to_string(), secs);
    }

    /// The stored keyring.
    pub fn keyring(&self) -> Option<Keyring> {
        self.s.lock().expect("pool").keyring.clone()
    }
}

/// [`MemPool`]'s keyring and members, with the relay's rules: a member is
/// a device that registered a pool key its device key signed and was not
/// revoked; a keyring update names the version it replaces; a keyring is
/// wrapped only to members; within a generation wraps are only added (and
/// not while a device that holds them has left); a new generation replaces
/// them; a reset (a new chain) only when no member holds the keys.
#[derive(Clone)]
pub struct MemKeys {
    pool: MemPool,
}

#[async_trait]
impl KeyDirectory for MemKeys {
    async fn register(&self, me: &Member) -> Result<(), KeyError> {
        let mut s = self.pool.s.lock().expect("pool");
        if s.revoked.contains(&me.device) {
            return Err(KeyError::Refused("revoked".into()));
        }
        if !me.verify(&s.account) {
            return Err(KeyError::Refused("bad signature".into()));
        }
        s.members.insert(me.device.clone(), me.clone());
        Ok(())
    }

    async fn fetch(&self) -> Result<KeyState, KeyError> {
        let s = self.pool.s.lock().expect("pool");
        Ok(KeyState {
            keyring: s.keyring.clone(),
            version: s.keyring_version,
            members: s.members(),
        })
    }

    async fn put(&self, version: u64, ring: &Keyring, reset: bool) -> Result<u64, KeyError> {
        let mut s = self.pool.s.lock().expect("pool");
        if version != s.keyring_version {
            return Err(KeyError::Conflict);
        }
        check_keyring(s.keyring.as_ref(), ring, reset, &s.members()).map_err(KeyError::Refused)?;
        if reset {
            s.entries.clear();
            s.epochs.clear();
            s.ended.clear();
        }
        s.keyring = Some(ring.clone());
        s.keyring_version += 1;
        let to = ring.wraps.iter().map(|w| w.device.clone()).collect();
        s.history.push(Event::Keyring {
            generation: ring.generation,
            to,
        });
        Ok(s.keyring_version)
    }

    async fn last_seen(&self) -> HashMap<String, u64> {
        self.pool.s.lock().expect("pool").last_seen.clone()
    }
}

/// The relay's rules for a keyring update (`old` stored, `new` proposed).
pub fn check_keyring(
    old: Option<&Keyring>,
    new: &Keyring,
    reset: bool,
    members: &[Member],
) -> Result<(), String> {
    let member = |d: &str, k: &str| members.iter().any(|m| m.device == d && m.pool_key == k);
    if new.generation == 0 || new.wraps.is_empty() {
        return Err("an empty keyring".into());
    }
    if let Some(w) = new.wraps.iter().find(|w| !member(&w.device, &w.key)) {
        return Err(format!("wrapped to {}, which is not a member", w.device));
    }
    if let Some(w) = new
        .wraps
        .iter()
        .find(|w| !members.iter().any(|m| m.device == w.from))
    {
        return Err(format!("wrapped by {}, which is not a member", w.from));
    }
    let Some(old) = old else {
        return if reset {
            Err("nothing to reset".into())
        } else {
            Ok(())
        };
    };
    if reset {
        if old.wraps.iter().any(|w| member(&w.device, &w.key)) {
            return Err("a member still holds the keys: no reset".into());
        }
        return if new.generation > old.generation {
            Ok(())
        } else {
            Err("a reset must start a newer generation".into())
        };
    }
    if new.generation == old.generation + 1 {
        return Ok(());
    }
    if new.generation != old.generation {
        return Err(format!(
            "generation {} after {}",
            new.generation, old.generation
        ));
    }
    if old.stale(members) {
        return Err("a device that holds the keys left: make a new generation".into());
    }
    if old.wraps.iter().any(|w| !new.wraps.contains(w)) {
        return Err("within a generation, wraps are only added".into());
    }
    Ok(())
}

/// [`MemPool`]'s log under one epoch.
#[derive(Clone)]
pub struct MemLog {
    pool: MemPool,
    epoch: u64,
}

#[async_trait]
impl Journal for MemLog {
    fn replicated(&self) -> bool {
        true
    }

    async fn append(&self, from: u64, entries: Vec<LogEntry>) -> Result<u64, JournalError> {
        let mut s = self.pool.s.lock().expect("pool");
        if s.drop_requests > 0 {
            s.drop_requests -= 1;
            return Err(JournalError::Unavailable("the request was lost".into()));
        }
        if self.epoch != s.epoch {
            let current = s.epoch;
            s.history.push(Event::Fenced {
                epoch: self.epoch,
                current,
            });
            return Err(JournalError::Fenced(format!(
                "epoch {} is over ({current} now)",
                self.epoch
            )));
        }
        let head = s.entries.len() as u64;
        let n = entries.len() as u64;
        if from < head {
            let same = from + n <= head
                && entries.iter().enumerate().all(|(i, e)| {
                    let stored = &s.entries[(from as usize) + i];
                    s.epochs[(from as usize) + i] == self.epoch
                        && stored.kind == e.kind
                        && stored.job == e.job
                        && stored.terminal == e.terminal
                        && stored.data == e.data
                });
            return if same {
                Ok(from + n)
            } else {
                Err(JournalError::Conflict(format!(
                    "head is {head}, not {from}"
                )))
            };
        }
        if from > head {
            return Err(JournalError::Conflict(format!(
                "head is {head}, not {from}"
            )));
        }
        if s.log_limit
            .is_some_and(|limit| head - s.snapshot_seq >= limit)
        {
            return Err(JournalError::Full(format!(
                "{} entries since the snapshot at {}: write a snapshot",
                head - s.snapshot_seq,
                s.snapshot_seq
            )));
        }
        for e in &entries {
            if let Err((sealed, current)) = s.key_gate(&e.data) {
                s.history.push(Event::Rekey { sealed, current });
                return Err(JournalError::Rekey(format!(
                    "sealed under generation {sealed}; the keys are at {current}"
                )));
            }
        }
        let mut seen = HashSet::new();
        for e in &entries {
            if e.terminal && (s.ended.contains(&e.job) || !seen.insert(e.job.clone())) {
                return Err(JournalError::Conflict(format!(
                    "duplicate_terminal: {}",
                    e.job
                )));
            }
        }
        for (i, mut e) in entries.into_iter().enumerate() {
            e.seq = head + 1 + i as u64;
            if e.terminal {
                s.ended.insert(e.job.clone());
            }
            s.entries.push(e);
            let epoch = self.epoch;
            s.epochs.push(epoch);
        }
        let epoch = self.epoch;
        s.history.push(Event::Appended {
            epoch,
            from: head,
            head: head + n,
        });
        if s.drop_answers > 0 {
            s.drop_answers -= 1;
            return Err(JournalError::Unavailable("the answer was lost".into()));
        }
        Ok(head + n)
    }

    async fn snapshot(&self, seq: u64, _data: Vec<u8>) -> Result<(), JournalError> {
        let mut s = self.pool.s.lock().expect("pool");
        if self.epoch != s.epoch {
            return Err(JournalError::Fenced(format!(
                "epoch {} is over ({} now)",
                self.epoch, s.epoch
            )));
        }
        if seq > s.entries.len() as u64 {
            return Err(JournalError::Unavailable(
                "snapshot seq outside the log".into(),
            ));
        }
        s.snapshot_seq = s.snapshot_seq.max(seq);
        let epoch = self.epoch;
        s.snapshots.push((epoch, seq));
        Ok(())
    }
}

#[async_trait]
impl LogReader for MemLog {
    async fn read(&self, after: u64) -> Result<Option<(Vec<LogEntry>, u64)>, JournalError> {
        let s = self.pool.s.lock().expect("pool");
        let head = s.entries.len() as u64;
        Ok(Some((
            s.entries
                .iter()
                .skip(after as usize)
                .take(5000)
                .cloned()
                .collect(),
            head,
        )))
    }

    async fn snapshot(&self) -> Result<Option<(u64, Vec<u8>)>, JournalError> {
        Ok(None)
    }
}
