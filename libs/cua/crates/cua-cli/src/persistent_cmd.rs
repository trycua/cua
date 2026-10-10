//! `cua agent create|tell|pause|resume|...`: persistent agents, their
//! routines, notifications and access to your computers.
//!
//! ```text
//! cua agent create ada --harness hermes --in local:dev --env-from-host OPENAI_API_KEY
//! cua agent tell ada "Every morning, summarize what changed in the repo"
//! cua agent routine add ada --title "Morning sweep" --prompt "Summarize overnight changes" --daily 08:00
//! cua agent pause ada
//! cua agent resume ada
//! cua agent notifications --unread
//! ```

use crate::util::{json_line, line};
use clap::Subcommand;
use cua_sdk::{Cua, CuaError, PersistentAgentOptions};
use std::io::Write;
use std::sync::Arc;

/// Persistent-agent commands (flattened into `cua agent`).
#[derive(Subcommand, Debug, Clone)]
pub enum PersistentCmd {
    /// Create a persistent agent: a named harness whose memory lives in the
    /// Cua Volume (agents/NAME/) and outlives its Space.
    #[command(after_help = "Examples:
  cua agent create ada --harness hermes --in local:dev --env-from-host OPENAI_API_KEY
  cua agent create scout --harness claude-code --in cloud:scout --env-from-host ANTHROPIC_API_KEY")]
    Create {
        name: String,
        /// Harness id (`cua agent harnesses`).
        #[arg(long)]
        harness: String,
        /// The Space it works in.
        #[arg(long = "in", value_name = "SPACE")]
        space: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        /// Provider key variable to forward at each start. Repeatable.
        #[arg(long = "env-from-host")]
        env_from_host: Vec<String>,
    },
    /// List persistent agents.
    #[command(after_help = "Examples:
  cua agent persistent")]
    Persistent,
    /// Give a persistent agent a turn (starts a run with its home restored,
    /// or follows up on its idle run).
    #[command(after_help = "Examples:
  cua agent tell ada \"what did you learn yesterday?\"")]
    Tell { name: String, text: String },
    /// Save a persistent agent's home into the drive now.
    #[command(after_help = "Examples:
  cua agent save ada")]
    Save { name: String },
    /// Pause a persistent agent: its run, its routines, and its Space
    /// (suspended when local; a cloud Space is released after the home is
    /// saved).
    #[command(after_help = "Examples:
  cua agent pause ada")]
    Pause { name: String },
    /// Resume a paused persistent agent.
    #[command(after_help = "Examples:
  cua agent resume ada
  cua agent resume ada --prompt \"pick up where you left off\"")]
    Resume {
        name: String,
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Forget a persistent agent (its home stays in the drive).
    #[command(after_help = "Examples:
  cua agent forget ada")]
    Forget { name: String },
    /// A persistent agent's routines, fired by the daemon with or without
    /// the app open.
    #[command(subcommand)]
    Routine(RoutineCmd),
    /// Let a persistent agent use one of your computers (asks for
    /// presence).
    #[command(after_help = "Examples:
  cua agent allow-computer ada relay:0123456789abcdef")]
    AllowComputer {
        name: String,
        /// The machine's Space id (`cua host status` shows this machine's).
        machine: String,
        /// End the grant after this many seconds.
        #[arg(long = "for", value_name = "SECS")]
        for_secs: Option<u64>,
    },
    /// Take a persistent agent's computer access back.
    #[command(after_help = "Examples:
  cua agent revoke-computer ada
  cua agent revoke-computer ada relay:0123456789abcdef")]
    RevokeComputer {
        name: String,
        machine: Option<String>,
    },
    /// Which agents may use which computers, and the audit log.
    #[command(after_help = "Examples:
  cua agent computer-access --audit 20")]
    ComputerAccess {
        name: Option<String>,
        /// Also print this many audit entries.
        #[arg(long, default_value_t = 0)]
        audit: u32,
    },
    /// The notifications the Cua app shows (turn ends, notify_user,
    /// requests).
    #[command(after_help = "Examples:
  cua agent notifications --unread
  cua agent notifications --mark-read")]
    Notifications {
        #[arg(long)]
        unread: bool,
        /// Mark every notification read.
        #[arg(long)]
        mark_read: bool,
    },
}

/// `cua agent routine ...`.
#[derive(Subcommand, Debug, Clone)]
pub enum RoutineCmd {
    /// Add a routine (give one of --every, --daily, --weekly).
    #[command(after_help = "Examples:
  cua agent routine add ada --title \"Inbox sweep\" --prompt \"Triage new issues\" --every 60
  cua agent routine add ada --title \"Morning\" --prompt \"Plan the day\" --daily 08:00")]
    Add {
        agent: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        prompt: String,
        /// Every N minutes.
        #[arg(long, value_name = "MINUTES")]
        every: Option<i64>,
        /// Every day at HH:MM.
        #[arg(long, value_name = "HH:MM")]
        daily: Option<String>,
        /// Every week, `mon 09:00`.
        #[arg(long, value_name = "DAY HH:MM")]
        weekly: Option<String>,
    },
    /// List routines.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua agent routine ls
  cua agent routine ls ada"
    )]
    Ls { agent: Option<String> },
    /// Delete a routine.
    #[command(after_help = "Examples:
  cua agent routine rm 0F3A2C1E-8B7D-4E6F-9A1B-2C3D4E5F6A7B")]
    Rm { id: String },
    /// Turn a routine on.
    #[command(after_help = "Examples:
  cua agent routine enable 0F3A2C1E-8B7D-4E6F-9A1B-2C3D4E5F6A7B")]
    Enable { id: String },
    /// Turn a routine off.
    #[command(after_help = "Examples:
  cua agent routine disable 0F3A2C1E-8B7D-4E6F-9A1B-2C3D4E5F6A7B")]
    Disable { id: String },
}

fn ago(ms: u64) -> String {
    if ms == 0 {
        return "never".into();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ms);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

pub async fn run(
    cua: &Arc<Cua>,
    cmd: PersistentCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let spaces = cua.spaces();
    let show = |out: &mut dyn Write, v: serde_json::Value| json_line(out, &v);
    match cmd {
        PersistentCmd::Create {
            name,
            harness,
            space,
            model,
            base_url,
            env_from_host,
        } => {
            let a = spaces
                .persistent_agent_create(
                    name,
                    harness,
                    space,
                    Some(PersistentAgentOptions {
                        model,
                        base_url,
                        env_from_host,
                        env: Default::default(),
                    }),
                )
                .await?;
            if json {
                show(
                    out,
                    serde_json::json!({"name": a.name, "harness": a.harness, "space": a.space}),
                );
            } else {
                line(
                    out,
                    format!(
                        "Created {} ({} in {}). Its home is agents/{}/ in the Cua Volume.",
                        a.name, a.harness, a.space, a.name
                    ),
                );
            }
        }
        PersistentCmd::Persistent => {
            let all = spaces.persistent_agents().await?;
            if json {
                let rows: Vec<_> = all
                    .iter()
                    .map(|a| {
                        serde_json::json!({"name": a.name, "harness": a.harness, "space": a.space,
                        "paused": a.paused, "space_state": a.space_state, "run_id": a.run_id,
                        "saved_ms": a.saved_ms, "last_error": a.last_error})
                    })
                    .collect();
                show(out, serde_json::json!({"agents": rows}));
            } else if all.is_empty() {
                line(
                    out,
                    "No persistent agents. Create one with `cua agent create`.",
                );
            } else {
                for a in all {
                    let state = if a.paused {
                        "paused"
                    } else if a.run_id.is_some() {
                        "running"
                    } else {
                        "idle"
                    };
                    line(
                        out,
                        format!(
                            "{:<16} {:<14} {:<8} {:<24} saved {}",
                            a.name,
                            a.harness,
                            state,
                            a.space,
                            ago(a.saved_ms)
                        ),
                    );
                }
            }
        }
        PersistentCmd::Tell { name, text } => {
            let d = spaces.persistent_agent_send(name.clone(), text).await?;
            if json {
                show(
                    out,
                    serde_json::json!({"run_id": d.run_id, "started": d.started}),
                );
            } else if d.started {
                let r = d.restored.unwrap_or_default();
                line(
                    out,
                    format!(
                        "Started {name} ({}), home restored: {} files, {} bytes.",
                        d.run_id, r.files, r.bytes
                    ),
                );
            } else {
                line(out, format!("Sent to {name} ({}).", d.run_id));
            }
        }
        PersistentCmd::Save { name } => {
            let t = spaces.persistent_agent_save(name.clone()).await?;
            if json {
                show(
                    out,
                    serde_json::json!({"files": t.files, "bytes": t.bytes, "unchanged": t.unchanged,
                    "removed": t.removed, "blocked": t.blocked, "millis": t.millis}),
                );
            } else {
                line(
                    out,
                    format!(
                        "Saved {name}: {} files changed, {} bytes, {} ms.",
                        t.files, t.bytes, t.millis
                    ),
                );
                for b in &t.blocked {
                    line(out, format!("  not saved (looks like a secret): {b}"));
                }
            }
        }
        PersistentCmd::Pause { name } => {
            let r = spaces.agent_pause(name.clone()).await?;
            if json {
                show(
                    out,
                    serde_json::json!({"stopped_run": r.stopped_run, "space_state": r.space_state, "millis": r.millis}),
                );
            } else {
                let space = match r.space_state.as_str() {
                    "suspended" => "Space suspended",
                    "released" => "cloud Space released (resume creates it again)",
                    _ => "Space left running",
                };
                line(out, format!("Paused {name}: home saved, {space}."));
            }
        }
        PersistentCmd::Resume { name, prompt } => {
            let r = spaces.agent_resume(name.clone(), prompt).await?;
            if json {
                show(
                    out,
                    serde_json::json!({"space": r.space, "recreated": r.recreated,
                    "run_id": r.run_id, "ready_ms": r.ready_ms}),
                );
            } else {
                line(
                    out,
                    format!(
                        "Resumed {name} in {} ({} ms){}.",
                        r.space,
                        r.ready_ms,
                        r.run_id.map(|id| format!(", run {id}")).unwrap_or_default()
                    ),
                );
            }
        }
        PersistentCmd::Forget { name } => {
            spaces.persistent_agent_remove(name.clone()).await?;
            if json {
                show(out, serde_json::json!({"removed": name}));
            } else {
                line(
                    out,
                    format!(
                        "Forgot {name}. Its home is still in the Cua Volume at agents/{name}/."
                    ),
                );
            }
        }
        PersistentCmd::Routine(r) => {
            let on = !matches!(r, RoutineCmd::Disable { .. });
            match r {
                RoutineCmd::Add {
                    agent,
                    title,
                    prompt,
                    every,
                    daily,
                    weekly,
                } => {
                    let r = spaces
                        .routine_add(agent, title, prompt, every, daily, weekly)
                        .await?;
                    if json {
                        show(
                            out,
                            serde_json::json!({"id": r.id, "label": r.label, "next_fire": r.next_fire}),
                        );
                    } else {
                        line(out, format!("Added {} ({}): {}.", r.title, r.id, r.label));
                    }
                }
                RoutineCmd::Ls { agent } => {
                    let all = spaces.routines(agent).await?;
                    if json {
                        let rows: Vec<_> = all
                            .iter()
                            .map(|r| {
                                serde_json::json!({"id": r.id, "agent": r.agent, "title": r.title,
                                    "label": r.label, "enabled": r.enabled, "next_fire": r.next_fire,
                                    "last_outcome": r.last_outcome})
                            })
                            .collect();
                        show(out, serde_json::json!({"routines": rows}));
                    } else if all.is_empty() {
                        line(out, "No routines.");
                    } else {
                        for r in all {
                            line(
                                out,
                                format!(
                                    "{}  {:<12} {:<24} {}{}",
                                    r.id,
                                    r.agent,
                                    r.title,
                                    r.label,
                                    if r.enabled { "" } else { " (off)" }
                                ),
                            );
                        }
                    }
                }
                RoutineCmd::Rm { id } => {
                    spaces.routine_remove(id.clone()).await?;
                    line(
                        out,
                        if json {
                            serde_json::json!({"removed": id}).to_string()
                        } else {
                            format!("Removed {id}.")
                        },
                    );
                }
                RoutineCmd::Enable { id } | RoutineCmd::Disable { id } => {
                    let r = spaces.routine_set_enabled(id, on).await?;
                    line(
                        out,
                        if json {
                            serde_json::json!({"id": r.id, "enabled": r.enabled}).to_string()
                        } else {
                            format!("{} is {}.", r.title, if r.enabled { "on" } else { "off" })
                        },
                    );
                }
            }
        }
        PersistentCmd::AllowComputer {
            name,
            machine,
            for_secs,
        } => {
            let g = spaces
                .computer_access_grant(name, machine, for_secs)
                .await?;
            if json {
                show(
                    out,
                    serde_json::json!({"id": g.id, "agent": g.agent, "machine": g.machine, "expires_ms": g.expires_ms}),
                );
            } else {
                line(out, format!("{} may use {}.", g.agent, g.machine));
            }
        }
        PersistentCmd::RevokeComputer { name, machine } => {
            let n = spaces.computer_access_revoke(name.clone(), machine).await?;
            line(
                out,
                if json {
                    serde_json::json!({"revoked": n}).to_string()
                } else {
                    format!("Revoked {n} grant(s) of {name}.")
                },
            );
        }
        PersistentCmd::ComputerAccess { name, audit } => {
            let v = spaces
                .call_tool_json(
                    "computer_access_list".into(),
                    Some(serde_json::json!({"agent": name, "audit": audit}).to_string()),
                )
                .await?;
            if v.is_error {
                return Err(CuaError::InvalidArgument(v.text));
            }
            let body: serde_json::Value = serde_json::from_str(&v.text).unwrap_or_default();
            if json {
                show(out, body);
            } else {
                let grants = body["grants"].as_array().cloned().unwrap_or_default();
                let live: Vec<_> = grants.iter().filter(|g| g["revoked"] != true).collect();
                if live.is_empty() {
                    line(out, "No agent may use your computers.");
                }
                for g in live {
                    line(
                        out,
                        format!(
                            "{:<16} {}",
                            g["agent"].as_str().unwrap_or(""),
                            g["machine"].as_str().unwrap_or("")
                        ),
                    );
                }
                for e in body["audit"].as_array().into_iter().flatten() {
                    line(
                        out,
                        format!(
                            "  {} {:<8} {} {}",
                            ago(e["ts_ms"].as_u64().unwrap_or(0)),
                            e["action"].as_str().unwrap_or(""),
                            e["principal"].as_str().unwrap_or(""),
                            e["path"].as_str().unwrap_or("")
                        ),
                    );
                }
            }
        }
        PersistentCmd::Notifications { unread, mark_read } => {
            let all = spaces.notifications(unread, None).await?;
            if mark_read {
                spaces.notifications_ack(vec![]).await?;
            }
            if json {
                let rows: Vec<_> = all.iter().map(|n| serde_json::json!({"id": n.id, "at_ms": n.at_ms,
                    "agent": n.agent, "kind": n.kind, "title": n.title, "body": n.body, "read": n.read})).collect();
                show(out, serde_json::json!({"notifications": rows}));
            } else if all.is_empty() {
                line(out, "No notifications.");
            } else {
                for n in all {
                    let body: String = n
                        .body
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(80)
                        .collect();
                    line(
                        out,
                        format!(
                            "{:<8} {}{}  {}",
                            ago(n.at_ms),
                            if n.read { "" } else { "* " },
                            n.title,
                            body
                        ),
                    );
                }
            }
        }
    }
    Ok(0)
}
