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

pub async fn run(
    cua: &Arc<Cua>,
    cmd: AgentCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    if let AgentCmd::Persistent(p) = cmd {
        return crate::persistent_cmd::run(cua, p, json, out).await;
    }
    if let AgentCmd::Harnesses = cmd {
        let h = cua_sdk::agent_harnesses();
        if json {
            line(out, h);
        } else {
            let v: serde_json::Value = serde_json::from_str(&h).unwrap_or_default();
            for x in v.as_array().into_iter().flatten() {
                line(
                    out,
                    format!(
                        "{:<20} {:<6} keys: {}",
                        x["id"].as_str().unwrap_or(""),
                        if x["ready"] == true { "ready" } else { "no" },
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
            }
        }
        return Ok(0);
    }
    let (at, sandbox) = match &cmd {
        AgentCmd::Run(a) => (a.at, a.sandbox.clone()),
        AgentCmd::Ls { at, sandbox }
        | AgentCmd::Logs { at, sandbox, .. }
        | AgentCmd::Send { at, sandbox, .. }
        | AgentCmd::Interrupt { at, sandbox, .. }
        | AgentCmd::Stop { at, sandbox, .. }
        | AgentCmd::Status { at, sandbox, .. }
        | AgentCmd::Rm { at, sandbox, .. }
        | AgentCmd::Ensure { at, sandbox, .. } => (*at, sandbox.clone()),
        AgentCmd::Harnesses | AgentCmd::Persistent(_) => unreachable!(),
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
        AgentCmd::Run(a) => {
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
                    serde_json::json!({"run_id": run.run_id(), "harness": run.harness()})
                        .to_string(),
                );
            } else {
                eprintln!(
                    "{} started ({}); follow with: cua agent logs {sandbox} {} -f",
                    run.run_id(),
                    run.harness(),
                    run.run_id()
                );
            }
            if a.follow {
                follow(&run, 0, true, false, json, out).await?;
                let r = run.result().await?;
                return Ok(if r.error.is_some() || r.status == "failed" {
                    1
                } else {
                    0
                });
            }
        }
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
        AgentCmd::Harnesses | AgentCmd::Persistent(_) => {}
    }
    Ok(0)
}
