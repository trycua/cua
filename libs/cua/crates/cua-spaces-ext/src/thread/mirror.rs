// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The thread's replica on the Cua Volume (`spaces/<space>/thread/`), for
//! check-ins from anywhere and for rebuilding a thread whose owner lost it.
//! Only the owner daemon writes it (as the user principal: the Space and
//! its agents may only read there).
//!
//! | Object | Written how |
//! |---|---|
//! | `entries/<seq, 12 digits>.json` | One immutable object per entry, create-only |
//! | `segments/<first>-<last>.jsonl` | One immutable roll-up per closed segment; then per-entry objects older than a week are deleted |
//! | `members.json` | The Space's agent members (no env, no secrets), etag CAS |
//! | `cursors.json` | Members' read positions, etag CAS |
//!
//! [`ship`] runs in the supervisor tick and never blocks posting: a Volume
//! that is down only delays it (the caller backs off).

use sha2::Digest as _;

use super::{ThreadEntry, ThreadStore};
use crate::DriveResult as _;
use cua_spaces::Result;
use cua_volume::{Condition, Session};

/// Entries shipped per pass at most.
const SHIP_BATCH: usize = 500;
/// Per-entry objects deleted per pass at most.
const PRUNE_BATCH: usize = 200;
/// Per-entry objects of a rolled-up segment are kept this long.
pub const KEEP_ENTRIES_MS: u64 = 7 * 24 * 3600 * 1000;

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipReport {
    pub shipped: usize,
    pub rolled: usize,
    pub pruned: usize,
    pub snapshots: usize,
}

fn thread_dir(store: &ThreadStore) -> String {
    format!("{}thread/", store.volume_folder())
}

/// The object of entry `seq`.
pub fn entry_key(store: &ThreadStore, seq: u64) -> String {
    format!("{}entries/{seq:012}.json", thread_dir(store))
}

fn segment_key(store: &ThreadStore, first: u64, last: u64) -> String {
    format!("{}segments/{first:012}-{last:012}.jsonl", thread_dir(store))
}

/// A create-only write that found the object there already wrote it
/// before (an earlier pass that lost its bookkeeping).
async fn create_only(session: &Session, key: &str, bytes: Vec<u8>) -> Result<()> {
    match session.write(key, bytes, Condition::IfNoneMatch).await {
        Ok(_) | Err(cua_volume::Error::Precondition(_)) => Ok(()),
        Err(e) => Err(crate::drive_err(e)),
    }
}

/// One pass: entries after `mirrored_seq`, closed segments' roll-ups,
/// pruning, and the `members` (when given) and cursors snapshots.
pub async fn ship(
    store: &ThreadStore,
    session: &Session,
    members: Option<serde_json::Value>,
) -> Result<ShipReport> {
    let mut report = ShipReport::default();
    let head = store.head()?;
    // Entries, in order; the bookkeeping moves after each one lands.
    let pending = store.read(head.mirrored_seq, SHIP_BATCH)?;
    let mut shipped_to = head.mirrored_seq;
    let mut failure = None;
    for e in &pending {
        match create_only(session, &entry_key(store, e.seq), serde_json::to_vec(e)?).await {
            Ok(()) => {
                shipped_to = e.seq;
                report.shipped += 1;
            }
            Err(err) => {
                failure = Some(err);
                break;
            }
        }
    }
    if shipped_to > head.mirrored_seq {
        store.update_head(|h| h.mirrored_seq = h.mirrored_seq.max(shipped_to))?;
    }
    if let Some(e) = failure {
        return Err(e);
    }
    let head = store.head()?;
    // Closed segments whose entries are all shipped: one roll-up each.
    let segs = store.segments();
    for (i, first) in segs.iter().enumerate() {
        let Some(next) = segs.get(i + 1) else { break };
        if *first <= head.rolled_through || next - 1 > head.mirrored_seq {
            continue;
        }
        let entries = store.read_segment(*first)?;
        let Some(last) = entries.last().map(|e| e.seq) else {
            continue;
        };
        let mut body = vec![];
        for e in &entries {
            body.extend(serde_json::to_vec(e)?);
            body.push(b'\n');
        }
        create_only(session, &segment_key(store, *first, last), body).await?;
        store.update_head(|h| h.rolled_through = h.rolled_through.max(*first))?;
        report.rolled += 1;
    }
    // Per-entry objects of rolled segments, a week old.
    let head = store.head()?;
    if head.rolled_through > 0 {
        let rolled_last = segs
            .iter()
            .find(|f| **f > head.rolled_through)
            .map_or(head.mirrored_seq, |f| f - 1);
        let now = cua_volume::now_ms();
        let mut pruned_to = head.pruned_through;
        for e in store.read(head.pruned_through, PRUNE_BATCH)? {
            if e.seq > rolled_last || now.saturating_sub(e.ts_ms) < KEEP_ENTRIES_MS {
                break;
            }
            match session
                .delete(&entry_key(store, e.seq), Condition::None)
                .await
            {
                Ok(()) | Err(cua_volume::Error::NotFound(_)) => {}
                Err(err) => return Err(crate::drive_err(err)),
            }
            pruned_to = e.seq;
            report.pruned += 1;
        }
        if pruned_to > head.pruned_through {
            store.update_head(|h| h.pruned_through = h.pruned_through.max(pruned_to))?;
        }
    }
    // Snapshots.
    if let Some(m) = members {
        let bytes = serde_json::to_vec_pretty(&m)?;
        if snapshot(store, session, "members.json", bytes, true).await? {
            report.snapshots += 1;
        }
    }
    let cursors = serde_json::to_vec_pretty(&store.cursors()?)?;
    if snapshot(store, session, "cursors.json", cursors, false).await? {
        report.snapshots += 1;
    }
    Ok(report)
}

/// Writes `name` when its content changed, with an etag compare-and-swap
/// (a lost race re-reads the etag; the next pass writes again).
async fn snapshot(
    store: &ThreadStore,
    session: &Session,
    name: &str,
    bytes: Vec<u8>,
    members: bool,
) -> Result<bool> {
    let hash = hex::encode(sha2::Sha256::digest(&bytes));
    let head = store.head()?;
    let (known_hash, etag) = if members {
        (head.members_hash, head.members_etag)
    } else {
        (head.cursors_hash, head.cursors_etag)
    };
    if known_hash.as_deref() == Some(hash.as_str()) {
        return Ok(false);
    }
    let key = format!("{}{name}", thread_dir(store));
    let cond = match etag {
        Some(e) => Condition::IfMatch(e),
        None => Condition::IfNoneMatch,
    };
    match session.write(&key, bytes, cond).await {
        Ok(meta) => {
            store.update_head(|h| {
                if members {
                    h.members_hash = Some(hash);
                    h.members_etag = Some(meta.etag);
                } else {
                    h.cursors_hash = Some(hash);
                    h.cursors_etag = Some(meta.etag);
                }
            })?;
            Ok(true)
        }
        Err(cua_volume::Error::Precondition(_)) => {
            let current = match session.open(&key, None).await {
                Ok(m) => Some(m.etag),
                Err(cua_volume::Error::NotFound(_)) => None,
                Err(e) => return Err(crate::drive_err(e)),
            };
            store.update_head(|h| {
                if members {
                    h.members_etag = current;
                } else {
                    h.cursors_etag = current;
                }
            })?;
            Ok(false)
        }
        Err(e) => Err(crate::drive_err(e)),
    }
}

/// Rebuilds a thread with no local log from the Volume: the roll-ups,
/// then the per-entry objects. Returns the entries replayed.
pub async fn rebuild(store: &ThreadStore, session: &Session) -> Result<usize> {
    let dir = thread_dir(store);
    let mut entries: std::collections::BTreeMap<u64, ThreadEntry> = Default::default();
    for (sub, lines) in [("segments/", true), ("entries/", false)] {
        let objs = match session.walk(&format!("{dir}{sub}")).await {
            Ok(o) => o,
            Err(cua_volume::Error::NotFound(_)) => continue,
            Err(e) => return Err(crate::drive_err(e)),
        };
        for o in objs {
            let (bytes, _) = session.read(&o.key, None).await.drive()?;
            if lines {
                for l in bytes.split(|b| *b == b'\n') {
                    if let Ok(e) = serde_json::from_slice::<ThreadEntry>(l) {
                        entries.entry(e.seq).or_insert(e);
                    }
                }
            } else if let Ok(e) = serde_json::from_slice::<ThreadEntry>(&bytes) {
                entries.entry(e.seq).or_insert(e);
            }
        }
    }
    if entries.is_empty() {
        return Ok(0);
    }
    store.replay(entries.into_values().collect())
}
