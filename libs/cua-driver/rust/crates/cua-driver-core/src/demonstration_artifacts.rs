//! Deterministic, local artifacts for a human demonstration.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;
use serde_json::Value;

const MAX_INLINE_FRAMES: usize = 4;

#[derive(Debug, Clone, Copy, Default)]
pub struct CaptureEvidence {
    pub dropped_events: usize,
    pub screenshot_failures: usize,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub action_count: usize,
    pub duration_ms: u64,
    pub dropped_events: usize,
    pub screenshot_failures: usize,
    pub complete: bool,
    pub action_histogram: BTreeMap<String, usize>,
}

#[derive(Debug)]
pub struct Artifacts {
    pub trajectory_md: PathBuf,
    pub summary_json: PathBuf,
    pub summary: Summary,
}

#[derive(Debug)]
struct Turn {
    index: u32,
    dir_name: String,
    action: String,
    arguments: Value,
    t_ms: u64,
    has_screenshot: bool,
}

pub fn write(directory: &Path, evidence: CaptureEvidence) -> anyhow::Result<Artifacts> {
    let turns = read_turns(directory)?;
    let summary = summarize(&turns, evidence);
    let inline = pick_inline_frames(&turns);

    let trajectory_md = directory.join("TRAJECTORY.md");
    std::fs::write(
        &trajectory_md,
        render_markdown(directory, &turns, &summary, &inline),
    )?;

    let summary_json = directory.join("SUMMARY.json");
    std::fs::write(&summary_json, serde_json::to_vec_pretty(&summary)?)?;

    Ok(Artifacts {
        trajectory_md,
        summary_json,
        summary,
    })
}

fn read_turns(directory: &Path) -> anyhow::Result<Vec<Turn>> {
    let mut entries: Vec<_> = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.path().is_dir()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("turn-"))
        })
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    let mut turns = Vec::new();
    for entry in entries {
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let action_path = entry.path().join("action.json");
        let text = std::fs::read_to_string(&action_path)
            .with_context(|| format!("read {}", action_path.display()))?;
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("parse {}", action_path.display()))?;
        let index = dir_name
            .strip_prefix("turn-")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        turns.push(Turn {
            index,
            dir_name,
            action: value
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            arguments: value.get("arguments").cloned().unwrap_or(Value::Null),
            t_ms: value
                .get("t_ms_from_session_start")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            has_screenshot: entry.path().join("screenshot.png").exists(),
        });
    }
    Ok(turns)
}

fn summarize(turns: &[Turn], evidence: CaptureEvidence) -> Summary {
    let mut action_histogram = BTreeMap::new();
    for turn in turns {
        *action_histogram.entry(turn.action.clone()).or_insert(0) += 1;
    }
    Summary {
        action_count: turns.len(),
        duration_ms: turns.last().map_or(0, |turn| turn.t_ms),
        dropped_events: evidence.dropped_events,
        screenshot_failures: evidence.screenshot_failures,
        complete: evidence.dropped_events == 0 && evidence.screenshot_failures == 0,
        action_histogram,
    }
}

fn pick_inline_frames(turns: &[Turn]) -> BTreeSet<u32> {
    let frames: Vec<_> = turns.iter().filter(|turn| turn.has_screenshot).collect();
    let mut selected = BTreeSet::new();
    if frames.is_empty() {
        return selected;
    }
    selected.insert(frames[0].index);
    selected.insert(frames[frames.len() - 1].index);
    if frames.len() > 2 {
        let extra = MAX_INLINE_FRAMES.saturating_sub(2);
        let step = frames.len() as f64 / (extra + 1) as f64;
        for position in 1..=extra {
            let index = (position as f64 * step) as usize;
            if let Some(turn) = frames.get(index.min(frames.len() - 1)) {
                selected.insert(turn.index);
            }
        }
    }
    selected
}

fn describe(turn: &Turn) -> String {
    let number = |key: &str| turn.arguments.get(key).and_then(Value::as_f64);
    let text = |key: &str| turn.arguments.get(key).and_then(Value::as_str);
    match turn.action.as_str() {
        "human_click" => {
            let verb = match text("button") {
                Some("right") => "Right-click",
                Some("middle") => "Middle-click",
                _ => "Click",
            };
            point(verb, number("x"), number("y"))
        }
        "human_scroll" => format!(
            "Scroll (dx={}, dy={}) at ({}, {})",
            number("dx").unwrap_or(0.0),
            number("dy").unwrap_or(0.0),
            coordinate(number("x")),
            coordinate(number("y"))
        ),
        "human_drag" => format!(
            "Drag from ({}, {}) to ({}, {})",
            coordinate(number("start_x")),
            coordinate(number("start_y")),
            coordinate(number("end_x")),
            coordinate(number("end_y"))
        ),
        "human_key" => describe_key(&turn.arguments),
        "human_text" => "Enter text (redacted)".into(),
        action => format!("{action} {}", compact(&turn.arguments)),
    }
}

fn describe_key(arguments: &Value) -> String {
    let key = arguments.get("key").and_then(Value::as_str).unwrap_or("?");
    let modifiers = arguments.get("modifiers");
    let active = |name: &str| {
        modifiers
            .and_then(|value| value.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let mut chord = String::new();
    for (name, label) in [
        ("ctrl", "Ctrl+"),
        ("alt", "Alt+"),
        ("meta", "Meta+"),
        ("shift", "Shift+"),
    ] {
        if active(name) {
            chord.push_str(label);
        }
    }
    chord.push_str(key);
    format!("Press {chord}")
}

fn render_markdown(
    directory: &Path,
    turns: &[Turn],
    summary: &Summary,
    inline: &BTreeSet<u32>,
) -> String {
    let name = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "demonstration".into());
    let mut output = format!(
        "# Demonstration: {name}\n\n{} actions over {:.1}s.\n\n",
        summary.action_count,
        summary.duration_ms as f64 / 1000.0
    );
    output
        .push_str("> Screenshots are best-effort post-event captures linked by relative path.\n\n");
    if !summary.complete {
        output.push_str(&format!(
            "> **Incomplete capture:** {} events dropped; {} screenshots unavailable.\n\n",
            summary.dropped_events, summary.screenshot_failures
        ));
    }

    if turns.is_empty() {
        output.push_str("No input was captured.\n");
        return output;
    }

    output.push_str("## Action histogram\n\n");
    for (action, count) in &summary.action_histogram {
        output.push_str(&format!("- `{action}` × {count}\n"));
    }
    output.push_str("\n## Steps\n\n");

    for turn in turns {
        output.push_str(&format!(
            "### Step {} — {}\n\n- t = {:.2}s\n",
            turn.index,
            describe(turn),
            turn.t_ms as f64 / 1000.0
        ));
        if turn.has_screenshot {
            let relative = format!("{}/screenshot.png", turn.dir_name);
            if inline.contains(&turn.index) {
                output.push_str(&format!("\n![step {}]({relative})\n", turn.index));
            } else {
                output.push_str(&format!("- screenshot: [{relative}]({relative})\n"));
            }
        }
        output.push('\n');
    }
    output
}

fn point(verb: &str, x: Option<f64>, y: Option<f64>) -> String {
    format!("{verb} at ({}, {})", coordinate(x), coordinate(y))
}

fn coordinate(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.0}"))
        .unwrap_or_else(|| "?".into())
}

fn compact(value: &Value) -> String {
    if value.is_null() {
        String::new()
    } else {
        serde_json::to_string(value).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn directory(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cua-demonstration-artifacts-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn turn(directory: &Path, index: u32, value: Value, screenshot: bool) {
        let path = directory.join(format!("turn-{index:05}"));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("action.json"), value.to_string()).unwrap();
        if screenshot {
            std::fs::write(path.join("screenshot.png"), b"png").unwrap();
        }
    }

    #[test]
    fn writes_human_focused_artifacts() {
        let directory = directory("actions");
        turn(
            &directory,
            1,
            json!({
                "tool": "human_click",
                "arguments": {"x": 10, "y": 20, "button": "right"},
                "t_ms_from_session_start": 50
            }),
            true,
        );
        let artifacts = write(&directory, CaptureEvidence::default()).unwrap();
        let markdown = std::fs::read_to_string(&artifacts.trajectory_md).unwrap();
        assert!(markdown.contains("Right-click at (10, 20)"));
        assert!(markdown.contains("best-effort post-event"));
        assert_eq!(artifacts.summary.action_count, 1);
        assert!(artifacts.summary.complete);
    }

    #[test]
    fn empty_demonstration_is_valid() {
        let directory = directory("empty");
        let artifacts = write(&directory, CaptureEvidence::default()).unwrap();
        assert_eq!(artifacts.summary.action_count, 0);
        assert!(std::fs::read_to_string(artifacts.trajectory_md)
            .unwrap()
            .contains("No input was captured"));
    }

    #[test]
    fn embeds_at_most_four_frames() {
        let directory = directory("frames");
        for index in 1..=10 {
            turn(
                &directory,
                index,
                json!({
                    "tool": "human_click",
                    "arguments": {"x": index, "y": index},
                    "t_ms_from_session_start": index * 100
                }),
                true,
            );
        }
        let artifacts = write(&directory, CaptureEvidence::default()).unwrap();
        let markdown = std::fs::read_to_string(artifacts.trajectory_md).unwrap();
        assert!(markdown.matches("\n![step").count() <= MAX_INLINE_FRAMES);
    }

    #[test]
    fn reports_incomplete_capture() {
        let directory = directory("incomplete");
        let artifacts = write(
            &directory,
            CaptureEvidence {
                dropped_events: 2,
                screenshot_failures: 1,
            },
        )
        .unwrap();
        assert!(!artifacts.summary.complete);
        assert!(std::fs::read_to_string(artifacts.trajectory_md)
            .unwrap()
            .contains("Incomplete capture"));
    }
}
