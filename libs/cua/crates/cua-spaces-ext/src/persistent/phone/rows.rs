// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `thread_tool_rows`: what the agents of a Space are doing, one short row
//! per tool call, for a phone.
//!
//! The rows are the ones the desktop thread draws: each call goes through
//! [`cua_agents::tool_row::tool_row`] (the one writer of the collapsed
//! line and its category), so `Ran pytest test_cart.py` reads the same on
//! every client. The phone gets structure only (category, line, a short
//! detail, state, time) grouped under the thread task they follow; it
//! builds no label from model text, and the group header (`Ran 4
//! commands`) is the app core's, from the categories.
//!
//! Nothing here is specific to where the Space runs: events are read from
//! the run through the Space's agent service, the same path for a
//! container, a VM or a cloud machine.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

use super::super::{AgentRecord, Persistent};
use cua_agents::events::AgentEvent;
use cua_agents::tool_row::{Cat, ROW_MAX};
use cua_spaces::extension::Caller;
use cua_spaces::mcp::ToolOutcome;
use cua_spaces::{Error, Result};

/// Calls returned at most (the newest are kept).
pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 500;
/// The longest `follow_ms`, as `thread_read`'s.
pub const MAX_FOLLOW_MS: u64 = 25_000;
/// Longest `detail`, in characters.
pub const DETAIL_MAX: usize = 200;
/// Calls remembered per run, and runs remembered.
const CALLS_PER_RUN: usize = 1500;
const RUNS_KEPT: usize = 48;
/// Events read from a run in one go.
const PAGE_EVENTS: usize = 500;

/// One tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub turn: u32,
    pub cat: Cat,
    pub line: String,
    pub detail: String,
    pub state: &'static str,
    pub at_ms: u64,
}

/// A run's calls so far, and how far its events were read.
#[derive(Default)]
struct RunRows {
    cursor: u64,
    calls: Vec<Call>,
    by_id: HashMap<String, usize>,
    /// Bumped by every change, for the response's `cursor`.
    revision: u64,
}

fn cache() -> &'static Mutex<HashMap<String, RunRows>> {
    static C: std::sync::OnceLock<Mutex<HashMap<String, RunRows>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// The state word of an ACP tool status.
pub fn state_of(status: Option<&str>) -> &'static str {
    match status {
        Some("completed") => "done",
        Some("failed") => "failed",
        _ => "running",
    }
}

/// `text` with a credential-shaped value replaced: the Cua Volume's secret
/// scanner decides, as it does for a post.
fn hide_secrets(text: &str, fallback: &str) -> String {
    if cua_volume::scan::scan(text.as_bytes()).is_empty() {
        text.to_string()
    } else {
        fallback.to_string()
    }
}

/// The call an event is about, as a row (`None`: not a tool call).
pub fn call_of(e: &AgentEvent) -> Option<Call> {
    let id = e.tool_id.clone()?;
    let row = e.tool_row();
    let verb = row.line.split_whitespace().next().unwrap_or("Used");
    let line = hide_secrets(&row.line, &format!("{verb} (hidden)"));
    let detail = e
        .tool_title
        .as_deref()
        .map(|t| cua_agents::events::one_line(t, DETAIL_MAX))
        .unwrap_or_default();
    Some(Call {
        id,
        turn: e.turn,
        cat: row.cat,
        line: cut_line(&line),
        detail: hide_secrets(&detail, "(hidden)"),
        state: state_of(e.tool_status.as_deref()),
        at_ms: e.ts_ms,
    })
}

fn cut_line(line: &str) -> String {
    cua_agents::events::one_line(line, ROW_MAX)
}

impl RunRows {
    fn absorb(&mut self, e: &AgentEvent) {
        if !matches!(e.kind, "tool_call" | "tool_update") {
            return;
        }
        let Some(new) = call_of(e) else { return };
        match self.by_id.get(&new.id).copied() {
            Some(i) => {
                let old = &mut self.calls[i];
                // The first event may carry no arguments yet: the update
                // that names what the call does replaces an empty or
                // generic line.
                if !new.line.is_empty() && (old.line.is_empty() || new.cat != Cat::Other) {
                    old.line = new.line;
                    old.cat = new.cat;
                }
                if !new.detail.is_empty() {
                    old.detail = new.detail;
                }
                // A call that ended does not run again.
                if old.state == "running" || new.state != "running" {
                    old.state = new.state;
                }
                self.revision += 1;
            }
            None => {
                if self.calls.len() >= CALLS_PER_RUN {
                    // Forget the oldest; ids shift.
                    self.calls.remove(0);
                    self.by_id = self
                        .calls
                        .iter()
                        .enumerate()
                        .map(|(i, c)| (c.id.clone(), i))
                        .collect();
                }
                self.by_id.insert(new.id.clone(), self.calls.len());
                self.calls.push(new);
                self.revision += 1;
            }
        }
    }
}

/// Reads what is new of `run` in `space` and returns its calls and a
/// revision number.
async fn run_calls(
    agents: &cua_spaces::agents::Agents,
    space: &str,
    run: &str,
) -> Result<(Vec<Call>, u64)> {
    let key = format!("{space}\u{0}{run}");
    let from = cache()
        .lock()
        .expect("tool rows")
        .get(&key)
        .map_or(0, |r| r.cursor);
    let mut cursor = from;
    let mut events = vec![];
    loop {
        let page = agents.events(run, cursor, PAGE_EVENTS).await?;
        let moved = page.cursor != cursor;
        cursor = page.cursor;
        events.extend(page.events);
        if page.caught_up || !moved {
            break;
        }
    }
    let mut c = cache().lock().expect("tool rows");
    if c.len() >= RUNS_KEPT && !c.contains_key(&key) {
        // Forget the run read longest ago (any one will do: it is read
        // again from the start if it comes back).
        if let Some(k) = c.keys().next().cloned() {
            c.remove(&k);
        }
    }
    let r = c.entry(key).or_default();
    // Another reader got there first: do not read the same events twice.
    if r.cursor > from {
        return Ok((r.calls.clone(), r.revision));
    }
    for e in &events {
        r.absorb(e);
    }
    r.cursor = cursor;
    Ok((r.calls.clone(), r.revision))
}

/// The groups of calls of all members of `space` now, and a number that
/// changes when anything does.
async fn snapshot(
    p: &Persistent,
    space: &str,
    members: &[AgentRecord],
    fold: &crate::thread::Fold,
    min_after: u64,
) -> Result<(Vec<Value>, u64, Vec<(u64, Call)>)> {
    let agents = match tokio::time::timeout(Duration::from_secs(15), p.cached_agents(space)).await {
        Ok(Ok(a)) => Some(a),
        // A Space that cannot be reached now has no live rows to show.
        _ => None,
    };
    let mut groups: Vec<(u64, u64, String, Vec<Call>)> = vec![];
    let mut mix = 0u64;
    if let Some(agents) = agents {
        for rec in members {
            let handle = rec.handle().to_string();
            for s in rec.session_views() {
                let Some(run) = s.run_id.as_deref() else {
                    continue;
                };
                let (calls, rev) = match run_calls(&agents, space, run).await {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::debug!(agent = %rec.name, "tool rows: {e}");
                        continue;
                    }
                };
                mix = mix
                    .wrapping_mul(1_000_003)
                    .wrapping_add(rev)
                    .wrapping_add(calls.len() as u64);
                // One group per turn: the calls the agent made for one task.
                let mut by_turn: std::collections::BTreeMap<u32, Vec<Call>> = Default::default();
                for c in calls {
                    by_turn.entry(c.turn).or_default().push(c);
                }
                for (_turn, calls) in by_turn {
                    let at = calls.first().map_or(0, |c| c.at_ms);
                    let after_seq = fold
                        .tasks
                        .values()
                        .filter(|t| t.to == rec.name && t.ts_ms <= at)
                        .map(|t| t.seq)
                        .max()
                        .unwrap_or(0);
                    if after_seq >= min_after {
                        groups.push((after_seq, at, handle.clone(), calls));
                    }
                }
            }
        }
    }
    groups.sort_by_key(|g| (g.0, g.1));
    let flat = groups
        .iter()
        .flat_map(|g| g.3.iter().map(|c| (g.0, c.clone())))
        .collect();
    let out = groups
        .into_iter()
        .map(|(after_seq, _, agent, calls)| (after_seq, agent, calls))
        .map(|(after_seq, agent, calls)| {
            json!({
                "after_seq": after_seq,
                "agent": agent,
                "calls": calls.iter().map(call_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok((out, mix, flat))
}

/// A call as the wire shows it.
pub fn call_json(c: &Call) -> Value {
    json!({
        "id": c.id,
        "cat": c.cat.word(),
        "line": c.line,
        "detail": c.detail,
        "state": c.state,
        "at_ms": c.at_ms,
    })
}

/// Keeps the newest `limit` calls of `groups`, dropping empty groups.
fn limited(groups: Vec<Value>, limit: usize) -> (Vec<Value>, bool) {
    let total: usize = groups
        .iter()
        .map(|g| g["calls"].as_array().map_or(0, Vec::len))
        .sum();
    if total <= limit {
        return (groups, false);
    }
    let mut skip = total - limit;
    let mut out = vec![];
    for mut g in groups {
        let calls = g["calls"].as_array().cloned().unwrap_or_default();
        if skip >= calls.len() {
            skip -= calls.len();
            continue;
        }
        g["calls"] = json!(calls[skip..]);
        skip = 0;
        out.push(g);
    }
    (out, true)
}

/// `thread_tool_rows`.
pub(crate) async fn tool_rows(p: &Persistent, caller: &Caller, a: Value) -> Result<ToolOutcome> {
    let space = a
        .get("space")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid("thread_tool_rows: `space` is required"))?;
    let min_after = a.get("after_seq").and_then(Value::as_u64).unwrap_or(0);
    let limit = a
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_LIMIT, |l| l as usize)
        .clamp(1, MAX_LIMIT);
    let follow = a
        .get("follow_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(MAX_FOLLOW_MS);
    let seen = a.get("cursor").and_then(Value::as_u64);
    // Reading the thread is open to everyone in it, guests included; the
    // rows are what the thread's own live row summarizes.
    let t = p.thread(caller, space).await?;
    let id = t.store().space().to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(follow);
    loop {
        let members = p.members_of(&id)?;
        let fold = t.store().fold()?;
        let (groups, cursor, _) = snapshot(p, &id, &members, &fold, min_after).await?;
        let changed = seen.is_none_or(|s| s != cursor);
        if changed || follow == 0 || tokio::time::Instant::now() >= deadline {
            let (groups, cut) = limited(groups, limit);
            return Ok(ToolOutcome::json(&json!({
                "groups": groups,
                "cursor": cursor,
                "caught_up": !cut,
            })));
        }
        // Nothing new: look again shortly (events are read incrementally,
        // so a look costs one small read per live run).
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(line: &str) -> AgentEvent {
        AgentEvent::parse(line).expect("an event")
    }

    fn call_event(seq: u64, id: &str, title: &str, status: &str, kind: &str) -> AgentEvent {
        let kind_word = if kind == "tool_call" {
            "tool_call"
        } else {
            "tool_call_update"
        };
        event(
            &json!({
                "seq": seq, "ts": 1_000 + seq, "turn": 1, "type": "update",
                "update": {
                    "sessionUpdate": kind_word, "toolCallId": id, "title": title,
                    "kind": "execute", "status": status,
                    "rawInput": {"command": title},
                }
            })
            .to_string(),
        )
    }

    #[test]
    fn a_call_and_its_update_are_one_row_that_ends() {
        let mut r = RunRows::default();
        r.absorb(&call_event(
            1,
            "tc1",
            "pytest tests/test_cart.py -x",
            "in_progress",
            "tool_call",
        ));
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.calls[0].state, "running");
        assert_eq!(r.calls[0].cat, Cat::Ran);
        assert!(r.calls[0].line.starts_with("Ran "), "{}", r.calls[0].line);
        assert!(r.calls[0].line.chars().count() <= ROW_MAX);
        r.absorb(&call_event(
            2,
            "tc1",
            "pytest tests/test_cart.py -x",
            "completed",
            "tool_update",
        ));
        assert_eq!(r.calls.len(), 1, "an update is not a new row");
        assert_eq!(r.calls[0].state, "done");
        // A call that ended does not run again, whatever order events came.
        r.absorb(&call_event(
            3,
            "tc1",
            "pytest tests/test_cart.py -x",
            "in_progress",
            "tool_update",
        ));
        assert_eq!(r.calls[0].state, "done");
    }

    #[test]
    fn a_failed_call_says_so_and_other_events_are_not_rows() {
        let mut r = RunRows::default();
        r.absorb(&call_event(1, "a", "false", "failed", "tool_call"));
        assert_eq!(r.calls[0].state, "failed");
        r.absorb(&event(
            r#"{"seq":2,"ts":5,"turn":1,"type":"notice","message":"hi"}"#,
        ));
        assert_eq!(r.calls.len(), 1);
    }

    #[test]
    fn a_secret_in_a_command_never_reaches_the_row() {
        let key = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
        let c = call_of(&call_event(
            1,
            "k",
            &format!("aws s3 ls --secret {key}"),
            "pending",
            "tool_call",
        ))
        .expect("a call");
        assert!(!c.line.contains(&key) && !c.detail.contains(&key), "{c:?}");
    }

    #[test]
    fn the_newest_calls_are_kept_under_the_limit() {
        let groups = vec![
            json!({"after_seq": 1, "agent": "a", "calls": [{"id": "1"}, {"id": "2"}, {"id": "3"}]}),
            json!({"after_seq": 5, "agent": "a", "calls": [{"id": "4"}, {"id": "5"}]}),
        ];
        let (kept, cut) = limited(groups.clone(), 10);
        assert!(!cut);
        assert_eq!(kept.len(), 2);
        let (kept, cut) = limited(groups, 3);
        assert!(cut);
        let ids: Vec<&str> = kept
            .iter()
            .flat_map(|g| g["calls"].as_array().unwrap())
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["3", "4", "5"]);
    }

    #[test]
    fn state_words() {
        assert_eq!(state_of(Some("completed")), "done");
        assert_eq!(state_of(Some("failed")), "failed");
        assert_eq!(state_of(Some("pending")), "running");
        assert_eq!(state_of(None), "running");
    }
}
