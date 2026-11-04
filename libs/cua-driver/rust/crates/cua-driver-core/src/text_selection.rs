// SPDX-License-Identifier: MIT
//! Shared semantics for the `select_text` tool.
//!
//! The tool finds a string inside an element's text value and moves the
//! element's selection (or caret) onto it. The matching rules (where the text
//! is, whether `prefix`/`suffix` disambiguate repeats, and the range in the
//! platform's index units) live here so every adapter refuses the same inputs
//! the same way. Accessibility text ranges count UTF-16 code units on macOS
//! (`AXSelectedTextRange` is a `CFRange` over an `NSString`), so ranges are
//! reported in UTF-16 units alongside the matched Unicode scalar offsets.
//!
//! Only macOS implements the tool today. Linux and Windows register
//! [`UnsupportedSelectTextTool`], which refuses every call with
//! `unsupported_platform` instead of substituting a weaker behavior.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

pub const TOOL_NAME: &str = "select_text";

/// Refusal codes `select_text` returns in `structuredContent.code`.
pub const CODE_TEXT_NOT_FOUND: &str = "text_not_found";
pub const CODE_AMBIGUOUS_TEXT: &str = "ambiguous_text_match";
pub const CODE_SELECTION_NOT_SETTABLE: &str = "selection_not_settable";
pub const CODE_SELECTION_NOT_APPLIED: &str = "selection_not_applied";
pub const CODE_UNSUPPORTED_PLATFORM: &str = "unsupported_platform";

/// Where the selection lands relative to the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionType {
    /// Select the matched text.
    Text,
    /// Zero-length range at the start of the match.
    CursorBefore,
    /// Zero-length range at the end of the match.
    CursorAfter,
}

impl SelectionType {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("text") => Ok(Self::Text),
            Some("cursor_before") => Ok(Self::CursorBefore),
            Some("cursor_after") => Ok(Self::CursorAfter),
            Some(other) => Err(format!(
                "selection_type must be \"text\", \"cursor_before\" or \"cursor_after\", not {other:?}"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::CursorBefore => "cursor_before",
            Self::CursorAfter => "cursor_after",
        }
    }
}

/// A range over a string, in UTF-16 code units (what AX text ranges count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf16Range {
    pub location: usize,
    pub length: usize,
}

/// A located selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSelection {
    /// The range to set, in UTF-16 code units.
    pub range: Utf16Range,
    /// The whole match, in UTF-16 code units (equal to `range` for `text`).
    pub match_range: Utf16Range,
    /// Byte offsets of the match in the searched value.
    pub match_bytes: std::ops::Range<usize>,
}

/// Why no single selection could be located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectTextError {
    EmptyText,
    /// `text` does not occur, or none of its `occurrences` has the given
    /// context.
    NotFound {
        occurrences: usize,
    },
    /// More than one occurrence fits the given context.
    Ambiguous {
        matches: usize,
    },
}

impl SelectTextError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::EmptyText => "invalid_arguments",
            Self::NotFound { .. } => CODE_TEXT_NOT_FOUND,
            Self::Ambiguous { .. } => CODE_AMBIGUOUS_TEXT,
        }
    }

    pub fn message(&self, has_context: bool) -> String {
        match self {
            Self::EmptyText => "text must not be empty".to_owned(),
            Self::NotFound { occurrences: 0 } => {
                "text does not occur in the element's value".to_owned()
            }
            Self::NotFound { occurrences } => format!(
                "text occurs {occurrences} time(s) but none is immediately preceded by \
                 prefix and followed by suffix"
            ),
            Self::Ambiguous { matches } if has_context => format!(
                "{matches} occurrences match text with the given prefix/suffix; \
                 lengthen prefix or suffix so exactly one matches"
            ),
            Self::Ambiguous { matches } => format!(
                "text occurs {matches} times; pass prefix and/or suffix (the text \
                 immediately before/after the wanted occurrence) so exactly one matches"
            ),
        }
    }
}

/// Number of UTF-16 code units in `s`.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Locate exactly one occurrence of `text` in `value` that is immediately
/// preceded by `prefix` and followed by `suffix` (empty or absent context
/// always fits), and return the range to select.
///
/// Every start position counts, so overlapping occurrences (`"aa"` in
/// `"aaa"`) are reported as ambiguous rather than silently picking one.
pub fn locate_selection(
    value: &str,
    text: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
    selection_type: SelectionType,
) -> Result<TextSelection, SelectTextError> {
    if text.is_empty() {
        return Err(SelectTextError::EmptyText);
    }
    let prefix = prefix.unwrap_or("");
    let suffix = suffix.unwrap_or("");
    let mut occurrences = 0usize;
    let mut fitting: Vec<usize> = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = value[from..].find(text) {
        let start = from + offset;
        let end = start + text.len();
        occurrences += 1;
        if value[..start].ends_with(prefix) && value[end..].starts_with(suffix) {
            fitting.push(start);
        }
        // Advance one character so overlapping occurrences are counted.
        from = start + value[start..].chars().next().map_or(1, char::len_utf8);
        if from > value.len() {
            break;
        }
    }
    match fitting.as_slice() {
        [] => Err(SelectTextError::NotFound { occurrences }),
        [start] => {
            let start = *start;
            let end = start + text.len();
            let location = utf16_len(&value[..start]);
            let length = utf16_len(text);
            let match_range = Utf16Range { location, length };
            let range = match selection_type {
                SelectionType::Text => match_range,
                SelectionType::CursorBefore => Utf16Range {
                    location,
                    length: 0,
                },
                SelectionType::CursorAfter => Utf16Range {
                    location: location + length,
                    length: 0,
                },
            };
            Ok(TextSelection {
                range,
                match_range,
                match_bytes: start..end,
            })
        }
        many => Err(SelectTextError::Ambiguous {
            matches: many.len(),
        }),
    }
}

/// The substring of `value` covered by a UTF-16 `range`, or `None` when the
/// range is out of bounds or splits a surrogate pair.
pub fn utf16_slice(value: &str, range: Utf16Range) -> Option<&str> {
    let end_units = range.location.checked_add(range.length)?;
    let mut units = 0usize;
    let mut start_byte = None;
    let mut end_byte = None;
    for (byte, ch) in value.char_indices() {
        if units == range.location && start_byte.is_none() {
            start_byte = Some(byte);
        }
        if units == end_units {
            end_byte = Some(byte);
            break;
        }
        if units > end_units {
            return None;
        }
        units += ch.len_utf16();
    }
    if units == range.location && start_byte.is_none() {
        start_byte = Some(value.len());
    }
    if units == end_units && end_byte.is_none() {
        end_byte = Some(value.len());
    }
    Some(&value[start_byte?..end_byte?])
}

/// Structured refusal shared by every adapter.
pub fn refusal(code: &str, message: impl Into<String>, extra: Value) -> ToolResult {
    let message = message.into();
    let mut structured = json!({
        "code": code,
        "effect": "refused",
        "reason": message,
    });
    if let (Some(target), Value::Object(extra)) = (structured.as_object_mut(), extra) {
        target.extend(extra);
    }
    ToolResult::error(format!("{TOOL_NAME} refused ({code}): {message}"))
        .with_structured(structured)
}

/// Refusal for a located-selection error.
pub fn locate_refusal(error: &SelectTextError, has_context: bool) -> ToolResult {
    let extra = match error {
        SelectTextError::EmptyText => json!({}),
        SelectTextError::NotFound { occurrences } => json!({ "occurrences": occurrences }),
        SelectTextError::Ambiguous { matches } => json!({ "matches": matches }),
    };
    refusal(error.code(), error.message(has_context), extra)
}

/// The tool's input schema. `platform_note` is appended to the description.
pub fn tool_def(description: String) -> ToolDef {
    ToolDef {
        name: TOOL_NAME.into(),
        description,
        input_schema: json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "session": crate::tool_schema::session_schema(),
                "pid": { "type": "integer", "description": "Target process ID. Optional with element_token (the token carries the pid)." },
                "window_id": { "type": "integer", "description": "Window ID. Omit with element_token." },
                "element_token": crate::tool_schema::element_token_schema(),
                "text": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Exact, case-sensitive text to find in the element's value."
                },
                "prefix": {
                    "type": "string",
                    "description": "Text that must immediately precede the match. Use it (or suffix) to pick one of several occurrences."
                },
                "suffix": {
                    "type": "string",
                    "description": "Text that must immediately follow the match."
                },
                "selection_type": {
                    "type": "string",
                    "enum": ["text", "cursor_before", "cursor_after"],
                    "default": "text",
                    "description": "\"text\" selects the match; \"cursor_before\"/\"cursor_after\" put a zero-length caret at its start/end."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: true,
    }
}

pub const MACOS_DESCRIPTION: &str = "Select text inside a text element by `element_token`, or place the caret next to it, without typing or fronting the app. Finds `text` (exact, case-sensitive) in the element's AXValue; when it occurs more than once, `prefix`/`suffix` (the text immediately before/after the wanted occurrence) must narrow it to exactly one. `selection_type`: \"text\" (default) selects the match, \"cursor_before\"/\"cursor_after\" put a zero-length caret at its start/end. Sets AXSelectedTextRange (UTF-16 units) and reads it back; `structuredContent` has `range` {location, length}, `selected_text` and `verified`. Refuses with `text_not_found`, `ambiguous_text_match` (with `matches`), `selection_not_settable` (the element has no settable AXSelectedTextRange) or `selection_not_applied` (the read-back differs). Changes only the selection, never the text; follow with type_text or press_key to edit it.";

/// Registered on platforms without a native implementation: same schema,
/// every call refused with `unsupported_platform`.
pub struct UnsupportedSelectTextTool {
    platform: &'static str,
    def: ToolDef,
}

impl UnsupportedSelectTextTool {
    pub fn new(platform: &'static str) -> Self {
        Self {
            platform,
            def: tool_def(format!(
                "Not supported on {platform}: every call refuses with `unsupported_platform`. \
                 On macOS, selects text inside a text element by `element_token` (or places \
                 the caret next to it). On {platform}, click or use press_key/hotkey to move \
                 the selection."
            )),
        }
    }
}

#[async_trait]
impl Tool for UnsupportedSelectTextTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        refusal(
            CODE_UNSUPPORTED_PLATFORM,
            format!(
                "select_text is implemented only on macOS, not {}",
                self.platform
            ),
            json!({ "platform": self.platform }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locate(
        value: &str,
        text: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
        kind: SelectionType,
    ) -> Result<Utf16Range, SelectTextError> {
        locate_selection(value, text, prefix, suffix, kind).map(|s| s.range)
    }

    #[test]
    fn selects_a_unique_match() {
        let range = locate("hello world", "world", None, None, SelectionType::Text).unwrap();
        assert_eq!(
            range,
            Utf16Range {
                location: 6,
                length: 5
            }
        );
    }

    #[test]
    fn cursor_types_collapse_to_the_match_edges() {
        let before = locate(
            "hello world",
            "world",
            None,
            None,
            SelectionType::CursorBefore,
        );
        let after = locate(
            "hello world",
            "world",
            None,
            None,
            SelectionType::CursorAfter,
        );
        assert_eq!(
            before.unwrap(),
            Utf16Range {
                location: 6,
                length: 0
            }
        );
        assert_eq!(
            after.unwrap(),
            Utf16Range {
                location: 11,
                length: 0
            }
        );
    }

    #[test]
    fn counts_utf16_units_across_emoji_and_accents() {
        // "😀" is 2 UTF-16 units (4 bytes), "é" is 1 unit (2 bytes).
        let value = "😀 café 👍🏽 done";
        let s = locate_selection(value, "done", None, None, SelectionType::Text).unwrap();
        // 😀(2) + ' '(1) + café(4) + ' '(1) + 👍🏽(4) + ' '(1) = 13
        assert_eq!(
            s.range,
            Utf16Range {
                location: 13,
                length: 4
            }
        );
        assert_eq!(&value[s.match_bytes.clone()], "done");
        assert_eq!(utf16_slice(value, s.range), Some("done"));
        let emoji = locate(value, "👍🏽", None, None, SelectionType::Text).unwrap();
        assert_eq!(
            emoji,
            Utf16Range {
                location: 8,
                length: 4
            }
        );
        assert_eq!(utf16_slice(value, emoji), Some("👍🏽"));
    }

    #[test]
    fn missing_text_is_not_found() {
        assert_eq!(
            locate("abc", "x", None, None, SelectionType::Text),
            Err(SelectTextError::NotFound { occurrences: 0 })
        );
    }

    #[test]
    fn repeats_without_context_are_ambiguous_with_a_count() {
        let err = locate(
            "a cat, a cat, a cat",
            "cat",
            None,
            None,
            SelectionType::Text,
        )
        .unwrap_err();
        assert_eq!(err, SelectTextError::Ambiguous { matches: 3 });
        assert_eq!(err.code(), CODE_AMBIGUOUS_TEXT);
        assert!(err.message(false).contains("3 times"));
    }

    #[test]
    fn overlapping_occurrences_count_as_ambiguous() {
        assert_eq!(
            locate("aaa", "aa", None, None, SelectionType::Text),
            Err(SelectTextError::Ambiguous { matches: 2 })
        );
        assert_eq!(
            locate("aaa", "aa", Some("a"), None, SelectionType::Text),
            Ok(Utf16Range {
                location: 1,
                length: 2
            })
        );
    }

    #[test]
    fn prefix_and_suffix_pick_one_occurrence() {
        let value = "one cat, two cat, three cat";
        assert_eq!(
            locate(value, "cat", Some("two "), None, SelectionType::Text),
            Ok(Utf16Range {
                location: 13,
                length: 3
            })
        );
        assert_eq!(
            locate(value, "cat", None, Some(", three"), SelectionType::Text),
            Ok(Utf16Range {
                location: 13,
                length: 3
            })
        );
        // Last occurrence: suffix "" fits, so prefix is what narrows it.
        assert_eq!(
            locate(
                value,
                "cat",
                Some("three "),
                Some(""),
                SelectionType::CursorAfter
            ),
            Ok(Utf16Range {
                location: 27,
                length: 0
            })
        );
    }

    #[test]
    fn context_that_fits_nothing_is_not_found_with_the_raw_count() {
        let err = locate(
            "a cat, a cat",
            "cat",
            Some("dog "),
            None,
            SelectionType::Text,
        )
        .unwrap_err();
        assert_eq!(err, SelectTextError::NotFound { occurrences: 2 });
        assert_eq!(err.code(), CODE_TEXT_NOT_FOUND);
    }

    #[test]
    fn context_that_still_fits_several_stays_ambiguous() {
        let err = locate(
            "x cat. x cat. y cat",
            "cat",
            Some("x "),
            None,
            SelectionType::Text,
        )
        .unwrap_err();
        assert_eq!(err, SelectTextError::Ambiguous { matches: 2 });
        assert!(err.message(true).contains("prefix/suffix"));
    }

    #[test]
    fn empty_text_is_refused() {
        assert_eq!(
            locate("abc", "", None, None, SelectionType::Text),
            Err(SelectTextError::EmptyText)
        );
    }

    #[test]
    fn matching_is_case_sensitive() {
        assert_eq!(
            locate("Cat cat", "cat", None, None, SelectionType::Text),
            Ok(Utf16Range {
                location: 4,
                length: 3
            })
        );
    }

    #[test]
    fn selection_type_parses_the_three_values_and_rejects_others() {
        assert_eq!(SelectionType::parse(None), Ok(SelectionType::Text));
        assert_eq!(SelectionType::parse(Some("text")), Ok(SelectionType::Text));
        assert_eq!(
            SelectionType::parse(Some("cursor_before")),
            Ok(SelectionType::CursorBefore)
        );
        assert_eq!(
            SelectionType::parse(Some("cursor_after")),
            Ok(SelectionType::CursorAfter)
        );
        assert!(SelectionType::parse(Some("word")).is_err());
    }

    #[test]
    fn utf16_slice_rejects_out_of_range_and_split_surrogates() {
        assert_eq!(
            utf16_slice(
                "ab",
                Utf16Range {
                    location: 0,
                    length: 3
                }
            ),
            None
        );
        assert_eq!(
            utf16_slice(
                "😀",
                Utf16Range {
                    location: 1,
                    length: 1
                }
            ),
            None
        );
        assert_eq!(
            utf16_slice(
                "ab",
                Utf16Range {
                    location: 2,
                    length: 0
                }
            ),
            Some("")
        );
    }

    #[tokio::test]
    async fn unsupported_platforms_refuse_with_unsupported_platform() {
        let tool = UnsupportedSelectTextTool::new("Linux");
        assert_eq!(tool.def().name, TOOL_NAME);
        let result = tool.invoke(json!({"text": "x"})).await;
        let structured = result
            .structured_content
            .clone()
            .expect("structured refusal");
        assert_eq!(structured["code"], CODE_UNSUPPORTED_PLATFORM);
        assert_eq!(structured["effect"], "refused");
        assert_eq!(result.is_error, Some(true));
    }
}
