//! Turn a recorded trajectory directory into a token-cheap, human-readable
//! `TRAJECTORY.md` plus a `SUMMARY.json`.
//!
//! Reading a raw `turn-NNNNN/action.json` tree (or, worse, a jsonl full of
//! base64 screenshots) blows up an agent's context. This processor renders the
//! trajectory as readable prose with screenshots referenced by **relative
//! path** — never inlined as base64 — and only embeds a handful of *key frames*
//! so a reader sees the 1–2 images that matter instead of all N. The full
//! per-turn screenshots stay on disk, linked but not embedded, for when a
//! reader deliberately wants one.
//!
//! This is the always-available output of `process_recording`; no network and
//! no model are involved. It is the substrate the `DEMONSTRATION.md` guide
//! (and the optional Anthropic-backed `author_skill`) read to write a SKILL.md.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// One parsed turn (agent action or human event), from `turn-NNNNN/action.json`.
#[derive(Debug, Clone)]
struct Turn {
    index: u32,
    dir_name: String,
    tool: String,
    arguments: Value,
    result_summary: Option<String>,
    t_ms: u64,
    /// "agent" (driver tool call) or "human" (captured demonstration input).
    source: String,
    has_screenshot: bool,
    has_click_png: bool,
}

#[derive(Debug, Clone)]
pub struct ProcessOptions {
    /// Maximum number of screenshots to embed inline as markdown images. The
    /// rest are referenced as plain relative links. Keeps the rendered doc
    /// cheap to read while preserving access to every frame.
    pub max_inline_frames: usize,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self { max_inline_frames: 4 }
    }
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub turn_count: usize,
    pub human_turns: usize,
    pub agent_turns: usize,
    pub duration_ms: u64,
    /// Tool name -> count, for a quick histogram of what the demo did.
    pub action_histogram: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug)]
pub struct ProcessResult {
    pub trajectory_md: PathBuf,
    pub summary_json: PathBuf,
    pub summary: Summary,
}

/// Process the recording directory `dir` in place, writing `TRAJECTORY.md` and
/// `SUMMARY.json` next to its `turn-*/` folders.
pub fn process(dir: &Path, opts: &ProcessOptions) -> anyhow::Result<ProcessResult> {
    let turns = read_turns(dir)?;
    if turns.is_empty() {
        anyhow::bail!("no turn-* folders found under {}", dir.display());
    }

    let summary = summarize(&turns);
    let inline = pick_inline_frames(&turns, opts.max_inline_frames);

    let md = render_markdown(dir, &turns, &summary, &inline);
    let trajectory_md = dir.join("TRAJECTORY.md");
    std::fs::write(&trajectory_md, md)?;

    let summary_json = dir.join("SUMMARY.json");
    std::fs::write(&summary_json, serde_json::to_vec_pretty(&summary)?)?;

    Ok(ProcessResult { trajectory_md, summary_json, summary })
}

fn read_turns(dir: &Path) -> anyhow::Result<Vec<Turn>> {
    let mut turns = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("turn-"))
                .unwrap_or(false)
        })
        .collect();
    // Lexical sort == chronological (5-digit zero-padded turn numbers).
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let action_path = entry.path().join("action.json");
        let Ok(text) = std::fs::read_to_string(&action_path) else {
            continue;
        };
        let Ok(v): Result<Value, _> = serde_json::from_str(&text) else {
            continue;
        };
        let index = dir_name
            .strip_prefix("turn-")
            .and_then(|s| s.trim_start_matches('0').parse().ok())
            .unwrap_or(0);
        turns.push(Turn {
            index,
            tool: v.get("tool").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
            arguments: v.get("arguments").cloned().unwrap_or(Value::Null),
            result_summary: v.get("result_summary").and_then(|x| x.as_str()).map(String::from),
            t_ms: v.get("t_ms_from_session_start").and_then(|x| x.as_u64()).unwrap_or(0),
            source: v.get("source").and_then(|x| x.as_str()).unwrap_or("agent").to_string(),
            has_screenshot: entry.path().join("screenshot.png").exists(),
            has_click_png: entry.path().join("click.png").exists(),
            dir_name,
        });
    }
    Ok(turns)
}

fn summarize(turns: &[Turn]) -> Summary {
    let mut hist = std::collections::BTreeMap::new();
    let mut human = 0usize;
    for t in turns {
        *hist.entry(t.tool.clone()).or_insert(0) += 1;
        if t.source == "human" {
            human += 1;
        }
    }
    let duration_ms = turns.last().map(|t| t.t_ms).unwrap_or(0);
    Summary {
        turn_count: turns.len(),
        human_turns: human,
        agent_turns: turns.len() - human,
        duration_ms,
        action_histogram: hist,
    }
}

/// Choose which turns get an inline image: always the first and last, then
/// evenly spaced across the middle up to the budget. Prefer turns that have a
/// click marker (those are the visually meaningful steps).
fn pick_inline_frames(turns: &[Turn], budget: usize) -> std::collections::BTreeSet<u32> {
    let mut set = std::collections::BTreeSet::new();
    if budget == 0 {
        return set;
    }
    let with_img: Vec<&Turn> = turns.iter().filter(|t| t.has_screenshot).collect();
    if with_img.is_empty() {
        return set;
    }
    set.insert(with_img[0].index);
    set.insert(with_img[with_img.len() - 1].index);
    // Evenly sample the remainder.
    if budget > 2 && with_img.len() > 2 {
        let extra = budget - 2;
        let step = with_img.len() as f64 / (extra + 1) as f64;
        for k in 1..=extra {
            let idx = (k as f64 * step) as usize;
            if let Some(t) = with_img.get(idx.min(with_img.len() - 1)) {
                set.insert(t.index);
            }
        }
    }
    set
}

/// Render a single turn's action into a human-readable sentence.
fn describe(turn: &Turn) -> String {
    let a = &turn.arguments;
    let num = |k: &str| a.get(k).and_then(|v| v.as_f64());
    let s = |k: &str| a.get(k).and_then(|v| v.as_str());
    match turn.tool.as_str() {
        "click" | "left_click" => point("Click", num("x"), num("y")),
        "right_click" => point("Right-click", num("x"), num("y")),
        "double_click" => point("Double-click", num("x"), num("y")),
        "scroll" => format!(
            "Scroll (dx={}, dy={}) at ({}, {})",
            num("dx").unwrap_or(0.0),
            num("dy").unwrap_or(0.0),
            fmt(num("x")),
            fmt(num("y"))
        ),
        "drag" => format!(
            "Drag from ({}, {}) to ({}, {})",
            fmt(num("start_x")),
            fmt(num("start_y")),
            fmt(num("end_x")),
            fmt(num("end_y"))
        ),
        "type_text" => {
            // Demonstration turns carry a redacted summary in `redacted`; agent
            // turns carry literal `text`. Prefer the privacy-safe field.
            let shown = s("redacted").or_else(|| s("text")).unwrap_or("");
            format!("Type {}", quote(shown))
        }
        "press_key" | "hotkey" => format!("Press {}", s("key").or_else(|| s("keys")).unwrap_or("?")),
        "set_value" => format!("Set value to {}", quote(s("value").unwrap_or(""))),
        other => format!("{other} {}", compact_args(a)),
    }
}

fn point(verb: &str, x: Option<f64>, y: Option<f64>) -> String {
    format!("{verb} at ({}, {})", fmt(x), fmt(y))
}
fn fmt(v: Option<f64>) -> String {
    v.map(|n| format!("{n:.0}")).unwrap_or_else(|| "?".into())
}
fn quote(s: &str) -> String {
    format!("\"{s}\"")
}
fn compact_args(a: &Value) -> String {
    if a.is_null() {
        String::new()
    } else {
        serde_json::to_string(a).unwrap_or_default()
    }
}

fn render_markdown(
    dir: &Path,
    turns: &[Turn],
    summary: &Summary,
    inline: &std::collections::BTreeSet<u32>,
) -> String {
    let mut out = String::new();
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".into());
    out.push_str(&format!("# Trajectory: {name}\n\n"));
    out.push_str(&format!(
        "{} steps ({} human, {} agent) over {:.1}s.\n\n",
        summary.turn_count,
        summary.human_turns,
        summary.agent_turns,
        summary.duration_ms as f64 / 1000.0
    ));
    out.push_str("> Screenshots are linked by relative path; only key frames are embedded inline. \
                  Open a `turn-*/screenshot.png` link directly if you need to see a specific step.\n\n");

    out.push_str("## Action histogram\n\n");
    for (tool, n) in &summary.action_histogram {
        out.push_str(&format!("- `{tool}` × {n}\n"));
    }
    out.push('\n');

    out.push_str("## Steps\n\n");
    for t in turns {
        let tag = if t.source == "human" { " _(human)_" } else { "" };
        out.push_str(&format!(
            "### Step {} — {}{tag}\n\n",
            t.index,
            describe(t)
        ));
        out.push_str(&format!("- t = {:.2}s\n", t.t_ms as f64 / 1000.0));
        if let Some(rs) = &t.result_summary {
            if !rs.is_empty() {
                out.push_str(&format!("- result: {}\n", truncate(rs, 160)));
            }
        }
        // Image: embed key frames; link the rest.
        let img = if t.has_click_png { "click.png" } else { "screenshot.png" };
        if t.has_screenshot || t.has_click_png {
            let rel = format!("{}/{}", t.dir_name, img);
            if inline.contains(&t.index) {
                out.push_str(&format!("\n![step {}]({rel})\n", t.index));
            } else {
                out.push_str(&format!("- screenshot: [{rel}]({rel})\n"));
            }
        }
        out.push('\n');
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_turn(dir: &Path, n: u32, body: Value, screenshot: bool) {
        let td = dir.join(format!("turn-{n:05}"));
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("action.json"), body.to_string()).unwrap();
        if screenshot {
            std::fs::write(td.join("screenshot.png"), b"\x89PNG").unwrap();
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("cua-md-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn renders_readable_markdown_with_relative_links() {
        let dir = tmp("renders");
        write_turn(
            &dir,
            1,
            json!({"tool":"click","arguments":{"x":10,"y":20},"t_ms_from_session_start":0,"source":"human"}),
            true,
        );
        write_turn(
            &dir,
            2,
            json!({"tool":"type_text","arguments":{"redacted":"•••• (4 chars, alpha)"},"t_ms_from_session_start":500,"source":"human"}),
            true,
        );
        let res = process(&dir, &ProcessOptions::default()).unwrap();
        let md = std::fs::read_to_string(&res.trajectory_md).unwrap();
        assert!(md.contains("Click at (10, 20)"));
        // redacted text is shown, raw never present
        assert!(md.contains("4 chars"));
        assert!(md.contains("_(human)_"));
        // relative path link, not base64
        assert!(md.contains("turn-00001/screenshot.png"));
        // no base64/binary inlined — the doc stays text
        assert!(!md.contains("PNG"));
        assert_eq!(res.summary.human_turns, 2);
        assert_eq!(res.summary.turn_count, 2);
    }

    #[test]
    fn caps_inline_frames() {
        let dir = tmp("caps");
        for n in 1..=10 {
            write_turn(
                &dir,
                n,
                json!({"tool":"click","arguments":{"x":n,"y":n},"t_ms_from_session_start": n as u64 *100}),
                true,
            );
        }
        let res = process(&dir, &ProcessOptions { max_inline_frames: 3 }).unwrap();
        let md = std::fs::read_to_string(&res.trajectory_md).unwrap();
        // Embedded images use the "![" syntax; links use "- screenshot:".
        let embedded = md.matches("\n![step").count();
        assert!(embedded <= 3 && embedded >= 2, "embedded={embedded}");
        // The rest are present as plain links.
        assert!(md.contains("- screenshot: ["));
    }

    #[test]
    fn empty_dir_errors() {
        let dir = tmp("empty");
        assert!(process(&dir, &ProcessOptions::default()).is_err());
    }
}
