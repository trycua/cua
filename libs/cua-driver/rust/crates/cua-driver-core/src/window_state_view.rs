//! Response shaping for `get_window_state`, shared by every platform backend.
//!
//! A backend walks its accessibility API and builds the complete legacy
//! payload (structured `elements`, `tree_markdown`, a text block with the same
//! markdown, plus metadata). [`apply`] then reduces that payload to what a
//! model needs:
//!
//! * ONE tree representation (`tree_format`, default compact markdown),
//! * trimmed metadata (`_note`, `background_input`) unless `verbose`,
//! * a stated truncation line when the walk hit the `max_elements` budget,
//! * with `since: <snapshot_id>`, only the rows that changed against that
//!   snapshot, falling back to a full read when the snapshot is unknown.
//!
//! `full_output: true` restores the previous payload byte for byte (both
//! representations, all metadata, the platform walk caps).
//!
//! The diff works on the rendered markdown rows, which every backend emits in
//! the same `- [N] Role "label" ...` shape, so it also sees display-only rows
//! (static text) that the structured `elements` array omits.

use crate::protocol::{Content, ToolResult};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Default accessibility-node budget for a model-facing read. The platform
/// walkers default to 2 000 (macOS) or 5 000 (Windows, Linux) nodes, which
/// renders 40-100K characters for an ordinary window.
pub const DEFAULT_MAX_ELEMENTS: usize = 250;

/// Snapshots remembered per (pid, window) for `since`.
const STORE_PER_WINDOW: usize = 8;
/// Snapshots remembered across all windows.
const STORE_TOTAL: usize = 32;
/// A remembered snapshot stops being a valid `since` baseline after this long.
const STORE_TTL: Duration = Duration::from_secs(15 * 60);
/// Largest old-rows x new-rows product the line diff will compute.
const DIFF_CELL_LIMIT: usize = 6_000_000;
/// A diff larger than this share (percent) of the full markdown is not worth
/// it: return the full read instead.
const DIFF_MAX_PERCENT_OF_FULL: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeFormat {
    /// Compact markdown only (default).
    Markdown,
    /// Structured `elements` only.
    Elements,
    /// Both, as before this change.
    Both,
}

impl TreeFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            TreeFormat::Markdown => "markdown",
            TreeFormat::Elements => "elements",
            TreeFormat::Both => "both",
        }
    }
}

/// Caller-controlled shape of the response.
#[derive(Clone, Debug)]
pub struct ViewOptions {
    pub tree_format: TreeFormat,
    pub verbose: bool,
    pub full_output: bool,
    pub since: Option<String>,
}

/// JSON schema fragments for the shared arguments, so every backend declares
/// the same properties.
pub fn schema_properties() -> Value {
    json!({
        "tree_format": {
            "type": "string",
            "enum": ["markdown", "elements", "both"],
            "description": "Which tree representation to return. Default \"markdown\": the compact `tree_markdown` only, where a row `[N]` is addressed with element_token `<snapshot_id>:N`. \"elements\": the structured `elements` array only (per-element element_token, frame, parent_index, value). \"both\": the two together, as before; roughly doubles the size."
        },
        "since": {
            "type": "string",
            "description": "A `snapshot_id` from an earlier get_window_state of the same window. Returns only what changed against that snapshot: `+` added rows, `~` changed rows, `-` removed rows (removed ids are the old snapshot's), or `no change`. Falls back to a full read when the snapshot is unknown, expired, belongs to another window, or was read with a different query/max_elements/max_depth; `since_status` says which. The response still carries a NEW snapshot_id and its element_tokens; rows not listed keep their previous [N] unless a `reindexed:` line says otherwise."
        },
        "verbose": {
            "type": "boolean",
            "description": "Default false. Set true to include the `_note` string and the full `background_input` capability report (macOS), which are omitted by default to save context."
        },
        "full_output": {
            "type": "boolean",
            "description": "Default false. Set true to restore the previous full response: both `elements` and `tree_markdown`, all metadata, and the platform's own walk limits unless max_elements / max_depth are passed. Does not combine with `since`."
        }
    })
}

/// Add the shared `tree_format` / `since` / `verbose` / `full_output`
/// properties to a backend's `get_window_state` input schema.
pub fn extend_input_schema(mut schema: Value) -> Value {
    if let (Some(props), Some(extra)) = (
        schema.get_mut("properties").and_then(Value::as_object_mut),
        schema_properties().as_object().cloned(),
    ) {
        props.extend(extra);
    }
    schema
}

impl ViewOptions {
    pub fn from_args(args: &Value) -> Result<Self, ToolResult> {
        let full_output = args.get("full_output").and_then(Value::as_bool) == Some(true);
        let tree_format = match args.get("tree_format") {
            None | Some(Value::Null) => {
                if full_output {
                    TreeFormat::Both
                } else {
                    TreeFormat::Markdown
                }
            }
            Some(Value::String(s)) => match s.as_str() {
                "markdown" => TreeFormat::Markdown,
                "elements" => TreeFormat::Elements,
                "both" => TreeFormat::Both,
                other => {
                    return Err(ToolResult::error(format!(
                        "tree_format must be one of \"markdown\", \"elements\", \"both\"; got \"{other}\"."
                    )))
                }
            },
            Some(_) => {
                return Err(ToolResult::error(
                    "tree_format must be one of \"markdown\", \"elements\", \"both\".",
                ))
            }
        };
        let since = match args.get("since") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            Some(_) => {
                return Err(ToolResult::error(
                    "since must be a snapshot_id string from an earlier get_window_state.",
                ))
            }
        };
        Ok(Self {
            tree_format,
            verbose: args.get("verbose").and_then(Value::as_bool) == Some(true),
            full_output,
            since: if full_output { None } else { since },
        })
    }

    /// Node budget for the walk. An explicit `max_elements` always wins;
    /// otherwise a model-facing read gets [`DEFAULT_MAX_ELEMENTS`], while
    /// `full_output` and internal observation reads keep the platform default.
    pub fn max_elements(
        &self,
        args: &Value,
        platform_default: usize,
        observation_only: bool,
    ) -> usize {
        if let Some(n) = args.get("max_elements").and_then(Value::as_u64) {
            return n.max(1) as usize;
        }
        if self.full_output || observation_only {
            platform_default
        } else {
            DEFAULT_MAX_ELEMENTS.min(platform_default)
        }
    }
}

/// What a `since` read resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SinceOutcome {
    Diff(DiffReport),
    Fallback(&'static str),
}

/// Inputs that identify the view a snapshot was read with. A diff is only
/// meaningful between two reads of the same view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewKey {
    pub query: Option<String>,
    pub max_elements: usize,
    pub max_depth: Option<usize>,
}

pub struct ViewContext<'a> {
    pub pid: i64,
    pub window_id: u64,
    pub key: ViewKey,
    /// Describes the focused element, called only for a `no change` answer.
    pub focus_probe: Option<&'a dyn Fn() -> Option<String>>,
}

/// Shape a finished `get_window_state` payload in place.
pub fn apply(
    opts: &ViewOptions,
    ctx: &ViewContext<'_>,
    content: &mut [Content],
    structured: &mut Value,
) {
    if opts.full_output {
        return;
    }
    trim_metadata(opts, structured);
    let Some(md) = structured
        .get("tree_markdown")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    let snapshot_id = structured
        .get("snapshot_id")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let outcome = opts
        .since
        .as_deref()
        .map(|since| resolve_since(since, snapshot_id.as_deref(), &md, ctx));
    if let Some(sid) = &snapshot_id {
        remember(sid, ctx, &md);
    }

    // Header text added to the first line of the tree block.
    let mut header_extra = String::new();
    if let Some(sid) = &snapshot_id {
        header_extra.push_str(&format!(" snapshot_id={sid}"));
    }
    let truncation = truncation_line(structured, &ctx.key);
    if let Some(line) = &truncation {
        structured["truncation_hint"] = json!(line);
    }

    let body;
    match outcome {
        Some(SinceOutcome::Diff(report)) => {
            let since = opts.since.as_deref().unwrap_or_default();
            let focus = if report.is_empty() {
                ctx.focus_probe.and_then(|probe| probe())
            } else {
                None
            };
            let text = report.render(since, snapshot_id.as_deref(), focus.as_deref());
            structured["since"] = json!(since);
            structured["since_status"] = json!(if report.is_empty() {
                "no_change"
            } else {
                "diff"
            });
            structured["tree_diff"] = json!(text);
            structured["diff_counts"] = json!({
                "added": report.added.len(),
                "changed": report.changed.len(),
                "removed": report.removed.len(),
            });
            if let Some(focus) = focus {
                structured["focused_element"] = json!(focus);
            }
            let keep: std::collections::HashSet<u64> = report
                .added
                .iter()
                .chain(report.changed.iter())
                .filter_map(|row| row.index)
                .collect();
            retain_elements(structured, opts.tree_format, Some(&keep));
            if let Some(obj) = structured.as_object_mut() {
                obj.remove("tree_markdown");
            }
            body = text;
        }
        other => {
            if let Some(SinceOutcome::Fallback(reason)) = &other {
                let since = opts.since.as_deref().unwrap_or_default();
                structured["since"] = json!(since);
                structured["since_status"] = json!(reason);
                header_extra.push_str(&format!(
                    "\nsince={since}: {}; full read follows.",
                    fallback_text(reason)
                ));
            }
            match opts.tree_format {
                TreeFormat::Markdown => {
                    body = md.clone();
                    retain_elements(structured, TreeFormat::Markdown, None);
                }
                TreeFormat::Elements => {
                    body = "(tree is in structuredContent.elements; pass tree_format \"markdown\" for the compact text rendering)".to_owned();
                    if let Some(obj) = structured.as_object_mut() {
                        obj.remove("tree_markdown");
                    }
                }
                TreeFormat::Both => {
                    body = md.clone();
                }
            }
            if opts.tree_format == TreeFormat::Markdown && snapshot_id.is_some() {
                header_extra
                    .push_str("\nelement_token for row [N] = <snapshot_id>:N (e.g. s0000002a:N)");
            }
        }
    }
    structured["tree_format"] = json!(opts.tree_format.as_str());

    if let Some(line) = truncation {
        // Backends that already print the long PARTIAL TREE note keep it.
        let already_noted = content.iter().any(|c| match c {
            Content::Text { text, .. } => text.contains("PARTIAL TREE"),
            _ => false,
        });
        if !already_noted {
            header_extra.push('\n');
            header_extra.push_str(&line);
        }
    }

    rewrite_tree_block(content, &md, &header_extra, &body);
}

fn fallback_text(reason: &str) -> &'static str {
    match reason {
        "unknown_snapshot" => "unknown or expired snapshot_id",
        "other_window" => "that snapshot belongs to a different window",
        "view_changed" => "query, max_elements or max_depth differ from that snapshot's read",
        "too_much_changed" => "most of the tree changed",
        "diff_too_large" => "the trees are too large to compare",
        "no_snapshot" => "this read produced no snapshot to compare against",
        _ => "cannot diff",
    }
}

/// Replace the markdown inside the platform's text block with `body`, and add
/// `header_extra` to the end of that block's first line.
fn rewrite_tree_block(content: &mut [Content], md: &str, header_extra: &str, body: &str) {
    if md.is_empty() {
        return;
    }
    for part in content.iter_mut() {
        let Content::Text { text, .. } = part else {
            continue;
        };
        let Some(pos) = text.find(md) else { continue };
        let prefix = &text[..pos];
        let suffix = &text[pos + md.len()..];
        let mut out = String::with_capacity(text.len());
        match prefix.find('\n') {
            Some(nl) => {
                out.push_str(&prefix[..nl]);
                out.push_str(header_extra);
                out.push_str(&prefix[nl..]);
            }
            None => {
                out.push_str(prefix);
                out.push_str(header_extra);
                out.push('\n');
            }
        }
        out.push_str(body);
        out.push_str(suffix);
        *text = out;
        return;
    }
}

fn trim_metadata(opts: &ViewOptions, structured: &mut Value) {
    if opts.verbose {
        return;
    }
    let degraded = structured.get("degraded").and_then(Value::as_bool) == Some(true);
    if let Some(obj) = structured.as_object_mut() {
        obj.remove("_note");
        // Linux restates the pixel-coordinate convention on every read; the
        // tool description already carries it.
        obj.remove("frame_note");
        if !degraded {
            obj.remove("background_input");
        }
    }
}

/// Keep `elements` per the requested format. `keep` limits them to the given
/// element indices (diff reads).
fn retain_elements(
    structured: &mut Value,
    format: TreeFormat,
    keep: Option<&std::collections::HashSet<u64>>,
) {
    let Some(obj) = structured.as_object_mut() else {
        return;
    };
    if format == TreeFormat::Markdown {
        obj.remove("elements");
        return;
    }
    if let (Some(keep), Some(Value::Array(elements))) = (keep, obj.get_mut("elements")) {
        elements.retain(|e| {
            e.get("element_index")
                .and_then(Value::as_u64)
                .is_some_and(|i| keep.contains(&i))
        });
    }
}

fn truncation_line(structured: &Value, key: &ViewKey) -> Option<String> {
    if structured.get("truncated").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let reason = structured
        .get("truncation_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    if reason != "node_budget" {
        return None;
    }
    let pending = structured
        .get("nodes_pending")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(format!(
        "Tree truncated at max_elements={} ({pending} more node(s) not shown). Pass a larger max_elements, or query/max_depth to reach the rest.",
        key.max_elements
    ))
}

// ── snapshot memory for `since` ──────────────────────────────────────────

#[derive(Clone)]
struct StoredSnapshot {
    snapshot_id: String,
    pid: i64,
    window_id: u64,
    key: ViewKey,
    markdown: String,
    at: Instant,
}

fn store() -> &'static Mutex<VecDeque<StoredSnapshot>> {
    static STORE: OnceLock<Mutex<VecDeque<StoredSnapshot>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn remember(snapshot_id: &str, ctx: &ViewContext<'_>, md: &str) {
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    guard.retain(|s| s.snapshot_id != snapshot_id);
    guard.push_back(StoredSnapshot {
        snapshot_id: snapshot_id.to_owned(),
        pid: ctx.pid,
        window_id: ctx.window_id,
        key: ctx.key.clone(),
        markdown: md.to_owned(),
        at: Instant::now(),
    });
    let same_window = |s: &StoredSnapshot| s.pid == ctx.pid && s.window_id == ctx.window_id;
    while guard.iter().filter(|s| same_window(s)).count() > STORE_PER_WINDOW {
        if let Some(pos) = guard.iter().position(same_window) {
            guard.remove(pos);
        }
    }
    while guard.len() > STORE_TOTAL {
        guard.pop_front();
    }
}

fn resolve_since(
    since: &str,
    current_id: Option<&str>,
    md: &str,
    ctx: &ViewContext<'_>,
) -> SinceOutcome {
    if current_id.is_none() {
        return SinceOutcome::Fallback("no_snapshot");
    }
    let found = {
        let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|s| s.at.elapsed() <= STORE_TTL);
        guard.iter().find(|s| s.snapshot_id == since).cloned()
    };
    let Some(old) = found else {
        return SinceOutcome::Fallback("unknown_snapshot");
    };
    if old.pid != ctx.pid || old.window_id != ctx.window_id {
        return SinceOutcome::Fallback("other_window");
    }
    if old.key != ctx.key {
        return SinceOutcome::Fallback("view_changed");
    }
    match diff_markdown(&old.markdown, md) {
        Err(DiffError::TooLarge) => SinceOutcome::Fallback("diff_too_large"),
        Ok(report) => {
            if !report.is_empty() && report.render_len() * 100 > md.len() * DIFF_MAX_PERCENT_OF_FULL
            {
                SinceOutcome::Fallback("too_much_changed")
            } else {
                SinceOutcome::Diff(report)
            }
        }
    }
}

// ── markdown row diff ────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    indent: usize,
    index: Option<u64>,
    /// Row text without indentation and without the `[N]` index.
    key: String,
    /// Role plus identifier or label; pairs a removed row with its changed
    /// replacement.
    identity: String,
    /// Row text as rendered, index included, without indentation.
    text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DiffReport {
    added: Vec<Row>,
    changed: Vec<Row>,
    removed: Vec<Row>,
    reindexed: Vec<(u64, u64, i64)>,
}

#[derive(Debug, PartialEq, Eq)]
enum DiffError {
    TooLarge,
}

fn parse_rows(md: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for line in md.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("- ") {
            let indent = line.len() - trimmed.len();
            let (index, after) = match rest.strip_prefix('[') {
                Some(r) => match r.split_once(']') {
                    Some((n, tail)) if n.parse::<u64>().is_ok() => {
                        (n.parse::<u64>().ok(), tail.trim_start())
                    }
                    _ => (None, rest),
                },
                None => (None, rest),
            };
            rows.push(Row {
                indent,
                index,
                identity: identity_of(indent, after),
                key: after.to_owned(),
                text: rest.to_owned(),
            });
        } else if let Some(last) = rows.last_mut() {
            if !line.trim().is_empty() {
                // Continuation of a multi-line row (an action name with a
                // newline in it).
                last.key.push('\n');
                last.key.push_str(line);
                last.text.push('\n');
                last.text.push_str(line);
            }
        }
    }
    rows
}

fn identity_of(indent: usize, after_index: &str) -> String {
    let role = after_index.split_whitespace().next().unwrap_or("");
    let id = after_index.find("id=").map(|p| {
        let tail = &after_index[p + 3..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c == ']')
            .unwrap_or(tail.len());
        &tail[..end]
    });
    // A quoted string right after the role is the title; after `=` it is the
    // value, which changes and so cannot identify the row.
    let label = after_index
        .split_once(char::is_whitespace)
        .map(|(_, tail)| tail.trim_start())
        .and_then(|tail| tail.strip_prefix('"'))
        .and_then(|tail| tail.split('"').next());
    format!("{indent}|{role}|{}", id.or(label).unwrap_or(""))
}

fn diff_markdown(old_md: &str, new_md: &str) -> Result<DiffReport, DiffError> {
    let old = parse_rows(old_md);
    let new = parse_rows(new_md);
    let ops = diff_ops(&old, &new)?;

    let mut report = DiffReport::default();
    let mut dels: Vec<usize> = Vec::new();
    let mut inss: Vec<usize> = Vec::new();
    // Unchanged rows whose index moved, grouped into runs of equal delta.
    let mut run: Option<(u64, u64, i64)> = None;

    let flush_hunk = |dels: &mut Vec<usize>, inss: &mut Vec<usize>, report: &mut DiffReport| {
        let mut used = vec![false; dels.len()];
        for &i in inss.iter() {
            let pair = dels
                .iter()
                .enumerate()
                .find(|(d, &o)| !used[*d] && old[o].identity == new[i].identity);
            match pair {
                Some((d, _)) => {
                    used[d] = true;
                    report.changed.push(new[i].clone());
                }
                None => report.added.push(new[i].clone()),
            }
        }
        for (d, &o) in dels.iter().enumerate() {
            if !used[d] {
                report.removed.push(old[o].clone());
            }
        }
        dels.clear();
        inss.clear();
    };

    for op in ops {
        match op {
            Op::Del(o) => dels.push(o),
            Op::Ins(n) => inss.push(n),
            Op::Equal(o, n) => {
                flush_hunk(&mut dels, &mut inss, &mut report);
                if let (Some(oi), Some(ni)) = (old[o].index, new[n].index) {
                    let delta = ni as i64 - oi as i64;
                    match (&mut run, delta) {
                        (Some((_, last_old, d)), delta2) if *d == delta2 && delta2 != 0 => {
                            *last_old = oi;
                        }
                        _ => {
                            if let Some(done) = run.take() {
                                report.reindexed.push(done);
                            }
                            if delta != 0 {
                                run = Some((oi, oi, delta));
                            }
                        }
                    }
                }
            }
        }
    }
    flush_hunk(&mut dels, &mut inss, &mut report);
    if let Some(done) = run.take() {
        report.reindexed.push(done);
    }
    Ok(report)
}

enum Op {
    Equal(usize, usize),
    Del(usize),
    Ins(usize),
}

fn diff_ops(old: &[Row], new: &[Row]) -> Result<Vec<Op>, DiffError> {
    let mut prefix = 0;
    while prefix < old.len()
        && prefix < new.len()
        && old[prefix].key == new[prefix].key
        && old[prefix].indent == new[prefix].indent
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && old[old.len() - 1 - suffix].key == new[new.len() - 1 - suffix].key
        && old[old.len() - 1 - suffix].indent == new[new.len() - 1 - suffix].indent
    {
        suffix += 1;
    }
    let a = &old[prefix..old.len() - suffix];
    let b = &new[prefix..new.len() - suffix];
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > DIFF_CELL_LIMIT {
        return Err(DiffError::TooLarge);
    }
    let same = |x: &Row, y: &Row| x.key == y.key && x.indent == y.indent;

    let mut ops: Vec<Op> = (0..prefix).map(|i| Op::Equal(i, i)).collect();
    // dp[i][j] = LCS length of a[i..] and b[j..].
    let width = m + 1;
    let mut dp = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * width + j] = if same(&a[i], &b[j]) {
                dp[(i + 1) * width + j + 1] + 1
            } else {
                dp[(i + 1) * width + j].max(dp[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if same(&a[i], &b[j]) {
            ops.push(Op::Equal(prefix + i, prefix + j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * width + j] >= dp[i * width + j + 1] {
            ops.push(Op::Del(prefix + i));
            i += 1;
        } else {
            ops.push(Op::Ins(prefix + j));
            j += 1;
        }
    }
    while i < n {
        ops.push(Op::Del(prefix + i));
        i += 1;
    }
    while j < m {
        ops.push(Op::Ins(prefix + j));
        j += 1;
    }
    for k in 0..suffix {
        ops.push(Op::Equal(old.len() - suffix + k, new.len() - suffix + k));
    }
    Ok(ops)
}

impl DiffReport {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    fn render_len(&self) -> usize {
        self.added
            .iter()
            .chain(&self.changed)
            .chain(&self.removed)
            .map(|r| r.text.len() + 3)
            .sum()
    }

    fn render(&self, since: &str, snapshot_id: Option<&str>, focus: Option<&str>) -> String {
        let new_id = snapshot_id.unwrap_or("the new snapshot");
        if self.is_empty() {
            let mut out = format!("no change since {since}");
            if let Some(focus) = focus {
                out.push_str(&format!("; focused element is {focus}"));
            }
            out.push_str(&format!(
                ". Rows and indices are as before; use snapshot_id {new_id} in element_tokens ({new_id}:N)."
            ));
            return out;
        }
        let mut out = format!(
            "since {since}: {} added, {} changed, {} removed. Use element_tokens {new_id}:N for + and ~ rows and unchanged rows; - ids are from {since}.\n",
            self.added.len(),
            self.changed.len(),
            self.removed.len()
        );
        for row in &self.added {
            out.push_str(&format!("+ {}\n", row.text));
        }
        for row in &self.changed {
            out.push_str(&format!("~ {}\n", row.text));
        }
        for row in &self.removed {
            out.push_str(&format!("- {}\n", row.text));
        }
        const MAX_RUNS: usize = 20;
        for (first_old, last_old, delta) in self.reindexed.iter().take(MAX_RUNS) {
            out.push_str(&format!(
                "reindexed: [{first_old}-{last_old}] -> [{}-{}]\n",
                *first_old as i64 + delta,
                *last_old as i64 + delta
            ));
        }
        if self.reindexed.len() > MAX_RUNS {
            out.push_str("reindexed: more shifted ranges not listed; pass full_output or omit since for a fresh index.\n");
        }
        out.truncate(out.trim_end().len());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(args: Value) -> ViewOptions {
        ViewOptions::from_args(&args).unwrap()
    }

    const OLD: &str = "- [0] AXWindow \"Doc\"\n  - [1] AXTextField \"Name\" = \"a\" [id=name]\n  - AXStaticText = \"hello\"\n  - [2] AXButton \"Save\" [id=save actions=[press]]\n  - [3] AXButton \"Cancel\" [id=cancel actions=[press]]";

    #[test]
    fn identical_trees_report_no_change() {
        let report = diff_markdown(OLD, OLD).unwrap();
        assert!(report.is_empty());
        let text = report.render(
            "s00000001",
            Some("s00000002"),
            Some("[1] AXTextField \"Name\""),
        );
        assert!(text.starts_with("no change since s00000001; focused element is [1]"));
        assert!(text.contains("s00000002:N"));
    }

    #[test]
    fn value_edit_is_a_change_and_removal_and_addition_are_reported() {
        let new = "- [0] AXWindow \"Doc\"\n  - [1] AXTextField \"Name\" = \"ab\" [id=name]\n  - AXStaticText = \"hello\"\n  - [2] AXButton \"Save\" [id=save actions=[press]]\n  - [3] AXButton \"Help\" [id=help actions=[press]]";
        let report = diff_markdown(OLD, new).unwrap();
        assert_eq!(report.changed.len(), 1);
        assert_eq!(report.changed[0].index, Some(1));
        assert_eq!(report.added.len(), 1);
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].index, Some(3));
        let text = report.render("sA", Some("sB"), None);
        assert!(text.contains("~ [1] AXTextField \"Name\" = \"ab\""));
        assert!(text.contains("+ [3] AXButton \"Help\""));
        assert!(text.contains("- [3] AXButton \"Cancel\""));
    }

    #[test]
    fn inserted_row_reports_reindexed_range() {
        let new = "- [0] AXWindow \"Doc\"\n  - [1] AXTextField \"Name\" = \"a\" [id=name]\n  - AXStaticText = \"hello\"\n  - [2] AXCheckBox \"New\" [id=new]\n  - [3] AXButton \"Save\" [id=save actions=[press]]\n  - [4] AXButton \"Cancel\" [id=cancel actions=[press]]";
        let report = diff_markdown(OLD, new).unwrap();
        assert_eq!(report.added.len(), 1);
        assert!(report.removed.is_empty());
        assert_eq!(report.reindexed, vec![(2, 3, 1)]);
        assert!(report
            .render("sA", Some("sB"), None)
            .contains("reindexed: [2-3] -> [3-4]"));
    }

    #[test]
    fn display_only_text_changes_are_seen() {
        let new = OLD.replace("hello", "goodbye");
        let report = diff_markdown(OLD, &new).unwrap();
        assert_eq!(report.changed.len(), 1);
        assert_eq!(report.changed[0].index, None);
    }

    #[test]
    fn multiline_action_rows_stay_one_row() {
        let md = "- [0] AXWindow\n  - [1] AXButton \"x\" [actions=[press,name:move\ntarget:0x0\nselector:(null)]]\n  - [2] AXButton \"y\"";
        assert_eq!(parse_rows(md).len(), 3);
        assert!(diff_markdown(md, md).unwrap().is_empty());
    }

    #[test]
    fn options_parse_and_defaults() {
        let o = opts(json!({}));
        assert_eq!(o.tree_format, TreeFormat::Markdown);
        assert!(!o.verbose && !o.full_output && o.since.is_none());
        assert_eq!(
            opts(json!({"full_output": true})).tree_format,
            TreeFormat::Both
        );
        assert_eq!(
            opts(json!({"tree_format": "elements"})).tree_format,
            TreeFormat::Elements
        );
        assert!(ViewOptions::from_args(&json!({"tree_format": "xml"})).is_err());
        assert!(ViewOptions::from_args(&json!({"since": 3})).is_err());
        // full_output is a full read, never a diff.
        assert!(opts(json!({"full_output": true, "since": "s00000001"}))
            .since
            .is_none());
    }

    #[test]
    fn max_elements_precedence() {
        let o = opts(json!({}));
        assert_eq!(
            o.max_elements(&json!({}), 2000, false),
            DEFAULT_MAX_ELEMENTS
        );
        assert_eq!(
            o.max_elements(&json!({"max_elements": 900}), 2000, false),
            900
        );
        assert_eq!(o.max_elements(&json!({}), 2000, true), 2000);
        let full = opts(json!({"full_output": true}));
        assert_eq!(full.max_elements(&json!({}), 5000, false), 5000);
    }

    fn payload(sid: &str, md: &str) -> (Vec<Content>, Value) {
        let content = vec![
            Content::image_png("AAAA".into()),
            Content::text(format!("window_id=7 pid=9 size=10x10 elements=4\n\n{md}")),
        ];
        let structured = json!({
            "window_id": 7, "pid": 9, "snapshot_id": sid,
            "tree_markdown": md,
            "elements": [{"element_index": 1}, {"element_index": 2}],
            "_note": "n", "background_input": {"routes": []},
            "element_count": 4
        });
        (content, structured)
    }

    fn ctx(window_id: u64) -> ViewContext<'static> {
        ViewContext {
            pid: 9,
            window_id,
            key: ViewKey {
                query: None,
                max_elements: 250,
                max_depth: None,
            },
            focus_probe: None,
        }
    }

    fn text_of(content: &[Content]) -> String {
        content
            .iter()
            .find_map(|c| match c {
                Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn default_read_is_markdown_only_without_noise() {
        let (mut content, mut s) = payload("s000000a1", OLD);
        apply(&opts(json!({})), &ctx(7001), &mut content, &mut s);
        assert!(s.get("elements").is_none());
        assert!(s.get("_note").is_none());
        assert!(s.get("background_input").is_none());
        assert_eq!(s["tree_markdown"], OLD);
        assert_eq!(s["tree_format"], "markdown");
        let text = text_of(&content);
        assert!(text.starts_with("window_id=7 pid=9 size=10x10 elements=4 snapshot_id=s000000a1\n"));
        assert!(text.contains("<snapshot_id>:N"));
        assert!(text.ends_with(OLD));
    }

    #[test]
    fn full_output_is_untouched() {
        let (mut content, mut s) = payload("s000000a2", OLD);
        let before = s.clone();
        let text_before = text_of(&content);
        apply(
            &opts(json!({"full_output": true})),
            &ctx(7002),
            &mut content,
            &mut s,
        );
        assert_eq!(s, before);
        assert_eq!(text_of(&content), text_before);
        assert_eq!(s["elements"], before["elements"]);
        assert_eq!(s["_note"], before["_note"]);
        assert_eq!(s["background_input"], before["background_input"]);
        assert_eq!(s["tree_markdown"], OLD);
    }

    #[test]
    fn elements_format_drops_markdown() {
        let (mut content, mut s) = payload("s000000a3", OLD);
        apply(
            &opts(json!({"tree_format": "elements"})),
            &ctx(7003),
            &mut content,
            &mut s,
        );
        assert!(s.get("tree_markdown").is_none());
        assert_eq!(s["elements"].as_array().unwrap().len(), 2);
        assert!(!text_of(&content).contains("AXButton"));
    }

    #[test]
    fn since_returns_diff_then_falls_back() {
        let window = 7004;
        let (mut c1, mut s1) = payload("s000000b1", OLD);
        apply(&opts(json!({})), &ctx(window), &mut c1, &mut s1);

        // Unchanged tree.
        let (mut c2, mut s2) = payload("s000000b2", OLD);
        apply(
            &opts(json!({"since": "s000000b1"})),
            &ctx(window),
            &mut c2,
            &mut s2,
        );
        assert_eq!(s2["since_status"], "no_change");
        assert!(s2.get("tree_markdown").is_none());
        assert!(text_of(&c2).contains("no change since s000000b1"));

        // One edit, chained against the previous snapshot.
        let edited = OLD.replace("\"a\"", "\"ab\"");
        let (mut c3, mut s3) = payload("s000000b3", &edited);
        apply(
            &opts(json!({"since": "s000000b2"})),
            &ctx(window),
            &mut c3,
            &mut s3,
        );
        assert_eq!(s3["since_status"], "diff");
        assert_eq!(s3["diff_counts"]["changed"], 1);
        assert!(text_of(&c3).contains("~ [1] AXTextField"));
        assert!(!text_of(&c3).contains("AXButton \"Save\""));
        assert!(s3.get("elements").is_none());

        // Unknown snapshot falls back to the full read.
        let (mut c4, mut s4) = payload("s000000b4", &edited);
        apply(
            &opts(json!({"since": "s0000dead"})),
            &ctx(window),
            &mut c4,
            &mut s4,
        );
        assert_eq!(s4["since_status"], "unknown_snapshot");
        assert_eq!(s4["tree_markdown"], edited);
        assert!(text_of(&c4).contains("unknown or expired snapshot_id; full read follows"));

        // Another window's snapshot is not a baseline.
        let (mut c5, mut s5) = payload("s000000b5", &edited);
        apply(
            &opts(json!({"since": "s000000b4"})),
            &ctx(window + 1),
            &mut c5,
            &mut s5,
        );
        assert_eq!(s5["since_status"], "other_window");
    }

    #[test]
    fn a_rewritten_tree_falls_back_to_full() {
        let window = 7005;
        let (mut c1, mut s1) = payload("s000000c1", OLD);
        apply(&opts(json!({})), &ctx(window), &mut c1, &mut s1);
        let other = "- [0] AXWindow \"Other\"\n  - [1] AXList \"x\"\n  - [2] AXButton \"Q\"\n  - [3] AXButton \"R\"\n  - [4] AXButton \"S\"";
        let (mut c2, mut s2) = payload("s000000c2", other);
        apply(
            &opts(json!({"since": "s000000c1"})),
            &ctx(window),
            &mut c2,
            &mut s2,
        );
        assert_eq!(s2["since_status"], "too_much_changed");
        assert_eq!(s2["tree_markdown"], other);
    }

    #[test]
    fn node_budget_truncation_is_stated() {
        let (mut content, mut s) = payload("s000000d1", OLD);
        s["truncated"] = json!(true);
        s["truncation_reason"] = json!("node_budget");
        s["nodes_pending"] = json!(40);
        apply(&opts(json!({})), &ctx(7006), &mut content, &mut s);
        let text = text_of(&content);
        assert!(text.contains("Tree truncated at max_elements=250 (40 more node(s) not shown)"));
        assert!(s["truncation_hint"]
            .as_str()
            .unwrap()
            .contains("larger max_elements"));
    }

    #[test]
    fn degraded_snapshots_keep_background_input() {
        let (mut content, mut s) = payload("s000000e1", OLD);
        s["degraded"] = json!(true);
        apply(&opts(json!({})), &ctx(7007), &mut content, &mut s);
        assert!(s.get("background_input").is_some());
        let (mut content, mut s) = payload("s000000e2", OLD);
        apply(
            &opts(json!({"verbose": true})),
            &ctx(7008),
            &mut content,
            &mut s,
        );
        assert!(s.get("background_input").is_some());
        assert!(s.get("_note").is_some());
    }
}
