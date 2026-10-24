//! Executing a plan. The runner takes a baseline inventory, plays the plan
//! (operations through the client's driver command, faults through the
//! [`FaultTable`]), undoes every fault, cleans up, waits for the system to
//! settle and diffs the end state against the baseline. A run that leaks
//! reports its seed and the exact command that replays it.

use crate::collect::{Sources, collect};
use crate::inject::{FaultTable, run_sh};
use crate::inventory::{Report, Wanted};
use crate::plan::{Plan, Step, StepKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How to run a plan.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Working directory of the run (pidfiles, slot files, the report).
    pub dir: PathBuf,
    /// Executes one operation: argv; the step is in `CHAOS_STEP` (JSON).
    pub driver: Vec<String>,
    pub faults: FaultTable,
    pub sources: Sources,
    /// Prints the wanted Spaces as JSON (a `Wanted`), run before the diff.
    pub wanted_cmd: Option<String>,
    /// Deletes what the scenario still tracks, run before settling.
    pub cleanup_cmd: Option<String>,
    /// How long the end state may take to converge (reapers, TTLs).
    pub settle: Duration,
    /// A driver that runs longer than this is killed.
    pub step_timeout: Duration,
    /// Divides every `at_ms` (a faster replay of a long plan).
    pub speedup: f64,
    /// Extra env for drivers and fault commands.
    pub env: Vec<(String, String)>,
}

/// What one step did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub index: usize,
    pub step: Step,
    /// `ok`, `failed`, `killed` (SIGKILLed by the plan) or `timeout`.
    pub outcome: String,
    pub output: String,
}

/// The outcome of a run.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub seed: u64,
    pub profile: String,
    pub digest: String,
    pub steps: Vec<StepResult>,
    pub faults_injected: Vec<String>,
    pub fault_errors: Vec<String>,
    pub report: Report,
    pub settle_ms: u64,
}

impl RunResult {
    pub fn killed(&self) -> usize {
        self.steps.iter().filter(|s| s.outcome == "killed").count()
    }
}

fn exec_op(
    cfg: &RunConfig,
    plan: &Plan,
    index: usize,
    step: &Step,
    env: &[(String, String)],
) -> StepResult {
    let kill_after = step
        .args
        .get("kill_after_ms")
        .and_then(serde_json::Value::as_u64);
    let Some((exe, rest)) = cfg.driver.split_first() else {
        return StepResult {
            index,
            step: step.clone(),
            outcome: "failed".into(),
            output: "no driver".into(),
        };
    };
    let mut c = Command::new(exe);
    c.args(rest)
        .env(
            "CHAOS_STEP",
            serde_json::to_string(step).unwrap_or_default(),
        )
        .env("CHAOS_ACTOR", step.actor.to_string())
        .env("CHAOS_SEED", plan.seed.to_string())
        .env("CHAOS_DIR", &cfg.dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = match c.spawn() {
        Ok(c) => c,
        Err(e) => {
            return StepResult {
                index,
                step: step.clone(),
                outcome: "failed".into(),
                output: format!("{exe}: {e}"),
            };
        }
    };
    let start = Instant::now();
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(st)) => break if st.success() { "ok" } else { "failed" },
            Ok(None) => {
                if kill_after.is_some_and(|k| start.elapsed() >= Duration::from_millis(k)) {
                    // The client dies mid-operation: SIGKILL, no cleanup.
                    let _ = child.kill();
                    let _ = child.wait();
                    break "killed";
                }
                if start.elapsed() > cfg.step_timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break "timeout";
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break "failed",
        }
    };
    let output = child
        .wait_with_output()
        .map(|o| {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s.chars().take(2000).collect()
        })
        .unwrap_or_default();
    StepResult {
        index,
        step: step.clone(),
        outcome: outcome.into(),
        output,
    }
}

/// Plays `plan` and checks for leaks.
pub fn run(cfg: &RunConfig, plan: &Plan, scope: &str) -> RunResult {
    let _ = std::fs::create_dir_all(&cfg.dir);
    let mut base_env = cfg.env.clone();
    base_env.push(("CHAOS_DIR".into(), cfg.dir.to_string_lossy().into()));
    base_env.push(("CHAOS_SEED".into(), plan.seed.to_string()));
    let baseline = collect(scope, "before", &cfg.sources);
    let start = Instant::now();
    let results: Arc<Mutex<Vec<StepResult>>> = Arc::default();
    let mut threads = Vec::new();
    let mut applied = Vec::new();
    let mut injected = Vec::new();
    let mut fault_errors = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        let due = Duration::from_millis((step.at_ms as f64 / cfg.speedup.max(0.001)) as u64);
        if let Some(wait) = due.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
        match step.kind {
            StepKind::Op => {
                let (cfg2, plan2, step2, env2, res) = (
                    cfg.clone(),
                    plan.clone(),
                    step.clone(),
                    base_env.clone(),
                    results.clone(),
                );
                threads.push(std::thread::spawn(move || {
                    let r = exec_op(&cfg2, &plan2, i, &step2, &env2);
                    res.lock().expect("results").push(r);
                }));
            }
            StepKind::Fault => match cfg.faults.apply(&step.name, &step.args, &base_env) {
                Ok(a) => {
                    injected.push(format!("{}@{}ms", step.name, step.at_ms));
                    applied.push(a);
                }
                Err(e) => fault_errors.push(e),
            },
        }
    }
    for t in threads {
        let _ = t.join();
    }
    // Undo every fault, newest first, whatever happened.
    for a in applied.iter().rev() {
        if let Some(u) = &a.undo {
            let (ok, out) = run_sh(u, &base_env, Duration::from_secs(60));
            if !ok {
                fault_errors.push(format!("undo of {} failed: {}", a.name, out.trim()));
            }
        }
    }
    if let Some(c) = &cfg.cleanup_cmd {
        let (ok, out) = run_sh(c, &base_env, Duration::from_secs(600));
        if !ok {
            fault_errors.push(format!("cleanup failed: {}", out.trim()));
        }
    }
    let settle_start = Instant::now();
    let report = loop {
        let wanted: Wanted = cfg
            .wanted_cmd
            .as_ref()
            .and_then(|c| {
                let (ok, out) = run_sh(c, &base_env, Duration::from_secs(60));
                ok.then(|| serde_json::from_str(&out).ok()).flatten()
            })
            .unwrap_or_default();
        let after = collect(scope, "after", &cfg.sources);
        let r = after.leaks(&baseline, &wanted);
        if r.is_clean() || settle_start.elapsed() >= cfg.settle {
            break r;
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    let mut steps = results.lock().expect("results").clone();
    steps.sort_by_key(|s| s.index);
    RunResult {
        seed: plan.seed,
        profile: plan.profile.clone(),
        digest: plan.digest(),
        steps,
        faults_injected: injected,
        fault_errors,
        report,
        settle_ms: settle_start.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::Source;
    use crate::inject::FaultCmd;
    use crate::plan::Profile;

    fn cfg(dir: &std::path::Path, driver: &str) -> RunConfig {
        let mut faults = FaultTable::default();
        faults.faults.insert(
            "gateway_kill".into(),
            FaultCmd {
                apply: "true".into(),
                undo: String::new(),
            },
        );
        RunConfig {
            dir: dir.join("run"),
            driver: vec!["sh".into(), "-c".into(), driver.into()],
            faults,
            sources: Sources {
                sources: vec![Source::Dir {
                    kind: "state".into(),
                    dir: dir.join("state").to_string_lossy().into(),
                    depth: 1,
                    exclude: vec![],
                }],
            },
            wanted_cmd: None,
            cleanup_cmd: None,
            settle: Duration::from_millis(600),
            step_timeout: Duration::from_secs(10),
            speedup: 100.0,
            env: vec![],
        }
    }

    fn small_plan(seed: u64) -> Plan {
        let mut p = Profile::preset("sandbox").unwrap();
        p.actors = 2;
        p.ops_per_actor = 3;
        p.plan(seed)
    }

    #[test]
    fn a_clean_driver_passes_and_a_leaky_one_is_caught_with_its_seed() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("state")).unwrap();
        let state = d.path().join("state");
        // The driver creates a file per create and removes it on delete.
        let driver = format!(
            "S=\"{}\"; case \"$CHAOS_STEP\" in *'\"create\"'*) touch \"$S/c$CHAOS_ACTOR\";; esac; true",
            state.display()
        );
        let mut c = cfg(d.path(), &driver);
        c.cleanup_cmd = Some(format!("rm -f {}/c*", state.display()));
        let ok = run(&c, &small_plan(3), "t");
        assert!(ok.report.is_clean(), "{}", ok.report);
        assert_eq!(ok.steps.len(), 6);
        // Without the cleanup the creates leak.
        let mut leaky = cfg(d.path(), &driver);
        leaky.cleanup_cmd = None;
        let r = run(&leaky, &small_plan(3), "t");
        assert!(!r.report.is_clean());
        assert!(r.report.assert_clean("seed 3").is_err());
        assert_eq!(r.seed, 3);
    }

    #[test]
    fn kill_after_ms_sigkills_the_client_mid_operation() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("state")).unwrap();
        let c = cfg(d.path(), "sleep 5");
        let mut plan = small_plan(1);
        plan.steps.retain(|s| s.kind == StepKind::Op);
        plan.steps.truncate(1);
        plan.steps[0]
            .args
            .insert("kill_after_ms".into(), serde_json::json!(50));
        let r = run(&c, &plan, "t");
        assert_eq!(r.killed(), 1);
    }

    #[test]
    fn faults_are_applied_and_undone() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("state")).unwrap();
        let mut c = cfg(d.path(), "true");
        let marker = d.path().join("undone");
        c.faults.faults.insert(
            "gateway_kill".into(),
            FaultCmd {
                apply: "true".into(),
                undo: format!("touch {}", marker.display()),
            },
        );
        let plan = Plan {
            seed: 9,
            profile: "t".into(),
            steps: vec![Step {
                at_ms: 0,
                actor: 0,
                kind: StepKind::Fault,
                name: "gateway_kill".into(),
                args: Default::default(),
            }],
        };
        let r = run(&c, &plan, "t");
        assert_eq!(r.faults_injected.len(), 1);
        assert!(marker.exists());
    }
}
