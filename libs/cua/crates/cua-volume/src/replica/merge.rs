// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What to do when two machines changed one key.
//!
//! Almost always: keep the owner's version at the key and store the other
//! machine's content next to it as `name (conflict from <machine> <time>).ext`
//! (the same naming the sync feed uses). Nothing is overwritten and nothing
//! is lost. Control objects whose meaning is an order merge by that meaning
//! instead: the session fence is a monotonic epoch, so the higher epoch
//! wins and a stale driver is fenced the moment the machines meet again.

use crate::feed::conflict_path;
use crate::journal::FENCE;

/// How a conflict on `key` resolves.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Keep the head already at the key, drop the incoming content (it is
    /// identical or lower).
    KeepHead,
    /// Replace the head with the incoming content (it is higher).
    TakeIncoming,
    /// Keep the head; store the incoming content at this key.
    Copy(String),
}

/// A session fence key (`.../sessions/<session>/fence`).
fn is_fence(key: &str) -> bool {
    key.rsplit('/').next() == Some(FENCE) && key.contains("/sessions/")
}

/// A durable run's supervisor lease (`agents/<agent>/.cua-supervisor`).
fn is_supervisor(key: &str) -> bool {
    key.rsplit('/').next() == Some(crate::replica::supervision::LEASE)
}

/// A run's record (`.cua-runs/<agent>.json`).
fn is_run_record(key: &str) -> bool {
    key.starts_with(crate::replica::supervision::RUNS_DIR)
}

/// Keys whose value is an order (fence, supervisor lease, run record): a
/// change to one is always judged against the head by that order, even when
/// it was written against the head's own version, because a stale writer's
/// later changes chain onto its earlier ones.
pub fn is_ordered(key: &str) -> bool {
    is_fence(key) || is_supervisor(key) || is_run_record(key)
}

fn json_field(bytes: &[u8], field: &str) -> Option<serde_json::Value> {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()?
        .get(field)
        .cloned()
}

fn fence_epoch(bytes: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()?
        .get("epoch")?
        .as_u64()
}

/// Resolves a clash on `key` between the head's bytes and the incoming
/// bytes from `machine` at `ts_ms`.
pub fn resolve(key: &str, head: &[u8], incoming: &[u8], machine: &str, ts_ms: u64) -> Resolution {
    if head == incoming {
        return Resolution::KeepHead;
    }
    if is_fence(key)
        && let (Some(h), Some(i)) = (fence_epoch(head), fence_epoch(incoming))
    {
        return if i > h {
            Resolution::TakeIncoming
        } else {
            Resolution::KeepHead
        };
    }
    // The supervisor lease is an order: the higher epoch holds the run, and
    // within an epoch the later beat.
    if is_supervisor(key) {
        let order = |b: &[u8]| {
            (
                json_field(b, "epoch").and_then(|v| v.as_u64()).unwrap_or(0),
                json_field(b, "beat").and_then(|v| v.as_u64()).unwrap_or(0),
            )
        };
        return if order(incoming) > order(head) {
            Resolution::TakeIncoming
        } else {
            Resolution::KeepHead
        };
    }
    // A finished run stays finished.
    if is_run_record(key) {
        let done = |b: &[u8]| {
            json_field(b, "state")
                .and_then(|v| v.as_str().map(|s| s == "done"))
                .unwrap_or(false)
        };
        return if done(incoming) && !done(head) {
            Resolution::TakeIncoming
        } else {
            Resolution::KeepHead
        };
    }
    Resolution::Copy(conflict_path(key, machine, ts_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_files_become_conflict_copies() {
        let r = resolve("agents/ada/notes.md", b"a", b"b", "MacBook", 0);
        match r {
            Resolution::Copy(k) => {
                assert!(
                    k.starts_with("agents/ada/notes (conflict from MacBook "),
                    "{k}"
                );
                assert!(k.ends_with(").md"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            resolve("agents/ada/notes.md", b"same", b"same", "m", 0),
            Resolution::KeepHead
        );
    }

    #[test]
    fn a_supervisor_lease_goes_to_the_higher_epoch_and_a_done_run_stays_done() {
        let k = "agents/ada/.cua-supervisor";
        let a = br#"{"epoch":2,"beat":9}"#;
        let b = br#"{"epoch":3,"beat":0}"#;
        assert_eq!(resolve(k, a, b, "m", 0), Resolution::TakeIncoming);
        assert_eq!(resolve(k, b, a, "m", 0), Resolution::KeepHead);
        let r = ".cua-runs/ada.json";
        assert_eq!(
            resolve(r, br#"{"state":"running"}"#, br#"{"state":"done"}"#, "m", 0),
            Resolution::TakeIncoming
        );
        assert_eq!(
            resolve(r, br#"{"state":"done"}"#, br#"{"state":"running"}"#, "m", 0),
            Resolution::KeepHead
        );
    }

    #[test]
    fn a_fence_merges_to_the_higher_epoch() {
        let k = "spaces/dev/sessions/s1/fence";
        let low = br#"{"epoch":2,"holder":"a"}"#;
        let high = br#"{"epoch":5,"holder":"b"}"#;
        assert_eq!(resolve(k, low, high, "m", 0), Resolution::TakeIncoming);
        assert_eq!(resolve(k, high, low, "m", 0), Resolution::KeepHead);
    }
}
