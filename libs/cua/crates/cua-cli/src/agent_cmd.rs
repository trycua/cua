//! `cua agent`: coding agents inside sandboxes (`cua agents` is host
//! onboarding: skills and the cua MCP server in your local agents).
//!
//! ```text
//! cua agent run local:dev claude-code "fix the failing test" --env-from-host ANTHROPIC_API_KEY --follow
//! cua agent ls local:dev
//! cua agent logs local:dev run-1a2b3c4d --follow
//! cua agent send local:dev run-1a2b3c4d "now add a regression test"
//! cua agent interrupt local:dev run-1a2b3c4d
//! cua agent stop local:dev run-1a2b3c4d
//! ```

use crate::sandbox::{Narrow, env_of};
use crate::util::line;
use clap::Subcommand;
use cua_sdk::{AgentFile, AgentRun, AgentRunMcpServer, AgentRunOptions, Cua, CuaError};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Start a coding agent in a sandbox (returns at once unless --follow).
    #[command(after_help = "Examples:
  cua agent run local:dev claude-code \"fix the failing test\" --env-from-host ANTHROPIC_API_KEY -f
  cua agent run cloud:ci openai-codex \"add tests\" --repo https://github.com/org/repo --exit-when-idle")]
    Run(Box<RunArgs>),
    /// List runs in a sandbox.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua agent ls local:dev"
    )]
    Ls {
        #[command(flatten)]
        at: Narrow,
        /// Sandbox (`local:NAME`, `cloud:NAME`, `direct:HOST:PORT`, or NAME).
        sandbox: String,
    },
    /// Print a run's normalized events (--follow to stream).
    #[command(after_help = "Examples:
  cua agent logs local:dev run-1a2b3c4d -f")]
    Logs {
        #[command(flatten)]
        at: Narrow,
        sandbox: String,
        run_id: String,
        /// Keep streaming until the turn ends.
        #[arg(long, short)]
        follow: bool,
        /// Print each event's JSON (raw ACP payload included).
        #[arg(long)]
        raw: bool,
    },
    /// Send a follow-up (queued while a turn runs).
    #[command(after_help = "Examples:
  cua agent send local:dev run-1a2b3c4d \"now add a regression test\" -f")]
    Send {
        #[command(flatten)]
        at: Narrow,
        sandbox: String,
        run_id: String,
        text: String,
        #[arg(long, short)]
        follow: bool,
    },
    /// Cancel the turn in flight (the session stays open).
    #[command(after_help = "Examples:
  cua agent interrupt local:dev run-1a2b3c4d")]
    Interrupt {
        #[command(flatten)]
        at: Narrow,
        sandbox: String,
        run_id: String,
    },
    /// Stop a run and verify it is gone.
    #[command(after_help = "Examples:
  cua agent stop local:dev run-1a2b3c4d")]
    Stop {
        #[command(flatten)]
        at: Narrow,
        sandbox: String,
        run_id: String,
    },
    /// A run's status and last result.
    #[command(after_help = "Examples:
  cua agent status local:dev run-1a2b3c4d")]
    Status {
        #[command(flatten)]
        at: Narrow,
        sandbox: String,
        run_id: String,
    },
    /// Stop a run and delete its record (secrets included).
    #[command(after_help = "Examples:
  cua agent rm local:dev run-1a2b3c4d")]
    Rm {
        #[command(flatten)]
        at: Narrow,
        sandbox: String,
        run_id: String,
    },
    /// Install harnesses or apps now (`claude-code`, `blender`, ...).
    #[command(after_help = "Examples:
  cua agent ensure local:dev claude-code vscode")]
    Ensure {
        #[command(flatten)]
        at: Narrow,
        sandbox: String,
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// The harnesses: readiness, installs, key variables, limits.
    #[command(after_help = "Examples:
  cua agent harnesses
  cua --json agent harnesses")]
    Harnesses,
    /// The provider keys agents get, kept in the Keychain by the Cua daemon
    /// (Cua Spaces → Settings → Agents). Values are read from stdin and never
    /// printed.
    #[command(after_help = "Examples:
  cua agent keys
  cua agent keys set anthropic
  cua agent keys rm OPENAI_API_KEY")]
    Keys {
        #[command(subcommand)]
        cmd: Option<crate::agent_keys_cmd::KeysCmd>,
    },
    #[command(flatten)]
    Persistent(crate::persistent_cmd::PersistentCmd),
}

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    #[command(flatten)]
    at: Narrow,
    /// Sandbox (`local:NAME`, `cloud:NAME`, `direct:HOST:PORT`, or NAME).
    sandbox: String,
    /// Harness (`cua agent harnesses`).
    harness: String,
    /// The task.
    prompt: String,
    /// Forward a provider key from this shell's environment (repeatable),
    /// for example ANTHROPIC_API_KEY. The value never appears in argv.
    #[arg(long = "env-from-host", value_name = "VAR")]
    env_from_host: Vec<String>,
    /// Git repository cloned into the working directory first.
    #[arg(long)]
    repo: Option<String>,
    #[arg(long)]
    branch: Option<String>,
    /// Working directory in the sandbox.
    #[arg(long)]
    cwd: Option<String>,
    #[arg(long)]
    model: Option<String>,
    /// A custom model endpoint (proxy or compatible server).
    #[arg(long)]
    base_url: Option<String>,
    /// Its wire format: anthropic, openai-responses, openai-chat, gemini.
    #[arg(long)]
    wire: Option<String>,
    /// An MCP server for the agent: NAME=URL (HTTP, as reachable inside the
    /// sandbox) or NAME=cmd:COMMAND ARGS... (stdio, run in the sandbox).
    #[arg(long = "mcp", value_name = "NAME=URL")]
    mcp: Vec<String>,
    /// Attach a local file to the prompt (repeatable).
    #[arg(long = "file", value_name = "PATH")]
    files: Vec<std::path::PathBuf>,
    /// Do not give the agent the sandbox's own MCP tools.
    #[arg(long)]
    no_sandbox_mcp: bool,
    /// Do not copy the cua skills into the harness.
    #[arg(long)]
    no_skills: bool,
    /// Stop (resumably) once the prompt is answered.
    #[arg(long)]
    exit_when_idle: bool,
    #[arg(long)]
    label: Option<String>,
    /// Stream events until the turn ends.
    #[arg(long, short)]
    follow: bool,
}

fn mcp_arg(s: &str) -> Result<AgentRunMcpServer, CuaError> {
    let (name, rest) = s
        .split_once('=')
        .ok_or_else(|| CuaError::InvalidArgument(format!("--mcp {s:?}: expected NAME=URL")))?;
    let mut m = AgentRunMcpServer {
        name: name.into(),
        ..Default::default()
    };
    if let Some(cmd) = rest.strip_prefix("cmd:") {
        let mut words = cmd.split_whitespace().map(str::to_string);
        m.command = words.next();
        m.args = words.collect();
    } else {
        m.url = Some(rest.into());
    }
    Ok(m)
}

async fn follow(
    run: &AgentRun,
    mut cursor: u64,
    until_turn_end: bool,
    raw: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<u64, CuaError> {
    // Bounded: at most 24 h of polling at 500 ms.
    for _ in 0..172_800 {
        let page = run.events(cursor, Some(500)).await?;
        cursor = page.cursor;
        let mut done = false;
        for e in &page.events {
            if json || raw {
                line(out, &e.json);
            } else if let Some(l) = &e.line {
                line(out, l);
            }
            if matches!(e.kind.as_str(), "turn_ended" | "exited") {
                done = true;
            }
        }
        if !until_turn_end || (done && page.caught_up) {
            break;
        }
        if page.caught_up {
            let s = run.status().await?;
            if s.status != "running" && !done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    Ok(cursor)
}

/// The harnesses with the readiness the Spaces server reports
/// (`agent_capabilities`, which counts the keys saved in Cua Spaces →
/// Settings → Agents and the server's environment), so `cua agent
/// harnesses`, MCP and the app agree. Without a server: the static list.
async fn harnesses(cua: &Arc<Cua>) -> serde_json::Value {
    let caps = cua
        .spaces()
        .call_tool_json("agent_capabilities".into(), None)
        .await;
    if let Ok(r) = caps
        && !r.is_error
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&r.text)
        && v["harnesses"].is_array()
    {
        return v["harnesses"].clone();
    }
    serde_json::from_str(&cua_sdk::agent_harnesses()).unwrap_or_default()
}

/// The provider key variables `--env-from-host` may name.
fn provider_key_names() -> Vec<String> {
    let v: serde_json::Value =
        serde_json::from_str(&cua_sdk::agent_harnesses()).unwrap_or_default();
    let mut names: Vec<String> = v
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|h| h["keys"].as_array().cloned().unwrap_or_default())
        .filter_map(|k| k.as_str().map(str::to_string))
        .chain(["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"].map(String::from))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// `--env-from-host`: names set in this shell go as the run's env (this
/// shell's value wins, as before); the others are left to the server,
/// which gives a run the keys saved in Cua Spaces and its own environment.
fn split_env_from_host(
    names: &[String],
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(serde_json::Map<String, serde_json::Value>, Vec<String>), CuaError> {
    let allowed = provider_key_names();
    let mut env = serde_json::Map::new();
    let mut server = vec![];
    for n in names {
        match lookup(n).filter(|v| !v.is_empty()) {
            Some(v) if allowed.contains(n) => {
                env.insert(n.clone(), v.into());
            }
            Some(_) => {
                return Err(CuaError::InvalidArgument(format!(
                    "{n} is not a provider key variable; allowed: {}",
                    allowed.join(", ")
                )));
            }
            None => server.push(n.clone()),
        }
    }
    Ok((env, server))
}

/// The `agent_start` arguments for `cua agent run`.
fn start_args(a: &RunArgs, space: &str) -> Result<serde_json::Value, CuaError> {
    let (env, env_from_host) = split_env_from_host(&a.env_from_host, |n| std::env::var(n).ok())?;
    let mut files = vec![];
    for p in &a.files {
        let abs = std::fs::canonicalize(p)
            .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", p.display())))?;
        files.push(abs.to_string_lossy().to_string());
    }
    let mut mcp = vec![];
    for m in &a.mcp {
        let m = mcp_arg(m)?;
        mcp.push(serde_json::json!({
            "name": m.name, "url": m.url, "command": m.command, "args": m.args,
        }));
    }
    Ok(serde_json::json!({
        "space": space,
        "agent": a.harness,
        "prompt": a.prompt,
        "env": env,
        "env_from_host": env_from_host,
        "repo": a.repo,
        "branch": a.branch,
        "cwd": a.cwd,
        "model": a.model,
        "base_url": a.base_url,
        "wire": a.wire,
        "mcp_servers": mcp,
        "files": files,
        "sandbox_mcp": !a.no_sandbox_mcp,
        "skills": !a.no_skills,
        "exit_when_idle": a.exit_when_idle,
        "label": a.label,
    }))
}

/// A tool error as the CLI reports it.
fn tool_error(r: &cua_sdk::SpaceToolResult) -> CuaError {
    let structured: serde_json::Value = r
        .structured_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let msg = structured["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            r.text
                .strip_prefix("error: ")
                .unwrap_or(&r.text)
                .to_string()
        });
    match structured["error"]["kind"].as_str() {
        Some("invalid_argument") => CuaError::InvalidArgument(msg),
        Some("not_found") => CuaError::NotFound(msg),
        _ => CuaError::Env(msg),
    }
}

/// `cua agent run`: the Spaces server's `agent_start`, the one path MCP
/// and the app take too (saved keys, readiness, the failure
/// notification). A direct sandbox the server does not know (its token is
/// kept by this CLI) is started from here.
async fn start(
    cua: &Arc<Cua>,
    a: RunArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let sandbox = a.at.apply(&a.sandbox)?;
    let args = start_args(&a, &sandbox)?;
    let r = cua
        .spaces()
        .call_tool_json("agent_start".into(), Some(args.to_string()))
        .await?;
    if r.is_error {
        let e = tool_error(&r);
        // A direct sandbox the server does not know: its token is here.
        if matches!(e, CuaError::NotFound(_))
            && let Ok(info) = cua.sandboxes().get(sandbox.clone()).await
            && info.location == "direct"
        {
            return start_direct(cua, a, &sandbox, json, out).await;
        }
        return Err(e);
    }
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap_or_default();
    let run_id = v["run_id"].as_str().unwrap_or_default().to_string();
    let harness = v["agent"].as_str().unwrap_or(&a.harness).to_string();
    if json && !a.follow {
        line(
            out,
            serde_json::json!({"run_id": run_id, "harness": harness}).to_string(),
        );
    } else {
        for n in v["notes"].as_array().into_iter().flatten() {
            if let Some(n) = n.as_str() {
                eprintln!("note: {n}");
            }
        }
        eprintln!(
            "{run_id} started ({harness}); follow with: cua agent logs {} {run_id} -f",
            a.sandbox
        );
    }
    if !a.follow {
        return Ok(0);
    }
    let run = env_of(cua, &sandbox)
        .await?
        .agents()
        .await?
        .get(run_id)
        .await?;
    finish(&run, json, out).await
}

/// Follows `run` to the end of its turn; 1 when it failed.
async fn finish(run: &AgentRun, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    follow(run, 0, true, false, json, out).await?;
    let r = run.result().await?;
    Ok(if r.error.is_some() || r.status == "failed" {
        1
    } else {
        0
    })
}

/// `cua agent run` on a direct sandbox: the SDK's runner over the token
/// this CLI keeps (no keys saved in Cua Spaces; `--env-from-host` reads
/// this shell).
async fn start_direct(
    cua: &Arc<Cua>,
    a: RunArgs,
    sandbox: &str,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let agents = env_of(cua, sandbox).await?.agents().await?;
    let mut files = vec![];
    for p in &a.files {
        let bytes = std::fs::read(p)
            .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", p.display())))?;
        files.push(AgentFile {
            name: p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            bytes,
        });
    }
    let opts = AgentRunOptions {
        cwd: a.cwd,
        repo: a.repo,
        branch: a.branch,
        env: HashMap::new(),
        env_from_host: a.env_from_host,
        model: a.model,
        base_url: a.base_url,
        wire: a.wire,
        mcp_servers: a.mcp.iter().map(|m| mcp_arg(m)).collect::<Result<_, _>>()?,
        sandbox_mcp: Some(!a.no_sandbox_mcp),
        skills: Some(!a.no_skills),
        install: None,
        files,
        exit_when_idle: a.exit_when_idle,
        label: a.label,
    };
    let run = agents.run(a.harness, a.prompt, Some(opts)).await?;
    if json && !a.follow {
        line(
            out,
            serde_json::json!({"run_id": run.run_id(), "harness": run.harness()}).to_string(),
        );
    } else {
        eprintln!(
            "{} started ({}); follow with: cua agent logs {} {} -f",
            run.run_id(),
            run.harness(),
            a.sandbox,
            run.run_id()
        );
    }
    if a.follow {
        return finish(&run, json, out).await;
    }
    Ok(0)
}

pub async fn run(
    cua: &Arc<Cua>,
    cmd: AgentCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    if let AgentCmd::Persistent(p) = cmd {
        return crate::persistent_cmd::run(cua, p, json, out).await;
    }
    if let AgentCmd::Keys { cmd } = cmd {
        return crate::agent_keys_cmd::run(cua, cmd, json, out).await;
    }
    if let AgentCmd::Harnesses = cmd {
        let v = harnesses(cua).await;
        if json {
            line(out, serde_json::to_string_pretty(&v).unwrap_or_default());
        } else {
            for x in v.as_array().into_iter().flatten() {
                line(
                    out,
                    format!(
                        "{:<20} {:<6} {:<12} keys: {}",
                        x["id"].as_str().unwrap_or(""),
                        if x["ready"] == true { "ready" } else { "no" },
                        format!("auth: {}", x["auth"].as_str().unwrap_or("unknown")),
                        x["keys"]
                            .as_array()
                            .map(|k| k
                                .iter()
                                .filter_map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", "))
                            .unwrap_or_default()
                    ),
                );
                if x["ready"] != true
                    && let Some(hint) = x["auth_hint"].as_str()
                {
                    line(out, format!("  {hint}"));
                }
            }
        }
        return Ok(0);
    }
    if let AgentCmd::Run(a) = cmd {
        return start(cua, *a, json, out).await;
    }
    let (at, sandbox) = match &cmd {
        AgentCmd::Ls { at, sandbox }
        | AgentCmd::Logs { at, sandbox, .. }
        | AgentCmd::Send { at, sandbox, .. }
        | AgentCmd::Interrupt { at, sandbox, .. }
        | AgentCmd::Stop { at, sandbox, .. }
        | AgentCmd::Status { at, sandbox, .. }
        | AgentCmd::Rm { at, sandbox, .. }
        | AgentCmd::Ensure { at, sandbox, .. } => (*at, sandbox.clone()),
        AgentCmd::Run(_)
        | AgentCmd::Harnesses
        | AgentCmd::Keys { .. }
        | AgentCmd::Persistent(_) => unreachable!(),
    };
    let guest = env_of(cua, &at.apply(&sandbox)?).await?;
    let agents = guest.agents().await?;
    let info_line = |s: &cua_sdk::AgentRunInfo| {
        format!(
            "{}  {:<18} {:<8} {:<10} turn {}  {}",
            s.run_id,
            s.harness.clone().unwrap_or_default(),
            s.status,
            s.phase,
            s.turn,
            s.prompt
                .clone()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(60)
                .collect::<String>()
        )
    };
    match cmd {
        AgentCmd::Ls { .. } => {
            let runs = agents.list().await?;
            if json {
                let v: Vec<serde_json::Value> = runs
                    .iter()
                    .map(|r| serde_json::from_str(&r.json).unwrap_or_default())
                    .collect();
                line(out, serde_json::Value::Array(v).to_string());
            } else if runs.is_empty() {
                line(out, "no agent runs");
            } else {
                for r in &runs {
                    line(out, info_line(r));
                }
            }
        }
        AgentCmd::Logs {
            run_id,
            follow: f,
            raw,
            ..
        } => {
            let run = agents.get(run_id).await?;
            follow(&run, 0, f, raw, json, out).await?;
        }
        AgentCmd::Send {
            run_id,
            text,
            follow: f,
            ..
        } => {
            let run = agents.get(run_id).await?;
            let start = run.events(0, Some(1_000_000)).await?.cursor;
            let s = run.send(text, None).await?;
            if !f {
                line(out, info_line(&s));
            } else {
                follow(&run, start, true, false, json, out).await?;
            }
        }
        AgentCmd::Interrupt { run_id, .. } => {
            let s = agents.get(run_id).await?.interrupt().await?;
            line(out, info_line(&s));
        }
        AgentCmd::Stop { run_id, .. } => {
            let s = agents.get(run_id).await?.stop().await?;
            line(out, info_line(&s));
            return Ok(if s.alive == Some(false) { 0 } else { 1 });
        }
        AgentCmd::Status { run_id, .. } => {
            let run = agents.get(run_id).await?;
            let s = run.status().await?;
            let r = run.result().await?;
            if json {
                line(
                    out,
                    serde_json::json!({
                        "status": serde_json::from_str::<serde_json::Value>(&s.json).unwrap_or_default(),
                        "result": {"turn": r.turn, "text": r.text, "stop_reason": r.stop_reason,
                                   "error": r.error, "tool_calls": r.tool_calls},
                    })
                    .to_string(),
                );
            } else {
                line(out, info_line(&s));
                line(out, format!("reason: {}", s.reason));
                if !r.text.is_empty() {
                    line(out, format!("last turn ({:?}): {}", r.stop_reason, r.text));
                }
            }
        }
        AgentCmd::Rm { run_id, .. } => {
            agents.get(run_id.clone()).await?.remove().await?;
            line(out, format!("removed {run_id}"));
        }
        AgentCmd::Ensure { ids, .. } => {
            for p in agents.ensure(ids).await? {
                line(out, p);
            }
        }
        AgentCmd::Run(_)
        | AgentCmd::Harnesses
        | AgentCmd::Keys { .. }
        | AgentCmd::Persistent(_) => {}
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        cmd: AgentCmd,
    }

    fn run_args(argv: &[&str]) -> RunArgs {
        let mut v = vec!["agent", "run"];
        v.extend_from_slice(argv);
        match Cli::parse_from(v).cmd {
            AgentCmd::Run(a) => *a,
            _ => unreachable!(),
        }
    }

    #[test]
    fn env_from_host_uses_this_shell_then_the_server() {
        let shell = |n: &str| match n {
            "ANTHROPIC_API_KEY" => Some("sk-ant-test-0000".to_string()),
            "AWS_SECRET_ACCESS_KEY" => Some("aws".to_string()),
            "OPENAI_API_KEY" => Some(String::new()),
            _ => None,
        };
        let names = [
            "ANTHROPIC_API_KEY".to_string(),
            "OPENAI_API_KEY".to_string(),
        ];
        let (env, server) = split_env_from_host(&names, shell).unwrap();
        assert_eq!(env["ANTHROPIC_API_KEY"], "sk-ant-test-0000");
        // Unset (or empty) here: the server resolves it from the keys saved
        // in Cua Spaces or its own environment.
        assert_eq!(server, vec!["OPENAI_API_KEY"]);
        let e = split_env_from_host(&["AWS_SECRET_ACCESS_KEY".into()], shell).unwrap_err();
        assert!(
            !e.to_string().contains("aws"),
            "the value is never quoted: {e}"
        );
        assert!(e.to_string().contains("not a provider key variable"));
    }

    #[test]
    fn agent_run_is_a_valid_agent_start() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("brief.md");
        std::fs::write(&f, "x").unwrap();
        let a = run_args(&[
            "local:dev",
            "claude-code",
            "fix it",
            "--file",
            f.to_str().unwrap(),
            "--mcp",
            "docs=http://127.0.0.1:9/mcp",
            "--no-skills",
            "--exit-when-idle",
            "--label",
            "e2e",
            "--wire",
            "anthropic",
        ]);
        let v = start_args(&a, "local:dev").unwrap();
        // No key is named, so none is sent: the server adds the saved one.
        assert_eq!(v["env"], serde_json::json!({}));
        assert_eq!(v["env_from_host"], serde_json::json!([]));
        assert_eq!(v["skills"], false);
        assert_eq!(v["sandbox_mcp"], true);
        assert_eq!(
            v["files"][0],
            std::fs::canonicalize(&f).unwrap().to_str().unwrap()
        );
        let parsed: cua_spaces::contract::inputs::AgentStart =
            serde_json::from_value(v).expect("the contract's agent_start input");
        assert_eq!(parsed.agent, "claude-code");
        assert_eq!(
            parsed.mcp_servers[0].url.as_deref(),
            Some("http://127.0.0.1:9/mcp")
        );
        assert_eq!(parsed.label.as_deref(), Some("e2e"));
        assert_eq!(parsed.exit_when_idle, Some(true));
        let missing = run_args(&["local:dev", "claude-code", "x", "--file", "/no/such/file"]);
        assert!(start_args(&missing, "local:dev").is_err());
    }
}
