// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The persistent-agent tools: agents, routines, notifications, pause and
//! resume, and per-agent computer access (`crate::persistent`). Their
//! records are agent members of their Spaces; `persistent_agent_send` is
//! also recorded in the Space's thread.

use serde_json::{Value, json};

use crate::persistent::{AgentRecord, AgentSpec, SpacesPersistent as _, parse_schedule};
use cua_spaces::Spaces;
use cua_spaces::error::{Error, Result};
use cua_spaces::extension::Caller;
use cua_spaces::mcp::ToolOutcome;
use cua_spaces_contract::inputs as i;

/// The tools this module answers, by their current names (a call by a
/// former name, `persistent_agent_send` or `routine_add`, arrives here
/// resolved by `cua_spaces_contract::canonical`).
pub(crate) const TOOLS: &[&str] = &[
    "agent_add",
    "persistent_agent_list",
    "agent_remove",
    "agent_send",
    "persistent_agent_save",
    "agent_pause",
    "agent_resume",
    "agent_schedule_add",
    "agent_schedule_list",
    "agent_schedule_remove",
    "agent_schedule_set_enabled",
    "notify_user",
    "notifications_list",
    "notifications_ack",
    "computer_access_grant",
    "computer_access_revoke",
    "computer_access_list",
];

pub(crate) fn row(r: &AgentRecord) -> Value {
    json!({"name": r.name, "harness": r.harness, "space": r.space, "model": r.model,
        "base_url": r.base_url, "env_from_host": r.env_from_host, "paused": r.paused,
        "space_state": r.space_state, "run_id": r.run_id, "saved_ms": r.saved_ms,
        "created_ms": r.created_ms, "last_error": r.last_error})
}

pub(crate) fn routine_row(r: &cua_spaces::routines::Routine) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
    v["label"] = json!(r.schedule.label());
    v["next_fire"] = json!(
        r.next_fire(chrono::Utc::now())
            .map(|t| cua_spaces::routines::iso::format(&t))
    );
    v
}

/// The usage-telemetry action of a persistent-agent tool (reads are not
/// counted).
fn telemetry_action(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "agent_add" => "create",
        "agent_remove" => "remove",
        "agent_send" => "send",
        "agent_pause" => "pause",
        "agent_resume" => "resume",
        "agent_schedule_add" => "routine_add",
        "agent_schedule_set_enabled" => "routine_toggle",
        "agent_schedule_remove" => "routine_remove",
        "computer_access_grant" => "computer_allow",
        "computer_access_revoke" => "computer_revoke",
        _ => return None,
    })
}

pub(crate) async fn call(
    spaces: &Spaces,
    caller: &Caller,
    tool: &str,
    a: Value,
) -> Result<ToolOutcome> {
    let Some(action) = telemetry_action(tool) else {
        return call_inner(spaces, caller, tool, a).await;
    };
    // Only the harness id of a new agent (a catalog id, else `other`);
    // never its name, Space, model, endpoint or prompt.
    let harness = (action == "create")
        .then(|| a.get("agent").and_then(Value::as_str).map(str::to_string))
        .flatten();
    let r = call_inner(spaces, caller, tool, a).await;
    let ok = r.as_ref().is_ok_and(|o| !o.is_error);
    if let Some(e) = cua_telemetry::events::persistent_agent(
        action,
        harness.as_deref(),
        if ok {
            cua_telemetry::Outcome::Ok
        } else {
            cua_telemetry::Outcome::Error
        },
        (!ok).then_some("other"),
    ) {
        cua_telemetry::capture(e);
    }
    r
}

async fn call_inner(spaces: &Spaces, caller: &Caller, tool: &str, a: Value) -> Result<ToolOutcome> {
    let p = spaces.persistent();
    // Who may call it at all (a relay caller by role); the agent's own
    // rights are checked where the tool acts.
    crate::thread::policy::gate(
        caller,
        p.role_of(caller),
        tool,
        crate::thread::policy::need(tool),
    )?;
    match tool {
        "agent_add" => {
            // An agent member of its Space (persistent_agent_create is an
            // alias): the record's fields as before, plus the member's.
            let a: i::PersistentAgentCreate = crate::args(tool, a)?;
            Ok(ToolOutcome::json(
                &crate::thread_tools::agent_add(&p, caller, a).await?,
            ))
        }
        "persistent_agent_list" => {
            let _: i::PersistentAgentList = crate::args(tool, a)?;
            // Agents connected from a computer are members, not persistent
            // agents (agent_members lists them).
            let rows: Vec<Value> = p
                .list()?
                .iter()
                .filter(|r| !crate::persistent::identity::is_external(r))
                .map(row)
                .collect();
            Ok(ToolOutcome::json(&json!({"agents": rows})))
        }
        "agent_remove" => {
            let a: i::AgentRemove = crate::args(tool, a)?;
            Ok(ToolOutcome::json(
                &crate::thread_tools::agent_remove(&p, caller, a).await?,
            ))
        }
        "agent_send" => {
            let a: i::PersistentAgentSend = crate::args(tool, a)?;
            if let Some(space) = a.space.as_deref() {
                let id = spaces.resolve(space)?.to_string();
                if p.get(&a.name)?.space != id {
                    return Err(Error::invalid(format!(
                        "{} is not a member of {id}",
                        a.name
                    )));
                }
            }
            // Also recorded in the agent's Space thread, as the caller.
            Ok(ToolOutcome::json(
                &p.send_as(caller, &a.name, &a.text).await?,
            ))
        }
        "persistent_agent_save" => {
            let a: i::PersistentAgentName = crate::args(tool, a)?;
            Ok(ToolOutcome::json(&p.save(&a.name).await?))
        }
        "agent_pause" => {
            let a: i::PersistentAgentName = crate::args(tool, a)?;
            p.check_control(caller, &p.get(&a.name)?)?;
            Ok(ToolOutcome::json(&p.pause(&a.name).await?))
        }
        "agent_resume" => {
            let a: i::AgentResume = crate::args(tool, a)?;
            // Gives control back when someone took it (a member that was
            // only held keeps its Space and run as they are); else resumes.
            let rec = p.get(&a.name)?;
            p.check_control(caller, &rec)?;
            if p.give_back_control(caller, &a.name)? && !rec.paused && a.prompt.is_none() {
                return Ok(ToolOutcome::json(&crate::persistent::ResumeReport {
                    space: rec.space,
                    run_id: rec.run_id,
                    ..Default::default()
                }));
            }
            Ok(ToolOutcome::json(
                &p.resume(&a.name, a.prompt.as_deref()).await?,
            ))
        }
        "agent_schedule_add" => {
            let a: i::RoutineAdd = crate::args(tool, a)?;
            let schedule = parse_schedule(
                a.every_minutes,
                a.daily_at.as_deref(),
                a.weekly_on.as_deref(),
            )?;
            let r = p
                .routine_add(
                    &a.agent,
                    &a.title,
                    &a.prompt,
                    schedule,
                    a.enabled.unwrap_or(true),
                )
                .await?;
            Ok(ToolOutcome::json(&routine_row(&r)))
        }
        "agent_schedule_list" => {
            let a: i::RoutineList = crate::args(tool, a)?;
            let rows: Vec<Value> = p
                .routines(a.agent.as_deref())
                .await?
                .iter()
                .map(routine_row)
                .collect();
            Ok(ToolOutcome::json(&json!({"routines": rows})))
        }
        "agent_schedule_remove" => {
            let a: i::RoutineId = crate::args(tool, a)?;
            p.routine_remove(&a.id).await?;
            Ok(ToolOutcome::json(&json!({"removed": a.id})))
        }
        "agent_schedule_set_enabled" => {
            let a: i::RoutineSetEnabled = crate::args(tool, a)?;
            Ok(ToolOutcome::json(&routine_row(
                &p.routine_set_enabled(&a.id, a.enabled).await?,
            )))
        }
        "notify_user" => {
            let a: i::NotifyUser = crate::args(tool, a)?;
            let n = p.feed().post(
                a.agent.as_deref(),
                "message",
                &a.title,
                a.body.as_deref().unwrap_or(""),
                None,
                None,
            )?;
            Ok(ToolOutcome::json(&json!({"notified": true, "id": n.id})))
        }
        "notifications_list" => {
            let a: i::NotificationsList = crate::args(tool, a)?;
            let list = p.feed().list(a.unread_only.unwrap_or(false), a.since_ms)?;
            Ok(ToolOutcome::json(&json!({"notifications": list})))
        }
        "notifications_ack" => {
            let a: i::NotificationsAck = crate::args(tool, a)?;
            Ok(ToolOutcome::json(&json!({"marked": p.feed().ack(&a.ids)?})))
        }
        "computer_access_grant" => {
            let a: i::ComputerAccessGrant = crate::args(tool, a)?;
            p.get(&a.agent)?;
            let machine = spaces.resolve(&a.machine)?.to_string();
            let expires = a
                .expires_in_secs
                .map(|s| cua_volume::now_ms() + s.saturating_mul(1000));
            let g = p
                .access()
                .grant(p.drive.presence().as_ref(), &a.agent, &machine, expires)?;
            Ok(ToolOutcome::json(&g))
        }
        "computer_access_revoke" => {
            let a: i::ComputerAccessRevoke = crate::args(tool, a)?;
            let machine = match a.machine.as_deref() {
                Some(m) => Some(spaces.resolve(m)?.to_string()),
                None => None,
            };
            let n = p.access().revoke(&a.agent, machine.as_deref())?;
            Ok(ToolOutcome::json(&json!({"revoked": n})))
        }
        "computer_access_list" => {
            let a: i::ComputerAccessList = crate::args(tool, a)?;
            let grants: Vec<_> = p
                .access()
                .grants()?
                .into_iter()
                .filter(|g| a.agent.as_deref().is_none_or(|x| g.agent == x))
                .collect();
            let mut out = json!({"grants": grants});
            if let Some(n) = a.audit.filter(|n| *n > 0) {
                let (events, verdict) = p
                    .access()
                    .audit()
                    .tail(n as usize)
                    .map_err(crate::drive_err)?;
                out["audit"] = json!(events);
                out["audit_verified"] = json!(verdict.is_ok());
            }
            Ok(ToolOutcome::json(&out))
        }
        other => Err(Error::NotFound(format!("tool {other}"))),
    }
}

/// `agent_start` with `home`: the persistent agent `name` (created with
/// this harness and Space when new) gets a run with its home restored.
pub(crate) async fn start_with_home(
    spaces: &Spaces,
    s: &cua_spaces::Space,
    a: Value,
) -> Result<ToolOutcome> {
    let a: i::AgentStart = crate::args("agent_start", a)?;
    let name = a
        .home
        .clone()
        .ok_or_else(|| Error::invalid("agent_start: `home` is required"))?;
    let name = name.as_str();
    let p = spaces.persistent();
    let space = s.id().to_string();
    let rec = match p.get(name) {
        Ok(r) => r,
        Err(Error::NotFound(_)) => p.create(AgentSpec {
            name: name.into(),
            harness: a.agent.clone(),
            space: space.clone(),
            model: a.model.clone(),
            base_url: a.base_url.clone(),
            env_from_host: a.env_from_host.clone(),
            env: a.env.clone(),
        })?,
        Err(e) => return Err(e),
    };
    if rec.harness != a.agent {
        return Err(Error::invalid(format!(
            "persistent agent {name} runs {}, not {}",
            rec.harness, a.agent
        )));
    }
    if rec.space != space {
        return Err(Error::invalid(format!(
            "persistent agent {name} works in {}, not {space}",
            rec.space
        )));
    }
    for (what, set) in [
        ("repo", a.repo.is_some()),
        ("cwd", a.cwd.is_some()),
        ("mcp_servers", !a.mcp_servers.is_empty()),
    ] {
        if set {
            return Err(Error::invalid(format!(
                "{what} is not supported with home (a persistent agent works in its home)"
            )));
        }
    }
    let d = p.start(name, &a.prompt).await?;
    Ok(ToolOutcome::json(&json!({
        "run_id": d.run_id, "agent": rec.harness, "space": space, "home": name,
        "restored": d.restored,
        "hint": "the daemon saves the home after every turn and notifies the user; follow the run with agent_events, continue it with persistent_agent_send",
    })))
}
