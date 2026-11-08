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
use serde_json::Value;

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

/// Providers a run may name in `on` that spend the owner's cloud account.
const PROVIDERS: [&str; 6] = ["modal", "aws", "gcp", "hetzner", "cloud", "elastic"];

/// What a Space's `submit` may ask for, and what the daemon sets on it: the
/// run becomes the Space's child wherever it is placed. Refused: a native
/// machine job (it runs outside every network rule) unless the Space has
/// none to escape, host services (`services`, they make Spaces on the host's
/// own terms) and a cloud provider (the owner's money, outside the tree's
/// budget). Set, over anything the request said (the Space's own say-so is
/// never believed): the parent, root, depth and network the job's Space is
/// bound to ([`cua_cluster::model::ENV_PARENT_SPACE`]).
pub fn confine_submit(
    params: &mut Value,
    run: &str,
    space: &str,
    root: &str,
    depth: u32,
    network: &cua_spaces::network::NetworkSettings,
) -> Result<(), String> {
    use cua_cluster::model::{
        ENV_PARENT_NETWORK, ENV_PARENT_SPACE, ENV_ROOT_SPACE, ENV_TREE_DEPTH, KIND_MACHINE,
    };
    let open = *network == cua_spaces::network::NetworkSettings::internet();
    let tag = serde_json::to_string(network).unwrap_or_default();
    let check = |v: &Value| -> Result<(), String> {
        if v["kind"].as_str() == Some(KIND_MACHINE) && !open {
            return Err("start a native machine job from a Space with network rules: it would run                         outside them (ask the owner, or use a container job)"
                .into());
        }
        if v["services"].as_array().is_some_and(|s| !s.is_empty()) {
            return Err("ask for host services: they are the owner's".into());
        }
        let on = v["on"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if PROVIDERS.contains(&on.as_str()) {
            return Err(format!(
                "place a run on {on}: it spends the owner's cloud account outside the tree's budget"
            ));
        }
        Ok(())
    };
    check(params)?;
    for j in params["jobs"].as_array().into_iter().flatten() {
        check(&j["spec"])?;
    }
    // The row's name: the agent's own, cut to a pet-name shape, else one made from the run.
    let name = cua_cluster::model::job_display_name(
        params["env"][cua_cluster::model::ENV_JOB_NAME].as_str(),
        run,
    );
    let stamp = |v: &mut Value| {
        if !v["env"].is_object() {
            v["env"] = serde_json::json!({});
        }
        v["env"][ENV_PARENT_SPACE] = Value::String(space.to_string());
        v["env"][ENV_ROOT_SPACE] = Value::String(root.to_string());
        v["env"][ENV_TREE_DEPTH] = Value::String((depth + 1).to_string());
        v["env"][ENV_PARENT_NETWORK] = Value::String(tag.clone());
        v["env"][cua_cluster::model::ENV_JOB_NAME] = Value::String(name.clone());
    };
    stamp(params);
    for j in params
        .get_mut("jobs")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(spec) = j.get_mut("spec") {
            stamp(spec);
        }
    }
    Ok(())
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

    fn net(preset: cua_spaces::network::NetworkPreset) -> cua_spaces::network::NetworkSettings {
        cua_spaces::network::NetworkSettings::preset(preset)
    }

    #[test]
    fn a_submit_becomes_a_child_run_that_says_so_over_any_claim() {
        use serde_json::json;
        let inside = net(cua_spaces::network::NetworkPreset::None);
        let mut p = json!({
            "command": ["python", "x.py"],
            // The Space lies about its place and its network.
            "env": {"CUA_PARENT_SPACE": "local:victim", "CUA_PARENT_NETWORK": "{}", "A": "1"},
            "jobs": [{"spec": {"env": {"CUA_TREE_DEPTH": "0"}, "argv": ["x"]}}],
        });
        confine_submit(&mut p, "p-1", "local:me", "local:root", 1, &inside).unwrap();
        for env in [&p["env"], &p["jobs"][0]["spec"]["env"]] {
            assert_eq!(env["CUA_PARENT_SPACE"], "local:me");
            assert_eq!(env["CUA_ROOT_SPACE"], "local:root");
            assert_eq!(env["CUA_TREE_DEPTH"], "2");
            let n: cua_spaces::network::NetworkSettings =
                serde_json::from_str(env["CUA_PARENT_NETWORK"].as_str().unwrap()).unwrap();
            assert_eq!(n, inside);
        }
        assert_eq!(p["env"]["A"], "1", "its own variables stay");
        // A request with no explicit jobs gets no `jobs` key (a null one is refused by the coordinator).
        let mut plain = json!({"command": ["x"]});
        confine_submit(&mut plain, "p-1", "local:me", "local:root", 1, &inside).unwrap();
        assert!(plain.get("jobs").is_none(), "{plain}");
        // No name asked for: a pet name of the run, never the run's id.
        assert!(!p["env"]["CUA_JOB_NAME"].as_str().unwrap().starts_with("p-"));
        let mut q = json!({"command": ["x"], "env": {"CUA_JOB_NAME": "NCA Logo"}});
        confine_submit(&mut q, "p-9", "local:me", "local:root", 1, &inside).unwrap();
        assert_eq!(q["env"]["CUA_JOB_NAME"], "nca-logo");
    }

    #[test]
    fn a_space_with_network_rules_cannot_start_what_would_run_outside_them() {
        use serde_json::json;
        let closed = net(cua_spaces::network::NetworkPreset::None);
        let open = cua_spaces::network::NetworkSettings::internet();
        let machine = json!({"kind": "machine", "command": ["x"]});
        let explicit = json!({"jobs": [{"spec": {"kind": "machine", "argv": ["x"]}}]});
        for p in [&machine, &explicit] {
            assert!(
                confine_submit(&mut p.clone(), "p-1", "local:a", "local:r", 0, &closed).is_err()
            );
            assert!(confine_submit(&mut p.clone(), "p-1", "local:a", "local:r", 0, &open).is_ok());
        }
        // Host services and the owner's cloud are refused whatever the network.
        for p in [
            json!({"services": ["compat:e2b"], "command": ["x"]}),
            json!({"jobs": [{"spec": {"services": ["compat:e2b"]}}]}),
            json!({"on": "modal", "command": ["x"]}),
            json!({"on": "AWS", "command": ["x"]}),
            json!({"jobs": [{"spec": {"on": "hetzner"}}]}),
        ] {
            assert!(
                confine_submit(&mut p.clone(), "p-1", "local:a", "local:r", 0, &open).is_err(),
                "{p}"
            );
        }
        // A host of the owner's by name, and a container job, are fine.
        let ok = json!({"on": "host:mac1", "kind": "container", "command": ["x"]});
        assert!(confine_submit(&mut ok.clone(), "p-1", "local:a", "local:r", 0, &closed).is_ok());
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
