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
use core_foundation::boolean::{kCFBooleanTrue, CFBoolean};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::CFString;
use cua_driver_core::{
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef, ToolRegistry},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

use super::ToolState;
use crate::ax::bindings::{
    copy_action_names, copy_string_attr, perform_action, AXUIElementCopyAttributeValue,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetAttributeValue, AXValueRef,
    _AXUIElementGetWindow, kAXErrorSuccess, kAXValueCFRangeType,
};

const ANONYMOUS_SESSION: &str = "__codex_compat_connection__";
const MAX_CLICK_COUNT: u64 = 10;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
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

    {
        let snapshots = state.snapshots.clone();
        let native_sessions = state.native_sessions.clone();
        cua_driver_core::session::register_session_end_hook(move |session_id| {
            snapshots.clear_session(session_id);
            native_sessions.lock().unwrap().remove(session_id);
        });
    }

    for kind in CompatKind::ALL {
        registry.register(Box::new(CompatTool {
            kind,
            state: state.clone(),
        }));
    }
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

    fn clear_session(&self, session: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.by_session.remove(session);
        inner
            .latest_by_window
            .retain(|(owner, _, _), _| owner != session);
    }
}

struct CompatState {
    snapshots: Arc<SnapshotStore>,
    native_sessions: Arc<Mutex<HashMap<String, Arc<ToolState>>>>,
}

impl CompatState {
    fn new() -> Self {
        Self {
            snapshots: Arc::new(SnapshotStore::default()),
            native_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn native_for_session(&self, session: &str) -> Arc<ToolState> {
        let mut sessions = self.native_sessions.lock().unwrap();
        sessions
            .entry(session.to_owned())
            .or_insert_with(|| Arc::new(ToolState::default()))
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
        if let Err(error) = ensure_interactive_session_unlocked() {
            return error.into_result();
        }
        let native = self.native_for_session(&session);
        let app = match resolve_or_launch_app(&app_ref).await {
            Ok(app) => app,
            Err(error) => return error.into_result(),
        };
        if let Err(error) = enforce_target_policy(&app) {
            return error.into_result();
        }

        let (window_id, window_title) = match wait_for_key_window(app.pid).await {
            Ok(window) => window,
            Err(error) => return error.into_result(),
        };

        let native_args = json!({
            "pid": app.pid,
            "window_id": window_id,
            "_session_id": session,
            "include_screenshot": true,
            "max_elements": 800,
            "max_depth": 20,
            "_codex_compat_full_ax_map": true,
        });
        let native_result = super::get_window_state::GetWindowStateTool::new(native.clone())
            .invoke(native_args)
            .await;
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
        let snapshot = self.snapshots.insert(
            &session,
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
        match state_result(native_result, &snapshot) {
            Ok(result) => result,
            Err(error) => error.into_result(),
        }
    }

    async fn run_action(&self, kind: CompatKind, args: Value) -> ToolResult {
        let app_ref = match required_string(&args, "app") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let session = session_key(&args);
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

        if let Err(error) = ensure_interactive_session_unlocked() {
            return error.into_result();
        }

        if let Err(error) = validate_live_snapshot(&snapshot) {
            return error.into_result();
        }

        let action_name = defs()[kind.index()].name.clone();
        let action_result = match kind {
            CompatKind::Click => self.click(&snapshot, &args, &session).await,
            CompatKind::Drag => self.drag(&snapshot, &args, &session).await,
            CompatKind::PerformSecondaryAction => {
                self.perform_secondary_action(&snapshot, &args).await
            }
            CompatKind::PressKey => self.press_key(&snapshot, &args, &session).await,
            CompatKind::Scroll => self.scroll(&snapshot, &args, &session).await,
            CompatKind::SelectText => self.select_text(&snapshot, &args).await,
            CompatKind::SetValue => self.set_value(&snapshot, &args, &session).await,
            CompatKind::TypeText => self.type_text(&snapshot, &args, &session).await,
            CompatKind::ListApps | CompatKind::GetAppState => unreachable!(),
        };

        if action_result.is_error == Some(true) {
            return action_error(&action_name, action_result);
        }

        let action_summary = first_text(&action_result);
        let action_structured = action_result.structured_content.clone();
        let mut refreshed = self.get_app_state(&args).await;
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

    async fn click(&self, snapshot: &AppSnapshot, args: &Value, session: &str) -> ToolResult {
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
        if let Some(index) = element_index {
            let native_index = match snapshot_element_index(snapshot, index) {
                Ok(index) => index,
                Err(result) => return result,
            };
            native_args["element_index"] = json!(native_index);
        } else {
            let (native_x, native_y) = compat_point_to_native(snapshot, x.unwrap(), y.unwrap());
            native_args["x"] = json!(native_x);
            native_args["y"] = json!(native_y);
        }

        // Native AX clicks are single-press operations. Repeat explicitly when
        // Codex asks for a multi-click so the compatibility field is honored on
        // both AX and pixel paths (the native pixel path handles count itself).
        if element_index.is_some() && click_count > 1 {
            let mut last = ToolResult::default();
            for _ in 0..click_count {
                last = super::click::ClickTool::new(snapshot.native.clone())
                    .invoke(native_args.clone())
                    .await;
                if last.is_error == Some(true) {
                    return last;
                }
            }
            last
        } else {
            super::click::ClickTool::new(snapshot.native.clone())
                .invoke(native_args)
                .await
        }
    }

    async fn drag(&self, snapshot: &AppSnapshot, args: &Value, session: &str) -> ToolResult {
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
        let (from_x, from_y) = compat_point_to_native(snapshot, from_x, from_y);
        let (to_x, to_y) = compat_point_to_native(snapshot, to_x, to_y);
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

    async fn perform_secondary_action(&self, snapshot: &AppSnapshot, args: &Value) -> ToolResult {
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
        let result = tokio::task::spawn_blocking(move || {
            let available = unsafe { copy_action_names(element_ptr as AXUIElementRef) };
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
            let error = unsafe { perform_action(element_ptr as AXUIElementRef, &actual) };
            if error != kAXErrorSuccess {
                anyhow::bail!("AXUIElementPerformAction({actual}) failed with error {error}");
            }
            Ok::<String, anyhow::Error>(actual)
        })
        .await;
        match result {
            Ok(Ok(actual)) => ToolResult::text(format!("Performed {actual} on element {index}.")),
            Ok(Err(error)) => ToolResult::error(error.to_string()),
            Err(error) => ToolResult::error(format!("Secondary action task failed: {error}")),
        }
    }

    async fn press_key(&self, snapshot: &AppSnapshot, args: &Value, session: &str) -> ToolResult {
        let raw_key = match required_string(args, "key") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let (key, modifiers) = match parse_xdotool_key(&raw_key) {
            Ok(parsed) => parsed,
            Err(message) => return ToolResult::error(message),
        };
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

    async fn scroll(&self, snapshot: &AppSnapshot, args: &Value, session: &str) -> ToolResult {
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
        let pages = args.get("pages").and_then(Value::as_f64).unwrap_or(1.0);
        if !pages.is_finite() || pages <= 0.0 {
            return ToolResult::error("pages must be a positive finite number.");
        }
        // One native page is five 120px line steps. Integral page counts stay
        // page-granular; fractions use line granularity so 0.2-page increments
        // remain meaningful without changing the native API.
        let (by, amount) = if pages.fract().abs() < f64::EPSILON {
            ("page", pages.min(50.0) as u64)
        } else {
            ("line", (pages * 5.0).round().clamp(1.0, 50.0) as u64)
        };
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

    async fn select_text(&self, snapshot: &AppSnapshot, args: &Value) -> ToolResult {
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
        let result = tokio::task::spawn_blocking(move || {
            select_text_range(
                element_ptr,
                &text,
                prefix.as_deref(),
                suffix.as_deref(),
                &selection,
            )
        })
        .await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Updated text selection on element {index}.")),
            Ok(Err(error)) => ToolResult::error(error.to_string()),
            Err(error) => ToolResult::error(format!("Text selection task failed: {error}")),
        }
    }

    async fn set_value(&self, snapshot: &AppSnapshot, args: &Value, session: &str) -> ToolResult {
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

    async fn type_text(&self, snapshot: &AppSnapshot, args: &Value, session: &str) -> ToolResult {
        let text = match required_string(args, "text") {
            Ok(value) => value,
            Err(result) => return result,
        };
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
        "Computer Use state (cmux-cua Codex compatibility)\n<app_state>\n\
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
    let jpeg = logical_jpeg(png, snapshot.logical_width, snapshot.logical_height).ok_or_else(|| {
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

async fn resolve_or_launch_app(app_ref: &str) -> Result<AppIdentity, CompatError> {
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
    let mut identity = AppIdentity {
        requested: query.clone(),
        name: info.name.clone(),
        bundle_id: info.bundle_id.clone(),
        launch_path: info.launch_path.clone(),
        pid: info.pid,
    };
    enforce_target_policy(&identity)?;

    if info.running && info.pid > 0 {
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
    let protected_bundle = matches!(
        bundle_lower.as_str(),
        "com.apple.loginwindow"
            | "com.apple.securityagent"
            | "com.apple.coreservicesuiagent"
            | "com.apple.authorizationhost"
            | "com.apple.keychainaccess"
            | "com.apple.passwords"
            | "com.apple.systempreferences"
            | "com.apple.terminal"
            | "com.mitchellh.ghostty"
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
            | "terminal"
            | "ghostty"
    );
    if self_target || host_target || driver_target || protected_bundle || protected_name {
        return Err(CompatError::new(
            "target_not_allowed",
            format!(
                "Computer Use is not allowed to target '{}' ({bundle}). The driver/host and macOS security-sensitive or authentication surfaces are protected.",
                app.name
            ),
        ));
    }
    Ok(())
}

fn ensure_interactive_session_unlocked() -> Result<(), CompatError> {
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return Err(CompatError::new(
            "session_state_unavailable",
            "Computer Use could not verify the current macOS login session. Refusing to inspect or control apps until an unlocked GUI session is available.",
        ));
    }
    let session: CFDictionary<CFString, CFBoolean> =
        unsafe { TCFType::wrap_under_create_rule(raw) };
    let key = CFString::new("CGSSessionScreenIsLocked");
    let locked = session
        .find(&key)
        .map(|value| value.as_concrete_TypeRef() == unsafe { kCFBooleanTrue })
        .unwrap_or(false);
    if locked {
        return Err(CompatError::new(
            "screen_locked",
            "Computer Use is unavailable while the macOS screen is locked.",
        ));
    }
    Ok(())
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
    for _ in 0..50 {
        let window = tokio::task::spawn_blocking(move || {
            let id =
                key_window_id(pid).or_else(|| crate::windows::resolve_main_window_id(pid).ok())?;
            let title = crate::windows::all_windows()
                .into_iter()
                .find(|window| window.pid == pid && window.window_id == id)
                .map(|window| window.title)
                .unwrap_or_default();
            Some((id, title))
        })
        .await
        .ok()
        .flatten();
        if let Some(window) = window {
            return Ok(window);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(CompatError::new(
        "app_has_no_window",
        format!("App pid {pid} did not expose a key window within 5 seconds."),
    ))
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
            let expected_read_only = matches!(
                tool["name"].as_str(),
                Some("list_apps" | "get_app_state")
            );
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
    fn target_policy_blocks_driver_auth_terminal_and_ghostty_but_allows_cmux() {
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
            ("Terminal", "com.apple.Terminal"),
            ("Ghostty", "com.mitchellh.ghostty"),
        ] {
            let error = enforce_target_policy(&identity(name, bundle)).unwrap_err();
            assert_eq!(error.code, "target_not_allowed", "{name} should be blocked");
        }
        assert!(enforce_target_policy(&identity("cmux", "com.cmuxterm.app")).is_ok());
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
            Some(Content::Image { data, mime_type, .. }) => {
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
        assert_eq!(validated_click_count(&json!({"click_count": 10})).unwrap(), 10);
        for invalid in [json!({"click_count": 0}), json!({"click_count": 11}), json!({"click_count": -1})] {
            assert_eq!(validated_click_count(&invalid).unwrap_err().is_error, Some(true));
        }
        assert!(defs()[CompatKind::Click.index()].input_schema["properties"]["click_count"]
            .get("minimum")
            .is_none());
        assert!(defs()[CompatKind::Click.index()].input_schema["properties"]["click_count"]
            .get("maximum")
            .is_none());
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
}
