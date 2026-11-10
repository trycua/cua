// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
// Installed during tool registration by each platform crate. Takes (window_id, pid)
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

/// A state walk in flight for one target pid. At most one exists per pid, so
/// a hung accessibility provider cannot accumulate threads and two turns
/// cannot walk the same application at once.
#[derive(Debug, Clone, Copy)]
enum Worker {
    /// The turn that started the walk is still waiting for it.
    Running,
    /// The turn gave up at its backstop; the thread is left to finish.
    Abandoned { since: Instant },
}

/// Per-pid state workers. Every check, insert, transition and removal holds
/// this lock, which guards nothing else.
static STATE_CAPTURE_WORKERS: OnceLock<Mutex<HashMap<i64, Worker>>> = OnceLock::new();

fn state_capture_workers() -> std::sync::MutexGuard<'static, HashMap<i64, Worker>> {
    STATE_CAPTURE_WORKERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    use std::collections::hash_map::Entry;
    let key = pid.unwrap_or(-1);
    let busy = match state_capture_workers().entry(key) {
        Entry::Occupied(worker) => Some(*worker.get()),
        Entry::Vacant(slot) => {
            slot.insert(Worker::Running);
            None
        }
    };
    if let Some(worker) = busy {
        if let Worker::Abandoned { since } = worker {
            tracing::debug!(
                target: "recording",
                "application-state capture for pid {key} skipped; a walk abandoned {} ms ago \
                 is still running",
                since.elapsed().as_millis()
            );
        }
        return StateCapture::unavailable("state_capture_busy");
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("cua-recording-state".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                hook(window_id, pid, budget)
            }))
            .unwrap_or(None);
            let mut workers = state_capture_workers();
            if let Some(Worker::Abandoned { .. }) = workers.get(&key) {
                workers.remove(&key);
            } else {
                let _ = sender.send(result);
            }
        });
    if spawned.is_err() {
        state_capture_workers().remove(&key);
        return StateCapture::unavailable("capture_failed");
    }
    let received = match receiver.recv_timeout(budget.backstop()) {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let mut workers = state_capture_workers();
            // The worker sends under this lock, so a result is either here
            // now or the worker will see `Abandoned` and clean up itself.
            match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(_) => {
                    workers.insert(
                        key,
                        Worker::Abandoned {
                            since: Instant::now(),
                        },
                    );
                    drop(workers);
                    tracing::warn!(
                        target: "recording",
                        "application-state capture for pid {key} exceeded its {} ms budget; \
                         recording the turn without state",
                        budget.timeout_ms
                    );
                    return StateCapture::unavailable("state_capture_timeout");
                }
            }
        }
    };
    state_capture_workers().remove(&key);
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
/// Classification for a turn whose pid has no window to photograph or walk.
pub const TARGET_UNAVAILABLE: &str = "target_unavailable";
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
    /// Caller-supplied secret values scrubbed from this turn's text files.
    redactions: Redactions,
}

/// Secret values given to `start_recording` that must never reach the
/// recording's files. They live only in memory for the recording's lifetime
/// and are replaced with [`REDACTED`] in every text artifact of a turn
/// (`action.json`, `before_state.json`, `after_state.json`, `app_state.json`).
/// Screenshots and video are not altered.
#[derive(Clone, Default)]
pub struct Redactions(Arc<Vec<String>>);

/// Placeholder written in place of a redacted value.
pub const REDACTED: &str = "[redacted]";

impl Redactions {
    pub fn new(values: impl IntoIterator<Item = String>) -> Self {
        let mut values: Vec<String> = values.into_iter().filter(|v| !v.is_empty()).collect();
        // Longest first, so a value that contains another is replaced whole.
        values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        values.dedup();
        Self(Arc::new(values))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn scrub_str(&self, text: &str) -> Option<String> {
        let mut out: Option<String> = None;
        for value in self.0.iter() {
            let current = out.as_deref().unwrap_or(text);
            if current.contains(value.as_str()) {
                out = Some(current.replace(value.as_str(), REDACTED));
            }
        }
        out
    }

    /// Replace every occurrence in the string values and object keys of
    /// `value`. Returns whether anything was replaced.
    pub fn scrub_json(&self, value: &mut Value) -> bool {
        if self.is_empty() {
            return false;
        }
        match value {
            Value::String(text) => match self.scrub_str(text) {
                Some(scrubbed) => {
                    *text = scrubbed;
                    true
                }
                None => false,
            },
            Value::Array(items) => items
                .iter_mut()
                .fold(false, |hit, item| self.scrub_json(item) | hit),
            Value::Object(map) => {
                let mut hit = false;
                let keys: Vec<String> = map.keys().cloned().collect();
                for key in keys {
                    let mut child = map.remove(&key).unwrap_or(Value::Null);
                    hit |= self.scrub_json(&mut child);
                    let key = match self.scrub_str(&key) {
                        Some(scrubbed) => {
                            hit = true;
                            scrubbed
                        }
                        None => key,
                    };
                    map.insert(key, child);
                }
                hit
            }
            _ => false,
        }
    }

    /// Scrub a captured state file. JSON is scrubbed structurally so escaped
    /// characters in a value cannot hide it; anything else is scrubbed as text.
    fn scrub_bytes(&self, bytes: &mut Vec<u8>) -> bool {
        if self.is_empty() {
            return false;
        }
        if let Ok(mut json) = serde_json::from_slice::<Value>(bytes) {
            if self.scrub_json(&mut json) {
                if let Ok(out) = serde_json::to_vec_pretty(&json) {
                    *bytes = out;
                    return true;
                }
            }
            return false;
        }
        let text = String::from_utf8_lossy(bytes);
        match self.scrub_str(&text) {
            Some(scrubbed) => {
                *bytes = scrubbed.into_bytes();
                true
            }
            None => false,
        }
    }
}

/// Trusted identity of the call that may reserve a turn.
///
/// `session` is the call's runtime session (its explicit `session` label, or
/// the transport lease when it has none) and `transport` is the connection
/// that carried it. A recording owned by a session keeps calls from that
/// session. A recording started without a label is owned by its transport's
/// lease, so it also keeps the same connection's named-session calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordingCaller<'a> {
    pub session: Option<&'a str>,
    pub transport: Option<&'a str>,
}

impl RecordingCaller<'_> {
    fn within(self, owner: &str) -> bool {
        self.session == Some(owner) || self.transport == Some(owner)
    }
}

/// Which recording a stop applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBy<'a> {
    /// Manual stop and runtime shutdown: whatever recording is live.
    Anyone,
    /// Session end: only a recording this session owns. An anonymous
    /// recording is owned by nobody, so only [`StopBy::Anyone`] reaches it.
    Owner(&'a str),
    /// Idle TTL: only the generation the caller read from
    /// [`RecordingState::generation`], never a recording started after it.
    Generation(u64),
}

/// Persistent recording session state owned by one tool registry.
pub struct RecordingSession {
    inner: Mutex<RecordingInner>,
    pixel_point_fn: OnceLock<PixelPointFnBox>,
}

struct RecordingInner {
    /// Bumped by every start; a turn or idle stop names the one it read.
    generation: u64,
    recorder: Recorder,
    /// Per-turn application-state budget, or `None` when the caller disabled
    /// state capture with `include_accessibility_tree: false`.
    state_budget: Option<StateCaptureBudget>,
    last_error: Option<String>,
}

enum Recorder {
    /// `last_video` is the finalized video of the previous recording, if any.
    Idle {
        last_video: Option<VideoMetadata>,
    },
    Live(Live),
}

struct Live {
    /// Session that owns the recording, from the daemon-injected
    /// `_session_id`; `None` for an anonymous (CLI one-shot) start. The
    /// recorder is shared, so the latest start owns it and an earlier owner's
    /// session end no-ops in [`RecordingSession::stop`].
    owner: Option<String>,
    dir: PathBuf,
    next_turn: u32,
    started_at_ms: u64,
    /// SCKit on macOS, an ffmpeg subprocess elsewhere.
    video: Option<Box<dyn VideoBackend>>,
    cursor: Option<CursorSampler>,
    /// Secret values scrubbed from every turn of this recording.
    redactions: Redactions,
}

impl RecordingInner {
    fn is_live(&self, generation: u64) -> bool {
        matches!(self.recorder, Recorder::Live(_)) && self.generation == generation
    }

    /// Stop the live recording's media and rewrite its `session.json` with
    /// what landed. Leaves the recorder idle; a no-op when already idle.
    fn finalize(&mut self) -> anyhow::Result<()> {
        let idle = Recorder::Idle { last_video: None };
        let Recorder::Live(live) = std::mem::replace(&mut self.recorder, idle) else {
            return Ok(());
        };
        let (video_meta, stop_error) = match live.video.map(|rec| rec.stop()) {
            Some(Ok(meta)) => match validate_video_metadata(meta) {
                Ok(meta) => (Some(meta), None),
                Err(error) => (None, Some(error.to_string())),
            },
            Some(Err(error)) => (None, Some(error.to_string())),
            None => (None, None),
        };
        let cursor_samples = live.cursor.map(|c| c.stop()).unwrap_or(0);
        if let Some(error) = &stop_error {
            self.last_error = Some(error.clone());
        } else if video_meta.is_some() {
            self.last_error = None;
        }
        let video_block = match &video_meta {
            Some(meta) => video_session_payload(true, None, Some(meta)),
            None => video_session_payload(false, self.last_error.as_deref(), None),
        };
        let session_payload = serde_json::json!({
            "schema_version": 1,
            "started_at_monotonic_ms": live.started_at_ms,
            "video": video_block,
            "cursor": { "present": cursor_samples > 0, "sample_count": cursor_samples }
        });
        let _ = write_json_atomic(&live.dir.join("session.json"), &session_payload);
        self.recorder = Recorder::Idle {
            last_video: video_meta,
        };
        match stop_error {
            Some(error) => anyhow::bail!("video finalization failed: {error}"),
            None => Ok(()),
        }
    }
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
    /// Session id that owns the live recording, stamped on `start()` from the
    /// daemon-injected `_session_id`. `None` for an anonymous recording (CLI
    /// one-shot) or when idle.
    pub owner: Option<String>,
    /// Generation of the live recording, for [`StopBy::Generation`].
    pub generation: Option<u64>,
}

impl RecordingSession {
    pub fn new() -> Self {
        Self {
            pixel_point_fn: OnceLock::new(),
            inner: Mutex::new(RecordingInner {
                generation: 0,
                recorder: Recorder::Idle { last_video: None },
                state_budget: Some(StateCaptureBudget::default()),
                last_error: None,
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
    /// Counterpart to [`Self::stop`]. Returns the resulting state.
    ///
    /// `record_video=true` spawns ffmpeg writing `<output_dir>/recording.mp4`
    /// for the lifetime of the session. NOTE: the MCP `start_recording` tool
    /// now defaults `record_video` to *false* (opt-in) — see
    /// `recording_tools.rs` — so video only records when explicitly requested.
    /// If ffmpeg isn't on PATH the start still succeeds —
    /// the per-turn capture (action.json + pre/post evidence) is independent
    /// of video — but the structured state carries the ffmpeg error so
    /// the caller can surface it.
    ///
    /// `owner` stamps the session that owns this recording (the daemon-injected
    /// `_session_id`). `None` marks an anonymous start (CLI one-shot) owned by nobody. See [`StopBy`] for how this
    /// gates teardown. A start over a live recording finalizes that recording
    /// first, as [`Self::stop`] does.
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
        self.start_with_redactions(
            output_dir,
            record_video,
            owner,
            state_budget,
            Redactions::default(),
        )
    }

    /// [`Self::start_with_state_budget`] plus secret values that are scrubbed
    /// from every turn's text files. The values are held in memory only and
    /// cleared when the recording stops or restarts.
    pub fn start_with_redactions(
        &self,
        output_dir: &str,
        record_video: bool,
        owner: Option<&str>,
        state_budget: Option<StateCaptureBudget>,
        redactions: Redactions,
    ) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        // Write-boundary resurrection guard — checked INSIDE the lock so the
        // is_session_ended test is atomic with the recorder write below.
        // An in-flight start_recording that lands after its owning session ended
        // (passed the dispatch gate, then the proxy died) must not create a
        // recording owned by a dead session — a leaked ffmpeg/SCStream. The
        // teardown sites call `fire_session_end` (which marks ENDED_SESSIONS)
        // BEFORE `stop(StopBy::Owner)`, so either the mark is already set and we bail
        // here, or we win the lock first and the reaper's later stop
        // reaps what we started. Anonymous starts (owner = None: CLI one-shot) are never gated.
        if let Some(o) = owner {
            if crate::session::is_session_ended(o) {
                anyhow::bail!(
                    "session {o} has ended; refusing to start a recording owned by a dead session"
                );
            }
        }
        let dir = expand_tilde(output_dir);
        // Create the new directory while the live recording still runs, so a
        // failure leaves it untouched.
        std::fs::create_dir_all(&dir)?;
        // Finalize the recording this start replaces; its error stays in
        // `last_error` and does not fail the new start.
        let _ = inner.finalize();

        // Single monotonic anchor shared by video, cursor sampler, and
        // per-turn `t_ms_from_session_start` math in `finish_turn()` — so all
        // three timelines line up at the millisecond.
        let monotonic_start = Instant::now();

        let mut video = None;
        let mut video_error: Option<String> = None;
        if record_video {
            let path = dir.join("recording.mp4");
            match video::start_video(&path) {
                Ok(rec) => video = Some(rec),
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
        let cursor = CursorSampler::start(cursor_path, monotonic_start)
            .inspect_err(
                |e| tracing::warn!(target: "recording", "Cursor sampler failed to start: {e}"),
            )
            .ok();

        // Write initial session.json — final video metadata is rewritten on
        // stop. We mark `present` based on whether ffmpeg actually started,
        // not just whether the caller asked for video.
        let started_at_ms = now_ms();
        let session_payload = serde_json::json!({
            "schema_version": 1,
            "started_at_monotonic_ms": started_at_ms,
            "video": video_session_payload(video.is_some(), video_error.as_deref(), None),
            "cursor": { "present": cursor.is_some(), "sample_count": 0 }
        });
        let _ = write_json_atomic(&dir.join("session.json"), &session_payload);

        inner.generation = inner.generation.wrapping_add(1);
        inner.state_budget = state_budget;
        inner.last_error = video_error;
        inner.recorder = Recorder::Live(Live {
            owner: owner.map(str::to_owned),
            dir,
            next_turn: 1,
            started_at_ms,
            video,
            cursor,
            redactions,
        });
        Ok(())
    }

    /// Finalize the recording `by` selects: stop its video and cursor sampler
    /// and rewrite `session.json` with what landed. A no-op when idle or when
    /// `by` selects a recording other than the live one, so a stale session
    /// end or idle check never stops a newer recording. The check runs under
    /// the lock, so it is race-free against a concurrent `start()`.
    pub fn stop(&self, by: StopBy<'_>) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let Recorder::Live(live) = &inner.recorder else {
            return Ok(());
        };
        let selected = match by {
            StopBy::Anyone => true,
            StopBy::Owner(session) => live.owner.as_deref() == Some(session),
            StopBy::Generation(generation) => inner.generation == generation,
        };
        if !selected {
            return Ok(());
        }
        inner.finalize()
    }

    /// Return a snapshot of the current state (non-blocking).
    pub fn current_state(&self) -> RecordingState {
        let inner = self.inner.lock().unwrap();
        let (live, last_video) = match &inner.recorder {
            Recorder::Live(live) => (Some(live), None),
            Recorder::Idle { last_video } => (None, last_video.as_ref()),
        };
        RecordingState {
            enabled: live.is_some(),
            output_dir: live.map(|l| l.dir.to_string_lossy().into_owned()),
            next_turn: live.map_or(1, |l| l.next_turn),
            last_error: inner.last_error.clone(),
            video_active: live.is_some_and(|l| l.video.is_some()),
            last_video_path: last_video.map(|m| m.path.to_string_lossy().into_owned()),
            owner: live.and_then(|l| l.owner.clone()),
            generation: live.map(|_| inner.generation),
        }
    }

    /// Per-turn application-state budget of the current (or next) recording;
    /// `None` when state capture is disabled.
    pub fn state_budget(&self) -> Option<StateCaptureBudget> {
        self.inner.lock().unwrap().state_budget
    }

    /// Reserve a turn and capture its target immediately before tool dispatch.
    /// No-op when disabled or when `caller` is outside the recording's scope.
    pub fn begin_turn(
        &self,
        tool_name: &str,
        args: &Value,
        start_ms: u64,
        caller: RecordingCaller<'_>,
    ) -> Option<PendingTurn> {
        self.begin_turn_with_capture(tool_name, args, start_ms, caller, true, None)
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
        caller: RecordingCaller<'_>,
        predicted_refusal: Option<&'static str>,
    ) -> Option<PendingTurn> {
        self.begin_turn_with_capture(tool_name, args, start_ms, caller, true, predicted_refusal)
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
        caller: RecordingCaller<'_>,
    ) -> Option<PendingTurn> {
        self.begin_turn_with_capture(tool_name, args, start_ms, caller, false, None)
    }

    /// Scope-check and reserve under one lock; ownerless recordings accept all callers.
    fn begin_turn_with_capture(
        &self,
        tool_name: &str,
        args: &Value,
        start_ms: u64,
        caller: RecordingCaller<'_>,
        capture_visual_state: bool,
        predicted_refusal: Option<&'static str>,
    ) -> Option<PendingTurn> {
        let (turn_dir, session_start_ms, generation, state_budget, redactions) = {
            let mut inner = self.inner.lock().unwrap();
            let (generation, state_budget) = (inner.generation, inner.state_budget);
            let Recorder::Live(live) = &mut inner.recorder else {
                return None;
            };
            if let Some(owner) = live.owner.as_deref() {
                if !caller.within(owner) {
                    return None;
                }
            }
            let idx = live.next_turn;
            live.next_turn += 1;
            (
                live.dir.join(format!("turn-{idx:05}")),
                live.started_at_ms,
                generation,
                state_budget,
                live.redactions.clone(),
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
        let mut before = if capture_visual_state {
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
        if !inner.is_live(generation) {
            return None;
        }
        if let Some(state) = before.state.bytes.as_mut() {
            redactions.scrub_bytes(state);
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
            redactions,
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
            let live = inner.is_live(pending.generation);
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
    // The platform found no window for the pid, so there is no tree to walk
    // either. Say so instead of letting the walk fail as `capture_failed`.
    let no_target_window = window_id.is_none()
        && pid.is_some()
        && screenshot.classification == Some(TARGET_UNAVAILABLE);
    TurnCapture {
        state: match state {
            // No target application: there is no state to walk.
            _ if pid.is_none() => StateCapture::default(),
            _ if no_target_window => StateCapture::skipped(TARGET_UNAVAILABLE),
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

/// How a turn's click marker is expected and whether it was captured.
struct ClickEvidence {
    expected: bool,
    captured: bool,
    /// Recorded when no click marker applies to the turn.
    not_applicable_classification: &'static str,
}

fn write_evidence_manifest(
    turn_dir: &Path,
    before: &TurnCapture,
    after: &TurnCapture,
    state_expected: bool,
    click: ClickEvidence,
    supplemental: &DispatchClickCapture,
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
        "click": if click.expected {
            capture_status(click.captured, true, None)
        } else {
            serde_json::json!({
                "status": "not_applicable",
                "classification": click.not_applicable_classification,
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
        && args["element_token"]
            .as_str()
            .is_some_and(|token| !token.is_empty())
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
    mut after: TurnCapture,
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
        redactions,
    } = pending;
    if let Some(state) = after.state.bytes.as_mut() {
        redactions.scrub_bytes(state);
    }
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
        "timestamp": iso8601_utc_from_unix_ms(now),
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
    // The result summary and action truth can echo typed text back, so the
    // whole file is scrubbed. `redacted` marks a turn whose arguments lost a
    // value, which replay cannot re-invoke as recorded.
    let arguments_redacted = redactions.scrub_json(&mut payload["arguments"]);
    redactions.scrub_json(&mut payload);
    if arguments_redacted {
        payload["redacted"] = Value::Bool(true);
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
        ClickEvidence {
            expected: click_expected,
            captured: click_captured,
            not_applicable_classification: if refused_before_target_resolution {
                "action_refused_before_target_resolution"
            } else if refused_before_dispatch {
                "action_refused_before_dispatch"
            } else if semantic_without_point {
                "semantic_action_without_point"
            } else {
                "not_a_click_action"
            },
        },
        &supplemental,
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

/// Format milliseconds since Unix epoch as an ISO-8601 UTC date-time with
/// millisecond precision, e.g. `2026-01-01T00:00:00.000Z`.
fn iso8601_utc_from_unix_ms(ms: u64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
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

    fn live_at(dir: &Path) -> Live {
        Live {
            owner: None,
            dir: dir.to_path_buf(),
            next_turn: 1,
            started_at_ms: now_ms(),
            video: None,
            cursor: None,
            redactions: Redactions::default(),
        }
    }

    #[test]
    fn pixel_mappers_follow_registry_lifetimes_and_concurrent_runtime_state() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        fn registry(ratio: &Arc<AtomicU32>) -> (crate::tool::ToolRegistry, tempfile::TempDir) {
            let registry = crate::tool::ToolRegistry::new();
            let output = tempfile::tempdir().unwrap();
            {
                let mut inner = registry.recording.inner.lock().unwrap();
                inner.recorder = Recorder::Live(live_at(output.path()));
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
                    RecordingCaller::default(),
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
            "arguments": {"pid": 1, "window_id": 2, "element_token": "s00000001:3"},
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
            args: serde_json::json!({"pid": 901, "window_id": 902, "element_token": "s00000001:3"}),
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
            redactions: Redactions::default(),
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
        assert!(session
            .begin_turn("click", &pending.args, 0, RecordingCaller::default())
            .is_none());
        {
            let mut inner = session.inner.lock().unwrap();
            inner.recorder = Recorder::Live(live_at(root.path()));
        }
        let private = session
            .begin_private_turn("click", &pending.args, 0, RecordingCaller::default())
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
            inner.recorder = Recorder::Live(live_at(root.path()));
        }
        let pending = session
            .begin_turn(
                "click",
                &serde_json::json!({
                    "pid":901, "window_id":902, "element_token": "s00000001:3",
                }),
                0,
                RecordingCaller::default(),
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
                ("/arguments/element_token", serde_json::json!([null, ""])),
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
                args: serde_json::json!({"pid":1,"element_token": "s00000001:3"}),
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
                redactions: Redactions::default(),
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
                    serde_json::json!({"pid":1,"element_token": "s00000001:3"})
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
                        redactions: Redactions::default(),
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
            let cache = crate::snapshot_store::current_runtime_store::<
                crate::snapshot_test_support::Payload,
            >()?;
            let target = cache.resolve(pid as i32, args).ok()?;
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
            inner.recorder = Recorder::Live(live_at(&output_dir));
        }
        let pending = session
            .begin_turn(
                "click",
                &serde_json::json!({"pid": 1, "window_id": 2, "x": 3, "y": 4}),
                now_ms(),
                RecordingCaller::default(),
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
                RecordingCaller::default(),
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
                RecordingCaller::default(),
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
                RecordingCaller::default(),
            )
            .expect("resolved refusal should reserve an evidence turn");
        // Derive the record from the real shared stale-zoom refusal so the
        // recorder is proven to treat it as refused-before-dispatch (no
        // click marker), not as an unverifiable delivered click.
        let zoom_refusal = cache
            .zoom(1, Some(2), None)
            .expect_err("no zoom context was published")
            .structured_content
            .expect("zoom refusal is structured");
        let refusal_record = crate::action_record::ActionExecutionRecord::from_legacy(
            "click",
            &serde_json::json!({"pid": 1, "window_id": 2, "x": 3, "y": 4, "from_zoom": true}),
            &zoom_refusal,
        )
        .expect("click refusal normalizes to an action record");
        assert_eq!(
            refusal_record.effect,
            crate::action_record::ActionEffect::Refused
        );
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
            inner.generation = 1;
            inner.recorder = Recorder::Live(live_at(&output_dir));
        }
        let pending = session
            .begin_turn(
                "click",
                &serde_json::json!({"x": 1, "y": 2}),
                now_ms(),
                RecordingCaller::default(),
            )
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
        while state_capture_workers().contains_key(&PID) {
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
    fn state_hook_admits_one_running_walk_per_pid() {
        const PID: i64 = 990_101;
        let (entered_tx, entered) = std::sync::mpsc::channel::<()>();
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let entered_tx = Mutex::new(entered_tx);
        let gate = Mutex::new(gate);
        let slow: AxSnapshotFn = Arc::new(move |_, _, _| {
            let _ = entered_tx.lock().unwrap().send(());
            let _ = gate.lock().unwrap().recv();
            Some(b"{}".to_vec())
        });
        let budget = StateCaptureBudget { timeout_ms: 10_000 };
        let first = std::thread::spawn(move || run_state_hook(slow, None, Some(PID), budget));
        entered.recv().unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let counting_calls = calls.clone();
        let fast: AxSnapshotFn = Arc::new(move |_, _, _| {
            counting_calls.fetch_add(1, Ordering::SeqCst);
            Some(b"{}".to_vec())
        });
        let second = run_state_hook(fast.clone(), None, Some(PID), budget);
        assert_eq!(second.classification, Some("state_capture_busy"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "one walk per app at a time"
        );

        release.send(()).unwrap();
        assert!(first.join().unwrap().bytes.is_some());
        let after = run_state_hook(fast, None, Some(PID), budget);
        assert!(after.bytes.is_some(), "a finished walk frees its pid");
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
            redactions: Redactions::default(),
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

    fn turn_files_contain_no_secret(root: &Path, secrets: &[&str]) {
        let mut stack = vec![root.to_path_buf()];
        let mut files = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                files += 1;
                let bytes = std::fs::read(&path).unwrap();
                for secret in secrets {
                    let escaped = serde_json::to_string(secret).unwrap();
                    let escaped = &escaped[1..escaped.len() - 1];
                    for needle in [*secret, escaped] {
                        assert!(
                            !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                            "{} holds a redacted value",
                            path.display()
                        );
                    }
                }
            }
        }
        assert!(files > 0, "no turn files were written");
    }

    #[test]
    fn redact_values_never_reach_turn_files() {
        let secrets = ["hunter2-Secret", "pa\"ss\\word"];
        let root = tempfile::tempdir().unwrap();
        let redactions = Redactions::new(secrets.iter().map(|s| (*s).to_owned()));

        // Arguments and result summary through the session API.
        let session = RecordingSession::new();
        {
            let mut inner = session.inner.lock().unwrap();
            inner.recorder = Recorder::Live(Live {
                redactions: redactions.clone(),
                ..live_at(&root.path().join("session"))
            });
        }
        let pending = session
            .begin_private_turn(
                "type_text",
                &serde_json::json!({"pid": 1, "text": "login hunter2-Secret now"}),
                now_ms(),
                RecordingCaller::default(),
            )
            .expect("reserve turn");
        session.finish_turn(pending, "typed 'login hunter2-Secret now'");
        let action: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("session/turn-00001/action.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(action["arguments"]["text"], "login [redacted] now");
        assert_eq!(action["result_summary"], "typed 'login [redacted] now'");
        assert_eq!(action["redacted"], true);

        // A turn without a secret in its arguments is not marked.
        let pending = session
            .begin_private_turn(
                "press_key",
                &serde_json::json!({"pid": 1, "key": "return"}),
                now_ms(),
                RecordingCaller::default(),
            )
            .unwrap();
        session.finish_turn(pending, "pressed");
        let action: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("session/turn-00002/action.json")).unwrap(),
        )
        .unwrap();
        assert!(action.get("redacted").is_none());

        // Stopping forgets the values.
        session.stop(StopBy::Anyone).unwrap();
        assert!(!session.current_state().enabled);

        // Before/after accessibility state, including JSON-escaped values.
        let state = serde_json::to_vec(&serde_json::json!({
            "role": "AXTextField",
            "value": "pa\"ss\\word",
            "children": [{"title": "hunter2-Secret"}],
        }))
        .unwrap();
        let turn_dir = root.path().join("state");
        let pending = PendingTurn {
            generation: 0,
            turn_dir: turn_dir.clone(),
            tool_name: "set_value".into(),
            args: serde_json::json!({"pid": 1, "value": "pa\"ss\\word"}),
            start_ms: 0,
            session_start_ms: 0,
            window_id: None,
            pid: Some(1),
            click_point: None,
            capture_visual_state: true,
            state_budget: Some(StateCaptureBudget::default()),
            predicted_refusal: None,
            before: TurnCapture::default(),
            dispatch_click: None,
            redactions: redactions.clone(),
        };
        let after = TurnCapture {
            state: StateCapture::captured(state.clone()),
            ..Default::default()
        };
        write_turn_with_after(pending, after, "set", None, false).unwrap();
        let app_state: Value =
            serde_json::from_slice(&std::fs::read(turn_dir.join("app_state.json")).unwrap())
                .unwrap();
        assert_eq!(app_state["value"], "[redacted]");
        assert_eq!(app_state["children"][0]["title"], "[redacted]");

        turn_files_contain_no_secret(root.path(), &secrets);
    }

    #[test]
    fn redactions_replace_longest_value_first_and_ignore_empty() {
        let redactions = Redactions::new(["abc".to_owned(), "abcdef".to_owned(), String::new()]);
        let mut value = serde_json::json!({"text": "xabcdefx abc"});
        assert!(redactions.scrub_json(&mut value));
        assert_eq!(value["text"], "x[redacted]x [redacted]");
        assert!(!Redactions::new([String::new()]).scrub_json(&mut value));
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
            inner.recorder = Recorder::Live(live_at(&output_dir));
        }
        let pending = session
            .begin_private_turn(
                "browser_prepare",
                &serde_json::json!({"pid": 1, "window_id": 2}),
                now_ms(),
                RecordingCaller::default(),
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
    fn stop_surfaces_video_finalization_failure() {
        let output_dir = std::env::temp_dir().join(format!(
            "cua-recording-stop-failure-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&output_dir).expect("create recording test directory");
        let session = RecordingSession::new();
        {
            let mut inner = session.inner.lock().expect("recording lock");
            inner.recorder = Recorder::Live(Live {
                video: Some(Box::new(FailingVideo)),
                ..live_at(&output_dir)
            });
        }

        let error = session
            .stop(StopBy::Anyone)
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

    #[test]
    fn owned_recording_reserves_turns_only_for_its_owning_session() {
        let root = tempfile::tempdir().expect("temp dir");
        let output_dir = root.path().join("owned");
        let session = RecordingSession::new();
        session
            .start(
                output_dir.to_str().expect("utf-8 output dir"),
                false,
                Some("owner"),
            )
            .expect("start an owned recording");

        // Avoid the (pid 1, window 2) target whose screenshot phase another test counts.
        let args = serde_json::json!({"pid": 41, "window_id": 42, "x": 3, "y": 4});
        assert!(
            session
                .begin_turn(
                    "click",
                    &args,
                    now_ms(),
                    RecordingCaller {
                        session: Some("other"),
                        transport: None
                    }
                )
                .is_none(),
            "another session's call must not be recorded"
        );
        assert!(
            session
                .begin_turn("click", &args, now_ms(), RecordingCaller::default())
                .is_none(),
            "a sessionless call must not be recorded"
        );

        let pending = session
            .begin_turn(
                "click",
                &args,
                now_ms(),
                RecordingCaller {
                    session: Some("owner"),
                    transport: None,
                },
            )
            .expect("the owning session's call reserves a turn");
        session.finish_turn(pending, "clicked");
        session.stop(StopBy::Anyone).expect("stop owned recording");

        let action: Value = serde_json::from_slice(
            &std::fs::read(output_dir.join("turn-00001").join("action.json"))
                .expect("read the owner's turn"),
        )
        .expect("parse action.json");
        assert_eq!(action["tool"], "click");
        assert!(
            !output_dir.join("turn-00002").exists(),
            "foreign calls must not leave turn folders"
        );
    }

    #[test]
    fn stop_preserves_recording_start_timestamp_in_session_manifest() {
        let root = tempfile::tempdir().expect("temp dir");
        let output_dir = root.path().join("recording");
        let session = RecordingSession::new();
        session
            .start(output_dir.to_str().expect("utf-8 output dir"), false, None)
            .expect("start recording");

        let read_manifest = || -> Value {
            serde_json::from_slice(
                &std::fs::read(output_dir.join("session.json")).expect("read session manifest"),
            )
            .expect("parse session manifest")
        };
        let started_at_ms = read_manifest()["started_at_monotonic_ms"]
            .as_u64()
            .expect("start timestamp is an integer");

        std::thread::sleep(Duration::from_millis(100));
        session.stop(StopBy::Anyone).expect("stop recording");

        assert_eq!(
            read_manifest()["started_at_monotonic_ms"],
            started_at_ms,
            "finalizing the manifest must not replace the recording start time"
        );
    }

    #[test]
    fn unlabelled_recording_keeps_named_sessions_on_its_own_transport() {
        let root = tempfile::tempdir().expect("temp dir");
        let output_dir = root.path().join("transport-owned");
        let session = RecordingSession::new();
        // An unlabelled start is owned by the transport lease that carried it.
        session
            .start(
                output_dir.to_str().expect("utf-8 output dir"),
                false,
                Some("transport-1"),
            )
            .expect("start a transport-owned recording");

        // Avoid the (pid 1, window 2) target whose screenshot phase another test counts.
        let args = serde_json::json!({"pid": 41, "window_id": 42, "x": 3, "y": 4});
        assert!(
            session
                .begin_turn(
                    "click",
                    &args,
                    now_ms(),
                    RecordingCaller {
                        session: Some("demo"),
                        transport: Some("transport-2"),
                    },
                )
                .is_none(),
            "a named session on another transport must not be recorded"
        );
        let pending = session
            .begin_turn(
                "click",
                &args,
                now_ms(),
                RecordingCaller {
                    session: Some("demo"),
                    transport: Some("transport-1"),
                },
            )
            .expect("a named session on the owning transport reserves a turn");
        session.finish_turn(pending, "clicked");
        session
            .stop(StopBy::Anyone)
            .expect("stop transport-owned recording");

        assert!(output_dir.join("turn-00001").join("action.json").exists());
        assert!(!output_dir.join("turn-00002").exists());
    }

    #[test]
    fn anonymous_recording_stays_daemon_wide() {
        let root = tempfile::tempdir().expect("temp dir");
        let output_dir = root.path().join("anonymous");
        let session = RecordingSession::new();
        session
            .start(output_dir.to_str().expect("utf-8 output dir"), false, None)
            .expect("start an anonymous recording");

        // Avoid the (pid 1, window 2) target whose screenshot phase another test counts.
        let args = serde_json::json!({"pid": 41, "window_id": 42, "x": 3, "y": 4});
        for caller in [
            RecordingCaller {
                session: Some("some-session"),
                transport: None,
            },
            RecordingCaller::default(),
        ] {
            let pending = session
                .begin_turn("click", &args, now_ms(), caller)
                .expect("an anonymous recording records every caller");
            session.finish_turn(pending, "clicked");
        }
        session
            .stop(StopBy::Anyone)
            .expect("stop anonymous recording");

        assert!(output_dir.join("turn-00001").join("action.json").exists());
        assert!(output_dir.join("turn-00002").join("action.json").exists());
    }
}
