// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Compact tool docs for `tools/list`.
//!
//! An MCP client that forwards every advertised schema to the model pays for
//! every word of every description on every request. Compact docs keep what a
//! model needs to pick and call a tool: the lead paragraph of the description
//! and the first sentence of each parameter note. Nothing is removed from the
//! tools themselves: the full text stays one `get_guide({tool})` away, and
//! `cua-driver describe <tool>` still prints it.
//!
//! * `CUA_DRIVER_TOOL_DOCS=compact` (default) or `full`.
//! * Config key `tool_docs` in `~/.cua-driver/config.json` does the same; the
//!   environment variable wins.

use serde_json::Value;

/// Environment variable that selects compact or full docs.
pub const TOOL_DOCS_ENV: &str = "CUA_DRIVER_TOOL_DOCS";
/// Config key with the same meaning; the environment variable wins.
pub const TOOL_DOCS_CONFIG_KEY: &str = "tool_docs";

/// Longest compact tool description.
pub const DESCRIPTION_CAP: usize = 600;
/// Longest compact parameter description.
pub const PARAM_CAP: usize = 110;

/// Advanced properties shared by almost every tool. They stay accepted when
/// sent; compact docs only leave them out of the listed schema, unless the
/// tool marks one required.
pub const ADVANCED_PROPERTIES: &[&str] = &["session", "target", "debug_image_out"];

/// Schema keywords compact docs drop.
const DROPPED_KEYWORDS: &[&str] = &["examples", "$comment", "title"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolDocs {
    Compact,
    Full,
}

impl ToolDocs {
    /// Parse `compact` or `full`; `None` for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "compact" => Some(Self::Compact),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// The docs mode of this process: environment, then config, then compact.
    /// A bad value falls back to compact with a warning.
    pub fn from_env() -> Self {
        Self::resolve(
            std::env::var(TOOL_DOCS_ENV).ok().as_deref(),
            crate::tool_profile::config_string(TOOL_DOCS_CONFIG_KEY).as_deref(),
        )
    }

    fn resolve(env: Option<&str>, config: Option<&str>) -> Self {
        let Some((source, value)) = env
            .map(|v| (TOOL_DOCS_ENV, v))
            .or(config.map(|v| (TOOL_DOCS_CONFIG_KEY, v)))
        else {
            return Self::Compact;
        };
        if value.trim().is_empty() {
            return Self::Compact;
        }
        Self::parse(value).unwrap_or_else(|| {
            tracing::warn!("{source}: expected compact or full, got {value:?}; using compact");
            Self::Compact
        })
    }

    /// Compact the `tools` of a `tools/list` result in place. `Full` is a no-op.
    pub fn apply(self, list: &mut Value) {
        if self == Self::Full {
            return;
        }
        let Some(tools) = list.get_mut("tools").and_then(Value::as_array_mut) else {
            return;
        };
        for tool in tools {
            compact_tool(tool);
        }
    }
}

/// Compact one `tools/list` entry. `outputSchema` and `annotations` are left
/// exactly as they are.
pub fn compact_tool(tool: &mut Value) {
    if let Some(description) = tool.get("description").and_then(Value::as_str) {
        let compact = lead_paragraph(description, DESCRIPTION_CAP);
        tool["description"] = Value::String(compact);
    }
    if let Some(schema) = tool.get_mut("inputSchema") {
        compact_input_schema(schema);
    }
}

/// Compact a tool's `inputSchema`: short parameter notes, no examples, and no
/// advanced shared properties unless required.
pub fn compact_input_schema(schema: &mut Value) {
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for name in ADVANCED_PROPERTIES {
            if !required.iter().any(|r| r == name) {
                properties.remove(*name);
            }
        }
    }
    compact_schema(schema);
}

/// Recursively shorten descriptions and drop `examples`, `$comment` and
/// `title` keywords. Walks schema positions only, so a property that happens
/// to be called `title` or `examples` is kept.
pub fn compact_schema(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    for keyword in DROPPED_KEYWORDS {
        object.remove(*keyword);
    }
    if let Some(Value::String(description)) = object.get_mut("description") {
        *description = first_sentence(description, PARAM_CAP);
    }
    for keyword in ["properties", "patternProperties", "$defs", "definitions"] {
        if let Some(Value::Object(children)) = object.get_mut(keyword) {
            for child in children.values_mut() {
                compact_schema(child);
            }
        }
    }
    for keyword in [
        "items",
        "additionalProperties",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
    ] {
        if let Some(child) = object.get_mut(keyword) {
            compact_schema(child);
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(children)) = object.get_mut(keyword) {
            for child in children {
                compact_schema(child);
            }
        }
    }
}

/// The text before the first blank line, cut at a sentence boundary to at most
/// `cap` characters.
pub fn lead_paragraph(text: &str, cap: usize) -> String {
    let text = text.replace("\r\n", "\n");
    let lead = text.trim().split("\n\n").next().unwrap_or("").trim();
    let joined = lead.split_whitespace().collect::<Vec<_>>().join(" ");
    cut_at_sentence(&joined, cap)
}

/// The first sentence of `text`, cut to at most `cap` characters.
pub fn first_sentence(text: &str, cap: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = sentence_ends(&joined)
        .first()
        .copied()
        .unwrap_or(joined.len());
    cut_at_sentence(&joined[..end], cap)
}

/// Keep whole sentences while they fit in `cap` characters. When even the
/// first sentence is longer, cut it at a word boundary and end it with `...`.
fn cut_at_sentence(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_owned();
    }
    let mut keep = 0;
    for end in sentence_ends(text) {
        if text[..end].chars().count() <= cap {
            keep = end;
        } else {
            break;
        }
    }
    if keep > 0 {
        return text[..keep].trim_end().to_owned();
    }
    let budget = cap.saturating_sub(3);
    let mut cut = 0;
    for (count, (index, ch)) in text.char_indices().enumerate() {
        if count >= budget {
            break;
        }
        if ch.is_whitespace() {
            cut = index;
        }
    }
    if cut == 0 {
        cut = text
            .char_indices()
            .nth(budget)
            .map(|(index, _)| index)
            .unwrap_or(text.len());
    }
    format!("{}...", text[..cut].trim_end_matches([',', ';', ':', ' ']))
}

/// Byte offsets just past each sentence end in `text` (a `.`, `!` or `?`
/// followed by whitespace or the end of the text). Punctuation inside backtick
/// spans and after common abbreviations is not a sentence end.
fn sentence_ends(text: &str) -> Vec<usize> {
    const ABBREVIATIONS: &[&str] = &["e.g", "i.e", "vs", "etc", "approx", "cf"];
    let bytes = text.as_bytes();
    let mut ends = Vec::new();
    let mut in_code = false;
    for (index, ch) in text.char_indices() {
        if ch == '`' {
            in_code = !in_code;
            continue;
        }
        if in_code || !matches!(ch, '.' | '!' | '?') {
            continue;
        }
        let after = index + ch.len_utf8();
        let at_end = after >= bytes.len();
        let before_space = !at_end && text[after..].starts_with(char::is_whitespace);
        // A closing quote or bracket may follow the punctuation.
        let closes_then_space = !at_end
            && !before_space
            && text[after..]
                .chars()
                .next()
                .is_some_and(|c| matches!(c, ')' | '"' | '\'' | ']'))
            && text[after + 1..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace);
        if !(at_end || before_space || closes_then_space) {
            continue;
        }
        let word_start = text[..index]
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        let word = text[word_start..index].to_ascii_lowercase();
        if ch == '.' && ABBREVIATIONS.contains(&word.as_str()) {
            continue;
        }
        ends.push(if closes_then_space { after + 1 } else { after });
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lead_paragraph_stops_at_the_first_blank_line() {
        let text = "Click an element.\nPrefer element_token.\n\nLong detail that follows.";
        assert_eq!(
            lead_paragraph(text, 600),
            "Click an element. Prefer element_token."
        );
    }

    #[test]
    fn lead_paragraph_cuts_at_a_sentence_boundary_under_the_cap() {
        let text = "First sentence here. Second sentence is longer than the rest. Third.";
        let cut = lead_paragraph(text, 45);
        assert_eq!(cut, "First sentence here.");
        let cut = lead_paragraph(text, 62);
        assert_eq!(
            cut,
            "First sentence here. Second sentence is longer than the rest."
        );
    }

    #[test]
    fn a_single_overlong_sentence_is_cut_at_a_word_with_an_ellipsis() {
        let text = "word ".repeat(60);
        let cut = lead_paragraph(&text, 50);
        assert!(cut.ends_with("..."), "{cut}");
        assert!(cut.chars().count() <= 50, "{cut}");
        assert!(!cut.contains("wor..."), "cut inside a word: {cut}");
    }

    #[test]
    fn abbreviations_and_code_spans_do_not_end_a_sentence() {
        let text = "Use a key, e.g. `cmd+a`. Then `x.y` works. Done.";
        assert_eq!(first_sentence(text, 110), "Use a key, e.g. `cmd+a`.");
        assert_eq!(
            first_sentence("Read `a. b` now. More.", 110),
            "Read `a. b` now."
        );
    }

    #[test]
    fn first_sentence_caps_parameter_notes() {
        let long = format!("{} end.", "long ".repeat(40));
        let cut = first_sentence(&long, PARAM_CAP);
        assert!(cut.chars().count() <= PARAM_CAP, "{cut}");
        assert_eq!(
            first_sentence("Window id. Use list_windows to find it.", PARAM_CAP),
            "Window id."
        );
    }

    #[test]
    fn compact_schema_keeps_required_and_drops_advanced_and_examples() {
        let mut tool = json!({
            "name": "click",
            "description": "Click it.\n\nDetail.",
            "inputSchema": {
                "type": "object",
                "required": ["pid", "target"],
                "properties": {
                    "pid": {"type": "integer", "description": "Process id. More text.", "examples": [1]},
                    "title": {"type": "string", "title": "T", "$comment": "x"},
                    "session": {"type": "string"},
                    "target": {"type": "object"},
                    "debug_image_out": {"type": "string"},
                    "nested": {"type": "object", "properties": {
                        "inner": {"type": "string", "description": "Inner note. Extra."}
                    }}
                }
            },
            "annotations": {"readOnlyHint": true},
            "outputSchema": {"type": "object", "description": "Keep this. And this.", "title": "Out"}
        });
        compact_tool(&mut tool);
        let props = &tool["inputSchema"]["properties"];
        assert_eq!(tool["description"], "Click it.");
        assert_eq!(props["pid"]["description"], "Process id.");
        assert!(props["pid"].get("examples").is_none());
        assert!(props.get("title").is_some(), "a property named title stays");
        assert!(props["title"].get("title").is_none());
        assert!(props.get("session").is_none());
        assert!(props.get("debug_image_out").is_none());
        assert!(
            props.get("target").is_some(),
            "required advanced property stays"
        );
        assert_eq!(
            props["nested"]["properties"]["inner"]["description"],
            "Inner note."
        );
        assert_eq!(tool["annotations"], json!({"readOnlyHint": true}));
        assert_eq!(
            tool["outputSchema"],
            json!({"type": "object", "description": "Keep this. And this.", "title": "Out"})
        );
    }

    #[test]
    fn docs_mode_resolution() {
        assert_eq!(ToolDocs::resolve(None, None), ToolDocs::Compact);
        assert_eq!(ToolDocs::resolve(Some("full"), None), ToolDocs::Full);
        assert_eq!(ToolDocs::resolve(None, Some("full")), ToolDocs::Full);
        assert_eq!(
            ToolDocs::resolve(Some("compact"), Some("full")),
            ToolDocs::Compact
        );
        assert_eq!(ToolDocs::resolve(Some("bogus"), None), ToolDocs::Compact);
    }

    #[test]
    fn full_docs_are_a_no_op() {
        let mut list = json!({"tools": [{"name": "x", "description": "A.\n\nB."}]});
        let before = list.clone();
        ToolDocs::Full.apply(&mut list);
        assert_eq!(list, before);
    }
}
