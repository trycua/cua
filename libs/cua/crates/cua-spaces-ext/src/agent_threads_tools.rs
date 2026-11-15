// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `agent_threads` and `agent_thread_read`: the coding agents' own recent
//! chats on this computer, for "Pick up where you left off" (CUA-3319).
//!
//! Both are person-only tools (no MCP server lists or runs them), and they
//! answer only this computer's own user: a relay caller, an agent member
//! and an agent's own process are refused, so a chat never leaves this
//! computer before the person picked it. Reading is `cua_agent_threads`,
//! read-only, on a blocking thread.

use serde_json::{Value, json};

use crate::thread::policy::denied;
use cua_agent_threads::{Agent, ListOptions, Roots};
use cua_spaces::error::{Error, Result};
use cua_spaces::extension::Caller;
use cua_spaces::mcp::ToolOutcome;
use cua_spaces_contract::inputs as i;

pub(crate) const TOOLS: &[&str] = &["agent_threads", "agent_thread_read"];

pub(crate) async fn call(caller: &Caller, tool: &str, a: Value) -> Result<ToolOutcome> {
    own_user_only(caller, tool)?;
    let roots = Roots::from_env();
    match tool {
        "agent_threads" => {
            let a: i::AgentThreads = crate::args(tool, a)?;
            let agents = parse_agents(&a.agents)?;
            let opts = ListOptions {
                agents,
                limit: a
                    .limit
                    .unwrap_or(4)
                    .clamp(1, cua_agent_threads::RECENT_MAX as u32) as usize,
            };
            let threads =
                blocking(move || Ok(cua_agent_threads::recent_cached(&roots, &opts))).await?;
            Ok(ToolOutcome::json(&json!({ "threads": threads })))
        }
        "agent_thread_read" => {
            let a: i::AgentThreadRead = crate::args(tool, a)?;
            let agent = Agent::parse(&a.agent).ok_or_else(|| {
                Error::invalid(format!(
                    "agent: {:?} is not claude-code, openai-codex or hermes",
                    a.agent
                ))
            })?;
            let limit = a.limit.map(|n| n as usize);
            let id = a.id;
            let h = blocking(move || {
                cua_agent_threads::history(&roots, agent, &id, limit).map_err(|e| match e {
                    cua_agent_threads::Error::NotFound { .. } => Error::NotFound(e.to_string()),
                    cua_agent_threads::Error::Invalid(m) => Error::invalid(m),
                    other => Error::extension("agent_threads", other.to_string()),
                })
            })
            .await?;
            Ok(ToolOutcome::json(&h))
        }
        _ => Err(Error::invalid(format!("{tool}: not an agent_threads tool"))),
    }
}

/// Only this computer's own user, never an agent (CUA-3319 privacy defaults).
fn own_user_only(caller: &Caller, tool: &str) -> Result<()> {
    if *caller != Caller::Owner || cua_sandbox_core::byoc::cua_agent().is_some() {
        return Err(denied(format!(
            "{tool}: only this computer's own user can read its coding agents' chats"
        )));
    }
    Ok(())
}

fn parse_agents(ids: &[String]) -> Result<Vec<Agent>> {
    if ids.is_empty() {
        return Ok(Agent::ALL.to_vec());
    }
    ids.iter()
        .map(|s| {
            Agent::parse(s).ok_or_else(|| {
                Error::invalid(format!(
                    "agents: {s:?} is not claude-code, openai-codex or hermes"
                ))
            })
        })
        .collect()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::extension("agent_threads", e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_anyone_but_this_computers_user() {
        let relay = Caller::Relay {
            account: "acct_x".into(),
            name: None,
            role: "owner".into(),
        };
        assert!(own_user_only(&relay, "agent_threads").is_err());
        let agent = Caller::Agent {
            name: "bot".into(),
            space: "s".into(),
        };
        assert!(own_user_only(&agent, "agent_thread_read").is_err());
    }

    #[test]
    fn agents_default_to_all_three() {
        assert_eq!(parse_agents(&[]).unwrap(), Agent::ALL.to_vec());
        assert_eq!(parse_agents(&["codex".into()]).unwrap(), vec![Agent::Codex]);
        assert!(parse_agents(&["cursor".into()]).is_err());
    }

    #[test]
    fn they_are_person_only_tools() {
        for t in TOOLS {
            assert!(cua_spaces_contract::person_only(t), "{t}");
            assert!(cua_spaces_contract::tool(t).is_some(), "{t}");
        }
    }
}
