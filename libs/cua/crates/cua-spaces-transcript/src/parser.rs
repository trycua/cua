//! Stage one: a rendered frame becomes a typed model of what is on screen.
//!
//! A direct port of `libs/spaces-sdk-swift/Sources/CuaSpacesTranscript/
//! ClaudeCodeParser.swift` and `TranscriptElement.swift`. The Swift
//! implementation is the incumbent source of truth; where this file looks odd
//! it is because it reproduces a decision made there, and the fixtures under
//! `contract/fixtures/transcript/jsonui` are what hold the two honest.
//!
//! The parser is a *reader*, not an interpreter. It may say "row 11 contains a
//! tool-call row for `Update` with the argument `calc.py`" because that is what
//! the characters say. It may not say "the agent called the Edit tool with
//! `{file_path: ...}`", because the frame does not contain that.
//!
//! Everything that is a conclusion rather than a reading is
//! [`Provenance::Inferred`] and travels to the consumer that way. Everything
//! the parser cannot classify comes back as [`Element::RawText`] — degrading to
//! plain text is the required behaviour when Claude Code's layout moves, not an
//! error.

use crate::cast::RenderedFrame;
use crate::screen::TerminalColor;

/// The layout this parser was written against.
pub const LAYOUT_PROFILE: &str = "claude-code/2.x";

// MARK: - Provenance

/// How a fact in a parsed frame came to be known.
///
/// This is the load-bearing type in the whole module: the SDK must never mint a
/// structured element the agent did not actually offer. A terminal frame is
/// pixels-as-characters, not an event stream. Some things really are readable
/// off it, and some things are a parser's opinion, and a consumer has to be
/// able to tell which without reading our source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provenance {
    /// The text is on the screen and the element is a direct reading of it.
    Observed,
    /// The element is a conclusion the parser drew from layout, colour or
    /// adjacency, and the agent never said it in those words.
    Inferred,
    /// The parser recognised the region as *something*, could not identify
    /// what, and is handing back the text unchanged rather than guessing.
    Unrecognised,
}

impl Provenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Provenance::Observed => "observed",
            Provenance::Inferred => "inferred",
            Provenance::Unrecognised => "unrecognised",
        }
    }
}

/// Where on the screen an element was read from. Always present, because a
/// claim about a frame that cannot be pointed at is not checkable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameRegion {
    pub first_row: usize,
    pub last_row: usize,
}

impl FrameRegion {
    pub fn new(first_row: usize, last_row: usize) -> Self {
        FrameRegion {
            first_row,
            last_row,
        }
    }
    pub fn row(row: usize) -> Self {
        FrameRegion::new(row, row)
    }
}

/// The permission/mode indicator states, spelled as Claude Code 2.1.x draws
/// them rather than as a tidier vocabulary invented here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PermissionMode {
    Manual,
    Auto,
    AcceptEdits,
    Plan,
    BypassPermissions,
    Unrecognised,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Manual => "manual",
            PermissionMode::Auto => "auto",
            PermissionMode::AcceptEdits => "accept_edits",
            PermissionMode::Plan => "plan",
            PermissionMode::BypassPermissions => "bypass_permissions",
            PermissionMode::Unrecognised => "unrecognised",
        }
    }
}

/// A tool call's status as far as the frame can support.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolCallStatus {
    Succeeded,
    Failed,
    /// No result line yet. Always inferred — "not finished" is our reading of
    /// an absence.
    Running,
    Completed,
    Unknown,
}

impl ToolCallStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolCallStatus::Succeeded => "succeeded",
            ToolCallStatus::Failed => "failed",
            ToolCallStatus::Running => "running",
            ToolCallStatus::Completed => "completed",
            ToolCallStatus::Unknown => "unknown",
        }
    }
}

// MARK: - Element payloads

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentMessage {
    pub markdown: String,
    pub text: String,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UserMessage {
    pub text: String,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ToolCall {
    /// The tool's name exactly as drawn, e.g. `Update`, `Read`, `Bash`.
    pub name: String,
    /// The single argument Claude Code shows in parentheses, verbatim.
    pub argument_summary: Option<String>,
    pub status: ToolCallStatus,
    /// Whether `status` was read or concluded. Split out from `provenance`
    /// because the *name* can be observed while the *status* is inferred, and
    /// collapsing the two would overstate one or understate the other.
    pub status_provenance: Provenance,
    pub result_lines: Vec<String>,
    pub is_collapsed: bool,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Subagent {
    pub kind: String,
    pub task: Option<String>,
    pub status: ToolCallStatus,
    pub status_provenance: Provenance,
    pub detail_lines: Vec<String>,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentRunning {
    pub spinner: Option<String>,
    /// The word the CLI is using this tick ("Baking", "Thinking", …). Claude
    /// Code rotates these; they are quoted, never interpreted.
    pub verb: Option<String>,
    pub elapsed_seconds: Option<i64>,
    pub tokens: Option<i64>,
    pub hint: Option<String>,
    pub raw_line: String,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Composer {
    pub content: String,
    pub placeholder: Option<String>,
    /// Whether the terminal cursor is inside the composer's box. This is the
    /// only focus signal a frame carries, and it is exactly that.
    pub cursor_is_inside: bool,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModeIndicator {
    pub mode: PermissionMode,
    /// The footer text as drawn, always. When `mode` is `Unrecognised` this is
    /// the only thing a consumer should show.
    pub raw_text: String,
    pub hint: Option<String>,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestionOption {
    pub key: Option<String>,
    pub label: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Question {
    pub prompt: String,
    pub options: Vec<QuestionOption>,
    pub selected_index: Option<usize>,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffLineKind {
    Added,
    Removed,
    Context,
}

impl DiffLineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffLineKind::Added => "added",
            DiffLineKind::Removed => "removed",
            DiffLineKind::Context => "context",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub number: Option<i64>,
    pub text: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diff {
    pub path: Option<String>,
    pub hunk_lines: Vec<DiffLine>,
    pub added_count: i64,
    pub removed_count: i64,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ErrorBanner {
    pub text: String,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TurnBoundaryKind {
    Completed,
    Interrupted,
}

impl TurnBoundaryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnBoundaryKind::Completed => "completed",
            TurnBoundaryKind::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TurnBoundary {
    pub kind: TurnBoundaryKind,
    pub text: String,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawText {
    pub lines: Vec<String>,
    pub region: FrameRegion,
    pub provenance: Provenance,
}

/// A single structured thing on the screen.
///
/// Deliberately a flat enum rather than a class hierarchy: a frame is a list of
/// regions read top to bottom, and anything more elaborate would imply
/// structure the terminal does not have. It crosses the FFI boundary as JSON
/// with a `type` discriminator rather than as a UniFFI sum type, because
/// UniFFI 0.31 sum types with payloads do not round-trip cleanly to Swift,
/// TypeScript and Python at once.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Element {
    AgentMessage(AgentMessage),
    UserMessage(UserMessage),
    ToolCall(ToolCall),
    Subagent(Subagent),
    AgentRunning(AgentRunning),
    Composer(Composer),
    ModeIndicator(ModeIndicator),
    Question(Question),
    Diff(Diff),
    Error(ErrorBanner),
    TurnBoundary(TurnBoundary),
    RawText(RawText),
}

impl Element {
    pub fn type_name(&self) -> &'static str {
        match self {
            Element::AgentMessage(_) => "agentMessage",
            Element::UserMessage(_) => "userMessage",
            Element::ToolCall(_) => "toolCall",
            Element::Subagent(_) => "subagent",
            Element::AgentRunning(_) => "agentRunning",
            Element::Composer(_) => "composer",
            Element::ModeIndicator(_) => "modeIndicator",
            Element::Question(_) => "question",
            Element::Diff(_) => "diff",
            Element::Error(_) => "error",
            Element::TurnBoundary(_) => "turnBoundary",
            Element::RawText(_) => "rawText",
        }
    }

    pub fn provenance(&self) -> Provenance {
        match self {
            Element::AgentMessage(v) => v.provenance,
            Element::UserMessage(v) => v.provenance,
            Element::ToolCall(v) => v.provenance,
            Element::Subagent(v) => v.provenance,
            Element::AgentRunning(v) => v.provenance,
            Element::Composer(v) => v.provenance,
            Element::ModeIndicator(v) => v.provenance,
            Element::Question(v) => v.provenance,
            Element::Diff(v) => v.provenance,
            Element::Error(v) => v.provenance,
            Element::TurnBoundary(v) => v.provenance,
            Element::RawText(v) => v.provenance,
        }
    }

    pub fn region(&self) -> FrameRegion {
        match self {
            Element::AgentMessage(v) => v.region,
            Element::UserMessage(v) => v.region,
            Element::ToolCall(v) => v.region,
            Element::Subagent(v) => v.region,
            Element::AgentRunning(v) => v.region,
            Element::Composer(v) => v.region,
            Element::ModeIndicator(v) => v.region,
            Element::Question(v) => v.region,
            Element::Diff(v) => v.region,
            Element::Error(v) => v.region,
            Element::TurnBoundary(v) => v.region,
            Element::RawText(v) => v.region,
        }
    }

    /// The strings an `observed` element asserts are on the screen.
    ///
    /// Deliberately excludes anything reassembled across rows: a wrapped agent
    /// message is stitched from several rows and no single row contains it, so
    /// quoting the whole thing would fail for the right reason and the wrong
    /// assertion. Single-row text is checked in full.
    pub fn quotable_strings(&self) -> Vec<String> {
        match self {
            Element::ToolCall(v) => {
                let mut out = vec![v.name.clone()];
                out.extend(v.argument_summary.clone());
                out
            }
            Element::Subagent(v) => {
                let mut out = vec![v.kind.clone()];
                out.extend(v.task.clone());
                out
            }
            Element::AgentRunning(v) => vec![v.raw_line.clone()],
            Element::ModeIndicator(v) => vec![v.raw_text.clone()],
            Element::TurnBoundary(v) => vec![v.text.clone()],
            Element::Question(v) => {
                let mut out = vec![v.prompt.clone()];
                out.extend(v.options.iter().map(|o| o.label.clone()));
                out
            }
            Element::Composer(v) => {
                if v.content.contains('\n') || v.content.is_empty() {
                    vec![]
                } else {
                    vec![v.content.clone()]
                }
            }
            Element::UserMessage(v) => {
                if v.text.contains('\n') {
                    vec![]
                } else {
                    vec![v.text.clone()]
                }
            }
            Element::AgentMessage(v) => {
                if v.text.contains('\n') {
                    vec![]
                } else {
                    vec![v.text.clone()]
                }
            }
            Element::Diff(_) | Element::Error(_) | Element::RawText(_) => vec![],
        }
    }
}

/// Everything the parser concluded about one frame.
#[derive(Clone, PartialEq, Debug)]
pub struct ParsedFrame {
    pub elements: Vec<Element>,
    /// The CLI the frame came from and the version that drew it, carried
    /// through from the recording. A structured reading of a terminal is only
    /// meaningful against the layout it was written for.
    pub cli: Option<String>,
    pub cli_version: Option<String>,
    /// The layout profile that matched. `None` means nothing matched and the
    /// whole frame degraded to raw text.
    pub layout_profile: Option<String>,
    pub frame_time: f64,
    pub unsupported_sequences: u32,
    pub is_alternate_screen: bool,
}

impl ParsedFrame {
    pub fn observed(&self) -> Vec<&Element> {
        self.elements
            .iter()
            .filter(|e| e.provenance() == Provenance::Observed)
            .collect()
    }
    pub fn inferred(&self) -> Vec<&Element> {
        self.elements
            .iter()
            .filter(|e| e.provenance() == Provenance::Inferred)
            .collect()
    }
}

// MARK: - Vocabulary

/// The glyphs Claude Code uses to open a transcript entry.
const BULLET_GLYPHS: &[char] = &['⏺', '●'];
/// The glyph that opens a result/continuation row.
const CONTINUATION_GLYPHS: &[char] = &['⎿', '└', '╰'];
/// The spinner frames. Claude Code rotates through these on the working row and
/// reuses the last one on the completion line.
const SPINNER_GLYPHS: &[char] = &['✻', '✽', '✶', '✳', '✢', '·', '∗', '*'];
/// The glyphs that open a prompt row: the composer's own marker and the echo of
/// a sent message.
const PROMPT_GLYPHS: &[char] = &['❯', '>', '›'];
/// Glyphs that open an error banner.
const ERROR_GLYPHS: &[char] = &['✗', '✘', '×', '⚠', '🛑'];
/// Tools whose row means "a subagent is doing this".
const SUBAGENT_TOOLS: &[&str] = &[
    "Task",
    "Agent",
    "Explore",
    "Plan",
    "general-purpose",
    "Subagent",
];
/// The leading words of a bullet-less activity row. Deliberately a closed list:
/// an open rule ("any capitalised first word") turns every sentence into a tool
/// call.
const ACTIVITY_VERBS: &[&str] = &[
    "Read",
    "Reading",
    "Wrote",
    "Writing",
    "Ran",
    "Running",
    "Listed",
    "Listing",
    "Searched",
    "Searching",
    "Fetched",
    "Fetching",
    "Updated",
    "Updating",
    "Created",
    "Creating",
    "Edited",
    "Editing",
    "Found",
    "Deleted",
    "Deleting",
    "Globbed",
    "Grepped",
];
const ERROR_MARKERS: &[&str] = &[
    "error",
    "failed",
    "no such file",
    "denied",
    "not found",
    "cannot ",
    "exception",
];
const SUCCESS_MARKERS: &[&str] = &[
    "added ", "updated ", "read ", "wrote ", "applied ", "found ", "listed ", "removed ",
    "created ",
];
const COLLAPSED_HINTS: &[&str] = &[
    "ctrl+o to expand",
    "ctrl-o to expand",
    "… +",
    "... +",
    "more lines",
    "expand",
];
/// 256-colour background indices Claude Code uses for diff shading. Two
/// families because the palette differs between its light and dark themes.
const ADDED_BACKGROUNDS: &[u8] = &[22, 28, 29, 35, 65, 151, 194, 2];
const REMOVED_BACKGROUNDS: &[u8] = &[52, 88, 89, 95, 124, 131, 224, 1];

// MARK: - String helpers
//
// Swift's `trimmingCharacters(in: .whitespaces)` is Unicode Zs plus tab, and
// deliberately not newlines. Rows never contain newlines, so the set below is
// the same set.

fn is_space_like(c: char) -> bool {
    c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}'))
}

fn trimmed(text: &str) -> String {
    text.trim_matches(is_space_like).to_string()
}

fn drop_spaces(text: &str) -> &str {
    text.trim_start_matches(' ')
}

fn leading_spaces(text: &str) -> usize {
    text.chars().take_while(|c| *c == ' ').count()
}

fn first_char(text: &str) -> Option<char> {
    text.chars().next()
}

fn drop_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next();
    chars.collect()
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn is_horizontal_rule(text: &str) -> bool {
    let text = trimmed(text);
    char_count(&text) >= 20 && text.chars().all(|c| c == '─' || c == '━' || c == '—')
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

// MARK: - The five capture patterns
//
// Hand-rolled rather than pulled from a regex crate: five fixed shapes are
// cheaper to read than a dependency, and each one is exercised by the fixtures.

/// `\((shift\+tab to cycle[^)]*)\)`, case-insensitive.
fn capture_cycle_hint(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let needle: Vec<char> = "shift+tab to cycle".chars().collect();
    if lower.len() != chars.len() {
        // A case fold that changes length cannot be index-aligned; fall back to
        // refusing the hint rather than slicing at the wrong place.
        return None;
    }
    for start in 0..chars.len() {
        if chars[start] != '(' {
            continue;
        }
        let body = start + 1;
        if body + needle.len() > chars.len() || lower[body..body + needle.len()] != needle[..] {
            continue;
        }
        let mut end = body;
        while end < chars.len() && chars[end] != ')' {
            end += 1;
        }
        if end == chars.len() {
            continue;
        }
        return Some(chars[body..end].iter().collect());
    }
    None
}

/// `for (\d+)s` / `\((\d+)s`, case-insensitive. The literal is ASCII, so the
/// case fold is index-preserving here.
fn capture_digits_after(text: &str, prefix: &str, suffix: char) -> Option<i64> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    if lower.len() != chars.len() {
        return None;
    }
    let needle: Vec<char> = prefix.to_lowercase().chars().collect();
    for start in 0..chars.len() {
        if start + needle.len() > chars.len() || lower[start..start + needle.len()] != needle[..] {
            continue;
        }
        let mut end = start + needle.len();
        let digits_start = end;
        while end < chars.len() && chars[end].is_ascii_digit() {
            end += 1;
        }
        if end == digits_start || end >= chars.len() || chars[end] != suffix {
            continue;
        }
        let digits: String = chars[digits_start..end].iter().collect();
        if let Ok(value) = digits.parse::<i64>() {
            return Some(value);
        }
    }
    None
}

/// `([\d,]+) tokens`, leftmost match, with the regex's own backtracking.
fn capture_tokens(text: &str) -> Option<i64> {
    let chars: Vec<char> = text.chars().collect();
    let lower: String = text.to_lowercase();
    let lower_chars: Vec<char> = lower.chars().collect();
    if lower_chars.len() != chars.len() {
        return None;
    }
    let tail: Vec<char> = " tokens".chars().collect();
    for start in 0..chars.len() {
        if !(chars[start].is_ascii_digit() || chars[start] == ',') {
            continue;
        }
        let mut longest = start;
        while longest < chars.len() && (chars[longest].is_ascii_digit() || chars[longest] == ',') {
            longest += 1;
        }
        let mut end = longest;
        while end > start {
            if end + tail.len() <= chars.len() && lower_chars[end..end + tail.len()] == tail[..] {
                let digits: String = chars[start..end].iter().filter(|c| **c != ',').collect();
                if let Ok(value) = digits.parse::<i64>() {
                    return Some(value);
                }
            }
            end -= 1;
        }
    }
    None
}

/// `(esc to interrupt[^)]*)`, case-insensitive.
fn capture_interrupt_hint(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let lower_chars: Vec<char> = text.to_lowercase().chars().collect();
    if lower_chars.len() != chars.len() {
        return None;
    }
    let needle: Vec<char> = "esc to interrupt".chars().collect();
    for start in 0..chars.len() {
        if start + needle.len() > chars.len()
            || lower_chars[start..start + needle.len()] != needle[..]
        {
            continue;
        }
        let mut end = start + needle.len();
        while end < chars.len() && chars[end] != ')' {
            end += 1;
        }
        return Some(chars[start..end].iter().collect());
    }
    None
}

// MARK: - Shape recognisers

/// `Name(argument)` where `Name` is a bare identifier. Anything else is prose
/// that happens to contain a bracket.
pub fn tool_call_pattern(text: &str) -> Option<(String, Option<String>)> {
    let chars: Vec<char> = text.chars().collect();
    let open = chars.iter().position(|c| *c == '(');
    let ends_with_close = chars.last() == Some(&')');
    match (open, ends_with_close) {
        (Some(open), true) => {
            let name: String = chars[..open].iter().collect();
            if name.is_empty()
                || !name.chars().all(|c| c.is_alphabetic() || c == '_')
                || !name.chars().next().map(char::is_uppercase).unwrap_or(false)
            {
                return None;
            }
            let argument: String = chars[open + 1..chars.len() - 1].iter().collect();
            Some((
                name,
                if argument.is_empty() {
                    None
                } else {
                    Some(argument)
                },
            ))
        }
        _ => {
            // A bare `Name` with no argument is also a tool row the CLI draws
            // (e.g. `TodoWrite`), but only when the whole line is one word in
            // PascalCase — otherwise every one-word sentence becomes a call.
            let trimmed_text = trimmed(text);
            if trimmed_text.contains(' ')
                || char_count(&trimmed_text) < 3
                || !trimmed_text
                    .chars()
                    .next()
                    .map(char::is_uppercase)
                    .unwrap_or(false)
                || !trimmed_text.chars().all(char::is_alphabetic)
            {
                return None;
            }
            Some((trimmed_text, None))
        }
    }
}

/// `     5  def subtract(a, b):` (context) / `     9 +def multiply(a, b):`
/// (added): a right-aligned line number, an optional sign, then source.
pub fn numbered_source_line(text: &str) -> Option<(Option<i64>, Option<char>, String)> {
    let stripped: String = text
        .chars()
        .skip_while(|c| *c == ' ' || *c == '\u{a0}')
        .collect();
    let digits: String = stripped.chars().take_while(|c| c.is_numeric()).collect();
    if digits.is_empty() {
        return None;
    }
    let number: i64 = digits.parse().ok()?;
    let mut rest: String = stripped.chars().skip(char_count(&digits)).collect();
    if !rest.starts_with(' ') {
        return None;
    }
    rest = drop_first(&rest);
    if let Some(first) = first_char(&rest)
        && (first == '+' || first == '-')
    {
        return Some((Some(number), Some(first), drop_first(&rest)));
    }
    // An unchanged line still reserves the sign column, so drop the blank that
    // stands where a `+` would be. Leaving it in shifts every context line one
    // space right relative to the lines around it.
    if rest.starts_with(' ') {
        rest = drop_first(&rest);
    }
    Some((Some(number), None, rest))
}

/// `Agent "List functions in calc.py" finished · 4s`.
pub fn agent_finished_row(text: &str) -> Option<(String, bool)> {
    if !text.starts_with("Agent \"") {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let closing = chars.iter().skip(7).position(|c| *c == '"')? + 7;
    let task: String = chars[7..closing].iter().collect();
    let tail_raw: String = chars[closing + 1..].iter().collect();
    let tail = trimmed(&tail_raw).to_lowercase();
    if !(tail.starts_with("finished")
        || tail.starts_with("failed")
        || tail.starts_with("stopped")
        || tail.starts_with("errored"))
    {
        return None;
    }
    Some((
        task,
        tail.starts_with("failed") || tail.starts_with("errored"),
    ))
}

/// `❯ 1. Yes` / `  2. No, and tell Claude what to do differently`.
pub fn option_pattern(text: &str) -> Option<(Option<String>, String, bool)> {
    let mut working = text.to_string();
    let mut selected = false;
    for marker in ["❯", ">", "▸", "→"] {
        if working.starts_with(marker) {
            working = trimmed(&working[marker.len()..]);
            selected = true;
            break;
        }
    }
    let digits: String = working.chars().take_while(|c| c.is_numeric()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest: String = working.chars().skip(char_count(&digits)).collect();
    if !(rest.starts_with('.') || rest.starts_with(')')) {
        return None;
    }
    let label = trimmed(&drop_first(&rest));
    if label.is_empty() {
        return None;
    }
    Some((Some(digits), label, selected))
}

/// Reconstruct Markdown source from lines the CLI has already rendered.
///
/// Genuinely lossy, and the type system should not pretend otherwise: bold
/// drawn with SGR is gone by the time the text is read. What survives is block
/// structure, which is the part a renderer needs.
pub fn markdown_from_screen_lines(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| {
            if line.starts_with("• ") || line.starts_with("◦ ") || line.starts_with("- ") {
                format!(
                    "- {}",
                    &line[line
                        .char_indices()
                        .nth(2)
                        .map(|(i, _)| i)
                        .unwrap_or(line.len())..]
                )
            } else {
                line.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// MARK: - The parser

#[derive(Clone, Copy, Default)]
pub struct ClaudeCodeParser;

struct ComposerBox {
    first_row: usize,
    last_row: usize,
    body: Vec<String>,
}

impl ClaudeCodeParser {
    pub fn new() -> Self {
        ClaudeCodeParser
    }

    pub fn parse(&self, frame: &RenderedFrame) -> ParsedFrame {
        let lines = frame.lines();

        // A frame that is not on the alternate screen is not the agent UI.
        // Claude Code enters the alternate buffer for its TUI; shell
        // scrollback, an exited session, or a `claude -p` run is not something
        // this layout profile describes, and claiming otherwise would be the
        // whole defect. Hand the text back and say so.
        if !frame.screen.is_alternate_screen {
            return ParsedFrame {
                elements: non_empty_raw_text(&lines),
                cli: frame.cli.clone(),
                cli_version: frame.cli_version.clone(),
                layout_profile: None,
                frame_time: frame.effective_time,
                unsupported_sequences: frame.unsupported_sequences,
                is_alternate_screen: false,
            };
        }

        let mut elements: Vec<Element> = Vec::new();
        let mut unclassified: Vec<(usize, String)> = Vec::new();
        let mut index = 0usize;

        while index < lines.len() {
            let line = &lines[index];

            if let Some(box_) = self.parse_composer(&lines, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                let last = box_.last_row;
                elements.push(Element::Composer(self.composer_element(&box_, frame)));
                index = last + 1;
                continue;
            }
            if let Some((question, next)) = self.parse_question(&lines, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                elements.push(Element::Question(question));
                index = next;
                continue;
            }
            if let Some(mode) = self.parse_mode_indicator(line, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                elements.push(Element::ModeIndicator(mode));
                index += 1;
                continue;
            }
            // Order matters: the completion line reuses a spinner glyph
            // ("✻ Cogitated for 5s · done 5:58 PM"), so it has to be claimed as
            // a turn boundary before the working-indicator rule sees it.
            if let Some(boundary) = self.parse_turn_boundary(line, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                elements.push(Element::TurnBoundary(boundary));
                index += 1;
                continue;
            }
            if let Some(running) = self.parse_agent_running(line, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                elements.push(Element::AgentRunning(running));
                index += 1;
                continue;
            }
            if let Some(banner) = self.parse_error_banner(line, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                elements.push(Element::Error(banner));
                index += 1;
                continue;
            }
            if let Some((produced, next)) = self.parse_bullet_row(&lines, frame, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                elements.extend(produced);
                index = next;
                continue;
            }
            if let Some((summary, next)) = self.parse_activity_summary(&lines, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                elements.push(Element::ToolCall(summary));
                index = next;
                continue;
            }
            if let Some((user, next)) = self.parse_user_message(&lines, index) {
                flush_unclassified(&mut unclassified, &mut elements);
                elements.push(Element::UserMessage(user));
                index = next;
                continue;
            }

            unclassified.push((index, line.clone()));
            index += 1;
        }
        flush_unclassified(&mut unclassified, &mut elements);

        let matched = elements
            .iter()
            .any(|element| element.provenance() != Provenance::Unrecognised);
        ParsedFrame {
            elements,
            cli: frame.cli.clone(),
            cli_version: frame.cli_version.clone(),
            layout_profile: if matched {
                Some(LAYOUT_PROFILE.to_string())
            } else {
                None
            },
            frame_time: frame.effective_time,
            unsupported_sequences: frame.unsupported_sequences,
            is_alternate_screen: true,
        }
    }

    // MARK: The bullet rows

    /// Claude Code draws every transcript entry as a `⏺` bullet in column 1
    /// followed by either prose (an agent message) or `Name(arg)` (a tool
    /// call), with `⎿` continuation rows underneath for results.
    fn parse_bullet_row(
        &self,
        lines: &[String],
        frame: &RenderedFrame,
        row: usize,
    ) -> Option<(Vec<Element>, usize)> {
        let line = &lines[row];
        let trimmed_leading = drop_spaces(line);
        let first = first_char(trimmed_leading)?;
        if !BULLET_GLYPHS.contains(&first) {
            return None;
        }
        let head = trimmed(&drop_first(trimmed_leading));
        if head.is_empty() {
            return None;
        }

        // Gather the continuation block: the `⎿` result rows and any indented
        // rows that belong with them.
        let mut last_row = row;
        let mut result_lines: Vec<String> = Vec::new();
        let mut scan = row + 1;
        let mut saw_continuation_marker = false;
        while scan < lines.len() {
            let candidate = &lines[scan];
            let stripped = trimmed(candidate);
            if stripped.is_empty() {
                break;
            }
            let first_non_space = first_char(drop_spaces(candidate));
            if let Some(glyph) = first_non_space {
                if BULLET_GLYPHS.contains(&glyph) {
                    break;
                }
                if CONTINUATION_GLYPHS.contains(&glyph) {
                    saw_continuation_marker = true;
                    result_lines.push(trimmed(&drop_first(drop_spaces(candidate))));
                    last_row = scan;
                    scan += 1;
                    continue;
                }
            }
            // An indented row only belongs to this entry once a `⎿` has
            // introduced the block. Otherwise it is a wrapped prose line and is
            // handled as part of the message body below.
            if saw_continuation_marker && candidate.starts_with("    ") {
                result_lines.push(stripped);
                last_row = scan;
                scan += 1;
                continue;
            }
            break;
        }

        let region = FrameRegion::new(row, last_row);

        // `Agent "…" finished · 4s` — the CLI's own completion row for a
        // backgrounded subagent. Both the task and the outcome are literally
        // drawn, so this is observed end to end.
        if let Some((task, failed)) = agent_finished_row(&head) {
            return Some((
                vec![Element::Subagent(Subagent {
                    kind: "Agent".to_string(),
                    task: Some(task),
                    status: if failed {
                        ToolCallStatus::Failed
                    } else {
                        ToolCallStatus::Succeeded
                    },
                    status_provenance: Provenance::Observed,
                    detail_lines: result_lines,
                    region,
                    provenance: Provenance::Observed,
                })],
                last_row + 1,
            ));
        }

        if let Some((name, argument)) = tool_call_pattern(&head) {
            if SUBAGENT_TOOLS.contains(&name.as_str()) {
                let joined = result_lines.join(" ").to_lowercase();
                let (status, status_provenance) =
                    if joined.contains("backgrounded") || joined.contains("running") {
                        // "Backgrounded agent (↓ to manage)" says it was launched,
                        // not that it finished. Reading that as a result would
                        // report a running subagent as done.
                        (ToolCallStatus::Running, Provenance::Inferred)
                    } else if result_lines.is_empty() {
                        (ToolCallStatus::Running, Provenance::Inferred)
                    } else {
                        (status_for(&result_lines), Provenance::Observed)
                    };
                return Some((
                    vec![Element::Subagent(Subagent {
                        kind: name,
                        task: argument,
                        status,
                        status_provenance,
                        detail_lines: result_lines,
                        region,
                        provenance: Provenance::Observed,
                    })],
                    last_row + 1,
                ));
            }
            let collapsed = result_lines
                .iter()
                .any(|line| contains_any(line, COLLAPSED_HINTS));
            let mut produced = vec![Element::ToolCall(ToolCall {
                name,
                argument_summary: argument.clone(),
                status: status_for(&result_lines),
                // "running" is never on the screen. It is our reading of a call
                // row that has no result under it yet, and it is labelled as
                // such so a renderer cannot present it as the agent's word.
                status_provenance: if result_lines.is_empty() {
                    Provenance::Inferred
                } else {
                    Provenance::Observed
                },
                result_lines,
                is_collapsed: collapsed,
                region,
                provenance: Provenance::Observed,
            })];
            if let Some(diff) = self.parse_diff(frame, argument, row + 1, last_row) {
                produced.push(Element::Diff(diff));
            }
            return Some((produced, last_row + 1));
        }

        // A bullet whose continuation block opens with `$ <command>` is a shell
        // call the CLI has titled with the agent's own description rather than
        // a tool name. Both parts are kept: the description as the message it
        // is, and the command as a call whose *name* is our conclusion (the
        // word "Bash" is nowhere on the screen) and so is inferred.
        if let Some(first) = result_lines.first()
            && let Some(command) = first.strip_prefix("$ ")
        {
            let command = command.to_string();
            let rest: Vec<String> = result_lines.iter().skip(1).cloned().collect();
            let collapsed = rest.iter().any(|line| contains_any(line, COLLAPSED_HINTS));
            return Some((
                vec![
                    Element::AgentMessage(AgentMessage {
                        markdown: head.clone(),
                        text: head.clone(),
                        region: FrameRegion::row(row),
                        provenance: Provenance::Observed,
                    }),
                    Element::ToolCall(ToolCall {
                        name: "Bash".to_string(),
                        argument_summary: Some(command),
                        status: if rest.is_empty() {
                            ToolCallStatus::Running
                        } else {
                            status_for(&rest)
                        },
                        status_provenance: Provenance::Inferred,
                        result_lines: rest,
                        is_collapsed: collapsed,
                        region: FrameRegion::new(row + 1, last_row),
                        provenance: Provenance::Inferred,
                    }),
                ],
                last_row + 1,
            ));
        }

        // Not `Name(arg)`: prose. Continue through wrapped lines, which the CLI
        // indents to the bullet's text column.
        //
        // A message is not one run of lines. Claude Code separates paragraphs
        // and list blocks inside a single message with a blank row, so a parser
        // that stops at the first blank splits one message into a paragraph and
        // a pile of raw text — which is exactly the "markdown structure
        // preserved" requirement failing quietly. A blank row is therefore kept
        // only when the row after it is still part of the message body.
        let mut body = vec![head];
        let mut prose = row + 1;
        let mut last_content = row;
        let mut pending_blanks = 0usize;
        while prose < lines.len() {
            let candidate = &lines[prose];
            if trimmed(candidate).is_empty() {
                pending_blanks += 1;
                if pending_blanks > 1 {
                    break;
                }
                prose += 1;
                continue;
            }
            if let Some(glyph) = first_char(drop_spaces(candidate))
                && (BULLET_GLYPHS.contains(&glyph)
                    || CONTINUATION_GLYPHS.contains(&glyph)
                    || PROMPT_GLYPHS.contains(&glyph))
            {
                break;
            }
            if !candidate.starts_with("  ") || is_horizontal_rule(candidate) {
                break;
            }
            if self.parse_mode_indicator(candidate, prose).is_some() {
                break;
            }
            if self.parse_turn_boundary(candidate, prose).is_some() {
                break;
            }
            if self.parse_agent_running(candidate, prose).is_some() {
                break;
            }
            for _ in 0..pending_blanks {
                body.push(String::new());
            }
            pending_blanks = 0;
            body.push(trimmed(candidate));
            last_content = prose;
            prose += 1;
        }
        let text = body.join("\n");
        Some((
            vec![Element::AgentMessage(AgentMessage {
                markdown: markdown_from_screen_lines(&body),
                text,
                region: FrameRegion::new(row, last_content),
                provenance: Provenance::Observed,
            })],
            last_content + 1,
        ))
    }

    // MARK: Diffs

    /// A Claude Code edit draws numbered source lines whose *background* colour
    /// is the only thing distinguishing an addition from a removal. That is why
    /// the emulator keeps attributes: read as text alone, a diff is
    /// indistinguishable from a file listing.
    fn parse_diff(
        &self,
        frame: &RenderedFrame,
        path: Option<String>,
        first_row: usize,
        last_row: usize,
    ) -> Option<Diff> {
        if first_row > last_row {
            return None;
        }
        let mut hunk: Vec<DiffLine> = Vec::new();
        let mut added = 0i64;
        let mut removed = 0i64;
        let mut any_sign_drawn = false;
        let end = last_row.min(frame.screen.rows.saturating_sub(1));
        for row in first_row..=end {
            let text = frame.screen.line(row);
            let Some((number, sign, source)) = numbered_source_line(&text) else {
                continue;
            };
            let kind = if let Some(sign) = sign {
                // 2.1.x draws a literal `+`/`-` between the line number and the
                // source. When it is there, "added" is something the agent
                // wrote, not something we concluded.
                any_sign_drawn = true;
                if sign == '+' {
                    DiffLineKind::Added
                } else {
                    DiffLineKind::Removed
                }
            } else {
                match dominant_background(frame, row) {
                    Some(TerminalColor::Indexed(index)) if ADDED_BACKGROUNDS.contains(&index) => {
                        DiffLineKind::Added
                    }
                    Some(TerminalColor::Indexed(index)) if REMOVED_BACKGROUNDS.contains(&index) => {
                        DiffLineKind::Removed
                    }
                    _ => DiffLineKind::Context,
                }
            };
            match kind {
                DiffLineKind::Added => added += 1,
                DiffLineKind::Removed => removed += 1,
                DiffLineKind::Context => {}
            }
            hunk.push(DiffLine {
                kind,
                number,
                text: source,
            });
        }
        if hunk.is_empty() || added + removed == 0 {
            return None;
        }
        Some(Diff {
            path,
            hunk_lines: hunk,
            added_count: added,
            removed_count: removed,
            region: FrameRegion::new(first_row, last_row),
            // If the `+`/`-` markers were drawn, the classification is a
            // reading. If it rests on background colour alone, it is our
            // conclusion and is labelled as one.
            provenance: if any_sign_drawn {
                Provenance::Observed
            } else {
                Provenance::Inferred
            },
        })
    }

    // MARK: Composer

    /// 2.1.x draws the composer as two full-width horizontal rules with the
    /// prompt rows between them. Earlier builds drew a rounded box, so both are
    /// accepted: recognising one layout and silently mis-reading the other is
    /// the failure mode this module is for.
    fn parse_composer(&self, lines: &[String], row: usize) -> Option<ComposerBox> {
        self.parse_boxed_composer(lines, row)
            .or_else(|| self.parse_ruled_composer(lines, row))
    }

    fn parse_boxed_composer(&self, lines: &[String], row: usize) -> Option<ComposerBox> {
        let line = &lines[row];
        if !(line.starts_with('╭') && line.contains('─') && line.ends_with('╮')) {
            return None;
        }
        let mut scan = row + 1;
        let mut body: Vec<String> = Vec::new();
        while scan < lines.len() {
            let candidate = &lines[scan];
            if candidate.starts_with('╰') {
                return Some(ComposerBox {
                    first_row: row,
                    last_row: scan,
                    body,
                });
            }
            if !candidate.starts_with('│') {
                return None;
            }
            let mut inner = drop_first(candidate);
            if inner.ends_with('│') {
                inner = inner[..inner.len() - '│'.len_utf8()].to_string();
            }
            body.push(inner);
            scan += 1;
            if scan - row > 24 {
                return None;
            }
        }
        None
    }

    fn parse_ruled_composer(&self, lines: &[String], row: usize) -> Option<ComposerBox> {
        if !is_horizontal_rule(&lines[row]) {
            return None;
        }
        let mut scan = row + 1;
        let mut body: Vec<String> = Vec::new();
        while scan < lines.len() && scan - row <= 12 {
            let candidate = &lines[scan];
            if is_horizontal_rule(candidate) {
                // A pair of rules with nothing prompt-shaped between them is a
                // divider, not the composer.
                let prompt_shaped = body.iter().any(|line| {
                    first_char(drop_spaces(line))
                        .map(|glyph| PROMPT_GLYPHS.contains(&glyph))
                        .unwrap_or(false)
                });
                if !prompt_shaped {
                    return None;
                }
                return Some(ComposerBox {
                    first_row: row,
                    last_row: scan,
                    body,
                });
            }
            body.push(candidate.clone());
            scan += 1;
        }
        None
    }

    fn composer_element(&self, box_: &ComposerBox, frame: &RenderedFrame) -> Composer {
        let mut content: Vec<String> = Vec::new();
        let mut prompt_column = 0usize;
        for (offset, raw) in box_.body.iter().enumerate() {
            let mut text = trimmed(raw);
            if let Some(first) = first_char(&text)
                && PROMPT_GLYPHS.contains(&first)
            {
                if offset == 0 {
                    prompt_column = leading_spaces(raw) + 2;
                }
                text = trimmed(&drop_first(&text));
            }
            content.push(text);
        }
        while content.last().map(String::is_empty).unwrap_or(false) {
            content.pop();
        }
        let joined = content.join("\n");

        // A placeholder is dim; typed text is not. Reading it off the cells is
        // the difference between "the user typed nothing" and "the user typed
        // the placeholder", which a text-only parser cannot tell apart.
        let mut placeholder: Option<String> = None;
        if !joined.is_empty() && box_.first_row + 1 < frame.screen.rows {
            let attributes = frame.screen.attributes_for_row(box_.first_row + 1);
            let body_attributes: Vec<_> = attributes
                .into_iter()
                .skip(prompt_column)
                .take(char_count(&joined))
                .collect();
            if !body_attributes.is_empty() && body_attributes.iter().all(|a| a.dim) {
                placeholder = Some(joined.clone());
            }
        }
        let cursor_inside =
            frame.screen.cursor_row > box_.first_row && frame.screen.cursor_row < box_.last_row;
        Composer {
            content: if placeholder.is_none() {
                joined
            } else {
                String::new()
            },
            placeholder,
            cursor_is_inside: cursor_inside,
            region: FrameRegion::new(box_.first_row, box_.last_row),
            provenance: Provenance::Observed,
        }
    }

    // MARK: Mode indicator

    fn parse_mode_indicator(&self, line: &str, row: usize) -> Option<ModeIndicator> {
        let text = trimmed(line);
        if text.is_empty() {
            return None;
        }
        let lowered = text.to_lowercase();
        // The footer is recognisable by its cycle hint, which is the one part
        // that has not moved across releases.
        if !(lowered.contains("shift+tab to cycle")
            || lowered.contains("shift-tab to cycle")
            || lowered.starts_with("⏵⏵")
            || lowered.starts_with('⏸'))
        {
            return None;
        }
        let hint = capture_cycle_hint(&text);
        let mode = if lowered.contains("accept edits") {
            PermissionMode::AcceptEdits
        } else if lowered.contains("plan mode") {
            PermissionMode::Plan
        } else if lowered.contains("bypass") || lowered.contains("dangerously") {
            PermissionMode::BypassPermissions
        } else if lowered.contains("auto mode") {
            PermissionMode::Auto
        } else if lowered.contains("manual mode") {
            PermissionMode::Manual
        } else {
            PermissionMode::Unrecognised
        };
        Some(ModeIndicator {
            mode,
            raw_text: text,
            hint,
            region: FrameRegion::row(row),
            // The words are on screen; mapping them onto our enum is ours. When
            // the mapping fails we say so and hand back the raw text.
            provenance: if mode == PermissionMode::Unrecognised {
                Provenance::Unrecognised
            } else {
                Provenance::Observed
            },
        })
    }

    // MARK: Working indicator

    fn parse_agent_running(&self, line: &str, row: usize) -> Option<AgentRunning> {
        let text = trimmed(line);
        let first = first_char(&text)?;
        if !SPINNER_GLYPHS.contains(&first) {
            return None;
        }
        let rest = trimmed(&drop_first(&text));
        if rest.is_empty() {
            return None;
        }

        // "Channeling… (3s · ↓ 59 tokens)" — the verb is the leading word with
        // its ellipsis removed. Claude Code rotates these words constantly;
        // they are quoted back, never mapped onto a state vocabulary we made
        // up, because "Channeling" does not mean anything the SDK can define.
        let verb = rest
            .split(' ')
            .find(|part| !part.is_empty())
            .map(|word| {
                word.trim_matches(|c| c == '…' || c == '·' || c == '.')
                    .to_string()
            })
            .filter(|word| !word.is_empty());
        let elapsed = capture_digits_after(&rest, "for ", 's')
            .or_else(|| capture_digits_after(&rest, "(", 's'));
        let tokens = capture_tokens(&rest);
        let hint = capture_interrupt_hint(&rest);

        Some(AgentRunning {
            spinner: Some(first.to_string()),
            verb,
            elapsed_seconds: elapsed,
            tokens,
            hint,
            raw_line: text,
            region: FrameRegion::row(row),
            provenance: Provenance::Observed,
        })
    }

    // MARK: Turn boundaries

    fn parse_turn_boundary(&self, line: &str, row: usize) -> Option<TurnBoundary> {
        let text = trimmed(line);
        if text.is_empty() {
            return None;
        }
        let lowered = text.to_lowercase();
        if lowered.contains("interrupted by user") || lowered.contains("request interrupted") {
            return Some(TurnBoundary {
                kind: TurnBoundaryKind::Interrupted,
                text,
                region: FrameRegion::row(row),
                provenance: Provenance::Observed,
            });
        }
        // "✻ Baked for 4s · done 5:55 PM" — the CLI's own completion line.
        if let Some(first) = first_char(&text)
            && SPINNER_GLYPHS.contains(&first)
            && lowered.contains("· done")
        {
            return Some(TurnBoundary {
                kind: TurnBoundaryKind::Completed,
                text,
                region: FrameRegion::row(row),
                provenance: Provenance::Observed,
            });
        }
        None
    }

    // MARK: Error banners

    /// A standalone error row: the CLI marks these with a cross or a bare
    /// `Error:` prefix, outside any tool row. Matching on the marker keeps this
    /// from claiming every sentence that contains the word "error" — including
    /// the agent's own prose *about* an error, which is a message, not a
    /// banner.
    fn parse_error_banner(&self, line: &str, row: usize) -> Option<ErrorBanner> {
        let text = trimmed(line);
        if text.is_empty() {
            return None;
        }
        if let Some(first) = first_char(&text)
            && ERROR_GLYPHS.contains(&first)
        {
            return Some(ErrorBanner {
                text: trimmed(&drop_first(&text)),
                region: FrameRegion::row(row),
                provenance: Provenance::Observed,
            });
        }
        let lowered = text.to_lowercase();
        if !(lowered.starts_with("error:")
            || lowered.starts_with("api error")
            || lowered.starts_with("fatal:"))
        {
            return None;
        }
        Some(ErrorBanner {
            text,
            region: FrameRegion::row(row),
            provenance: Provenance::Observed,
        })
    }

    // MARK: Questions

    fn parse_question(&self, lines: &[String], row: usize) -> Option<(Question, usize)> {
        // The rule is structural, not lexical: a line that asks something,
        // followed by numbered options. Matching on the wording ("Do you want
        // to…") fails the moment the CLI rephrases, and the rephrasing is
        // exactly when a mis-parse would be least visible.
        let text = trimmed(&lines[row]);
        if !text.ends_with('?') || char_count(&text) <= 3 {
            return None;
        }

        let mut options: Vec<QuestionOption> = Vec::new();
        let mut selected: Option<usize> = None;
        let mut scan = row + 1;
        let mut last = row;
        while scan < lines.len() && scan - row < 12 {
            let candidate = trimmed(&lines[scan]);
            if candidate.is_empty() {
                scan += 1;
                continue;
            }
            let Some((key, label, is_selected)) = option_pattern(&candidate) else {
                // Only blank rows may separate the prompt from its first
                // option. Skipping arbitrary prose would let any question mark
                // anywhere on screen adopt an unrelated numbered list.
                if options.is_empty() {
                    return None;
                }
                break;
            };
            if is_selected {
                selected = Some(options.len());
            }
            options.push(QuestionOption { key, label });
            last = scan;
            scan += 1;
        }
        if options.is_empty() {
            return None;
        }
        Some((
            Question {
                prompt: text,
                options,
                selected_index: selected,
                region: FrameRegion::new(row, last),
                provenance: Provenance::Observed,
            },
            last + 1,
        ))
    }

    // MARK: Bullet-less tool activity

    /// While a turn is in flight, and after it collapses, Claude Code draws
    /// tool activity *without* a bullet and without the `Name(arg)` shape:
    /// `  Reading calc.py` with `  ⎿  calc.py` under it, or `  Read 1 file`
    /// once the call has folded away.
    ///
    /// These are emitted as tool calls because that is plainly what they are,
    /// but the whole element is inferred: the CLI never wrote a tool name here.
    /// "Reading" is a participle in a status line, and treating it as
    /// `ToolCall.name == "Reading"` is our reading of the sentence, not the
    /// agent's declaration of a call.
    fn parse_activity_summary(&self, lines: &[String], row: usize) -> Option<(ToolCall, usize)> {
        let line = &lines[row];
        if !line.starts_with("  ") || line.starts_with("   ") {
            return None;
        }
        let text = trimmed(line);
        if !first_char(&text).map(char::is_uppercase).unwrap_or(false) {
            return None;
        }
        let words: Vec<&str> = text.split(' ').filter(|part| !part.is_empty()).collect();
        let head = (*words.first()?).to_string();
        if !ACTIVITY_VERBS.contains(&head.as_str()) {
            return None;
        }

        let mut result_lines: Vec<String> = Vec::new();
        let mut last = row;
        let mut scan = row + 1;
        while scan < lines.len() {
            let candidate = &lines[scan];
            let Some(glyph) = first_char(drop_spaces(candidate)) else {
                break;
            };
            if !CONTINUATION_GLYPHS.contains(&glyph) {
                break;
            }
            result_lines.push(trimmed(&drop_first(drop_spaces(candidate))));
            last = scan;
            scan += 1;
        }
        let argument = words[1..].join(" ");
        let collapsed = result_lines
            .iter()
            .any(|line| contains_any(line, COLLAPSED_HINTS));
        Some((
            ToolCall {
                name: head,
                argument_summary: if argument.is_empty() {
                    None
                } else {
                    Some(argument)
                },
                // An activity row with nothing under it is *not* evidence that
                // the call is still running: it is just as likely to be a
                // finished call the CLI has folded away. `unknown` is the only
                // status this shape supports.
                status: if result_lines.is_empty() {
                    ToolCallStatus::Unknown
                } else {
                    status_for(&result_lines)
                },
                status_provenance: Provenance::Inferred,
                result_lines,
                is_collapsed: collapsed,
                region: FrameRegion::new(row, last),
                provenance: Provenance::Inferred,
            },
            last + 1,
        ))
    }

    // MARK: User messages

    /// The transcript echoes what the user sent as `> text`, left-aligned and
    /// outside any box. The composer is a box and is matched first, so a `>`
    /// row reaching here is a sent message.
    fn parse_user_message(&self, lines: &[String], row: usize) -> Option<(UserMessage, usize)> {
        let line = &lines[row];
        let stripped = drop_spaces(line);
        let marker = first_char(stripped)?;
        if !PROMPT_GLYPHS.contains(&marker) {
            return None;
        }
        let after_marker = drop_first(stripped);
        if !after_marker.starts_with(' ') {
            return None;
        }
        let mut body = vec![trimmed(&drop_first(&after_marker))];
        let mut scan = row + 1;
        while scan < lines.len() {
            let candidate = &lines[scan];
            if !candidate.starts_with("  ") || trimmed(candidate).is_empty() {
                break;
            }
            if let Some(glyph) = first_char(drop_spaces(candidate))
                && (PROMPT_GLYPHS.contains(&glyph)
                    || BULLET_GLYPHS.contains(&glyph)
                    || CONTINUATION_GLYPHS.contains(&glyph)
                    || SPINNER_GLYPHS.contains(&glyph))
            {
                break;
            }
            body.push(trimmed(candidate));
            scan += 1;
        }
        Some((
            UserMessage {
                text: body.join("\n"),
                region: FrameRegion::new(row, scan - 1),
                provenance: Provenance::Observed,
            },
            scan,
        ))
    }
}

fn status_for(result_lines: &[String]) -> ToolCallStatus {
    if result_lines.is_empty() {
        return ToolCallStatus::Running;
    }
    let joined = result_lines.join(" ").to_lowercase();
    if contains_any(&joined, ERROR_MARKERS) {
        return ToolCallStatus::Failed;
    }
    if contains_any(&joined, SUCCESS_MARKERS) {
        return ToolCallStatus::Succeeded;
    }
    ToolCallStatus::Completed
}

fn dominant_background(frame: &RenderedFrame, row: usize) -> Option<TerminalColor> {
    let mut counts: Vec<(TerminalColor, usize)> = Vec::new();
    for column in 0..frame.screen.columns {
        if let Some(background) = frame.screen.get(row, column).attributes.background {
            match counts.iter_mut().find(|(colour, _)| *colour == background) {
                Some((_, count)) => *count += 1,
                None => counts.push((background, 1)),
            }
        }
    }
    // Require a real run, not a stray cell.
    counts
        .into_iter()
        .filter(|(_, count)| *count >= 4)
        .max_by_key(|(_, count)| *count)
        .map(|(colour, _)| colour)
}

fn flush_unclassified(unclassified: &mut Vec<(usize, String)>, elements: &mut Vec<Element>) {
    if unclassified.is_empty() {
        return;
    }
    let body: Vec<String> = unclassified.iter().map(|(_, line)| line.clone()).collect();
    if body.iter().any(|line| !trimmed(line).is_empty()) {
        elements.push(Element::RawText(RawText {
            lines: body,
            region: FrameRegion::new(unclassified[0].0, unclassified[unclassified.len() - 1].0),
            provenance: Provenance::Unrecognised,
        }));
    }
    unclassified.clear();
}

fn non_empty_raw_text(lines: &[String]) -> Vec<Element> {
    let mut out: Vec<Element> = Vec::new();
    let mut run: Vec<(usize, String)> = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        if trimmed(line).is_empty() && run.is_empty() {
            continue;
        }
        run.push((row, line.clone()));
    }
    if run.iter().any(|(_, line)| !trimmed(line).is_empty()) {
        out.push(Element::RawText(RawText {
            lines: run.iter().map(|(_, line)| line.clone()).collect(),
            region: FrameRegion::new(run[0].0, run[run.len() - 1].0),
            provenance: Provenance::Unrecognised,
        }));
    }
    out
}
