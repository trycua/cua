//! The normalized event stream.
//!
//! The runner writes one JSON object per line to `events.jsonl`: ACP
//! `session/update` notifications verbatim (`"type":"update"`), plus the
//! run's own lifecycle (install progress, turns, permissions, exit). This
//! module turns each line into one [`AgentEvent`] with a closed `kind`,
//! keeping the whole line as `raw`, so a consumer can switch on `kind` and
//! still reach every ACP field.

use serde::Serialize;
use serde_json::Value;

/// One event.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentEvent {
    /// Monotonic within a run.
    pub seq: u64,
    /// Unix milliseconds when the runner wrote it.
    pub ts_ms: u64,
    /// Turn number (0 before the first prompt).
    pub turn: u32,
    /// One of [`KINDS`].
    pub kind: &'static str,
    /// Message, thought or prompt text; an error or notice message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tool call id, for `tool_call`, `tool_update` and `permission`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    /// Tool title (for example the command a shell tool runs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_title: Option<String>,
    /// ACP tool kind (`execute`, `edit`, `read`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<String>,
    /// ACP tool status (`pending`, `in_progress`, `completed`, `failed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_status: Option<String>,
    /// ACP stop reason, for `turn_ended`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// How a conversation view shows it: one of [`CATEGORIES`] (see
    /// [`category`]).
    pub category: &'static str,
    /// One short line for an `activity` event (see [`AgentEvent::summary`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The line as written.
    pub raw: Value,
}

/// How a conversation view shows an event, one rule for every client:
///
/// - `message`: the agent's own words, a chat bubble or prose;
/// - `user`: the user's prompt as the agent received it (`turn_started`,
///   `user_message`). An app that already shows what the user typed skips
///   these, so the prompt is never repeated as agent text;
/// - `activity`: everything else worth a muted one-line row (install
///   progress, thinking, tool calls and results, plans, permissions, turn
///   ends, notices, errors, exits). Consecutive rows fold into one
///   collapsible group ([`crate::transcript::Transcript`]);
/// - `hidden`: bookkeeping with nothing to show (usage, commands, modes,
///   session setup).
pub const CATEGORIES: &[&str] = &["message", "user", "activity", "hidden"];

/// The category of an event kind (one of [`KINDS`]); unknown kinds are
/// `hidden`.
pub fn category(kind: &str) -> &'static str {
    match kind {
        "message" => "message",
        "turn_started" | "user_message" => "user",
        "thought" | "tool_call" | "tool_update" | "plan" | "permission" | "install"
        | "turn_ended" | "notice" | "error" | "exited" | "cancel_requested" => "activity",
        _ => "hidden",
    }
}

/// Longest [`AgentEvent::summary`], in characters.
pub const SUMMARY_MAX: usize = 160;

/// `text` on one line: whitespace runs collapsed, cut to `max` characters
/// with an ellipsis.
pub fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('\u{2026}');
    cut
}

/// Every kind an event can have.
pub const KINDS: &[&str] = &[
    "install",
    "initialized",
    "authenticated",
    "session",
    "mode",
    "turn_started",
    "message",
    "thought",
    "user_message",
    "tool_call",
    "tool_update",
    "plan",
    "usage",
    "permission",
    "commands",
    "info",
    "cancel_requested",
    "turn_ended",
    "notice",
    "error",
    "exited",
    "other",
];

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn content_text(v: &Value) -> Option<String> {
    match v.get("content") {
        Some(Value::Object(c)) => c.get("text").and_then(Value::as_str).map(str::to_string),
        Some(Value::Array(a)) => {
            let t: Vec<&str> = a
                .iter()
                .filter_map(|b| {
                    b.get("text")
                        .or_else(|| b.get("content").and_then(|c| c.get("text")))
                        .and_then(Value::as_str)
                })
                .collect();
            (!t.is_empty()).then(|| t.join("\n"))
        }
        _ => None,
    }
}

impl AgentEvent {
    /// Parses one `events.jsonl` line; `None` for a line that is not JSON.
    pub fn parse(line: &str) -> Option<AgentEvent> {
        let raw: Value = serde_json::from_str(line.trim()).ok()?;
        let mut e = AgentEvent {
            seq: raw.get("seq").and_then(Value::as_u64).unwrap_or(0),
            ts_ms: raw.get("ts").and_then(Value::as_u64).unwrap_or(0),
            turn: raw.get("turn").and_then(Value::as_u64).unwrap_or(0) as u32,
            kind: "other",
            text: None,
            tool_id: None,
            tool_title: None,
            tool_kind: None,
            tool_status: None,
            stop_reason: None,
            category: "hidden",
            summary: None,
            raw: Value::Null,
        };
        let ty = raw.get("type").and_then(Value::as_str).unwrap_or("");
        e.kind = match ty {
            "install" => {
                e.text = Some(format!(
                    "{} {} {}",
                    s(&raw, "id").unwrap_or_default(),
                    s(&raw, "phase").unwrap_or_default(),
                    s(&raw, "detail").unwrap_or_default()
                ));
                "install"
            }
            "initialized" => "initialized",
            "authenticated" => "authenticated",
            "session" => "session",
            "mode" => "mode",
            "turn_started" => {
                e.text = s(&raw, "prompt");
                "turn_started"
            }
            "turn_ended" => {
                e.stop_reason = s(&raw, "stopReason");
                "turn_ended"
            }
            "permission" => {
                let t = &raw["toolCall"];
                e.tool_id = s(t, "toolCallId");
                e.tool_title = s(t, "title");
                e.tool_kind = s(t, "kind");
                e.text = s(&raw, "chosen");
                "permission"
            }
            "cancel_requested" => "cancel_requested",
            "notice" => {
                e.text = s(&raw, "message");
                "notice"
            }
            "error" => {
                e.text = s(&raw, "message");
                "error"
            }
            "run_exited" => {
                e.text = s(&raw, "reason");
                "exited"
            }
            "update" => {
                let u = &raw["update"];
                match u.get("sessionUpdate").and_then(Value::as_str).unwrap_or("") {
                    "agent_message_chunk" => {
                        e.text = content_text(u);
                        "message"
                    }
                    "agent_thought_chunk" => {
                        e.text = content_text(u);
                        "thought"
                    }
                    "user_message_chunk" => {
                        e.text = content_text(u);
                        "user_message"
                    }
                    k @ ("tool_call" | "tool_call_update") => {
                        e.tool_id = s(u, "toolCallId");
                        e.tool_title = s(u, "title");
                        e.tool_kind = s(u, "kind");
                        e.tool_status = s(u, "status");
                        e.text = content_text(u);
                        if k == "tool_call" {
                            "tool_call"
                        } else {
                            "tool_update"
                        }
                    }
                    "plan" | "plan_update" | "plan_removed" => "plan",
                    "usage_update" => "usage",
                    "available_commands_update" => "commands",
                    "current_mode_update" => "mode",
                    "session_info_update" | "config_option_update" => "info",
                    "notice" => {
                        e.text = content_text(u).or_else(|| s(u, "message"));
                        "notice"
                    }
                    _ => "other",
                }
            }
            _ => "other",
        };
        e.raw = raw;
        e.classify();
        Some(e)
    }

    /// An event as the `agent_events` tool (or this type's `Serialize`)
    /// wrote it: `kind` already normalized, `raw` present only on request.
    /// A raw `events.jsonl` line (`type`, no `kind`) is parsed with
    /// [`AgentEvent::parse`]. `None` for anything else.
    pub fn from_value(v: &Value) -> Option<AgentEvent> {
        let Some(kind) = v.get("kind").and_then(Value::as_str) else {
            return v
                .get("type")
                .is_some()
                .then(|| AgentEvent::parse(&v.to_string()))
                .flatten();
        };
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        let mut e = AgentEvent {
            seq: n("seq"),
            ts_ms: n("ts_ms"),
            turn: n("turn") as u32,
            kind: KINDS
                .iter()
                .copied()
                .find(|k| *k == kind)
                .unwrap_or("other"),
            text: s(v, "text"),
            tool_id: s(v, "tool_id"),
            tool_title: s(v, "tool_title"),
            tool_kind: s(v, "tool_kind"),
            tool_status: s(v, "tool_status"),
            stop_reason: s(v, "stop_reason"),
            category: "hidden",
            summary: None,
            raw: v.get("raw").cloned().unwrap_or(Value::Null),
        };
        e.classify();
        // The writer saw `raw`; its summary can say more than one rebuilt
        // without it.
        if e.category == "activity"
            && let Some(summary) = s(v, "summary").filter(|x| !x.trim().is_empty())
        {
            e.summary = Some(one_line(&summary, SUMMARY_MAX));
        }
        Some(e)
    }

    fn classify(&mut self) {
        self.category = category(self.kind);
        self.summary = self.summary();
    }

    /// One short line saying what an `activity` event was, for a muted row:
    /// `Installing node: cached`, `Tool mcp__cua-driver__click`, `Thinking:
    /// ...`, `Turn 1 ended (end_turn)`. `None` for other categories and for
    /// an update that says nothing (a tool update without a status).
    pub fn summary(&self) -> Option<String> {
        if category(self.kind) != "activity" {
            return None;
        }
        let t = |x: &Option<String>| x.clone().unwrap_or_default();
        let first = |x: &Option<String>| {
            x.as_deref()
                .and_then(|x| x.lines().map(str::trim).find(|l| !l.is_empty()))
                .map(str::to_string)
        };
        let tool = || {
            self.tool_title
                .clone()
                .filter(|x| !x.trim().is_empty())
                .or_else(|| self.tool_kind.clone())
                .unwrap_or_else(|| "tool".into())
        };
        let line = match self.kind {
            "install" => {
                let raw = &self.raw;
                let (id, phase, detail) = (s(raw, "id"), s(raw, "phase"), s(raw, "detail"));
                match (id, phase) {
                    (Some(id), Some(phase)) => {
                        let detail = detail.filter(|d| !d.trim().is_empty());
                        format!(
                            "Install {id}: {phase}{}",
                            detail.map(|d| format!(" ({d})")).unwrap_or_default()
                        )
                    }
                    _ => format!("Install {}", t(&self.text).trim()),
                }
            }
            "thought" => match first(&self.text) {
                Some(x) => format!("Thinking: {x}"),
                None => "Thinking".into(),
            },
            "tool_call" => format!("Tool {}", tool()),
            "tool_update" => {
                let status = self.tool_status.as_deref()?;
                let head = format!("Tool {} {status}", tool());
                match first(&self.text) {
                    Some(x) => format!("{head}: {x}"),
                    None => head,
                }
            }
            "plan" => {
                let entries: Vec<String> = self.raw["update"]["entries"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| e["content"].as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if entries.is_empty() {
                    "Updated the plan".into()
                } else {
                    format!("Plan: {}", entries.join("; "))
                }
            }
            "permission" => match &self.text {
                Some(c) => format!("Allowed {} ({c})", tool()),
                None => format!("Permission for {}", tool()),
            },
            "turn_ended" => match &self.stop_reason {
                Some(r) => format!("Turn {} ended ({r})", self.turn),
                None => format!("Turn {} ended", self.turn),
            },
            "notice" => format!("Notice: {}", t(&self.text)),
            "error" => format!("Error: {}", t(&self.text)),
            "exited" => match first(&self.text) {
                Some(x) => format!("Exited: {x}"),
                None => "Exited".into(),
            },
            "cancel_requested" => "Interrupt requested".into(),
            _ => return None,
        };
        Some(one_line(&line, SUMMARY_MAX))
    }

    /// One human-readable line (CLI logs, the MCP `output_tail`).
    pub fn render(&self) -> Option<String> {
        let t = |s: &Option<String>| s.clone().unwrap_or_default();
        Some(match self.kind {
            "message" => t(&self.text),
            "thought" => format!("[thinking] {}", t(&self.text)),
            "turn_started" => format!("> {}", t(&self.text)),
            "turn_ended" => format!("[turn {} ended: {}]", self.turn, t(&self.stop_reason)),
            "tool_call" => format!(
                "[tool {}] {}",
                self.tool_kind.clone().unwrap_or_else(|| "call".into()),
                t(&self.tool_title)
            ),
            "tool_update" if self.tool_status.is_some() => format!(
                "[tool {}] {}{}",
                t(&self.tool_status),
                t(&self.tool_title),
                self.text
                    .as_ref()
                    .map(|x| format!(": {}", x.lines().next().unwrap_or("")))
                    .unwrap_or_default()
            ),
            "plan" => {
                let entries = self.raw["update"]["entries"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|e| {
                                format!(
                                    "{} {}",
                                    e["status"].as_str().unwrap_or(""),
                                    e["content"].as_str().unwrap_or("")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_default();
                format!("[plan] {entries}")
            }
            "permission" => format!("[approved {}] {}", t(&self.text), t(&self.tool_title)),
            "install" => format!("[install] {}", t(&self.text)),
            "error" => format!("[error] {}", t(&self.text)),
            "notice" => format!("[notice] {}", t(&self.text)),
            "exited" => format!("[exited] {}", t(&self.text)),
            "cancel_requested" => "[interrupt requested]".into(),
            _ => return None,
        })
    }
}

/// A page of events and where to continue.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct EventPage {
    pub events: Vec<AgentEvent>,
    /// Pass back to continue after the last event returned.
    pub cursor: u64,
    /// No complete line was left to read when this page was cut.
    pub caught_up: bool,
}

/// Parses complete lines of `bytes` read from byte offset `from`; the
/// cursor stops after the last newline, so a line still being written is
/// read whole next time.
pub fn page(bytes: &[u8], from: u64, max: usize) -> EventPage {
    let mut events = vec![];
    let mut used = 0usize;
    let mut rest = bytes;
    while events.len() < max {
        let Some(nl) = rest.iter().position(|b| *b == b'\n') else {
            break;
        };
        let line = String::from_utf8_lossy(&rest[..nl]);
        if let Some(e) = AgentEvent::parse(&line) {
            events.push(e);
        }
        used += nl + 1;
        rest = &rest[nl + 1..];
    }
    EventPage {
        caught_up: !rest.contains(&b'\n'),
        events,
        cursor: from + used as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines recorded from real runs (claude-agent-acp 0.81.1 and codex-acp
    /// 1.13.1 against cua-mock-llm, the mock provider).
    const RECORDED: &str =
        include_str!("../tests/fixtures/recorded/claude-agent-acp-0.81.1.events.jsonl");

    #[test]
    fn recorded_claude_run_normalizes() {
        let p = page(RECORDED.as_bytes(), 0, 1000);
        assert!(p.caught_up);
        let kinds: Vec<&str> = p.events.iter().map(|e| e.kind).collect();
        for k in [
            "initialized",
            "session",
            "turn_started",
            "thought",
            "message",
            "tool_call",
            "permission",
            "tool_update",
            "usage",
            "turn_ended",
            "exited",
        ] {
            assert!(kinds.contains(&k), "{k} missing from {kinds:?}");
        }
        assert!(p.events.iter().all(|e| KINDS.contains(&e.kind)));
        let done = p
            .events
            .iter()
            .find(|e| e.kind == "tool_update" && e.tool_status.as_deref() == Some("completed"))
            .unwrap();
        assert!(done.text.as_deref().unwrap().contains("hello-from-mock"));
        let end = p.events.iter().find(|e| e.kind == "turn_ended").unwrap();
        assert_eq!(end.stop_reason.as_deref(), Some("end_turn"));
        let seqs: Vec<u64> = p.events.iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
    }

    #[test]
    fn a_partial_last_line_is_left_for_next_time() {
        let a = b"{\"seq\":1,\"type\":\"notice\",\"message\":\"a\"}\n{\"seq\":2,\"ty";
        let p = page(a, 100, 10);
        assert_eq!(p.events.len(), 1);
        assert_eq!(
            p.cursor,
            100 + a.iter().position(|b| *b == b'\n').unwrap() as u64 + 1
        );
        assert!(p.caught_up);
        let limited = page(RECORDED.as_bytes(), 0, 2);
        assert_eq!(limited.events.len(), 2);
        assert!(!limited.caught_up);
    }
}
