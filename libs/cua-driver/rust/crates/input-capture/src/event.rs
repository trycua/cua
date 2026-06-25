//! Human input events captured during a demonstration recording.
//!
//! These are intentionally window-scoped and privacy-preserving: literal typed
//! text is redacted by default (see [`redact`]). The event stream is meant to
//! be persisted in the same per-turn folder schema as agent actions, with a
//! `source: "human"` discriminator, so a demonstration trajectory interleaves
//! human and agent actions on one timeline.

use serde::{Deserialize, Serialize};

/// Mouse / pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Active keyboard modifiers at the time of an event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Mods {
    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }
    /// True when a modifier that turns a keystroke into a command (rather than
    /// text entry) is held — Ctrl/Alt/Meta. Shift alone still produces text.
    pub fn is_command(&self) -> bool {
        self.ctrl || self.alt || self.meta
    }
    /// Render as a hotkey prefix like `Ctrl+Alt+`.
    pub fn prefix(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.meta {
            s.push_str("Meta+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        s
    }
}

/// A single captured human input event. Timestamps are milliseconds from the
/// recording session's monotonic anchor (shared with agent actions and the
/// cursor sampler).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HumanEvent {
    Click {
        x: f64,
        y: f64,
        button: Button,
        t_ms: u64,
    },
    DoubleClick {
        x: f64,
        y: f64,
        button: Button,
        t_ms: u64,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        t_ms: u64,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
        button: Button,
        t_ms: u64,
    },
    /// A non-text / command keystroke recorded by semantic name, e.g. `Enter`,
    /// `Tab`, `Escape`, `F2`, or a hotkey like `Ctrl+C`.
    Key {
        name: String,
        mods: Mods,
        t_ms: u64,
    },
    /// A run of text-producing keystrokes coalesced into one event. `redacted`
    /// is always safe to log/share; `raw` is only present when the caller
    /// explicitly opted into raw capture.
    Text {
        redacted: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw: Option<String>,
        char_count: usize,
        t_ms: u64,
    },
    FocusGained {
        t_ms: u64,
    },
    FocusLost {
        t_ms: u64,
    },
}

impl HumanEvent {
    pub fn t_ms(&self) -> u64 {
        match self {
            HumanEvent::Click { t_ms, .. }
            | HumanEvent::DoubleClick { t_ms, .. }
            | HumanEvent::Scroll { t_ms, .. }
            | HumanEvent::Drag { t_ms, .. }
            | HumanEvent::Key { t_ms, .. }
            | HumanEvent::Text { t_ms, .. }
            | HumanEvent::FocusGained { t_ms }
            | HumanEvent::FocusLost { t_ms } => *t_ms,
        }
    }

    /// Map to a `(tool, arguments)` pair for **persistence** in the recording
    /// trajectory (the `action.json` schema). Unlike [`to_tool_call`], this
    /// keeps the privacy-safe `redacted` summary for text and persists key
    /// names, so the trajectory is readable. Focus transitions return `None`
    /// (they are not recorded as turns). The persisted args mirror the
    /// replayable shape where possible so `replay_trajectory` still works.
    pub fn to_record(&self) -> Option<(&'static str, serde_json::Value)> {
        use serde_json::json;
        match self {
            HumanEvent::Text { redacted, raw, char_count, .. } => Some((
                "type_text",
                json!({
                    "redacted": redacted,
                    "char_count": char_count,
                    // Present only when raw capture was opted into; replay uses it.
                    "text": raw,
                }),
            )),
            HumanEvent::FocusGained { .. } | HumanEvent::FocusLost { .. } => None,
            // Click/scroll/drag/double_click/key already serialize cleanly.
            other => other.to_tool_call(),
        }
    }

    /// Map to the equivalent cua-driver tool name + arguments so a
    /// demonstration turn can be persisted in the same `action.json` schema as
    /// agent actions and replayed by `replay_trajectory`. Returns `None` for
    /// events that are not directly replayable (focus transitions).
    pub fn to_tool_call(&self) -> Option<(&'static str, serde_json::Value)> {
        use serde_json::json;
        match self {
            HumanEvent::Click { x, y, button, .. } => Some(match button {
                Button::Right => ("right_click", json!({ "x": x, "y": y })),
                _ => ("click", json!({ "x": x, "y": y })),
            }),
            HumanEvent::DoubleClick { x, y, .. } => {
                Some(("double_click", json!({ "x": x, "y": y })))
            }
            HumanEvent::Scroll { x, y, dx, dy, .. } => {
                Some(("scroll", json!({ "x": x, "y": y, "dx": dx, "dy": dy })))
            }
            HumanEvent::Drag { from, to, .. } => Some((
                "drag",
                json!({ "start_x": from.0, "start_y": from.1, "end_x": to.0, "end_y": to.1 }),
            )),
            HumanEvent::Key { name, .. } => Some(("press_key", json!({ "key": name }))),
            // Replay uses raw text when available; otherwise the redacted
            // placeholder is intentionally a no-op-ish marker the caller can
            // skip. We only emit a replayable type_text when raw is present.
            HumanEvent::Text { raw: Some(raw), .. } => {
                Some(("type_text", json!({ "text": raw })))
            }
            HumanEvent::Text { raw: None, .. }
            | HumanEvent::FocusGained { .. }
            | HumanEvent::FocusLost { .. } => None,
        }
    }
}

/// Produce a privacy-preserving redaction of literal typed text. The result
/// never contains the original characters — only a bullet mask plus a summary
/// of length and character classes, so a trajectory reader understands *that*
/// text was entered and roughly what kind, without leaking secrets.
pub fn redact(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count == 0 {
        return String::new();
    }
    let mut has_alpha = false;
    let mut has_digit = false;
    let mut has_space = false;
    let mut has_symbol = false;
    for c in text.chars() {
        if c.is_alphabetic() {
            has_alpha = true;
        } else if c.is_ascii_digit() {
            has_digit = true;
        } else if c.is_whitespace() {
            has_space = true;
        } else {
            has_symbol = true;
        }
    }
    let mut classes = Vec::new();
    if has_alpha {
        classes.push("alpha");
    }
    if has_digit {
        classes.push("digits");
    }
    if has_symbol {
        classes.push("symbols");
    }
    if has_space {
        classes.push("spaces");
    }
    let mask: String = "•".repeat(char_count.min(12));
    let ellipsis = if char_count > 12 { "…" } else { "" };
    format!(
        "{mask}{ellipsis} ({char_count} chars, {})",
        classes.join("+")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_hides_literal_text() {
        let r = redact("hunter2");
        assert!(!r.contains("hunter2"));
        assert!(r.contains("7 chars"));
        assert!(r.contains("alpha"));
        assert!(r.contains("digits"));
    }

    #[test]
    fn redact_empty_is_empty() {
        assert_eq!(redact(""), "");
    }

    #[test]
    fn redact_caps_mask_length() {
        let r = redact(&"a".repeat(100));
        assert!(r.contains("100 chars"));
        // mask is capped so a huge paste does not produce a huge string
        assert!(r.matches('•').count() <= 12);
        assert!(r.contains('…'));
    }

    #[test]
    fn mods_command_excludes_shift_only() {
        let shift = Mods { shift: true, ..Default::default() };
        assert!(!shift.is_command());
        let ctrl = Mods { ctrl: true, ..Default::default() };
        assert!(ctrl.is_command());
    }

    #[test]
    fn text_event_replay_needs_raw() {
        let redacted = HumanEvent::Text {
            redacted: "•••".into(),
            raw: None,
            char_count: 3,
            t_ms: 0,
        };
        assert!(redacted.to_tool_call().is_none());
        let raw = HumanEvent::Text {
            redacted: "•••".into(),
            raw: Some("abc".into()),
            char_count: 3,
            t_ms: 0,
        };
        let (tool, args) = raw.to_tool_call().unwrap();
        assert_eq!(tool, "type_text");
        assert_eq!(args["text"], "abc");
    }

    #[test]
    fn right_click_maps_to_right_click_tool() {
        let e = HumanEvent::Click { x: 1.0, y: 2.0, button: Button::Right, t_ms: 0 };
        assert_eq!(e.to_tool_call().unwrap().0, "right_click");
    }
}
