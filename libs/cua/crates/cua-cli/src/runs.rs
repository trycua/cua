// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua sb run` and `cua sb runs`: a command in fresh Spaces, `--count`
//! times, on this machine (the default) or on all your machines (`--on
//! auto`), and the runs so far.
//!
//! The daemon answers them (`SpaceService.PoolRequest`): runs on this
//! machine go to its own coordinator; `--on auto` and `--on <machine>` go
//! to whichever of your signed-in machines coordinates them, which keeps
//! going when this one sleeps. Coordination ships with Cua Spaces; an MIT
//! build of the daemon answers that it does not have it.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use clap::Args;
use cua_sdk::CuaError;
use serde_json::{Value, json};

use crate::util::{hint_line, line};

/// `cua sb run`.
#[derive(Args, Debug)]
pub struct RunArgs {
    /// Where: omitted or `local` (this machine), `auto` (the least-loaded
    /// of your machines that provide Spaces), or one machine by name.
    #[arg(long)]
    pub on: Option<String>,
    /// How many times to run it, each in its own new Space.
    #[arg(long, default_value_t = 1)]
    pub count: u32,
    /// Image of each Space (default: the canonical Linux image).
    #[arg(long)]
    pub image: Option<String>,
    /// `container`, `vm` or `microvm` (default: what the image is).
    #[arg(long)]
    pub kind: Option<String>,
    /// A runtime (`gvisor`, `runc`, `firecracker`, ...).
    #[arg(long)]
    pub runtime: Option<String>,
    /// CPUs reserved for each.
    #[arg(long, default_value_t = 1.0)]
    pub cpus: f64,
    /// Memory reserved for each, in MiB.
    #[arg(long, default_value_t = 1024)]
    pub memory: u64,
    /// How long each may run.
    #[arg(long, default_value = "10m")]
    pub timeout: humantime::Duration,
    /// `KEY=VALUE` for the command's environment.
    #[arg(long = "env", short = 'e')]
    pub env: Vec<String>,
    /// A file each run leaves behind, returned with its result.
    #[arg(long)]
    pub result: Option<String>,
    /// Most at once (default: as many as the machines take).
    #[arg(long, default_value_t = 0)]
    pub concurrency: u32,
    /// Times a job may be lost (its machine went away) and run again.
    #[arg(long, default_value_t = 0)]
    pub max_attempts: u32,
    /// Only machines of this OS (`linux`, `macos`).
    #[arg(long)]
    pub os: Option<String>,
    /// Only machines of this CPU architecture (`x86_64`, `arm64`).
    #[arg(long)]
    pub arch: Option<String>,
    /// The run's id (default: a new one). Submitting the same id again
    /// adds nothing twice.
    #[arg(long)]
    pub id: Option<String>,
    /// Submit and print the run's id without waiting.
    #[arg(long)]
    pub detach: bool,
    /// The command (after `--`).
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// `cua sb runs`.
#[derive(Args, Debug)]
pub struct RunsArgs {
    /// A run: its jobs.
    pub run: Option<String>,
    /// One job of the run, with its output.
    #[arg(long)]
    pub job: Option<i64>,
    /// Only jobs in this state (`failed`, `error`, `running`, ...).
    #[arg(long)]
    pub state: Option<String>,
    /// Cancel the run (or the job with `--job`).
    #[arg(long)]
    pub cancel: bool,
    /// With a run and `--json`: each job's output too.
    #[arg(long)]
    pub output: bool,
    /// Runs of this machine only (`local`) or of all your machines
    /// (`auto`, the default).
    #[arg(long)]
    pub on: Option<String>,
}

/// `--on <cloud>` for a batch run or job: the pool placement
/// `cloud:<provider>` for a cloud that supplies pool hosts (`cua cloud
/// hosts`), an error for any other cloud name, `None` for no cloud's name.
pub fn cloud_on(on: &str) -> Option<Result<String, CuaError>> {
    let on = on.trim().to_ascii_lowercase();
    match on.as_str() {
        "aws" | "gcp" | "hetzner" => Some(Ok(format!("cloud:{on}"))),
        "azure" | "modal" | "cloud" => Some(Err(CuaError::InvalidArgument(format!(
            "--on {on}: {}; use --on aws, gcp or hetzner (linked with `cua cloud connect`), \
             --on auto, or --on host:<pool host>",
            if on == "cloud" {
                "name the cloud".to_string()
            } else {
                format!("{on} does not supply pool hosts that run batch jobs")
            }
        )))),
        _ => None,
    }
}

/// `local` or `pool`, from `--on` (omitted: local).
pub fn target_of(on: Option<&str>) -> Result<(&'static str, String), CuaError> {
    let on = on.map(str::trim).unwrap_or("");
    if let Some(c) = cloud_on(on) {
        return Ok(("pool", c?));
    }
    let on = on.strip_prefix("host:").unwrap_or(on);
    match on.to_ascii_lowercase().as_str() {
        "" | "local" => Ok(("local", "local".into())),
        "auto" => Ok(("pool", String::new())),
        _ if on.contains(':') => Err(CuaError::InvalidArgument(format!(
            "--on {on}: use local, auto, a cloud (aws, gcp, hetzner) or a machine's name"
        ))),
        _ => Ok(("pool", on.to_string())),
    }
}

/// A run id says where its coordinator is: `l-` this machine, `p-` the
/// account's machines.
pub fn target_of_run(run: &str) -> &'static str {
    if run.starts_with("l-") {
        "local"
    } else {
        "pool"
    }
}

/// One request to the daemon's coordinators.
pub async fn request(target: &str, method: &str, params: Value) -> Result<Value, CuaError> {
    let d = cua_daemon::client::ensure_daemon()
        .await
        .map_err(|e| CuaError::Internal(format!("the cua daemon: {e}")))?;
    let r = d
        .spaces()
        .pool_request(cua_proto::daemon::v1::PoolRequestRequest {
            target: target.into(),
            method: method.into(),
            params_json: params.to_string(),
        })
        .await
        .map_err(|s| match s.code() {
            tonic::Code::Unimplemented => CuaError::InvalidArgument(
                "batch runs on your machines ship with Cua Spaces; this cua daemon does not have them"
                    .into(),
            ),
            _ => CuaError::Internal(s.message().to_string()),
        })?
        .into_inner();
    let body: Value = serde_json::from_str(&r.body_json).unwrap_or(Value::Null);
    if r.status == 200 {
        return Ok(body);
    }
    let msg = body["error"].as_str().unwrap_or("failed").to_string();
    Err(match r.status {
        400 => CuaError::InvalidArgument(msg),
        404 => CuaError::NotFound(msg),
        401 => CuaError::Unauthenticated(msg),
        403 => CuaError::PermissionDenied(msg),
        503 | 504 => CuaError::Timeout(msg),
        _ => CuaError::Internal(msg),
    })
}

fn env_map(env: &[String]) -> Result<serde_json::Map<String, Value>, CuaError> {
    env.iter()
        .map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                .ok_or_else(|| CuaError::InvalidArgument(format!("--env {kv}: use KEY=VALUE")))
        })
        .collect()
}

/// `cua sb run`.
pub async fn run(a: RunArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let (target, on) = target_of(a.on.as_deref())?;
    if a.count == 0 {
        return Err(CuaError::InvalidArgument("--count is at least 1".into()));
    }
    let run = a.id.clone().unwrap_or_else(|| {
        format!(
            "{}-{:08x}",
            if target == "local" { "l" } else { "p" },
            rand::random::<u32>()
        )
    });
    if target_of_run(&run) != target {
        return Err(CuaError::InvalidArgument(format!(
            "--id {run}: a run on this machine starts with l-, one on your machines with p-"
        )));
    }
    let params = json!({
        "run": run,
        "command": a.command,
        "count": a.count,
        "image": a.image.unwrap_or_default(),
        "kind": a.kind.unwrap_or_default(),
        "runtime": a.runtime.unwrap_or_default(),
        "cpus": a.cpus,
        "memory_mb": a.memory,
        "timeout_secs": a.timeout.as_secs(),
        "env": env_map(&a.env)?,
        "result_path": a.result.unwrap_or_default(),
        "os": a.os.unwrap_or_default(),
        "arch": a.arch.unwrap_or_default(),
        "on": on,
        "concurrency": a.concurrency,
        "max_attempts": a.max_attempts,
    });
    let started = Instant::now();
    request(target, "submit", params).await?;
    let count = a.count;
    if a.detach {
        if json {
            line(out, json!({"run": run, "jobs": count}).to_string());
        } else {
            hint_line(
                out,
                format!(
                    "{run}: {count} job{} submitted (cua sb runs {run})",
                    if count == 1 { "" } else { "s" }
                ),
            );
        }
        return Ok(0);
    }
    let tty = std::io::stderr().is_terminal() && !json;
    let summary = loop {
        let s = match request(target, "run", json!({"run": run})).await {
            Ok(s) => s,
            // The coordinating machine changed: ask again.
            Err(CuaError::Timeout(_)) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        if tty {
            let c = &s["counts"];
            let g = |k: &str| c[k].as_u64().unwrap_or(0);
            let done = g("succeeded") + g("failed") + g("error") + g("canceled");
            eprint!(
                "\r{run}: {done}/{count} done · {} running · {} waiting · {} failed   ",
                g("running") + g("dispatched"),
                g("queued"),
                g("failed") + g("error")
            );
        }
        if s["done"].as_bool() == Some(true) {
            if tty {
                eprintln!();
            }
            break s;
        }
        tokio::time::sleep(Duration::from_millis(if count == 1 { 250 } else { 1000 })).await;
    };
    let wall = started.elapsed();
    let c = &summary["counts"];
    let g = |k: &str| c[k].as_u64().unwrap_or(0);
    let ok = g("succeeded");
    if count == 1 {
        // One job: its output and exit code, as if it ran here.
        let jobs = request(
            target,
            "jobs",
            json!({"run": run, "output": true, "limit": 1}),
        )
        .await?;
        let j = &jobs["jobs"][0];
        if json {
            line(out, j.to_string());
        } else {
            let _ = out.write_all(j["stdout"].as_str().unwrap_or("").as_bytes());
            let err = j["stderr"].as_str().unwrap_or("");
            if !err.is_empty() {
                eprint!("{err}");
            }
            if j["state"] != "succeeded" && j["state"] != "failed" {
                eprintln!(
                    "{}: {}",
                    j["state"].as_str().unwrap_or(""),
                    j["error"].as_str().unwrap_or("")
                );
            }
        }
        return Ok(match j["state"].as_str() {
            Some("succeeded") => 0,
            Some("failed") => j["exit_code"].as_i64().map_or(1, |c| c as i32),
            _ => 1,
        });
    }
    if json {
        let mut s = summary.clone();
        s["wall_ms"] = json!(wall.as_millis() as u64);
        line(out, s.to_string());
    } else {
        line(
            out,
            format!(
                "{run}: {ok}/{count} succeeded, {} failed, {} errors in {:.1}s · Space start p50 {} ms · {} retries",
                g("failed"),
                g("error"),
                wall.as_secs_f64(),
                summary["create_p50_ms"],
                summary["retries"],
            ),
        );
        if ok < u64::from(count) {
            hint_line(
                out,
                format!("See what failed: cua sb runs {run} --state failed"),
            );
        }
    }
    Ok(if ok == u64::from(count) { 0 } else { 1 })
}

/// `cua computers`: the computers in your pool as the coordinator sees them
/// now (state, OS/arch, load, jobs, accelerators and $/h), with the queue
/// and who holds the keys the pool's data is sealed with.
pub async fn computers(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let status = request("pool", "status", Value::Null).await;
    // Which devices hold the keys the pool's data is sealed with (asked
    // even when the pool did not answer: it says why, and who can help).
    let pool = request("pool", "pool", Value::Null).await.ok();
    let keys = pool.as_ref().and_then(|p| p.get("keys").cloned());
    let mut v = match status {
        Ok(v) => v,
        Err(e) => {
            if let (Some(k), false) = (&keys, json) {
                for l in keys_lines(k, crate::onboard::stdout_columns()) {
                    line(out, l);
                }
            }
            return Err(e);
        }
    };
    if let Some(k) = keys {
        v["keys"] = k;
    }
    if let Some(c) = pool.as_ref().and_then(coordinator_of) {
        v["coordinator"] = c;
    }
    if json {
        line(out, v.to_string());
        return Ok(0);
    }
    let width = crate::onboard::stdout_columns();
    let rows = match width {
        Some(w) => crate::onboard::machines_table_fit(&v, w),
        None => crate::onboard::machines_table(&v),
    };
    for r in rows {
        line(out, r);
    }
    line(
        out,
        format!(
            "{} waiting · {} running",
            v["queued_in_memory"], v["in_flight"]
        ),
    );
    if let Some(c) = v.get("coordinator") {
        line(out, coordinator_line(c));
    }
    if let Some(k) = v.get("keys") {
        for l in keys_lines(k, width) {
            line(out, l);
        }
    }
    Ok(0)
}

/// Which machine coordinates the pool (the relay's lease holder), as the
/// pool agent's `pool` view says (`coordinator`).
fn coordinator_of(p: &Value) -> Option<Value> {
    p.get("coordinator").filter(|c| !c.is_null()).cloned()
}

fn coordinator_line(c: &Value) -> String {
    let name = c["name"].as_str().unwrap_or("?");
    let mut s = format!("Coordinated by {name}");
    if c["this_machine"].as_bool() == Some(true) {
        s.push_str(" (this machine)");
    }
    if c["older"].as_bool() == Some(true) {
        s.push_str(&format!(
            " · runs an older Cua: update Cua on {name} to run jobs with services or --on <cloud>"
        ));
    }
    s
}

/// What `cua computers` says of the pool's keys (the agent's `keys`
/// view): which devices hold them, of which at least one must be online
/// to add a new device.
fn keys_lines(k: &Value, width: Option<usize>) -> Vec<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let holders: Vec<String> = k["holders"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|h| {
                    let name = h["name"]
                        .as_str()
                        .filter(|n| !n.is_empty())
                        .or(h["device"].as_str())
                        .unwrap_or("?");
                    match h["last_seen"].as_u64() {
                        Some(t) => {
                            format!("{name} (last seen {})", seen_ago(now.saturating_sub(t)))
                        }
                        None => name.to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let with: Vec<&str> = k["shared_with"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    // The command is spelled with this build's CLI name, not rewritten with the
    // line: the device names in it are the user's.
    let cli = crate::util::cli();
    match k["waiting"].as_str() {
        Some(_) if !holders.is_empty() => {
            out.push(format!(
                "Not shared with this device yet. Your pool keys are on: {}; at least one must be online to add a new device (open Cua on one of them), or revoke the ones you no longer have (`{cli} devices revoke <id>`).",
                holders.join(", ")
            ));
        }
        Some(w) => out.push(format!("Sealed with your devices' keys: {w}")),
        None => {
            out.push(format!(
                "Sealed with your devices' keys (generation {}). Your pool keys are on: {}; at least one must be online to add a new device.",
                k["gen"],
                if holders.is_empty() {
                    with.join(", ")
                } else {
                    holders.join(", ")
                }
            ));
        }
    }
    out.iter().flat_map(|l| fit_line(l, width)).collect()
}

/// `just now`, `5 min ago`, `3 h ago`, `12 days ago`.
fn seen_ago(secs: u64) -> String {
    match secs {
        0..120 => "just now".into(),
        120..7200 => format!("{} min ago", secs / 60),
        7200..172_800 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// `s` as it fits a terminal `width` columns wide: broken at spaces into
/// lines of at most `width` (a word longer than that stays whole), so a
/// narrow terminal never wraps it mid-word. No width: one line.
fn fit_line(s: &str, width: Option<usize>) -> Vec<String> {
    let Some(w) = width.filter(|&w| s.chars().count() > w) else {
        return vec![s.to_string()];
    };
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split(' ') {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > w {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// `cua sb runs`.
pub async fn runs(a: RunsArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let target = match (&a.run, a.on.as_deref()) {
        (Some(r), _) => target_of_run(r),
        (None, on) => target_of(Some(on.unwrap_or("auto")))?.0,
    };
    let Some(run) = a.run else {
        let v = request(target, "runs", json!({"limit": 20})).await?;
        if json {
            line(out, v.to_string());
            return Ok(0);
        }
        line(
            out,
            format!(
                "{:<12}  {:>6}  {:>9}  {:>6}  {:>8}",
                "RUN", "JOBS", "SUCCEEDED", "FAILED", "RUNNING"
            ),
        );
        for r in v["runs"].as_array().cloned().unwrap_or_default() {
            let c = &r["counts"];
            let g = |k: &str| c[k].as_u64().unwrap_or(0);
            line(
                out,
                format!(
                    "{:<12}  {:>6}  {:>9}  {:>6}  {:>8}",
                    r["run"].as_str().unwrap_or(""),
                    r["total"],
                    g("succeeded"),
                    g("failed") + g("error"),
                    g("running") + g("dispatched") + g("queued"),
                ),
            );
        }
        return Ok(0);
    };
    if let Some(id) = a.job {
        if a.cancel {
            let v = request(target, "cancel_job", json!({"id": id})).await?;
            line(
                out,
                format!("job {id}: {}", v["state"].as_str().unwrap_or("")),
            );
            return Ok(0);
        }
        let j = request(target, "job", json!({"id": id})).await?;
        if json {
            line(out, j.to_string());
            return Ok(0);
        }
        line(
            out,
            format!(
                "job {id} · {} · on {} · attempt {} · exit {}",
                j["state"].as_str().unwrap_or(""),
                j["node"].as_str().unwrap_or("-"),
                j["attempt"],
                j["exit_code"]
            ),
        );
        for k in ["stdout", "stderr"] {
            let s = j[k].as_str().unwrap_or("");
            if !s.is_empty() {
                line(out, format!("[{k}]\n{}", s.trim_end()));
            }
        }
        if let Some(e) = j["error"].as_str().filter(|e| !e.is_empty()) {
            line(out, format!("[error] {e}"));
        }
        return Ok(0);
    }
    if a.cancel {
        let v = request(target, "cancel_run", json!({"run": run})).await?;
        line(out, format!("{run}: canceled {} jobs", v["canceled"]));
        return Ok(0);
    }
    let v = request(
        target,
        "jobs",
        json!({"run": run, "state": a.state.unwrap_or_default(), "limit": 10000, "output": a.output}),
    )
    .await?;
    if json {
        line(out, v.to_string());
        return Ok(0);
    }
    line(
        out,
        format!(
            "{:<8}  {:<10}  {:<16}  {:>7}  {:>4}  ERROR",
            "JOB", "STATE", "MACHINE", "ATTEMPT", "EXIT"
        ),
    );
    for j in v["jobs"].as_array().cloned().unwrap_or_default() {
        line(
            out,
            format!(
                "{:<8}  {:<10}  {:<16}  {:>7}  {:>4}  {}",
                j["id"],
                j["state"].as_str().unwrap_or(""),
                j["node"].as_str().unwrap_or("-"),
                j["attempt"],
                j["exit_code"]
                    .as_i64()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".into()),
                j["error"]
                    .as_str()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
            ),
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computers_names_the_devices_that_hold_the_pool_keys() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Not shared with this device: who holds them, and the ways on.
        let k = json!({"gen": 0, "shared_with": [], "waiting": "waiting",
            "holders": [{"device": "dev_a", "name": "ada-laptop", "last_seen": now - 3 * 86_400},
                        {"device": "dev_b"}]});
        let l = keys_lines(&k, None);
        assert_eq!(l.len(), 1);
        assert!(
            l[0].contains("Your pool keys are on: ada-laptop (last seen 3 days ago), dev_b;"),
            "{l:?}"
        );
        assert!(l[0].contains("at least one must be online to add a new device"));
        assert!(l[0].contains("cua devices revoke"));
        // Held here too: the same visibility.
        let k = json!({"gen": 2, "shared_with": ["ada-laptop", "box"],
            "holders": [{"device": "dev_a", "name": "ada-laptop"}, {"device": "dev_c", "name": "box"}]});
        assert_eq!(
            keys_lines(&k, None),
            vec!["Sealed with your devices' keys (generation 2). Your pool keys are on: ada-laptop, box; at least one must be online to add a new device.".to_string()]
        );
        // An older daemon (no holders): the devices it is shared with.
        let k = json!({"gen": 1, "shared_with": ["mac"]});
        assert!(keys_lines(&k, None)[0].contains("Your pool keys are on: mac;"));
    }

    #[test]
    fn the_keys_line_fits_a_narrow_terminal() {
        let s = "Sealed with your devices' keys (generation 2), held by: macbook";
        assert_eq!(fit_line(s, None), vec![s.to_string()]);
        assert_eq!(fit_line(s, Some(80)), vec![s.to_string()]);
        let l = fit_line(s, Some(58));
        assert_eq!(l.join(" "), s);
        assert!(
            l.len() == 2 && l.iter().all(|x| x.chars().count() <= 58),
            "{l:?}"
        );
    }

    #[test]
    fn local_is_the_default_and_auto_is_the_pool() {
        assert_eq!(target_of(None).unwrap(), ("local", "local".into()));
        assert_eq!(target_of(Some("LOCAL")).unwrap(), ("local", "local".into()));
        assert_eq!(target_of(Some("auto")).unwrap(), ("pool", String::new()));
        assert_eq!(
            target_of(Some("mac-mini")).unwrap(),
            ("pool", "mac-mini".into())
        );
        assert!(target_of(Some("cloud")).is_err());
        assert_eq!(
            target_of(Some("host:aws-2")).unwrap(),
            ("pool", "aws-2".into())
        );
        // A cloud that supplies pool hosts is a pin, for runs and jobs alike.
        for c in ["aws", "gcp", "hetzner"] {
            let pin = format!("cloud:{c}");
            assert_eq!(target_of(Some(c)).unwrap(), ("pool", pin.clone()));
            assert_eq!(crate::jobs_cmd::placement(c).unwrap(), pin);
        }
        assert_eq!(
            target_of(Some(" AWS ")).unwrap(),
            ("pool", "cloud:aws".into())
        );
        // One that does not gets one clear error on both paths.
        let msg = |on: &str| target_of(Some(on)).unwrap_err().to_string();
        let jmsg = |on: &str| crate::jobs_cmd::placement(on).unwrap_err().to_string();
        for c in ["azure", "modal", "cloud"] {
            assert!(msg(c).contains("--on auto") && msg(c).contains("cua cloud connect"));
            assert_eq!(msg(c), jmsg(c));
        }
        assert!(target_of(Some("direct:1.2.3.4:5")).is_err());
        assert_eq!(target_of_run("l-1234"), "local");
        assert_eq!(target_of_run("p-1234"), "pool");
    }
}
