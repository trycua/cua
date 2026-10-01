//! `cua.transcript.frame/1` — the cross-language conformance surface.
//!
//! The point of this module is narrow and load-bearing: every language binding
//! of the Spaces SDK must turn the same `.cast` bytes, scrubbed to the same
//! instant, into the *same bytes* of JSON. That is a real proof that the
//! terminal emulator was ported rather than re-guessed, and it is the thing
//! the fixtures under `contract/fixtures/transcript` assert.
//!
//! Three rules make byte-identity achievable across Rust, Swift and
//! TypeScript, and a follow-on schema must keep all three:
//!
//! 1. **No floats on the wire.** Times are milliseconds, rounded half-away
//!    from zero. `0.1 + 0.2` and every language's float formatter are exactly
//!    the class of difference that would make a conformance failure mean
//!    nothing.
//! 2. **Key order is written, not sorted by a serializer.** Each language uses
//!    the small ordered writer in this module rather than its native JSON
//!    encoder, because `JSONSerialization`, `serde_json` and `JSON.stringify`
//!    disagree about ordering, escaping and spacing.
//! 3. **Escaping is spelled out.** Only `"`, `\` and C0 are escaped; every
//!    other scalar is emitted as literal UTF-8. No `\/`, no `\uXXXX` for
//!    non-ASCII, no lone-surrogate games.

use crate::cast::RenderedFrame;
use crate::screen::{CellAttributes, TerminalColor};

pub const FRAME_SCHEMA: &str = "cua.transcript.frame/1";

/// An ordered JSON value. Deliberately not `serde_json::Value`: the ordering
/// and formatting guarantees are the product here.
#[derive(Clone, Debug)]
pub enum Node {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

impl Node {
    pub fn object(entries: Vec<(&str, Node)>) -> Node {
        Node::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    pub fn string(value: impl Into<String>) -> Node {
        Node::Str(value.into())
    }

    pub fn optional_string(value: &Option<String>) -> Node {
        match value {
            Some(value) => Node::Str(value.clone()),
            None => Node::Null,
        }
    }

    /// Two-space indented JSON with a trailing newline.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_node(self, 0, &mut out);
        out.push('\n');
        out
    }
}

fn write_indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_node(node: &Node, depth: usize, out: &mut String) {
    match node {
        Node::Null => out.push_str("null"),
        Node::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Node::Int(value) => out.push_str(&value.to_string()),
        Node::Str(value) => write_json_string(value, out),
        Node::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                write_indent(depth + 1, out);
                write_node(item, depth + 1, out);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(depth, out);
            out.push(']');
        }
        Node::Object(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (index, (key, value)) in entries.iter().enumerate() {
                write_indent(depth + 1, out);
                write_json_string(key, out);
                out.push_str(": ");
                write_node(value, depth + 1, out);
                if index + 1 < entries.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(depth, out);
            out.push('}');
        }
    }
}

pub fn write_json_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

/// Milliseconds, rounded half away from zero. Never a float on the wire.
pub fn milliseconds(seconds: f64) -> i64 {
    let scaled = seconds * 1000.0;
    if scaled >= 0.0 {
        (scaled + 0.5).floor() as i64
    } else {
        (scaled - 0.5).ceil() as i64
    }
}

fn colour_token(colour: &Option<TerminalColor>) -> Option<String> {
    match colour {
        None => None,
        Some(TerminalColor::Indexed(index)) => Some(format!("i{index}")),
        Some(TerminalColor::Rgb(r, g, b)) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

/// A compact, stable spelling of one cell's SGR state.
///
/// `"-"` means the terminal default. Anything else is a `+`-joined list in a
/// fixed order, so a diff of two fixtures reads as a diff of attributes rather
/// than a diff of formatting.
pub fn attribute_token(attributes: &CellAttributes) -> String {
    if attributes.is_plain() {
        return "-".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if attributes.bold {
        parts.push("bold".into());
    }
    if attributes.dim {
        parts.push("dim".into());
    }
    if attributes.italic {
        parts.push("italic".into());
    }
    if attributes.underline {
        parts.push("underline".into());
    }
    if attributes.inverse {
        parts.push("inverse".into());
    }
    if attributes.strikethrough {
        parts.push("strike".into());
    }
    if let Some(token) = colour_token(&attributes.foreground) {
        parts.push(format!("fg={token}"));
    }
    if let Some(token) = colour_token(&attributes.background) {
        parts.push(format!("bg={token}"));
    }
    parts.join("+")
}

/// Attribute runs for one row: `[start_column, end_column, token]`, emitted
/// only for rows that carry at least one non-default cell.
fn attribute_runs(frame: &RenderedFrame, row: usize) -> Option<Node> {
    let attributes = frame.screen.attributes_for_row(row);
    if attributes.iter().all(CellAttributes::is_plain) {
        return None;
    }
    let mut runs: Vec<Node> = Vec::new();
    let mut start = 0usize;
    for column in 1..=attributes.len() {
        let ended = column == attributes.len() || attributes[column] != attributes[start];
        if !ended {
            continue;
        }
        let token = attribute_token(&attributes[start]);
        if token != "-" {
            runs.push(Node::Array(vec![
                Node::Int(start as i64),
                Node::Int(column as i64 - 1),
                Node::Str(token),
            ]));
        }
        start = column;
    }
    Some(Node::object(vec![
        ("row", Node::Int(row as i64)),
        ("runs", Node::Array(runs)),
    ]))
}

/// The conformance document for one frame.
pub fn frame_document(cast: &str, instant: &str, frame: &RenderedFrame) -> Node {
    let rows: Vec<Node> = (0..frame.screen.rows)
        .filter_map(|row| attribute_runs(frame, row))
        .collect();
    Node::object(vec![
        ("schema", Node::string(FRAME_SCHEMA)),
        ("cast", Node::string(cast)),
        ("instant", Node::string(instant)),
        ("cli", Node::optional_string(&frame.cli)),
        ("cli_version", Node::optional_string(&frame.cli_version)),
        ("columns", Node::Int(frame.screen.columns as i64)),
        ("rows", Node::Int(frame.screen.rows as i64)),
        (
            "requested_time_ms",
            Node::Int(milliseconds(frame.requested_time)),
        ),
        (
            "effective_time_ms",
            Node::Int(milliseconds(frame.effective_time)),
        ),
        ("events_applied", Node::Int(frame.events_applied as i64)),
        (
            "unsupported_sequences",
            Node::Int(frame.unsupported_sequences as i64),
        ),
        ("unsupported_resize", Node::Bool(frame.unsupported_resize)),
        (
            "alternate_screen",
            Node::Bool(frame.screen.is_alternate_screen),
        ),
        ("window_title", Node::string(frame.window_title.clone())),
        (
            "cursor",
            Node::object(vec![
                ("row", Node::Int(frame.screen.cursor_row as i64)),
                ("column", Node::Int(frame.screen.cursor_column as i64)),
                ("visible", Node::Bool(frame.screen.cursor_visible)),
            ]),
        ),
        (
            "lines",
            Node::Array(frame.lines().into_iter().map(Node::Str).collect()),
        ),
        ("attributes", Node::Array(rows)),
    ])
}

/// The whole slice in one call: cast text plus an instant, JSON out. This is
/// the function the UniFFI surface exports, because a string in and a string
/// out is the one signature that is identical in every language.
pub fn render_frame_document(
    cast_text: &str,
    cast_name: &str,
    instant: &str,
    time_ms: i64,
) -> Result<String, crate::cast::TranscriptError> {
    let player = crate::cast::CastPlayer::parse(cast_text)?;
    let frame = player.frame(time_ms as f64 / 1000.0);
    Ok(frame_document(cast_name, instant, &frame).to_json())
}
