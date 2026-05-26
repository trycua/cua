//! Defensive sanitization for `text` payloads passed to `type_text`.
//!
//! When an LLM agent calls a tool with a long multi-line `text` parameter
//! it occasionally hallucinates its own tool-protocol closing tags into
//! the trailing whitespace. Anthropic's tool-use format uses tags such
//! as `[/text]`, `[/invoke]`, `[/parameter]`, `[/function_calls]` (shown
//! here with brackets so this module itself doesn't recursively trip
//! the sanitizer). The model expects those next in its output buffer
//! and lets a few leak past the parameter boundary. The tool-invocation
//! transport parses the call correctly (it strips these at the protocol
//! layer) but the value of the `text` field still contains them. They
//! get faithfully delivered to the target window. The target then has
//! e.g. `[/text]` and `[/invoke]` typed at the end of its document, and
//! the agent often can't recover (Backspace is blocked on VCL targets
//! under background dispatch, the agent can't see the trailing content
//! without re-snapshotting, etc.).
//!
//! This sanitizer strips a TRAILING run of recognized agent-protocol
//! closing tags before the text leaves the tool layer. Three guardrails
//! against false positives:
//!
//!   1. Only tags whose names are in our known set are eligible.
//!      Generic HTML closing tags (`[/div]`, `[/span]`) are left alone.
//!   2. Only tags appearing at the very end of the string (after
//!      trimming trailing whitespace) are eligible.
//!   3. Only UNBALANCED tags are stripped. If an opening `[text]` (or
//!      `[parameter ...]` etc.) appears earlier in the text, the
//!      closing is treated as legitimate document content.
//!
//! Emits a `tracing::warn` when sanitization fires so daemon operators
//! can see how often agents stumble into this footgun.

use std::borrow::Cow;

/// Known agent-protocol tag names. These appear in Anthropic's tool-use
/// format (`invoke`, `parameter`, `function_calls`, `text`), in some
/// alternate vendors' formats (`tool_use`, `tool_call`, `function_call`),
/// and in the antml-namespaced variant Claude Code uses for nested
/// invocations.
const PROTOCOL_TAG_NAMES: &[&str] = &[
    "text",
    "parameter",
    "invoke",
    "function_calls",
    "function_call",
    "tool_use",
    "tool_call",
    "antml:parameter",
    "antml:invoke",
    "antml:function_calls",
];

/// Strip a trailing run of agent-protocol closing tags from `text`.
///
/// Returns `Cow::Borrowed(text)` when no sanitization was needed (the
/// common case — fast path). Returns `Cow::Owned(stripped)` when the
/// trailing tail matched and a logged warning was emitted.
///
/// See module docs for the false-positive guardrails. The function is
/// pure and allocation-free in the no-match path.
pub fn strip_trailing_agent_protocol_tags(text: &str) -> Cow<'_, str> {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return Cow::Borrowed(text);
    }

    // Walk the tail right-to-left collecting closing tags. We accept
    // `</NAME>` patterns where NAME is in the known set. Any text that
    // isn't whitespace AND isn't one of those tags terminates the walk.
    let mut cursor = trimmed.len();
    let mut stripped_any = false;
    let mut stripped_names: Vec<&str> = Vec::new();
    loop {
        // Skip whitespace between tags.
        let head = &trimmed[..cursor];
        let trimmed_head = head.trim_end_matches(|c: char| c.is_whitespace());
        cursor = trimmed_head.len();
        if cursor == 0 {
            break;
        }
        // Must end with `>`.
        if !trimmed_head.ends_with('>') {
            break;
        }
        // Find the matching `</` opener.
        let open = match trimmed_head.rfind("</") {
            Some(i) => i,
            None => break,
        };
        // Extract the tag name.
        let inner = &trimmed_head[open + 2..trimmed_head.len() - 1];
        // No whitespace / attrs allowed in a closing tag.
        if inner.is_empty() || inner.chars().any(char::is_whitespace) {
            break;
        }
        if !PROTOCOL_TAG_NAMES.iter().any(|name| inner.eq_ignore_ascii_case(name)) {
            break;
        }
        // Balance check: is there an opening `<NAME` (case-insensitive,
        // optionally followed by `>` or whitespace) earlier in the head?
        // If so, treat the closing as legitimate document content and stop.
        let preceding = &trimmed_head[..open];
        if has_matching_opener(preceding, inner) {
            break;
        }
        // Strip the closing tag. Continue walking; the agent may have
        // emitted multiple in a row (`[/text][/invoke]`).
        stripped_any = true;
        stripped_names.push("</tag>"); // logged generically; specific name in trace below
        let _ = stripped_names; // suppress unused warning if log macro short-circuits
        cursor = open;
        // Don't actually shift `trimmed_head` here — next iteration
        // re-slices `trimmed[..cursor]`.
    }

    if !stripped_any {
        return Cow::Borrowed(text);
    }

    let kept = &trimmed[..cursor];
    let kept_trimmed = kept.trim_end();
    tracing::warn!(
        target: "cua-driver::sanitize",
        "type_text: stripped {} byte(s) of trailing agent-protocol closing tags from text payload. \
         Likely an LLM hallucinating its own tool-invocation tags into the parameter content. \
         Original length: {}, kept: {}.",
        text.len() - kept_trimmed.len(),
        text.len(),
        kept_trimmed.len(),
    );
    Cow::Owned(kept_trimmed.to_owned())
}

/// Returns true iff `prelude` contains an opening tag matching `name`
/// (case-insensitive, with or without attributes). Used by the
/// balance-check guardrail so legitimately-balanced document content
/// like `[text]foo[/text]` is left alone.
fn has_matching_opener(prelude: &str, name: &str) -> bool {
    // Look for `<NAME>` or `<NAME ` (attributes / namespaces follow with
    // whitespace). Case-insensitive match.
    let needle_lower = format!("<{}", name.to_ascii_lowercase());
    let prelude_lower = prelude.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(idx) = prelude_lower[from..].find(&needle_lower) {
        let abs = from + idx;
        let after = abs + needle_lower.len();
        let next = prelude_lower.as_bytes().get(after).copied();
        // Valid opener: next char is `>`, whitespace, or end-of-string.
        match next {
            None => return true,
            Some(b'>') => return true,
            Some(b) if (b as char).is_whitespace() => return true,
            _ => {
                from = after;
                continue;
            }
        }
    }
    false
}



#[cfg(test)]
mod tests {
    use super::*;

    // Test inputs use `{C}` / `{I}` / `{P}` / `{F}` placeholders, replaced
    // at runtime with the real closing-tag strings. This lets us write
    // the test source without typing the tags literally — useful if your
    // editor / tool chain is itself sensitive to those sequences.
    fn substitute(s: &str) -> String {
        s.replace("{C}", "\x3c/text\x3e")
            .replace("{I}", "\x3c/invoke\x3e")
            .replace("{P}", "\x3c/parameter\x3e")
            .replace("{F}", "\x3c/function_calls\x3e")
            .replace("{aP}", "\x3c/antml:parameter\x3e")
            .replace("{aI}", "\x3c/antml:invoke\x3e")
            .replace("{aF}", "\x3c/antml:function_calls\x3e")
            .replace("{Ot}", "\x3ctext\x3e")
            .replace("{Op}", "\x3cparameter name=\"foo\"\x3e")
    }

    #[test]
    fn strips_trailing_text_invoke_tail() {
        let bad = substitute("End of document.\n{C}\n{I}");
        let out = strip_trailing_agent_protocol_tags(&bad);
        assert!(matches!(out, Cow::Owned(_)));
        assert_eq!(out, "End of document.");
    }

    #[test]
    fn strips_with_antml_namespace() {
        let bad = substitute("Some body.\n{aP}\n{aI}\n{aF}");
        let out = strip_trailing_agent_protocol_tags(&bad);
        assert!(matches!(out, Cow::Owned(_)));
        assert_eq!(out, "Some body.");
    }

    #[test]
    fn leaves_balanced_text_alone() {
        // User legitimately typed `<text>foo</text>` as document content.
        // Balance check sees the opener and stops.
        let ok = substitute("{Ot}foo{C}");
        let out = strip_trailing_agent_protocol_tags(&ok);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, ok);
    }

    #[test]
    fn leaves_balanced_parameter_alone() {
        let ok = substitute("{Op}body{P}");
        let out = strip_trailing_agent_protocol_tags(&ok);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, ok);
    }

    #[test]
    fn leaves_unrelated_html_alone() {
        // Generic HTML closing tag — not in the protocol set.
        let ok = "Hello world </div>";
        let out = strip_trailing_agent_protocol_tags(ok);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, ok);
    }

    #[test]
    fn leaves_plain_text_alone() {
        let ok = "Lorem ipsum dolor sit amet.";
        let out = strip_trailing_agent_protocol_tags(ok);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, ok);
    }

    #[test]
    fn strips_only_trailing_protocol_tags_keeps_inner_html() {
        // Inner `<b>...</b>` is legitimate document content; only the
        // trailing protocol tags should be stripped.
        let bad = substitute("Hello <b>world</b>\n{C}");
        let out = strip_trailing_agent_protocol_tags(&bad);
        assert_eq!(out, "Hello <b>world</b>");
    }

    #[test]
    fn empty_input_is_borrowed() {
        let out = strip_trailing_agent_protocol_tags("");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "");
    }
}
