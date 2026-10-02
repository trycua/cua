//! `cua doctor parity A B`: the same image as two variants (a rootfs under
//! docker/gVisor and a containerDisk under QEMU/KubeVirt) must behave the
//! same for a task.
//!
//! 1. The doctor passes on both (otherwise parity is `blocked`: a failure
//!    here would mean breakage, not fidelity).
//! 2. Fidelity diff: every key of the two reports' `fidelity` blocks is
//!    `identical`, an `expected` difference (the image manifest's
//!    `parity.expected_diff`, plus `--allow-diff`), or an `unexpected`
//!    difference, which fails.
//! 3. Task parity: each task runs in three modes on both variants:
//!    `oracle` (scripted solve, expect the top score), `null` (no actions,
//!    expect the bottom score; catches evaluators that pass trivially on
//!    one variant) and `partial` (a fixed partial solve). The evaluator's
//!    score must match across variants in every mode.
//!
//! Tasks implement [`Task`]; two built-ins exercise the path now
//! (`builtin:form` drives a GUI fixture through ComputerService and
//! AccessibilityService, `builtin:file` works through ProcessService and
//! FilesystemService). Benchmark tasks plug in behind the same trait.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use clap::Args;
use cua_sdk::{Cua, CuaError};
use cua_spacesd_client::diagnose::Report;
use cua_spacesd_client::manifest::Loaded;
use cua_spacesd_client::{Command, SpacesdClient, UploadOptions, pb};
use serde::Serialize;
use serde_json::{Value, json};

/// `cua doctor parity` arguments.
#[derive(Args, Debug, Clone)]
pub struct ParityArgs {
    /// First variant (a sandbox ref or a spacesd URL).
    pub a: String,
    /// Second variant.
    pub b: String,
    /// Token for URL targets (both), default CUA_ENV_TOKEN.
    #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    /// Token for B when it differs from A's.
    #[arg(long)]
    pub token_b: Option<String>,
    /// Tasks (`builtin:form`, `builtin:file`), comma separated.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "builtin:form,builtin:file"
    )]
    pub tasks: Vec<String>,
    /// Extra fidelity keys allowed to differ.
    #[arg(long, value_delimiter = ',')]
    pub allow_diff: Vec<String>,
    /// Write parity.json here.
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,
    /// Doctor budget per variant, seconds.
    #[arg(long, default_value_t = 240)]
    pub timeout: u64,
}

/// One task run's outcome.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Outcome {
    /// Evaluator score, 0 to 1.
    pub score: f64,
    /// What the evaluator saw.
    pub debug: Value,
}

/// A parity task.
#[async_trait::async_trait]
pub trait Task: Send + Sync {
    /// Task id.
    fn id(&self) -> &str;
    /// Expected score per mode ("oracle", "null", "partial").
    fn expected(&self, mode: &str) -> f64;
    /// Prepares the guest; returns a per-run state string.
    async fn setup(
        &self,
        guest: &SpacesdClient,
        manifest: &Loaded,
        nonce: &str,
    ) -> Result<Value, String>;
    /// Acts in `mode` ("null" does nothing).
    async fn solve(&self, guest: &SpacesdClient, state: &Value, mode: &str) -> Result<(), String>;
    /// Scores the guest.
    async fn evaluate(&self, guest: &SpacesdClient, state: &Value) -> Result<Outcome, String>;
    /// Removes what setup created.
    async fn teardown(&self, guest: &SpacesdClient, state: &Value);
}

/// Built-in task by name.
pub fn builtin(name: &str) -> Option<Box<dyn Task>> {
    match name {
        "builtin:form" => Some(Box::new(FormTask)),
        "builtin:file" => Some(Box::new(FileTask)),
        _ => None,
    }
}

const MODES: [&str; 3] = ["oracle", "null", "partial"];

async fn sh(guest: &SpacesdClient, line: &str) -> Result<String, String> {
    let out = guest
        .run(
            Command::new("/bin/sh")
                .args(["-c", line])
                .timeout(Duration::from_secs(30)),
        )
        .await
        .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err(format!(
            "`{line}` exited {:?}: {}",
            out.status.code,
            out.stderr_str().trim()
        ));
    }
    Ok(out.stdout_str())
}

/// `builtin:file`: write "cua-parity" to a file. Oracle writes it, partial
/// writes the wrong content, null writes nothing. Score: 1 exact, 0.5 wrong
/// content, 0 missing.
struct FileTask;

#[async_trait::async_trait]
impl Task for FileTask {
    fn id(&self) -> &str {
        "builtin:file"
    }

    fn expected(&self, mode: &str) -> f64 {
        match mode {
            "oracle" => 1.0,
            "partial" => 0.5,
            _ => 0.0,
        }
    }

    async fn setup(
        &self,
        guest: &SpacesdClient,
        _manifest: &Loaded,
        nonce: &str,
    ) -> Result<Value, String> {
        let dir = format!("/tmp/cua-parity-{nonce}");
        guest.make_dir(&dir).await.map_err(|e| e.to_string())?;
        Ok(json!({"dir": dir, "path": format!("{dir}/answer.txt")}))
    }

    async fn solve(&self, guest: &SpacesdClient, state: &Value, mode: &str) -> Result<(), String> {
        let path = state["path"].as_str().unwrap_or_default();
        let body: &[u8] = match mode {
            "oracle" => b"cua-parity",
            "partial" => b"cua-par",
            _ => return Ok(()),
        };
        guest
            .upload(path, body, UploadOptions::default())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn evaluate(&self, guest: &SpacesdClient, state: &Value) -> Result<Outcome, String> {
        let path = state["path"].as_str().unwrap_or_default();
        let content = sh(guest, &format!("cat '{path}' 2>/dev/null || true")).await?;
        let score = match content.as_str() {
            "cua-parity" => 1.0,
            "" => 0.0,
            _ => 0.5,
        };
        Ok(Outcome {
            score,
            debug: json!({"content": content}),
        })
    }

    async fn teardown(&self, guest: &SpacesdClient, state: &Value) {
        if let Some(dir) = state["dir"].as_str() {
            let _ = guest.remove(dir, true).await;
        }
    }
}

/// `builtin:form`: put "cua-parity" in the form fixture's Name field and
/// press Submit. Oracle types and submits (through ComputerService and
/// AccessibilityService, so cua-driver), partial types without submitting,
/// null does nothing. Score: 1 submitted with the right name, 0.5 typed but
/// not submitted, 0 otherwise. Needs the image's form fixture.
struct FormTask;

impl FormTask {
    async fn events(guest: &SpacesdClient, log: &str) -> Vec<Value> {
        match guest.download(log).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes)
                .lines()
                .take(20_000)
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    async fn window(guest: &SpacesdClient, title: &str) -> Option<pb::WindowInfo> {
        for _ in 0..100 {
            if let Ok(r) = guest
                .windows()
                .list_windows(pb::ListWindowsRequest {
                    filter: Some(pb::WindowFilter {
                        title_contains: title.into(),
                        ..Default::default()
                    }),
                })
                .await
                && let Some(w) = r.into_inner().windows.into_iter().next()
            {
                return Some(w);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        None
    }

    async fn find(
        guest: &SpacesdClient,
        window: &pb::WindowRef,
        name: &str,
    ) -> Option<(String, pb::AccessibilityNode)> {
        let found = guest
            .accessibility()
            .find(pb::FindRequest {
                window: Some(window.clone()),
                query: Some(pb::AccessibilityQuery {
                    name: name.into(),
                    ..Default::default()
                }),
                max_results: 5,
            })
            .await
            .ok()?
            .into_inner();
        let snapshot = found.snapshot_id.clone();
        found.nodes.into_iter().next().map(|n| (snapshot, n))
    }
}

#[async_trait::async_trait]
impl Task for FormTask {
    fn id(&self) -> &str {
        "builtin:form"
    }

    fn expected(&self, mode: &str) -> f64 {
        match mode {
            "oracle" => 1.0,
            "partial" => 0.5,
            _ => 0.0,
        }
    }

    async fn setup(
        &self,
        guest: &SpacesdClient,
        manifest: &Loaded,
        nonce: &str,
    ) -> Result<Value, String> {
        if !manifest.manifest.fixtures.has("form") {
            return Err("the image ships no form fixture".into());
        }
        let dir = format!("/tmp/cua-parity-{nonce}-fixtures");
        let title = format!("CUA Parity form {nonce}");
        let tag = format!("cua-parity-{nonce}-form");
        let cmd = Command::new("python3")
            .arg(format!("{}/form.py", manifest.manifest.fixtures.root))
            .env("CUA_FIXTURE_NAME", "parity-form")
            .env("CUA_FIXTURE_LOG_DIR", &dir)
            .env("CUA_FORM_TITLE", &title)
            .tag(&tag);
        let handle = guest.spawn(cmd).await.map_err(|e| e.to_string())?;
        handle.detach();
        let log = format!("{dir}/parity-form.jsonl");
        for _ in 0..100 {
            if Self::events(guest, &log)
                .await
                .iter()
                .any(|e| e["type"] == "ready")
            {
                let window = Self::window(guest, &title)
                    .await
                    .ok_or("form window never listed")?;
                return Ok(json!({"dir": dir, "log": log, "tag": tag, "title": title,
                    "window": {"id": window.r#ref.as_ref().map(|r| r.id.clone()), "epoch": window.r#ref.as_ref().map(|r| r.epoch)}}));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Err("form fixture never reported ready".into())
    }

    async fn solve(&self, guest: &SpacesdClient, state: &Value, mode: &str) -> Result<(), String> {
        if mode == "null" {
            return Ok(());
        }
        let window = pb::WindowRef {
            id: state["window"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            epoch: state["window"]["epoch"].as_u64().unwrap_or(0),
        };
        let _ = guest
            .windows()
            .activate_window(pb::ActivateWindowRequest {
                window: Some(window.clone()),
            })
            .await;
        let (_, entry) = Self::find(guest, &window, "Name")
            .await
            .ok_or("Name field not in the accessibility tree")?;
        let b = entry.bounds.unwrap_or_default();
        guest
            .click(b.x + b.width / 2.0, b.y + b.height / 2.0)
            .await
            .map_err(|e| e.to_string())?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        guest
            .type_text("cua-parity")
            .await
            .map_err(|e| e.to_string())?;
        if mode == "oracle" {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let (snapshot, submit) = Self::find(guest, &window, "Submit")
                .await
                .ok_or("Submit not in the accessibility tree")?;
            guest
                .accessibility()
                .act(pb::ActRequest {
                    element: Some(pb::ElementRef {
                        snapshot_id: snapshot,
                        element_id: submit.element_id,
                    }),
                    action: pb::AccessibilityAction::Press as i32,
                    ..Default::default()
                })
                .await
                .map_err(|s| s.message().to_owned())?;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    }

    async fn evaluate(&self, guest: &SpacesdClient, state: &Value) -> Result<Outcome, String> {
        let events = Self::events(guest, state["log"].as_str().unwrap_or_default()).await;
        let submitted = events
            .iter()
            .any(|e| e["type"] == "submit" && e["name"] == "cua-parity");
        let typed = events
            .iter()
            .rev()
            .find(|e| e["type"] == "entry_changed")
            .map(|e| e["text"].clone())
            .unwrap_or(Value::Null);
        let score = if submitted {
            1.0
        } else if typed == "cua-parity" {
            0.5
        } else {
            0.0
        };
        Ok(Outcome {
            score,
            debug: json!({"submitted": submitted, "last_text": typed, "events": events.len()}),
        })
    }

    async fn teardown(&self, guest: &SpacesdClient, state: &Value) {
        if let Some(tag) = state["tag"].as_str() {
            let _ = guest
                .process()
                .signal_process(pb::SignalProcessRequest {
                    process: Some(pb::ProcessSelector {
                        selector: Some(pb::process_selector::Selector::Tag(tag.to_owned())),
                    }),
                    signal: pb::Signal::Kill as i32,
                    process_group: true,
                })
                .await;
        }
        if let Some(dir) = state["dir"].as_str() {
            let _ = guest.remove(dir, true).await;
        }
    }
}

/// A fidelity key comparison.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FidelityDiff {
    /// Key.
    pub key: String,
    /// Value on A.
    pub a: String,
    /// Value on B.
    pub b: String,
    /// `identical`, `expected` or `unexpected`.
    pub class: String,
}

/// Classifies every fidelity key.
pub fn diff_fidelity(a: &Report, b: &Report, allowed: &[String]) -> Vec<FidelityDiff> {
    let (fa, fb) = (a.fidelity.flatten(), b.fidelity.flatten());
    let keys: std::collections::BTreeSet<&String> = fa.keys().chain(fb.keys()).collect();
    keys.into_iter()
        .map(|key| {
            let (va, vb) = (
                fa.get(key).cloned().unwrap_or_default(),
                fb.get(key).cloned().unwrap_or_default(),
            );
            let top = key.split('.').next().unwrap_or(key);
            let class = if va == vb {
                "identical"
            } else if allowed.iter().any(|k| k == key || k == top) {
                "expected"
            } else {
                "unexpected"
            };
            FidelityDiff {
                key: key.clone(),
                a: va,
                b: vb,
                class: class.into(),
            }
        })
        .collect()
}

async fn connect(
    cua: &std::sync::Arc<Cua>,
    target: &str,
    token: Option<String>,
) -> Result<SpacesdClient, CuaError> {
    let sdk = if target.starts_with("http://") || target.starts_with("https://") {
        cua.spacesd(target.to_owned(), token).await?
    } else {
        crate::sandbox::env_of(cua, target).await?
    };
    Ok(sdk.inner().clone())
}

async fn manifest_of(guest: &SpacesdClient) -> Loaded {
    match guest
        .download(cua_spacesd_client::manifest::DEFAULT_PATH)
        .await
    {
        Ok(bytes) => Loaded::parse(&bytes, cua_spacesd_client::manifest::DEFAULT_PATH),
        Err(_) => Loaded::default(),
    }
}

async fn run_task(
    task: &dyn Task,
    guest: &SpacesdClient,
    manifest: &Loaded,
    mode: &str,
) -> Result<Outcome, String> {
    let nonce = format!(
        "{:x}",
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            % 0xffff_ffff
    );
    let state = task.setup(guest, manifest, &nonce).await?;
    let result = async {
        task.solve(guest, &state, mode).await?;
        task.evaluate(guest, &state).await
    }
    .await;
    task.teardown(guest, &state).await;
    result
}

/// Runs parity; returns (parity JSON, pass).
pub async fn run(cua: &std::sync::Arc<Cua>, args: &ParityArgs) -> Result<(Value, bool), CuaError> {
    let a = connect(cua, &args.a, args.token.clone()).await?;
    let b = connect(cua, &args.b, args.token_b.clone().or(args.token.clone())).await?;
    let options = pb::DiagnoseOptions {
        effects: pb::DiagnoseEffects::VirtualOnly as i32,
        timeout: Some(pbjson_types::Duration {
            seconds: args.timeout as i64,
            nanos: 0,
        }),
        ..Default::default()
    };
    // One after the other, like the tasks below: both variants usually share
    // one host, and the doctor's timing checks (A/V sync, input latency)
    // fail on a host that is running two doctors at once.
    let ra = a
        .diagnose_report(options.clone(), |_| {})
        .await
        .map_err(CuaError::from)?;
    let rb = b
        .diagnose_report(options, |_| {})
        .await
        .map_err(CuaError::from)?;
    let doctor_ok = ra.summary.status != cua_spacesd_client::diagnose::Status::Fail
        && rb.summary.status != cua_spacesd_client::diagnose::Status::Fail;
    let (ma, mb) = (manifest_of(&a).await, manifest_of(&b).await);
    let mut allowed: Vec<String> = ma.manifest.parity.expected_diff.clone();
    allowed.extend(mb.manifest.parity.expected_diff.iter().cloned());
    allowed.extend(args.allow_diff.iter().cloned());
    let fidelity = diff_fidelity(&ra, &rb, &allowed);
    let unexpected = fidelity.iter().filter(|d| d.class == "unexpected").count();

    let mut tasks = Vec::new();
    let mut tasks_ok = true;
    if doctor_ok {
        for name in &args.tasks {
            let Some(task) = builtin(name) else {
                return Err(CuaError::InvalidArgument(format!(
                    "unknown task {name:?} (builtin:form, builtin:file)"
                )));
            };
            for mode in MODES {
                // Variants run one after the other: tasks drive a desktop.
                let oa = run_task(task.as_ref(), &a, &ma, mode).await;
                let ob = run_task(task.as_ref(), &b, &mb, mode).await;
                let expected = task.expected(mode);
                let (sa, sb) = (
                    oa.as_ref().map(|o| o.score).ok(),
                    ob.as_ref().map(|o| o.score).ok(),
                );
                let matched = sa.is_some() && sa == sb;
                let as_expected = sa == Some(expected) && sb == Some(expected);
                tasks_ok &= matched && as_expected;
                let side = |o: &Result<Outcome, String>| match o {
                    Ok(o) => json!({"score": o.score, "debug": o.debug}),
                    Err(e) => json!({"error": e}),
                };
                tasks.push(json!({
                    "task": task.id(), "mode": mode, "expected": expected,
                    "a": side(&oa), "b": side(&ob), "match": matched, "ok": matched && as_expected,
                }));
            }
        }
    }
    let status = if !doctor_ok {
        "blocked"
    } else if unexpected == 0 && tasks_ok {
        "pass"
    } else {
        "fail"
    };
    let side = |target: &str, r: &Report| {
        json!({"target": target, "doctor": r.summary.status.as_str(), "runtime": r.environment.runtime,
               "variant": r.image.variant, "image": r.image.name, "failed_checks": failed_checks(r)})
    };
    let result = json!({
        "schema_version": 1,
        "status": status,
        "a": side(&args.a, &ra),
        "b": side(&args.b, &rb),
        "fidelity": fidelity,
        "tasks": tasks,
    });
    Ok((result, status == "pass"))
}

/// The doctor checks that failed, `id: message`, so a blocked parity names
/// what blocked it.
fn failed_checks(r: &Report) -> Vec<String> {
    r.checks
        .iter()
        .filter(|c| c.status == cua_spacesd_client::diagnose::Status::Fail)
        .map(|c| format!("{}: {}", c.id, c.message))
        .collect()
}

/// Markdown for a job summary.
pub fn markdown(result: &Value) -> String {
    let mut out = format!(
        "### Eval parity: **{}**\n\nA: {} ({} {}, doctor {}); B: {} ({} {}, doctor {})\n\n",
        result["status"].as_str().unwrap_or("?"),
        result["a"]["target"],
        result["a"]["variant"],
        result["a"]["runtime"],
        result["a"]["doctor"],
        result["b"]["target"],
        result["b"]["variant"],
        result["b"]["runtime"],
        result["b"]["doctor"],
    );
    for (label, side) in [("A", &result["a"]), ("B", &result["b"])] {
        let failed: Vec<&str> = side["failed_checks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !failed.is_empty() {
            out.push_str(&format!("#### Doctor failures on {label}\n\n"));
            for f in failed {
                out.push_str(&format!("- {f}\n"));
            }
            out.push('\n');
        }
    }
    let rows = |class: &str| -> Vec<String> {
        result["fidelity"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|d| d["class"] == class)
                    .map(|d| {
                        format!(
                            "| {} | {} | {} |",
                            d["key"].as_str().unwrap_or(""),
                            d["a"].as_str().unwrap_or(""),
                            d["b"].as_str().unwrap_or("")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let blocking = rows("unexpected");
    out.push_str("#### Blocking\n\n| task | mode | expected | A | B |\n|---|---|---|---|---|\n");
    for t in result["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["ok"] != true)
    {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            t["task"], t["mode"], t["expected"], t["a"], t["b"]
        ));
    }
    if !blocking.is_empty() {
        out.push_str("\nUnexpected fidelity differences:\n\n| key | A | B |\n|---|---|---|\n");
        out.push_str(&(blocking.join("\n") + "\n"));
    }
    out.push_str("\n#### Expected differences\n\n| key | A | B |\n|---|---|---|\n");
    out.push_str(&(rows("expected").join("\n") + "\n"));
    let mut by_task: BTreeMap<String, usize> = BTreeMap::new();
    for t in result["tasks"].as_array().into_iter().flatten() {
        *by_task
            .entry(t["task"].as_str().unwrap_or("").to_owned())
            .or_default() += 1;
    }
    out.push_str(&format!(
        "\n{} task runs across {} tasks\n",
        result["tasks"].as_array().map(|a| a.len()).unwrap_or(0),
        by_task.len()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_spacesd_client::diagnose::Fidelity;

    fn report(kernel: &str, tz: &str) -> Report {
        Report {
            fidelity: Fidelity {
                kernel: kernel.into(),
                tz: tz.into(),
                ..Fidelity::default()
            },
            ..Report::default()
        }
    }

    #[test]
    fn fidelity_keys_are_classified() {
        let diffs = diff_fidelity(
            &report("6.8", "UTC"),
            &report("4.19-gvisor", "Europe/Paris"),
            &["kernel".into()],
        );
        let class = |k: &str| diffs.iter().find(|d| d.key == k).unwrap().class.clone();
        assert_eq!(class("kernel"), "expected");
        assert_eq!(class("tz"), "unexpected");
        assert_eq!(class("display"), "identical");
    }

    #[test]
    fn tasks_are_registered() {
        assert_eq!(builtin("builtin:form").unwrap().expected("oracle"), 1.0);
        assert_eq!(builtin("builtin:file").unwrap().expected("null"), 0.0);
        assert!(builtin("osworld:x").is_none());
    }

    #[test]
    fn markdown_groups_sections() {
        let md = markdown(&json!({"status": "fail", "a": {}, "b": {},
            "fidelity": [{"key": "tz", "a": "UTC", "b": "X", "class": "unexpected"},
                         {"key": "kernel", "a": "1", "b": "2", "class": "expected"}],
            "tasks": [{"task": "builtin:file", "mode": "null", "expected": 0.0, "a": {"score": 0.0}, "b": {"score": 1.0}, "ok": false}]}));
        assert!(md.contains("#### Blocking"));
        assert!(md.contains("| tz | UTC | X |"));
        assert!(md.contains("| kernel | 1 | 2 |"));
    }

    #[test]
    fn markdown_names_the_doctor_failures_that_block() {
        let md = markdown(&json!({"status": "blocked", "a": {"failed_checks": []},
            "b": {"failed_checks": ["desktop.display: no X display"]},
            "fidelity": [], "tasks": []}));
        assert!(md.contains("#### Doctor failures on B\n\n- desktop.display: no X display\n"));
        assert!(!md.contains("Doctor failures on A"));
    }
}
