//! Trajectory recording session.
//!
//! When enabled, every non-read-only, non-recording tool call writes a
//! `turn-NNNNN/action.json` file to the configured output directory. Targeted
//! turns also persist explicit before/after state and image evidence. The
//! legacy `app_state.json` and `screenshot.png` names remain post-action aliases.
//!
//! Schema mirrors the Swift/Windows reference `action.json`:
//!   { tool, arguments, result_summary, result_error, timestamp,
//!     t_ms_from_session_start, t_start_ms_from_session_start }

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use std::time::Instant;

use serde_json::Value;

use crate::cursor_sampler::CursorSampler;
use crate::video::{self, VideoBackend, VideoMetadata};

// ── Platform screenshot callback ─────────────────────────────────────────────
//
// Registered once at startup by each platform crate. Takes (window_id, pid)
// and returns raw PNG bytes, or None if capture fails. The callback is called
// synchronously from write_turn (a blocking context).

pub struct ScreenshotCapture {
    pub png: Option<Vec<u8>>,
    pub classification: Option<&'static str>,
}

impl ScreenshotCapture {
    pub fn captured(png: Vec<u8>) -> Self {
        Self {
            png: Some(png),
            classification: None,
        }
    }

    pub fn unavailable(classification: &'static str) -> Self {
        Self {
            png: None,
            classification: Some(classification),
        }
    }
}

type ScreenshotFnBox = Box<dyn Fn(Option<u64>, Option<i64>) -> ScreenshotCapture + Send + Sync>;
static SCREENSHOT_FN: OnceLock<ScreenshotFnBox> = OnceLock::new();

/// Register the platform-specific screenshot callback. Call once at startup
/// before any tool invocations. Subsequent calls are silently ignored.
pub fn set_screenshot_fn(
    f: impl Fn(Option<u64>, Option<i64>) -> Option<Vec<u8>> + Send + Sync + 'static,
) {
    set_classified_screenshot_fn(move |window_id, pid| {
        f(window_id, pid)
            .map(ScreenshotCapture::captured)
            .unwrap_or_else(|| ScreenshotCapture::unavailable("capture_failed"))
    });
}

/// Register a screenshot callback that preserves a stable unavailable-capture
/// classification for the turn evidence manifest.
pub fn set_classified_screenshot_fn(
    f: impl Fn(Option<u64>, Option<i64>) -> ScreenshotCapture + Send + Sync + 'static,
) {
    let _ = SCREENSHOT_FN.set(Box::new(f));
}

/// Invoke the registered screenshot callback. Returns `None` when no
/// callback was registered or when the platform capture failed. Used
/// by the PiP push hook (and by anything else that wants to share the
/// per-turn screenshot pipeline without duplicating the platform glue).
pub fn screenshot_for(window_id: Option<u64>, pid: Option<i64>) -> Option<Vec<u8>> {
    SCREENSHOT_FN
        .get()
        .and_then(|capture| capture(window_id, pid).png)
}

// ── Platform click-marker callback ───────────────────────────────────────────
//
// Takes (png_bytes, cx, cy) and returns modified PNG bytes with a red crosshair
// at (cx, cy), or None if drawing fails. Used to produce click.png alongside
// before.png when a click-family tool is recorded, producing click.png.

type ClickMarkerFnBox = Box<dyn Fn(&[u8], f64, f64) -> Option<Vec<u8>> + Send + Sync>;
static CLICK_MARKER_FN: OnceLock<ClickMarkerFnBox> = OnceLock::new();

/// Register the platform-specific click-marker callback. Call once at startup.
pub fn set_click_marker_fn(f: impl Fn(&[u8], f64, f64) -> Option<Vec<u8>> + Send + Sync + 'static) {
    let _ = CLICK_MARKER_FN.set(Box::new(f));
}

// ── Platform AX-snapshot callback ────────────────────────────────────────────
//
// Takes (window_id, pid, budget) and returns JSON bytes for the phase's
// application state. The post-action bytes are also kept as legacy
// `app_state.json`.
//
// Every platform walks its own accessibility API, but the per-turn contract is
// shared: the walk honours `budget.timeout_ms` through the common
// [`crate::walk_budget::WalkBudget`] and reports the shared walk fields
// (`truncated`, `truncation_reason`, `nodes_visited`, `nodes_pending`,
// `walk_elapsed_ms`, `timeout_ms`) in its payload. The recorder additionally
// runs the hook under a hard backstop so a provider that ignores the budget
// cannot hold the action for longer than `timeout_ms` plus a fixed grace.

/// Default wall-clock budget for one per-turn application-state walk. Matches
/// the default `timeout_ms` of `get_window_state` on every platform.
pub const TURN_STATE_TIMEOUT_MS_DEFAULT: u64 = crate::tool_schema::TIMEOUT_MS_DEFAULT;

/// Slack past the budget before the recorder stops waiting for a state hook:
/// one in-flight accessibility request plus serialization.
pub const TURN_STATE_BACKSTOP_GRACE: Duration = Duration::from_millis(1000);

/// Budget handed to the platform application-state hook for one turn phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateCaptureBudget {
    pub timeout_ms: u64,
}

impl StateCaptureBudget {
    /// The recorder stops waiting for the hook at this point.
    pub fn backstop(self) -> Duration {
        Duration::from_millis(self.timeout_ms) + TURN_STATE_BACKSTOP_GRACE
    }
}

impl Default for StateCaptureBudget {
    fn default() -> Self {
        Self {
            timeout_ms: TURN_STATE_TIMEOUT_MS_DEFAULT,
        }
    }
}

type AxSnapshotFn =
    Arc<dyn Fn(Option<u64>, Option<i64>, StateCaptureBudget) -> Option<Vec<u8>> + Send + Sync>;
static AX_SNAPSHOT_FN: OnceLock<AxSnapshotFn> = OnceLock::new();

/// Register an AX/UIA snapshot callback that ignores the per-turn budget.
/// The recorder's backstop still bounds how long a turn waits for it. Call
/// once at startup; platform runtimes use [`set_budgeted_ax_snapshot_fn`].
pub fn set_ax_snapshot_fn(
    f: impl Fn(Option<u64>, Option<i64>) -> Option<Vec<u8>> + Send + Sync + 'static,
) {
    set_budgeted_ax_snapshot_fn(move |window_id, pid, _budget| f(window_id, pid));
}

/// Register the platform-specific AX/UIA/AT-SPI snapshot callback. The hook
/// must bound its walk by `budget.timeout_ms` and should report the shared
/// walk fields in its JSON payload. Call once at startup.
pub fn set_budgeted_ax_snapshot_fn(
    f: impl Fn(Option<u64>, Option<i64>, StateCaptureBudget) -> Option<Vec<u8>> + Send + Sync + 'static,
) {
    let _ = AX_SNAPSHOT_FN.set(Arc::new(f));
}

/// Hooks abandoned at their backstop and still running, per target pid. While
/// one is outstanding the recorder does not start another walk of the same
/// application, so a hung accessibility provider cannot accumulate threads.
static ABANDONED_STATE_CAPTURES: OnceLock<Mutex<HashMap<i64, usize>>> = OnceLock::new();

fn abandoned_state_captures() -> std::sync::MutexGuard<'static, HashMap<i64, usize>> {
    ABANDONED_STATE_CAPTURES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn release_abandoned_state_capture(key: i64) {
    let mut abandoned = abandoned_state_captures();
    if let Some(count) = abandoned.get_mut(&key) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            abandoned.remove(&key);
        }
    }
}

/// Outcome of one phase's application-state capture.
#[derive(Debug, Default)]
struct StateCapture {
    bytes: Option<Vec<u8>>,
    /// Stable classification when `bytes` is absent for a known reason.
    classification: Option<&'static str>,
    /// `not_applicable` instead of `unavailable` (deliberate skip).
    not_applicable: bool,
    /// Shared walk fields copied from the payload for the evidence manifest.
    walk: Option<serde_json::Map<String, Value>>,
}

impl StateCapture {
    fn skipped(classification: &'static str) -> Self {
        Self {
            classification: Some(classification),
            not_applicable: true,
            ..Self::default()
        }
    }

    fn unavailable(classification: &'static str) -> Self {
        Self {
            classification: Some(classification),
            ..Self::default()
        }
    }

    fn captured(bytes: Vec<u8>) -> Self {
        let walk = walk_fields(&bytes);
        Self {
            bytes: Some(bytes),
            walk,
            ..Self::default()
        }
    }
}

/// Copy the shared walk fields (see [`crate::walk_budget::WalkOutcome::apply`])
/// out of a state payload so the manifest can say when the tree is partial.
fn walk_fields(bytes: &[u8]) -> Option<serde_json::Map<String, Value>> {
    const KEYS: [&str; 6] = [
        "truncated",
        "truncation_reason",
        "nodes_visited",
        "nodes_pending",
        "walk_elapsed_ms",
        "timeout_ms",
    ];
    let payload: Value = serde_json::from_slice(bytes).ok()?;
    let fields: serde_json::Map<String, Value> = KEYS
        .iter()
        .filter_map(|key| {
            payload
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect();
    (!fields.is_empty()).then_some(fields)
}

/// Run a state hook with a hard deadline. The hook runs on its own thread; if
/// it has not answered by `budget.backstop()` the turn proceeds without state
/// (classified `state_capture_timeout`) and the thread is left to finish on
/// its own. Its late result is discarded.
fn run_state_hook(
    hook: AxSnapshotFn,
    window_id: Option<u64>,
    pid: Option<i64>,
    budget: StateCaptureBudget,
) -> StateCapture {
    use std::sync::atomic::{AtomicBool, Ordering};
    let key = pid.unwrap_or(-1);
    if abandoned_state_captures()
        .get(&key)
        .is_some_and(|count| *count > 0)
    {
        return StateCapture::unavailable("state_capture_busy");
    }
    let abandoned = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let worker_abandoned = abandoned.clone();
    let spawned = std::thread::Builder::new()
        .name("cua-recording-state".into())
        .spawn(move || {
            let result = hook(window_id, pid, budget);
            if worker_abandoned.swap(true, Ordering::SeqCst) {
                release_abandoned_state_capture(key);
            } else {
                let _ = sender.send(result);
            }
        });
    if spawned.is_err() {
        return StateCapture::unavailable("capture_failed");
    }
    let received = match receiver.recv_timeout(budget.backstop()) {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            *abandoned_state_captures().entry(key).or_insert(0) += 1;
            if abandoned.swap(true, Ordering::SeqCst) {
                // The worker finished between the timeout and the mark.
                release_abandoned_state_capture(key);
                receiver.try_recv().ok()
            } else {
                tracing::warn!(
                    target: "recording",
                    "application-state capture for pid {key} exceeded its {} ms budget; \
                     recording the turn without state",
                    budget.timeout_ms
                );
                return StateCapture::unavailable("state_capture_timeout");
            }
        }
    };
    match received.flatten() {
        Some(bytes) => StateCapture::captured(bytes),
        None => StateCapture::unavailable("capture_failed"),
    }
}

fn capture_state(
    window_id: Option<u64>,
    pid: Option<i64>,
    budget: StateCaptureBudget,
) -> StateCapture {
    match AX_SNAPSHOT_FN.get() {
        Some(hook) => run_state_hook(hook.clone(), window_id, pid, budget),
        None => StateCapture::default(),
    }
}

/// How a turn phase should treat the application-state walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatePlan {
    /// Walk under this budget.
    Capture(StateCaptureBudget),
    /// Deliberately skipped: recorded as `not_applicable` with this reason.
    Skip(&'static str),
}

/// Classification for application state that was not walked because the
/// action was refused before any input was dispatched.
pub const STATE_SKIPPED_REFUSED: &str = "action_refused_before_dispatch";
/// Classification for application state disabled by `start_recording`.
pub const STATE_SKIPPED_DISABLED: &str = "state_capture_disabled";
/// Classification for before-state skipped on a predicted refusal that the
/// tool did not in fact refuse.
pub const STATE_SKIPPED_PREDICTION_MISSED: &str = "skipped_predicted_refusal";

// ── Platform element-bounds callback ─────────────────────────────────────────
//
// Resolves an element_index to its center point in window-local screenshot
// pixels (the same coordinate space as the existing `(cx, cy)` arg to
// `CLICK_MARKER_FN`). Used so click.png is also written on element-indexed
// clicks, not just pixel-addressed ones.

type ElementBoundsFnBox =
    Box<dyn Fn(i64, &Value, bool) -> Option<(u64, Option<(f64, f64)>)> + Send + Sync>;
static ELEMENT_BOUNDS_FN: OnceLock<ElementBoundsFnBox> = OnceLock::new();

type PixelPointFnBox =
    Box<dyn Fn(&Value, Option<u64>, Option<i64>, f64, f64) -> Option<(f64, f64)> + Send + Sync>;

/// Register the platform-specific element-bounds resolver. Args: (window_id, pid, element_index).
pub fn set_element_bounds_fn(
    f: impl Fn(i64, &Value, bool) -> Option<(u64, Option<(f64, f64)>)> + Send + Sync + 'static,
) {
    let _ = ELEMENT_BOUNDS_FN.set(Box::new(f));
}

#[derive(Default)]
struct TurnCapture {
    state: StateCapture,
    screenshot: Option<Vec<u8>>,
    screenshot_classification: Option<&'static str>,
}

#[derive(Default)]
struct DispatchClickCapture {
    image: Option<(Vec<u8>, f64, f64)>,
    failure: Option<&'static str>,
}

struct DispatchClickScope {
    window_id: u64,
    pid: i64,
    capture: Mutex<DispatchClickCapture>,
}

tokio::task_local! {
    static DISPATCH_CLICK_SCOPE: Option<Arc<DispatchClickScope>>;
}

/// Capture the exact target after scrolling/focus preparation and before input.
/// The closure is lazy: unrelated, private, and unrecorded calls never capture.
/// Only the first matching capture is attempted, including capture failures.
pub fn capture_dispatch_click_target(
    window_id: u64,
    pid: i64,
    capture: impl FnOnce() -> Option<(Vec<u8>, f64, f64)>,
) {
    let _ = DISPATCH_CLICK_SCOPE.try_with(|scope| {
        let Some(scope) = scope else { return };
        if (scope.window_id, scope.pid) != (window_id, pid) {
            return;
        }
        let mut retained = scope.capture.lock().unwrap();
        if retained.image.is_some() || retained.failure.is_some() {
            return;
        }
        match capture() {
            Some((png, x, y)) if point_in_image(&png, x, y) => {
                retained.image = Some((png, x, y));
            }
            Some(_) => retained.failure = Some("invalid_capture"),
            None => retained.failure = Some("capture_failed"),
        }
    });
}

fn point_in_image(png: &[u8], x: f64, y: f64) -> bool {
    image::load_from_memory_with_format(png, image::ImageFormat::Png).is_ok_and(|image| {
        let (width, height) = (image.width(), image.height());
        x.is_finite()
            && y.is_finite()
            && x >= 0.0
            && y >= 0.0
            && x < f64::from(width)
            && y < f64::from(height)
    })
}

/// Task-local state follows the invocation future and disappears on cancellation.
/// An empty scope also masks any outer recording during a nested dispatch.
pub(crate) async fn scope_dispatch_click_capture<F: std::future::Future>(
    pending: Option<&PendingTurn>,
    dispatch: F,
) -> F::Output {
    DISPATCH_CLICK_SCOPE
        .scope(
            pending.and_then(|turn| turn.dispatch_click.clone()),
            dispatch,
        )
        .await
}

/// A reserved recording turn captured immediately before tool dispatch.
/// `ToolRegistry` passes this token back after dispatch so both phases share
/// one stable `turn-NNNNN` directory even when calls complete out of order.
pub struct PendingTurn {
    generation: u64,
    turn_dir: PathBuf,
    tool_name: String,
    args: Value,
    start_ms: u64,
    session_start_ms: u64,
    window_id: Option<u64>,
    pid: Option<i64>,
    click_point: Option<(f64, f64)>,
    capture_visual_state: bool,
    /// Per-turn application-state budget; `None` when state capture is
    /// disabled for this recording.
    state_budget: Option<StateCaptureBudget>,
    /// Refusal code predicted before dispatch (for example an unknown or
    /// expired `capture_id`). The before-state walk was skipped for it.
    predicted_refusal: Option<&'static str>,
    before: TurnCapture,
    dispatch_click: Option<Arc<DispatchClickScope>>,
}

/// Persistent recording session state owned by one tool registry.
pub struct RecordingSession {
    inner: Mutex<RecordingInner>,
    pixel_point_fn: OnceLock<PixelPointFnBox>,
}

struct RecordingInner {
    enabled: bool,
    generation: u64,
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
    /// Per-turn application-state budget, or `None` when the caller disabled
    /// state capture with `include_accessibility_tree: false`.
    state_budget: Option<StateCaptureBudget>,
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
            pixel_point_fn: OnceLock::new(),
            inner: Mutex::new(RecordingInner {
                enabled: false,
                generation: 0,
                owner: None,
                output_dir: None,
                state_budget: Some(StateCaptureBudget::default()),
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

    /// Install this registry's mapper from tool pixels to recording pixels.
    /// Register once during runtime assembly; absent mappers preserve coordinates.
    pub fn set_pixel_point_fn(
        &self,
        f: impl Fn(&Value, Option<u64>, Option<i64>, f64, f64) -> Option<(f64, f64)>
            + Send
            + Sync
            + 'static,
    ) {
        let _ = self.pixel_point_fn.set(Box::new(f));
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
    /// the per-turn capture (action.json + pre/post evidence) is independent
    /// of video — but the structured state carries the ffmpeg error so
    /// the caller can surface it.
    ///
    /// `owner` stamps the session that owns this recording (the daemon-injected
    /// `_session_id`). `None` marks an anonymous start (CLI one-shot / legacy
    /// `configure()` shim) owned by nobody. See `stop_owner()` for how this
    /// gates teardown.
    pub fn start(
        &self,
        output_dir: &str,
        record_video: bool,
        owner: Option<&str>,
    ) -> anyhow::Result<()> {
        self.start_with_state_budget(
            output_dir,
            record_video,
            owner,
            Some(StateCaptureBudget::default()),
        )
    }

    /// [`Self::start`] with an explicit per-turn application-state budget.
    /// `None` disables the before/after accessibility walks entirely; the
    /// turn evidence then classifies state as `state_capture_disabled`.
    pub fn start_with_state_budget(
        &self,
        output_dir: &str,
        record_video: bool,
        owner: Option<&str>,
        state_budget: Option<StateCaptureBudget>,
    ) -> anyhow::Result<()> {
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
            Ok(s) => {
                inner.cursor = Some(s);
            }
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
        let _ = write_json_atomic(&dir.join("session.json"), &session_payload);

        // Stamp the owning session on every successful start (reached only on
        // the success path — start() returns early via `?` on `create_dir_all`
        // failure above). `owner` clobbers any previous owner, which is correct:
        // the daemon-global recorder is a singleton, so the latest start() owns
        // it. The previous owner's disconnect then no-ops in stop_owner().
        inner.owner = owner.map(str::to_owned);
        inner.generation = inner.generation.wrapping_add(1);
        inner.enabled = true;
        inner.output_dir = Some(dir);
        inner.state_budget = state_budget;
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
    ///
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
            Some(Ok(meta)) => match validate_video_metadata(meta) {
                Ok(meta) => (Some(meta), None),
                Err(error) => (None, Some(error.to_string())),
            },
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
            let _ = write_json_atomic(&dir.join("session.json"), &session_payload);
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
            output_dir: inner
                .output_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            next_turn: inner.next_turn,
            last_error: inner.last_error.clone(),
            video_active: inner.video.is_some(),
            last_video_path: inner
                .last_video
                .as_ref()
                .map(|m| m.path.to_string_lossy().into_owned()),
            owner: inner.owner.clone(),
        }
    }

    /// Per-turn application-state budget of the current (or next) recording;
    /// `None` when state capture is disabled.
    pub fn state_budget(&self) -> Option<StateCaptureBudget> {
        self.inner.lock().unwrap().state_budget
    }

    /// Reserve a turn and capture its target immediately before tool dispatch.
    /// No-op when recording is disabled.
    pub fn begin_turn(&self, tool_name: &str, args: &Value, start_ms: u64) -> Option<PendingTurn> {
        self.begin_turn_with_capture(tool_name, args, start_ms, true, None)
    }

    /// [`Self::begin_turn`] for a call the dispatcher already knows the tool
    /// will refuse before dispatching input (`predicted_refusal` carries the
    /// refusal code, e.g. `capture_expired`). The before screenshot is still
    /// captured, but the application-state walk is skipped because the action
    /// cannot change the application. If the tool does not in fact refuse, the
    /// after phase is captured normally and the manifest records the miss.
    pub fn begin_turn_with_refusal_hint(
        &self,
        tool_name: &str,
        args: &Value,
        start_ms: u64,
        predicted_refusal: Option<&'static str>,
    ) -> Option<PendingTurn> {
        self.begin_turn_with_capture(tool_name, args, start_ms, true, predicted_refusal)
    }

    /// Reserve a turn while deliberately suppressing visual and accessibility
    /// capture. Used for consent-bearing operations where the target may be an
    /// authenticated browser profile: action metadata and the structured result
    /// remain auditable without persisting page or dialog contents.
    pub fn begin_private_turn(
        &self,
        tool_name: &str,
        args: &Value,
        start_ms: u64,
    ) -> Option<PendingTurn> {
        self.begin_turn_with_capture(tool_name, args, start_ms, false, None)
    }

    fn begin_turn_with_capture(
        &self,
        tool_name: &str,
        args: &Value,
        start_ms: u64,
        capture_visual_state: bool,
        predicted_refusal: Option<&'static str>,
    ) -> Option<PendingTurn> {
        let (turn_dir, session_start_ms, generation, state_budget) = {
            let mut inner = self.inner.lock().unwrap();
            if !inner.enabled {
                return None;
            }
            let out = inner.output_dir.clone()?;
            let idx = inner.next_turn;
            inner.next_turn += 1;
            (
                out.join(format!("turn-{idx:05}")),
                inner.session_start_ms,
                inner.generation,
                inner.state_budget,
            )
        };

        // Strip the daemon-injected `_session_id` (and any other reserved
        // `_`-prefixed internal keys) before recording so the UUID never lands
        // in action.json's `arguments`. The injection point is the daemon
        // `call` branch (serve.rs); recording is the single chokepoint where
        // those internal keys must not leak into the persisted trajectory.
        let args = strip_internal_keys(args).into_owned();
        use crate::tool_args::ArgsExt;
        let pid = args.opt_i64("pid");
        let element = pid.and_then(|pid| {
            ELEMENT_BOUNDS_FN.get()?(
                pid,
                &args,
                matches!(tool_name, "click" | "double_click" | "right_click"),
            )
        });
        let window_id = element
            .map(|(window, _)| window)
            .or_else(|| args.opt_u64("window_id"));
        let click_point = resolve_click_point(
            tool_name,
            &args,
            element.and_then(|(_, point)| point),
            window_id,
            pid,
            self.pixel_point_fn.get(),
        );
        let before = if capture_visual_state {
            capture_turn(
                window_id,
                pid,
                match (state_budget, predicted_refusal) {
                    (None, _) => StatePlan::Skip(STATE_SKIPPED_DISABLED),
                    (Some(_), Some(_)) => StatePlan::Skip(STATE_SKIPPED_REFUSED),
                    (Some(budget), None) => StatePlan::Capture(budget),
                },
            )
        } else {
            TurnCapture::privacy_suppressed()
        };

        let mut inner = self.inner.lock().unwrap();
        if !inner.enabled || inner.generation != generation {
            return None;
        }
        if let Err(error) = write_phase_artifacts(&turn_dir, "before", &before) {
            inner.last_error = Some(error.to_string());
        }
        drop(inner);

        Some(PendingTurn {
            generation,
            turn_dir,
            tool_name: tool_name.to_owned(),
            args,
            start_ms,
            session_start_ms,
            window_id,
            pid,
            click_point,
            capture_visual_state,
            state_budget,
            predicted_refusal,
            before,
            dispatch_click: if capture_visual_state
                && matches!(tool_name, "click" | "double_click" | "right_click")
            {
                window_id.zip(pid).map(|(window_id, pid)| {
                    Arc::new(DispatchClickScope {
                        window_id,
                        pid,
                        capture: Mutex::new(DispatchClickCapture::default()),
                    })
                })
            } else {
                None
            },
        })
    }

    /// Finalize a previously reserved turn after tool dispatch.
    pub fn finish_turn(&self, pending: PendingTurn, result_text: &str) {
        self.finish_turn_with_action(pending, result_text, None);
    }

    /// Finalize a turn while retaining the daemon's rich, non-wire action
    /// truth in the recording artifact. Existing trajectory readers can ignore
    /// the additive `action_truth` key.
    pub fn finish_turn_with_action(
        &self,
        pending: PendingTurn,
        result_text: &str,
        action_record: Option<&crate::action_record::ActionExecutionRecord>,
    ) {
        self.finish_turn_with_outcome(pending, result_text, action_record, false);
    }

    /// Finalize a turn while also recording whether dispatch returned an
    /// error. A click-family call rejected before its target can be resolved
    /// has no click point to annotate; retaining this bit lets the evidence
    /// manifest distinguish that honest non-action from a missing marker on a
    /// dispatched click.
    pub fn finish_turn_with_outcome(
        &self,
        pending: PendingTurn,
        result_text: &str,
        action_record: Option<&crate::action_record::ActionExecutionRecord>,
        result_is_error: bool,
    ) {
        let is_live = |inner: &RecordingInner| {
            let live = inner.enabled && inner.generation == pending.generation;
            if !live {
                tracing::warn!(
                    target: "recording",
                    "discarding a turn from an inactive recording generation"
                );
            }
            live
        };
        if !is_live(&self.inner.lock().unwrap()) {
            return;
        }
        // Capture outside the session lock: the after phase is bounded, but
        // recording-state queries and unrelated turns must not wait on it.
        let after = capture_after(&pending, action_record, result_is_error);
        let mut inner = self.inner.lock().unwrap();
        if !is_live(&inner) {
            return;
        }
        if let Err(error) =
            write_turn_with_after(pending, after, result_text, action_record, result_is_error)
        {
            inner.last_error = Some(error.to_string());
        }
    }

    /// Compatibility helper for callers that only report completed calls.
    /// New dispatch paths should use `begin_turn` and `finish_turn` so the
    /// before phase is captured before the action changes application state.
    pub fn record(&self, tool_name: &str, args: &Value, result_text: &str, start_ms: u64) {
        let Some(pending) = self.begin_turn(tool_name, args, start_ms) else {
            return;
        };
        self.finish_turn(pending, result_text);
    }
}

impl Default for RecordingSession {
    fn default() -> Self {
        Self::new()
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

impl TurnCapture {
    fn privacy_suppressed() -> Self {
        Self {
            state: StateCapture::default(),
            screenshot: None,
            screenshot_classification: Some("privacy_suppressed"),
        }
    }
}

fn capture_turn(window_id: Option<u64>, pid: Option<i64>, state: StatePlan) -> TurnCapture {
    let screenshot = SCREENSHOT_FN
        .get()
        .map(|capture| capture(window_id, pid))
        .unwrap_or_else(|| ScreenshotCapture::unavailable("capture_hook_unavailable"));
    TurnCapture {
        state: match state {
            // No target application: there is no state to walk.
            _ if pid.is_none() => StateCapture::default(),
            StatePlan::Capture(budget) => capture_state(window_id, pid, budget),
            StatePlan::Skip(classification) => StateCapture::skipped(classification),
        },
        screenshot: screenshot.png,
        screenshot_classification: screenshot.classification,
    }
}

/// Whether the tool refused the call before any input reached the target, so
/// the application cannot have changed. A click-family call rejected with no
/// action record is only treated as refused when the dispatcher predicted it.
fn refused_before_any_dispatch(
    pending: &PendingTurn,
    action_record: Option<&crate::action_record::ActionExecutionRecord>,
    result_is_error: bool,
) -> bool {
    result_is_error
        && match action_record {
            Some(record) => record.effect == crate::action_record::ActionEffect::Refused,
            None => pending.predicted_refusal.is_some(),
        }
}

/// Plan and run the post-action capture. The after screenshot is always taken
/// (it is cheap and keeps the visual trajectory continuous); the state walk is
/// skipped when the call was refused before dispatch.
fn capture_after(
    pending: &PendingTurn,
    action_record: Option<&crate::action_record::ActionExecutionRecord>,
    result_is_error: bool,
) -> TurnCapture {
    if !pending.capture_visual_state {
        return TurnCapture::privacy_suppressed();
    }
    let plan = match pending.state_budget {
        None => StatePlan::Skip(STATE_SKIPPED_DISABLED),
        Some(_) if refused_before_any_dispatch(pending, action_record, result_is_error) => {
            StatePlan::Skip(STATE_SKIPPED_REFUSED)
        }
        Some(budget) => StatePlan::Capture(budget),
    };
    capture_turn(pending.window_id, pending.pid, plan)
}

fn resolve_click_point(
    tool_name: &str,
    args: &Value,
    element_point: Option<(f64, f64)>,
    window_id: Option<u64>,
    pid: Option<i64>,
    pixel_point_fn: Option<&PixelPointFnBox>,
) -> Option<(f64, f64)> {
    use crate::tool_args::ArgsExt;
    if !matches!(tool_name, "click" | "double_click" | "right_click") {
        return None;
    }
    match (args.opt_f64("x"), args.opt_f64("y")) {
        (Some(x), Some(y)) => match pixel_point_fn {
            Some(resolve) => resolve(args, window_id, pid, x, y),
            None => Some((x, y)),
        },
        _ => element_point,
    }
}

fn write_phase_artifacts(
    turn_dir: &Path,
    phase: &str,
    capture: &TurnCapture,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(turn_dir)?;
    if let Some(state) = &capture.state.bytes {
        std::fs::write(turn_dir.join(format!("{phase}_state.json")), state)?;
    }
    if let Some(screenshot) = &capture.screenshot {
        std::fs::write(turn_dir.join(format!("{phase}.png")), screenshot)?;
    }
    Ok(())
}

fn capture_status(captured: bool, expected: bool, classification: Option<&'static str>) -> Value {
    if captured {
        serde_json::json!({ "status": "captured" })
    } else if expected {
        serde_json::json!({
            "status": "unavailable",
            "classification": classification.unwrap_or("capture_failed")
        })
    } else {
        serde_json::json!({
            "status": "not_applicable",
            "classification": "no_target_pid"
        })
    }
}

fn state_status(state: &StateCapture, expected: bool) -> Value {
    if state.not_applicable && expected {
        return serde_json::json!({
            "status": "not_applicable",
            "classification": state.classification.unwrap_or("not_applicable"),
        });
    }
    let mut status = capture_status(state.bytes.is_some(), expected, state.classification);
    if let (Some(walk), Some(object)) = (&state.walk, status.as_object_mut()) {
        // A partial tree is still captured evidence; say where it stopped.
        object.extend(walk.clone());
    }
    status
}

fn write_evidence_manifest(
    turn_dir: &Path,
    before: &TurnCapture,
    after: &TurnCapture,
    state_expected: bool,
    click_expected: bool,
    click_captured: bool,
    supplemental: &DispatchClickCapture,
    click_not_applicable_classification: &'static str,
) -> anyhow::Result<()> {
    let mut manifest = serde_json::json!({
        "schema": "cua-turn-evidence/v1",
        "before": {
            "state": state_status(&before.state, state_expected),
            "screenshot": capture_status(
                before.screenshot.is_some(),
                true,
                before.screenshot_classification,
            ),
        },
        "after": {
            "state": state_status(&after.state, state_expected),
            "screenshot": capture_status(
                after.screenshot.is_some(),
                true,
                after.screenshot_classification,
            ),
        },
        "click": if click_expected {
            capture_status(click_captured, true, None)
        } else {
            serde_json::json!({
                "status": "not_applicable",
                "classification": click_not_applicable_classification,
            })
        },
    });
    manifest["click"]["source_image"] = serde_json::json!(if supplemental.image.is_some()
        || supplemental.failure.is_some()
    {
        "click_source.png"
    } else {
        "before.png"
    });
    if supplemental.image.is_some() || supplemental.failure.is_some() {
        manifest["click_source"] =
            capture_status(supplemental.image.is_some(), true, supplemental.failure);
        if supplemental.image.is_some() {
            manifest["click_source"]["source_image"] = serde_json::json!("click_source.png");
        }
    }
    write_json_atomic(&turn_dir.join("evidence.json"), &manifest)
}

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

// Semantic activation has no spatial marker only when the retained execution
// record proves a single accessibility dispatch without escalation or replay.
fn semantic_action_without_point(action: &Value) -> bool {
    let args = &action["arguments"];
    let truth = &action["action_truth"];
    action["tool"] == "click"
        && action["result_error"] == false
        && action.get("click_point").is_none()
        && action.get("click_point_image").is_none()
        && (args["element_index"].as_u64().is_some()
            || args["element_token"]
                .as_str()
                .is_some_and(|token| !token.is_empty()))
        && args.get("x").is_none()
        && args.get("y").is_none()
        && args.get("raw").is_none_or(|value| value == false)
        && args.get("button").is_none_or(|value| value == "left")
        && args
            .get("count")
            .is_none_or(|value| value.as_u64() == Some(1))
        && args
            .get("click_count")
            .is_none_or(|value| value.as_u64() == Some(1))
        && ["modifier", "modifiers"].iter().all(|key| {
            args.get(*key)
                .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
        })
        && matches!(
            truth["transport"].as_str(),
            Some(
                "macos_ax_action"
                    | "linux_at_spi_action"
                    | "windows_uia_invoke"
                    | "windows_uia_toggle"
                    | "windows_uia_selection"
                    | "windows_uia_expand_collapse"
            )
        )
        && truth["route"] == "accessibility"
        && matches!(truth["effect"].as_str(), Some("confirmed" | "unverifiable"))
        && matches!(
            truth["actual_delivery"].as_str(),
            Some("background" | "foreground")
        )
        && truth["requested_delivery"] == truth["actual_delivery"]
        && truth.get("escalation").is_some_and(Value::is_null)
        && truth["fallbacks"].as_array().is_some_and(Vec::is_empty)
        && truth
            .get("delivered_count")
            .is_some_and(|value| value.is_null() || value.as_u64() == Some(1))
        && truth["attempts"].as_array().is_some_and(|attempts| {
            // Legacy single-dispatch results carry transport/delivery above
            // without a per-attempt journal. Any supplied attempt must match.
            attempts.len() <= 1
                && attempts.iter().all(|attempt| {
                    attempt["transport"] == truth["transport"]
                        && attempt["delivery"] == truth["actual_delivery"]
                })
        })
}

#[cfg(test)]
fn write_turn(
    pending: PendingTurn,
    result_text: &str,
    action_record: Option<&crate::action_record::ActionExecutionRecord>,
    result_is_error: bool,
) -> anyhow::Result<()> {
    let after = capture_after(&pending, action_record, result_is_error);
    write_turn_with_after(pending, after, result_text, action_record, result_is_error)
}

fn write_turn_with_after(
    pending: PendingTurn,
    after: TurnCapture,
    result_text: &str,
    action_record: Option<&crate::action_record::ActionExecutionRecord>,
    result_is_error: bool,
) -> anyhow::Result<()> {
    let PendingTurn {
        generation: _,
        turn_dir,
        tool_name,
        args,
        start_ms,
        session_start_ms,
        window_id: _,
        pid,
        click_point,
        capture_visual_state,
        state_budget: _,
        predicted_refusal,
        mut before,
        dispatch_click,
    } = pending;
    let supplemental = dispatch_click
        .map(|scope| std::mem::take(&mut *scope.capture.lock().unwrap()))
        .unwrap_or_default();
    let click_source = supplemental
        .image
        .as_ref()
        .map(|(png, _, _)| png.as_slice());
    let click_point = supplemental
        .image
        .as_ref()
        .map(|(_, x, y)| (*x, *y))
        .or_else(|| {
            supplemental
                .failure
                .is_none()
                .then_some(click_point)
                .flatten()
        });
    let marker_source = click_source.or(before.screenshot.as_deref());
    // Offscreen accessibility bounds can be sentinel coordinates. Only a
    // point inside a readable pre-action image can ground a spatial marker.
    let click_point =
        match marker_source.and_then(|png| crate::image_utils::png_dimensions(png).ok()) {
            Some((width, height)) => click_point.filter(|(x, y)| {
                x.is_finite()
                    && y.is_finite()
                    && *x >= 0.0
                    && *y >= 0.0
                    && *x < f64::from(width)
                    && *y < f64::from(height)
            }),
            None => click_point,
        };
    std::fs::create_dir_all(&turn_dir)?;
    let now = now_ms();
    let click_family = matches!(tool_name.as_str(), "click" | "double_click" | "right_click");
    let action_refused = action_record
        .is_some_and(|record| record.effect == crate::action_record::ActionEffect::Refused);
    let refused_before_target_resolution = click_family
        && result_is_error
        && click_point.is_none()
        && (action_record.is_none() || action_refused);
    // A target may resolve successfully and still be refused before input
    // dispatch (for example, a minimized Windows element). Retaining the
    // resolved point in action.json is useful diagnostic context, but a
    // crosshair would falsely imply that a click was delivered.
    let refused_before_dispatch = click_family && result_is_error && action_refused;
    // The before-state walk was skipped on a predicted refusal. If the tool
    // did not in fact refuse, say so instead of claiming a refusal.
    if predicted_refusal.is_some()
        && before.state.classification == Some(STATE_SKIPPED_REFUSED)
        && !(result_is_error && (action_refused || action_record.is_none()))
    {
        before.state = StateCapture::unavailable(STATE_SKIPPED_PREDICTION_MISSED);
    }
    let mut payload = serde_json::json!({
        "tool": tool_name,
        "arguments": args,
        "result_summary": result_text,
        "result_error": result_is_error,
        "timestamp": iso_now(),
        "t_ms_from_session_start": now.saturating_sub(session_start_ms),
        "t_start_ms_from_session_start": start_ms.saturating_sub(session_start_ms),
    });
    if let Some((cx, cy)) = click_point {
        payload["click_point"] = serde_json::json!({"x": cx, "y": cy});
    }
    if click_source.is_some() || supplemental.failure.is_some() {
        payload["click_point_image"] = serde_json::json!("click_source.png");
    }
    if let Some(action_record) = action_record {
        payload["action_truth"] = action_record.debug_json();
    }
    let semantic_without_point =
        supplemental.failure.is_none() && semantic_action_without_point(&payload);
    let click_expected = click_family
        && !refused_before_target_resolution
        && !refused_before_dispatch
        && !semantic_without_point;
    write_json_atomic(&turn_dir.join("action.json"), &payload)?;
    write_phase_artifacts(&turn_dir, "after", &after)?;
    if let Some(source) = click_source {
        std::fs::write(turn_dir.join("click_source.png"), source)?;
    }

    // Preserve the original post-action names for existing trajectory readers.
    if let Some(state) = &after.state.bytes {
        std::fs::write(turn_dir.join("app_state.json"), state)?;
    }
    if let Some(screenshot) = &after.screenshot {
        std::fs::write(turn_dir.join("screenshot.png"), screenshot)?;
    }

    // A click marker describes where the action was aimed, so ground it on
    // the pre-action image. This also keeps modal-dismiss evidence available
    // after the modal HWND has closed.
    let mut click_captured = false;
    if click_expected {
        if let (Some((cx, cy)), Some(screenshot), Some(marker)) =
            (click_point, marker_source, CLICK_MARKER_FN.get())
        {
            if let Some(click_png) = marker(screenshot, cx, cy) {
                std::fs::write(turn_dir.join("click.png"), click_png)?;
                click_captured = true;
            }
        }
    }
    write_evidence_manifest(
        &turn_dir,
        &before,
        &after,
        pid.is_some() && capture_visual_state,
        click_expected,
        click_captured,
        &supplemental,
        if refused_before_target_resolution {
            "action_refused_before_target_resolution"
        } else if refused_before_dispatch {
            "action_refused_before_dispatch"
        } else if semantic_without_point {
            "semantic_action_without_point"
        } else {
            "not_a_click_action"
        },
    )?;

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

fn validate_video_metadata(meta: VideoMetadata) -> anyhow::Result<VideoMetadata> {
    if !meta.finalized {
        anyhow::bail!("video backend did not finalize {}", meta.path.display());
    }
    let output = std::fs::metadata(&meta.path).map_err(|error| {
        anyhow::anyhow!(
            "finalized video is missing at {}: {error}",
            meta.path.display()
        )
    })?;
    if output.len() == 0 {
        anyhow::bail!("finalized video is empty at {}", meta.path.display());
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_mappers_follow_registry_lifetimes_and_concurrent_runtime_state() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        fn registry(ratio: &Arc<AtomicU32>) -> (crate::tool::ToolRegistry, tempfile::TempDir) {
            let registry = crate::tool::ToolRegistry::new();
            let output = tempfile::tempdir().unwrap();
            {
                let mut inner = registry.recording.inner.lock().unwrap();
                inner.enabled = true;
                inner.output_dir = Some(output.path().to_owned());
            }
            let ratio = Arc::downgrade(ratio);
            registry.recording.set_pixel_point_fn(move |_, _, _, x, y| {
                let ratio = f64::from(ratio.upgrade()?.load(Ordering::SeqCst));
                Some((x * ratio, y * ratio))
            });
            (registry, output)
        }

        fn point(registry: &crate::tool::ToolRegistry) -> Option<(f64, f64)> {
            // Suppress platform capture so this tests runtime mapping without
            // depending on the process-wide screenshot and accessibility hooks.
            registry
                .recording
                .begin_private_turn(
                    "click",
                    &serde_json::json!({"pid": 1, "window_id": 2, "x": 3, "y": 4}),
                    now_ms(),
                )
                .unwrap()
                .click_point
        }

        let first_state = Arc::new(AtomicU32::new(2));
        let (first, first_output) = registry(&first_state);
        assert_eq!(point(&first), Some((6.0, 8.0)));
        drop(first);
        drop(first_state);
        drop(first_output);

        let replacement_state = Arc::new(AtomicU32::new(3));
        let other_state = Arc::new(AtomicU32::new(5));
        let (replacement, _replacement_output) = registry(&replacement_state);
        let (other, _other_output) = registry(&other_state);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                assert_eq!(point(&replacement), Some((9.0, 12.0)));
                replacement_state.store(7, Ordering::SeqCst);
                assert_eq!(point(&replacement), Some((21.0, 28.0)));
            });
            scope.spawn(|| {
                assert_eq!(point(&other), Some((15.0, 20.0)));
                other_state.store(11, Ordering::SeqCst);
                assert_eq!(point(&other), Some((33.0, 44.0)));
            });
        });
        drop(other);
        drop(other_state);
        assert_eq!(point(&replacement), Some((21.0, 28.0)));
    }

    fn semantic_click_fixture() -> Value {
        serde_json::json!({
            "tool": "click", "result_error": false,
            "arguments": {"pid": 1, "window_id": 2, "element_index": 3},
            "action_truth": {
                "effect": "unverifiable", "transport": "linux_at_spi_action",
                "route": "accessibility", "requested_delivery": "background",
                "actual_delivery": "background", "delivered_count": null,
                "attempts": [], "fallbacks": [], "escalation": null
            }
        })
    }

    fn dispatch_pending(root: &Path) -> PendingTurn {
        PendingTurn {
            generation: 0,
            turn_dir: root.to_path_buf(),
            tool_name: "click".into(),
            args: serde_json::json!({"pid": 901, "window_id": 902, "element_index": 3}),
            start_ms: 0,
            session_start_ms: 0,
            window_id: Some(902),
            pid: Some(901),
            click_point: Some((500.0, 500.0)),
            capture_visual_state: true,
            state_budget: Some(StateCaptureBudget::default()),
            predicted_refusal: None,
            before: TurnCapture {
                screenshot: Some(crate::image_utils::encode_rgba_to_png(&[0; 24], 3, 2).unwrap()),
                state: StateCapture::captured(b"original state".to_vec()),
                screenshot_classification: None,
            },
            dispatch_click: Some(Arc::new(DispatchClickScope {
                window_id: 902,
                pid: 901,
                capture: Mutex::new(DispatchClickCapture::default()),
            })),
        }
    }

    #[tokio::test]
    async fn dispatch_capture_is_lazy_exact_private_and_cancel_safe() {
        let root = tempfile::tempdir().unwrap();
        let pending = dispatch_pending(root.path());
        let forbidden = || -> Option<(Vec<u8>, f64, f64)> { panic!("unexpected capture") };
        capture_dispatch_click_target(902, 901, forbidden);
        scope_dispatch_click_capture(None, async {
            capture_dispatch_click_target(902, 901, forbidden);
        })
        .await;
        scope_dispatch_click_capture(Some(&pending), async {
            capture_dispatch_click_target(903, 901, forbidden);
            capture_dispatch_click_target(902, 903, forbidden);
            scope_dispatch_click_capture(None, async {
                capture_dispatch_click_target(902, 901, forbidden);
            })
            .await;
        })
        .await;

        let session = RecordingSession::new();
        assert!(session.begin_turn("click", &pending.args, 0).is_none());
        {
            let mut inner = session.inner.lock().unwrap();
            inner.enabled = true;
            inner.output_dir = Some(root.path().to_path_buf());
        }
        let private = session
            .begin_private_turn("click", &pending.args, 0)
            .unwrap();
        scope_dispatch_click_capture(Some(&private), async {
            capture_dispatch_click_target(902, 901, forbidden);
        })
        .await;
        assert!(private.dispatch_click.is_none());

        let mut cancelled = Box::pin(scope_dispatch_click_capture(Some(&pending), async {
            capture_dispatch_click_target(902, 901, || None);
            std::future::pending::<()>().await;
        }));
        assert!(matches!(
            std::future::poll_fn(|cx| {
                std::task::Poll::Ready(std::future::Future::poll(cancelled.as_mut(), cx))
            })
            .await,
            std::task::Poll::Pending
        ));
        drop(cancelled);
        capture_dispatch_click_target(902, 901, forbidden);
        assert_eq!(
            pending
                .dispatch_click
                .as_ref()
                .unwrap()
                .capture
                .lock()
                .unwrap()
                .failure,
            Some("capture_failed")
        );
    }

    #[tokio::test]
    async fn concurrent_dispatch_scopes_keep_their_own_images() {
        let root = tempfile::tempdir().unwrap();
        let first = dispatch_pending(root.path());
        let second = dispatch_pending(root.path());
        let capture = |pending: PendingTurn, byte: u8| async move {
            scope_dispatch_click_capture(Some(&pending), async {
                tokio::task::yield_now().await;
                capture_dispatch_click_target(902, 901, || {
                    Some((
                        crate::image_utils::encode_rgba_to_png(&[byte; 16], 2, 2).unwrap(),
                        f64::from(byte % 2),
                        1.0,
                    ))
                });
                tokio::task::yield_now().await;
            })
            .await;
            let scope = pending.dispatch_click.unwrap();
            let retained = scope.capture.lock().unwrap();
            let (png, x, y) = retained.image.as_ref().unwrap();
            assert_eq!(
                *png,
                crate::image_utils::encode_rgba_to_png(&[byte; 16], 2, 2).unwrap()
            );
            assert_eq!((*x, *y), (f64::from(byte % 2), 1.0));
        };
        let (first, second) = tokio::join!(
            tokio::spawn(capture(first, 1)),
            tokio::spawn(capture(second, 2))
        );
        first.unwrap();
        second.unwrap();
    }

    #[tokio::test]
    async fn stale_generation_discards_supplemental_capture() {
        let root = tempfile::tempdir().unwrap();
        let session = RecordingSession::new();
        {
            let mut inner = session.inner.lock().unwrap();
            inner.enabled = true;
            inner.output_dir = Some(root.path().to_path_buf());
        }
        let pending = session
            .begin_turn(
                "click",
                &serde_json::json!({
                    "pid":901, "window_id":902, "element_index":3,
                }),
                0,
            )
            .unwrap();
        scope_dispatch_click_capture(Some(&pending), async {
            capture_dispatch_click_target(902, 901, || {
                Some((
                    crate::image_utils::encode_rgba_to_png(&[255; 16], 2, 2).unwrap(),
                    1.0,
                    1.0,
                ))
            });
        })
        .await;
        session.inner.lock().unwrap().generation += 1;
        session.finish_turn(pending, "discard old generation");
        assert!(!root.path().join("turn-00001/action.json").exists());
        assert!(!root.path().join("turn-00001/click_source.png").exists());
    }

    #[tokio::test]
    async fn supplemental_click_preserves_before_and_rejects_invalid_capture() {
        setup_test_marker();
        for point in [
            Some((1.0, 0.5)),
            None,
            Some((-1.0, 0.0)),
            Some((2.0, 0.0)),
            Some((0.0, 2.0)),
            Some((f64::NAN, 0.0)),
            Some((0.0, f64::INFINITY)),
        ] {
            let root = tempfile::tempdir().unwrap();
            let pending = dispatch_pending(root.path());
            let original = pending.before.screenshot.clone().unwrap();
            let source = crate::image_utils::encode_rgba_to_png(&[255; 16], 2, 2).unwrap();
            write_phase_artifacts(root.path(), "before", &pending.before).unwrap();
            scope_dispatch_click_capture(Some(&pending), async {
                capture_dispatch_click_target(902, 901, || {
                    point.map(|(x, y)| (source.clone(), x, y))
                });
                capture_dispatch_click_target(902, 901, || {
                    panic!("second attempt must be ignored")
                });
            })
            .await;
            write_turn(pending, "clicked", None, false).unwrap();
            let action: Value =
                serde_json::from_slice(&std::fs::read(root.path().join("action.json")).unwrap())
                    .unwrap();
            let evidence: Value =
                serde_json::from_slice(&std::fs::read(root.path().join("evidence.json")).unwrap())
                    .unwrap();
            assert_eq!(
                std::fs::read(root.path().join("before.png")).unwrap(),
                original
            );
            assert_eq!(
                std::fs::read(root.path().join("before_state.json")).unwrap(),
                b"original state"
            );
            assert_eq!(action["click_point_image"], "click_source.png");
            assert_eq!(evidence["click"]["source_image"], "click_source.png");
            if point == Some((1.0, 0.5)) {
                assert_eq!(action["click_point"], serde_json::json!({"x":1.0,"y":0.5}));
                assert_eq!(evidence["click_source"]["status"], "captured");
                assert_eq!(
                    std::fs::read(root.path().join("click_source.png")).unwrap(),
                    source
                );
                assert_eq!(
                    std::fs::read(root.path().join("click.png")).unwrap(),
                    crate::image_utils::crosshair_png_bytes(&source, 1.0, 0.5).unwrap(),
                );
            } else {
                assert!(action.get("click_point").is_none());
                assert_eq!(evidence["click_source"]["status"], "unavailable");
                assert_eq!(evidence["click"]["status"], "unavailable");
                assert!(!root.path().join("click_source.png").exists());
                assert!(!root.path().join("click.png").exists());
            }
        }
        assert!(!point_in_image(b"invalid PNG", 0.0, 0.0));
    }

    #[test]
    fn windows_semantic_exception_requires_matching_uia_attempt() {
        let mut action = semantic_click_fixture();
        action["action_truth"]["transport"] = serde_json::json!("windows_uia_invoke");
        action["action_truth"]["attempts"] =
            serde_json::json!([{"transport":"windows_uia_invoke","delivery":"background"}]);
        assert!(semantic_action_without_point(&action));
        for transport in ["windows_send_input", "linux_at_spi_action", "unknown"] {
            let mut rejected = action.clone();
            rejected["action_truth"]["attempts"][0]["transport"] = serde_json::json!(transport);
            assert!(!semantic_action_without_point(&rejected));
        }
        action["click_point_image"] = serde_json::json!("click_source.png");
        assert!(!semantic_action_without_point(&action));
        action.as_object_mut().unwrap().remove("click_point_image");
        action["action_truth"]["transport"] = serde_json::json!("windows_send_input");
        action["action_truth"]["attempts"][0]["transport"] =
            serde_json::json!("windows_send_input");
        assert!(!semantic_action_without_point(&action));
    }

    #[test]
    fn semantic_click_without_point_requires_exact_action_truth() {
        for transport in [
            "macos_ax_action",
            "linux_at_spi_action",
            "windows_uia_invoke",
            "windows_uia_toggle",
            "windows_uia_selection",
            "windows_uia_expand_collapse",
        ] {
            let mut original = semantic_click_fixture();
            original["action_truth"]["transport"] = serde_json::json!(transport);
            assert!(semantic_action_without_point(&original));
            let mut token = original.clone();
            token["arguments"]
                .as_object_mut()
                .unwrap()
                .remove("element_index");
            token["arguments"]["element_token"] = serde_json::json!("e:fixture");
            token["action_truth"]["requested_delivery"] = serde_json::json!("foreground");
            token["action_truth"]["actual_delivery"] = serde_json::json!("foreground");
            token["action_truth"]["effect"] = serde_json::json!("confirmed");
            assert!(semantic_action_without_point(&token));
            for (pointer, replacements) in [
                ("/result_error", serde_json::json!([true, null])),
                ("/tool", serde_json::json!(["double_click", "right_click"])),
                ("/action_truth", serde_json::json!([null])),
                (
                    "/action_truth/effect",
                    serde_json::json!(["unknown", "partial", "suspected_noop", "refused"]),
                ),
                (
                    "/action_truth/transport",
                    serde_json::json!(["linux_x11_event", "unknown"]),
                ),
                (
                    "/action_truth/route",
                    serde_json::json!(["unknown", "synthetic_events"]),
                ),
                (
                    "/action_truth/actual_delivery",
                    serde_json::json!(["unknown", null]),
                ),
                (
                    "/action_truth/requested_delivery",
                    serde_json::json!(["foreground"]),
                ),
                ("/action_truth/delivered_count", serde_json::json!([0, 2])),
                (
                    "/action_truth/escalation",
                    serde_json::json!([{"kind":"retry_with_pixel_target"}]),
                ),
                ("/action_truth/fallbacks", serde_json::json!([[{}]])),
                (
                    "/action_truth/attempts",
                    serde_json::json!([[{}], [{}, {}]]),
                ),
                ("/arguments/element_index", serde_json::json!([null, -1])),
            ] {
                for replacement in replacements.as_array().unwrap() {
                    let mut action = original.clone();
                    *action.pointer_mut(pointer).unwrap() = replacement.clone();
                    assert!(
                        !semantic_action_without_point(&action),
                        "{pointer}: {action}"
                    );
                }
            }
            for (key, value) in [
                ("x", serde_json::json!(10)),
                ("y", serde_json::json!(20)),
                ("raw", serde_json::json!(true)),
                ("button", serde_json::json!("right")),
                ("count", serde_json::json!(2)),
                ("click_count", serde_json::json!(2)),
                ("modifier", serde_json::json!(["shift"])),
                ("modifiers", serde_json::json!(["ctrl"])),
            ] {
                let mut action = original.clone();
                action["arguments"][key] = value;
                assert!(!semantic_action_without_point(&action), "{key}");
            }
            let mut point = original.clone();
            point["click_point"] = serde_json::json!({"x":10,"y":20});
            assert!(!semantic_action_without_point(&point));
            for key in [
                "actual_delivery",
                "requested_delivery",
                "attempts",
                "fallbacks",
                "escalation",
                "delivered_count",
                "transport",
                "route",
                "effect",
            ] {
                let mut action = original.clone();
                action["action_truth"].as_object_mut().unwrap().remove(key);
                assert!(!semantic_action_without_point(&action), "missing {key}");
            }
        }
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn semantic_turn_records_truth_without_inventing_a_marker() {
        use crate::action_record::{
            ActionEffect, ActionExecutionRecord, ActionTransport, ActualDelivery, RequestedDelivery,
        };
        for (actual, is_error, point, expected) in [
            (
                ActualDelivery::Background,
                false,
                None,
                "semantic_action_without_point",
            ),
            (
                ActualDelivery::Foreground,
                false,
                None,
                "semantic_action_without_point",
            ),
            (ActualDelivery::Unknown, true, None, "capture_failed"),
            (
                ActualDelivery::Background,
                false,
                Some((10.0, 20.0)),
                "capture_failed",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let requested = if actual == ActualDelivery::Foreground {
                RequestedDelivery::Foreground
            } else {
                RequestedDelivery::Background
            };
            let record = ActionExecutionRecord::builder(
                ActionEffect::Unverifiable,
                ActionTransport::LinuxAtSpiAction,
                requested,
            )
            .actual_delivery(actual)
            .build()
            .unwrap();
            let pending = PendingTurn {
                generation: 0,
                turn_dir: root.path().to_path_buf(),
                tool_name: "click".into(),
                args: serde_json::json!({"pid":1,"element_index":3}),
                start_ms: 0,
                session_start_ms: 0,
                window_id: None,
                pid: Some(1),
                click_point: point,
                capture_visual_state: false,
                state_budget: Some(StateCaptureBudget::default()),
                predicted_refusal: None,
                before: TurnCapture::default(),
                dispatch_click: None,
            };
            write_turn(pending, "AT-SPI outcome", Some(&record), is_error).unwrap();
            let action: Value =
                serde_json::from_slice(&std::fs::read(root.path().join("action.json")).unwrap())
                    .unwrap();
            let manifest: Value =
                serde_json::from_slice(&std::fs::read(root.path().join("evidence.json")).unwrap())
                    .unwrap();
            assert_eq!(action["action_truth"], record.debug_json());
            assert_eq!(manifest["click"]["classification"], expected);
            assert_eq!(
                manifest["click"]["status"],
                if expected == "capture_failed" {
                    "unavailable"
                } else {
                    "not_applicable"
                }
            );
            assert!(!root.path().join("click.png").exists());
        }
    }

    #[test]
    fn offscreen_semantic_points_are_absent_but_pixel_clicks_still_need_markers() {
        use crate::action_record::{
            ActionEffect, ActionExecutionRecord, ActionTransport, ActualDelivery, RequestedDelivery,
        };
        let png = crate::image_utils::encode_rgba_to_png(&[0; 16], 2, 2).unwrap();
        for point in [
            (f64::NAN, 0.0),
            (f64::INFINITY, 0.0),
            (-1.0, 0.0),
            (2.0, 0.0),
            (0.0, 2.0),
            (i32::MIN as f64, i32::MAX as f64),
        ] {
            for pixel in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let record = ActionExecutionRecord::builder(
                    ActionEffect::Unverifiable,
                    ActionTransport::LinuxAtSpiAction,
                    RequestedDelivery::Background,
                )
                .actual_delivery(ActualDelivery::Background)
                .build()
                .unwrap();
                let args = if pixel {
                    serde_json::json!({"pid":1,"x":point.0,"y":point.1})
                } else {
                    serde_json::json!({"pid":1,"element_index":3})
                };
                write_turn(
                    PendingTurn {
                        generation: 0,
                        turn_dir: root.path().to_path_buf(),
                        tool_name: "click".into(),
                        args,
                        start_ms: 0,
                        session_start_ms: 0,
                        window_id: None,
                        pid: Some(1),
                        click_point: Some(point),
                        capture_visual_state: false,
                        state_budget: Some(StateCaptureBudget::default()),
                        predicted_refusal: None,
                        before: TurnCapture {
                            screenshot: Some(png.clone()),
                            ..Default::default()
                        },
                        dispatch_click: None,
                    },
                    "AT-SPI outcome",
                    Some(&record),
                    false,
                )
                .unwrap();
                let action: Value = serde_json::from_slice(
                    &std::fs::read(root.path().join("action.json")).unwrap(),
                )
                .unwrap();
                let manifest: Value = serde_json::from_slice(
                    &std::fs::read(root.path().join("evidence.json")).unwrap(),
                )
                .unwrap();
                assert!(action.get("click_point").is_none());
                assert_eq!(
                    manifest["click"]["classification"],
                    if pixel {
                        "capture_failed"
                    } else {
                        "semantic_action_without_point"
                    }
                );
                assert!(!root.path().join("click.png").exists());
            }
        }
    }

    struct FailingVideo;

    fn setup_test_marker() {
        set_click_marker_fn(|png, x, y| {
            Some(
                crate::image_utils::crosshair_png_bytes(png, x, y)
                    .unwrap_or_else(|_| b"click".to_vec()),
            )
        });
    }

    impl VideoBackend for FailingVideo {
        fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
            anyhow::bail!("recorder did not finalize")
        }
    }

    #[test]
    fn turn_capture_brackets_action_and_preserves_post_action_aliases() {
        static SCREENSHOTS: AtomicUsize = AtomicUsize::new(0);
        static STATES: AtomicUsize = AtomicUsize::new(0);
        set_screenshot_fn(|window_id, pid| {
            if (window_id, pid) == (Some(2), Some(1)) {
                let phase = SCREENSHOTS.fetch_add(1, Ordering::SeqCst);
                return Some(if phase == 0 {
                    b"before".to_vec()
                } else {
                    b"after".to_vec()
                });
            }
            // Other recording tests share this process-global hook and may run
            // concurrently. Give them stable bytes without advancing this
            // test's before/after phase counter.
            Some(b"after".to_vec())
        });
        set_ax_snapshot_fn(|_, _| {
            let phase = STATES.fetch_add(1, Ordering::SeqCst);
            Some(format!(r#"{{"phase":{phase}}}"#).into_bytes())
        });
        setup_test_marker();
        set_element_bounds_fn(|pid, args, capture_point| {
            use crate::tool_args::ArgsExt;
            let cache = crate::element_cache::current_runtime_cache::<
                crate::snapshot_test_support::Payload,
            >()?;
            let target = cache
                .resolve_element_args(
                    pid as i32,
                    args.opt_u64("element_index").map(|index| index as usize),
                    args.get("element_token").and_then(Value::as_str),
                    args.get("snapshot_id").and_then(Value::as_str),
                    args.opt_u64("window_id"),
                    "recording",
                )
                .ok()?;
            let (index, window, _) = target.into_parts(None);
            let window = window?;
            Some((
                window,
                capture_point.then_some((window as f64 + index? as f64, pid as f64)),
            ))
        });
        let cache = crate::snapshot_test_support::cache();

        let output_dir = std::env::temp_dir().join(format!(
            "cua-recording-turn-evidence-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let session = RecordingSession::new();
        {
            let mut inner = session.inner.lock().expect("recording lock");
            inner.enabled = true;
            inner.output_dir = Some(output_dir.clone());
            inner.session_start_ms = now_ms();
        }
        let pending = session
            .begin_turn(
                "click",
                &serde_json::json!({"pid": 1, "window_id": 2, "x": 3, "y": 4}),
                now_ms(),
            )
            .expect("recording should reserve a turn");
        let turn = output_dir.join("turn-00001");
        assert_eq!(std::fs::read(turn.join("before.png")).unwrap(), b"before");
        assert!(!turn.join("after.png").exists());

        let action_record = crate::action_record::ActionExecutionRecord::builder(
            crate::action_record::ActionEffect::Unverifiable,
            crate::action_record::ActionTransport::MacosCgEventPid,
            crate::action_record::RequestedDelivery::Background,
        )
        .actual_delivery(crate::action_record::ActualDelivery::Background)
        .build()
        .expect("valid action record");
        session.finish_turn_with_action(pending, "clicked", Some(&action_record));
        assert_eq!(std::fs::read(turn.join("after.png")).unwrap(), b"after");
        assert_eq!(
            std::fs::read(turn.join("screenshot.png")).unwrap(),
            std::fs::read(turn.join("after.png")).unwrap()
        );
        assert_eq!(
            std::fs::read(turn.join("app_state.json")).unwrap(),
            std::fs::read(turn.join("after_state.json")).unwrap()
        );
        assert_eq!(std::fs::read(turn.join("click.png")).unwrap(), b"click");
        let action: Value = serde_json::from_slice(
            &std::fs::read(turn.join("action.json")).expect("read action truth"),
        )
        .expect("parse action truth");
        assert_eq!(action["action_truth"]["effect"], "unverifiable");
        assert_eq!(action["action_truth"]["route"], "synthetic_events");
        assert_eq!(action["action_truth"]["requested_delivery"], "background");

        let snapshot_id = cache.publish(1, 77, crate::snapshot_test_support::Payload(vec![0]));
        let token = crate::element_token::token_for(snapshot_id, 0);
        let pending = session
            .begin_turn(
                "click",
                &serde_json::json!({"pid": 1, "element_token": token}),
                now_ms(),
            )
            .expect("token-only click should reserve a targeted turn");
        session.finish_turn(pending, "token click");
        let token_turn = output_dir.join("turn-00002");
        let token_action: Value = serde_json::from_slice(
            &std::fs::read(token_turn.join("action.json")).expect("read token action"),
        )
        .expect("parse token action");
        assert_eq!(token_action["click_point"]["x"], 77.0);
        assert_eq!(token_action["click_point"]["y"], 1.0);
        assert!(token_turn.join("click.png").exists());

        let stale_snapshot = cache.publish(1, 88, crate::snapshot_test_support::Payload(vec![0]));
        let stale_token = crate::element_token::token_for(stale_snapshot, 0);
        let _newer_snapshot = cache.publish(1, 88, crate::snapshot_test_support::Payload(vec![0]));
        let pending = session
            .begin_turn(
                "click",
                &serde_json::json!({"pid": 1, "element_token": stale_token}),
                now_ms(),
            )
            .expect("stale-token refusal should reserve an evidence turn");
        session.finish_turn_with_outcome(pending, "stale token", None, true);
        let refused_turn = output_dir.join("turn-00003");
        let refused_action: Value = serde_json::from_slice(
            &std::fs::read(refused_turn.join("action.json")).expect("read refused action"),
        )
        .expect("parse refused action");
        assert_eq!(refused_action["result_error"], true);
        assert!(refused_action.get("click_point").is_none());
        assert!(!refused_turn.join("click.png").exists());
        let refused_manifest: Value = serde_json::from_slice(
            &std::fs::read(refused_turn.join("evidence.json")).expect("read refused evidence"),
        )
        .expect("parse refused evidence");
        assert_eq!(refused_manifest["click"]["status"], "not_applicable");
        assert_eq!(
            refused_manifest["click"]["classification"],
            "action_refused_before_target_resolution"
        );

        let pending = session
            .begin_turn(
                "click",
                &serde_json::json!({"pid": 1, "window_id": 2, "x": 3, "y": 4}),
                now_ms(),
            )
            .expect("resolved refusal should reserve an evidence turn");
        let refusal_record = crate::action_record::ActionExecutionRecord::builder(
            crate::action_record::ActionEffect::Refused,
            crate::action_record::ActionTransport::WindowsTargetedInjection,
            crate::action_record::RequestedDelivery::Background,
        )
        .build()
        .expect("valid refusal record");
        session.finish_turn_with_outcome(
            pending,
            "refused before dispatch",
            Some(&refusal_record),
            true,
        );
        let resolved_refusal_turn = output_dir.join("turn-00004");
        let resolved_refusal_action: Value = serde_json::from_slice(
            &std::fs::read(resolved_refusal_turn.join("action.json"))
                .expect("read resolved refusal action"),
        )
        .expect("parse resolved refusal action");
        assert_eq!(resolved_refusal_action["click_point"]["x"], 3.0);
        assert_eq!(resolved_refusal_action["action_truth"]["effect"], "refused");
        assert!(!resolved_refusal_turn.join("click.png").exists());
        let resolved_refusal_manifest: Value = serde_json::from_slice(
            &std::fs::read(resolved_refusal_turn.join("evidence.json"))
                .expect("read resolved refusal evidence"),
        )
        .expect("parse resolved refusal evidence");
        assert_eq!(
            resolved_refusal_manifest["click"]["status"],
            "not_applicable"
        );
        assert_eq!(
            resolved_refusal_manifest["click"]["classification"],
            "action_refused_before_dispatch"
        );
        // A refusal before dispatch cannot change the application, so the
        // expensive after-state walk is skipped and classified, while the
        // cheap after screenshot keeps the visual trajectory continuous.
        assert_eq!(
            resolved_refusal_manifest["after"]["state"],
            serde_json::json!({
                "status": "not_applicable",
                "classification": STATE_SKIPPED_REFUSED,
            })
        );
        assert_eq!(
            resolved_refusal_manifest["before"]["state"]["status"],
            "captured"
        );
        assert!(!resolved_refusal_turn.join("after_state.json").exists());
        assert!(!resolved_refusal_turn.join("app_state.json").exists());
        assert!(resolved_refusal_turn.join("after.png").exists());

        let files = [
            "action.json",
            "app_state.json",
            "screenshot.png",
            "click.png",
            "before_state.json",
            "before.png",
            "after_state.json",
            "after.png",
            "evidence.json",
        ];
        for directory in [&turn, &token_turn] {
            for file in files {
                std::fs::remove_file(directory.join(file)).expect("remove turn fixture file");
            }
            std::fs::remove_dir(directory).expect("remove turn fixture directory");
        }
        for file in files
            .iter()
            .copied()
            .filter(|file| !matches!(*file, "click.png" | "after_state.json" | "app_state.json"))
        {
            std::fs::remove_file(resolved_refusal_turn.join(file))
                .expect("remove resolved refusal fixture file");
        }
        std::fs::remove_dir(&resolved_refusal_turn).expect("remove resolved refusal directory");
        for directory in [&refused_turn] {
            for file in files.iter().copied().filter(|file| *file != "click.png") {
                std::fs::remove_file(directory.join(file))
                    .expect("remove refused turn fixture file");
            }
            std::fs::remove_dir(directory).expect("remove refused turn fixture directory");
        }
        std::fs::remove_dir(&output_dir).expect("remove recording fixture directory");
    }

    #[test]
    fn stale_recording_generation_cannot_finalize_a_reserved_turn() {
        let output_dir = std::env::temp_dir().join(format!(
            "cua-recording-generation-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let session = RecordingSession::new();
        {
            let mut inner = session.inner.lock().expect("recording lock");
            inner.enabled = true;
            inner.generation = 1;
            inner.output_dir = Some(output_dir.clone());
            inner.session_start_ms = now_ms();
        }
        let pending = session
            .begin_turn("click", &serde_json::json!({"x": 1, "y": 2}), now_ms())
            .expect("reserve first generation turn");
        session.inner.lock().unwrap().generation = 2;
        session.finish_turn(pending, "must be discarded");

        let turn = output_dir.join("turn-00001");
        assert!(!turn.join("action.json").exists());
        for entry in std::fs::read_dir(&turn).expect("read partial turn") {
            std::fs::remove_file(entry.expect("turn entry").path()).expect("remove partial file");
        }
        std::fs::remove_dir(&turn).expect("remove partial turn");
        std::fs::remove_dir(&output_dir).expect("remove recording directory");
    }

    #[test]
    fn state_status_reports_partial_walks_and_classified_skips() {
        let partial = StateCapture::captured(
            serde_json::to_vec(&serde_json::json!({
                "tree_markdown": "- AXWindow",
                "truncated": true,
                "truncation_reason": "timeout",
                "nodes_visited": 3,
                "nodes_pending": 9,
                "walk_elapsed_ms": 1004,
                "timeout_ms": 1000,
            }))
            .unwrap(),
        );
        assert_eq!(
            state_status(&partial, true),
            serde_json::json!({
                "status": "captured",
                "truncated": true,
                "truncation_reason": "timeout",
                "nodes_visited": 3,
                "nodes_pending": 9,
                "walk_elapsed_ms": 1004,
                "timeout_ms": 1000,
            })
        );
        // Payloads without walk fields (older hooks) keep the plain status.
        assert_eq!(
            state_status(&StateCapture::captured(b"{}".to_vec()), true),
            serde_json::json!({"status": "captured"})
        );
        assert_eq!(
            state_status(&StateCapture::skipped(STATE_SKIPPED_REFUSED), true),
            serde_json::json!({
                "status": "not_applicable",
                "classification": "action_refused_before_dispatch",
            })
        );
        assert_eq!(
            state_status(&StateCapture::unavailable("state_capture_timeout"), true),
            serde_json::json!({
                "status": "unavailable",
                "classification": "state_capture_timeout",
            })
        );
        // Without a target pid nothing was expected, whatever the plan said.
        assert_eq!(
            state_status(&StateCapture::skipped(STATE_SKIPPED_REFUSED), false),
            serde_json::json!({"status": "not_applicable", "classification": "no_target_pid"})
        );
    }

    #[test]
    fn state_hook_backstop_releases_a_hung_provider_and_recovers() {
        // A distinct pid keeps this test's abandoned-walk accounting apart
        // from other tests sharing the process-global table.
        const PID: i64 = 990_001;
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let hung: AxSnapshotFn = Arc::new(move |_, _, _| {
            let _ = gate.lock().unwrap().recv();
            Some(b"{}".to_vec())
        });
        let budget = StateCaptureBudget { timeout_ms: 50 };
        let started = Instant::now();
        let timed_out = run_state_hook(hung.clone(), None, Some(PID), budget);
        assert!(started.elapsed() < budget.backstop() + Duration::from_millis(500));
        assert_eq!(timed_out.classification, Some("state_capture_timeout"));
        assert!(timed_out.bytes.is_none());

        let calls = Arc::new(AtomicUsize::new(0));
        let counting_calls = calls.clone();
        let fast: AxSnapshotFn = Arc::new(move |_, _, budget| {
            counting_calls.fetch_add(1, Ordering::SeqCst);
            Some(format!(r#"{{"timeout_ms":{}}}"#, budget.timeout_ms).into_bytes())
        });
        let busy = run_state_hook(fast.clone(), None, Some(PID), budget);
        assert_eq!(busy.classification, Some("state_capture_busy"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no second walk of a hung app"
        );
        // Another application is not blocked by the hung one.
        let other = run_state_hook(fast.clone(), None, Some(PID + 1), budget);
        assert!(other.bytes.is_some());

        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while abandoned_state_captures().contains_key(&PID) {
            assert!(Instant::now() < deadline, "abandoned walk never released");
            std::thread::sleep(Duration::from_millis(10));
        }
        let recovered = run_state_hook(fast, None, Some(PID), budget);
        assert_eq!(
            recovered.walk.unwrap()["timeout_ms"],
            serde_json::json!(50),
            "the hook receives the configured budget"
        );
    }

    #[test]
    fn predicted_refusal_skips_state_and_records_a_missed_prediction() {
        let refusal = crate::action_record::ActionExecutionRecord::builder(
            crate::action_record::ActionEffect::Refused,
            crate::action_record::ActionTransport::MacosCgEventPid,
            crate::action_record::RequestedDelivery::Background,
        )
        .build()
        .unwrap();
        let delivered = crate::action_record::ActionExecutionRecord::builder(
            crate::action_record::ActionEffect::Unverifiable,
            crate::action_record::ActionTransport::MacosCgEventPid,
            crate::action_record::RequestedDelivery::Background,
        )
        .actual_delivery(crate::action_record::ActualDelivery::Background)
        .build()
        .unwrap();
        let pending = |root: &Path| PendingTurn {
            generation: 0,
            turn_dir: root.to_path_buf(),
            tool_name: "click".into(),
            args: serde_json::json!({"pid": 1, "window_id": 2, "x": 0, "y": 0}),
            start_ms: 0,
            session_start_ms: 0,
            window_id: Some(2),
            pid: Some(1),
            click_point: None,
            capture_visual_state: true,
            state_budget: Some(StateCaptureBudget::default()),
            predicted_refusal: Some("capture_expired"),
            before: TurnCapture {
                state: StateCapture::skipped(STATE_SKIPPED_REFUSED),
                ..Default::default()
            },
            dispatch_click: None,
        };
        let manifest = |root: &Path| -> Value {
            serde_json::from_slice(&std::fs::read(root.join("evidence.json")).unwrap()).unwrap()
        };

        let refused = tempfile::tempdir().unwrap();
        let turn = pending(refused.path());
        let after = capture_after(&turn, Some(&refusal), true);
        assert_eq!(after.state.classification, Some(STATE_SKIPPED_REFUSED));
        write_turn_with_after(turn, after, "refused", Some(&refusal), true).unwrap();
        let evidence = manifest(refused.path());
        assert_eq!(evidence["before"]["state"]["status"], "not_applicable");
        assert_eq!(evidence["after"]["state"]["status"], "not_applicable");

        // The adapter is the authority: if it dispatched anyway, the skipped
        // before-state must not be presented as a refusal.
        let missed = tempfile::tempdir().unwrap();
        write_turn_with_after(
            pending(missed.path()),
            TurnCapture::default(),
            "clicked",
            Some(&delivered),
            false,
        )
        .unwrap();
        assert_eq!(
            manifest(missed.path())["before"]["state"],
            serde_json::json!({
                "status": "unavailable",
                "classification": STATE_SKIPPED_PREDICTION_MISSED,
            })
        );
    }

    #[test]
    fn private_turn_records_metadata_without_visual_or_ax_artifacts() {
        let output_dir = std::env::temp_dir().join(format!(
            "cua-recording-private-turn-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let session = RecordingSession::new();
        {
            let mut inner = session.inner.lock().expect("recording lock");
            inner.enabled = true;
            inner.output_dir = Some(output_dir.clone());
            inner.session_start_ms = now_ms();
        }
        let pending = session
            .begin_private_turn(
                "browser_prepare",
                &serde_json::json!({"pid": 1, "window_id": 2}),
                now_ms(),
            )
            .expect("reserve private consent turn");
        session.finish_turn(pending, "attached");

        let turn = output_dir.join("turn-00001");
        assert!(turn.join("action.json").exists());
        assert!(turn.join("evidence.json").exists());
        for private_artifact in [
            "before.png",
            "after.png",
            "screenshot.png",
            "before_state.json",
            "after_state.json",
            "app_state.json",
        ] {
            assert!(!turn.join(private_artifact).exists(), "{private_artifact}");
        }
        let evidence: Value = serde_json::from_slice(
            &std::fs::read(turn.join("evidence.json")).expect("read private evidence"),
        )
        .expect("parse private evidence");
        assert_eq!(
            evidence["before"]["screenshot"]["classification"],
            "privacy_suppressed"
        );
        assert_eq!(
            evidence["after"]["screenshot"]["classification"],
            "privacy_suppressed"
        );

        for file in ["action.json", "evidence.json"] {
            std::fs::remove_file(turn.join(file)).expect("remove private turn artifact");
        }
        std::fs::remove_dir(&turn).expect("remove private turn directory");
        std::fs::remove_dir(&output_dir).expect("remove private recording directory");
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

    #[test]
    fn video_metadata_requires_finalized_nonempty_output() {
        let output_dir = std::env::temp_dir().join(format!(
            "cua-recording-metadata-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&output_dir).expect("create video metadata test directory");
        let path = output_dir.join("recording.mp4");
        std::fs::write(&path, b"video").expect("write video fixture");

        let error = validate_video_metadata(VideoMetadata {
            path: path.clone(),
            duration_ms: 1,
            finalized: false,
        })
        .expect_err("unfinalized output must fail");
        assert!(error.to_string().contains("did not finalize"));

        std::fs::write(&path, []).expect("empty video fixture");
        let error = validate_video_metadata(VideoMetadata {
            path: path.clone(),
            duration_ms: 1,
            finalized: true,
        })
        .expect_err("empty finalized output must fail");
        assert!(error.to_string().contains("is empty"));

        std::fs::write(&path, b"video").expect("restore video fixture");
        validate_video_metadata(VideoMetadata {
            path,
            duration_ms: 1,
            finalized: true,
        })
        .expect("finalized nonempty output must pass");
        let _ = std::fs::remove_dir_all(output_dir);
    }
}
