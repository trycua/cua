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
    /// The name its row shows (a pet-name shape, never an id).
    #[serde(default)]
    pub name: String,
    /// It asks for a GPU (its row shows the GPU icon).
    #[serde(default)]
    pub gpu: bool,
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
                        name: String::new(),
                        gpu: false,
                    },
                );
                true
            }
        }
    }

    /// Records what the run's row shows.
    pub fn describe(&mut self, run: &str, name: &str, gpu: bool) {
        if let Some(o) = self.runs.get_mut(run) {
            o.name = name.to_string();
            o.gpu = gpu;
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

/// The name and GPU mark of a stamped submit ([`confine_submit`] ran first).
pub fn job_facts(params: &Value) -> (String, bool) {
    let name = params["env"][cua_cluster::model::ENV_JOB_NAME]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let wants = |v: &Value| {
        v["accelerator"]
            .as_str()
            .is_some_and(|a| !a.trim().is_empty() && a.trim() != "none")
    };
    let gpu = wants(params)
        || params["jobs"]
            .as_array()
            .is_some_and(|j| j.iter().any(|x| wants(&x["spec"])));
    (name, gpu)
}

/// One job row of the child Spaces list: ONE per run, wherever it runs (a Space on this
/// computer, another computer, a native machine job with no Space at all, a gang of
/// several members: one row with the member count).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRowOut {
    /// `job:<run>`.
    pub id: String,
    pub run: String,
    /// The Space that submitted it.
    pub parent: String,
    pub name: String,
    pub gpu: bool,
    /// Where it runs, in words; empty while it waits for a computer.
    pub host: String,
    /// `queued`, `starting`, `running`, `failed`.
    pub state: String,
    /// Members of a gang (1: a plain job).
    pub members: u32,
    /// Why it failed, when it did.
    #[serde(default)]
    pub error: String,
}

/// The row of `owned`'s run from the pool's own answer for it (`None`: the pool does not
/// know the run yet, so it is queued). A run that ended well or was canceled has no row
/// (its parent's list is clean again); a failed one keeps its row, with why.
pub fn job_row(run: &str, owned: &Owned, pool: Option<&Value>) -> Option<JobRowOut> {
    let name = if owned.name.is_empty() {
        cua_cluster::model::pet_name(run)
    } else {
        owned.name.clone()
    };
    // The gang's size: `total` counts every attempt's jobs, so a retried single job would read as a gang.
    let members = pool
        .and_then(|p| p["gang"]["size"].as_u64())
        .unwrap_or(1)
        .max(1) as u32;
    let hosts: Vec<&str> = pool
        .and_then(|p| p["gang"]["hosts"].as_array())
        .map(|h| h.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let host = match hosts.as_slice() {
        [] => String::new(),
        ["local"] => "this computer".to_string(),
        [one] => (*one).to_string(),
        many => format!("{} computers", many.len()),
    };
    let word = pool.and_then(|p| p["state"].as_str()).unwrap_or("queued");
    let waiting = pool
        .and_then(|p| p["gang"]["waiting"].as_str())
        .is_some_and(|w| !w.is_empty());
    let state = match word {
        // A host that died mid-job puts the run back in the queue: never "Running".
        _ if waiting => "queued",
        "queued" | "waiting" | "pending" | "submitted" | "placing" => "queued",
        "starting" | "creating" | "preparing" => "starting",
        "running" => "running",
        "failed" | "error" | "timed_out" | "lost" => "failed",
        // Succeeded, canceled, canceling and anything over: no row.
        _ => return None,
    };
    let error = if state == "failed" {
        pool.and_then(|p| p["gang"]["error"].as_str())
            .filter(|e| !e.is_empty())
            .unwrap_or("the run did not finish")
            .to_string()
    } else {
        String::new()
    };
    Some(JobRowOut {
        id: format!("job:{run}"),
        run: run.to_string(),
        parent: owned.space.clone(),
        name,
        gpu: owned.gpu,
        host,
        state: state.into(),
        members,
        error,
    })
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

    fn owned(gpu: bool) -> Owned {
        Owned {
            space: "local:me".into(),
            root: "local:root".into(),
            at: 1,
            name: "logo-grower".into(),
            gpu,
        }
    }

    #[test]
    fn a_run_is_one_job_row_wherever_it_runs() {
        use serde_json::json;
        // On this computer.
        let here = json!({"state": "running", "total": 1, "gang": {"hosts": ["local"]}});
        let r = job_row("p-1", &owned(true), Some(&here)).unwrap();
        assert_eq!(
            (r.state.as_str(), r.host.as_str(), r.gpu, r.members),
            ("running", "this computer", true, 1)
        );
        assert_eq!(r.id, "job:p-1");
        assert_eq!(r.name, "logo-grower");
        // On another computer, Apple GPU (a native machine job has no Space at all: the row still shows).
        let mac = json!({"state": "running", "total": 1, "gang": {"hosts": ["mac-m2max"]}});
        let r = job_row("p-2", &owned(true), Some(&mac)).unwrap();
        assert_eq!(
            (r.host.as_str(), r.state.as_str()),
            ("mac-m2max", "running")
        );
        // A gang of several members is ONE row with a count.
        let gang =
            json!({"state": "running", "total": 3, "gang": {"size": 3, "hosts": ["a", "b", "c"]}});
        let r = job_row("p-3", &owned(true), Some(&gang)).unwrap();
        assert_eq!((r.members, r.host.as_str()), (3, "3 computers"));
        // Not yet known to the pool, or waiting: queued, with no host.
        assert_eq!(job_row("p-4", &owned(false), None).unwrap().state, "queued");
        let wait = json!({"state": "queued", "total": 1, "gang": {"hosts": [], "waiting": "no computer has a free GPU"}});
        assert_eq!(
            job_row("p-4", &owned(false), Some(&wait)).unwrap().state,
            "queued"
        );
    }

    #[test]
    fn a_retried_single_job_is_still_one_member() {
        use serde_json::json;
        // Four attempts of one job: the pool's `total` is 4, the gang is 1.
        let retried = json!({"state": "running", "total": 4, "gang": {"size": 1, "attempt": 4, "hosts": ["mac-m2max"]}});
        assert_eq!(
            job_row("p-9", &owned(true), Some(&retried))
                .unwrap()
                .members,
            1
        );
    }

    #[test]
    fn a_host_that_dies_mid_job_puts_the_row_back_to_queued_not_stuck_at_running() {
        use serde_json::json;
        let lost = json!({"state": "running", "total": 1, "gang": {"hosts": ["mac-m2max"], "waiting": "placing again: its computer was lost"}});
        assert_eq!(
            job_row("p-5", &owned(true), Some(&lost)).unwrap().state,
            "queued"
        );
        let dead = json!({"state": "lost", "total": 1, "gang": {"hosts": ["mac-m2max"], "error": "its computer was lost"}});
        let r = job_row("p-5", &owned(true), Some(&dead)).unwrap();
        assert_eq!(r.state, "failed");
        assert_eq!(r.error, "its computer was lost");
    }

    #[test]
    fn a_finished_run_leaves_no_row_and_a_failed_one_keeps_it() {
        use serde_json::json;
        for done in ["succeeded", "canceled", "canceling"] {
            let p = json!({"state": done, "total": 1, "gang": {"hosts": ["local"]}});
            assert!(job_row("p-6", &owned(false), Some(&p)).is_none(), "{done}");
        }
        let f =
            json!({"state": "failed", "total": 1, "gang": {"hosts": ["local"], "error": "exit 1"}});
        assert_eq!(
            job_row("p-6", &owned(false), Some(&f)).unwrap().state,
            "failed"
        );
    }

    #[test]
    fn a_runs_name_and_gpu_come_from_the_stamped_submit() {
        use serde_json::json;
        let net = cua_spaces::network::NetworkSettings::internet();
        let mut p = json!({"command": ["x"], "accelerator": "apple", "env": {}});
        confine_submit(&mut p, "p-7", "local:a", "local:r", 0, &net).unwrap();
        let (name, gpu) = job_facts(&p);
        assert!(gpu && !name.is_empty() && !name.starts_with("p-"), "{name}");
        let mut q = json!({"command": ["x"], "env": {}});
        confine_submit(&mut q, "p-8", "local:a", "local:r", 0, &net).unwrap();
        assert!(!job_facts(&q).1, "no accelerator: the OS icon");
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
