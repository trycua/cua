//! Read a `start_recording` output directory into the inputs the
//! renderer needs: `SessionMetadata` block, sorted `[ClickEvent]`,
//! sorted `[CursorSample]`, and sorted `[ActionSpan]`. Pure file I/O
//! and JSON parsing — cross-platform.
//!
//! Reference: `libs/cua-driver/swift/Sources/CuaDriverCore/Recording/Render/TrajectoryLoader.swift`
//!
//! Failure philosophy: missing optional files (cursor.jsonl, no
//! click-class turns) degrade to empty vectors; the renderer copes
//! with zero clicks (output = raw input re-encoded, no zoom). Missing
//! structurally-required files (session.json, recording.mp4) return
//! a hard `LoadError` since a render with no video has nothing to do.

use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::recording_zoom::{ActionSpan, ClickEvent, CursorSample, WindowBounds};

// ── public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub video_width: u32,
    pub video_height: u32,
    pub cursor_sample_count: usize,
    /// Display backing-scale factor (1.0 on non-Retina, 2.0 on Retina).
    /// Multiplies cursor-space points → video pixels.
    pub display_scale_factor: f64,
    /// Source mp4 path (absolute), as recorded by the recording session.
    pub video_path: PathBuf,
}

#[derive(Debug)]
pub struct LoadedTrajectory {
    pub metadata: SessionMetadata,
    pub clicks: Vec<ClickEvent>,
    pub cursor_samples: Vec<CursorSample>,
    /// All action turns (click/type-class) projected as spans, for
    /// variable-speed playback. Empty when the trajectory has no
    /// addressable timing data.
    pub action_spans: Vec<ActionSpan>,
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("session.json not found at {0}")]
    SessionMissing(PathBuf),
    #[error("session.json at {0} is malformed: {1}")]
    SessionMalformed(PathBuf, String),
    #[error("recording.mp4 not found at {0}")]
    VideoMissing(PathBuf),
}

// ── entry point ──────────────────────────────────────────────────────────────

pub fn load(dir: &Path) -> Result<LoadedTrajectory, LoadError> {
    let session_path = dir.join("session.json");
    let cursor_path = dir.join("cursor.jsonl");
    let video_path = dir.join("recording.mp4");

    let metadata = load_session_metadata(&session_path, &video_path)?;
    if !video_path.exists() {
        return Err(LoadError::VideoMissing(video_path));
    }

    let cursor_samples = load_cursor_samples(&cursor_path);
    let clicks = load_clicks(dir);
    let action_spans = load_action_spans(dir);

    Ok(LoadedTrajectory { metadata, clicks, cursor_samples, action_spans })
}

// ── session.json ─────────────────────────────────────────────────────────────

fn load_session_metadata(
    session_path: &Path,
    video_path: &Path,
) -> Result<SessionMetadata, LoadError> {
    if !session_path.exists() {
        return Err(LoadError::SessionMissing(session_path.to_owned()));
    }
    let text = std::fs::read_to_string(session_path)
        .map_err(|e| LoadError::SessionMalformed(session_path.to_owned(), e.to_string()))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| LoadError::SessionMalformed(session_path.to_owned(), e.to_string()))?;

    // Width/height may not yet exist in session.json (the recorder we
    // just shipped doesn't write them — that's a follow-up). Probe the
    // recording.mp4 directly with ffprobe to learn the dimensions.
    let (mut video_width, mut video_height) = (0u32, 0u32);
    if let Some(vid) = v.get("video") {
        video_width = vid.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        video_height = vid.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    }
    if video_width == 0 || video_height == 0 {
        if let Some((w, h)) = probe_video_dimensions(video_path) {
            video_width = w;
            video_height = h;
        }
    }

    let cursor_sample_count = v.get("cursor")
        .and_then(|c| c.get("sample_count"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as usize;

    let display_scale_factor = v.get("display_scale_factor")
        .and_then(|x| x.as_f64())
        .unwrap_or(1.0);

    Ok(SessionMetadata {
        video_width,
        video_height,
        cursor_sample_count,
        display_scale_factor,
        video_path: video_path.to_owned(),
    })
}

/// Shell to `ffprobe` to learn the recorded mp4's pixel dimensions.
/// Returns `None` if ffprobe isn't available or the output isn't
/// parseable — the renderer falls back to 1920×1080 only as an
/// absolute last resort.
fn probe_video_dimensions(video_path: &Path) -> Option<(u32, u32)> {
    use std::process::Command;
    let ffprobe = crate::video_ffmpeg::find_ffprobe()?;
    let out = Command::new(ffprobe)
        .args(["-v", "error",
               "-select_streams", "v:0",
               "-show_entries", "stream=width,height",
               "-of", "csv=p=0:s=x"])
        .arg(video_path)
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout);
    let trimmed = s.trim();
    let mut parts = trimmed.split('x');
    let w: u32 = parts.next()?.trim().parse().ok()?;
    let h: u32 = parts.next()?.trim().parse().ok()?;
    Some((w, h))
}

// ── cursor.jsonl ─────────────────────────────────────────────────────────────

fn load_cursor_samples(path: &Path) -> Vec<CursorSample> {
    if !path.exists() { return Vec::new(); }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<CursorSample> = Vec::with_capacity(text.len() / 48);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let v: Value = match serde_json::from_str(line) { Ok(v) => v, Err(_) => continue };
        let t_ms = double_value(v.get("t_ms")).unwrap_or(0.0);
        let x = match double_value(v.get("x")) { Some(x) => x, None => continue };
        let y = match double_value(v.get("y")) { Some(y) => y, None => continue };
        out.push(CursorSample { t_ms, x, y });
    }
    out.sort_by(|a, b| a.t_ms.partial_cmp(&b.t_ms).unwrap_or(std::cmp::Ordering::Equal));
    out
}

// ── turn-*/action.json → clicks ──────────────────────────────────────────────

fn load_clicks(dir: &Path) -> Vec<ClickEvent> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) { Ok(e) => e, Err(_) => return out };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if !name_s.starts_with("turn-") { continue; }
        let path = entry.path();
        if !path.is_dir() { continue; }
        let action_path = path.join("action.json");
        if let Some(c) = parse_click_turn(&action_path) {
            out.push(c);
        }
    }
    out.sort_by(|a, b| a.t_ms.partial_cmp(&b.t_ms).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn parse_click_turn(action_path: &Path) -> Option<ClickEvent> {
    let text = std::fs::read_to_string(action_path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let tool = v.get("tool")?.as_str()?;
    if !is_click_class_tool(tool) { return None; }

    // Coordinate recovery, in priority order:
    //   1. `arguments.{x,y}` — pixel-addressed clicks pass coords directly.
    //   2. `result_summary` text scan — element-indexed clicks whose tool
    //      result surfaces screen coords like "Performed UIA Invoke on [28]
    //      (screen (745,679))".
    //
    // `click_point` is deliberately NOT a zoom source: the element-bounds
    // hook records it in window-local screenshot pixels (the click.png
    // marker space), while zoompan x/y are full-video-frame coordinates —
    // treating one as the other zooms onto the wrong region for any
    // non-maximized window. Translating would require per-turn window
    // bounds, which the recorder doesn't write yet.
    let (x, y) = if let Some(args) = v.get("arguments") {
        let ax = double_value(args.get("x"));
        let ay = double_value(args.get("y"));
        if let (Some(x), Some(y)) = (ax, ay) {
            (x, y)
        } else if let Some(screen) = parse_screen_from_summary(
            v.get("result_summary").and_then(|s| s.as_str()).unwrap_or("")
        ) {
            screen
        } else {
            return None;
        }
    } else {
        return None;
    };

    let t_ms = double_value(v.get("t_ms_from_session_start"))?;
    Some(ClickEvent { t_ms, x, y })
}

/// Pull "(screen (X,Y))" out of a tool-result text. Returns `(x, y)`
/// when the pattern is present and parses as numbers. Used as last-
/// resort coord recovery for element-indexed clicks whose action.json
/// doesn't yet carry click_point.
fn parse_screen_from_summary(summary: &str) -> Option<(f64, f64)> {
    // Looking for: "...(screen (123,456))..." — find the inner parens
    // right after "screen ".
    let idx = summary.find("(screen (")?;
    let rest = &summary[idx + "(screen (".len()..];
    let close = rest.find("))")?;
    let pair = &rest[..close];
    let mut parts = pair.splitn(2, ',');
    let x: f64 = parts.next()?.trim().parse().ok()?;
    let y: f64 = parts.next()?.trim().parse().ok()?;
    Some((x, y))
}

fn is_click_class_tool(name: &str) -> bool {
    matches!(name, "click" | "double_click" | "right_click")
}

fn is_click_or_type_class_tool(name: &str) -> bool {
    matches!(name, "click" | "double_click" | "right_click" | "type_text" | "type_text_chars")
}

// ── turn-*/action.json → action spans ────────────────────────────────────────

fn load_action_spans(dir: &Path) -> Vec<ActionSpan> {
    let mut spans = Vec::new();
    let entries = match std::fs::read_dir(dir) { Ok(e) => e, Err(_) => return spans };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if !name_s.starts_with("turn-") { continue; }
        let path = entry.path();
        if !path.is_dir() { continue; }
        if let Some(s) = parse_action_span(&path.join("action.json")) {
            spans.push(s);
        }
    }
    spans.sort_by(|a, b| a.start_ms.partial_cmp(&b.start_ms).unwrap_or(std::cmp::Ordering::Equal));
    spans
}

fn parse_action_span(action_path: &Path) -> Option<ActionSpan> {
    let text = std::fs::read_to_string(action_path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let tool = v.get("tool")?.as_str()?;
    if !is_click_or_type_class_tool(tool) { return None; }

    let end_ms = double_value(v.get("t_ms_from_session_start"))?;
    let start_ms = double_value(v.get("t_start_ms_from_session_start")).unwrap_or(end_ms);

    let window_bounds = v.get("window_bounds").and_then(|wb| {
        let x = double_value(wb.get("x"))?;
        let y = double_value(wb.get("y"))?;
        let width = double_value(wb.get("width"))?;
        let height = double_value(wb.get("height"))?;
        if width > 0.0 && height > 0.0 {
            Some(WindowBounds { x, y, width, height })
        } else { None }
    });

    let click_point = v.get("click_point").and_then(|cp| {
        let x = double_value(cp.get("x"))?;
        let y = double_value(cp.get("y"))?;
        Some((x, y))
    });

    Some(ActionSpan {
        start_ms,
        end_ms,
        window_bounds,
        click_point,
        focus_waypoints: None,
    })
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn double_value(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    if let Some(d) = v.as_f64() { return Some(d); }
    if let Some(i) = v.as_i64() { return Some(i as f64); }
    if let Some(u) = v.as_u64() { return Some(u as f64); }
    if let Some(s) = v.as_str() { return s.parse().ok(); }
    None
}
