// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Cua AI, Inc.

//! What an agent said, and the subagents it started, read from a run's
//! events. One rule for every client (the thread, the phone, the CLI).
//!
//! - [`said`]: the agent's words as separate messages. Chunks of one
//!   stretch of prose join; a tool call, a thought, the end of a turn or an
//!   error ends the stretch, so what the agent said before a tool is its own
//!   message and is never glued to what it says after. Nothing is dropped:
//!   every non-empty stretch is a message, in order.
//! - [`fold`]: the subagents (Claude Code's `Task`/`Agent` tool): what each
//!   was asked, its state (`running`, `done`, `failed`), how many steps it
//!   took, and its result. Events a child produced carry
//!   [`AgentEvent::parent_tool_id`].

use serde::Serialize;

use crate::events::{AgentEvent, one_line};

/// One stretch of the agent's own words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Said {
    /// The sequence number of the first chunk (stable for a run: with the
    /// run id it names the message).
    pub first_seq: u64,
    pub last_seq: u64,
    pub turn: u32,
    pub text: String,
    /// Something else happened after it (a tool call, the end of the turn):
    /// it will not grow. An open one is still being written.
    pub closed: bool,
}

/// Whether an event ends a stretch of prose.
fn ends_prose(e: &AgentEvent) -> bool {
    matches!(
        e.kind,
        "tool_call"
            | "thought"
            | "turn_started"
            | "turn_ended"
            | "error"
            | "exited"
            | "permission"
            | "user_message"
    )
}

/// The agent's own messages in `events`, oldest first, empty ones left out.
/// Words a subagent said belong to it ([`AgentEvent::parent_tool_id`]) and
/// are not the agent's.
pub fn said<'a>(events: impl IntoIterator<Item = &'a AgentEvent>) -> Vec<Said> {
    let mut out: Vec<Said> = vec![];
    let mut open = false;
    for e in events {
        if e.kind == "message" && e.parent_tool_id.is_none() {
            let Some(t) = e.text.as_deref().filter(|t| !t.is_empty()) else {
                continue;
            };
            match out.last_mut().filter(|_| open) {
                Some(s) if s.turn == e.turn => {
                    s.text.push_str(t);
                    s.last_seq = e.seq;
                }
                _ => {
                    if let Some(s) = out.last_mut() {
                        s.closed = true;
                    }
                    out.push(Said {
                        first_seq: e.seq,
                        last_seq: e.seq,
                        turn: e.turn,
                        text: t.to_string(),
                        closed: false,
                    });
                    open = true;
                }
            }
        } else if ends_prose(e) {
            if let Some(s) = out.last_mut() {
                s.closed = true;
            }
            open = false;
        }
    }
    out.retain(|s| !s.text.trim().is_empty());
    out
}

/// The longest result a [`Subagent`] carries (characters).
pub const RESULT_MAX: usize = 600;

/// A subagent the agent started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Subagent {
    /// The `tool_id` of the call that started it.
    pub id: String,
    /// One line on what it does (the call's description).
    pub title: String,
    /// What it was asked.
    pub prompt: String,
    /// `running`, `done` or `failed`.
    pub state: &'static str,
    /// What it handed back, when the harness reports it.
    pub result: Option<String>,
    /// Tool calls it made.
    pub steps: u32,
    /// The latest tool call it made, as a row.
    pub last_step: Option<String>,
    /// Runs on its own after the call returned (the agent did not wait).
    pub background: bool,
    pub turn: u32,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
}

fn claude_meta(e: &AgentEvent) -> &serde_json::Value {
    &e.raw["update"]["_meta"]["claudeCode"]
}

/// Whether the event is the call that starts a subagent.
pub fn is_subagent_call(e: &AgentEvent) -> bool {
    if !matches!(e.kind, "tool_call" | "tool_update") || e.parent_tool_id.is_some() {
        return false;
    }
    let m = claude_meta(e);
    m["subagent"].as_bool() == Some(true)
        || matches!(m["toolName"].as_str(), Some("Task" | "Agent"))
}

fn generic_title(t: &str) -> bool {
    matches!(t.trim(), "" | "Task" | "Agent")
}

/// The subagents in `events`, in the order they started.
pub fn fold<'a>(events: impl IntoIterator<Item = &'a AgentEvent>) -> Vec<Subagent> {
    let mut subs: Vec<Subagent> = vec![];
    for e in events {
        if is_subagent_call(e) {
            let id = e.tool_id.clone().unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            let i = match subs.iter().position(|s| s.id == id) {
                Some(i) => i,
                None => {
                    subs.push(Subagent {
                        id: id.clone(),
                        title: String::new(),
                        prompt: String::new(),
                        state: "running",
                        result: None,
                        steps: 0,
                        last_step: None,
                        background: false,
                        turn: e.turn,
                        started_ms: e.ts_ms,
                        ended_ms: None,
                    });
                    subs.len() - 1
                }
            };
            let s = &mut subs[i];
            let input = &e.raw["update"]["rawInput"];
            if let Some(d) = input["description"]
                .as_str()
                .filter(|d| !d.trim().is_empty())
            {
                s.title = one_line(d, 120);
            } else if s.title.is_empty()
                && let Some(t) = e.tool_title.as_deref().filter(|t| !generic_title(t))
            {
                s.title = one_line(t, 120);
            }
            if let Some(p) = input["prompt"].as_str().filter(|p| !p.trim().is_empty()) {
                s.prompt = p.trim().to_string();
            }
            let resp = &claude_meta(e)["toolResponse"];
            if resp["status"].as_str() == Some("async_launched")
                || resp["isAsync"].as_bool() == Some(true)
            {
                s.background = true;
            }
            match e.tool_status.as_deref() {
                Some("failed") => {
                    s.state = "failed";
                    s.ended_ms = Some(e.ts_ms);
                    s.result = e.text.as_deref().map(|t| one_line(t, RESULT_MAX));
                }
                // The call returned. A background subagent has only started.
                Some("completed") if !s.background => {
                    s.state = "done";
                    s.ended_ms = Some(e.ts_ms);
                    s.result = e
                        .text
                        .as_deref()
                        .map(|t| one_line(t, RESULT_MAX))
                        .filter(|t| !t.is_empty());
                }
                _ => {}
            }
            continue;
        }
        if let Some(p) = &e.parent_tool_id {
            let Some(s) = subs.iter_mut().find(|s| &s.id == p) else {
                continue;
            };
            match e.kind {
                "tool_call" => {
                    s.steps += 1;
                    s.last_step = Some(e.tool_row().line);
                }
                "message" => {
                    if let Some(t) = e.text.as_deref().filter(|t| !t.trim().is_empty()) {
                        let r = s.result.get_or_insert_with(String::new);
                        r.push_str(t);
                        *r = one_line(r, RESULT_MAX);
                    }
                }
                _ => {}
            }
            continue;
        }
        match e.kind {
            // A background subagent reports back to the agent as a task
            // notification; the usage update that carries it ends them.
            "usage"
                if e.raw["update"]["_meta"]["_claude/origin"]["kind"].as_str()
                    == Some("task-notification") =>
            {
                for s in subs
                    .iter_mut()
                    .filter(|s| s.background && s.state == "running")
                {
                    s.state = "done";
                    s.ended_ms = Some(e.ts_ms);
                }
            }
            // The run is gone: nothing is still running.
            "exited" => {
                for s in subs.iter_mut().filter(|s| s.state == "running") {
                    s.state = "failed";
                    s.ended_ms = Some(e.ts_ms);
                }
            }
            _ => {}
        }
    }
    for s in &mut subs {
        if s.title.is_empty() {
            s.title = "Subagent".into();
        }
    }
    subs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(seq: u64, turn: u32, update: serde_json::Value) -> AgentEvent {
        AgentEvent::parse(&json!({"seq": seq, "ts": seq * 1000, "turn": turn, "type": "update", "update": update}).to_string()).unwrap()
    }
    fn say(seq: u64, text: &str) -> AgentEvent {
        ev(
            seq,
            1,
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}),
        )
    }
    fn tool(seq: u64) -> AgentEvent {
        ev(
            seq,
            1,
            json!({"sessionUpdate": "tool_call", "toolCallId": format!("t{seq}"), "title": "ls", "kind": "execute", "status": "pending"}),
        )
    }

    #[test]
    fn every_stretch_between_tool_calls_is_its_own_message_in_order() {
        let evs = [
            say(1, "I'll set it up. "),
            say(2, "One moment."),
            tool(3),
            say(4, "It is made."),
            tool(5),
            say(6, "Done"),
            say(7, "!"),
        ];
        let s = said(evs.iter());
        let texts: Vec<_> = s.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            ["I'll set it up. One moment.", "It is made.", "Done!"]
        );
        assert_eq!(
            s.iter().map(|m| m.closed).collect::<Vec<_>>(),
            [true, true, false]
        );
        assert_eq!(s[0].first_seq, 1);
        assert_eq!(s[2].first_seq, 6);
    }

    #[test]
    fn words_after_the_turn_ended_are_a_message_of_their_own() {
        // A background task finished and the agent woke up on its own: the
        // words arrive after `turn_ended`, in the same turn.
        let end = AgentEvent::parse(
            r#"{"seq":3,"ts":1,"turn":1,"type":"turn_ended","stopReason":"end_turn"}"#,
        )
        .unwrap();
        let evs = [
            say(1, "Creating it."),
            end,
            say(4, "It failed: no gateway."),
        ];
        let s = said(evs.iter());
        assert_eq!(s.len(), 2);
        assert!(s[0].closed && !s[1].closed);
        assert_eq!(s[1].text, "It failed: no gateway.");
    }

    #[test]
    fn a_subagents_words_are_not_the_agents() {
        let child = ev(
            2,
            1,
            json!({"_meta": {"claudeCode": {"parentToolUseId": "toolu_1"}}, "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "child talking"}}),
        );
        let evs = [say(1, "Started."), child, say(3, " Waiting.")];
        let s = said(evs.iter());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "Started. Waiting.");
    }

    fn task_call(seq: u64, status: Option<&str>, background: bool) -> AgentEvent {
        let mut meta = json!({"subagent": true, "toolName": "Agent"});
        if background {
            meta["toolResponse"] = json!({"status": "async_launched"});
        }
        let mut u = json!({"_meta": {"claudeCode": meta}, "sessionUpdate": if status.is_some() { "tool_call_update" } else { "tool_call" }, "toolCallId": "toolu_1", "title": "Task", "kind": "think", "rawInput": {"description": "Map the desktop", "prompt": "List the files."}});
        if let Some(s) = status {
            u["status"] = json!(s);
            u["content"] =
                json!([{"type": "content", "content": {"type": "text", "text": "Found 3 files."}}]);
        }
        ev(seq, 1, u)
    }
    fn child_tool(seq: u64) -> AgentEvent {
        ev(
            seq,
            1,
            json!({"_meta": {"claudeCode": {"parentToolUseId": "toolu_1"}}, "sessionUpdate": "tool_call", "toolCallId": format!("c{seq}"), "title": "ls -la", "kind": "execute", "status": "pending", "rawInput": {"command": "ls -la"}}),
        )
    }

    #[test]
    fn a_waited_for_subagent_runs_then_is_done_with_its_result() {
        let evs = [task_call(1, None, false), child_tool(2), child_tool(3)];
        let s = fold(evs.iter());
        assert_eq!(s.len(), 1);
        assert_eq!(
            (s[0].title.as_str(), s[0].state, s[0].steps),
            ("Map the desktop", "running", 2)
        );
        assert_eq!(s[0].prompt, "List the files.");
        let evs = [
            task_call(1, None, false),
            child_tool(2),
            task_call(4, Some("completed"), false),
        ];
        let s = fold(evs.iter());
        assert_eq!(s[0].state, "done");
        assert_eq!(s[0].result.as_deref(), Some("Found 3 files."));
        assert!(s[0].ended_ms.is_some());
    }

    #[test]
    fn a_failed_call_is_a_failed_subagent() {
        let evs = [
            task_call(1, None, false),
            task_call(2, Some("failed"), false),
        ];
        assert_eq!(fold(evs.iter())[0].state, "failed");
    }

    #[test]
    fn a_background_subagent_runs_until_its_notification_or_the_run_exits() {
        let launched = task_call(2, Some("completed"), true);
        let evs = [task_call(1, None, false), launched.clone(), child_tool(3)];
        let s = fold(evs.iter());
        assert_eq!((s[0].state, s[0].background), ("running", true));
        let note = ev(
            9,
            1,
            json!({"_meta": {"_claude/origin": {"kind": "task-notification"}}, "sessionUpdate": "usage_update"}),
        );
        let evs = [
            task_call(1, None, false),
            launched.clone(),
            child_tool(3),
            note,
        ];
        assert_eq!(fold(evs.iter())[0].state, "done");
        let gone =
            AgentEvent::parse(r#"{"seq":10,"ts":1,"turn":1,"type":"run_exited","reason":"gone"}"#)
                .unwrap();
        let evs = [task_call(1, None, false), launched, gone];
        assert_eq!(fold(evs.iter())[0].state, "failed");
    }

    #[test]
    fn a_plain_tool_is_not_a_subagent() {
        assert!(fold([tool(1)].iter()).is_empty());
    }
}
