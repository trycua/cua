//! Trajectory recording session.
//!
//! When enabled, every non-read-only, non-recording tool call writes a
//! `turn-NNNNN/action.json` file to the configured output directory.
//! When a screenshot callback is registered via `set_screenshot_fn`, it also
//! writes `screenshot.png` (extracted from `pid`/`window_id` in the args).
//!
//! Schema mirrors the Swift/Windows reference `action.json`:
//!   { tool, arguments, result_summary, timestamp, t_ms_from_session_start,
//!     t_start_ms_from_session_start }

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

// ── Platform screenshot callback ─────────────────────────────────────────────
//
// Registered once at startup by each platform crate. Takes (window_id, pid)
// and returns raw PNG bytes, or None if capture fails. The callback is called
// synchronously from write_turn (a blocking context).

type ScreenshotFnBox = Box<dyn Fn(Option<u64>, Option<i64>) -> Option<Vec<u8>> + Send + Sync>;
static SCREENSHOT_FN: OnceLock<ScreenshotFnBox> = OnceLock::new();

/// Register the platform-specific screenshot callback. Call once at startup
/// before any tool invocations. Subsequent calls are silently ignored.
pub fn set_screenshot_fn(f: impl Fn(Option<u64>, Option<i64>) -> Option<Vec<u8>> + Send + Sync + 'static) {
    let _ = SCREENSHOT_FN.set(Box::new(f));
}

// ── Platform click-marker callback ───────────────────────────────────────────
//
// Takes (png_bytes, cx, cy) and returns modified PNG bytes with a red crosshair
// at (cx, cy), or None if drawing fails. Used to produce click.png alongside
// screenshot.png when a click-family tool is recorded.

type ClickMarkerFnBox = Box<dyn Fn(&[u8], f64, f64) -> Option<Vec<u8>> + Send + Sync>;
static CLICK_MARKER_FN: OnceLock<ClickMarkerFnBox> = OnceLock::new();

/// Register the platform-specific click-marker callback. Call once at startup.
pub fn set_click_marker_fn(f: impl Fn(&[u8], f64, f64) -> Option<Vec<u8>> + Send + Sync + 'static) {
    let _ = CLICK_MARKER_FN.set(Box::new(f));
}

/// Persistent recording session state (singleton per process).
pub struct RecordingSession {
    inner: Mutex<RecordingInner>,
}

struct RecordingInner {
    enabled: bool,
    output_dir: Option<PathBuf>,
    next_turn: u32,
    session_start_ms: u64,
    last_error: Option<String>,
}

/// Snapshot of the current recording state (cheap to clone).
#[derive(Debug, Clone)]
pub struct RecordingState {
    pub enabled: bool,
    pub output_dir: Option<String>,
    pub next_turn: u32,
    pub last_error: Option<String>,
}

impl RecordingSession {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RecordingInner {
                enabled: false,
                output_dir: None,
                next_turn: 1,
                session_start_ms: 0,
                last_error: None,
            }),
        }
    }

    /// Enable or disable recording. `output_dir` is required when `enabled=true`.
    pub fn configure(&self, enabled: bool, output_dir: Option<&str>) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if !enabled {
            inner.enabled = false;
            inner.output_dir = None;
            inner.next_turn = 1;
            inner.session_start_ms = 0;
            inner.last_error = None;
            return Ok(());
        }

        let dir_str = output_dir
            .ok_or_else(|| anyhow::anyhow!("output_dir is required when enabling recording"))?;
        let dir = expand_tilde(dir_str);
        std::fs::create_dir_all(&dir)?;

        // Write initial session.json.
        let session_payload = serde_json::json!({
            "schema_version": 1,
            "started_at_monotonic_ms": now_ms(),
            "video": { "present": false },
            "cursor": { "present": false }
        });
        let _ = write_json_atomic(
            &dir.join("session.json"),
            &session_payload,
        );

        inner.enabled = true;
        inner.output_dir = Some(dir);
        inner.next_turn = 1;
        inner.session_start_ms = now_ms();
        inner.last_error = None;
        Ok(())
    }

    /// Return a snapshot of the current state (non-blocking).
    pub fn current_state(&self) -> RecordingState {
        let inner = self.inner.lock().unwrap();
        RecordingState {
            enabled: inner.enabled,
            output_dir: inner.output_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
            next_turn: inner.next_turn,
            last_error: inner.last_error.clone(),
        }
    }

    /// Record a completed tool call. No-op when recording is disabled.
    /// `start_ms` — wall-clock ms at invocation start (use `now_ms()` before calling the tool).
    pub fn record(
        &self,
        tool_name: &str,
        args: &Value,
        result_text: &str,
        start_ms: u64,
    ) {
        let (turn_dir, session_start_ms) = {
            let mut inner = self.inner.lock().unwrap();
            if !inner.enabled {
                return;
            }
            let out = match inner.output_dir.clone() {
                Some(o) => o,
                None => return,
            };
            let idx = inner.next_turn;
            inner.next_turn += 1;
            (out.join(format!("turn-{idx:05}")), inner.session_start_ms)
        };

        if let Err(e) = write_turn(
            &turn_dir,
            tool_name,
            args,
            result_text,
            start_ms,
            session_start_ms,
        ) {
            let mut inner = self.inner.lock().unwrap();
            inner.last_error = Some(e.to_string());
        }
    }
}

impl Default for RecordingSession {
    fn default() -> Self { Self::new() }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn write_turn(
    turn_dir: &Path,
    tool_name: &str,
    args: &Value,
    result_text: &str,
    start_ms: u64,
    session_start_ms: u64,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(turn_dir)?;
    let now = now_ms();

    // Extract window_id and pid from args for screenshot capture.
    let window_id = args.get("window_id").and_then(|v| v.as_u64());
    let pid       = args.get("pid").and_then(|v| v.as_i64());

    // Extract click point for click-family tools.
    let click_point: Option<(f64, f64)> = if matches!(
        tool_name, "click" | "double_click" | "right_click"
    ) {
        match (args.get("x").and_then(|v| v.as_f64()), args.get("y").and_then(|v| v.as_f64())) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        }
    } else {
        None
    };

    let mut payload = serde_json::json!({
        "tool": tool_name,
        "arguments": args,
        "result_summary": result_text,
        "timestamp": iso_now(),
        "t_ms_from_session_start": now.saturating_sub(session_start_ms),
        "t_start_ms_from_session_start": start_ms.saturating_sub(session_start_ms),
    });
    if let Some((cx, cy)) = click_point {
        payload["click_point"] = serde_json::json!({"x": cx, "y": cy});
    }
    write_json_atomic(&turn_dir.join("action.json"), &payload)?;

    // Capture screenshot if a callback is registered.
    if let Some(screenshot_fn) = SCREENSHOT_FN.get() {
        if let Some(png_bytes) = screenshot_fn(window_id, pid) {
            let _ = std::fs::write(turn_dir.join("screenshot.png"), &png_bytes);
            // Write click.png (screenshot + red crosshair) for click-family tools.
            if let Some((cx, cy)) = click_point {
                if let Some(marker_fn) = CLICK_MARKER_FN.get() {
                    if let Some(click_png) = marker_fn(&png_bytes, cx, cy) {
                        let _ = std::fs::write(turn_dir.join("click.png"), &click_png);
                    }
                }
            }
        }
    }

    Ok(())
}

fn write_json_atomic(path: &Path, value: &Value) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Current wall-clock time as milliseconds since Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn iso_now() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Format as fractional Unix seconds (simple, unambiguous, machine-readable).
    format!("{:.3}", d.as_secs_f64())
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}
