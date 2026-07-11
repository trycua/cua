//! Opt-in compatibility surface for Codex Computer Use on macOS.
//!
//! The native cua-driver API is pid/window oriented and intentionally broad.
//! Codex Computer Use exposes a smaller app-oriented contract. These wrappers
//! preserve the native implementation underneath while providing that narrow
//! contract, a per-MCP-session app snapshot, automatic post-action refreshes,
//! and a centralized fail-closed target policy.

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::runloop::{
    kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRef,
    CFRunLoopRemoveSource, CFRunLoopRunInMode, CFRunLoopSourceRef, CFRunLoopStop,
};
use core_foundation::string::CFString;
use cua_driver_core::{
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef, ToolRegistry},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

use super::ToolState;
use crate::ax::bindings::{
    copy_action_names, copy_string_attr, element_screen_center, perform_action,
    AXUIElementCopyAttributeValue, AXUIElementCreateApplication, AXUIElementRef,
    AXUIElementSetAttributeValue, AXValueRef, _AXUIElementGetWindow, kAXErrorSuccess,
    kAXValueCFRangeType,
};
use crate::session::{SessionLockEpoch, SessionLockError, SessionLockGuardian};

const ANONYMOUS_SESSION: &str = "__codex_compat_connection__";
const MAX_CLICK_COUNT: u64 = 10;
const MAX_SCROLL_PAGES: f64 = 10.0;
const WINDOW_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

#[repr(C)]
struct __AXObserver(c_void);
type AXObserverRef = *mut __AXObserver;
type AXObserverCallback = extern "C" fn(
    observer: AXObserverRef,
    element: AXUIElementRef,
    notification: core_foundation::string::CFStringRef,
    context: *mut c_void,
);

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXObserverCreate(
        application: i32,
        callback: AXObserverCallback,
        observer: *mut AXObserverRef,
    ) -> crate::ax::bindings::AXError;
    fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: core_foundation::string::CFStringRef,
        context: *mut c_void,
    ) -> crate::ax::bindings::AXError;
    fn AXObserverRemoveNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: core_foundation::string::CFStringRef,
    ) -> crate::ax::bindings::AXError;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
}

#[derive(Clone, Copy, Debug)]
enum CompatKind {
    ListApps,
    GetAppState,
    Click,
    Drag,
    PerformSecondaryAction,
    PressKey,
    Scroll,
    SelectText,
    SetValue,
    TypeText,
}

impl CompatKind {
    const ALL: [Self; 10] = [
        Self::ListApps,
        Self::GetAppState,
        Self::Click,
        Self::PerformSecondaryAction,
        Self::SetValue,
        Self::SelectText,
        Self::Scroll,
        Self::Drag,
        Self::PressKey,
        Self::TypeText,
    ];

    fn index(self) -> usize {
        match self {
            Self::ListApps => 0,
            Self::GetAppState => 1,
            Self::Click => 2,
            Self::Drag => 3,
            Self::PerformSecondaryAction => 4,
            Self::PressKey => 5,
            Self::Scroll => 6,
            Self::SelectText => 7,
            Self::SetValue => 8,
            Self::TypeText => 9,
        }
    }
}

struct CompatTool {
    kind: CompatKind,
    state: Arc<CompatState>,
}

#[async_trait]
impl Tool for CompatTool {
    fn def(&self) -> &ToolDef {
        &defs()[self.kind.index()]
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        match self.kind {
            CompatKind::ListApps => self.state.list_apps().await,
            CompatKind::GetAppState => self.state.get_app_state(&args).await,
            kind => self.state.run_action(kind, args).await,
        }
    }
}

pub(super) fn register_all(registry: &mut ToolRegistry) {
    let state = Arc::new(CompatState::new());
    register_session_cleanup(&state);
    register_lock_cleanup(&state);

    for kind in CompatKind::ALL {
        registry.register(Box::new(CompatTool {
            kind,
            state: state.clone(),
        }));
    }
}

fn register_session_cleanup(state: &Arc<CompatState>) {
    let snapshots = state.snapshots.clone();
    let native_sessions = state.native_sessions.clone();
    let operation_locks = state.operation_locks.clone();
    let lock_removed_cursors = state.lock_removed_cursors.clone();
    cua_driver_core::session::register_session_end_hook(move |session_id| {
        snapshots.clear_session(session_id);
        let native = native_sessions.lock().unwrap().remove(session_id);
        if let Some(native) = native {
            native.session_config.clear(session_id);
            native.cursor_registry.remove(session_id);
        }
        crate::cursor::overlay::remove_cursor(session_id.to_owned());
        lock_removed_cursors.lock().unwrap().remove(session_id);
        operation_locks.lock().unwrap().remove(session_id);
    });
}

fn register_lock_cleanup(state: &Arc<CompatState>) {
    let weak = Arc::downgrade(state);
    state.guardian.register_lock_hook(move || {
        if let Some(state) = weak.upgrade() {
            state.invalidate_for_session_lock();
        }
    });
}

fn defs() -> &'static [ToolDef] {
    static DEFS: OnceLock<Vec<ToolDef>> = OnceLock::new();
    DEFS.get_or_init(|| {
        vec![
            tool_def(
                "list_apps",
                "List the apps on this computer. Returns the set of apps that are currently running, as well as any that have been used in the last 14 days, including details on usage frequency",
                json!({"type":"object","properties":{},"additionalProperties":false}),
                true,
            ),
            tool_def(
                "get_app_state",
                "Start an app use session if needed, then get the state of the app's key window and return a screenshot and accessibility tree. This must be called once per assistant turn before interacting with the app",
                json!({
                    "type":"object",
                    "required":["app"],
                    "properties": {"app": app_schema("App name, full app path, or unambiguous bundle identifier")},
                    "additionalProperties":false
                }),
                true,
            ),
            tool_def(
                "click",
                "Click an element by index or pixel coordinates from screenshot",
                json!({
                    "type":"object",
                    "required":["app"],
                    "properties": {
                        "app": app_schema("App name, full app path, or unambiguous bundle identifier"),
                        "element_index":{"type":"string","description":"Element index to click"},
                        "x":{"type":"number","description":"X coordinate in screenshot pixel coordinates"},
                        "y":{"type":"number","description":"Y coordinate in screenshot pixel coordinates"},
                        "click_count":{"type":"integer","description":"Number of clicks. Defaults to 1"},
                        "mouse_button":{"type":"string","enum":["left","right","middle"],"description":"Mouse button to click. Defaults to left."}
                    },
                    "additionalProperties":false
                }),
                false,
            ),
            tool_def(
                "drag",
                "Drag from one point to another using pixel coordinates",
                json!({
                    "type":"object",
                    "required":["app","from_x","from_y","to_x","to_y"],
                    "properties": {
                        "app": app_schema("App name, full app path, or unambiguous bundle identifier"),
                        "from_x":{"type":"number","description":"Start X coordinate"},
                        "from_y":{"type":"number","description":"Start Y coordinate"},
                        "to_x":{"type":"number","description":"End X coordinate"},
                        "to_y":{"type":"number","description":"End Y coordinate"}
                    },
                    "additionalProperties":false
                }),
                false,
            ),
            tool_def(
                "perform_secondary_action",
                "Invoke a secondary accessibility action exposed by an element",
                json!({
                    "type":"object",
                    "required":["app","element_index","action"],
                    "properties": {
                        "app": app_schema("App name, full app path, or unambiguous bundle identifier"),
                        "element_index":{"type":"string","description":"Element identifier"},
                        "action":{"type":"string","description":"Secondary accessibility action name"}
                    },
                    "additionalProperties":false
                }),
                false,
            ),
            tool_def(
                "press_key",
                "Press a key or key-combination on the keyboard, including modifier and navigation keys.\n  - This supports xdotool's `key` syntax.\n  - Examples: \"a\", \"Return\", \"Tab\", \"super+c\", \"Up\", \"KP_0\" (for the numpad 0 key).",
                json!({
                    "type":"object",
                    "required":["app","key"],
                    "properties": {
                        "app": app_schema("App name, full app path, or unambiguous bundle identifier"),
                        "key":{"type":"string","description":"Key or key combination to press"}
                    },
                    "additionalProperties":false
                }),
                false,
            ),
            tool_def(
                "scroll",
                "Scroll an element in a direction by a number of pages",
                json!({
                    "type":"object",
                    "required":["app","element_index","direction"],
                    "properties": {
                        "app": app_schema("App name, full app path, or unambiguous bundle identifier"),
                        "element_index":{"type":"string","description":"Element identifier"},
                        "direction":{"type":"string","description":"Scroll direction: up, down, left, or right"},
                        "pages":{"type":"number","description":"Number of pages to scroll. Fractional values are supported. Defaults to 1"}
                    },
                    "additionalProperties":false
                }),
                false,
            ),
            tool_def(
                "select_text",
                "Select text inside a text element, or place the text cursor before or after it. Provide text exactly as it appears in the accessibility tree, including any Markdown formatting. If the text is not unique, provide surrounding prefix or suffix text to disambiguate it.",
                json!({
                    "type":"object",
                    "required":["app","element_index","text"],
                    "properties": {
                        "app": app_schema("App name or bundle identifier"),
                        "element_index":{"type":"string","description":"Text element identifier"},
                        "text":{"type":"string","description":"Target text as shown in the accessibility tree"},
                        "prefix":{"type":"string","description":"Optional text immediately before the target, used to disambiguate repeated matches"},
                        "suffix":{"type":"string","description":"Optional text immediately after the target, used to disambiguate repeated matches"},
                        "selection":{"type":"string","enum":["text","cursor_before","cursor_after"],"description":"Whether to select the text or place the cursor before or after it. Defaults to text."}
                    },
                    "additionalProperties":false
                }),
                false,
            ),
            tool_def(
                "set_value",
                "Set the value of a settable accessibility element",
                json!({
                    "type":"object",
                    "required":["app","element_index","value"],
                    "properties": {
                        "app": app_schema("App name, full app path, or unambiguous bundle identifier"),
                        "element_index":{"type":"string","description":"Element identifier"},
                        "value":{"type":"string","description":"Value to assign"}
                    },
                    "additionalProperties":false
                }),
                false,
            ),
            tool_def(
                "type_text",
                "Type literal text using keyboard input",
                json!({
                    "type":"object",
                    "required":["app","text"],
                    "properties": {
                        "app": app_schema("App name, full app path, or unambiguous bundle identifier"),
                        "text":{"type":"string","description":"Literal text to type"}
                    },
                    "additionalProperties":false
                }),
                false,
            ),
        ]
    })
}

fn app_schema(description: &str) -> Value {
    json!({"type":"string","description":description})
}

fn tool_def(name: &str, description: &str, input_schema: Value, read_only: bool) -> ToolDef {
    ToolDef {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema,
        read_only,
        destructive: false,
        idempotent: read_only,
        open_world: false,
    }
}

#[derive(Clone, Debug)]
struct AppIdentity {
    requested: String,
    name: String,
    bundle_id: Option<String>,
    launch_path: Option<String>,
    pid: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppResolveMode {
    LaunchIfNeeded,
    RunningOnly,
}

impl AppIdentity {
    fn aliases(&self) -> Vec<String> {
        let mut aliases = vec![self.requested.clone(), self.name.clone()];
        if let Some(bundle_id) = &self.bundle_id {
            aliases.push(bundle_id.clone());
        }
        if let Some(path) = &self.launch_path {
            aliases.push(path.clone());
        }
        aliases
    }
}

#[derive(Clone)]
struct AppSnapshot {
    app: AppIdentity,
    window_id: u32,
    window_title: String,
    generation: u64,
    element_indices: HashSet<String>,
    /// Compatibility screenshots are normalized to logical window points.
    /// Native tools still consume the PNG pixel coordinate space, so actions
    /// multiply by these factors before delegating.
    native_pixels_per_compat_x: f64,
    native_pixels_per_compat_y: f64,
    logical_width: u32,
    logical_height: u32,
    /// Each MCP session owns its own native element cache. A snapshot taken by
    /// session B therefore cannot replace the AX handles session A will use.
    native: Arc<ToolState>,
}

#[derive(Default)]
struct SnapshotStoreInner {
    by_session: HashMap<String, HashMap<String, AppSnapshot>>,
    latest_by_window: HashMap<(String, i32, u32), u64>,
}

#[derive(Default)]
struct SnapshotStore {
    inner: Mutex<SnapshotStoreInner>,
    next_generation: AtomicU64,
}

#[derive(Debug)]
enum SnapshotLookupError {
    Missing,
    Stale,
}

impl SnapshotStore {
    fn insert(&self, session: &str, mut snapshot: AppSnapshot) -> AppSnapshot {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        snapshot.generation = generation;
        let aliases = snapshot.app.aliases();
        let mut inner = self.inner.lock().unwrap();
        inner.latest_by_window.insert(
            (session.to_owned(), snapshot.app.pid, snapshot.window_id),
            generation,
        );
        let session_map = inner.by_session.entry(session.to_owned()).or_default();
        for alias in aliases {
            session_map.insert(normalize_app_ref(&alias), snapshot.clone());
        }
        snapshot
    }

    fn lookup(&self, session: &str, app: &str) -> Result<AppSnapshot, SnapshotLookupError> {
        let inner = self.inner.lock().unwrap();
        let snapshot = inner
            .by_session
            .get(session)
            .and_then(|apps| apps.get(&normalize_app_ref(app)))
            .cloned()
            .ok_or(SnapshotLookupError::Missing)?;
        let latest = inner
            .latest_by_window
            .get(&(session.to_owned(), snapshot.app.pid, snapshot.window_id))
            .copied();
        if latest != Some(snapshot.generation) {
            return Err(SnapshotLookupError::Stale);
        }
        Ok(snapshot)
    }

    fn invalidate(&self, session: &str, snapshot: &AppSnapshot) {
        let mut inner = self.inner.lock().unwrap();
        let key = (session.to_owned(), snapshot.app.pid, snapshot.window_id);
        if inner.latest_by_window.get(&key) == Some(&snapshot.generation) {
            inner.latest_by_window.remove(&key);
        }
    }

    fn is_current(&self, session: &str, snapshot: &AppSnapshot) -> bool {
        self.inner.lock().unwrap().latest_by_window.get(&(
            session.to_owned(),
            snapshot.app.pid,
            snapshot.window_id,
        )) == Some(&snapshot.generation)
    }

    fn clear_session(&self, session: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.by_session.remove(session);
        inner
            .latest_by_window
            .retain(|(owner, _, _), _| owner != session);
    }

    fn clear_all(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.by_session.clear();
        inner.latest_by_window.clear();
    }
}

struct CompatState {
    snapshots: Arc<SnapshotStore>,
    native_sessions: Arc<Mutex<HashMap<String, Arc<ToolState>>>>,
    operation_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    lock_removed_cursors: Arc<Mutex<HashSet<String>>>,
    guardian: Arc<SessionLockGuardian>,
}

impl CompatState {
    fn new() -> Self {
        Self::with_guardian(crate::session::lock_guardian().clone())
    }

    fn with_guardian(guardian: Arc<SessionLockGuardian>) -> Self {
        Self {
            snapshots: Arc::new(SnapshotStore::default()),
            native_sessions: Arc::new(Mutex::new(HashMap::new())),
            operation_locks: Arc::new(Mutex::new(HashMap::new())),
            lock_removed_cursors: Arc::new(Mutex::new(HashSet::new())),
            guardian,
        }
    }

    fn invalidate_for_session_lock(&self) {
        self.snapshots.clear_all();
        let native_sessions = {
            let mut sessions = self.native_sessions.lock().unwrap();
            std::mem::take(&mut *sessions)
        };
        let mut removed = self.lock_removed_cursors.lock().unwrap();
        for (session_id, native) in native_sessions {
            native.session_config.clear(&session_id);
            native.cursor_registry.remove(&session_id);
            removed.insert(session_id);
        }
        let cursor_ids = removed.iter().cloned().collect::<Vec<_>>();
        drop(removed);
        for session_id in cursor_ids {
            // Re-sending Remove is deliberate. It closes the race where a
            // state capture queued Revive immediately after the lifecycle
            // hook's first Remove but observed the new lock at its final gate.
            crate::cursor::overlay::remove_cursor(session_id);
        }
    }

    fn begin_lock_epoch(&self) -> Result<SessionLockEpoch, CompatError> {
        self.reconcile_direct_lock_probe()?;
        self.guardian.begin().map_err(lock_guard_error)
    }

    fn validate_lock_epoch(&self, epoch: SessionLockEpoch) -> Result<(), CompatError> {
        let result = self
            .reconcile_direct_lock_probe()
            .and_then(|()| self.guardian.validate(epoch).map_err(lock_guard_error));
        if result.is_err() {
            // A native state may have been lazily created after the lifecycle
            // hook drained the maps. Repeating cleanup is idempotent and closes
            // that race before the guarded operation returns.
            self.invalidate_for_session_lock();
        }
        result
    }

    fn reconcile_direct_lock_probe(&self) -> Result<(), CompatError> {
        let Some(locked) = crate::session::current_screen_locked() else {
            self.invalidate_for_session_lock();
            return Err(CompatError::new(
                "session_state_unavailable",
                "Computer Use could not verify the current macOS login session. Refusing to inspect or control apps until an unlocked GUI session is available.",
            ));
        };
        self.guardian.observe(locked);
        if locked {
            return Err(lock_guard_error(SessionLockError::Locked));
        }
        Ok(())
    }

    fn revive_cursor_after_fresh_state(&self, session: &str) -> bool {
        let should_revive = self.lock_removed_cursors.lock().unwrap().contains(session);
        if should_revive
            && !cua_driver_core::session::is_session_ended(session)
        {
            crate::cursor::overlay::revive_cursor(session.to_owned());
        }
        should_revive
    }

    fn require_current_lock_epoch(&self, epoch: SessionLockEpoch) -> Result<(), ToolResult> {
        self.validate_lock_epoch(epoch)
            .map_err(CompatError::into_result)
    }

    fn native_for_session(&self, session: &str) -> Arc<ToolState> {
        let mut sessions = self.native_sessions.lock().unwrap();
        sessions
            .entry(session.to_owned())
            .or_insert_with(|| Arc::new(ToolState::default()))
            .clone()
    }

    fn operation_lock(&self, session: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut sessions = self.operation_locks.lock().unwrap();
        sessions
            .entry(session.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    async fn list_apps(&self) -> ToolResult {
        let apps = tokio::task::spawn_blocking(crate::apps::list_all_apps)
            .await
            .unwrap_or_default();
        let apps = tokio::task::spawn_blocking(move || recent_apps_with_usage(apps))
            .await
            .unwrap_or_default();

        let mut lines = Vec::with_capacity(apps.len());
        for app in &apps {
            let path = app
                .info
                .launch_path
                .as_deref()
                .map(with_trailing_slash)
                .unwrap_or_else(|| "unknown path".to_owned());
            let bundle = app.info.bundle_id.as_deref().unwrap_or("unknown bundle");
            let mut flags = Vec::new();
            if app.info.active {
                flags.push("frontmost".to_owned());
            }
            if app.info.running {
                flags.push("running".to_owned());
            }
            if let Some(last_used) = app.last_used.as_deref() {
                flags.push(format!(
                    "last-used={}",
                    last_used.get(..10).unwrap_or(last_used)
                ));
            }
            if let Some(use_count) = app.use_count {
                flags.push(format!("uses={use_count}"));
            }
            lines.push(format!(
                "{} — {} — {}{}",
                app.info.name,
                path,
                bundle,
                if flags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", flags.join(", "))
                }
            ));
        }

        let structured = json!({
            "apps": apps.iter().map(|app| json!({
                "name": app.info.name,
                "path": app.info.launch_path,
                "bundle_id": app.info.bundle_id,
                "pid": app.info.pid,
                "running": app.info.running,
                "frontmost": app.info.active,
                "last_used": app.last_used,
                "use_count": app.use_count,
            })).collect::<Vec<_>>()
        });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }

    async fn get_app_state(&self, args: &Value) -> ToolResult {
        let app_ref = match required_string(args, "app") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let session = session_key(args);
        let operation_lock = self.operation_lock(&session);
        let _operation = operation_lock.lock().await;
        self.capture_app_state(&app_ref, &session, AppResolveMode::LaunchIfNeeded)
            .await
    }

    async fn capture_app_state(
        &self,
        app_ref: &str,
        session: &str,
        resolve_mode: AppResolveMode,
    ) -> ToolResult {
        let lock_epoch = match self.begin_lock_epoch() {
            Ok(epoch) => epoch,
            Err(error) => return error.into_result(),
        };
        let app = match resolve_or_launch_app(app_ref, resolve_mode).await {
            Ok(app) => app,
            Err(error) => return error.into_result(),
        };
        if let Err(error) = self.validate_lock_epoch(lock_epoch) {
            return error.into_result();
        }
        if let Err(error) = enforce_target_policy(&app) {
            return error.into_result();
        }

        let (window_id, window_title) = match wait_for_key_window(app.pid).await {
            Ok(window) => window,
            Err(error) => return error.into_result(),
        };
        if let Err(error) = self.validate_lock_epoch(lock_epoch) {
            return error.into_result();
        }

        let native = self.native_for_session(session);
        let native_args = json!({
            "pid": app.pid,
            "window_id": window_id,
            "_session_id": session,
            "include_screenshot": true,
            "max_elements": 800,
            "max_depth": 20,
            "_codex_compat_full_ax_map": true,
        });
        let native_result = match run_guarded_capture(&self.guardian, lock_epoch, || async {
            super::get_window_state::GetWindowStateTool::new(native.clone())
                .invoke(native_args)
                .await
        })
        .await
        {
            Ok(result) => result,
            Err(error) => {
                self.invalidate_for_session_lock();
                return lock_guard_error(error).into_result();
            }
        };
        if let Err(error) = self.validate_lock_epoch(lock_epoch) {
            return error.into_result();
        }
        if native_result.is_error == Some(true) {
            return compat_error_from_native("app_state_failed", native_result);
        }
        if !native_result
            .content
            .iter()
            .any(|content| matches!(content, Content::Image { .. }))
        {
            return CompatError::new(
                "screen_capture_failed",
                "The accessibility tree was available, but the key-window screenshot failed. Grant Screen Recording to the embedding host and call get_app_state again.",
            )
            .into_result();
        }

        let structured = native_result
            .structured_content
            .clone()
            .unwrap_or_else(|| json!({}));
        let element_indices = element_index_set(&structured);
        let native_width = structured
            .get("screenshot_width")
            .and_then(Value::as_u64)
            .unwrap_or(1) as f64;
        let native_height = structured
            .get("screenshot_height")
            .and_then(Value::as_u64)
            .unwrap_or(1) as f64;
        let bounds = crate::windows::window_bounds_by_id(window_id);
        let logical_width = bounds
            .as_ref()
            .map(|bounds| bounds.width.round().max(1.0) as u32)
            .unwrap_or(native_width.max(1.0) as u32);
        let logical_height = bounds
            .as_ref()
            .map(|bounds| bounds.height.round().max(1.0) as u32)
            .unwrap_or(native_height.max(1.0) as u32);
        if let Err(error) = self.validate_lock_epoch(lock_epoch) {
            return error.into_result();
        }
        let snapshot = self.snapshots.insert(
            session,
            AppSnapshot {
                app,
                window_id,
                window_title,
                generation: 0,
                element_indices,
                native_pixels_per_compat_x: native_width / logical_width as f64,
                native_pixels_per_compat_y: native_height / logical_height as f64,
                logical_width,
                logical_height,
                native,
            },
        );
        let revived_cursor = self.revive_cursor_after_fresh_state(session);
        if let Err(error) = self.validate_lock_epoch(lock_epoch) {
            return error.into_result();
        }
        if revived_cursor {
            self.lock_removed_cursors.lock().unwrap().remove(session);
        }
        let result = match state_result(native_result, &snapshot) {
            Ok(result) => result,
            Err(error) => error.into_result(),
        };
        if let Err(error) = self.validate_lock_epoch(lock_epoch) {
            return error.into_result();
        }
        result
    }

    async fn run_action(&self, kind: CompatKind, args: Value) -> ToolResult {
        let app_ref = match required_string(&args, "app") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let session = session_key(&args);
        // Capture the generation before waiting for this session's operation
        // lane. If another state read or action wins the lane first, the
        // generation check below rejects these now-stale coordinates instead
        // of silently applying them to the newer native element cache.
        let snapshot = match self.snapshots.lookup(&session, &app_ref) {
            Ok(snapshot) => snapshot,
            Err(SnapshotLookupError::Missing) => {
                return CompatError::new(
                    "no_active_app_session",
                    format!(
                        "No current app snapshot for '{app_ref}'. Call get_app_state for this app before acting."
                    ),
                )
                .into_result()
            }
            Err(SnapshotLookupError::Stale) => {
                return CompatError::new(
                    "stale_app_state",
                    format!(
                        "The app snapshot for '{app_ref}' is stale. Call get_app_state again before acting."
                    ),
                )
                .into_result()
            }
        };
        let operation_lock = self.operation_lock(&session);
        let _operation = operation_lock.lock().await;
        if !self.snapshots.is_current(&session, &snapshot) {
            return CompatError::new(
                "stale_app_state",
                format!(
                    "The app snapshot for '{app_ref}' changed while this action was waiting. Call get_app_state again before acting."
                ),
            )
            .into_result();
        }

        let lock_epoch = match self.begin_lock_epoch() {
            Ok(epoch) => epoch,
            Err(error) => return error.into_result(),
        };
        if let Err(error) = validate_live_snapshot(&snapshot) {
            return error.into_result();
        }
        if let Err(error) = self.validate_lock_epoch(lock_epoch) {
            return error.into_result();
        }

        let action_name = defs()[kind.index()].name.clone();
        let action_result = match run_guarded_dispatch(&self.guardian, lock_epoch, || async {
            match kind {
                CompatKind::Click => self.click(&snapshot, &args, &session, lock_epoch).await,
                CompatKind::Drag => self.drag(&snapshot, &args, &session, lock_epoch).await,
                CompatKind::PerformSecondaryAction => {
                    self.perform_secondary_action(&snapshot, &args, lock_epoch)
                        .await
                }
                CompatKind::PressKey => {
                    self.press_key(&snapshot, &args, &session, lock_epoch)
                        .await
                }
                CompatKind::Scroll => {
                    self.scroll(&snapshot, &args, &session, lock_epoch)
                        .await
                }
                CompatKind::SelectText => self.select_text(&snapshot, &args, lock_epoch).await,
                CompatKind::SetValue => {
                    self.set_value(&snapshot, &args, &session, lock_epoch)
                        .await
                }
                CompatKind::TypeText => {
                    self.type_text(&snapshot, &args, &session, lock_epoch)
                        .await
                }
                CompatKind::ListApps | CompatKind::GetAppState => unreachable!(),
            }
        })
        .await
        {
            Ok(result) => result,
            Err(error) => {
                self.invalidate_for_session_lock();
                return lock_guard_error(error).into_result();
            }
        };

        let action_lock_status = self.validate_lock_epoch(lock_epoch);
        if action_result.is_error == Some(true) {
            if let Err(error) = action_lock_status {
                return error.into_result();
            }
            return action_error(&action_name, action_result);
        }

        // The action may have changed layout, focus, or window identity. Retire
        // its input snapshot before attempting perception refresh so a failed
        // screenshot cannot leave stale element handles reusable.
        self.snapshots.invalidate(&session, &snapshot);
        let action_summary = first_text(&action_result);
        let action_structured = action_result.structured_content.clone();
        if let Err(error) = action_lock_status {
            return refresh_warning_after_success(
                &action_name,
                action_summary,
                action_structured,
                error.into_result(),
            );
        }
        let mut refreshed = self
            .capture_app_state(&app_ref, &session, AppResolveMode::RunningOnly)
            .await;
        if refreshed.is_error == Some(true) {
            return refresh_warning_after_success(
                &action_name,
                action_summary,
                action_structured,
                refreshed,
            );
        }
        if let Some(structured) = refreshed.structured_content.as_mut() {
            structured["action"] = json!({
                "name": action_name,
                "result": action_summary,
            });
        }
        refreshed
    }

    async fn click(
        &self,
        snapshot: &AppSnapshot,
        args: &Value,
        session: &str,
        lock_epoch: SessionLockEpoch,
    ) -> ToolResult {
        let element_index = args.get("element_index").and_then(Value::as_str);
        let x = args.get("x").and_then(Value::as_f64);
        let y = args.get("y").and_then(Value::as_f64);
        if x.is_some() != y.is_some() {
            return ToolResult::error("click requires both x and y when using pixel coordinates.");
        }
        if element_index.is_some() == x.is_some() {
            return ToolResult::error(
                "click requires exactly one target: element_index or the x/y coordinate pair.",
            );
        }
        let click_count = match validated_click_count(args) {
            Ok(count) => count,
            Err(result) => return result,
        };
        let button = args
            .get("mouse_button")
            .and_then(Value::as_str)
            .unwrap_or("left");
        if !matches!(button, "left" | "right" | "middle") {
            return ToolResult::error("mouse_button must be left, right, or middle.");
        }

        let mut native_args = json!({
            "pid": snapshot.app.pid,
            "window_id": snapshot.window_id,
            "button": button,
            "count": click_count,
            "_session_id": session,
        });
        let mut uses_ax_element = false;
        if let Some(index) = element_index {
            let native_index = match snapshot_element_index(snapshot, index) {
                Ok(index) => index,
                Err(result) => return result,
            };
            if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
                return result;
            }
            let fallback = element_click_pixel_fallback(
                snapshot,
                native_index,
                button,
                self.guardian.clone(),
                lock_epoch,
            )
            .await;
            if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
                return result;
            }
            match fallback {
                Ok(Some((native_x, native_y))) => {
                    native_args["x"] = json!(native_x);
                    native_args["y"] = json!(native_y);
                }
                Ok(None) => {
                    native_args["element_index"] = json!(native_index);
                    uses_ax_element = true;
                }
                Err(result) => return result,
            }
        } else {
            let compat_x = x.unwrap();
            let compat_y = y.unwrap();
            if let Err(result) = validate_compat_point(snapshot, compat_x, compat_y, "click") {
                return result;
            }
            let (native_x, native_y) = compat_point_to_native(snapshot, compat_x, compat_y);
            native_args["x"] = json!(native_x);
            native_args["y"] = json!(native_y);
        }

        // Native AX clicks are single-press operations. Repeat explicitly when
        // Codex asks for a multi-click so the compatibility field is honored on
        // both AX and pixel paths (the native pixel path handles count itself).
        if uses_ax_element && click_count > 1 {
            let mut last = ToolResult::default();
            for _ in 0..click_count {
                if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
                    return result;
                }
                last = super::click::ClickTool::new(snapshot.native.clone())
                    .invoke(native_args.clone())
                    .await;
                if last.is_error == Some(true) {
                    return last;
                }
            }
            last
        } else {
            if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
                return result;
            }
            super::click::ClickTool::new(snapshot.native.clone())
                .invoke(native_args)
                .await
        }
    }

    async fn drag(
        &self,
        snapshot: &AppSnapshot,
        args: &Value,
        session: &str,
        lock_epoch: SessionLockEpoch,
    ) -> ToolResult {
        let from_x = match required_number(args, "from_x") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let from_y = match required_number(args, "from_y") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let to_x = match required_number(args, "to_x") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let to_y = match required_number(args, "to_y") {
            Ok(value) => value,
            Err(result) => return result,
        };
        for (label, x, y) in [("drag start", from_x, from_y), ("drag end", to_x, to_y)] {
            if let Err(result) = validate_compat_point(snapshot, x, y, label) {
                return result;
            }
        }
        let (from_x, from_y) = compat_point_to_native(snapshot, from_x, from_y);
        let (to_x, to_y) = compat_point_to_native(snapshot, to_x, to_y);
        if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
            return result;
        }
        super::drag::DragTool::new(snapshot.native.clone())
            .invoke(json!({
                "pid": snapshot.app.pid,
                "window_id": snapshot.window_id,
                "from_x": from_x,
                "from_y": from_y,
                "to_x": to_x,
                "to_y": to_y,
                "_session_id": session,
            }))
            .await
    }

    async fn perform_secondary_action(
        &self,
        snapshot: &AppSnapshot,
        args: &Value,
        lock_epoch: SessionLockEpoch,
    ) -> ToolResult {
        let index = match required_string(args, "element_index") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let action = match required_string(args, "action") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let native_index = match snapshot_element_index(snapshot, &index) {
            Ok(index) => index,
            Err(result) => return result,
        };
        let guard = match snapshot.native.element_cache.get_element_retained(
            snapshot.app.pid,
            snapshot.window_id,
            native_index,
        ) {
            Some(guard) => guard,
            None => {
                return CompatError::new(
                    "stale_app_state",
                    "The cached accessibility element is stale. Call get_app_state again.",
                )
                .into_result()
            }
        };
        let element_ptr = guard.as_ptr();
        let prior_frontmost = crate::apps::frontmost_pid();
        let window_snapshot =
            crate::window_change_detector::WindowChangeDetector::snapshot(prior_frontmost);
        if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
            return result;
        }
        let guardian = self.guardian.clone();
        let result = with_compat_focus_guard(
            snapshot.app.pid,
            prior_frontmost,
            "codex_compat.perform_secondary_action",
            || async move {
                tokio::task::spawn_blocking(move || {
                    dispatch_if_epoch_current(&guardian, lock_epoch, || {
                        let available =
                            unsafe { copy_action_names(element_ptr as AXUIElementRef) };
                        let requested = canonical_ax_action(&action);
                        let actual = available
                            .iter()
                            .find(|candidate| candidate.eq_ignore_ascii_case(&requested))
                            .cloned()
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "Element does not expose secondary action '{action}'. Available actions: {}",
                                    if available.is_empty() {
                                        "none".to_owned()
                                    } else {
                                        available.join(", ")
                                    }
                                )
                            })?;
                        let error =
                            unsafe { perform_action(element_ptr as AXUIElementRef, &actual) };
                        if error != kAXErrorSuccess {
                            anyhow::bail!(
                                "AXUIElementPerformAction({actual}) failed with error {error}"
                            );
                        }
                        Ok(actual)
                    })
                })
                .await
            },
        )
        .await;
        let changes = window_snapshot.detect_async().await;
        match result {
            Ok(Ok(actual)) => ToolResult::text(format!(
                "Performed {actual} on element {index}.{}",
                changes.result_suffix()
            )),
            Ok(Err(error)) => ToolResult::error(error.to_string()),
            Err(error) => ToolResult::error(format!("Secondary action task failed: {error}")),
        }
    }

    async fn press_key(
        &self,
        snapshot: &AppSnapshot,
        args: &Value,
        session: &str,
        lock_epoch: SessionLockEpoch,
    ) -> ToolResult {
        let raw_key = match required_string(args, "key") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let (key, modifiers) = match parse_xdotool_key(&raw_key) {
            Ok(parsed) => parsed,
            Err(message) => return ToolResult::error(message),
        };
        if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
            return result;
        }
        super::press_key::PressKeyTool::new(snapshot.native.clone())
            .invoke(json!({
                "pid": snapshot.app.pid,
                "window_id": snapshot.window_id,
                "key": key,
                "modifiers": modifiers,
                "_session_id": session,
            }))
            .await
    }

    async fn scroll(
        &self,
        snapshot: &AppSnapshot,
        args: &Value,
        session: &str,
        lock_epoch: SessionLockEpoch,
    ) -> ToolResult {
        let index = match required_string(args, "element_index") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let native_index = match snapshot_element_index(snapshot, &index) {
            Ok(index) => index,
            Err(result) => return result,
        };
        let direction = match required_string(args, "direction") {
            Ok(value) if matches!(value.as_str(), "up" | "down" | "left" | "right") => value,
            Ok(_) => return ToolResult::error("direction must be up, down, left, or right."),
            Err(result) => return result,
        };
        let pages = match validated_scroll_pages(args) {
            Ok(pages) => pages,
            Err(result) => return result,
        };
        // One native page is five 120px line steps. Integral page counts stay
        // page-granular; fractions use line granularity so 0.2-page increments
        // remain meaningful without changing the native API.
        let (by, amount) = if pages.fract().abs() < f64::EPSILON {
            ("page", pages.min(50.0) as u64)
        } else {
            ("line", (pages * 5.0).round().clamp(1.0, 50.0) as u64)
        };
        if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
            return result;
        }
        super::scroll::ScrollTool::new(snapshot.native.clone())
            .invoke(json!({
                "pid": snapshot.app.pid,
                "window_id": snapshot.window_id,
                "element_index": native_index,
                "direction": direction,
                "by": by,
                "amount": amount,
                "_session_id": session,
            }))
            .await
    }

    async fn select_text(
        &self,
        snapshot: &AppSnapshot,
        args: &Value,
        lock_epoch: SessionLockEpoch,
    ) -> ToolResult {
        let index = match required_string(args, "element_index") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let text = match required_string(args, "text") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let prefix = args
            .get("prefix")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let suffix = args
            .get("suffix")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let selection = args
            .get("selection")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .to_owned();
        if !matches!(
            selection.as_str(),
            "text" | "cursor_before" | "cursor_after"
        ) {
            return ToolResult::error("selection must be text, cursor_before, or cursor_after.");
        }
        let native_index = match snapshot_element_index(snapshot, &index) {
            Ok(index) => index,
            Err(result) => return result,
        };
        let guard = match snapshot.native.element_cache.get_element_retained(
            snapshot.app.pid,
            snapshot.window_id,
            native_index,
        ) {
            Some(guard) => guard,
            None => {
                return CompatError::new(
                    "stale_app_state",
                    "The cached accessibility element is stale. Call get_app_state again.",
                )
                .into_result()
            }
        };
        let element_ptr = guard.as_ptr();
        let prior_frontmost = crate::apps::frontmost_pid();
        let window_snapshot =
            crate::window_change_detector::WindowChangeDetector::snapshot(prior_frontmost);
        if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
            return result;
        }
        let guardian = self.guardian.clone();
        let result = with_compat_focus_guard(
            snapshot.app.pid,
            prior_frontmost,
            "codex_compat.select_text",
            || async move {
                tokio::task::spawn_blocking(move || {
                    dispatch_if_epoch_current(&guardian, lock_epoch, || {
                        select_text_range(
                            element_ptr,
                            &text,
                            prefix.as_deref(),
                            suffix.as_deref(),
                            &selection,
                        )
                    })
                })
                .await
            },
        )
        .await;
        let changes = window_snapshot.detect_async().await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Updated text selection on element {index}.{}",
                changes.result_suffix()
            )),
            Ok(Err(error)) => ToolResult::error(error.to_string()),
            Err(error) => ToolResult::error(format!("Text selection task failed: {error}")),
        }
    }

    async fn set_value(
        &self,
        snapshot: &AppSnapshot,
        args: &Value,
        session: &str,
        lock_epoch: SessionLockEpoch,
    ) -> ToolResult {
        let index = match required_string(args, "element_index") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let value = match required_string(args, "value") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let native_index = match snapshot_element_index(snapshot, &index) {
            Ok(index) => index,
            Err(result) => return result,
        };
        if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
            return result;
        }
        super::set_value::SetValueTool::new(snapshot.native.clone())
            .invoke(json!({
                "pid": snapshot.app.pid,
                "window_id": snapshot.window_id,
                "element_index": native_index,
                "value": value,
                "_session_id": session,
            }))
            .await
    }

    async fn type_text(
        &self,
        snapshot: &AppSnapshot,
        args: &Value,
        session: &str,
        lock_epoch: SessionLockEpoch,
    ) -> ToolResult {
        let text = match required_string(args, "text") {
            Ok(value) => value,
            Err(result) => return result,
        };
        if let Err(result) = self.require_current_lock_epoch(lock_epoch) {
            return result;
        }
        super::type_text::TypeTextTool::new(snapshot.native.clone())
            .invoke(json!({
                "pid": snapshot.app.pid,
                "window_id": snapshot.window_id,
                "text": text,
                "_session_id": session,
            }))
            .await
    }
}

fn element_supports_native_click(actions: &[String], button: &str) -> bool {
    match button {
        "left" => actions.iter().any(|action| action == "AXPress"),
        "right" => actions.iter().any(|action| action == "AXShowMenu"),
        "middle" => false,
        _ => false,
    }
}

async fn element_click_pixel_fallback(
    snapshot: &AppSnapshot,
    element_index: usize,
    button: &str,
    guardian: Arc<SessionLockGuardian>,
    lock_epoch: SessionLockEpoch,
) -> Result<Option<(f64, f64)>, ToolResult> {
    let guard = snapshot
        .native
        .element_cache
        .get_element_retained(snapshot.app.pid, snapshot.window_id, element_index)
        .ok_or_else(|| {
            CompatError::new(
                "stale_app_state",
                "The cached accessibility element is stale. Call get_app_state again.",
            )
            .into_result()
        })?;
    let button = button.to_owned();
    let inspection = tokio::task::spawn_blocking(move || {
        dispatch_if_epoch_current(&guardian, lock_epoch, || {
            let element = guard.as_ptr() as AXUIElementRef;
            let actions = unsafe { copy_action_names(element) };
            if element_supports_native_click(&actions, &button) {
                return Ok(None);
            }
            if actions.iter().any(|action| action == "AXScrollToVisible") {
                let _ = unsafe { perform_action(element, "AXScrollToVisible") };
            }
            unsafe { element_screen_center(element) }
                .map(Some)
                .ok_or_else(|| anyhow::anyhow!("the element has no usable on-screen frame"))
        })
    })
    .await
    .map_err(|error| ToolResult::error(format!("Element click inspection failed: {error}")))?
    .map_err(|error| ToolResult::error(format!("Element click fallback failed: {error}")))?;

    let Some(screen_point) = inspection else {
        return Ok(None);
    };
    let bounds = crate::windows::window_bounds_by_id(snapshot.window_id).ok_or_else(|| {
        ToolResult::error(format!(
            "Window {} disappeared before the element click.",
            snapshot.window_id
        ))
    })?;
    screen_point_to_native_click(snapshot, &bounds, screen_point).map(Some)
}

fn screen_point_to_native_click(
    snapshot: &AppSnapshot,
    bounds: &crate::windows::WindowBounds,
    (screen_x, screen_y): (f64, f64),
) -> Result<(f64, f64), ToolResult> {
    let local_x = screen_x - bounds.x;
    let local_y = screen_y - bounds.y;
    if !local_x.is_finite()
        || !local_y.is_finite()
        || local_x < 0.0
        || local_y < 0.0
        || local_x >= bounds.width
        || local_y >= bounds.height
    {
        return Err(ToolResult::error(
            "The addressed element is outside the latest app window. Call get_app_state again.",
        ));
    }
    Ok((
        local_x * snapshot.native_pixels_per_compat_x,
        local_y * snapshot.native_pixels_per_compat_y,
    ))
}

#[derive(Debug)]
struct CompatError {
    code: &'static str,
    message: String,
    details: serde_json::Map<String, Value>,
}

impl CompatError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: serde_json::Map::new(),
        }
    }

    fn with_detail(mut self, key: &str, value: Value) -> Self {
        self.details.insert(key.to_owned(), value);
        self
    }

    fn into_result(self) -> ToolResult {
        let mut structured = self.details;
        structured.insert("code".to_owned(), Value::String(self.code.to_owned()));
        structured.insert("message".to_owned(), Value::String(self.message.clone()));
        ToolResult::error(self.message).with_structured(Value::Object(structured))
    }
}

fn required_string(args: &Value, key: &str) -> Result<String, ToolResult> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ToolResult::error(format!("Missing required string field: {key}")))
}

fn required_number(args: &Value, key: &str) -> Result<f64, ToolResult> {
    args.get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| ToolResult::error(format!("Missing required numeric field: {key}")))
}

fn validated_click_count(args: &Value) -> Result<u64, ToolResult> {
    let Some(value) = args.get("click_count") else {
        return Ok(1);
    };
    let Some(count) = value.as_u64() else {
        return Err(ToolResult::error("click_count must be a positive integer."));
    };
    if !(1..=MAX_CLICK_COUNT).contains(&count) {
        return Err(ToolResult::error(format!(
            "click_count must be between 1 and {MAX_CLICK_COUNT}."
        )));
    }
    Ok(count)
}

fn validated_scroll_pages(args: &Value) -> Result<f64, ToolResult> {
    let Some(value) = args.get("pages") else {
        return Ok(1.0);
    };
    let Some(pages) = value.as_f64() else {
        return Err(ToolResult::error("pages must be a positive number."));
    };
    if !pages.is_finite() || pages <= 0.0 || pages > MAX_SCROLL_PAGES {
        return Err(ToolResult::error(format!(
            "pages must be greater than 0 and at most {MAX_SCROLL_PAGES}."
        )));
    }
    Ok(pages)
}

fn validate_compat_point(
    snapshot: &AppSnapshot,
    x: f64,
    y: f64,
    label: &str,
) -> Result<(), ToolResult> {
    let width = snapshot.logical_width as f64;
    let height = snapshot.logical_height as f64;
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x >= width || y >= height {
        return Err(ToolResult::error(format!(
            "{label} point ({x}, {y}) is outside the latest app screenshot bounds 0≤x<{width}, 0≤y<{height}. Call get_app_state again and use coordinates from that screenshot."
        )));
    }
    Ok(())
}

fn session_key(args: &Value) -> String {
    args.get("_session_id")
        .or_else(|| args.get("session"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(ANONYMOUS_SESSION)
        .to_owned()
}

fn normalize_app_ref(value: &str) -> String {
    value.trim().trim_end_matches('/').to_lowercase()
}

fn with_trailing_slash(value: &str) -> String {
    format!("{}/", value.trim_end_matches('/'))
}

fn first_text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|content| match content {
            Content::Text { text, .. } => Some(text.clone()),
            Content::Image { .. } => None,
        })
        .unwrap_or_default()
}

fn action_error(action_name: &str, result: ToolResult) -> ToolResult {
    let message = first_text(&result);
    CompatError::new(
        "action_failed",
        if message.is_empty() {
            format!("{action_name} failed.")
        } else {
            message
        },
    )
    .with_detail("action", Value::String(action_name.to_owned()))
    .with_detail(
        "native_error",
        result.structured_content.unwrap_or(Value::Null),
    )
    .into_result()
}

async fn with_compat_focus_guard<F, Fut, R>(
    target_pid: i32,
    prior_frontmost: Option<i32>,
    origin: &'static str,
    action: F,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    crate::focus_guard::with_focus_suppressed(Some(target_pid), prior_frontmost, origin, action)
        .await
}

fn refresh_warning_after_success(
    action_name: &str,
    action_summary: String,
    action_structured: Option<Value>,
    refreshed: ToolResult,
) -> ToolResult {
    let refresh_error = first_text(&refreshed);
    ToolResult::text(format!(
        "{action_name} completed, but refreshing the app state failed. Do not repeat the action; call get_app_state again. {refresh_error}"
    ))
    .with_structured(json!({
        "state_refresh_failed_after_success": true,
        "warning": "The action completed, but the required refreshed app state and screenshot are unavailable. Do not retry the action automatically.",
        "refresh_error": refresh_error,
        "refresh_error_details": refreshed.structured_content,
        "action_result": {
            "text": action_summary,
            "structured": action_structured,
        },
    }))
}

fn compat_error_from_native(code: &'static str, result: ToolResult) -> ToolResult {
    CompatError::new(code, first_text(&result))
        .with_detail(
            "native_error",
            result.structured_content.unwrap_or(Value::Null),
        )
        .into_result()
}

fn element_index_set(structured: &Value) -> HashSet<String> {
    structured
        .get("elements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|element| {
            let index = element.get("element_index")?;
            match index {
                Value::String(value) => Some(value.clone()),
                Value::Number(value) => Some(value.to_string()),
                _ => return None,
            }
        })
        .collect()
}

fn snapshot_element_index(snapshot: &AppSnapshot, index: &str) -> Result<usize, ToolResult> {
    if !snapshot.element_indices.contains(index) {
        return Err(
            CompatError::new(
                "element_not_in_snapshot",
                format!(
                    "Element index '{index}' is not present in the current app snapshot. Call get_app_state again and use an index from that result."
                ),
            )
            .into_result()
        );
    }
    index.parse::<usize>().map_err(|_| {
        CompatError::new(
            "invalid_element_index",
            format!("Element index '{index}' is not a valid non-negative integer."),
        )
        .into_result()
    })
}

fn state_result(
    native_result: ToolResult,
    snapshot: &AppSnapshot,
) -> Result<ToolResult, CompatError> {
    let tree = native_result
        .structured_content
        .as_ref()
        .and_then(|value| value.get("tree_markdown"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = snapshot
        .app
        .launch_path
        .as_deref()
        .map(with_trailing_slash)
        .unwrap_or_else(|| snapshot.app.name.clone());
    let bundle = snapshot.app.bundle_id.as_deref().unwrap_or("unknown");
    let text = format!(
        "Cua Driver Computer Use state\n<app_state>\n\
         App={path} (bundleID {bundle}, pid {})\n\
         Window: \"{}\", App: {}.\n{}\n\
         </app_state>",
        snapshot.app.pid, snapshot.window_title, snapshot.app.name, tree
    );

    let png = native_result
        .content
        .iter()
        .find_map(|content| match content {
            Content::Image { data, .. } => Some(data.as_str()),
            Content::Text { .. } => None,
        })
        .ok_or_else(|| {
            CompatError::new(
                "screen_capture_failed",
                "The native app state did not contain a screenshot.",
            )
        })?;
    let jpeg =
        logical_jpeg(png, snapshot.logical_width, snapshot.logical_height).ok_or_else(|| {
            CompatError::new(
                "jpeg_conversion_failed",
                "The key-window screenshot could not be converted to the Computer Use JPEG format.",
            )
        })?;
    let content = vec![Content::text(text), Content::image_jpeg(jpeg)];
    let mut structured = native_result
        .structured_content
        .unwrap_or_else(|| json!({}));
    if let Some(elements) = structured.get_mut("elements").and_then(Value::as_array_mut) {
        for element in elements {
            if let Some(object) = element.as_object_mut() {
                object.remove("element_token");
            }
        }
    }
    if let Some(object) = structured.as_object_mut() {
        object.remove("snapshot_id");
    }
    structured["app"] = json!({
        "requested": snapshot.app.requested,
        "name": snapshot.app.name,
        "path": snapshot.app.launch_path,
        "bundle_id": snapshot.app.bundle_id,
        "pid": snapshot.app.pid,
    });
    structured["window_title"] = Value::String(snapshot.window_title.clone());
    structured["compat_snapshot_generation"] = json!(snapshot.generation);
    structured["screenshot_width"] = json!(snapshot.logical_width);
    structured["screenshot_height"] = json!(snapshot.logical_height);
    structured["screenshot_mime_type"] = Value::String("image/jpeg".to_owned());
    structured["coordinate_space"] = Value::String(
        "logical window points; origin is the screenshot's top-left pixel".to_owned(),
    );
    Ok(ToolResult {
        content,
        is_error: None,
        structured_content: Some(structured),
    })
}

fn logical_jpeg(png_base64: &str, width: u32, height: u32) -> Option<String> {
    let bytes = BASE64.decode(png_base64).ok()?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?;
    let resized = if image.width() == width && image.height() == height {
        image
    } else {
        image.resize_exact(width, height, image::imageops::FilterType::Lanczos3)
    };
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&resized)
        .ok()?;
    Some(BASE64.encode(jpeg))
}

fn compat_point_to_native(snapshot: &AppSnapshot, x: f64, y: f64) -> (f64, f64) {
    (
        x * snapshot.native_pixels_per_compat_x,
        y * snapshot.native_pixels_per_compat_y,
    )
}

#[derive(Clone)]
struct RecentApp {
    info: crate::apps::AppInfo,
    last_used: Option<String>,
    use_count: Option<u64>,
}

fn recent_apps_with_usage(apps: Vec<crate::apps::AppInfo>) -> Vec<RecentApp> {
    let cutoff_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(14 * 24 * 60 * 60) as i64;
    let cutoff = crate::apps::unix_secs_to_rfc3339(cutoff_secs);
    let apps = dedupe_apps(apps);
    let metadata_paths: Vec<String> = apps
        .iter()
        .filter_map(|app| app.launch_path.clone())
        .collect();
    let spotlight = spotlight_usage_batch(&metadata_paths);
    let mut recent: Vec<RecentApp> = apps
        .into_iter()
        .filter_map(|info| {
            let (spotlight_last_used, use_count) = info
                .launch_path
                .as_deref()
                .and_then(|path| spotlight.get(&normalize_app_ref(path)).cloned())
                .unwrap_or((None, None));
            let last_used = spotlight_last_used.or_else(|| info.last_used.clone());
            let include = info.running
                || last_used
                    .as_deref()
                    .map(|value| value >= cutoff.as_str())
                    .unwrap_or(false);
            include.then_some(RecentApp {
                info,
                last_used,
                use_count,
            })
        })
        .collect();
    recent.sort_by(|left, right| {
        right
            .info
            .active
            .cmp(&left.info.active)
            .then_with(|| right.info.running.cmp(&left.info.running))
            .then_with(|| right.last_used.cmp(&left.last_used))
            .then_with(|| left.info.name.cmp(&right.info.name))
    });
    recent
}

/// Collapse duplicate process rows into one deterministic app entry while
/// retaining genuinely distinct bundles installed at different paths.
fn dedupe_apps(apps: Vec<crate::apps::AppInfo>) -> Vec<crate::apps::AppInfo> {
    let mut by_identity: BTreeMap<String, crate::apps::AppInfo> = BTreeMap::new();
    for app in apps {
        let bundle = app
            .bundle_id
            .as_deref()
            .map(normalize_app_ref)
            .unwrap_or_default();
        let path = app
            .launch_path
            .as_deref()
            .map(normalize_app_ref)
            .unwrap_or_default();
        let key = match (bundle.is_empty(), path.is_empty()) {
            (false, false) => format!("bundle:{bundle}|path:{path}"),
            (false, true) => format!("bundle:{bundle}"),
            (true, false) => format!("path:{path}"),
            (true, true) => format!("name:{}|pid:{}", normalize_app_ref(&app.name), app.pid),
        };
        match by_identity.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(app);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let current = entry.get();
                // Prefer the frontmost row, then a live row, then the lower
                // positive pid for deterministic output across enumerations.
                let candidate_rank = (app.active, app.running, std::cmp::Reverse(app.pid.max(0)));
                let current_rank = (
                    current.active,
                    current.running,
                    std::cmp::Reverse(current.pid.max(0)),
                );
                if candidate_rank > current_rank {
                    entry.insert(app);
                }
            }
        }
    }
    by_identity.into_values().collect()
}

/// Query Spotlight metadata for every app in one `mdls` process. The previous
/// per-app spawn made `list_apps` take tens of seconds on machines with many
/// apps; this keeps process cost constant and returns a path-keyed map that is
/// straightforward to benchmark.
fn spotlight_usage_batch(paths: &[String]) -> HashMap<String, (Option<String>, Option<u64>)> {
    if paths.is_empty() {
        return HashMap::new();
    }
    let mut command = std::process::Command::new("/usr/bin/mdls");
    command.args([
        "-plist",
        "/dev/stdout",
        "-name",
        "kMDItemLastUsedDate",
        "-name",
        "kMDItemUseCount",
    ]);
    command.args(paths);
    let output = command.output();
    let Ok(output) = output else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    let xml = String::from_utf8_lossy(&output.stdout);
    parse_spotlight_usage_batch(&xml, paths)
}

fn parse_spotlight_usage_batch(
    xml: &str,
    paths: &[String],
) -> HashMap<String, (Option<String>, Option<u64>)> {
    let dictionaries: Vec<&str> = xml
        .split("<dict>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</dict>").map(|(dict, _)| dict))
        .collect();
    paths
        .iter()
        .zip(dictionaries)
        .map(|(path, dict)| {
            let last_used =
                xml_value_after_key(dict, "kMDItemLastUsedDate", "date").map(str::to_owned);
            let use_count = xml_value_after_key(dict, "kMDItemUseCount", "integer")
                .and_then(|value| value.parse().ok());
            (normalize_app_ref(path), (last_used, use_count))
        })
        .collect()
}

fn xml_value_after_key<'a>(xml: &'a str, key: &str, tag: &str) -> Option<&'a str> {
    let key_marker = format!("<key>{key}</key>");
    let rest = xml.split_once(&key_marker)?.1;
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let value = rest.split_once(&open)?.1.split_once(&close)?.0;
    Some(value.trim())
}

async fn resolve_or_launch_app(
    app_ref: &str,
    mode: AppResolveMode,
) -> Result<AppIdentity, CompatError> {
    let query = app_ref.to_owned();
    let apps = tokio::task::spawn_blocking(crate::apps::list_all_apps)
        .await
        .map_err(|error| {
            CompatError::new(
                "app_enumeration_failed",
                format!("Could not enumerate apps: {error}"),
            )
        })?;
    let info = match resolve_app_from_list(&query, &apps) {
        Ok(info) => info,
        Err(error) if error.code == "invalid_app" && query.contains('/') => {
            app_info_for_path(&query).ok_or(error)?
        }
        Err(error) => return Err(error),
    };
    let (mut identity, needs_launch) = identity_and_launch_requirement(&query, &info, mode)?;
    if !needs_launch {
        return Ok(identity);
    }

    let launch_ref = if query.contains('/') {
        identity
            .launch_path
            .clone()
            .unwrap_or_else(|| query.clone())
    } else if let Some(bundle_id) = identity.bundle_id.clone() {
        bundle_id
    } else {
        identity.name.clone()
    };
    let apple_event_bundle_id = identity.bundle_id.clone();
    let pid = tokio::task::spawn_blocking(move || {
        let config = crate::apps::nsworkspace::OpenConfig {
            apple_event_bundle_id,
            ..Default::default()
        };
        let app = crate::apps::nsworkspace::open_application(&launch_ref, &config)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok::<i32, anyhow::Error>(unsafe { app.processIdentifier() })
    })
    .await
    .map_err(|error| {
        CompatError::new(
            "app_launch_failed",
            format!("App launch task failed for '{query}': {error}"),
        )
    })?
    .map_err(|error| {
        CompatError::new(
            "app_launch_failed",
            format!("Could not launch '{query}': {error}"),
        )
    })?;
    identity.pid = pid;
    Ok(identity)
}

fn identity_and_launch_requirement(
    query: &str,
    info: &crate::apps::AppInfo,
    mode: AppResolveMode,
) -> Result<(AppIdentity, bool), CompatError> {
    let identity = AppIdentity {
        requested: query.to_owned(),
        name: info.name.clone(),
        bundle_id: info.bundle_id.clone(),
        launch_path: info.launch_path.clone(),
        pid: info.pid,
    };
    enforce_target_policy(&identity)?;

    if info.running && info.pid > 0 {
        return Ok((identity, false));
    }
    if mode == AppResolveMode::RunningOnly {
        return Err(CompatError::new(
            "app_no_longer_running",
            format!(
                "'{query}' is no longer running after the action. The action was not repeated and the app was not relaunched."
            ),
        ));
    }
    Ok((identity, true))
}

fn resolve_app_from_list(
    app_ref: &str,
    apps: &[crate::apps::AppInfo],
) -> Result<crate::apps::AppInfo, CompatError> {
    let query = normalize_app_ref(app_ref);
    let is_path = app_ref.contains('/');
    let mut matches: Vec<&crate::apps::AppInfo> = apps
        .iter()
        .filter(|app| {
            if is_path {
                return app.launch_path.as_deref().map(normalize_app_ref).as_deref()
                    == Some(query.as_str());
            }
            normalize_app_ref(&app.name) == query
                || app.bundle_id.as_deref().map(normalize_app_ref).as_deref()
                    == Some(query.as_str())
        })
        .collect();

    // The running and installed scans can surface the same physical app as
    // duplicate rows. Deduplicate only identical paths/pids; distinct paths
    // sharing a bundle id remain ambiguous and require the full path.
    matches.sort_by_key(|app| {
        (
            app.launch_path.clone().unwrap_or_default(),
            app.pid,
            app.name.clone(),
        )
    });
    matches.dedup_by_key(|app| {
        (
            app.launch_path.clone().unwrap_or_default(),
            app.pid,
            app.name.clone(),
        )
    });

    match matches.as_slice() {
        [] => Err(CompatError::new(
            "invalid_app",
            format!("Invalid app: {app_ref}"),
        )),
        [app] => Ok((*app).clone()),
        many => {
            let paths = many
                .iter()
                .filter_map(|app| app.launch_path.clone())
                .collect::<Vec<_>>()
                .join(", ");
            Err(CompatError::new(
                "ambiguous_app",
                format!(
                    "Ambiguous app identifier '{app_ref}'. Multiple apps match: {}. Use a full app path.",
                    if paths.is_empty() {
                        many.iter()
                            .map(|app| app.name.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    } else {
                        paths
                    }
                ),
            ))
        }
    }
}

fn app_info_for_path(path: &str) -> Option<crate::apps::AppInfo> {
    let normalized = path.trim_end_matches('/');
    let app_path = std::path::Path::new(normalized);
    if !app_path.is_dir() || app_path.extension().and_then(|value| value.to_str()) != Some("app") {
        return None;
    }
    let name = app_path.file_stem()?.to_str()?.to_owned();
    let plist = app_path.join("Contents/Info.plist");
    let bundle_id = std::process::Command::new("/usr/bin/plutil")
        .args([
            "-extract",
            "CFBundleIdentifier",
            "raw",
            "-o",
            "-",
            plist.to_str()?,
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty());
    Some(crate::apps::AppInfo {
        name,
        pid: 0,
        bundle_id,
        running: false,
        active: false,
        launch_path: Some(normalized.to_owned()),
        kind: Some("desktop".to_owned()),
        last_used: None,
    })
}

fn enforce_target_policy(app: &AppIdentity) -> Result<(), CompatError> {
    let bundle = app.bundle_id.as_deref().unwrap_or_default();
    let bundle_lower = bundle.to_ascii_lowercase();
    let name_lower = app.name.to_ascii_lowercase();
    let host_bundle = std::env::var(cua_driver_core::HOST_BUNDLE_ID_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let self_target = app.pid > 0 && app.pid == std::process::id() as i32;
    let host_target = !host_bundle.is_empty() && bundle_lower == host_bundle;
    let driver_target = bundle_lower == "com.trycua.driver"
        || name_lower == "cuadriver"
        || name_lower == "cua driver";
    let protected_host_app = is_protected_host_app(&bundle_lower, &name_lower);
    let protected_terminal = is_protected_terminal_target(&bundle_lower, &name_lower);
    let protected_bundle = matches!(
        bundle_lower.as_str(),
        "com.apple.loginwindow"
            | "com.apple.securityagent"
            | "com.apple.coreservicesuiagent"
            | "com.apple.authorizationhost"
            | "com.apple.keychainaccess"
            | "com.apple.passwords"
            | "com.apple.systempreferences"
    ) || bundle_lower.starts_with("com.apple.localauthentication.")
        || bundle_lower.starts_with("com.apple.authenticationservices.");
    let protected_name = matches!(
        name_lower.as_str(),
        "loginwindow"
            | "securityagent"
            | "authorizationhost"
            | "coreauthd"
            | "localauthenticationremoteservice"
            | "keychain access"
            | "passwords"
            | "system settings"
            | "system preferences"
    );
    if self_target
        || host_target
        || driver_target
        || protected_host_app
        || protected_terminal
        || protected_bundle
        || protected_name
    {
        return Err(CompatError::new(
            "target_not_allowed",
            format!(
                "Computer Use is not allowed to target '{}' ({bundle}). The driver/host, terminal apps, and macOS security-sensitive or authentication surfaces are protected.",
                app.name
            ),
        ));
    }
    Ok(())
}

fn is_protected_host_app(bundle: &str, name: &str) -> bool {
    matches!(
        bundle,
        "com.openai.codex"
            | "com.openai.chat"
            | "com.openai.chatgpt"
            | "com.openai.chatgpt.mac"
            | "com.openai.chatgpt.desktop"
    ) || bundle.starts_with("com.openai.chatgpt.")
        || bundle.starts_with("com.openai.sky.")
        || matches!(
            name,
            "codex" | "codex computer use" | "skycomputeruseclient" | "chatgpt" | "chat gpt"
        )
}

fn is_protected_terminal_target(bundle: &str, name: &str) -> bool {
    matches!(
        bundle,
        "com.apple.terminal"
            | "com.cmuxterm.app"
            | "com.mitchellh.ghostty"
            | "com.googlecode.iterm2"
            | "net.kovidgoyal.kitty"
            | "org.alacritty"
            | "com.github.wez.wezterm"
            | "co.zeit.hyper"
            | "co.vercel.hyper"
    ) || bundle.starts_with("com.cmuxterm.app.")
        || bundle.starts_with("dev.warp.")
        || matches!(
            name,
            "terminal"
                | "cmux"
                | "cmux beta"
                | "cmux dev"
                | "cmux nightly"
                | "ghostty"
                | "iterm"
                | "iterm2"
                | "warp"
                | "warp preview"
                | "kitty"
                | "alacritty"
                | "wezterm"
                | "hyper"
        )
}

fn lock_guard_error(error: SessionLockError) -> CompatError {
    match error {
        SessionLockError::Locked => CompatError::new(
            "screen_locked",
            "Computer Use is unavailable while the macOS screen is locked.",
        ),
        SessionLockError::EpochChanged => CompatError::new(
            "session_lock_changed",
            "The macOS lock state changed during Computer Use. Call get_app_state again after the screen is unlocked.",
        ),
    }
}

fn dispatch_if_epoch_current<T>(
    guardian: &SessionLockGuardian,
    epoch: SessionLockEpoch,
    dispatch: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    guardian
        .validate(epoch)
        .map_err(|error| anyhow::anyhow!(lock_guard_error(error).message))?;
    dispatch()
}

async fn run_guarded_dispatch<F, Fut, T>(
    guardian: &SessionLockGuardian,
    epoch: SessionLockEpoch,
    dispatch: F,
) -> Result<T, SessionLockError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    guardian.validate(epoch)?;
    Ok(dispatch().await)
}

async fn run_guarded_capture<F, Fut, T>(
    guardian: &SessionLockGuardian,
    epoch: SessionLockEpoch,
    capture: F,
) -> Result<T, SessionLockError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    guardian.validate(epoch)?;
    let result = capture().await;
    guardian.validate(epoch)?;
    Ok(result)
}

fn validate_live_snapshot(snapshot: &AppSnapshot) -> Result<(), CompatError> {
    enforce_target_policy(&snapshot.app)?;
    if let Some(expected_bundle) = snapshot.app.bundle_id.as_deref() {
        let current_bundle = crate::apps::bundle_id_for_pid(snapshot.app.pid);
        if current_bundle
            .as_deref()
            .map(|bundle| !bundle.eq_ignore_ascii_case(expected_bundle))
            .unwrap_or(true)
        {
            return Err(stale_snapshot_error(&snapshot.app.requested));
        }
    }
    let window_is_live = crate::windows::all_windows()
        .iter()
        .any(|window| window.pid == snapshot.app.pid && window.window_id == snapshot.window_id);
    if !window_is_live {
        return Err(stale_snapshot_error(&snapshot.app.requested));
    }
    if let Some(key_window) = key_window_id(snapshot.app.pid) {
        if key_window != snapshot.window_id {
            return Err(stale_snapshot_error(&snapshot.app.requested));
        }
    }
    Ok(())
}

fn stale_snapshot_error(app_ref: &str) -> CompatError {
    CompatError::new(
        "stale_app_state",
        format!(
            "The pid or key window for '{app_ref}' changed after its snapshot. Call get_app_state again before acting."
        ),
    )
}

async fn wait_for_key_window(pid: i32) -> Result<(u32, String), CompatError> {
    tokio::task::spawn_blocking(move || wait_for_key_window_blocking(pid))
        .await
        .map_err(|error| {
            CompatError::new(
                "window_observer_failed",
                format!("The key-window observer task failed for pid {pid}: {error}"),
            )
        })?
}

fn wait_for_key_window_blocking(pid: i32) -> Result<(u32, String), CompatError> {
    if let Some(window) = resolve_key_window_info(pid) {
        return Ok(window);
    }

    let mut observer = WindowNotificationObserver::new(pid)?;
    resolve_after_notifications(
        WINDOW_WAIT_TIMEOUT,
        || resolve_key_window_info(pid),
        |remaining| observer.wait(remaining),
    )
    .ok_or_else(|| {
        CompatError::new(
            "app_has_no_window",
            format!("App pid {pid} did not expose a key window within 5 seconds."),
        )
    })
}

fn resolve_key_window_info(pid: i32) -> Option<(u32, String)> {
    let id = key_window_id(pid).or_else(|| crate::windows::resolve_main_window_id(pid).ok())?;
    let title = crate::windows::all_windows()
        .into_iter()
        .find(|window| window.pid == pid && window.window_id == id)
        .map(|window| window.title)
        .unwrap_or_default();
    Some((id, title))
}

fn resolve_after_notifications<T, Resolve, Wait>(
    timeout: Duration,
    mut resolve: Resolve,
    mut wait: Wait,
) -> Option<T>
where
    Resolve: FnMut() -> Option<T>,
    Wait: FnMut(Duration) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = resolve() {
            return Some(value);
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        if remaining.is_zero() || !wait(remaining) {
            return resolve();
        }
    }
}

struct WindowObserverContext {
    run_loop: CFRunLoopRef,
    signaled: AtomicBool,
}

struct WindowNotificationObserver {
    observer: AXObserverRef,
    app: AXUIElementRef,
    source: CFRunLoopSourceRef,
    run_loop: CFRunLoopRef,
    context: Box<WindowObserverContext>,
    notifications: Vec<CFString>,
}

impl WindowNotificationObserver {
    fn new(pid: i32) -> Result<Self, CompatError> {
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() {
                return Err(CompatError::new(
                    "window_observer_failed",
                    format!("Could not create an accessibility application element for pid {pid}."),
                ));
            }

            let mut observer = std::ptr::null_mut();
            let create_error = AXObserverCreate(pid, window_notification_callback, &mut observer);
            if create_error != kAXErrorSuccess || observer.is_null() {
                CFRelease(app as CFTypeRef);
                return Err(CompatError::new(
                    "window_observer_failed",
                    format!("AXObserverCreate failed for pid {pid} with error {create_error}."),
                ));
            }

            let run_loop = CFRunLoopGetCurrent();
            let source = AXObserverGetRunLoopSource(observer);
            if run_loop.is_null() || source.is_null() {
                CFRelease(observer as CFTypeRef);
                CFRelease(app as CFTypeRef);
                return Err(CompatError::new(
                    "window_observer_failed",
                    format!("AXObserver did not provide a run-loop source for pid {pid}."),
                ));
            }

            let mut context = Box::new(WindowObserverContext {
                run_loop,
                signaled: AtomicBool::new(false),
            });
            let context_ptr = context.as_mut() as *mut WindowObserverContext as *mut c_void;
            let mut notifications = Vec::new();
            for name in [
                "AXWindowCreated",
                "AXFocusedWindowChanged",
                "AXMainWindowChanged",
            ] {
                let notification = CFString::new(name);
                let error = AXObserverAddNotification(
                    observer,
                    app,
                    notification.as_concrete_TypeRef(),
                    context_ptr,
                );
                if error == kAXErrorSuccess {
                    notifications.push(notification);
                }
            }
            if notifications.is_empty() {
                CFRelease(observer as CFTypeRef);
                CFRelease(app as CFTypeRef);
                return Err(CompatError::new(
                    "window_observer_failed",
                    format!("App pid {pid} exposes no observable window notifications."),
                ));
            }

            CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
            Ok(Self {
                observer,
                app,
                source,
                run_loop,
                context,
                notifications,
            })
        }
    }

    fn wait(&mut self, timeout: Duration) -> bool {
        self.context.signaled.store(false, Ordering::Release);
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, timeout.as_secs_f64(), 0);
        }
        self.context.signaled.swap(false, Ordering::AcqRel)
    }
}

impl Drop for WindowNotificationObserver {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopRemoveSource(self.run_loop, self.source, kCFRunLoopDefaultMode);
            for notification in &self.notifications {
                let _ = AXObserverRemoveNotification(
                    self.observer,
                    self.app,
                    notification.as_concrete_TypeRef(),
                );
            }
            CFRelease(self.observer as CFTypeRef);
            CFRelease(self.app as CFTypeRef);
        }
    }
}

extern "C" fn window_notification_callback(
    _observer: AXObserverRef,
    _element: AXUIElementRef,
    _notification: core_foundation::string::CFStringRef,
    context: *mut c_void,
) {
    if context.is_null() {
        return;
    }
    let context = unsafe { &*(context as *const WindowObserverContext) };
    context.signaled.store(true, Ordering::Release);
    unsafe { CFRunLoopStop(context.run_loop) };
}

fn key_window_id(pid: i32) -> Option<u32> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let attribute = CFString::new("AXFocusedWindow");
        let mut value: CFTypeRef = std::ptr::null();
        let error = AXUIElementCopyAttributeValue(app, attribute.as_concrete_TypeRef(), &mut value);
        CFRelease(app as CFTypeRef);
        if error != kAXErrorSuccess || value.is_null() {
            return None;
        }
        let mut window_id = 0u32;
        let window_error = _AXUIElementGetWindow(value as AXUIElementRef, &mut window_id);
        CFRelease(value);
        (window_error == kAXErrorSuccess && window_id != 0).then_some(window_id)
    }
}

fn canonical_ax_action(action: &str) -> String {
    if action
        .get(..2)
        .map(|prefix| prefix.eq_ignore_ascii_case("AX"))
        .unwrap_or(false)
    {
        return format!("AX{}", action.get(2..).unwrap_or_default());
    }
    let mut canonical = String::from("AX");
    for word in action
        .split(|character: char| character.is_whitespace() || matches!(character, '_' | '-'))
        .filter(|word| !word.is_empty())
    {
        let mut characters = word.chars();
        if let Some(first) = characters.next() {
            canonical.extend(first.to_uppercase());
            canonical.push_str(&characters.as_str().to_lowercase());
        }
    }
    canonical
}

fn parse_xdotool_key(raw: &str) -> Result<(String, Vec<String>), String> {
    if raw.is_empty() {
        return Err("key must not be empty.".to_owned());
    }
    if raw == "+" {
        return Ok(("+".to_owned(), Vec::new()));
    }
    let mut parts: Vec<&str> = raw.split('+').collect();
    let key = parts
        .pop()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| format!("Invalid key combination: {raw}"))?;
    let mut modifiers = Vec::new();
    for modifier in parts {
        let normalized = match modifier.to_ascii_lowercase().as_str() {
            "super" | "meta" | "cmd" | "command" => "cmd",
            "ctrl" | "control" => "ctrl",
            "alt" | "option" => "option",
            "shift" => "shift",
            "fn" => "fn",
            unknown => return Err(format!("Unknown modifier '{unknown}' in key '{raw}'.")),
        };
        if !modifiers.iter().any(|current| current == normalized) {
            modifiers.push(normalized.to_owned());
        }
    }
    let normalized_key = match key.to_ascii_lowercase().as_str() {
        "return" | "enter" | "kp_enter" => "return".to_owned(),
        "backspace" => "backspace".to_owned(),
        "escape" | "esc" => "escape".to_owned(),
        "page_up" | "prior" => "pageup".to_owned(),
        "page_down" | "next" => "pagedown".to_owned(),
        "left" | "right" | "up" | "down" | "tab" | "space" | "delete" | "home" | "end" => {
            key.to_ascii_lowercase()
        }
        value if value.starts_with("kp_") && value[3..].chars().all(|c| c.is_ascii_digit()) => {
            value[3..].to_owned()
        }
        _ => key.to_owned(),
    };
    Ok((normalized_key, modifiers))
}

#[repr(C)]
struct AXCFRange {
    location: isize,
    length: isize,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXValueCreate(value_type: i32, value_ptr: *const std::ffi::c_void) -> AXValueRef;
}

fn select_text_range(
    element_ptr: usize,
    text: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
    selection: &str,
) -> anyhow::Result<()> {
    if text.is_empty() {
        anyhow::bail!("text must not be empty for select_text");
    }
    unsafe {
        let mut current = element_ptr as AXUIElementRef;
        let mut current_owned = false;
        let mut found_matching_text = false;
        for _ in 0..12 {
            for attribute in ["AXValue", "AXTitle", "AXDescription"] {
                let Some(value) = copy_string_attr(current, attribute) else {
                    continue;
                };
                let Some((byte_start, byte_end)) = find_text_match(&value, text, prefix, suffix)?
                else {
                    continue;
                };
                found_matching_text = true;
                let start_utf16 = value[..byte_start].encode_utf16().count();
                let end_utf16 = value[..byte_end].encode_utf16().count();
                let (location, length) = match selection {
                    "cursor_before" => (start_utf16, 0),
                    "cursor_after" => (end_utf16, 0),
                    _ => (start_utf16, end_utf16.saturating_sub(start_utf16)),
                };
                let range = AXCFRange {
                    location: location as isize,
                    length: length as isize,
                };
                let value = AXValueCreate(
                    kAXValueCFRangeType,
                    &range as *const AXCFRange as *const std::ffi::c_void,
                );
                if value.is_null() {
                    continue;
                }
                let attr = CFString::new("AXSelectedTextRange");
                let error = AXUIElementSetAttributeValue(
                    current,
                    attr.as_concrete_TypeRef(),
                    value as CFTypeRef,
                );
                CFRelease(value as CFTypeRef);
                if error == kAXErrorSuccess {
                    if current_owned {
                        CFRelease(current as CFTypeRef);
                    }
                    return Ok(());
                }
            }

            let parent_attr = CFString::new("AXParent");
            let mut parent: CFTypeRef = std::ptr::null();
            let parent_error = AXUIElementCopyAttributeValue(
                current,
                parent_attr.as_concrete_TypeRef(),
                &mut parent,
            );
            if current_owned {
                CFRelease(current as CFTypeRef);
            }
            if parent_error != kAXErrorSuccess || parent.is_null() {
                current_owned = false;
                break;
            }
            current = parent as AXUIElementRef;
            current_owned = true;
        }
        if current_owned {
            CFRelease(current as CFTypeRef);
        }
        if found_matching_text {
            anyhow::bail!(
                "The text was found, but neither the element nor its text-container ancestors support AXSelectedTextRange."
            );
        }
        anyhow::bail!(
            "Text was not found in the addressed element or its text-container ancestors."
        )
    }
}

fn find_text_match(
    value: &str,
    text: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
) -> anyhow::Result<Option<(usize, usize)>> {
    let matches: Vec<(usize, usize)> = value
        .match_indices(text)
        .filter_map(|(start, matched)| {
            let end = start + matched.len();
            let prefix_matches = prefix
                .map(|prefix| value[..start].ends_with(prefix))
                .unwrap_or(true);
            let suffix_matches = suffix
                .map(|suffix| value[end..].starts_with(suffix))
                .unwrap_or(true);
            (prefix_matches && suffix_matches).then_some((start, end))
        })
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [range] => Ok(Some(*range)),
        _ => anyhow::bail!(
            "Text occurs more than once. Provide prefix or suffix to identify one occurrence."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_info(name: &str, bundle_id: &str, path: &str, pid: i32) -> crate::apps::AppInfo {
        crate::apps::AppInfo {
            name: name.to_owned(),
            pid,
            bundle_id: Some(bundle_id.to_owned()),
            running: pid > 0,
            active: false,
            launch_path: Some(path.to_owned()),
            kind: Some("desktop".to_owned()),
            last_used: None,
        }
    }

    fn snapshot(requested: &str, session_native: Arc<ToolState>) -> AppSnapshot {
        AppSnapshot {
            app: AppIdentity {
                requested: requested.to_owned(),
                name: requested.to_owned(),
                bundle_id: Some(format!("com.example.{}", requested.to_lowercase())),
                launch_path: Some(format!("/Applications/{requested}.app")),
                pid: 42,
            },
            window_id: 7,
            window_title: requested.to_owned(),
            generation: 0,
            element_indices: HashSet::new(),
            native_pixels_per_compat_x: 2.0,
            native_pixels_per_compat_y: 2.0,
            logical_width: 232,
            logical_height: 408,
            native: session_native,
        }
    }

    #[test]
    fn schemas_match_codex_v829_roster_order_and_annotations() {
        let registry = crate::register_codex_computer_use_compat_tools();
        let list = registry.tools_list();
        let tools = list["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "list_apps",
                "get_app_state",
                "click",
                "perform_secondary_action",
                "set_value",
                "select_text",
                "scroll",
                "drag",
                "press_key",
                "type_text",
            ]
        );
        for tool in tools {
            let expected_read_only =
                matches!(tool["name"].as_str(), Some("list_apps" | "get_app_state"));
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert_eq!(tool["annotations"]["readOnlyHint"], expected_read_only);
            assert_eq!(tool["annotations"]["idempotentHint"], expected_read_only);
            assert_eq!(tool["annotations"]["destructiveHint"], false);
            assert_eq!(tool["annotations"]["openWorldHint"], false);
        }
        assert_eq!(
            tools[2]["inputSchema"]["properties"]["element_index"]["type"],
            "string"
        );
        assert_eq!(
            tools[5]["inputSchema"]["properties"]["selection"]["enum"],
            json!(["text", "cursor_before", "cursor_after"])
        );
        assert!(tools[2]["inputSchema"]["properties"]["click_count"]
            .get("minimum")
            .is_none());
        assert!(tools[6]["inputSchema"]["properties"]["pages"]
            .get("exclusiveMinimum")
            .is_none());
    }

    #[tokio::test]
    async fn action_without_app_snapshot_fails_closed() {
        let state = CompatState::new();
        let result = state
            .run_action(CompatKind::Click, json!({"app":"Calculator","x":10,"y":10}))
            .await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "no_active_app_session"
        );
    }

    #[test]
    fn app_resolution_accepts_name_path_and_bundle_and_rejects_ambiguity() {
        let apps = vec![
            app_info(
                "Calculator",
                "com.apple.calculator",
                "/System/Applications/Calculator.app",
                123,
            ),
            app_info("Harness", "com.example.harness", "/tmp/A/Harness.app", 0),
            app_info("Harness", "com.example.harness", "/tmp/B/Harness.app", 0),
        ];
        assert_eq!(resolve_app_from_list("Calculator", &apps).unwrap().pid, 123);
        assert_eq!(
            resolve_app_from_list("/System/Applications/Calculator.app/", &apps)
                .unwrap()
                .pid,
            123
        );
        assert_eq!(
            resolve_app_from_list("com.apple.calculator", &apps)
                .unwrap()
                .pid,
            123
        );
        assert_eq!(
            resolve_app_from_list("com.example.harness", &apps)
                .unwrap_err()
                .code,
            "ambiguous_app"
        );
    }

    #[test]
    fn app_list_dedupes_same_bundle_path_and_keeps_distinct_installs() {
        let apps = vec![
            app_info(
                "Ghostty",
                "com.mitchellh.ghostty",
                "/Applications/Ghostty.app",
                90,
            ),
            app_info(
                "Ghostty",
                "com.mitchellh.ghostty",
                "/Applications/Ghostty.app",
                91,
            ),
            app_info("Harness", "com.example.harness", "/tmp/A/Harness.app", 0),
            app_info("Harness", "com.example.harness", "/tmp/B/Harness.app", 0),
        ];
        let deduped = dedupe_apps(apps);
        assert_eq!(deduped.len(), 3);
        assert_eq!(
            deduped
                .iter()
                .filter(|app| app.bundle_id.as_deref() == Some("com.mitchellh.ghostty"))
                .count(),
            1
        );
        assert_eq!(
            deduped
                .iter()
                .filter(|app| app.bundle_id.as_deref() == Some("com.example.harness"))
                .count(),
            2
        );
    }

    #[test]
    fn post_action_resolution_never_relaunches_an_app_that_exited() {
        let stopped = app_info(
            "Calculator",
            "com.apple.calculator",
            "/System/Applications/Calculator.app",
            0,
        );
        let error =
            identity_and_launch_requirement("Calculator", &stopped, AppResolveMode::RunningOnly)
                .unwrap_err();
        assert_eq!(error.code, "app_no_longer_running");

        let (_, needs_launch) =
            identity_and_launch_requirement("Calculator", &stopped, AppResolveMode::LaunchIfNeeded)
                .unwrap();
        assert!(needs_launch);

        let running = app_info(
            "Calculator",
            "com.apple.calculator",
            "/System/Applications/Calculator.app",
            123,
        );
        let (_, needs_launch) =
            identity_and_launch_requirement("Calculator", &running, AppResolveMode::RunningOnly)
                .unwrap();
        assert!(!needs_launch);
    }

    #[test]
    fn batched_spotlight_parser_preserves_path_order() {
        let xml = r#"<plist><array>
          <dict><key>kMDItemLastUsedDate</key><date>2026-07-10T01:02:03Z</date><key>kMDItemUseCount</key><integer>42</integer></dict>
          <dict><key>kMDItemLastUsedDate</key><date>2026-07-09T04:05:06Z</date><key>kMDItemUseCount</key><integer>7</integer></dict>
        </array></plist>"#;
        let paths = vec!["/A.app".to_owned(), "/B.app".to_owned()];
        let parsed = parse_spotlight_usage_batch(xml, &paths);
        assert_eq!(
            parsed[&normalize_app_ref("/A.app")],
            (Some("2026-07-10T01:02:03Z".to_owned()), Some(42))
        );
        assert_eq!(
            parsed[&normalize_app_ref("/B.app")],
            (Some("2026-07-09T04:05:06Z".to_owned()), Some(7))
        );
    }

    #[test]
    fn snapshot_freshness_is_scoped_to_session_pid_and_window() {
        let store = SnapshotStore::default();
        let native_a = Arc::new(ToolState::default());
        let native_b = Arc::new(ToolState::default());
        store.insert("session-a", snapshot("AppA", native_a.clone()));
        store.insert("session-b", snapshot("AppB", native_b.clone()));
        let a = store.lookup("session-a", "AppA").unwrap();
        let b = store.lookup("session-b", "AppB").unwrap();
        assert!(Arc::ptr_eq(&a.native, &native_a));
        assert!(Arc::ptr_eq(&b.native, &native_b));

        // A later snapshot in A's own pid/window lane stales A's old alias.
        store.insert("session-a", snapshot("AppA-new-alias", native_a));
        assert!(matches!(
            store.lookup("session-a", "AppA"),
            Err(SnapshotLookupError::Stale)
        ));
        // B's independent lane remains current.
        assert!(store.lookup("session-b", "AppB").is_ok());

        store.clear_session("session-a");
        assert!(matches!(
            store.lookup("session-a", "AppA-new-alias"),
            Err(SnapshotLookupError::Missing)
        ));
        assert!(store
            .inner
            .lock()
            .unwrap()
            .latest_by_window
            .keys()
            .all(|(session, _, _)| session != "session-a"));
    }

    #[test]
    fn action_generation_is_rechecked_after_waiting_for_the_session_lane() {
        let store = SnapshotStore::default();
        let original = store.insert(
            "session-a",
            snapshot("AppA", Arc::new(ToolState::default())),
        );
        assert!(store.is_current("session-a", &original));

        let replacement = store.insert(
            "session-a",
            snapshot("AppA", Arc::new(ToolState::default())),
        );
        assert!(!store.is_current("session-a", &original));
        assert!(store.is_current("session-a", &replacement));
    }

    #[tokio::test]
    async fn lock_between_validation_and_dispatch_prevents_native_work() {
        let guardian = SessionLockGuardian::for_test(false);
        let epoch = guardian.begin().unwrap();
        guardian.validate(epoch).unwrap();

        // Inject the lifecycle edge in the exact gap between app/snapshot
        // validation and the native-dispatch gate.
        guardian.observe(true);
        let dispatched = Arc::new(AtomicBool::new(false));
        let dispatched_in_call = dispatched.clone();
        let result = run_guarded_dispatch(&guardian, epoch, move || async move {
            dispatched_in_call.store(true, Ordering::SeqCst);
        })
        .await;

        assert_eq!(result, Err(SessionLockError::Locked));
        assert!(!dispatched.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn lock_during_refresh_discards_the_capture() {
        let guardian = SessionLockGuardian::for_test(false);
        let epoch = guardian.begin().unwrap();
        let guardian_in_capture = guardian.clone();

        let result = run_guarded_capture(&guardian, epoch, move || async move {
            guardian_in_capture.observe(true);
            "captured state"
        })
        .await;

        assert_eq!(result, Err(SessionLockError::Locked));
    }

    #[test]
    fn lock_clears_every_compat_snapshot_and_cursor_until_fresh_state() {
        let guardian = SessionLockGuardian::for_test(false);
        let state = Arc::new(CompatState::with_guardian(guardian.clone()));
        register_lock_cleanup(&state);

        for session in ["lock-cleanup-a", "lock-cleanup-b"] {
            let native = state.native_for_session(session);
            native.cursor_registry.update_position(session, 40.0, 50.0);
            state
                .snapshots
                .insert(session, snapshot("AppA", native.clone()));
            assert!(native.cursor_registry.get(session).is_some());
        }

        guardian.observe(true);
        assert!(state.native_sessions.lock().unwrap().is_empty());
        for session in ["lock-cleanup-a", "lock-cleanup-b"] {
            assert!(matches!(
                state.snapshots.lookup(session, "AppA"),
                Err(SnapshotLookupError::Missing)
            ));
            assert!(state
                .lock_removed_cursors
                .lock()
                .unwrap()
                .contains(session));
        }

        guardian.observe(false);
        for session in ["lock-cleanup-a", "lock-cleanup-b"] {
            assert!(matches!(
                state.snapshots.lookup(session, "AppA"),
                Err(SnapshotLookupError::Missing)
            ));
        }
    }

    #[test]
    fn compat_session_end_clears_state_and_cursor_before_explicit_revival() {
        let session = "compat-cleanup-session-71D2A9";
        let state = Arc::new(CompatState::new());
        register_session_cleanup(&state);

        let native = state.native_for_session(session);
        native.cursor_registry.update_position(session, 40.0, 50.0);
        assert!(native.cursor_registry.get(session).is_some());
        state.operation_lock(session);
        state
            .snapshots
            .insert(session, snapshot("AppA", native.clone()));

        cua_driver_core::session::fire_session_end(session);
        assert!(cua_driver_core::session::is_session_ended(session));
        assert!(native.cursor_registry.get(session).is_none());
        assert!(!state.native_sessions.lock().unwrap().contains_key(session));
        assert!(!state.operation_locks.lock().unwrap().contains_key(session));
        assert!(matches!(
            state.snapshots.lookup(session, "AppA"),
            Err(SnapshotLookupError::Missing)
        ));

        native.cursor_registry.update_position(session, 60.0, 70.0);
        assert!(
            native.cursor_registry.get(session).is_none(),
            "a late action must not recreate cursor metadata after cleanup"
        );

        assert!(cua_driver_core::session::revive_session(session));
        let revived = state.native_for_session(session);
        revived.cursor_registry.update_position(session, 80.0, 90.0);
        assert!(revived.cursor_registry.get(session).is_some());
    }

    #[test]
    fn compat_indices_do_not_depend_on_global_native_tokens() {
        let mut app_snapshot = snapshot("AppA", Arc::new(ToolState::default()));
        app_snapshot.app.pid = 0x6c0d_0001;
        app_snapshot.element_indices.insert("3".to_owned());

        let registry = cua_driver_core::element_token::global();
        let first = registry.register_snapshot(app_snapshot.app.pid, 7, 4);
        let stale_token = cua_driver_core::element_token::token_for(first, 3);
        for window_id in 8..(8 + cua_driver_core::element_token::LRU_CAP_PER_PID as u32) {
            registry.register_snapshot(app_snapshot.app.pid, window_id, 4);
        }
        assert!(registry
            .resolve(app_snapshot.app.pid, &stale_token)
            .is_err());
        assert_eq!(snapshot_element_index(&app_snapshot, "3").unwrap(), 3);
    }

    #[test]
    fn degraded_refresh_after_success_leaves_consumed_snapshot_stale() {
        let store = SnapshotStore::default();
        let stored = store.insert(
            "session-a",
            snapshot("AppA", Arc::new(ToolState::default())),
        );
        store.invalidate("session-a", &stored);
        let result = refresh_warning_after_success(
            "click",
            "clicked".to_owned(),
            None,
            CompatError::new("screen_capture_failed", "capture failed").into_result(),
        );
        assert_ne!(result.is_error, Some(true));
        assert!(matches!(
            store.lookup("session-a", "AppA"),
            Err(SnapshotLookupError::Stale)
        ));

        let replacement = store.insert(
            "session-a",
            snapshot("AppA", Arc::new(ToolState::default())),
        );
        store.invalidate("session-a", &stored);
        assert_eq!(
            store.lookup("session-a", "AppA").unwrap().generation,
            replacement.generation,
            "invalidating an old action snapshot must not retire a concurrent refresh"
        );
    }

    #[test]
    fn target_policy_blocks_driver_auth_hosts_and_all_terminal_families() {
        let identity = |name: &str, bundle: &str| AppIdentity {
            requested: name.to_owned(),
            name: name.to_owned(),
            bundle_id: Some(bundle.to_owned()),
            launch_path: None,
            pid: 0,
        };
        for (name, bundle) in [
            ("CuaDriver", "com.trycua.driver"),
            ("SecurityAgent", "com.apple.SecurityAgent"),
            ("System Settings", "com.apple.systempreferences"),
            ("Codex", "com.openai.codex"),
            ("Codex Computer Use", "com.openai.sky.computer-use"),
            ("SkyComputerUseClient", "com.example.unknown-helper"),
            ("ChatGPT", "com.openai.chat"),
            ("Terminal", "com.apple.Terminal"),
            ("cmux", "com.cmuxterm.app"),
            ("cmux NIGHTLY", "com.cmuxterm.app.nightly"),
            ("Ghostty", "com.mitchellh.ghostty"),
            ("iTerm2", "com.googlecode.iterm2"),
            ("Warp", "dev.warp.Warp-Stable"),
            ("kitty", "net.kovidgoyal.kitty"),
            ("Alacritty", "org.alacritty"),
            ("WezTerm", "com.github.wez.wezterm"),
            ("Hyper", "co.zeit.hyper"),
        ] {
            let error = enforce_target_policy(&identity(name, bundle)).unwrap_err();
            assert_eq!(error.code, "target_not_allowed", "{name} should be blocked");
        }

        for name in [
            "Terminal",
            "cmux",
            "cmux BETA",
            "cmux DEV",
            "cmux NIGHTLY",
            "Ghostty",
            "iTerm",
            "iTerm2",
            "Warp",
            "Warp Preview",
            "kitty",
            "Alacritty",
            "WezTerm",
            "Hyper",
        ] {
            assert_eq!(
                enforce_target_policy(&identity(name, "com.example.unknown"))
                    .unwrap_err()
                    .code,
                "target_not_allowed",
                "{name} should be blocked by exact terminal name"
            );
        }
        assert!(enforce_target_policy(&identity("Hyperdrive", "com.example.hyperdrive")).is_ok());
    }

    #[test]
    fn xdotool_key_syntax_maps_to_native_key_and_modifiers() {
        assert_eq!(
            parse_xdotool_key("super+shift+c").unwrap(),
            ("c".to_owned(), vec!["cmd".to_owned(), "shift".to_owned()])
        );
        assert_eq!(
            parse_xdotool_key("KP_0").unwrap(),
            ("0".to_owned(), Vec::<String>::new())
        );
        assert_eq!(
            parse_xdotool_key("Return").unwrap(),
            ("return".to_owned(), Vec::<String>::new())
        );
    }

    #[test]
    fn select_text_disambiguates_and_counts_utf16() {
        let value = "😀 alpha, alpha omega";
        assert!(find_text_match(value, "alpha", None, None).is_err());
        let range = find_text_match(value, "alpha", Some("😀 "), Some(","))
            .unwrap()
            .unwrap();
        assert_eq!(&value[range.0..range.1], "alpha");
        assert_eq!(value[..range.0].encode_utf16().count(), 3);
    }

    #[test]
    fn logical_jpeg_and_coordinate_mapping_preserve_retina_target() {
        let image = image::DynamicImage::new_rgb8(464, 816);
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let jpeg = logical_jpeg(&BASE64.encode(png), 232, 408).unwrap();
        let decoded = image::load_from_memory(&BASE64.decode(jpeg).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (232, 408));

        let snapshot = snapshot("Retina", Arc::new(ToolState::default()));
        let compat_point = (91.5, 203.25);
        let native_point = compat_point_to_native(&snapshot, compat_point.0, compat_point.1);
        assert_eq!(native_point, (183.0, 406.5));
        // Native click divides screenshot pixels by the backing scale when it
        // converts to logical window coordinates, returning the exact point
        // the agent selected on the normalized JPEG.
        assert_eq!((native_point.0 / 2.0, native_point.1 / 2.0), compat_point);
    }

    #[test]
    fn non_actionable_text_clicks_fall_back_to_the_element_center() {
        assert!(element_supports_native_click(
            &["AXPress".to_owned()],
            "left"
        ));
        assert!(element_supports_native_click(
            &["AXShowMenu".to_owned()],
            "right"
        ));
        assert!(!element_supports_native_click(&[], "left"));
        assert!(!element_supports_native_click(&[], "right"));
        assert!(!element_supports_native_click(
            &["AXPress".to_owned()],
            "middle"
        ));

        let app_snapshot = snapshot("Retina", Arc::new(ToolState::default()));
        let bounds = crate::windows::WindowBounds {
            x: 100.0,
            y: 200.0,
            width: 232.0,
            height: 408.0,
        };
        assert_eq!(
            screen_point_to_native_click(&app_snapshot, &bounds, (116.0, 232.0)).unwrap(),
            (32.0, 64.0)
        );
        assert!(screen_point_to_native_click(&app_snapshot, &bounds, (99.0, 232.0)).is_err());
    }

    #[test]
    fn logical_jpeg_rejects_invalid_png_data() {
        assert!(logical_jpeg("not-base64", 10, 10).is_none());
        assert!(logical_jpeg(&BASE64.encode(b"not a png"), 10, 10).is_none());
    }

    #[test]
    fn state_result_is_text_then_logical_jpeg_and_removes_native_tokens() {
        let image = image::DynamicImage::new_rgb8(464, 816);
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut app_snapshot = snapshot("Calculator", Arc::new(ToolState::default()));
        app_snapshot.generation = 9;
        let result = state_result(
            ToolResult {
                content: vec![Content::image_png(BASE64.encode(png))],
                is_error: None,
                structured_content: Some(json!({
                    "tree_markdown": "- [0] AXWindow \"Calculator\"",
                    "snapshot_id": "s-native",
                    "elements": [{
                        "element_index": 0,
                        "element_token": "s-native:0",
                        "role": "AXWindow"
                    }]
                })),
            },
            &app_snapshot,
        )
        .unwrap();

        let wire = serde_json::to_value(&result).unwrap();
        assert_eq!(wire["content"][0]["type"], "text");
        assert_eq!(wire["content"][1]["type"], "image");
        assert_eq!(wire["content"][1]["mimeType"], "image/jpeg");
        assert!(wire.get("isError").is_none());
        assert!(matches!(result.content.first(), Some(Content::Text { .. })));
        let jpeg = match result.content.get(1) {
            Some(Content::Image {
                data, mime_type, ..
            }) => {
                assert_eq!(mime_type, "image/jpeg");
                data
            }
            other => panic!("expected JPEG as second content item, got {other:?}"),
        };
        let decoded = image::load_from_memory(&BASE64.decode(jpeg).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (232, 408));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["screenshot_mime_type"], "image/jpeg");
        assert_eq!(structured["compat_snapshot_generation"], 9);
        assert!(structured.get("snapshot_id").is_none());
        assert!(structured["elements"][0].get("element_token").is_none());
    }

    #[test]
    fn click_count_is_runtime_capped_without_schema_constraints() {
        assert_eq!(validated_click_count(&json!({})).unwrap(), 1);
        assert_eq!(
            validated_click_count(&json!({"click_count": 10})).unwrap(),
            10
        );
        for invalid in [
            json!({"click_count": 0}),
            json!({"click_count": 11}),
            json!({"click_count": -1}),
        ] {
            assert_eq!(
                validated_click_count(&invalid).unwrap_err().is_error,
                Some(true)
            );
        }
        assert!(
            defs()[CompatKind::Click.index()].input_schema["properties"]["click_count"]
                .get("minimum")
                .is_none()
        );
        assert!(
            defs()[CompatKind::Click.index()].input_schema["properties"]["click_count"]
                .get("maximum")
                .is_none()
        );
    }

    #[test]
    fn compat_points_must_be_finite_and_inside_latest_screenshot() {
        let app_snapshot = snapshot("Bounds", Arc::new(ToolState::default()));
        assert!(validate_compat_point(&app_snapshot, 0.0, 0.0, "point").is_ok());
        assert!(validate_compat_point(&app_snapshot, 231.999, 407.999, "point").is_ok());
        for (x, y) in [
            (-0.1, 0.0),
            (0.0, -0.1),
            (232.0, 0.0),
            (0.0, 408.0),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
        ] {
            assert!(
                validate_compat_point(&app_snapshot, x, y, "point").is_err(),
                "({x}, {y}) must be rejected"
            );
        }
    }

    #[test]
    fn scroll_pages_are_runtime_capped_without_schema_constraints() {
        assert_eq!(validated_scroll_pages(&json!({})).unwrap(), 1.0);
        assert_eq!(
            validated_scroll_pages(&json!({"pages": 0.25})).unwrap(),
            0.25
        );
        assert_eq!(validated_scroll_pages(&json!({"pages": 10})).unwrap(), 10.0);
        for invalid in [
            json!({"pages": 0}),
            json!({"pages": -1}),
            json!({"pages": 10.01}),
            json!({"pages": "many"}),
        ] {
            assert_eq!(
                validated_scroll_pages(&invalid).unwrap_err().is_error,
                Some(true)
            );
        }
        let schema = &defs()[CompatKind::Scroll.index()].input_schema["properties"]["pages"];
        assert!(schema.get("maximum").is_none());
        assert!(schema.get("exclusiveMinimum").is_none());
    }

    #[test]
    fn completed_action_with_failed_refresh_is_degraded_success_not_retryable_error() {
        let result = refresh_warning_after_success(
            "click",
            "clicked".to_owned(),
            Some(json!({"verified": true})),
            CompatError::new("screen_capture_failed", "capture failed").into_result(),
        );
        assert_ne!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["state_refresh_failed_after_success"], true);
        assert_eq!(structured["action_result"]["text"], "clicked");
        assert_eq!(structured["action_result"]["structured"]["verified"], true);
    }

    #[test]
    fn canonical_secondary_action_preserves_ax_vocabulary() {
        assert_eq!(canonical_ax_action("Raise"), "AXRaise");
        assert_eq!(canonical_ax_action("Move previous"), "AXMovePrevious");
        assert_eq!(canonical_ax_action("AXShowMenu"), "AXShowMenu");
    }

    #[test]
    fn notification_wait_resolves_immediately_without_waiting() {
        let mut waited = false;
        let result = resolve_after_notifications(
            Duration::from_secs(1),
            || Some(7),
            |_| {
                waited = true;
                false
            },
        );
        assert_eq!(result, Some(7));
        assert!(!waited);
    }

    #[test]
    fn notification_wait_rechecks_after_each_signal_and_stops_on_timeout() {
        let mut resolutions = 0;
        let mut signals = 0;
        let result = resolve_after_notifications(
            Duration::from_secs(1),
            || {
                resolutions += 1;
                (resolutions == 3).then_some("window")
            },
            |remaining| {
                assert!(!remaining.is_zero());
                signals += 1;
                true
            },
        );
        assert_eq!(result, Some("window"));
        assert_eq!(signals, 2);

        let mut final_resolve_count = 0;
        let timed_out: Option<()> = resolve_after_notifications(
            Duration::from_secs(1),
            || {
                final_resolve_count += 1;
                None
            },
            |_| false,
        );
        assert!(timed_out.is_none());
        assert_eq!(
            final_resolve_count, 2,
            "timeout performs one final race-closing resolve"
        );
    }

    #[tokio::test]
    async fn compat_focus_guard_seam_runs_action_without_frontmost_app() {
        let value = with_compat_focus_guard(42, None, "test.compat_focus", || async { 17 }).await;
        assert_eq!(value, 17);
    }
}
