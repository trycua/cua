//! A conversation view of the event stream: the one rule every client
//! (the Rust apps, and Swift and TypeScript through `cua-sdk`) uses to turn
//! events into transcript items.
//!
//! - the agent's `message` chunks join into one `message` item per stretch
//!   of prose;
//! - the user's prompt (`turn_started`, and a `user_message` echo only when
//!   the turn has no prompt yet) is one `user` item, so an app that shows
//!   what the user typed skips these and never repeats the prompt as agent
//!   text;
//! - consecutive `activity` events of one turn fold into one `activity`
//!   item: a group of one-line steps with a short summary (`5 steps`) that a
//!   view shows muted and collapsed. A tool call's updates rewrite its own
//!   step instead of adding lines;
//! - `hidden` events are dropped.
//!
//! [`Transcript`] folds incrementally (poll a page, absorb it), skipping
//! events it already absorbed, so re-reading a page never duplicates items.

use crate::events::{AgentEvent, SUMMARY_MAX, one_line};
use serde::Serialize;
use std::collections::HashMap;

/// One transcript item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TranscriptItem {
    /// `message`, `user` or `activity`.
    pub kind: &'static str,
    /// The turn it belongs to (0 before the first prompt).
    pub turn: u32,
    /// The message or prompt text; for `activity`, the group's one-line
    /// summary (`1 step`, `5 steps`, `3 steps, 1 error`).
    pub text: String,
    /// `activity` only: one line per step, oldest first.
    pub steps: Vec<String>,
}

/// The incremental fold. See the module docs.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    items: Vec<TranscriptItem>,
    /// Errors per activity item (for its summary).
    errors: HashMap<usize, usize>,
    /// Tool call id -> (item, step) of its row, and the call's title.
    tools: HashMap<String, (usize, usize, Option<String>)>,
    /// Highest `seq` absorbed (events without one are never skipped).
    last_seq: u64,
    revision: u64,
}

fn steps_label(steps: usize, errors: usize) -> String {
    let s = if steps == 1 {
        "1 step".to_string()
    } else {
        format!("{steps} steps")
    };
    match errors {
        0 => s,
        1 => format!("{s}, 1 error"),
        n => format!("{s}, {n} errors"),
    }
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds a whole event list.
    pub fn fold<'a>(events: impl IntoIterator<Item = &'a AgentEvent>) -> Vec<TranscriptItem> {
        let mut t = Transcript::new();
        for e in events {
            t.absorb(e);
        }
        t.items
    }

    /// The items so far.
    pub fn items(&self) -> &[TranscriptItem] {
        &self.items
    }

    /// Bumped on every change, for cheap change detection.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The last thing the agent said, on one line: the roster's preview.
    /// `None` before the agent's first message (install progress, turn ends
    /// and other activity never become a preview).
    pub fn preview(&self) -> Option<String> {
        self.items
            .iter()
            .rev()
            .filter(|i| i.kind == "message")
            .map(|i| one_line(&i.text, usize::MAX))
            .find(|t| !t.is_empty())
    }

    /// Adds one event.
    pub fn absorb(&mut self, e: &AgentEvent) {
        if e.seq != 0 {
            if e.seq <= self.last_seq {
                return;
            }
            self.last_seq = e.seq;
        }
        match e.category {
            "message" => {
                let text = e.text.clone().unwrap_or_default();
                if text.is_empty() {
                    return;
                }
                match self.items.last_mut() {
                    Some(l) if l.kind == "message" && l.turn == e.turn => l.text.push_str(&text),
                    _ => self.items.push(TranscriptItem {
                        kind: "message",
                        turn: e.turn,
                        text,
                        steps: vec![],
                    }),
                }
            }
            "user" => {
                let text = e.text.clone().unwrap_or_default();
                if e.kind == "turn_started" {
                    self.items.push(TranscriptItem {
                        kind: "user",
                        turn: e.turn,
                        text,
                        steps: vec![],
                    });
                } else {
                    // An echo of the prompt: only when the turn has none yet.
                    let has_prompt = self
                        .items
                        .iter()
                        .rev()
                        .take_while(|i| i.turn == e.turn)
                        .any(|i| i.kind == "user");
                    if has_prompt || text.is_empty() {
                        return;
                    }
                    self.items.push(TranscriptItem {
                        kind: "user",
                        turn: e.turn,
                        text,
                        steps: vec![],
                    });
                }
            }
            "activity" => {
                let known = e
                    .tool_id
                    .as_ref()
                    .and_then(|id| self.tools.get(id))
                    .cloned();
                // An update names its tool by the call's title when it has
                // none of its own.
                let titled;
                let e = match &known {
                    Some((_, _, Some(title)))
                        if e.tool_title.as_deref().is_none_or(|t| t.trim().is_empty()) =>
                    {
                        titled = AgentEvent {
                            tool_title: Some(title.clone()),
                            summary: None,
                            ..e.clone()
                        };
                        &titled
                    }
                    _ => e,
                };
                // The writer's summary (it saw `raw`), else one rebuilt here.
                let Some(line) = e.summary.clone().or_else(|| e.summary()) else {
                    return;
                };
                // A tool update rewrites the row of its call.
                if e.kind == "tool_update"
                    && let Some((i, s, _)) = known
                    && let Some(step) = self.items.get_mut(i).and_then(|it| it.steps.get_mut(s))
                {
                    *step = line;
                    self.revision += 1;
                    return;
                }
                self.step(e.turn, line, e.kind == "error");
                if let Some(id) = &e.tool_id
                    && matches!(e.kind, "tool_call" | "tool_update")
                {
                    let i = self.items.len() - 1;
                    let title = e.tool_title.clone().filter(|t| !t.trim().is_empty());
                    self.tools
                        .insert(id.clone(), (i, self.items[i].steps.len() - 1, title));
                }
                return;
            }
            _ => return,
        }
        self.revision += 1;
    }

    /// Adds an activity line the app itself produced (a start note, "queued
    /// while working"), folded like any other step of `turn`.
    pub fn note(&mut self, turn: u32, text: &str) {
        let line = one_line(text, SUMMARY_MAX);
        if !line.is_empty() {
            self.step(turn, line, false);
        }
    }

    fn step(&mut self, turn: u32, line: String, error: bool) {
        let i = match self.items.last() {
            Some(l) if l.kind == "activity" && l.turn == turn => self.items.len() - 1,
            _ => {
                self.items.push(TranscriptItem {
                    kind: "activity",
                    turn,
                    text: String::new(),
                    steps: vec![],
                });
                self.items.len() - 1
            }
        };
        if error {
            *self.errors.entry(i).or_default() += 1;
        }
        let item = &mut self.items[i];
        item.steps.push(line);
        item.text = steps_label(item.steps.len(), self.errors.get(&i).copied().unwrap_or(0));
        self.revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{CATEGORIES, KINDS, category, page};
    use serde_json::json;

    fn ev(v: serde_json::Value) -> AgentEvent {
        AgentEvent::parse(&v.to_string()).unwrap()
    }

    fn chunk(seq: u64, turn: u32, text: &str) -> AgentEvent {
        ev(json!({"seq": seq, "turn": turn, "type": "update",
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}))
    }

    #[test]
    fn every_kind_has_a_category() {
        for k in KINDS {
            assert!(CATEGORIES.contains(&category(k)), "{k}");
        }
        assert_eq!(category("message"), "message");
        assert_eq!(category("turn_started"), "user");
        assert_eq!(category("user_message"), "user");
        for k in [
            "install",
            "thought",
            "tool_call",
            "tool_update",
            "plan",
            "permission",
            "turn_ended",
            "notice",
            "error",
            "exited",
            "cancel_requested",
        ] {
            assert_eq!(category(k), "activity", "{k}");
        }
        for k in [
            "usage",
            "commands",
            "info",
            "mode",
            "session",
            "initialized",
            "other",
            "nope",
        ] {
            assert_eq!(category(k), "hidden", "{k}");
        }
    }

    #[test]
    fn summaries_are_one_short_line() {
        let install =
            ev(json!({"seq":1,"type":"install","id":"node","phase":"cached","detail":""}));
        assert_eq!(install.category, "activity");
        assert_eq!(install.summary.as_deref(), Some("Install node: cached"));
        let ended = ev(json!({"seq":2,"turn":1,"type":"turn_ended","stopReason":"end_turn"}));
        assert_eq!(ended.summary.as_deref(), Some("Turn 1 ended (end_turn)"));
        let long = "x ".repeat(400);
        let thought = ev(
            json!({"seq":3,"type":"update","update":{"sessionUpdate":"agent_thought_chunk",
            "content":{"type":"text","text": format!("\n  first\nsecond {long}")}}}),
        );
        assert_eq!(thought.summary.as_deref(), Some("Thinking: first"));
        let err = ev(json!({"seq":4,"type":"error","message": long}));
        let s = err.summary.unwrap();
        assert!(s.chars().count() <= SUMMARY_MAX && s.ends_with('\u{2026}') && !s.contains('\n'));
        let msg = chunk(5, 1, "hi");
        assert_eq!((msg.category, msg.summary), ("message", None));
        let silent = ev(
            json!({"seq":6,"type":"update","update":{"sessionUpdate":"tool_call_update","toolCallId":"t"}}),
        );
        assert_eq!(silent.summary, None);
    }

    #[test]
    fn the_fold_keeps_messages_and_groups_activity() {
        let events = vec![
            ev(json!({"seq":1,"turn":0,"type":"install","id":"node","phase":"cached","detail":""})),
            ev(
                json!({"seq":2,"turn":0,"type":"install","id":"claude-code","phase":"done","detail":""}),
            ),
            ev(json!({"seq":3,"turn":1,"type":"turn_started","prompt":"click it"})),
            ev(
                json!({"seq":4,"turn":1,"type":"update","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"click it"}}}),
            ),
            chunk(5, 1, "On "),
            chunk(6, 1, "it."),
            ev(
                json!({"seq":7,"turn":1,"type":"update","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"mcp__cua-driver__click","kind":"other","status":"pending"}}),
            ),
            ev(
                json!({"seq":8,"turn":1,"type":"update","update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"{\"ok\":true}"}}]}}),
            ),
            ev(json!({"seq":9,"turn":1,"type":"update","update":{"sessionUpdate":"usage_update"}})),
            chunk(10, 1, "Done."),
            ev(json!({"seq":11,"turn":1,"type":"turn_ended","stopReason":"end_turn"})),
            ev(json!({"seq":12,"turn":1,"type":"error","message":"boom"})),
        ];
        let items = Transcript::fold(&events);
        let kinds: Vec<&str> = items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            [
                "activity", "user", "message", "activity", "message", "activity"
            ]
        );
        assert_eq!(items[0].turn, 0);
        assert_eq!(items[0].text, "2 steps");
        assert_eq!(
            items[0].steps,
            ["Install node: cached", "Install claude-code: done"]
        );
        assert_eq!(items[1].text, "click it", "the prompt echo is not repeated");
        assert_eq!(items[2].text, "On it.");
        assert_eq!(items[3].text, "1 step", "a tool's update rewrites its row");
        assert_eq!(
            items[3].steps,
            [r#"Tool mcp__cua-driver__click completed: {"ok":true}"#]
        );
        assert_eq!(items[4].text, "Done.");
        assert_eq!(items[5].text, "2 steps, 1 error");
        assert_eq!(items[5].steps, ["Turn 1 ended (end_turn)", "Error: boom"]);
    }

    #[test]
    fn activity_groups_split_by_turn_and_notes_join_them() {
        let mut t = Transcript::new();
        t.absorb(&ev(
            json!({"seq":1,"turn":1,"type":"turn_ended","stopReason":"end_turn"}),
        ));
        t.note(1, "queued while working");
        t.absorb(&ev(
            json!({"seq":2,"turn":2,"type":"notice","message":"hello"}),
        ));
        let items = t.items();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].steps,
            ["Turn 1 ended (end_turn)", "queued while working"]
        );
        assert_eq!(items[1].turn, 2);
        assert_eq!(items[1].steps, ["Notice: hello"]);
    }

    #[test]
    fn repolling_never_duplicates_and_the_preview_is_the_last_message() {
        let mut t = Transcript::new();
        assert_eq!(t.preview(), None);
        let first = [
            chunk(1, 1, "Hello\n  there"),
            ev(json!({"seq":2,"turn":1,"type":"turn_ended","stopReason":"end_turn"})),
        ];
        for e in &first {
            t.absorb(e);
        }
        let rev = t.revision();
        for e in &first {
            t.absorb(e);
        }
        assert_eq!(t.revision(), rev);
        assert_eq!(t.items().len(), 2);
        assert_eq!(
            t.preview().as_deref(),
            Some("Hello there"),
            "turn ends never become the preview"
        );
    }

    #[test]
    fn serialized_events_fold_like_parsed_ones() {
        // The agent_events tool's shape: normalized, `raw` dropped.
        let recorded =
            include_str!("../tests/fixtures/recorded/claude-agent-acp-0.81.1.events.jsonl");
        let parsed = page(recorded.as_bytes(), 0, 10_000).events;
        let compact: Vec<AgentEvent> = parsed
            .iter()
            .map(|e| {
                let mut v = serde_json::to_value(e).unwrap();
                v.as_object_mut().unwrap().remove("raw");
                AgentEvent::from_value(&v).unwrap()
            })
            .collect();
        let a = Transcript::fold(&parsed);
        let b = Transcript::fold(&compact);
        let kinds = |x: &[TranscriptItem]| x.iter().map(|i| (i.kind, i.turn)).collect::<Vec<_>>();
        assert_eq!(kinds(&a), kinds(&b));
        assert!(a.iter().any(|i| i.kind == "message"));
        assert!(a.iter().any(|i| i.kind == "activity"));
        assert!(
            a.iter()
                .all(|i| i.kind != "message" || !i.text.starts_with('[')),
            "no bracketed harness line is a message: {a:?}"
        );
        // The writer's summary wins over one rebuilt without `raw`.
        let install = compact.iter().find(|e| e.kind == "install");
        if let Some(i) = install {
            let steps = &b.iter().find(|x| x.kind == "activity").unwrap().steps;
            assert!(steps.contains(i.summary.as_ref().unwrap()), "{steps:?}");
            assert!(
                i.summary.as_deref().unwrap().contains(": "),
                "{:?}",
                i.summary
            );
        }
        // A raw line is accepted too.
        let raw =
            serde_json::from_str::<serde_json::Value>(recorded.lines().next().unwrap()).unwrap();
        assert!(AgentEvent::from_value(&raw).is_some());
        assert!(AgentEvent::from_value(&json!({"x": 1})).is_none());
    }
}
