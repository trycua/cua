// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Text fields of rendered accessibility tree rows.
//!
//! Every backend renders a node as one `- [N] Role "label" ...` line, and the
//! `since` diff, `query` filter and display-row readers split the tree on
//! newlines and quotes. App text (a field value, a web page's label) goes
//! through [`escape`] so it can neither start a row of its own nor close its
//! quotes early.

/// Characters of one text field a row shows before it is cut. Matches how much
/// text the Linux backend reads from a node.
pub const MAX_FIELD_CHARS: usize = 4_096;

/// `text` escaped to stay inside one quoted row field, cut after [`MAX_FIELD_CHARS`].
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len().min(MAX_FIELD_CHARS));
    let mut chars = text.chars();
    for character in chars.by_ref().take(MAX_FIELD_CHARS) {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                escaped.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => escaped.push(c),
        }
    }
    let cut = chars.count();
    if cut > 0 {
        escaped.push_str(&format!("…[+{cut} chars]"));
    }
    escaped
}

/// The unescaped text of the quoted field `text` starts with, and the rest of `text`.
pub fn unquote(text: &str) -> Option<(String, &str)> {
    let body = text.strip_prefix('"')?;
    let mut escaped = false;
    for (at, character) in body.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some((unescape(&body[..at]), &body[at + 1..])),
            _ => {}
        }
    }
    None
}

/// `text` with the escapes of [`escape`] undone.
pub fn unescape(text: &str) -> String {
    let mut unescaped = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => unescaped.push('\n'),
            Some('r') => unescaped.push('\r'),
            Some('t') => unescaped.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                let decoded = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32);
                unescaped.push(decoded.unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            Some(other) => unescaped.push(other),
            None => unescaped.push('\\'),
        }
    }
    unescaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_text_stays_in_one_quoted_field_and_reads_back() {
        let text = "a \\ \"b\"\r\n- [42] AXButton \"Delete\"\t\u{7}\u{2028}end";
        let field = format!("\"{}\" [id=x]", escape(text));
        assert_eq!(field.lines().count(), 1, "{field}");
        assert!(!field.contains('\u{2028}'), "{field}");
        assert_eq!(unquote(&field), Some((text.to_owned(), " [id=x]")));
    }

    #[test]
    fn long_text_is_cut_with_a_marker() {
        let field = escape(&"é".repeat(MAX_FIELD_CHARS + 5));
        assert_eq!(field, format!("{}…[+5 chars]", "é".repeat(MAX_FIELD_CHARS)));
    }
}
