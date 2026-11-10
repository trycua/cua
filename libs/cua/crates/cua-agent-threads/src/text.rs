//! Titles, cut text and the lines an agent writes into its own chat (not
//! what the person typed).

use crate::{Message, Role};

/// A title or first prompt is cut to this many characters.
pub const TITLE_MAX: usize = 80;

/// `s` on one line (runs of whitespace become one space), cut to `max`
/// characters with an ellipsis.
pub fn truncate_chars(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

/// A title from a store, or `None` when it is blank.
pub(crate) fn title(s: &str) -> Option<String> {
    let t = truncate_chars(s, TITLE_MAX);
    (!t.is_empty()).then_some(t)
}

/// `s` cut to at most `max` bytes on a char boundary, and whether it was cut.
pub(crate) fn cut_bytes(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// Tags an agent puts into the person's turn itself: slash-command
/// echoes, hook and task notifications, environment preambles.
const INJECTED_TAGS: &[&str] = &[
    "command-name",
    "command-message",
    "command-args",
    "command-contents",
    "local-command-stdout",
    "local-command-stderr",
    "local-command-caveat",
    "task-notification",
    "system-reminder",
    "user-prompt-submit-hook",
    "bash-input",
    "bash-stdout",
    "bash-stderr",
    "environment_context",
    "user_instructions",
    "permissions",
    "turn_aborted",
];

/// Whether a "user" message is one the agent wrote itself (an injected
/// preamble or notification), not one the person typed.
pub(crate) fn injected(text: &str) -> bool {
    let t = text.trim_start();
    if t.is_empty() || t.starts_with("# AGENTS.md instructions") || t.starts_with("Caveat: ") {
        return true;
    }
    let Some(rest) = t.strip_prefix('<') else {
        return false;
    };
    let tag: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    INJECTED_TAGS.contains(&tag.as_str())
}

/// Consecutive agent messages (one reply split around tool calls) as one.
pub(crate) fn merge_agent_runs(messages: Vec<Message>) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    for m in messages {
        if m.text.trim().is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == Role::Agent && m.role == Role::Agent => {
                last.text.push_str("\n\n");
                last.text.push_str(&m.text);
                last.cut |= m.cut;
                if m.ts_ms.is_some() {
                    last.ts_ms = last.ts_ms.or(m.ts_ms);
                }
            }
            _ => out.push(m),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(truncate_chars("  fix\n the   tests ", 80), "fix the tests");
        let long = "word ".repeat(40);
        let t = truncate_chars(&long, 20);
        assert_eq!(t.chars().count(), 20);
        assert!(t.ends_with('…'));
        assert_eq!(title("   \n "), None);
    }

    #[test]
    fn cuts_on_a_char_boundary() {
        let (t, cut) = cut_bytes("ééé", 3);
        assert_eq!((t.as_str(), cut), ("é", true));
        assert_eq!(cut_bytes("abc", 3), ("abc".into(), false));
    }

    #[test]
    fn knows_what_the_agent_injected() {
        assert!(injected("<command-name>/clear</command-name>"));
        assert!(injected("  <environment_context>\n<cwd>/x</cwd>"));
        assert!(injected("# AGENTS.md instructions for /x"));
        assert!(injected(""));
        assert!(!injected("<div> renders wrong in Safari"));
        assert!(!injected("fix the flaky upload test"));
    }

    #[test]
    fn merges_one_reply_split_by_tools() {
        let m = |role, text: &str| Message {
            key: text.into(),
            role,
            text: text.into(),
            ts_ms: None,
            cut: false,
        };
        let out = merge_agent_runs(vec![
            m(Role::Person, "a"),
            m(Role::Agent, "b"),
            m(Role::Agent, " "),
            m(Role::Agent, "c"),
            m(Role::Person, "d"),
        ]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].text, "b\n\nc");
        assert_eq!(out[1].key, "b");
    }
}
