//! Pool runs of a Space's tree: who submitted what, and which runs a tree may
//! see, so a Space's jobs are its children in every way that matters.
//!
//! A Space with the children capability may use the account's pool (`cua run`,
//! `cua jobs`, `cua computers`) through its own channel. Every run it submits
//! is recorded here under the Space and the root of its tree
//! (`<home>/spawn/runs.json`):
//!
//! - the tree sees its own runs (and only those): list, logs, result, cancel;
//!   a run of another tree is "not found", as a Space of another tree is;
//! - the daemon cancels a run when the Space that submitted it is deleted,
//!   or stopped ([`super::bridge`]), with the Spaces the run made, so a
//!   finished conversation leaves no GPU job behind.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// One recorded run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owned {
    /// The Space that submitted it.
    pub space: String,
    /// The root of that Space's tree.
    pub root: String,
    /// When (unix seconds).
    pub at: u64,
}

/// The runs of every tree on this computer.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ledger {
    /// Run id to its owner.
    #[serde(default)]
    pub runs: BTreeMap<String, Owned>,
}

static LOCK: Mutex<()> = Mutex::new(());

impl Ledger {
    fn path(home: &Path) -> PathBuf {
        home.join("spawn").join("runs.json")
    }

    /// Loads it (empty when missing or unreadable).
    pub fn load(home: &Path) -> Self {
        std::fs::read(Self::path(home))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, home: &Path) -> std::io::Result<()> {
        let p = Self::path(home);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, p)
    }

    /// Runs `f` on the stored ledger and keeps what it changed.
    pub fn update<T>(home: &Path, f: impl FnOnce(&mut Ledger) -> T) -> T {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut l = Self::load(home);
        let out = f(&mut l);
        if let Err(e) = l.save(home) {
            tracing::warn!(error = %e, "could not save the runs of the Spaces' trees");
        }
        out
    }

    /// Records `run` for `space` of the tree under `root`. Refused (`false`)
    /// when another tree owns it.
    pub fn claim(&mut self, run: &str, space: &str, root: &str, now: u64) -> bool {
        match self.runs.get(run) {
            Some(o) if o.root != root => false,
            Some(_) => true,
            None => {
                self.runs.insert(
                    run.to_string(),
                    Owned {
                        space: space.into(),
                        root: root.into(),
                        at: now,
                    },
                );
                true
            }
        }
    }

    /// Whether the tree under `root` may see `run`.
    pub fn visible(&self, run: &str, root: &str) -> bool {
        self.runs.get(run).is_some_and(|o| o.root == root)
    }
}

/// What a pool method asks of the tree that sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolOp {
    /// The computers and the queue: read only, anyone's.
    Read,
    /// A new run: recorded under the sender.
    Submit,
    /// The runs of the sender's tree, listed.
    List,
    /// One run, named: only the sender's tree's.
    OwnRun,
    /// One job, named by id: found first, then only the sender's tree's.
    OwnJob,
    /// Anything else the pool offers (its keys, durable Spaces, audit): refused.
    Refused,
}

/// How the scoped channel treats pool method `method`.
pub fn pool_op(method: &str) -> PoolOp {
    match method {
        "status" | "pool" => PoolOp::Read,
        "submit" => PoolOp::Submit,
        "runs" => PoolOp::List,
        "run" | "run_result" | "job_output" | "jobs" | "cancel_run" => PoolOp::OwnRun,
        "job" | "cancel_job" => PoolOp::OwnJob,
        _ => PoolOp::Refused,
    }
}

/// Runs whose submitting Space is gone or turned off: the ones to cancel.
/// `alive` says whether a Space id is registered and on.
pub fn orphaned(l: &Ledger, alive: impl Fn(&str) -> bool) -> Vec<String> {
    l.runs
        .iter()
        .filter(|(_, o)| !alive(&o.space))
        .map(|(r, _)| r.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_space_may_use_the_pool_for_its_own_runs_and_nothing_else_of_it() {
        for m in ["status", "pool"] {
            assert_eq!(pool_op(m), PoolOp::Read);
        }
        assert_eq!(pool_op("submit"), PoolOp::Submit);
        assert_eq!(pool_op("runs"), PoolOp::List);
        for m in ["run", "run_result", "job_output", "jobs", "cancel_run"] {
            assert_eq!(pool_op(m), PoolOp::OwnRun, "{m}");
        }
        for m in ["job", "cancel_job"] {
            assert_eq!(pool_op(m), PoolOp::OwnJob, "{m}");
        }
        // The pool's keys, its audit log, durable Spaces and its other methods stay the owner's.
        for m in [
            "audit",
            "durable_create",
            "keys",
            "pool_keys",
            "drain",
            "",
            "SUBMIT",
        ] {
            assert_eq!(pool_op(m), PoolOp::Refused, "{m:?}");
        }
    }

    #[test]
    fn a_tree_sees_only_its_own_runs() {
        let mut l = Ledger::default();
        assert!(l.claim("p-1", "local:a", "local:root1", 1));
        assert!(l.claim("p-1", "local:b", "local:root1", 2), "same tree");
        assert!(!l.claim("p-1", "local:x", "local:root2", 3), "another tree");
        assert!(l.visible("p-1", "local:root1"));
        assert!(!l.visible("p-1", "local:root2"));
        assert!(!l.visible("p-2", "local:root1"));
    }

    #[test]
    fn runs_of_a_gone_or_stopped_space_are_the_orphans() {
        let mut l = Ledger::default();
        l.claim("p-1", "local:live", "local:r", 1);
        l.claim("p-2", "local:gone", "local:r", 1);
        let o = orphaned(&l, |s| s == "local:live");
        assert_eq!(o, vec!["p-2".to_string()]);
    }

    #[test]
    fn the_ledger_round_trips_on_disk() {
        let d = tempfile::tempdir().unwrap();
        Ledger::update(d.path(), |l| {
            l.claim("p-9", "local:a", "local:r", 5);
        });
        assert!(Ledger::load(d.path()).visible("p-9", "local:r"));
        Ledger::update(d.path(), |l| {
            l.runs.remove("p-9");
        });
        assert!(Ledger::load(d.path()).runs.is_empty());
    }
}
