//! `cua.transcript.jsonui/2` — the structured reading of a frame, on the wire.
//!
//! Two stages, kept apart on purpose: [`crate::parser::ClaudeCodeParser`] turns
//! a frame into a [`ParsedFrame`] (a Rust model of what is on screen), and
//! *this* file turns that into a document with no Rust in it. A Swift,
//! TypeScript or Python client gets the same structure, and a change to the
//! Rust model that does not change the document is not a wire change.
//!
//! # Why `/2` and not `/1`
//!
//! `libs/spaces-sdk-swift` emits `cua.transcript.jsonui/1`, whose `source`
//! carries `frameTime` as a float in seconds. A float on the wire makes a
//! byte-identity claim across three languages a test of three float formatters,
//! which is the one rule the frame slice settled and this slice inherits. The
//! cross-language document therefore renames that single key to `frameTimeMs`
//! and carries integer milliseconds, and it adds `cast` and `instant` so a
//! fixture identifies itself. A renamed key is a major bump by the schema's own
//! versioning rule, so this is `/2`. Everything else — every element type,
//! every body key, every spelling of `provenance` — is `/1` unchanged.
//!
//! The three rules from `framedoc.rs` hold here too, and a follow-on schema
//! must keep all three: no floats on the wire, key order written rather than
//! sorted, escaping spelled out. The ordered writer in `framedoc.rs` is shared
//! *within* Rust; each other language writes its own, because a shared writer
//! turns a conformance suite into a test of the writer.
//!
//! # Element bodies
//!
//! Every element has `type`, `provenance` and `region` and then a body:
//!
//! * `agentMessage` — `markdown`, `text`
//! * `userMessage` — `text`
//! * `toolCall` — `name`, `argumentSummary`, `status`, `statusProvenance`,
//!   `resultLines`, `collapsed`
//! * `subagent` — `kind`, `task`, `status`, `statusProvenance`, `detailLines`
//! * `agentRunning` — `spinner`, `verb`, `elapsedSeconds`, `tokens`, `hint`,
//!   `rawLine`
//! * `composer` — `content`, `placeholder`, `cursorIsInside`
//! * `modeIndicator` — `mode`, `rawText`, `hint`
//! * `question` — `prompt`, `options`, `selectedIndex`
//! * `diff` — `path`, `addedCount`, `removedCount`, `lines`
//! * `error` — `text`
//! * `turnBoundary` — `kind`, `text`
//! * `rawText` — `lines`; always `provenance: "unrecognised"`
//!
//! `provenance` is not decoration. A renderer that draws a confident card for
//! an `inferred` element is misrepresenting the agent, which is the defect this
//! schema exists to make impossible to commit by accident. `statusProvenance`
//! is separate from the element's own because a tool's *name* can be read off
//! the screen while its *status* is concluded from an absence.

use crate::framedoc::{Node, milliseconds};
use crate::parser::{ClaudeCodeParser, Element, ParsedFrame};

pub const JSONUI_SCHEMA: &str = "cua.transcript.jsonui/2";

fn optional_int(value: Option<i64>) -> Node {
    match value {
        Some(value) => Node::Int(value),
        None => Node::Null,
    }
}

fn strings(values: &[String]) -> Node {
    Node::Array(values.iter().cloned().map(Node::Str).collect())
}

/// The `type`/`provenance`/`region` head every element shares, then its body.
pub fn element_node(element: &Element) -> Node {
    let region = element.region();
    let mut entries: Vec<(String, Node)> = vec![
        ("type".to_string(), Node::string(element.type_name())),
        (
            "provenance".to_string(),
            Node::string(element.provenance().as_str()),
        ),
        (
            "region".to_string(),
            Node::object(vec![
                ("firstRow", Node::Int(region.first_row as i64)),
                ("lastRow", Node::Int(region.last_row as i64)),
            ]),
        ),
    ];
    let body: Vec<(&str, Node)> = match element {
        Element::AgentMessage(value) => vec![
            ("markdown", Node::string(value.markdown.clone())),
            ("text", Node::string(value.text.clone())),
        ],
        Element::UserMessage(value) => vec![("text", Node::string(value.text.clone()))],
        Element::ToolCall(value) => vec![
            ("name", Node::string(value.name.clone())),
            (
                "argumentSummary",
                Node::optional_string(&value.argument_summary),
            ),
            ("status", Node::string(value.status.as_str())),
            (
                "statusProvenance",
                Node::string(value.status_provenance.as_str()),
            ),
            ("resultLines", strings(&value.result_lines)),
            ("collapsed", Node::Bool(value.is_collapsed)),
        ],
        Element::Subagent(value) => vec![
            ("kind", Node::string(value.kind.clone())),
            ("task", Node::optional_string(&value.task)),
            ("status", Node::string(value.status.as_str())),
            (
                "statusProvenance",
                Node::string(value.status_provenance.as_str()),
            ),
            ("detailLines", strings(&value.detail_lines)),
        ],
        Element::AgentRunning(value) => vec![
            ("spinner", Node::optional_string(&value.spinner)),
            ("verb", Node::optional_string(&value.verb)),
            ("elapsedSeconds", optional_int(value.elapsed_seconds)),
            ("tokens", optional_int(value.tokens)),
            ("hint", Node::optional_string(&value.hint)),
            ("rawLine", Node::string(value.raw_line.clone())),
        ],
        Element::Composer(value) => vec![
            ("content", Node::string(value.content.clone())),
            ("placeholder", Node::optional_string(&value.placeholder)),
            ("cursorIsInside", Node::Bool(value.cursor_is_inside)),
        ],
        Element::ModeIndicator(value) => vec![
            ("mode", Node::string(value.mode.as_str())),
            ("rawText", Node::string(value.raw_text.clone())),
            ("hint", Node::optional_string(&value.hint)),
        ],
        Element::Question(value) => vec![
            ("prompt", Node::string(value.prompt.clone())),
            (
                "options",
                Node::Array(
                    value
                        .options
                        .iter()
                        .map(|option| {
                            Node::object(vec![
                                ("key", Node::optional_string(&option.key)),
                                ("label", Node::string(option.label.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "selectedIndex",
                optional_int(value.selected_index.map(|index| index as i64)),
            ),
        ],
        Element::Diff(value) => vec![
            ("path", Node::optional_string(&value.path)),
            ("addedCount", Node::Int(value.added_count)),
            ("removedCount", Node::Int(value.removed_count)),
            (
                "lines",
                Node::Array(
                    value
                        .hunk_lines
                        .iter()
                        .map(|line| {
                            Node::object(vec![
                                ("kind", Node::string(line.kind.as_str())),
                                ("number", optional_int(line.number)),
                                ("text", Node::string(line.text.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ],
        Element::Error(value) => vec![("text", Node::string(value.text.clone()))],
        Element::TurnBoundary(value) => vec![
            ("kind", Node::string(value.kind.as_str())),
            ("text", Node::string(value.text.clone())),
        ],
        Element::RawText(value) => vec![("lines", strings(&value.lines))],
    };
    entries.extend(
        body.into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    );
    Node::Object(entries)
}

/// The conformance document for one parsed frame.
pub fn jsonui_document(cast: &str, instant: &str, parsed: &ParsedFrame) -> Node {
    let source = Node::object(vec![
        ("cli", Node::optional_string(&parsed.cli)),
        ("cliVersion", Node::optional_string(&parsed.cli_version)),
        (
            "layoutProfile",
            Node::optional_string(&parsed.layout_profile),
        ),
        ("frameTimeMs", Node::Int(milliseconds(parsed.frame_time))),
        ("alternateScreen", Node::Bool(parsed.is_alternate_screen)),
        (
            "unsupportedSequences",
            Node::Int(parsed.unsupported_sequences as i64),
        ),
    ]);
    Node::object(vec![
        ("schema", Node::string(JSONUI_SCHEMA)),
        ("cast", Node::string(cast)),
        ("instant", Node::string(instant)),
        ("source", source),
        (
            "elements",
            Node::Array(parsed.elements.iter().map(element_node).collect()),
        ),
    ])
}

/// The whole slice in one call: cast text plus an instant, JSON out. String in
/// and string out is the one signature that is identical in every language, so
/// a byte comparison of the result is a statement about the parser rather than
/// about three binding generators' opinions on struct layout.
pub fn render_jsonui_document(
    cast_text: &str,
    cast_name: &str,
    instant: &str,
    time_ms: i64,
) -> Result<String, crate::cast::TranscriptError> {
    let player = crate::cast::CastPlayer::parse(cast_text)?;
    let frame = player.frame(time_ms as f64 / 1000.0);
    let parsed = ClaudeCodeParser::new().parse(&frame);
    Ok(jsonui_document(cast_name, instant, &parsed).to_json())
}
