// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the phone app asks of a machine's daemon, beyond the Space thread
//! itself: tool rows, starting a task, a Space's changes, and the standing
//! answers of "allow for this Space".
//!
//! These are wire calls of the paired phone, reached the way
//! `thread_read` is: `HostThreadService.CallThreadTool` through the relay,
//! as the account the relay verified. They are not Spaces tools: no MCP
//! server lists them, so an agent cannot call them, and each is gated by
//! the caller's role like the thread tools ([`policy::need`]). The phone
//! never gets a broader power than the account's own app: a call here is
//! something the desktop app does with the same daemon, and the daemon
//! decides by the verified role, not by who asked.
//!
//! Nothing here depends on how a Space runs: every read of a Space goes
//! through its agent service or its shell, and every create through the
//! daemon's own create.

pub mod changes;
pub mod rows;
pub mod sent;
pub mod task;

use serde_json::{Value, json};

use crate::thread::policy::{self, Need};
use cua_spaces::extension::Caller;
use cua_spaces::mcp::ToolOutcome;
use cua_spaces::{Error, Result, Spaces};

use super::{Persistent, SpacesPersistent as _};

/// The calls this module answers.
pub const TOOLS: &[&str] = &[
    "thread_tool_rows",
    "task_start",
    "task_status",
    "task_options",
    "space_changes",
    "space_file_diff",
    "space_file_preview",
    "thread_grants",
    "thread_grant_revoke",
];

/// What `tool` needs from a relay caller before it runs.
pub fn need(tool: &str) -> Need {
    match tool {
        "thread_tool_rows" | "thread_grants" => Need::Read,
        // Changes show the Space's files: editors, not guests.
        "task_start" | "task_status" | "task_options" | "space_changes" | "space_file_diff"
        | "space_file_preview" => Need::Write,
        // Taking back a standing answer is the owner's or an admin's.
        "thread_grant_revoke" => Need::Admin,
        _ => Need::Owner,
    }
}

/// The phone's calls and the phone's extras on the thread tools (`client_id`,
/// an answer's `scope`) are the experiment `mobile`: off, they are refused in
/// the experiment's own words and nothing about the thread changes.
pub fn require_mobile() -> Result<()> {
    if cua_spaces::experiments::enabled(cua_spaces::experiments::Gate::Mobile) {
        Ok(())
    } else {
        Err(Error::invalid(
            cua_spaces::experiments::Gate::Mobile.off_message(),
        ))
    }
}

pub(crate) async fn call(
    spaces: &Spaces,
    caller: &Caller,
    tool: &str,
    a: Value,
) -> Result<ToolOutcome> {
    require_mobile()?;
    let p = spaces.persistent();
    policy::gate(caller, p.role_of(caller), tool, need(tool))?;
    match tool {
        "thread_tool_rows" => rows::tool_rows(&p, caller, a).await,
        "task_start" => task::task_start(spaces, &p, caller, a).await,
        "task_status" => task::task_status(&p, caller, a).await,
        "task_options" => task::task_options(spaces, caller).await,
        "space_changes" => changes::space_changes(spaces, &p, caller, a).await,
        "space_file_diff" => changes::file_diff(spaces, &p, caller, a).await,
        "space_file_preview" => changes::file_preview(spaces, &p, caller, a).await,
        "thread_grants" => grants(&p, caller, a).await,
        "thread_grant_revoke" => grant_revoke(&p, caller, a).await,
        other => Err(Error::NotFound(format!("tool {other}"))),
    }
}

/// `thread_grants`: the standing answers of a Space.
async fn grants(p: &Persistent, caller: &Caller, a: Value) -> Result<ToolOutcome> {
    let space = a
        .get("space")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid("thread_grants: `space` is required"))?;
    let t = p.thread(caller, space).await?;
    let all = crate::thread::grants::list(t.store())?;
    Ok(ToolOutcome::json(&json!({
        "grants": all.iter().map(|g| json!({
            "agent": g.agent, "action": g.action, "approval": g.approval,
            "by": g.by_name, "ts_ms": g.ts_ms,
            "device": g.device, "from_phone": g.from_phone,
        })).collect::<Vec<_>>(),
    })))
}

/// `thread_grant_revoke`: the agent asks again next time.
async fn grant_revoke(p: &Persistent, caller: &Caller, a: Value) -> Result<ToolOutcome> {
    let get = |k: &str| {
        a.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::invalid(format!("thread_grant_revoke: `{k}` is required")))
    };
    let (space, agent, action) = (get("space")?, get("agent")?, get("action")?);
    let t = p.thread(caller, space).await?;
    let gone = crate::thread::grants::revoke(t.store(), agent, action)?;
    Ok(ToolOutcome::json(&json!({"revoked": gone})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_routed_names_are_the_ones_served() {
        assert_eq!(TOOLS, cua_spaces::thread_route::PHONE_TOOLS);
        // None of them is a Spaces tool an agent could be given.
        for t in TOOLS {
            assert!(cua_spaces_contract::tool(t).is_none(), "{t}");
        }
    }

    #[test]
    fn each_call_needs_the_role_it_should() {
        assert_eq!(need("thread_tool_rows"), Need::Read);
        assert_eq!(need("task_start"), Need::Write);
        assert_eq!(need("space_changes"), Need::Write);
        assert_eq!(need("thread_grant_revoke"), Need::Admin);
        // A name nobody gated is the owner's.
        assert_eq!(need("something_new"), Need::Owner);
        for t in TOOLS {
            assert_ne!(need(t), Need::Owner, "{t} has no explicit role");
        }
    }
}
