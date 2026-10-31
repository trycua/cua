//! Durable agent sessions: `cua agent run --durable`, `cua agent sessions`,
//! and `status` / `logs` / `stop` / `rm` of `session:NAME`.
//!
//! A durable session is kept alive by the coordinator of your machines
//! (`session_*` pool requests, `cua-spaces-cluster`): when its driver or
//! the machine it runs on dies, it is started again at a higher epoch, with
//! backoff and a cap, and ends `done`, `failed` or `stopped`.
//!
//! ```text
//! cua agent run --durable auto pi "refactor the parser" --env-from-host ANTHROPIC_API_KEY
//! cua agent run --durable research pi "keep going" --on auto --max-attempts 8 -f
//! cua agent sessions
//! cua agent status session:agent-1a2b3c4d
//! cua agent logs session:agent-1a2b3c4d -f
//! cua agent stop session:agent-1a2b3c4d
//! ```

use crate::agent_cmd::RunArgs;
use crate::util::line;
use cua_sdk::CuaError;
use serde_json::{Value, json};
use std::io::Write;
use std::time::Duration;

/// The prefix that names a durable session where a sandbox goes.
pub const PREFIX: &str = "session:";

/// The driver's entry in its bundle, for the `pi` harness.
pub const PI_DRIVER_ENTRY: &str = "pi-durable-driver";

/// The session `s` names (`session:NAME`), if it does.
pub fn session_of(s: &str) -> Option<&str> {
    s.strip_prefix(PREFIX).filter(|n| !n.is_empty())
}

/// The Space size of an ACP harness driver (the harness and its install
/// run in it; the job default of 1 CPU and 1 GiB runs out of memory).
const ACP_DRIVER_CPUS: u32 = 2;
const ACP_DRIVER_MEMORY_MB: u64 = 4096;

/// The driver of a session: what the coordinator runs, and how it is
/// packaged.
struct Driver {
    argv: Vec<String>,
    /// Extra environment (keys for an ACP harness).
    env: serde_json::Map<String, Value>,
    /// Base64 gzip tar unpacked in the driver's work directory.
    bundle: String,
    state_dir: String,
}

/// The driver of `harness`: `--driver-cmd` (testing), the pi-durable driver
/// for `pi`, and for every other ACP harness the durable ACP driver of
/// `cua-agents`.
fn driver_of(
    a: &RunArgs,
    name: &str,
    keys: serde_json::Map<String, Value>,
) -> Result<Driver, CuaError> {
    if let Some(c) = a.driver_cmd.as_deref() {
        let argv: Vec<String> = c.split_whitespace().map(str::to_string).collect();
        if argv.is_empty() {
            return Err(CuaError::InvalidArgument("--driver-cmd is empty".into()));
        }
        return Ok(Driver {
            argv,
            env: keys,
            bundle: String::new(),
            state_dir: String::new(),
        });
    }
    if a.harness == "pi" {
        return Ok(Driver {
            argv: vec!["node".into(), PI_DRIVER_ENTRY.into()],
            env: keys,
            bundle: String::new(),
            state_dir: String::new(),
        });
    }
    if !a.files.is_empty() {
        return Err(CuaError::InvalidArgument(
            "--file is not supported with --durable".into(),
        ));
    }
    let mut opts = cua_agents::RunOptions {
        cwd: a.cwd.clone(),
        repo: a.repo.clone(),
        branch: a.branch.clone(),
        model: a.model.clone(),
        label: a.label.clone(),
        skills: Some(!a.no_skills),
        sandbox_mcp: Some(!a.no_sandbox_mcp),
        exit_when_idle: a.exit_when_idle,
        durable: Some(cua_agents::durable::DurableOptions {
            session: Some(name.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    for (k, v) in keys {
        if let Some(v) = v.as_str() {
            opts.env.insert(k, v.to_string());
        }
    }
    if let Some(url) = &a.base_url {
        let wire = match a.wire.as_deref() {
            Some(w) => Some(cua_agents::harness::Wire::parse(w).ok_or_else(|| {
                CuaError::InvalidArgument(format!(
                    "--wire {w}: use anthropic, openai-responses, openai-chat or gemini"
                ))
            })?),
            None => None,
        };
        opts.endpoint = Some(cua_agents::harness::Endpoint {
            base_url: url.clone(),
            wire,
            model: a.model.clone(),
        });
    }
    for m in &a.mcp {
        let m = crate::agent_cmd::mcp_arg(m)?;
        opts.mcp_servers.push(cua_agents::McpServer {
            name: m.name,
            url: m.url,
            command: m.command,
            args: m.args,
            ..Default::default()
        });
    }
    let d = cua_agents::durable::durable_driver_spec(&a.harness, &a.prompt, &opts)
        .map_err(|e| CuaError::InvalidArgument(format!("--durable {}: {e}", a.harness)))?;
    Ok(Driver {
        argv: d.argv,
        env: d.env.into_iter().map(|(k, v)| (k, v.into())).collect(),
        bundle: d.bundle,
        state_dir: d.state_dir,
    })
}

/// `session_start` params for `cua agent run --durable`. `lookup` reads the
/// shell environment (injected for tests).
pub(crate) fn start_params(
    a: &RunArgs,
    name: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Value, CuaError> {
    use base64::Engine as _;
    let mut keys = serde_json::Map::new();
    let allowed = crate::agent_cmd::provider_key_names();
    for n in &a.env_from_host {
        if !allowed.contains(n) {
            return Err(CuaError::InvalidArgument(format!(
                "{n} is not a provider key variable; allowed: {}",
                allowed.join(", ")
            )));
        }
        match lookup(n).filter(|v| !v.is_empty()) {
            Some(v) => {
                keys.insert(n.clone(), v.into());
            }
            None => {
                return Err(CuaError::InvalidArgument(format!(
                    "--env-from-host {n}: not set in this shell (a durable session has no \
                     server to resolve it)"
                )));
            }
        }
    }
    let driver = driver_of(a, name, keys)?;
    let mut env = driver.env;
    // The pi driver (and a --driver-cmd) reads the task from its environment;
    // an ACP driver has it in its bundle.
    if driver.bundle.is_empty() {
        env.insert("CUA_AGENT_HARNESS".into(), a.harness.clone().into());
        env.insert("CUA_AGENT_PROMPT".into(), a.prompt.clone().into());
        for (k, v) in [
            ("CUA_AGENT_MODEL", &a.model),
            ("CUA_AGENT_BASE_URL", &a.base_url),
            ("CUA_AGENT_WIRE", &a.wire),
            ("CUA_AGENT_REPO", &a.repo),
            ("CUA_AGENT_BRANCH", &a.branch),
            ("CUA_AGENT_CWD", &a.cwd),
        ] {
            if let Some(v) = v {
                env.insert(k.into(), v.clone().into());
            }
        }
    }
    let bundle = match &a.driver_bundle {
        Some(p) => base64::engine::general_purpose::STANDARD.encode(
            std::fs::read(p)
                .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", p.display())))?,
        ),
        None => driver.bundle,
    };
    let bundle_is_empty = bundle.is_empty();
    let space = match a.sandbox.trim() {
        "" | "auto" => String::new(),
        s if s.contains(':') => {
            return Err(CuaError::InvalidArgument(format!(
                "{s}: --durable takes a durable Space by name, or `auto` for none"
            )));
        }
        s => s.to_string(),
    };
    let mut spec = json!({
        "argv": driver.argv,
        "state_dir": driver.state_dir,
        "env": env,
        "on": a.on.clone().unwrap_or_default(),
        "space": space,
        "label": a.harness,
        "bundle": bundle,
    });
    // A harness driver installs and runs the harness in its Space: the job
    // default (1 CPU, 1 GiB) is not enough for it (Claude Code's install
    // is killed for memory), so it gets a bigger one.
    if !bundle_is_empty {
        spec["cpus"] = ACP_DRIVER_CPUS.into();
        spec["memory_mb"] = ACP_DRIVER_MEMORY_MB.into();
    }
    if let Some(l) = &a.label {
        spec["label"] = l.clone().into();
    }
    if let Some(n) = a.stall_secs {
        spec["stall_secs"] = n.into();
    }
    Ok(json!({
        "name": name,
        "spec": spec,
        "max_attempts": a.max_attempts.unwrap_or(0),
    }))
}

/// `local` (this machine's coordinator) unless `--on` names the pool.
fn target_of_start(a: &RunArgs) -> Result<&'static str, CuaError> {
    Ok(crate::runs::target_of(a.on.as_deref())?.0)
}

/// Asks the coordinator that has session `name`: this machine's, else the
/// pool's.
async fn ask(method: &str, params: Value) -> Result<Value, CuaError> {
    match crate::runs::request("local", method, params.clone()).await {
        Err(CuaError::NotFound(first)) => crate::runs::request("pool", method, params)
            .await
            .map_err(|e| match e {
                CuaError::NotFound(_) => CuaError::NotFound(first),
                other => other,
            }),
        other => other,
    }
}

fn state_line(v: &Value) -> String {
    let mut s = format!(
        "{:<24} {:<8} epoch {:<3} drive {}/{}",
        v["name"].as_str().unwrap_or(""),
        v["state"].as_str().unwrap_or(""),
        v["epoch"],
        v["attempts"],
        v["max_attempts"],
    );
    if let Some(r) = v["reason"].as_str().filter(|r| !r.is_empty()) {
        s.push_str(&format!(" ({r})"));
    }
    if let Some(n) = v["next_attempt_in_secs"].as_i64() {
        s.push_str(&format!(", next drive in {n}s"));
    }
    if let Some(n) = v["node"].as_str().filter(|n| !n.is_empty()) {
        s.push_str(&format!(", on {n}"));
    }
    // No machine has room for the driver yet.
    if let Some(w) = v["waiting"].as_str().filter(|w| !w.is_empty()) {
        s.push_str(&format!(", {w}"));
    }
    s
}

/// `cua agent run --durable`.
pub async fn start(a: RunArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let name = a
        .name
        .clone()
        .unwrap_or_else(|| format!("agent-{:08x}", rand::random::<u32>()));
    let params = start_params(&a, &name, |n| std::env::var(n).ok())?;
    let v = crate::runs::request(target_of_start(&a)?, "session_start", params).await?;
    if json && !a.follow {
        line(out, v.to_string());
    } else {
        eprintln!(
            "{}",
            crate::util::hint(&format!(
                "{name} started as a durable session (epoch {}); follow with: cua agent logs session:{name} -f",
                v["epoch"]
            ))
        );
    }
    if !a.follow {
        return Ok(0);
    }
    logs(&name, true, json, out).await?;
    let v = ask("session_get", json!({"name": name})).await?;
    Ok(if v["state"] == "done" { 0 } else { 1 })
}

/// `cua agent sessions`.
pub async fn sessions(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let mut all: Vec<Value> = vec![];
    let mut errors = vec![];
    for target in ["local", "pool"] {
        match crate::runs::request(target, "session_list", Value::Null).await {
            Ok(v) => {
                for s in v["sessions"].as_array().into_iter().flatten() {
                    if !all.iter().any(|x| x["name"] == s["name"]) {
                        all.push(s.clone());
                    }
                }
            }
            Err(e) => errors.push(e),
        }
    }
    if errors.len() == 2 {
        return Err(errors.remove(0));
    }
    if json {
        line(out, json!({ "sessions": all }).to_string());
    } else if all.is_empty() {
        line(out, "no durable sessions");
    } else {
        for s in &all {
            line(out, state_line(s));
        }
    }
    Ok(0)
}

/// `cua agent status session:NAME`.
pub async fn status(name: &str, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let v = ask("session_get", json!({"name": name})).await?;
    if json {
        line(out, v.to_string());
        return Ok(0);
    }
    line(out, state_line(&v));
    if let Some(a) = v["last_output_age_secs"].as_i64() {
        line(
            out,
            format!(
                "last output {a}s ago (wedged after {}s)",
                v["spec"]["stall_secs"]
            ),
        );
    }
    if let Some(e) = v["error"].as_str().filter(|e| !e.is_empty()) {
        line(out, format!("last problem: {e}"));
    }
    for h in v["history"].as_array().into_iter().flatten() {
        line(
            out,
            format!(
                "  epoch {} drive {}  {}{}{}",
                h["epoch"],
                h["attempt"],
                h["event"].as_str().unwrap_or(""),
                h["exit_code"]
                    .as_i64()
                    .map(|c| format!(" (exit {c})"))
                    .unwrap_or_default(),
                h["node"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .map(|n| format!(" on {n}"))
                    .unwrap_or_default(),
            ),
        );
    }
    Ok(0)
}

/// `cua agent logs session:NAME [-f]`: the output of each epoch's driver in
/// turn (a separator line where a new driver starts).
pub async fn logs(
    name: &str,
    follow: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (mut job, mut from) = (0i64, 0u64);
    // Bounded: at most 24 h of polling at 1 s.
    for _ in 0..86_400 {
        let o = match ask("session_output", json!({"name": name, "from": from})).await {
            Ok(o) => o,
            Err(e) if follow => {
                // The driver has not started on a machine yet: wait, unless
                // the session is over.
                let s = ask("session_get", json!({"name": name})).await?;
                if matches!(s["state"].as_str(), Some("done" | "failed" | "stopped")) {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        let this = o["job"].as_i64().unwrap_or(0);
        if this != job {
            if job != 0 && !json {
                line(out, format!("--- epoch {} ---", o["epoch"]));
            }
            job = this;
            if from != 0 {
                from = 0;
                continue;
            }
        }
        let text = o["text"].as_str().unwrap_or("");
        if json {
            if !text.is_empty() {
                line(out, json!({"epoch": o["epoch"], "text": text}).to_string());
            }
        } else {
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
        }
        from = o["next"].as_u64().unwrap_or(from);
        if !follow || (o["done"] == true && text.is_empty()) {
            break;
        }
        if text.is_empty() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(0)
}

/// `cua agent stop session:NAME`.
pub async fn stop(name: &str, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let v = ask("session_stop", json!({"name": name})).await?;
    if json {
        line(out, v.to_string());
    } else {
        line(out, state_line(&v));
    }
    Ok(0)
}

/// `cua agent rm session:NAME`: stop it and forget it.
pub async fn rm(name: &str, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    ask("session_delete", json!({"name": name})).await?;
    if json {
        line(out, json!({"name": name, "deleted": true}).to_string());
    } else {
        line(out, format!("removed session {name}"));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_waiting_driver_says_what_it_waits_for() {
        let v = json!({"name":"n","state":"running","epoch":2,"attempts":2,"max_attempts":5,
            "waiting":"waiting for a machine with 2 CPUs and 4 GB memory free"});
        let l = state_line(&v);
        assert!(
            l.ends_with(", waiting for a machine with 2 CPUs and 4 GB memory free"),
            "{l}"
        );
        let v = json!({"name":"n","state":"running","epoch":1,"attempts":1,"max_attempts":5});
        assert!(!state_line(&v).contains("waiting"));
    }

    use super::*;
    use crate::agent_cmd::AgentCmd;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        cmd: AgentCmd,
    }

    fn args(argv: &[&str]) -> RunArgs {
        let mut v = vec!["agent", "run"];
        v.extend_from_slice(argv);
        match Cli::parse_from(v).cmd {
            AgentCmd::Run(a) => *a,
            _ => unreachable!(),
        }
    }

    #[test]
    fn durable_run_makes_a_session_start_the_coordinator_accepts() {
        let a = args(&[
            "--durable",
            "--on",
            "auto",
            "--max-attempts",
            "8",
            "--stall-secs",
            "45",
            "--name",
            "refactor",
            "research",
            "pi",
            "keep going",
            "--model",
            "m1",
            "--env-from-host",
            "ANTHROPIC_API_KEY",
        ]);
        let shell = |n: &str| (n == "ANTHROPIC_API_KEY").then(|| "sk-ant-test".to_string());
        let p = start_params(&a, "refactor", shell).unwrap();
        assert_eq!(p["name"], "refactor");
        assert_eq!(p["max_attempts"], 8);
        assert_eq!(p["spec"]["argv"], json!(["node", PI_DRIVER_ENTRY]));
        assert_eq!(p["spec"]["space"], "research");
        assert_eq!(p["spec"]["stall_secs"], 45);
        assert_eq!(p["spec"]["on"], "auto");
        assert_eq!(p["spec"]["env"]["CUA_AGENT_PROMPT"], "keep going");
        assert_eq!(p["spec"]["env"]["CUA_AGENT_MODEL"], "m1");
        assert_eq!(p["spec"]["env"]["ANTHROPIC_API_KEY"], "sk-ant-test");
        // Only fields the coordinator's SessionSpec has (it keeps the same
        // list in a test of its own).
        let known = [
            "argv",
            "env",
            "image",
            "kind",
            "runtime",
            "cpus",
            "memory_mb",
            "timeout_secs",
            "on",
            "os",
            "arch",
            "accelerator",
            "bundle",
            "space",
            "state_dir",
            "label",
            "stall_secs",
        ];
        for k in p["spec"].as_object().unwrap().keys() {
            assert!(
                known.contains(&k.as_str()),
                "unknown session spec field {k}"
            );
        }
        assert_eq!(target_of_start(&a).unwrap(), "pool");
        assert_eq!(
            target_of_start(&args(&["--durable", "auto", "pi", "x"])).unwrap(),
            "local"
        );
    }

    #[test]
    fn durable_run_refuses_what_it_cannot_do() {
        let none = |_: &str| None;
        // A harness without a durable driver, a key missing from the shell,
        // a sandbox reference where a Space name goes.
        let e = start_params(&args(&["--durable", "auto", "nope", "x"]), "n", none).unwrap_err();
        assert!(e.to_string().contains("unknown harness"), "{e}");
        let e = start_params(
            &args(&[
                "--durable",
                "auto",
                "pi",
                "x",
                "--env-from-host",
                "OPENAI_API_KEY",
            ]),
            "n",
            none,
        )
        .unwrap_err();
        assert!(e.to_string().contains("not set in this shell"), "{e}");
        let e = start_params(&args(&["--durable", "local:dev", "pi", "x"]), "n", none).unwrap_err();
        assert!(e.to_string().contains("durable Space by name"), "{e}");
        // The testing hook supplies any driver.
        let p = start_params(
            &args(&[
                "--durable",
                "auto",
                "claude-code",
                "x",
                "--driver-cmd",
                "sh -c true",
            ]),
            "n",
            none,
        )
        .unwrap();
        assert_eq!(p["spec"]["argv"], json!(["sh", "-c", "true"]));
    }

    #[test]
    fn an_acp_harness_runs_the_durable_acp_driver_from_cua_agents() {
        let a = args(&[
            "--durable",
            "--name",
            "fix-it",
            "auto",
            "claude-code",
            "fix the test",
            "--model",
            "m1",
            "--env-from-host",
            "ANTHROPIC_API_KEY",
            "--no-skills",
        ]);
        let shell = |n: &str| (n == "ANTHROPIC_API_KEY").then(|| "sk-ant-secret".to_string());
        let p = start_params(&a, "fix-it", shell).unwrap();
        let spec = &p["spec"];
        assert!(!spec["argv"].as_array().unwrap().is_empty());
        assert_ne!(spec["argv"], json!(["node", PI_DRIVER_ENTRY]));
        // Packaged for the job: a bundle, no prompt in the environment, the
        // key in the environment only (never in the bundle).
        let bundle = spec["bundle"].as_str().unwrap();
        assert!(!bundle.is_empty());
        assert!(spec["env"].get("CUA_AGENT_PROMPT").is_none());
        assert_eq!(spec["env"]["ANTHROPIC_API_KEY"], "sk-ant-secret");
        assert!(!bundle.contains("sk-ant-secret"));
        // The journal stays in the job's data directory, which outlives the
        // driver's Space on the same machine (never `~/.cua/...` in the Space).
        assert_eq!(spec["state_dir"], "");
        // The harness installs and runs in the driver's Space: sized for it.
        assert_eq!(spec["cpus"], ACP_DRIVER_CPUS);
        assert_eq!(spec["memory_mb"], ACP_DRIVER_MEMORY_MB);
        assert_eq!(spec["label"], "claude-code");
        // Same inputs, same driver.
        let again = start_params(&a, "fix-it", shell).unwrap();
        assert_eq!(again["spec"]["bundle"], spec["bundle"]);
        // What a durable driver cannot take is refused, not dropped.
        let e = start_params(
            &args(&[
                "--durable",
                "auto",
                "claude-code",
                "x",
                "--repo",
                "https://x/y",
            ]),
            "n",
            shell,
        )
        .unwrap_err();
        assert!(e.to_string().contains("repo"), "{e}");
    }

    #[test]
    fn a_session_is_named_with_a_prefix() {
        assert_eq!(session_of("session:work"), Some("work"));
        assert_eq!(session_of("session:"), None);
        assert_eq!(session_of("local:dev"), None);
    }
}
