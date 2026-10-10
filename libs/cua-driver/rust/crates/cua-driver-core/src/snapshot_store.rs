use crate::element_token::{
    format_snapshot_id, parse_token, refusal, token_for, ResolvedElement, LRU_CAP_PER_PID,
    STALE_TOKEN_ERROR, STALE_TOKEN_SESSION_HINT,
};
use crate::protocol::ToolResult;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// Reserved dispatch argument marking window-relative pixels as native
/// window pixels. Dispatch strips every caller-supplied underscore argument,
/// so only [`crate::tool::ToolRegistry::invoke_with_native_window_pixels`]
/// (an in-process Rust API) can set it.
pub const NATIVE_WINDOW_PIXELS_ARG: &str = "_native_window_pixels";

pub trait SnapshotPayload: Send + Sync + 'static {
    type Element;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn retain(&self, index: usize) -> Option<Self::Element>;
}

struct Snapshot<S> {
    id: u32,
    window_id: u64,
    screenshot_owner: Option<String>,
    screenshot_scale: Option<f64>,
    /// Window size in points when the screenshot was taken. A tree-only read
    /// keeps the screenshot frame only while the window still has this size.
    screenshot_window_size: Option<(f64, f64)>,
    zoom: Option<ZoomContext>,
    semantic: bool,
    payload: S,
}

impl<S> Snapshot<S> {
    fn screenshot(&self, session: Option<&str>) -> Option<ScreenshotContext> {
        let scale = self
            .screenshot_scale
            .filter(|_| self.screenshot_owner.as_deref() == session)?;
        Some(ScreenshotContext {
            snapshot_id: self.id,
            window_id: self.window_id,
            scale,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenshotContext {
    pub snapshot_id: u32,
    pub window_id: u64,
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomContext {
    pub screenshot: ScreenshotContext,
    pub origin_x: f64,
    pub origin_y: f64,
    pub scale_inv: f64,
}

impl ZoomContext {
    pub fn zoom_to_window(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.origin_x + x * self.scale_inv,
            self.origin_y + y * self.scale_inv,
        )
    }
}

/// Why a pixel action found no screenshot frame for its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MissingFrame {
    /// This pid has no snapshot of the window at all in this runtime.
    WindowNeverRead,
    /// The window's current snapshot carries no screenshot frame: the last
    /// read skipped the screenshot after the window changed size, or no
    /// screenshot was ever taken of it.
    NoScreenshot,
    /// The current screenshot frame belongs to another session.
    OtherSession,
    /// No window was named and the process's snapshots disagree.
    Ambiguous,
}

impl MissingFrame {
    fn as_str(self) -> &'static str {
        match self {
            Self::WindowNeverRead => "window_never_read",
            Self::NoScreenshot => "no_screenshot",
            Self::OtherSession => "other_session",
            Self::Ambiguous => "ambiguous_window",
        }
    }
}

fn screenshot_context_refusal(
    pid: Option<i32>,
    window_id: Option<u64>,
    why: MissingFrame,
) -> ToolResult {
    let read = match (pid, window_id) {
        (Some(pid), Some(window_id)) => {
            format!("get_window_state(pid:{pid}, window_id:{window_id})")
        }
        (None, Some(window_id)) => format!("get_window_state(pid, window_id:{window_id})"),
        _ => "get_window_state(pid, window_id)".to_owned(),
    };
    let window = window_id.map_or_else(|| "this window".to_owned(), |id| format!("window {id}"));
    let message = match why {
        MissingFrame::WindowNeverRead => format!(
            "No screenshot of {window} in this session: it has not been read yet. Call {read} \
             and take x,y from that window's own screenshot; pixels read off another window's \
             screenshot do not apply to it."
        ),
        MissingFrame::NoScreenshot => format!(
            "No screenshot frame for {window}: its latest read had no screenshot and the window \
             changed size since the last one (or it was never captured). Call {read} with the \
             default screenshot and take x,y from that image."
        ),
        MissingFrame::OtherSession => format!(
            "The current screenshot of {window} belongs to another session. Call {read} on \
             this connection before using pixels."
        ),
        MissingFrame::Ambiguous => format!(
            "Pass window_id: this process has several windows without one shared screenshot \
             frame. Then call {read} if that window has no screenshot yet."
        ),
    };
    ToolResult::error(message).with_structured(serde_json::json!({
        "code": "screenshot_context_missing",
        "reason": why.as_str(),
        "pid": pid,
        "window_id": window_id,
    }))
}

fn stale_token_refusal<S>(pid: i32, lane: &[Snapshot<S>]) -> ToolResult {
    let current: Vec<_> = lane
        .iter()
        .map(|snapshot| (format_snapshot_id(snapshot.id), snapshot.window_id))
        .collect();
    let message = match current.as_slice() {
        [] => format!(
            "{STALE_TOKEN_ERROR}; pid {pid} has no current snapshot. {STALE_TOKEN_SESSION_HINT}."
        ),
        current => format!(
            "{STALE_TOKEN_ERROR}; current snapshots for pid {pid}: {}. {STALE_TOKEN_SESSION_HINT}.",
            current
                .iter()
                .map(|(snapshot_id, window_id)| format!("{snapshot_id} (window {window_id})"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": { "code": "stale_element_token", "message": message },
        "current_snapshots": current
            .iter()
            .map(|(snapshot_id, window_id)| {
                serde_json::json!({ "snapshot_id": snapshot_id, "window_id": window_id })
            })
            .collect::<Vec<_>>(),
    }))
}

fn zoom_context_refusal(pid: i32, window_id: Option<u64>) -> ToolResult {
    let message = "The zoom coordinate context is missing or was replaced by a newer snapshot. Call get_window_state and zoom again on the same connection before using from_zoom coordinates.";
    ToolResult::error(message).with_structured(serde_json::json!({
        // Explicit shared refusal envelope: the context is resolved before any
        // input is dispatched, so nothing was delivered. Top-level fields stay
        // for existing consumers.
        "status": "refused",
        "refusal": { "code": "zoom_context_missing", "message": message },
        "code": "zoom_context_missing",
        "pid": pid,
        "window_id": window_id,
    }))
}

/// The snapshot a bare row number refers to: the current snapshot of the
/// named window, or the process's only snapshot when no window is named.
fn row_snapshot<S: SnapshotPayload>(
    pid: i32,
    lane: &[Snapshot<S>],
    window_id: Option<u64>,
    row: usize,
) -> Result<&Snapshot<S>, ToolResult> {
    let found = match window_id {
        Some(window_id) => lane.iter().find(|snapshot| snapshot.window_id == window_id),
        None => match lane {
            [only] => Some(only),
            _ => None,
        },
    };
    found.ok_or_else(|| {
        let current = lane
            .iter()
            .map(|snapshot| {
                format!(
                    "\"{}\" (window {})",
                    token_for(snapshot.id, row),
                    snapshot.window_id
                )
            })
            .collect::<Vec<_>>();
        let message = if current.is_empty() {
            format!(
                "element_token \"{row}\" is a row number, and pid {pid} has no current \
                 snapshot. Call get_window_state, then pass \"<snapshot_id>:{row}\"."
            )
        } else {
            format!(
                "element_token \"{row}\" is a row number, not a token. Pass the full token, \
                 <snapshot_id>:{row}: {}; or add window_id.",
                current.join(", ")
            )
        };
        refusal("invalid_element_token", message)
    })
}

fn invalid_token_refusal<S>(token: &str, lane: &[Snapshot<S>]) -> ToolResult {
    let example = lane.last().map_or_else(
        || "s0000002a:11".to_owned(),
        |snapshot| token_for(snapshot.id, 11),
    );
    refusal(
        "invalid_element_token",
        format!(
            "element_token \"{token}\" has invalid format: use <snapshot_id>:<row>, e.g. \
             \"{example}\" for row [11] of the read whose snapshot_id is {}.",
            example.split(':').next().unwrap_or_default()
        ),
    )
}

fn out_of_range_refusal<S: SnapshotPayload>(snapshot: &Snapshot<S>, row: usize) -> ToolResult {
    let snapshot_id = format_snapshot_id(snapshot.id);
    let rows = match snapshot.payload.len() {
        0 => "no rows".to_owned(),
        n => format!("rows [0] to [{}] only", n - 1),
    };
    refusal(
        "invalid_element_token",
        format!(
            "element_token {snapshot_id}:{row} is out of range: snapshot {snapshot_id} has \
             {rows} ({} element(s)). Row numbers belong to one read: a read with a smaller \
             max_elements, max_depth or another query can stop before row [{row}]. Use a row \
             printed in {snapshot_id}, or re-read with a larger max_elements (or a query that \
             matches the target) and use that read's rows.",
            snapshot.payload.len()
        ),
    )
}

pub struct SnapshotStore<S: SnapshotPayload> {
    runtime_scope: String,
    inner: Mutex<HashMap<i32, Vec<Snapshot<S>>>>,
}

impl<S: SnapshotPayload> SnapshotStore<S> {
    pub fn new() -> Self {
        Self {
            runtime_scope: current_runtime_scope(),
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn publish(&self, pid: i32, window_id: u64, payload: S) -> u32 {
        self.publish_for_session(pid, window_id, payload, None, None)
            .expect("anonymous snapshot publication cannot be retired")
            .0
    }

    /// Publish the latest runtime-owned snapshot and its screenshot coordinate frame,
    /// returning its id and the ids of the snapshots it replaced or evicted.
    ///
    /// `screenshot_scale` is the native-image-width / delivered-image-width ratio.
    /// A `None` scale deliberately records that the latest observation did not
    /// deliver an actionable screenshot. Publications completing after their
    /// owning session ended are discarded instead of resurrecting retired state.
    pub fn publish_for_session(
        &self,
        pid: i32,
        window_id: u64,
        payload: S,
        session: Option<&str>,
        screenshot_scale: Option<f64>,
    ) -> Option<(u32, Vec<u32>)> {
        self.publish_snapshot(
            pid,
            window_id,
            payload,
            session,
            (screenshot_scale, None),
            true,
        )
    }

    /// [`Self::publish_for_session`] that also records the window's size in
    /// points (`window_size`), so later reads can tell whether the screenshot
    /// frame still applies.
    ///
    /// * With a `screenshot_scale`, the size is stored with the new frame.
    /// * Without one (a tree-only read), the new snapshot keeps the replaced
    ///   snapshot's screenshot frame when that frame belongs to the same
    ///   session and was taken at the same window size. Window-relative pixels
    ///   off that screenshot still land where they did, so a pixel action
    ///   after a tree-only re-read is not refused. A resized window, an
    ///   unknown size or another session's frame retires the frame as before.
    pub fn publish_sized_for_session(
        &self,
        pid: i32,
        window_id: u64,
        payload: S,
        session: Option<&str>,
        screenshot_scale: Option<f64>,
        window_size: Option<(f64, f64)>,
    ) -> Option<(u32, Vec<u32>)> {
        self.publish_snapshot(
            pid,
            window_id,
            payload,
            session,
            (screenshot_scale, window_size),
            true,
        )
    }

    /// Refresh the screenshot frame of the window's current snapshot in place,
    /// keeping its id, its elements and their tokens. Used by a
    /// screenshot-only read (no tree walk): it adds a new image but no new
    /// element indices, so the tokens the caller holds stay valid. Returns the
    /// kept snapshot id, or `None` when the window has no snapshot with
    /// elements (the caller then publishes a new one).
    pub fn refresh_screenshot_for_session(
        &self,
        pid: i32,
        window_id: u64,
        session: Option<&str>,
        screenshot_scale: f64,
        window_size: Option<(f64, f64)>,
    ) -> Option<u32> {
        let mut inner = self.inner.lock().unwrap();
        if session.is_some_and(crate::session::is_session_ended) {
            return None;
        }
        let snapshot = inner
            .get_mut(&pid)?
            .iter_mut()
            .find(|snapshot| snapshot.window_id == window_id)
            .filter(|snapshot| snapshot.semantic && !snapshot.payload.is_empty())?;
        snapshot.screenshot_owner = session.map(str::to_owned);
        snapshot.screenshot_scale = Some(screenshot_scale);
        snapshot.screenshot_window_size = window_size;
        snapshot.zoom = None;
        Some(snapshot.id)
    }

    /// Publish screenshot/capture state without claiming that an accessibility
    /// walk has completed for this window.
    pub fn publish_capture_for_session(
        &self,
        pid: i32,
        window_id: u64,
        payload: S,
        session: Option<&str>,
        screenshot_scale: Option<f64>,
    ) -> Option<(u32, Vec<u32>)> {
        self.publish_snapshot(
            pid,
            window_id,
            payload,
            session,
            (screenshot_scale, None),
            false,
        )
    }

    fn publish_snapshot(
        &self,
        pid: i32,
        window_id: u64,
        payload: S,
        session: Option<&str>,
        // The new screenshot's scale, if any, and the window size in points
        // at this read.
        (screenshot_scale, window_size): (Option<f64>, Option<(f64, f64)>),
        semantic: bool,
    ) -> Option<(u32, Vec<u32>)> {
        let (id, retired) = {
            let mut inner = self.inner.lock().unwrap();
            if session.is_some_and(crate::session::is_session_ended) {
                return None;
            }
            let lane = inner.entry(pid).or_default();
            let mut retired = Vec::new();
            if let Some(position) = lane.iter().position(|entry| entry.window_id == window_id) {
                retired.push(lane.remove(position));
            }
            // A new screenshot brings its own frame. A tree-only read keeps the
            // replaced snapshot's frame while the window keeps its size.
            let (scale, frame_size) = match (screenshot_scale, window_size) {
                (Some(scale), size) => (Some(scale), size),
                (None, Some(now)) => retired
                    .first()
                    .filter(|previous| previous.screenshot_owner.as_deref() == session)
                    .and_then(|previous| {
                        let then = previous.screenshot_window_size?;
                        let same = (then.0 - now.0).abs() < 0.5 && (then.1 - now.1).abs() < 0.5;
                        same.then_some((previous.screenshot_scale?, then))
                    })
                    .map_or((None, None), |(scale, size)| (Some(scale), Some(size))),
                (None, None) => (None, None),
            };
            if lane.len() == LRU_CAP_PER_PID {
                retired.push(lane.remove(0));
            }
            let id = crate::element_token::mint_snapshot_id();
            lane.push(Snapshot {
                id,
                window_id,
                screenshot_owner: session.map(str::to_owned),
                screenshot_scale: scale,
                screenshot_window_size: frame_size,
                zoom: None,
                semantic,
                payload,
            });
            (id, retired)
        };
        let invalidated = retired.iter().map(|snapshot| snapshot.id).collect();
        drop(retired);
        Some((id, invalidated))
    }

    /// Resolve the screenshot transform from the same authoritative latest
    /// snapshot used for element tokens. Without a window, every snapshot of
    /// the process must agree on one transform owned by this session.
    pub fn screenshot_context(
        &self,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<ScreenshotContext, ToolResult> {
        let inner = self.inner.lock().unwrap();
        let lane = inner.get(&pid).map(Vec::as_slice).unwrap_or_default();
        match window_id {
            Some(window_id) => {
                let snapshot = lane
                    .iter()
                    .find(|snapshot| snapshot.window_id == window_id)
                    .ok_or_else(|| {
                        screenshot_context_refusal(
                            Some(pid),
                            Some(window_id),
                            MissingFrame::WindowNeverRead,
                        )
                    })?;
                snapshot.screenshot(session).ok_or_else(|| {
                    let why = if snapshot.screenshot_scale.is_some() {
                        MissingFrame::OtherSession
                    } else {
                        MissingFrame::NoScreenshot
                    };
                    screenshot_context_refusal(Some(pid), Some(window_id), why)
                })
            }
            None => {
                let mut contexts = lane.iter().map(|snapshot| snapshot.screenshot(session));
                let why = match lane {
                    [] => MissingFrame::WindowNeverRead,
                    [_] => MissingFrame::NoScreenshot,
                    _ => MissingFrame::Ambiguous,
                };
                contexts
                    .next()
                    .flatten()
                    .filter(|first| {
                        contexts.all(|context| {
                            context
                                .is_some_and(|context| (context.scale - first.scale).abs() < 1e-9)
                        })
                    })
                    .ok_or_else(|| screenshot_context_refusal(Some(pid), None, why))
            }
        }
    }

    /// The native-image / delivered-image scale for a window-relative pixel
    /// action: 1.0 for a trusted in-process call whose pixels are already
    /// native ([`NATIVE_WINDOW_PIXELS_ARG`]), otherwise the scale of the
    /// session's current screenshot of the window (refused without one).
    pub fn screenshot_scale(
        &self,
        pid: i32,
        window_id: Option<u64>,
        args: &serde_json::Value,
    ) -> Result<f64, ToolResult> {
        if args.get(NATIVE_WINDOW_PIXELS_ARG) == Some(&serde_json::Value::Bool(true)) {
            return Ok(1.0);
        }
        self.screenshot_context(
            pid,
            window_id,
            args.get("_session_id").and_then(serde_json::Value::as_str),
        )
        .map(|context| context.scale)
    }

    pub fn screenshot_context_for_zoom(
        &self,
        pid: Option<i32>,
        window_id: u64,
        session: Option<&str>,
    ) -> Result<(i32, ScreenshotContext), ToolResult> {
        if let Some(pid) = pid {
            return self
                .screenshot_context(pid, Some(window_id), session)
                .map(|context| (pid, context));
        }
        let inner = self.inner.lock().unwrap();
        let mut matches = inner.iter().filter_map(|(pid, lane)| {
            let snapshot = lane
                .iter()
                .find(|snapshot| snapshot.window_id == window_id)?;
            Some((*pid, snapshot.screenshot(session)?))
        });
        match (matches.next(), matches.next()) {
            (Some(found), None) => Ok(found),
            _ => Err(screenshot_context_refusal(
                None,
                Some(window_id),
                MissingFrame::WindowNeverRead,
            )),
        }
    }

    pub fn set_zoom(
        &self,
        pid: i32,
        session: Option<&str>,
        zoom: ZoomContext,
    ) -> Result<(), ToolResult> {
        let mut inner = self.inner.lock().unwrap();
        let snapshot = inner
            .get_mut(&pid)
            .and_then(|lane| {
                lane.iter_mut()
                    .find(|snapshot| snapshot.window_id == zoom.screenshot.window_id)
            })
            .filter(|snapshot| snapshot.screenshot(session) == Some(zoom.screenshot))
            .ok_or_else(|| zoom_context_refusal(pid, Some(zoom.screenshot.window_id)))?;
        snapshot.zoom = Some(zoom);
        Ok(())
    }

    pub fn zoom(
        &self,
        pid: i32,
        window_id: Option<u64>,
        session: Option<&str>,
    ) -> Result<ZoomContext, ToolResult> {
        let inner = self.inner.lock().unwrap();
        let mut zooms = inner
            .get(&pid)
            .into_iter()
            .flatten()
            .filter(|snapshot| {
                window_id.is_none_or(|window_id| snapshot.window_id == window_id)
                    && snapshot.screenshot_owner.as_deref() == session
            })
            .filter_map(|snapshot| snapshot.zoom);
        match (zooms.next(), zooms.next()) {
            (Some(zoom), None) => Ok(zoom),
            _ => Err(zoom_context_refusal(pid, window_id)),
        }
    }

    pub fn retire_session_screenshots(&self, session: &str) -> usize {
        let retired = {
            let mut inner = self.inner.lock().unwrap();
            let mut retired = Vec::new();
            for lane in inner.values_mut() {
                let (owned, kept) = std::mem::take(lane)
                    .into_iter()
                    .partition(|snapshot| snapshot.screenshot_owner.as_deref() == Some(session));
                *lane = kept;
                retired.extend::<Vec<_>>(owned);
            }
            inner.retain(|_, lane| !lane.is_empty());
            retired
        };
        let count = retired.len();
        drop(retired);
        count
    }

    pub fn window_for_snapshot(&self, pid: i32, snapshot_id: u32) -> Option<u64> {
        self.inner
            .lock()
            .unwrap()
            .get(&pid)?
            .iter()
            .find(|entry| entry.id == snapshot_id)
            .map(|entry| entry.window_id)
    }

    /// Whether this runtime has already published any snapshot for the window.
    pub fn contains_window(&self, pid: i32, window_id: u64) -> bool {
        self.inner
            .lock()
            .unwrap()
            .get(&pid)
            .is_some_and(|lane| lane.iter().any(|entry| entry.window_id == window_id))
    }

    /// Whether an accessibility/semantic snapshot has been published for the
    /// window. Screenshot-only previews deliberately do not satisfy this.
    pub fn contains_semantic_window(&self, pid: i32, window_id: u64) -> bool {
        self.inner.lock().unwrap().get(&pid).is_some_and(|lane| {
            lane.iter()
                .any(|entry| entry.window_id == window_id && entry.semantic)
        })
    }

    /// The pid an `element_token` was minted for, found without the caller
    /// naming it. Snapshot ids are unique per runtime, so a token alone names
    /// its snapshot, window and process. `None` when the token is absent,
    /// malformed, or no longer current; callers then fall back to the
    /// ordinary missing-`pid` error.
    pub fn pid_for_token(&self, args: &serde_json::Value) -> Option<i32> {
        let token = args.get("element_token")?.as_str()?;
        let (snapshot_id, _) = parse_token(token)?;
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|(_, lane)| lane.iter().any(|snapshot| snapshot.id == snapshot_id))
            .map(|(pid, _)| *pid)
    }

    pub fn resolve(
        &self,
        pid: i32,
        args: &serde_json::Value,
    ) -> Result<ResolvedElement<S::Element>, ToolResult> {
        let Some(token) = args
            .get("element_token")
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(ResolvedElement::None);
        };
        let window_arg = args["window_id"].as_u64();
        let inner = self.inner.lock().unwrap();
        let lane = inner.get(&pid).map(Vec::as_slice).unwrap_or_default();
        let (snapshot, element_index) = match parse_token(token) {
            Some((snapshot_id, element_index)) => {
                let Some(snapshot) = lane.iter().find(|snapshot| snapshot.id == snapshot_id) else {
                    return Err(stale_token_refusal(pid, lane));
                };
                (snapshot, element_index)
            }
            // A bare row number ("11") is the `[11]` of a markdown read. It
            // names a row of the window's current snapshot, the one a fresh
            // read just printed, so resolve it there when the window is known.
            None => match token.trim().parse::<usize>() {
                Ok(row) => (row_snapshot(pid, lane, window_arg, row)?, row),
                Err(_) => return Err(invalid_token_refusal(token, lane)),
            },
        };
        if window_arg.is_some_and(|window_id| window_id != snapshot.window_id) {
            return Err(refusal(
                "conflicting_element_target",
                "element_token conflicts with window_id".into(),
            ));
        }
        let element = snapshot
            .payload
            .retain(element_index)
            .ok_or_else(|| out_of_range_refusal(snapshot, element_index))?;
        Ok(ResolvedElement::Element {
            window_id: snapshot.window_id,
            element_index,
            element,
        })
    }

    pub fn remove(&self, pid: i32, window_id: u64) -> Option<u32> {
        let retired = {
            let mut inner = self.inner.lock().unwrap();
            inner.get_mut(&pid).and_then(|lane| {
                let position = lane.iter().position(|entry| entry.window_id == window_id)?;
                Some(lane.remove(position))
            })
        };
        retired.map(|snapshot| snapshot.id)
    }

    pub fn clear(&self) -> usize {
        let retired = std::mem::take(&mut *self.inner.lock().unwrap());
        let count = retired.len();
        drop(retired);
        count
    }
}

impl<S: SnapshotPayload> Drop for SnapshotStore<S> {
    fn drop(&mut self) {
        let mut caches = runtime_stores().lock().unwrap();
        if caches
            .get(&self.runtime_scope)
            .is_some_and(|cache| std::ptr::addr_eq(cache.as_ptr(), self as *const Self))
        {
            caches.remove(&self.runtime_scope);
        }
        if caches.is_empty() {
            caches.shrink_to_fit();
        }
    }
}

impl<S: SnapshotPayload> Default for SnapshotStore<S> {
    fn default() -> Self {
        Self::new()
    }
}

trait RuntimeStore: Any + Send + Sync {
    fn clear(&self) -> usize;
}

impl<S: SnapshotPayload> RuntimeStore for SnapshotStore<S> {
    fn clear(&self) -> usize {
        self.clear()
    }
}

fn runtime_stores() -> &'static Mutex<HashMap<String, Weak<dyn RuntimeStore>>> {
    static STORES: OnceLock<Mutex<HashMap<String, Weak<dyn RuntimeStore>>>> = OnceLock::new();
    STORES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn current_runtime_scope() -> String {
    crate::tool::current_dispatch_runtime_scope().unwrap_or_else(|| "legacy".into())
}

pub fn register_runtime_store<S: SnapshotPayload>(cache: &Arc<SnapshotStore<S>>) {
    let erased: Arc<dyn RuntimeStore> = cache.clone();
    let mut caches = runtime_stores().lock().unwrap();
    caches.retain(|_, cache| cache.strong_count() > 0);
    caches.insert(cache.runtime_scope.clone(), Arc::downgrade(&erased));
}

pub fn current_runtime_store<S: SnapshotPayload>() -> Option<Arc<SnapshotStore<S>>> {
    let cache = runtime_stores()
        .lock()
        .unwrap()
        .get(&current_runtime_scope())?
        .upgrade()?;
    let erased: Arc<dyn Any + Send + Sync> = cache;
    erased.downcast().ok()
}

pub fn retire_runtime_scope(runtime_scope: &str) -> usize {
    let cache = runtime_stores()
        .lock()
        .unwrap()
        .remove(runtime_scope)
        .and_then(|cache| cache.upgrade());
    cache.map_or(0, |cache| cache.clear())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element_token::token_for;
    use crate::snapshot_test_support::Payload;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn pid_for_token_finds_the_owning_process() {
        let cache = SnapshotStore::new();
        let id = cache.publish(42, 7, Payload(vec![10, 20]));
        cache.publish(43, 8, Payload(vec![1]));
        let args = serde_json::json!({ "element_token": token_for(id, 1) });
        assert_eq!(cache.pid_for_token(&args), Some(42));
        assert_eq!(cache.pid_for_token(&serde_json::json!({})), None);
        let unknown = serde_json::json!({ "element_token": token_for(id + 1000, 0) });
        assert_eq!(cache.pid_for_token(&unknown), None);
    }

    #[test]
    fn publish_then_resolve_returns_projection() {
        let cache = SnapshotStore::new();
        let id = cache.publish(42, 7, Payload(vec![10, 20, 30]));
        let result = cache
            .resolve(
                42,
                &serde_json::json!({ "element_token": token_for(id, 2) }),
            )
            .unwrap();
        assert!(matches!(
            result,
            ResolvedElement::Element { element: 30, .. }
        ));
    }

    #[test]
    fn miss_returns_refusal() {
        let cache = SnapshotStore::<Payload>::new();
        assert!(cache
            .resolve(1, &serde_json::json!({ "element_token": token_for(0, 0) }))
            .is_err());
    }

    #[test]
    fn token_resolution_rejects_conflicting_windows() {
        let cache = SnapshotStore::new();
        let token = token_for(cache.publish(7, 42, Payload(vec![1])), 0);
        let mut args = serde_json::json!({ "element_token": token });
        for window_id in [None, Some(42)] {
            if let Some(window_id) = window_id {
                args["window_id"] = serde_json::json!(window_id);
            }
            assert!(matches!(
                cache.resolve(7, &args).unwrap(),
                ResolvedElement::Element { window_id: 42, .. }
            ));
        }
        args["window_id"] = serde_json::json!(99);
        let error = cache.resolve(7, &args).unwrap_err();
        assert_eq!(
            error.structured_content.unwrap()["refusal"]["code"],
            "conflicting_element_target"
        );
    }

    #[test]
    fn membership_matches_payload_length() {
        let cache = SnapshotStore::new();
        let id = cache.publish(9, 99, Payload(vec![1, 2, 3, 4, 5]));
        for index in 0..5 {
            assert!(cache
                .resolve(
                    9,
                    &serde_json::json!({ "element_token": token_for(id, index) })
                )
                .is_ok());
        }
        assert!(cache
            .resolve(9, &serde_json::json!({ "element_token": token_for(id, 5) }))
            .is_err());
    }

    #[test]
    fn semantic_membership_ignores_capture_only_publication() {
        let cache = SnapshotStore::new();
        assert!(!cache.contains_window(9, 99));
        assert!(!cache.contains_semantic_window(9, 99));

        cache
            .publish_capture_for_session(9, 99, Payload(vec![]), None, Some(1.0))
            .unwrap();
        assert!(cache.contains_window(9, 99));
        assert!(!cache.contains_semantic_window(9, 99));

        cache.publish(9, 99, Payload(vec![]));
        assert!(cache.contains_semantic_window(9, 99));
        assert!(!cache.contains_semantic_window(9, 100));
        assert!(!cache.contains_semantic_window(10, 99));

        cache.remove(9, 99);
        assert!(!cache.contains_window(9, 99));
        assert!(!cache.contains_semantic_window(9, 99));
    }

    fn token_refusal(cache: &SnapshotStore<Payload>, pid: i32, token: &str) -> serde_json::Value {
        cache
            .resolve(pid, &serde_json::json!({ "element_token": token }))
            .unwrap_err()
            .structured_content
            .unwrap()
    }

    #[test]
    fn malformed_token_is_invalid_not_stale() {
        let cache = SnapshotStore::new();
        cache.publish(10, 1, Payload(vec![0]));
        assert_eq!(
            token_refusal(&cache, 10, "garbage")["refusal"]["code"],
            "invalid_element_token"
        );
    }

    #[test]
    fn tokens_in_different_pids_dont_collide() {
        let cache = SnapshotStore::new();
        let first = cache.publish(100, 11, Payload(vec![0]));
        cache.publish(200, 22, Payload(vec![0]));
        assert_eq!(
            token_refusal(&cache, 200, &token_for(first, 0))["refusal"]["code"],
            "stale_element_token"
        );
    }

    #[test]
    fn stale_token_names_the_current_snapshots() {
        let cache = SnapshotStore::new();
        let current = cache.publish(1, 555, Payload(vec![0]));
        let structured = token_refusal(&cache, 1, &token_for(0xdead, 0));
        assert_eq!(structured["refusal"]["code"], "stale_element_token");
        assert_eq!(
            structured["current_snapshots"],
            serde_json::json!([{ "snapshot_id": format_snapshot_id(current), "window_id": 555 }])
        );
        // The refusal tells a caller chaining one-shot calls how to keep tokens live.
        let message = structured["refusal"]["message"].as_str().unwrap();
        assert!(message.contains("\"session\":\"run-1\""), "{message}");
        assert!(message.contains("`cua-driver call`"), "{message}");
    }

    #[test]
    fn stale_zoom_is_an_explicit_refusal_with_no_delivery() {
        let cache = SnapshotStore::<Payload>::new();
        let structured = cache
            .zoom(7, Some(9), None)
            .unwrap_err()
            .structured_content
            .unwrap();
        // Legacy top-level fields remain for existing consumers.
        assert_eq!(structured["code"], "zoom_context_missing");
        assert_eq!(structured["pid"], 7);
        assert_eq!(structured["window_id"], 9);
        assert_eq!(structured["status"], "refused");
        assert_eq!(structured["refusal"]["code"], "zoom_context_missing");

        let args = serde_json::json!({
            "pid": 7, "window_id": 9, "x": 1, "y": 2, "from_zoom": true,
            "delivery_mode": "foreground",
        });
        let record =
            crate::action_record::ActionExecutionRecord::from_legacy("click", &args, &structured)
                .unwrap();
        assert_eq!(record.effect, crate::action_record::ActionEffect::Refused);
        assert_eq!(record.actual_delivery, None);
        assert_eq!(record.delivered_count, None);
        assert_eq!(record.refusal.unwrap().code, "zoom_context_missing");
    }

    #[test]
    fn clear_then_publish_starts_clean() {
        let cache = SnapshotStore::new();
        let first = cache.publish(1, 1, Payload(vec![0]));
        assert_eq!(cache.clear(), 1);
        assert_eq!(cache.clear(), 0);
        assert_eq!(
            token_refusal(&cache, 1, &token_for(first, 0))["refusal"]["code"],
            "stale_element_token"
        );
        let second = cache.publish(1, 1, Payload(vec![0]));
        assert!(cache
            .resolve(
                1,
                &serde_json::json!({ "element_token": token_for(second, 0) })
            )
            .is_ok());
    }

    #[test]
    fn missing_token_resolves_to_none() {
        assert!(matches!(
            SnapshotStore::<Payload>::new()
                .resolve(1, &serde_json::json!({}))
                .unwrap(),
            ResolvedElement::None
        ));
    }

    #[test]
    fn runtime_store_discovery_is_weak_and_shared_across_calls() {
        crate::tool::with_runtime_scope("token-discovery-test".into(), || {
            let cache = Arc::new(SnapshotStore::<Payload>::new());
            register_runtime_store(&cache);
            assert!(Arc::ptr_eq(
                &cache,
                &current_runtime_store::<Payload>().unwrap()
            ));
            drop(cache);
            assert!(current_runtime_store::<Payload>().is_none());
        });
    }

    fn refusal_code(result: ToolResult) -> String {
        result.structured_content.unwrap()["code"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn scale(cache: &SnapshotStore<Payload>, window_id: u64, session: &str) -> Option<f64> {
        cache
            .screenshot_context(10, Some(window_id), Some(session))
            .ok()
            .map(|context| context.scale)
    }

    fn zoom_on(snapshot: u32, scale: f64) -> ZoomContext {
        ZoomContext {
            screenshot: ScreenshotContext {
                snapshot_id: snapshot,
                window_id: 20,
                scale,
            },
            origin_x: 100.0,
            origin_y: 50.0,
            scale_inv: 2.0,
        }
    }

    #[test]
    fn screenshot_coordinates_never_borrow_another_sessions_latest_transform() {
        let cache = SnapshotStore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        assert_eq!(scale(&cache, 20, "client-a"), Some(7.35));

        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-b"), Some(1.0));
        assert_eq!(scale(&cache, 20, "client-b"), Some(1.0));
        let refusal = cache
            .screenshot_context(10, Some(20), Some("client-a"))
            .expect_err("stale image coordinates must be refused");
        assert_eq!(refusal_code(refusal), "screenshot_context_missing");
    }

    #[test]
    fn window_pixels_need_a_session_screenshot_unless_marked_native() {
        let cache = SnapshotStore::new();
        let session = |extra: serde_json::Value| {
            let mut args = serde_json::json!({ "_session_id": "client-a" });
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            args
        };
        let refusal = cache
            .screenshot_scale(10, Some(20), &session(serde_json::json!({})))
            .expect_err("pixels without a read are refused");
        assert_eq!(refusal_code(refusal), "screenshot_context_missing");
        let native = session(serde_json::json!({ NATIVE_WINDOW_PIXELS_ARG: true }));
        assert_eq!(cache.screenshot_scale(10, Some(20), &native).unwrap(), 1.0);

        // Native pixels ignore the session's screenshot scale; other calls use it.
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.5));
        assert_eq!(cache.screenshot_scale(10, Some(20), &native).unwrap(), 1.0);
        assert_eq!(
            cache
                .screenshot_scale(10, Some(20), &session(serde_json::json!({})))
                .unwrap(),
            2.5
        );
        // Only the boolean true marks native pixels.
        let refusal = cache
            .screenshot_scale(
                10,
                Some(21),
                &session(serde_json::json!({ NATIVE_WINDOW_PIXELS_ARG: "true" })),
            )
            .expect_err("a non-boolean marker is not native pixels");
        assert_eq!(refusal_code(refusal), "screenshot_context_missing");
    }

    #[test]
    fn screenshot_transforms_are_independent_across_windows() {
        let cache = SnapshotStore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        cache.publish_for_session(10, 21, Payload(vec![]), Some("client-b"), Some(2.0));
        assert_eq!(scale(&cache, 20, "client-a"), Some(7.35));
        assert_eq!(scale(&cache, 21, "client-b"), Some(2.0));
    }

    #[test]
    fn same_session_latest_snapshot_replaces_or_refuses_older_image_context() {
        let cache = SnapshotStore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35));
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0));
        assert_eq!(
            scale(&cache, 20, "client-a"),
            Some(1.0),
            "a newer native capture replaces the older resized frame"
        );

        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), None);
        assert_eq!(
            scale(&cache, 20, "client-a"),
            None,
            "a newer tree-only observation retires the older image frame"
        );
    }

    #[test]
    fn tree_only_read_keeps_the_screenshot_frame_while_the_window_keeps_its_size() {
        let cache = SnapshotStore::new();
        let size = Some((900.0, 680.0));
        cache.publish_sized_for_session(10, 20, Payload(vec![1]), Some("a"), Some(2.0), size);
        cache.publish_sized_for_session(10, 20, Payload(vec![1, 2]), Some("a"), None, size);
        assert_eq!(scale(&cache, 20, "a"), Some(2.0), "same size: frame kept");
        cache.publish_sized_for_session(10, 20, Payload(vec![1]), Some("a"), None, size);
        assert_eq!(
            scale(&cache, 20, "a"),
            Some(2.0),
            "kept across repeated tree reads"
        );

        cache.publish_sized_for_session(
            10,
            20,
            Payload(vec![1]),
            Some("a"),
            None,
            Some((901.0, 680.0)),
        );
        assert_eq!(scale(&cache, 20, "a"), None, "resized: frame retired");

        cache.publish_sized_for_session(10, 20, Payload(vec![1]), Some("a"), Some(1.0), size);
        cache.publish_sized_for_session(10, 20, Payload(vec![1]), Some("b"), None, size);
        assert_eq!(scale(&cache, 20, "a"), None, "another session's tree read");
        assert_eq!(
            scale(&cache, 20, "b"),
            None,
            "never inherits a foreign frame"
        );

        cache.publish_sized_for_session(10, 20, Payload(vec![1]), Some("a"), Some(1.0), size);
        cache.publish_sized_for_session(10, 20, Payload(vec![1]), Some("a"), None, None);
        assert_eq!(scale(&cache, 20, "a"), None, "unknown size: frame retired");
    }

    #[test]
    fn screenshot_only_read_refreshes_the_frame_and_keeps_tokens() {
        let cache = SnapshotStore::new();
        assert_eq!(
            cache.refresh_screenshot_for_session(10, 20, Some("a"), 1.0, None),
            None,
            "nothing to refresh"
        );
        let (id, _) = cache
            .publish_sized_for_session(10, 20, Payload(vec![5, 6, 7]), Some("a"), None, None)
            .unwrap();
        assert_eq!(scale(&cache, 20, "a"), None);
        assert_eq!(
            cache.refresh_screenshot_for_session(10, 20, Some("a"), 2.0, Some((10.0, 10.0))),
            Some(id)
        );
        assert_eq!(scale(&cache, 20, "a"), Some(2.0));
        let token = serde_json::json!({ "element_token": token_for(id, 2) });
        assert!(matches!(
            cache.resolve(10, &token).unwrap(),
            ResolvedElement::Element { element: 7, .. }
        ));

        // A capture-only snapshot has no rows to keep: publish a new one.
        cache.publish_sized_for_session(10, 21, Payload(vec![]), Some("a"), Some(1.0), None);
        assert_eq!(
            cache.refresh_screenshot_for_session(10, 21, Some("a"), 1.0, None),
            None
        );
    }

    #[test]
    fn missing_frame_refusals_say_why() {
        let cache = SnapshotStore::new();
        let reason = |cache: &SnapshotStore<Payload>, window: u64| {
            let refused = cache
                .screenshot_context(10, Some(window), Some("a"))
                .unwrap_err();
            let structured = refused.structured_content.clone().unwrap();
            assert_eq!(structured["code"], "screenshot_context_missing");
            (
                structured["reason"].as_str().unwrap().to_owned(),
                crate::snapshot_test_support::text(&refused),
            )
        };
        let (why, text) = reason(&cache, 20);
        assert_eq!(why, "window_never_read");
        assert!(
            text.contains("get_window_state(pid:10, window_id:20)"),
            "{text}"
        );
        assert!(text.contains("another window's"), "{text}");

        cache.publish_for_session(10, 20, Payload(vec![1]), Some("a"), None);
        assert_eq!(reason(&cache, 20).0, "no_screenshot");

        cache.publish_for_session(10, 20, Payload(vec![1]), Some("b"), Some(1.0));
        assert_eq!(reason(&cache, 20).0, "other_session");
    }

    #[test]
    fn bare_row_number_resolves_against_the_named_windows_current_snapshot() {
        let cache = SnapshotStore::new();
        let first = cache.publish(10, 20, Payload(vec![100, 101, 102]));
        cache.publish(10, 21, Payload(vec![200, 201]));

        let row = serde_json::json!({ "element_token": "2", "window_id": 20 });
        assert!(matches!(
            cache.resolve(10, &row).unwrap(),
            ResolvedElement::Element {
                window_id: 20,
                element_index: 2,
                element: 102
            }
        ));

        // Two windows and no window_id: refused, naming the full tokens.
        let refused = cache
            .resolve(10, &serde_json::json!({ "element_token": "1" }))
            .unwrap_err();
        let text = crate::snapshot_test_support::text(&refused);
        assert!(text.contains("is a row number, not a token"), "{text}");
        assert!(
            text.contains(&format!("\"{}\" (window 20)", token_for(first, 1))),
            "{text}"
        );

        // One snapshot for the pid: the row is unambiguous.
        let only = SnapshotStore::new();
        only.publish(11, 30, Payload(vec![7, 8]));
        assert!(matches!(
            only.resolve(11, &serde_json::json!({ "element_token": "1" }))
                .unwrap(),
            ResolvedElement::Element { element: 8, .. }
        ));

        // Garbage still fails, with the expected shape spelled out.
        let refused = cache
            .resolve(10, &serde_json::json!({ "element_token": "row11" }))
            .unwrap_err();
        let text = crate::snapshot_test_support::text(&refused);
        assert!(text.contains("use <snapshot_id>:<row>"), "{text}");
    }

    #[test]
    fn out_of_range_row_explains_that_rows_belong_to_one_read() {
        let cache = SnapshotStore::new();
        let id = cache.publish(10, 20, Payload(vec![0; 44]));
        let refused = cache
            .resolve(
                10,
                &serde_json::json!({ "element_token": token_for(id, 48) }),
            )
            .unwrap_err();
        let text = crate::snapshot_test_support::text(&refused);
        assert!(
            text.contains("rows [0] to [43] only (44 element(s))"),
            "{text}"
        );
        assert!(text.contains("smaller max_elements"), "{text}");
        assert_eq!(
            refused.structured_content.unwrap()["refusal"]["code"],
            "invalid_element_token"
        );
    }

    #[test]
    fn window_relative_pixels_require_a_current_snapshot() {
        let cache = SnapshotStore::<Payload>::new();
        assert_eq!(scale(&cache, 20, "client-a"), None);
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0));
        cache.remove(10, 20);
        assert_eq!(scale(&cache, 20, "client-a"), None);
    }

    #[test]
    fn window_less_screenshot_context_requires_one_agreed_transform() {
        let cache = SnapshotStore::new();
        assert!(cache
            .screenshot_context(10, None, Some("client-a"))
            .is_err());
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0));
        cache.publish_for_session(10, 21, Payload(vec![]), Some("client-a"), Some(2.0));
        assert_eq!(
            cache
                .screenshot_context(10, None, Some("client-a"))
                .unwrap()
                .scale,
            2.0
        );
        cache.publish_for_session(10, 22, Payload(vec![]), Some("client-a"), Some(3.0));
        assert!(cache
            .screenshot_context(10, None, Some("client-a"))
            .is_err());
    }

    #[test]
    fn publication_reports_replaced_and_evicted_snapshots() {
        let cache = SnapshotStore::new();
        let (first, invalidated) = cache
            .publish_for_session(10, 0, Payload(vec![]), None, None)
            .unwrap();
        assert!(invalidated.is_empty());
        let (second, invalidated) = cache
            .publish_for_session(10, 0, Payload(vec![]), None, None)
            .unwrap();
        assert_eq!(invalidated, vec![first]);
        for window_id in 1..LRU_CAP_PER_PID as u64 {
            cache.publish(10, window_id, Payload(vec![]));
        }
        let (_, invalidated) = cache
            .publish_for_session(10, LRU_CAP_PER_PID as u64, Payload(vec![]), None, None)
            .unwrap();
        assert_eq!(invalidated, vec![second]);
    }

    #[test]
    fn session_retirement_removes_only_snapshots_owned_by_that_session() {
        let cache = SnapshotStore::new();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("ending"), Some(7.35));
        cache.publish_for_session(10, 21, Payload(vec![]), Some("survivor"), Some(2.0));
        assert_eq!(cache.retire_session_screenshots("ending"), 1);
        assert_eq!(scale(&cache, 20, "ending"), None);
        assert_eq!(scale(&cache, 21, "survivor"), Some(2.0));
    }

    #[test]
    fn recreating_an_idle_reclaimed_implicit_session_keeps_its_old_tokens_stale() {
        use crate::session::{
            begin_session_dispatch, evict_idle_with_prefix, register_scoped_session_end_hook,
            SessionClientKind, SessionTransport,
        };
        let session = format!("snapshot-idle-implicit-{}", std::process::id());
        let cache = Arc::new(SnapshotStore::<Payload>::new());
        let retiring = cache.clone();
        let _hook = register_scoped_session_end_hook(move |ended| {
            retiring.retire_session_screenshots(ended);
        });
        let begin = || {
            begin_session_dispatch(
                &session,
                None,
                &session,
                true,
                SessionTransport::McpStdio,
                SessionClientKind::Mcp,
            )
        };
        let resolve =
            |token: &str| cache.resolve(10, &serde_json::json!({ "element_token": token }));

        let guard = begin().expect("first unnamed call starts the session");
        let (snapshot, _) = cache
            .publish_for_session(10, 20, Payload(vec![0, 1]), Some(&session), Some(1.0))
            .expect("live session publishes");
        let token = token_for(snapshot, 1);
        drop(guard);
        assert_eq!(
            evict_idle_with_prefix(std::time::Duration::ZERO, &session),
            std::slice::from_ref(&session)
        );

        let guard = begin().expect("next unnamed call recreates the session");
        assert!(resolve(&token).is_err());
        let (fresh, _) = cache
            .publish_for_session(10, 20, Payload(vec![0, 1]), Some(&session), Some(1.0))
            .expect("recreated session publishes again");
        assert!(matches!(
            resolve(&token_for(fresh, 1)).unwrap(),
            ResolvedElement::Element {
                window_id: 20,
                element: 1,
                ..
            }
        ));
        drop(guard);
        crate::session::end_session(&session);
        crate::session::revive_session(&session);
    }

    #[test]
    fn capture_completing_after_session_end_is_not_published() {
        let cache = SnapshotStore::new();
        let session = format!("snapshot-late-capture-{}", uuid::Uuid::new_v4());
        assert!(crate::session::fire_session_end(&session));
        assert_eq!(
            cache.publish_for_session(10, 20, Payload(vec![]), Some(&session), Some(7.35)),
            None
        );
        assert_eq!(scale(&cache, 20, &session), None);
    }

    #[test]
    fn zoom_context_is_bound_to_snapshot_session_and_window() {
        let cache = SnapshotStore::new();
        let snapshot = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35))
            .unwrap()
            .0;
        let context = zoom_on(snapshot, 7.35);
        cache.set_zoom(10, Some("client-a"), context).unwrap();

        assert_eq!(cache.zoom(10, Some(20), Some("client-a")).unwrap(), context);
        assert_eq!(cache.zoom(10, None, Some("client-a")).unwrap(), context);
        assert_eq!(context.zoom_to_window(3.0, 4.0), (106.0, 58.0));
        for (window_id, session) in [(20, "client-b"), (21, "client-a")] {
            assert_eq!(
                refusal_code(cache.zoom(10, Some(window_id), Some(session)).unwrap_err()),
                "zoom_context_missing"
            );
        }
    }

    #[test]
    fn window_only_screenshot_lookup_requires_one_current_owned_snapshot() {
        let cache = SnapshotStore::new();
        let first = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0))
            .unwrap()
            .0;
        assert_eq!(
            cache
                .screenshot_context_for_zoom(None, 20, Some("client-a"))
                .unwrap(),
            (
                10,
                ScreenshotContext {
                    snapshot_id: first,
                    window_id: 20,
                    scale: 2.0,
                }
            )
        );
        assert_eq!(
            cache
                .screenshot_context_for_zoom(Some(10), 20, Some("client-a"))
                .unwrap()
                .0,
            10
        );
        assert!(cache
            .screenshot_context_for_zoom(None, 20, Some("client-b"))
            .is_err());

        cache.publish_for_session(11, 20, Payload(vec![]), Some("client-a"), Some(1.0));
        assert!(cache
            .screenshot_context_for_zoom(None, 20, Some("client-a"))
            .is_err());
    }

    #[test]
    fn late_zoom_completion_cannot_replace_newer_valid_context() {
        let cache = SnapshotStore::new();
        let snapshot_a = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(2.0))
            .unwrap()
            .0;
        let snapshot_b = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0))
            .unwrap()
            .0;
        let valid_b = zoom_on(snapshot_b, 1.0);
        cache.set_zoom(10, Some("client-a"), valid_b).unwrap();
        assert!(cache
            .set_zoom(10, Some("client-a"), zoom_on(snapshot_a, 2.0))
            .is_err());
        assert_eq!(cache.zoom(10, Some(20), Some("client-a")).unwrap(), valid_b);
    }

    #[test]
    fn newer_snapshot_or_session_end_retires_zoom() {
        let cache = SnapshotStore::new();
        let snapshot = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(7.35))
            .unwrap()
            .0;
        cache
            .set_zoom(10, Some("client-a"), zoom_on(snapshot, 7.35))
            .unwrap();
        cache.publish_for_session(10, 20, Payload(vec![]), Some("client-b"), Some(1.0));
        assert_eq!(
            refusal_code(cache.zoom(10, Some(20), Some("client-a")).unwrap_err()),
            "zoom_context_missing"
        );

        let latest = cache
            .publish_for_session(10, 20, Payload(vec![]), Some("client-a"), Some(1.0))
            .unwrap()
            .0;
        cache
            .set_zoom(10, Some("client-a"), zoom_on(latest, 1.0))
            .unwrap();
        assert_eq!(cache.retire_session_screenshots("client-a"), 1);
        assert!(cache.zoom(10, Some(20), Some("client-a")).is_err());
    }

    struct DropCounter {
        owner: Weak<SnapshotStore<DropCounter>>,
        drops: Arc<AtomicUsize>,
    }
    impl SnapshotPayload for DropCounter {
        type Element = ();
        fn len(&self) -> usize {
            1
        }
        fn retain(&self, index: usize) -> Option<()> {
            (index == 0).then_some(())
        }
    }
    impl Drop for DropCounter {
        fn drop(&mut self) {
            if let Some(owner) = self.owner.upgrade() {
                assert!(
                    owner.inner.try_lock().is_ok(),
                    "native cleanup ran under the storage lock"
                );
            }
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn session_retirement_drops_payload_outside_lock() {
        let cache = Arc::new(SnapshotStore::new());
        let drops = Arc::new(AtomicUsize::new(0));
        cache.publish_for_session(
            10,
            20,
            DropCounter {
                owner: Arc::downgrade(&cache),
                drops: drops.clone(),
            },
            Some("ending"),
            Some(2.0),
        );
        assert_eq!(cache.retire_session_screenshots("ending"), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(cache.inner.lock().unwrap().is_empty());
    }

    #[test]
    fn replacement_remove_and_clear_run_drop_outside_lock() {
        let cache = Arc::new(SnapshotStore::new());
        let drops = Arc::new(AtomicUsize::new(0));
        let payload = || DropCounter {
            owner: Arc::downgrade(&cache),
            drops: drops.clone(),
        };
        cache.publish(1, 1, payload());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        cache.publish(1, 1, payload());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        cache.remove(1, 1);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        cache.publish(1, 1, payload());
        cache.clear();
        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn window_identity_preserves_high_bits_through_resolution_and_retirement() {
        let cache = SnapshotStore::new();
        let low = 7;
        let high = (1_u64 << 32) | low;
        let first = cache.publish(42, low, Payload(vec![10]));
        let second = cache.publish(42, high, Payload(vec![20]));
        let token = serde_json::json!({ "element_token": token_for(second, 0) });
        assert!(matches!(
            cache.resolve(42, &token).unwrap(),
            ResolvedElement::Element { window_id, element: 20, .. } if window_id == high
        ));
        cache.remove(42, high);
        assert!(cache.resolve(42, &token).is_err());
        assert!(matches!(
            cache
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(first, 0) })
                )
                .unwrap(),
            ResolvedElement::Element { window_id: 7, .. }
        ));
    }

    #[test]
    fn bindings_sharing_a_scope_keep_independent_payload_ownership() {
        crate::tool::with_runtime_scope("snapshot-binding-ownership".into(), || {
            let first = Arc::new(SnapshotStore::new());
            let second = Arc::new(SnapshotStore::new());
            register_runtime_store(&first);
            register_runtime_store(&second);
            let first_id = first.publish(42, 7, Payload(vec![10]));
            let second_id = second.publish(42, 7, Payload(vec![20]));
            assert!(second
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(first_id, 0) })
                )
                .is_err());
            drop(first);
            let resolved = second
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(second_id, 0) }),
                )
                .unwrap();
            assert!(matches!(
                resolved,
                ResolvedElement::Element { element: 20, .. }
            ));
            assert!(Arc::ptr_eq(
                &current_runtime_store::<Payload>().unwrap(),
                &second
            ));
            retire_runtime_scope("snapshot-binding-ownership");
        });
    }

    #[test]
    fn recording_discovery_does_not_extend_payload_lifetime() {
        crate::tool::with_runtime_scope("snapshot-weak-discovery".into(), || {
            let cache = Arc::new(SnapshotStore::new());
            let drops = Arc::new(AtomicUsize::new(0));
            cache.publish(
                42,
                7,
                DropCounter {
                    owner: Arc::downgrade(&cache),
                    drops: drops.clone(),
                },
            );
            register_runtime_store(&cache);
            assert_eq!(Arc::strong_count(&cache), 1);
            drop(cache);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            assert!(current_runtime_store::<DropCounter>().is_none());
            {
                let caches = runtime_stores().lock().unwrap();
                assert!(!caches.contains_key("snapshot-weak-discovery"));
                if caches.is_empty() {
                    assert_eq!(caches.capacity(), 0);
                }
            }
            assert_eq!(retire_runtime_scope("snapshot-weak-discovery"), 0);
        });
    }

    #[test]
    fn default_impl_matches_new() {
        let _cache: SnapshotStore<Payload> = SnapshotStore::default();
    }
}
