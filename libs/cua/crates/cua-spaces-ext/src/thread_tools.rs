// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Space thread tools (`thread_*`, `agent_members`), the member side
//! of `agent_add` and `agent_remove` (answered in `persistent_tools`), and the operation the run tools call with a member's `name`
//! ([`cua_spaces::agents::MEMBER_OP`]). The caller comes from the transport
//! ([`Caller`]), never from the arguments.

use std::time::Duration;

use serde_json::{Value, json};

use crate::persistent::members::{ImportEntry, MemberOptions, PostArgs, may_manage, member_row};
use crate::persistent::{AgentSpec, Persistent, SpacesPersistent as _, parse_schedule};
use crate::thread::policy::{self, Rights};
use cua_agents::RunStatus;
use cua_spaces::Spaces;
use cua_spaces::error::{Error, Result};
use cua_spaces::extension::Caller;
use cua_spaces::mcp::ToolOutcome;
use cua_spaces_contract::inputs as i;

/// The tools this module answers.
pub(crate) const TOOLS: &[&str] = &[
    "thread_read",
    "thread_post",
    "thread_inbox",
    "thread_claim",
    "thread_reply",
    "thread_import",
    "agent_members",
    "agent_set_rights",
    "thread_join",
    "thread_join_code_create",
    "thread_join_code_list",
    "thread_join_code_cancel",
    "thread_join_redeem",
    "thread_settings",
    "thread_device_revoke",
];

pub(crate) async fn call(
    spaces: &Spaces,
    caller: &Caller,
    tool: &str,
    a: Value,
) -> Result<ToolOutcome> {
    let p = spaces.persistent();
    if tool != cua_spaces::agents::MEMBER_OP {
        policy::gate(caller, p.role_of(caller), tool, policy::need(tool))?;
    }
    match tool {
        "thread_read" => {
            let a: i::ThreadRead = crate::args(tool, a)?;
            let t = p.thread(caller, &a.space).await?;
            if a.after.is_some() && a.before.is_some() {
                return Err(Error::invalid("give `after` or `before`, not both"));
            }
            let space = t.store().space().to_string();
            if let (Some(after), Some(ms)) = (a.after, a.follow_ms.filter(|ms| *ms > 0)) {
                // A follow also returns when an agent's live stage changed
                // from the one the reader last saw (`live_after`).
                match a.live_after.as_deref() {
                    Some(seen) => {
                        t.store()
                            .wait_after_or(after, Duration::from_millis(ms), || {
                                p.live_rows(&space).is_ok_and(|(key, _)| key != seen)
                            })
                            .await?;
                    }
                    None => {
                        t.store()
                            .wait_after(after, Duration::from_millis(ms))
                            .await?;
                    }
                }
            }
            let mut out = t.read(
                a.after,
                a.before,
                a.limit.unwrap_or(100) as usize,
                a.mark_read.unwrap_or(false),
                a.with.as_deref().filter(|w| !w.is_empty()),
            )?;
            // What each agent is doing now: the thread's live rows.
            if let Ok((key, agents)) = p.live_rows(&space) {
                out["live"] = json!({"key": key, "agents": agents});
            }
            Ok(ToolOutcome::json(&out))
        }
        "thread_post" => {
            let a: i::ThreadPost = crate::args(tool, a)?;
            let t = p.thread(caller, &a.space).await?;
            let e = t
                .post(PostArgs {
                    text: a.text,
                    reply_to: a.reply_to,
                    attachments: a.attachments,
                    approval: a
                        .approval
                        .map(|x| (x.action, x.details.unwrap_or_default())),
                    answer: a
                        .answer
                        .map(|x| (x.approval, x.approve, x.note.unwrap_or_default())),
                    to_session: a.to_session,
                    view: a.view.map(|v| crate::thread::ViewContext {
                        kind: v.kind,
                        app: v.app,
                        title: v.title,
                        window_id: v.window_id,
                    }),
                })
                .await?;
            Ok(ToolOutcome::json(&json!({"entry": e})))
        }
        "thread_inbox" => {
            let a: i::ThreadInbox = crate::args(tool, a)?;
            let t = p.thread(caller, &a.space).await?;
            let wait = Duration::from_millis(a.wait_ms.unwrap_or(0)).min(crate::thread::MAX_FOLLOW);
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let tasks = t.inbox(a.member.as_deref())?;
                let now = tokio::time::Instant::now();
                if !tasks.is_empty() || now >= deadline {
                    return Ok(ToolOutcome::json(&json!({"tasks": tasks})));
                }
                let last = t.store().last_seq()?;
                t.store().wait_after(last, deadline - now).await?;
            }
        }
        "thread_join" => {
            let a: i::ThreadJoin = crate::args(tool, a)?;
            Ok(ToolOutcome::json(
                &p.thread_join(
                    caller,
                    &a.space,
                    &a.name,
                    a.harness.as_deref(),
                    a.version.as_deref(),
                    a.icon.as_deref(),
                )
                .await?,
            ))
        }
        "thread_join_code_create" => {
            let a: i::ThreadJoinCodeCreate = crate::args(tool, a)?;
            Ok(ToolOutcome::json(&p.join_code_create(
                caller,
                &a.space,
                a.ttl_s,
                a.can_task.as_deref(),
                a.ask_before_join,
            )?))
        }
        "thread_join_code_list" => {
            let a: i::ThreadJoinCodeList = crate::args(tool, a)?;
            Ok(ToolOutcome::json(&p.join_code_list(caller, &a.space)?))
        }
        "thread_join_code_cancel" => {
            let a: i::ThreadJoinCodeCancel = crate::args(tool, a)?;
            Ok(ToolOutcome::json(
                &p.join_code_cancel(caller, &a.space, &a.id)?,
            ))
        }
        "thread_join_redeem" => {
            let a: i::ThreadJoinRedeem = crate::args(tool, a)?;
            let device = crate::persistent::join::DeviceArg {
                id: a.device.id,
                name: a.device.name,
                public_key: a.device.public_key,
            };
            Ok(ToolOutcome::json(&p.redeem(caller, &a.code, device).await?))
        }
        "thread_settings" => {
            let a: i::ThreadSettings = crate::args(tool, a)?;
            Ok(ToolOutcome::json(&p.thread_settings(
                caller,
                &a.space,
                a.ask_before_join,
            )?))
        }
        "thread_device_revoke" => {
            let a: i::ThreadDeviceRevoke = crate::args(tool, a)?;
            Ok(ToolOutcome::json(
                &p.revoke_device(caller, &a.device).await?,
            ))
        }
        "thread_claim" => {
            let a: i::ThreadClaim = crate::args(tool, a)?;
            let t = p.thread(caller, &a.space).await?;
            Ok(ToolOutcome::json(&t.claim(
                a.task,
                a.member.as_deref(),
                a.session,
            )?))
        }
        "thread_reply" => {
            let a: i::ThreadReply = crate::args(tool, a)?;
            let t = p.thread(caller, &a.space).await?;
            let (reply, update) = t
                .reply(a.task, &a.text, &a.attachments, a.state.as_deref())
                .await?;
            Ok(ToolOutcome::json(
                &json!({"reply": reply, "update": update}),
            ))
        }
        "thread_import" => {
            let a: i::ThreadImport = crate::args(tool, a)?;
            let t = p.thread(caller, &a.space).await?;
            let entries = a
                .entries
                .into_iter()
                .map(|e| ImportEntry {
                    import_id: e.import_id,
                    ts_ms: e.ts_ms,
                    author_kind: e.author_kind,
                    agent_name: e.agent_name,
                    text: e.text,
                    kind: e.kind,
                })
                .collect();
            Ok(ToolOutcome::json(&t.import(&a.source, entries)?))
        }
        "agent_members" => {
            let a: i::AgentMembers = crate::args(tool, a)?;
            // Reading the member list is open to anyone in the thread.
            p.thread(caller, &a.space).await?;
            let mut rows = p.members(&a.space).await?;
            if caller.agent_member().is_some() {
                // Agents see who is here, not how each is set up.
                for row in &mut rows {
                    if let Some(o) = row.as_object_mut() {
                        for k in ["model", "policy", "run_id"] {
                            o.remove(k);
                        }
                    }
                }
            }
            let me = p.author_for(caller).id;
            // One read of the records, not one per row.
            let records = p.list().unwrap_or_default();
            for row in &mut rows {
                let name = row["name"].as_str().unwrap_or_default().to_string();
                if let Some(rec) = records.iter().find(|r| r.name == name) {
                    row["you_can"] = Persistent::rights_of(rec).you_can(p.role_of(caller), &me);
                }
            }
            // The Agents tab (CUA-2266): each agent's DM with the caller.
            if a.overview.unwrap_or(false) && caller.agent_member().is_none() {
                let t = p.thread(caller, &a.space).await?;
                let asks: Vec<(String, bool)> = rows
                    .iter()
                    .map(|row| {
                        (
                            row["name"].as_str().unwrap_or_default().to_string(),
                            row["you_can"]["approve"].as_bool().unwrap_or(true),
                        )
                    })
                    .collect();
                for (row, sum) in rows.iter_mut().zip(t.dm_summaries(&asks)?) {
                    row["dm"] = json!({"unread": sum.unread, "last": sum.last});
                    row["needs_you"] = json!(sum.needs_you);
                }
            }
            Ok(ToolOutcome::json(&json!({"members": rows})))
        }
        "agent_set_rights" => {
            let a: i::AgentSetRights = crate::args(tool, a)?;
            let v = json!({"task": a.task, "control": a.control, "approve": a.approve});
            let rec = p.set_rights(caller, &a.name, &v)?;
            let mut out = member_row(&rec, &p.owner());
            out["you_can"] =
                Persistent::rights_of(&rec).you_can(p.role_of(caller), &p.author_for(caller).id);
            Ok(ToolOutcome::json(&out))
        }
        op if op == cua_spaces::agents::MEMBER_OP => member_run(spaces, caller, a).await,
        other => Err(Error::NotFound(format!("tool {other}"))),
    }
}

/// `agent_add`: a persistent agent that is an agent member of its Space,
/// with an optional schedule. The result is the old record's fields plus
/// the member's.
pub(crate) async fn agent_add(
    p: &crate::persistent::Persistent,
    caller: &Caller,
    a: i::PersistentAgentCreate,
) -> Result<Value> {
    may_manage(caller)?;
    // Checked before anything is created.
    let rights = a
        .rights
        .as_ref()
        .map(|r| Rights::default().with(&serde_json::to_value(r).unwrap_or_default()))
        .transpose()?;
    let has_schedule = a.every_minutes.is_some() || a.daily_at.is_some() || a.weekly_on.is_some();
    let schedule = if has_schedule {
        let prompt = a
            .prompt
            .clone()
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| Error::invalid("a schedule needs `prompt`"))?;
        Some((
            parse_schedule(
                a.every_minutes,
                a.daily_at.as_deref(),
                a.weekly_on.as_deref(),
            )?,
            prompt,
        ))
    } else {
        None
    };
    let (space, durable_space) =
        Box::pin(member_space(&p.spaces, &a.space, a.on.as_deref())).await?;
    let rec = p.add_member(
        caller,
        AgentSpec {
            name: a.name,
            harness: a.agent,
            space,
            model: a.model,
            base_url: a.base_url,
            env_from_host: a.env_from_host,
            env: a.env,
        },
        MemberOptions {
            durable: a.durable.unwrap_or(false),
            on: a.on,
            once: a.once.unwrap_or(false),
            sessions: a.sessions,
            rights,
            durable_space,
        },
    )?;
    let mut out = with_member(&rec, p);
    out["status"] = json!({"state": "asleep"});
    if let Some((schedule, prompt)) = schedule {
        let title = a.title.unwrap_or_else(|| first_words(&prompt));
        let r = p
            .routine_add(&rec.name, &title, &prompt, schedule, true)
            .await?;
        out["schedule"] = crate::persistent_tools::routine_row(&r);
    }
    Ok(out)
}

/// The Space a member is added to, by `on`: `local` (or none) takes
/// `space` as it is; `auto` makes it a durable Space (an existing durable
/// Space by name, or a new one named `space`) and returns its name too; a
/// machine creates `space` there when it does not exist yet.
async fn member_space(
    spaces: &cua_spaces::Spaces,
    space: &str,
    on: Option<&str>,
) -> Result<(String, Option<String>)> {
    let on = on
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .unwrap_or("local");
    if on == "local" {
        return Ok((space.to_string(), None));
    }
    let id = spaces.resolve(space).ok().map(|i| i.to_string());
    let known = spaces
        .list()?
        .into_iter()
        .find(|i| i.durable == space || Some(&i.id) == id.as_ref());
    if on == "auto" {
        return match known {
            Some(i) if i.durable.is_empty() => Err(Error::invalid(format!(
                "on: auto needs a durable Space, and {space} is not one; name a new Space to create it as a durable Space"
            ))),
            Some(i) => Ok((i.id, Some(i.durable))),
            None => {
                let info =
                    Box::pin(crate::persistent::durable::durable_space(spaces, space)).await?;
                Ok((info.id, Some(space.to_string())))
            }
        };
    }
    if let Some(i) = known {
        return Ok((i.id, None));
    }
    let where_ = cua_sandbox_core::placement::On::parse_or_host(on)
        .map_err(|e| Error::invalid(format!("on: {e}")))?;
    let created = Box::pin(spaces.create(cua_spaces::SpaceCreate {
        name: Some(space.to_string()),
        on: Some(where_),
        wait: Some(true),
        ..Default::default()
    }))
    .await?;
    match created {
        cua_spaces::SpaceCreated::Ready { info, .. } => Ok((info.id, None)),
        cua_spaces::SpaceCreated::Starting(p) => {
            Err(Error::Timeout(format!("{} is still starting", p.id)))
        }
    }
}

/// `agent_remove`: the member leaves its Space (its run stops, its
/// schedules and open tasks end); its home stays unless `keep_home` is
/// false.
pub(crate) async fn agent_remove(
    p: &crate::persistent::Persistent,
    caller: &Caller,
    a: i::AgentRemove,
) -> Result<Value> {
    let rec = p.get(&a.name)?;
    p.may_remove(caller, &rec)?;
    let external = crate::persistent::identity::is_external(&rec);
    let rec = p.remove(&a.name).await?;
    if let Err(e) = p.note_removed(&rec, &p.author_for(caller)) {
        tracing::warn!(agent = %a.name, "no removal note: {e}");
    }
    let mut home_deleted = false;
    if !external && !a.keep_home.unwrap_or(true) {
        let home = cua_volume::path::agent_home(&a.name).map_err(crate::drive_err)?;
        p.drive
            .session(cua_volume::Context::user())
            .remove_tree(&home)
            .await
            .map_err(crate::drive_err)?;
        home_deleted = true;
    }
    Ok(json!({"removed": with_member(&rec, p), "home_deleted": home_deleted}))
}

/// The old record row with the member fields added.
fn with_member(rec: &crate::persistent::AgentRecord, p: &crate::persistent::Persistent) -> Value {
    let mut out = crate::persistent_tools::row(rec);
    if let (Some(o), Value::Object(m)) = (out.as_object_mut(), member_row(rec, &p.owner())) {
        for (k, v) in m {
            o.entry(k).or_insert(v);
        }
    }
    out
}

fn first_words(text: &str) -> String {
    let t: String = text
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    if t.is_empty() { "Scheduled".into() } else { t }
}

/// A run tool given a member's `name`: the member's current run, or for
/// `agent_message` the message delivered through the member (recorded in
/// the thread as the caller).
async fn member_run(spaces: &Spaces, caller: &Caller, a: Value) -> Result<ToolOutcome> {
    let p = spaces.persistent();
    let field = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or_default();
    let (tool, space, name) = (field("tool"), field("space"), field("name"));
    let rec = p.get(name)?;
    if rec.space != space {
        return Err(Error::invalid(format!(
            "{name} is a member of {}, not {space}",
            rec.space
        )));
    }
    policy::gate(
        caller,
        p.role_of(caller),
        tool,
        policy::need_member_run(tool),
    )?;
    if tool != "agent_message" {
        let run = rec.run_id.ok_or_else(|| {
            Error::NotFound(format!(
                "{name} has no run now (it is asleep); post to its thread to wake it"
            ))
        })?;
        // Interrupting or stopping a member by name is taking control of
        // it: it is held (no task is delivered) until control is given
        // back (`agent_resume`).
        if matches!(tool, "agent_interrupt" | "agent_stop") {
            p.take_control(caller, name)?;
        }
        return Ok(ToolOutcome::json(&json!({"run_id": run})));
    }
    let text = a
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid("agent_message: `text` is required"))?;
    let force = a.get("force").and_then(Value::as_bool).unwrap_or(false);
    let s = spaces.space(space).await?;
    let agents = s.agents().await?;
    if let Some(run) = &rec.run_id {
        let st = agents.status(run).await?;
        if st.status == RunStatus::Running && !force {
            // Queued after the running turn, as agent_message does.
            let st = agents.send(run, text, vec![]).await?;
            return Ok(ToolOutcome::json(&json!({
                "delivered": true, "run_id": run, "status": st.status, "phase": st.phase,
                "reason": "queued after the running turn",
            })));
        }
        if st.status == RunStatus::Running {
            agents.interrupt(run).await?;
            // The interrupted turn ends before the next one starts.
            for _ in 0..40 {
                if agents.status(run).await?.status != RunStatus::Running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    let d = p.send_as(caller, name, text).await?;
    let st = agents.status(&d.run_id).await?;
    Ok(ToolOutcome::json(&json!({
        "delivered": true, "run_id": d.run_id, "status": st.status, "phase": st.phase,
        "reason": if d.started { "a new run with the agent's home restored" } else { "a new turn in the same session" },
    })))
}
