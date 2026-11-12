// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The verdict headline every action result starts with.
//!
//! An action result says whether the driver could confirm the action in its
//! `effect` field. Agents read the text, though, and batches shorten it: a
//! verdict at the end of a long line, or one only some platforms wrote, was
//! lost before the agent saw it. So the dispatch seam writes the verdict once,
//! from `effect`, as the first words of the result text and of its `summary`,
//! the same way on every platform and inside `run_steps` and `run_script`.
//!
//! - `confirmed`: `CONFIRMED: <what the tool did>`
//! - `unverifiable`: `UNCONFIRMED: <what the tool did>; nothing read back.
//!   Look first: get_window_state {...}`
//! - `suspected_noop`: `UNCONFIRMED: <what the tool did>; the read-back shows
//!   no change. Look first: ...`
//! - `partial`: `PARTIAL: <what the tool did>; only part of it was delivered.
//!   Look first: ...`
//! - `refused` on a non-error result: `REFUSED: <the refusal>`
//!
//! Errors keep their own text. The adapters write only what they did.

use serde_json::Value;

use crate::protocol::{Content, ToolResult};

/// Longest part of the tool's own first line the headline repeats, in
/// characters, so the "Look first" call survives a batch's 300-character cut.
const SAID_LIMIT: usize = 180;

/// How sure the driver is that an action took effect, as one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Confirmed,
    Unconfirmed,
    Partial,
    Refused,
}

impl Verdict {
    /// The verdict of a successful action result's `effect`.
    pub fn of_effect(effect: &str) -> Option<Self> {
        match effect {
            "confirmed" => Some(Self::Confirmed),
            "unverifiable" | "suspected_noop" => Some(Self::Unconfirmed),
            "partial" => Some(Self::Partial),
            "refused" => Some(Self::Refused),
            _ => None,
        }
    }

    /// The verdict of a result's structured content, when it has an `effect`.
    pub fn of_structured(structured: Option<&Value>) -> Option<Self> {
        structured
            .and_then(|value| value.get("effect"))
            .and_then(Value::as_str)
            .and_then(Self::of_effect)
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Confirmed => "CONFIRMED",
            Self::Unconfirmed => "UNCONFIRMED",
            Self::Partial => "PARTIAL",
            Self::Refused => "REFUSED",
        }
    }

    /// Whether the agent must look before it relies on the action.
    pub fn needs_a_look(self) -> bool {
        matches!(self, Self::Unconfirmed | Self::Partial)
    }
}

/// The tool's own line with the headline's `WORD: ` prefix removed, for a
/// batch line that already shows the verdict.
pub fn strip(line: &str) -> &str {
    for verdict in [
        Verdict::Confirmed,
        Verdict::Unconfirmed,
        Verdict::Partial,
        Verdict::Refused,
    ] {
        if let Some(rest) = line
            .strip_prefix(verdict.word())
            .and_then(|rest| rest.strip_prefix(": "))
        {
            return rest;
        }
    }
    line
}

/// Write the headline onto a successful action result. Called once per
/// action call at the dispatch seam, after the recovery hint, whose
/// `next_call` is the call the headline names.
pub fn annotate(tool: &str, result: &mut ToolResult) {
    if result.is_error == Some(true) || !crate::action_record::is_action_tool(tool) {
        return;
    }
    let Some(structured) = result.structured_content.as_ref() else {
        return;
    };
    let Some(verdict) = Verdict::of_structured(Some(structured)) else {
        return;
    };
    let noop = structured.get("effect").and_then(Value::as_str) == Some("suspected_noop");
    let look = look_first(tool, structured.get("next_call"));
    let said = result
        .content
        .iter()
        .find_map(|content| match content {
            Content::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let text = headline(verdict, noop, &said, &look);
    match result
        .content
        .iter_mut()
        .find(|content| matches!(content, Content::Text { text, .. } if !text.trim().is_empty()))
    {
        Some(Content::Text { text: slot, .. }) => *slot = text.clone(),
        _ => result.content.insert(0, Content::text(text.clone())),
    }
    if let Some(Value::Object(structured)) = result.structured_content.as_mut() {
        structured.insert("summary".into(), Value::String(text));
    }
}

/// `get_window_state {"pid":…,"window_id":…}`: the recovery hint's
/// `next_call`, else the read that fits the tool.
fn look_first(tool: &str, next_call: Option<&Value>) -> String {
    if let Some(call) = next_call {
        if let Some(name) = call.get("tool").and_then(Value::as_str) {
            let arguments = call
                .get("arguments")
                .filter(|arguments| arguments.as_object().is_some_and(|args| !args.is_empty()))
                .map(|arguments| format!(" {arguments}"))
                .unwrap_or_default();
            return format!("{name}{arguments}");
        }
    }
    if tool.starts_with("browser_") {
        "get_browser_state".to_owned()
    } else {
        "get_window_state".to_owned()
    }
}

fn headline(verdict: Verdict, noop: bool, said: &str, look: &str) -> String {
    let said = said.trim();
    let (first, rest) = said.split_once('\n').unwrap_or((said, ""));
    let first = first.trim().trim_end_matches('.');
    let clipped: String = first.chars().take(SAID_LIMIT).collect();
    let cut = clipped.len() < first.len();
    let what = if cut {
        format!("{clipped}…")
    } else {
        clipped
    };
    let word = verdict.word();
    let mut out = match verdict {
        Verdict::Confirmed | Verdict::Refused => format!("{word}: {what}."),
        Verdict::Unconfirmed if noop => {
            format!("{word}: {what}; the read-back shows no change. Look first: {look}")
        }
        Verdict::Unconfirmed => format!("{word}: {what}; nothing read back. Look first: {look}"),
        Verdict::Partial => {
            format!("{word}: {what}; only part of it was delivered. Look first: {look}")
        }
    };
    // A long first line keeps its full wording on the next line.
    if cut && verdict.needs_a_look() {
        out.push('\n');
        out.push_str(first);
    }
    if !rest.trim().is_empty() {
        out.push('\n');
        out.push_str(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn action(text: &str, structured: Value) -> ToolResult {
        ToolResult::text(text).with_structured(structured)
    }

    fn first_text(result: &ToolResult) -> &str {
        match &result.content[0] {
            Content::Text { text, .. } => text,
            _ => panic!("text first"),
        }
    }

    #[test]
    fn an_unconfirmed_click_leads_with_the_verdict_and_the_read_to_make() {
        let mut result = action(
            "Clicked element [2] button \"Apply\" with a real pointer click.",
            json!({
                "effect": "unverifiable",
                "route": "global_input",
                "next_call": {"tool": "get_window_state", "arguments": {"pid": 7, "window_id": 9}},
            }),
        );
        annotate("click", &mut result);
        let text = first_text(&result).to_owned();
        assert_eq!(
            text,
            "UNCONFIRMED: Clicked element [2] button \"Apply\" with a real pointer click; \
             nothing read back. Look first: get_window_state {\"pid\":7,\"window_id\":9}"
        );
        assert_eq!(result.structured_content.unwrap()["summary"], text);
    }

    #[test]
    fn every_effect_gets_its_own_word() {
        for (effect, starts) in [
            ("confirmed", "CONFIRMED: Set value to 42."),
            (
                "unverifiable",
                "UNCONFIRMED: Set value to 42; nothing read back",
            ),
            (
                "suspected_noop",
                "UNCONFIRMED: Set value to 42; the read-back shows no change",
            ),
            (
                "partial",
                "PARTIAL: Set value to 42; only part of it was delivered",
            ),
            ("refused", "REFUSED: Set value to 42."),
        ] {
            let mut result = action("Set value to 42.", json!({ "effect": effect }));
            annotate("set_value", &mut result);
            let text = first_text(&result);
            assert!(text.starts_with(starts), "{effect}: {text}");
            assert_eq!(
                text.contains("Look first"),
                effect != "confirmed" && effect != "refused"
            );
        }
    }

    #[test]
    fn the_read_survives_a_long_first_line_and_the_full_line_follows() {
        let long = "x".repeat(400);
        let mut result = action(&long, json!({ "effect": "unverifiable" }));
        annotate("click", &mut result);
        let text = first_text(&result);
        let (headline, next) = text.split_once('\n').unwrap();
        assert!(headline.chars().count() < 300, "{}", headline.len());
        assert!(
            headline.ends_with("Look first: get_window_state"),
            "{headline}"
        );
        assert_eq!(next, long);
    }

    #[test]
    fn later_lines_of_the_tool_text_stay() {
        let mut result = action(
            "Pressed Return.\nA dialog opened: Save.",
            json!({ "effect": "confirmed" }),
        );
        annotate("press_key", &mut result);
        assert_eq!(
            first_text(&result),
            "CONFIRMED: Pressed Return.\nA dialog opened: Save."
        );
    }

    #[test]
    fn errors_reads_and_results_without_an_effect_are_left_alone() {
        let mut error = ToolResult::error("click: no such element")
            .with_structured(json!({"effect": "refused"}));
        annotate("click", &mut error);
        assert_eq!(first_text(&error), "click: no such element");

        let mut read = action("3 windows", json!({"effect": "unverifiable"}));
        annotate("list_windows", &mut read);
        assert_eq!(first_text(&read), "3 windows");

        let mut plain = ToolResult::text("done");
        annotate("click", &mut plain);
        assert_eq!(first_text(&plain), "done");
    }

    #[test]
    fn a_browser_action_without_a_next_call_points_at_the_page_read() {
        let mut result = action("Typed 3 characters.", json!({"effect": "unverifiable"}));
        annotate("browser_type", &mut result);
        assert!(first_text(&result).ends_with("Look first: get_browser_state"));
    }

    #[test]
    fn strip_removes_only_a_verdict_prefix() {
        assert_eq!(strip("UNCONFIRMED: clicked; nothing"), "clicked; nothing");
        assert_eq!(strip("CONFIRMED: set"), "set");
        assert_eq!(strip("Clicked: UNCONFIRMED"), "Clicked: UNCONFIRMED");
    }
}
