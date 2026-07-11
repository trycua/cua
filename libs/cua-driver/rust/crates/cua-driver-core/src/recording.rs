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

use std::time::Instant;

use serde_json::Value;

use crate::cursor_sampler::CursorSampler;
use crate::video::{self, VideoBackend, VideoMetadata};

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

/// Invoke the registered screenshot callback. Returns `None` when no
/// callback was registered or when the platform capture failed. Used
/// by the PiP push hook (and by anything else that wants to share the
/// per-turn screenshot pipeline without duplicating the platform glue).
pub fn screenshot_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    SCREENSHOT_FN.get().and_then(|f| f(window_id, pid))
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

// ── Platform AX-snapshot callback ────────────────────────────────────────────
//
// Takes (window_id, pid) and returns JSON bytes for `app_state.json` (the
// post-action AX/UIA snapshot), or None if no snapshot is available on this
// platform.

type AxSnapshotFnBox = Box<dyn Fn(Option<u64>, Option<i64>) -> Option<Vec<u8>> + Send + Sync>;
static AX_SNAPSHOT_FN: OnceLock<AxSnapshotFnBox> = OnceLock::new();

/// Register the platform-specific AX/UIA snapshot callback. Call once at startup.
pub fn set_ax_snapshot_fn(f: impl Fn(Option<u64>, Option<i64>) -> Option<Vec<u8>> + Send + Sync + 'static) {
    let _ = AX_SNAPSHOT_FN.set(Box::new(f));
}

// ── Platform element-bounds callback ─────────────────────────────────────────
//
// Resolves an element_index to its center point in window-local screenshot
// pixels (the same coordinate space as the existing `(cx, cy)` arg to
// `CLICK_MARKER_FN`). Used so click.png is also written on element-indexed
// clicks, not just pixel-addressed ones.

type ElementBoundsFnBox = Box<dyn Fn(u64, i64, u32) -> Option<(f64, f64)> + Send + Sync>;
static ELEMENT_BOUNDS_FN: OnceLock<ElementBoundsFnBox> = OnceLock::new();

/// Register the platform-specific element-bounds resolver. Args: (window_id, pid, element_index).
pub fn set_element_bounds_fn(f: impl Fn(u64, i64, u32) -> Option<(f64, f64)> + Send + Sync + 'static) {
    let _ = ELEMENT_BOUNDS_FN.set(Box::new(f));
}

/// Persistent recording session state (singleton per process).
pub struct RecordingSession {
    inner: Mutex<RecordingInner>,
}

struct RecordingInner {
    enabled: bool,
    /// Session that owns the live recording, stamped on every successful
    /// `start()` from the daemon-injected `_session_id`. The daemon-global
    /// recorder is a singleton, so when session A starts a recording and
    /// session B later starts another (clobbering A's), A's disconnect must
    /// NOT stop B's recording. The proxy-exit `session_end` hook passes its
    /// own session id to `stop_owner()`, which no-ops when the live owner has
    /// moved on. `None` means the recording was started anonymously (CLI
    /// one-shot / legacy `configure()` shim) and is owned by nobody — only an
    /// unconditional `stop_owner(None)` can tear it down. Supersedes the
    /// #1775 generation token: a session id is a stable owner identity rather
    /// than a monotonic counter, and it doubles as the config-override key.
    owner: Option<String>,
    output_dir: Option<PathBuf>,
    next_turn: u32,
    session_start_ms: u64,
    /// Monotonic clock anchor for the cursor sampler so its `t_ms`
    /// matches the action-timeline anchor in `action.json`.
    session_monotonic_start: Option<Instant>,
    last_error: Option<String>,
    /// Live video backend when capture is active. Recreated per
    /// session. The concrete type is platform-determined (SCKit on
    /// macOS, ffmpeg subprocess elsewhere).
    video: Option<Box<dyn VideoBackend>>,
    /// Recorded after `stop()` until the next start — exposed in
    /// `current_state()` so callers can read the finalized video info
    /// after stopping.
    last_video: Option<VideoMetadata>,
    /// Cursor sampler thread. Runs alongside video so the renderer has
    /// per-frame cursor positions for smooth pan-between-clicks
    /// behavior. Stopped on `stop()` along with video.
    cursor: Option<CursorSampler>,
    /// Sample count from the last finalized cursor sampler; exposed in
    /// `session.json` after stop so the renderer can confirm the
    /// sampler ran.
    last_cursor_samples: usize,
}

/// Snapshot of the current recording state (cheap to clone).
#[derive(Debug, Clone)]
pub struct RecordingState {
    pub enabled: bool,
    pub output_dir: Option<String>,
    pub next_turn: u32,
    pub last_error: Option<String>,
    /// Whether a video subprocess is currently running.
    pub video_active: bool,
    /// Path to the most recently finalized video file, if any. Populated
    /// after a stop; cleared on next start.
    pub last_video_path: Option<String>,
    /// Session id that owns the current (or most recent) recording, stamped on
    /// `start()` from the daemon-injected `_session_id`. `None` for an
    /// anonymously-started recording (CLI one-shot / legacy shim). Surfaced so
    /// callers can see who owns the live recording; the proxy-exit teardown
    /// drives ownership via `session_end` (it already knows its own id) rather
    /// than reading this back.
    pub owner: Option<String>,
}

impl RecordingSession {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RecordingInner {
                enabled: false,
                owner: None,
                output_dir: None,
                next_turn: 1,
                session_start_ms: 0,
                session_monotonic_start: None,
                last_error: None,
                video: None,
                last_video: None,
                cursor: None,
                last_cursor_samples: 0,
            }),
        }
    }

    /// Enable recording at `output_dir`, optionally with video capture.
    /// Counterpart to `stop()`. Returns the resulting state.
    ///
    /// `record_video=true` spawns ffmpeg writing `<output_dir>/recording.mp4`
    /// for the lifetime of the session. NOTE: the MCP `start_recording` tool
    /// now defaults `record_video` to *false* (opt-in) — see
    /// `recording_tools.rs` — so video only records when explicitly requested.
    /// The legacy CLI `recording start` path via `configure()` still forces
    /// video on. If ffmpeg isn't on PATH the start still succeeds —
    /// the per-turn capture (action.json + screenshot.png) is independent
    /// of video — but the structured state carries the ffmpeg error so
    /// the caller can surface it.
    ///
    /// `owner` stamps the session that owns this recording (the daemon-injected
    /// `_session_id`). `None` marks an anonymous start (CLI one-shot / legacy
    /// `configure()` shim) owned by nobody. See `stop_owner()` for how this
    /// gates teardown.
    pub fn start(&self, output_dir: &str, record_video: bool, owner: Option<&str>) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        // Write-boundary resurrection guard — checked INSIDE the lock so the
        // is_session_ended test is atomic with the enabled/owner write below.
        // An in-flight start_recording that lands after its owning session ended
        // (passed the dispatch gate, then the proxy died) must not create a
        // recording owned by a dead session — a leaked ffmpeg/SCStream. The
        // teardown sites call `fire_session_end` (which marks ENDED_SESSIONS)
        // BEFORE `stop_owner`, so either the mark is already set and we bail
        // here, or we win the lock first and the reaper's later stop_owner(owner)
        // reaps what we started. Anonymous starts (owner = None: CLI one-shot /
        // legacy shim) are never gated.
        if let Some(o) = owner {
            if crate::session::is_session_ended(o) {
                anyhow::bail!(
                    "session {o} has ended; refusing to start a recording owned by a dead session"
                );
            }
        }
        // If a previous session is still open, gracefully tear it down
        // first so the caller doesn't accidentally leak an ffmpeg process.
        if let Some(rec) = inner.video.take() {
            let _ = rec.stop();
        }
        if let Some(cur) = inner.cursor.take() {
            let _ = cur.stop();
        }

        let dir = expand_tilde(output_dir);
        std::fs::create_dir_all(&dir)?;

        // Single monotonic anchor shared by video, cursor sampler, and
        // per-turn `t_ms_from_session_start` math in `record()` — so all
        // three timelines line up at the millisecond.
        let monotonic_start = Instant::now();

        let mut video_present = false;
        let mut video_error: Option<String> = None;
        if record_video {
            let path = dir.join("recording.mp4");
            match video::start_video(&path) {
                Ok(rec) => {
                    inner.video = Some(rec);
                    video_present = true;
                }
                Err(e) => {
                    video_error = Some(e.to_string());
                    tracing::warn!(target: "recording",
                        "Video capture failed to start; per-turn recording will \
                         continue without video: {e}");
                }
            }
        }

        // Cursor sampler always runs alongside video. Cheap (30 Hz
        // GetCursorPos / CGEventGetLocation poll) and the renderer
        // wants the data for smooth pan-between-clicks. When video is
        // off (record_video=false), the sampler is still useful for
        // post-hoc analysis, so we run it anyway — the cost is one
        // background thread + a small jsonl file.
        let cursor_path = dir.join("cursor.jsonl");
        match CursorSampler::start(cursor_path, monotonic_start) {
            Ok(s) => { inner.cursor = Some(s); }
            Err(e) => {
                tracing::warn!(target: "recording",
                    "Cursor sampler failed to start: {e}");
            }
        }

        // Write initial session.json — final video metadata is rewritten on
        // stop. We mark `present` based on whether ffmpeg actually started,
        // not just whether the caller asked for video.
        let session_payload = serde_json::json!({
            "schema_version": 1,
            "started_at_monotonic_ms": now_ms(),
            "video": video_session_payload(video_present, video_error.as_deref(), None),
            "cursor": { "present": inner.cursor.is_some(), "sample_count": 0 }
        });
        let _ = write_json_atomic(
            &dir.join("session.json"),
            &session_payload,
        );

        // Stamp the owning session on every successful start (reached only on
        // the success path — start() returns early via `?` on `create_dir_all`
        // failure above). `owner` clobbers any previous owner, which is correct:
        // the daemon-global recorder is a singleton, so the latest start() owns
        // it. The previous owner's disconnect then no-ops in stop_owner().
        inner.owner = owner.map(str::to_owned);
        inner.enabled = true;
        inner.output_dir = Some(dir);
        inner.next_turn = 1;
        inner.session_start_ms = now_ms();
        inner.session_monotonic_start = Some(monotonic_start);
        inner.last_error = video_error;
        inner.last_video = None;
        inner.last_cursor_samples = 0;
        Ok(())
    }

    /// Disable recording. Idempotent — calling stop on an already-stopped
    /// session is a no-op. If a video subprocess is running, it's
    /// gracefully terminated and the finalized metadata is folded into
    /// `session.json`.
    ///
    /// `requester` is the ownership guard for session-driven teardown
    /// (`session_end` / proxy-exit). Semantics:
    ///   - `None` — unconditional stop. Manual `stop_recording`, the legacy
    ///     `configure()` shim, the CLI one-shot path, and the idle-TTL backstop
    ///     all pass `None` to preserve today's manual-stop behavior.
    ///   - `Some(sid)` where `sid` owns the live recording — stop + clear owner.
    ///   - `Some(sid)` where `sid` does NOT own it (a disconnecting session
    ///     whose recording was already clobbered by a newer `start()`, or which
    ///     never started a recording) — silent no-op, leaving the current
    ///     owner's recording running.
    /// The guard lives inside the lock so it is race-free against a concurrent
    /// `start()`. Supersedes the #1775 generation-token `stop()`.
    pub fn stop_owner(&self, requester: Option<&str>) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.enabled {
            return Ok(());
        }
        if let Some(req) = requester {
            // A targeted stop only acts when the requester owns the live
            // recording. An anonymously-owned recording (owner == None) is
            // never torn down by a session-scoped stop — only an unconditional
            // `stop_owner(None)` reaches it.
            if inner.owner.as_deref() != Some(req) {
                return Ok(());
            }
        }
        inner.owner = None;
        let dir = inner.output_dir.clone();
        let (video_meta, stop_error) = match inner.video.take().map(|rec| rec.stop()) {
            Some(Ok(meta)) => (Some(meta), None),
            Some(Err(error)) => (None, Some(error.to_string())),
            None => (None, None),
        };
        let cursor_samples = inner.cursor.take().map(|c| c.stop()).unwrap_or(0);

        inner.enabled = false;
        inner.output_dir = None;
        inner.next_turn = 1;
        inner.session_start_ms = 0;
        inner.session_monotonic_start = None;
        if let Some(error) = &stop_error {
            inner.last_error = Some(error.clone());
        } else if video_meta.is_some() {
            inner.last_error = None;
        }
        inner.last_video = video_meta.clone();
        inner.last_cursor_samples = cursor_samples;
        let final_video_error = inner.last_error.clone();

        // Rewrite session.json with final video metadata + cursor count
        // so the renderer (and any external analysis) sees what actually
        // landed.
        if let Some(dir) = dir {
            let video_block = if let Some(ref m) = video_meta {
                video_session_payload(true, None, Some(m))
            } else {
                video_session_payload(false, final_video_error.as_deref(), None)
            };
            let session_payload = serde_json::json!({
                "schema_version": 1,
                "started_at_monotonic_ms": now_ms(),
                "video": video_block,
                "cursor": { "present": cursor_samples > 0, "sample_count": cursor_samples }
            });
            let _ = write_json_atomic(
                &dir.join("session.json"),
                &session_payload,
            );
        }
        if let Some(error) = stop_error {
            anyhow::bail!("video finalization failed: {error}");
        }
        Ok(())
    }

    /// Legacy toggle API kept as a thin shim over `start()`/`stop()` so
    /// existing callers (CLI subcommand, tests) keep compiling during the
    /// rename window. Forces `record_video` on for this legacy CLI path — the
    /// MCP `start_recording` tool now defaults video OFF (see
    /// `recording_tools.rs`), but the CLI `recording start` keeps video on.
    pub fn configure(&self, enabled: bool, output_dir: Option<&str>) -> anyhow::Result<()> {
        if !enabled {
            return self.stop_owner(None);
        }
        let dir = output_dir
            .ok_or_else(|| anyhow::anyhow!("output_dir is required when enabling recording"))?;
        // Legacy CLI path: anonymous owner (no MCP session id available here).
        self.start(dir, true, None)
    }

    /// Return a snapshot of the current state (non-blocking).
    pub fn current_state(&self) -> RecordingState {
        let inner = self.inner.lock().unwrap();
        RecordingState {
            enabled: inner.enabled,
            output_dir: inner.output_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
            next_turn: inner.next_turn,
            last_error: inner.last_error.clone(),
            video_active: inner.video.is_some(),
            last_video_path: inner.last_video.as_ref()
                .map(|m| m.path.to_string_lossy().into_owned()),
            owner: inner.owner.clone(),
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

        // Strip the daemon-injected `_session_id` (and any other reserved
        // `_`-prefixed internal keys) before recording so the UUID never lands
        // in action.json's `arguments`. The injection point is the daemon
        // `call` branch (serve.rs); recording is the single chokepoint where
        // those internal keys must not leak into the persisted trajectory.
        let args = strip_internal_keys(args);

        if let Err(e) = write_turn(
            &turn_dir,
            tool_name,
            args.as_ref(),
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

/// Drop reserved internal keys (any `_`-prefixed key, e.g. the daemon-injected
/// `_session_id`) from a tool-call args object so they never persist into a
/// recorded `action.json`. Returns the value unchanged when it isn't an object
/// or carries no internal keys (cheap clone-free fast path).
fn strip_internal_keys(args: &Value) -> std::borrow::Cow<'_, Value> {
    match args.as_object() {
        Some(map) if map.keys().any(|k| k.starts_with('_')) => {
            let cleaned: serde_json::Map<String, Value> = map
                .iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            std::borrow::Cow::Owned(Value::Object(cleaned))
        }
        _ => std::borrow::Cow::Borrowed(args),
    }
}

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

    use crate::tool_args::ArgsExt;
    // Extract window_id and pid from args for screenshot capture.
    let window_id = args.opt_u64("window_id");
    let pid       = args.opt_i64("pid");
    let element_index = args.opt_u64("element_index");

    // Extract click point for click-family tools. Falls back to the
    // platform element_index → window-local-pixels resolver when the call
    // used `element_index` instead of explicit `x, y`, so click.png is
    // written for AX-indexed clicks too.
    let click_point: Option<(f64, f64)> = if matches!(
        tool_name, "click" | "double_click" | "right_click"
    ) {
        match (args.opt_f64("x"), args.opt_f64("y")) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => match (window_id, pid, element_index, ELEMENT_BOUNDS_FN.get()) {
                (Some(wid), Some(p), Some(idx), Some(f)) => {
                    u32::try_from(idx).ok().and_then(|idx32| f(wid, p, idx32))
                }
                _ => None,
            },
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

    // Post-action AX/UIA snapshot — omitted on platforms that don't expose
    // a cheap snapshot helper (today: Linux ATSPI).
    if let Some(ax_fn) = AX_SNAPSHOT_FN.get() {
        if let Some(json_bytes) = ax_fn(window_id, pid) {
            let _ = std::fs::write(turn_dir.join("app_state.json"), &json_bytes);
        }
    }

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

/// Build the `session.json` `video` field. Three shapes:
///   - not requested or ffmpeg missing: `{ present: false, error?: "..." }`
///   - in-flight session before stop: `{ present: true, path: "recording.mp4" }`
///   - finalized session after stop: full metadata
fn video_session_payload(
    present: bool,
    error: Option<&str>,
    meta: Option<&VideoMetadata>,
) -> Value {
    if !present {
        let mut o = serde_json::json!({ "present": false });
        if let Some(err) = error {
            o["error"] = serde_json::Value::String(err.to_owned());
        }
        return o;
    }
    if let Some(meta) = meta {
        return serde_json::json!({
            "present": true,
            "path": "recording.mp4",
            "absolute_path": meta.path.to_string_lossy(),
            "duration_ms": meta.duration_ms,
            "finalized": meta.finalized,
        });
    }
    serde_json::json!({
        "present": true,
        "path": "recording.mp4",
    })
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingVideo;

    impl VideoBackend for FailingVideo {
        fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
            anyhow::bail!("recorder did not finalize")
        }
    }

    #[test]
    fn stop_owner_surfaces_video_finalization_failure() {
        let output_dir = std::env::temp_dir().join(format!(
            "cua-recording-stop-failure-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&output_dir).expect("create recording test directory");
        let session = RecordingSession::new();
        {
            let mut inner = session.inner.lock().expect("recording lock");
            inner.enabled = true;
            inner.output_dir = Some(output_dir.clone());
            inner.video = Some(Box::new(FailingVideo));
        }

        let error = session
            .stop_owner(None)
            .expect_err("video finalization failure must reach the caller");
        assert!(error.to_string().contains("recorder did not finalize"));
        let state = session.current_state();
        assert!(!state.enabled);
        assert!(state.last_video_path.is_none());
        assert_eq!(
            state.last_error.as_deref(),
            Some("recorder did not finalize")
        );

        let manifest: Value = serde_json::from_slice(
            &std::fs::read(output_dir.join("session.json")).expect("read session manifest"),
        )
        .expect("parse session manifest");
        assert_eq!(manifest["video"]["present"], false);
        assert_eq!(manifest["video"]["error"], "recorder did not finalize");
        let _ = std::fs::remove_dir_all(output_dir);
    }
}
