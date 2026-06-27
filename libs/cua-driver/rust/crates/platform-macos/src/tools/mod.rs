//! MCP tool implementations for macOS.

mod list_apps;
mod list_windows;
mod get_window_state;
mod launch_app;
mod kill_app;
mod bring_to_front;
mod click;
mod double_click;
mod right_click;
mod drag;
mod type_text;
mod press_key;
mod hotkey;
mod set_value;
mod scroll;
// `screenshot` / `screenshot_compat` modules removed in PR #1692 —
// `get_window_state` capture_mode:"vision" is the canonical screenshot
// path. The capture functions they wrapped (ScreenCaptureKit, CGWindow,
// etc.) live elsewhere under CuaDriverCore::Capture and are reached
// through GetWindowStateTool.
pub(crate) mod get_screen_size;
mod get_desktop_state;
mod get_cursor_position;
mod move_cursor;
mod cursor_tools;
mod check_permissions;
mod get_config;
mod set_config;
mod get_accessibility_tree;
mod health_report;
mod zoom;
mod type_text_chars;
mod page;

use cua_driver_core::tool::ToolRegistry;
use std::sync::Arc;
use std::collections::HashMap;

use crate::{
    ax::cache::ElementCache,
    cursor::state::CursorRegistry,
};

/// Per-process zoom context — stores the padded crop origin and resize scale
/// from the most recent `zoom` call, so `click(from_zoom=true)` can translate
/// zoom-image pixel coordinates back to full-window coordinates.
#[derive(Clone, Copy, Debug)]
pub struct ZoomContext {
    /// Padded crop X origin in full-window pixel space.
    pub origin_x: f64,
    /// Padded crop Y origin in full-window pixel space.
    pub origin_y: f64,
    /// Inverse resize scale: `cw / out_w` (1.0 = no downscale).
    pub scale_inv: f64,
}

impl ZoomContext {
    /// Translate a zoom-image coordinate `(px, py)` to full-window pixel coordinates.
    pub fn zoom_to_window(&self, px: f64, py: f64) -> (f64, f64) {
        (
            self.origin_x + px * self.scale_inv,
            self.origin_y + py * self.scale_inv,
        )
    }
}

/// Thread-safe per-pid zoom context registry.
pub struct ZoomRegistry {
    inner: std::sync::Mutex<HashMap<i32, ZoomContext>>,
}

impl ZoomRegistry {
    pub fn new() -> Self { Self { inner: std::sync::Mutex::new(HashMap::new()) } }

    pub fn set(&self, pid: i32, ctx: ZoomContext) {
        self.inner.lock().unwrap().insert(pid, ctx);
    }

    pub fn get(&self, pid: i32) -> Option<ZoomContext> {
        self.inner.lock().unwrap().get(&pid).copied()
    }
}

/// Tracks the per-pid ratio applied by `max_image_dimension` downscaling.
///
/// `ratio = original_dim / resized_dim` — multiply resized image coordinates
/// by this to recover original (native) window-local pixel coordinates.
/// Mirrors Swift's `ImageResizeRegistry`.
pub struct ResizeRegistry {
    inner: std::sync::Mutex<HashMap<i32, f64>>,
}

impl ResizeRegistry {
    pub fn new() -> Self { Self { inner: std::sync::Mutex::new(HashMap::new()) } }

    /// Record that pid's screenshot was downscaled by `ratio`.
    pub fn set_ratio(&self, pid: i32, ratio: f64) {
        self.inner.lock().unwrap().insert(pid, ratio);
    }

    /// Remove the ratio entry (no active downscale).
    pub fn clear_ratio(&self, pid: i32) {
        self.inner.lock().unwrap().remove(&pid);
    }

    /// Returns the most recent ratio, or `None` if no downscale happened.
    pub fn ratio(&self, pid: i32) -> Option<f64> {
        self.inner.lock().unwrap().get(&pid).copied()
    }
}

/// Runtime-mutable driver configuration persisted across calls within a session.
pub struct DriverConfig {
    /// Default capture_mode for get_window_state when not specified per-call.
    pub capture_mode: String,
    /// Capture scope for get_window_state: "window" (default) crops to the
    /// target window; "desktop" captures the full display.
    pub capture_scope: String,
    /// Max screenshot dimension (0 = no limit). Applied during screenshot/zoom.
    /// Default 1568 matches Swift's `CuaDriverConfig.defaultMaxImageDimension` —
    /// the long edge is downscaled to this before encoding.
    pub max_image_dimension: u32,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            capture_mode: "som".to_owned(),
            capture_scope: "window".to_string(),
            max_image_dimension: 1568,
        }
    }
}

/// Path to the persistent JSON config file shared by the CLI and MCP session.
pub fn config_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(format!("{home}/.cua-driver/config.json"))
}

/// Load `DriverConfig` from `~/.cua-driver/config.json`, falling back to
/// defaults for any missing or unrecognised keys.  Called at MCP startup so
/// that `cua-driver config set capture_mode vision` (CLI) carries over into
/// the next MCP session without requiring a per-call `set_config`.
pub fn load_driver_config() -> DriverConfig {
    let mut cfg = DriverConfig::default();
    let path = config_file_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return cfg,  // no file yet — use defaults
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return cfg,  // malformed file — use defaults
    };
    if let Some(v) = json.get("capture_mode").and_then(|v| v.as_str()) {
        cfg.capture_mode = v.to_owned();
    }
    if let Some(v) = json.get("capture_scope").and_then(|v| v.as_str()) {
        cfg.capture_scope = v.to_owned();
    }
    if let Some(v) = json.get("max_image_dimension").and_then(|v| v.as_u64()) {
        if let Ok(v32) = u32::try_from(v) {
            cfg.max_image_dimension = v32;
        }
    }
    cfg
}

/// Persist a single key/value pair to `~/.cua-driver/config.json`.
/// Merges with any existing file contents so other keys are preserved.
/// Returns `Err` if the directory cannot be created or the file cannot be written.
pub fn write_driver_config_key(key: &str, value: &serde_json::Value) -> Result<(), String> {
    let path = config_file_path();
    let mut json: serde_json::Value = path
        .exists()
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    json[key] = value.clone();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(())
}

/// Per-session config overrides layered over the global persisted `DriverConfig`.
///
/// The cua-driver daemon is one shared process: every `cua-driver mcp` proxy
/// connects to it and shares its `ToolState`. `DriverConfig` is therefore
/// multi-tenant AND persisted to disk — so without session scoping, session A's
/// `set_config capture_mode=vision` clobbers session B's value and flips the
/// on-disk default under everyone. These overrides fix that: a named MCP session
/// gets an in-memory, non-persisted override keyed by its `_session_id`; the
/// anonymous session (CLI / one-shot `call`) still writes the shared global +
/// disk. `None` fields mean "fall through to the global layer".
#[derive(Clone, Default)]
pub struct ConfigOverrides {
    pub capture_mode: Option<String>,
    pub capture_scope: Option<String>,
    pub max_image_dimension: Option<u32>,
}

/// Thread-safe map of `session_id` → `ConfigOverrides`, mirroring
/// `CursorRegistry`'s registry shape. Cleared per session on `session_end`.
pub struct SessionConfigRegistry {
    inner: std::sync::Mutex<HashMap<String, ConfigOverrides>>,
}

impl SessionConfigRegistry {
    pub fn new() -> Self { Self { inner: std::sync::Mutex::new(HashMap::new()) } }

    /// Merge `delta` into `session`'s overrides (only the `Some` fields of
    /// `delta` overwrite; existing overrides for unset fields are preserved).
    pub fn set(&self, session: &str, delta: ConfigOverrides) {
        // Write-boundary resurrection guard: keyed by session_id, so an
        // in-flight set_config that lands AFTER session_end (passed the dispatch
        // gate, then the proxy died and the reaper cleared this session's
        // overrides) must NOT re-create the entry — it would be invisible and
        // never reaped again. `fire_session_end` marks ENDED_SESSIONS *before*
        // running the config-clear hook, so this check is authoritative.
        if cua_driver_core::session::is_session_ended(session) {
            return;
        }
        let mut map = self.inner.lock().unwrap();
        let entry = map.entry(session.to_owned()).or_default();
        if delta.capture_mode.is_some() {
            entry.capture_mode = delta.capture_mode;
        }
        if delta.capture_scope.is_some() {
            entry.capture_scope = delta.capture_scope;
        }
        if delta.max_image_dimension.is_some() {
            entry.max_image_dimension = delta.max_image_dimension;
        }
    }

    /// Resolve the effective config for `session`, layering its overrides over
    /// the global `DriverConfig`. `session = None` (anonymous) returns the
    /// global values verbatim — today's behavior.
    pub fn effective(&self, session: Option<&str>, global: &DriverConfig) -> (String, u32) {
        let ov = session.and_then(|s| self.inner.lock().unwrap().get(s).cloned());
        match ov {
            Some(ov) => (
                ov.capture_mode.unwrap_or_else(|| global.capture_mode.clone()),
                ov.max_image_dimension.unwrap_or(global.max_image_dimension),
            ),
            None => (global.capture_mode.clone(), global.max_image_dimension),
        }
    }

    /// Resolve the effective `capture_scope` for `session`, layering its
    /// override over the global `DriverConfig`. Kept separate from `effective()`
    /// so existing `(String, u32)` call sites stay unchanged. `session = None`
    /// (anonymous) returns the global scope verbatim.
    pub fn effective_scope(&self, session_id: Option<&str>, global: &DriverConfig) -> String {
        let ov = session_id.and_then(|s| self.inner.lock().unwrap().get(s).cloned());
        match ov {
            Some(ov) => ov.capture_scope.unwrap_or_else(|| global.capture_scope.clone()),
            None => global.capture_scope.clone(),
        }
    }

    /// Drop `session`'s overrides. No-op for an unknown id (so `session_end`
    /// for an anonymous / never-set session is harmless).
    pub fn clear(&self, session: &str) {
        self.inner.lock().unwrap().remove(session);
    }
}

impl Default for SessionConfigRegistry {
    fn default() -> Self { Self::new() }
}

/// Shared state passed to all tools.
pub struct ToolState {
    pub element_cache: Arc<ElementCache>,
    pub cursor_registry: Arc<CursorRegistry>,
    pub zoom_registry: Arc<ZoomRegistry>,
    pub resize_registry: Arc<ResizeRegistry>,
    /// Global, disk-persisted config — the base layer and the only one the
    /// anonymous session / CLI writes.
    pub config: Arc<std::sync::RwLock<DriverConfig>>,
    /// Per-MCP-session in-memory config overrides layered over `config`.
    pub session_config: Arc<SessionConfigRegistry>,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            element_cache: Arc::new(ElementCache::new()),
            cursor_registry: Arc::new(CursorRegistry::new()),
            zoom_registry: Arc::new(ZoomRegistry::new()),
            resize_registry: Arc::new(ResizeRegistry::new()),
            // Load persisted config from ~/.cua-driver/config.json so that
            // `cua-driver config set` changes carry over into MCP sessions.
            config: Arc::new(std::sync::RwLock::new(load_driver_config())),
            session_config: Arc::new(SessionConfigRegistry::new()),
        }
    }
}

/// Register all macOS tools into the registry. `compat=true` swaps the
/// regular `screenshot` tool for the Claude Code computer-use compat
/// variant — same name, stricter args, window-scoped JPEG @ 85% + a text
/// note telling the caller to use pixel-addressed tools.
pub fn register_all(registry: &mut ToolRegistry, compat: bool) {
    let state = Arc::new(ToolState::default());
    // Share the element cache with the recording-hook layer so it can
    // resolve element_index → window-local screenshot coords for click.png.
    crate::recording_hooks::set_element_cache(state.element_cache.clone());

    // Drop a disconnecting session's config overrides + owned cursor on
    // `session_end`. The daemon fans the session id out to this hook;
    // recording ownership is handled separately on the core RecordingSession.
    {
        let session_config = state.session_config.clone();
        let cursor_registry = state.cursor_registry.clone();
        cua_driver_core::session::register_session_end_hook(move |session_id| {
            session_config.clear(session_id);
            // Per-session agent cursor: the session_id is the cursor key when
            // the caller gave no explicit cursor_id, so dropping it here both
            // prunes the metadata registry and stops the overlay painting that
            // session's cursor. Both paths guard "default" so the anonymous /
            // one-shot cursor survives. Anonymous sessions that never created a
            // cursor are a harmless no-op.
            cursor_registry.remove(session_id);
            crate::cursor::overlay::remove_cursor(session_id.to_owned());
        });
    }

    registry.register(Box::new(list_apps::ListAppsTool));
    registry.register(Box::new(list_windows::ListWindowsTool));
    registry.register(Box::new(get_window_state::GetWindowStateTool::new(state.clone())));
    registry.register(Box::new(launch_app::LaunchAppTool));
    registry.register(Box::new(kill_app::KillAppTool));
    registry.register(Box::new(bring_to_front::BringToFrontTool));
    registry.register(Box::new(click::ClickTool::new(state.clone())));
    registry.register(Box::new(double_click::DoubleClickTool::new(state.clone())));
    registry.register(Box::new(right_click::RightClickTool::new(state.clone())));
    registry.register(Box::new(drag::DragTool::new(state.clone())));
    registry.register(Box::new(type_text::TypeTextTool::new(state.clone())));
    registry.register(Box::new(press_key::PressKeyTool::new(state.clone())));
    registry.register(Box::new(hotkey::HotkeyTool::new(state.clone())));
    registry.register(Box::new(set_value::SetValueTool::new(state.clone())));
    registry.register(Box::new(scroll::ScrollTool::new(state.clone())));
    // `screenshot` removed - see the matching comment in
    // platform-windows/src/tools/impl_.rs::build_registry. Canonical
    // screenshot path is `get_window_state` with `capture_mode:"vision"`.
    let _ = compat;
    registry.register(Box::new(get_screen_size::GetScreenSizeTool));
    registry.register(Box::new(get_desktop_state::GetDesktopStateTool));
    registry.register(Box::new(get_cursor_position::GetCursorPositionTool));
    registry.register(Box::new(move_cursor::MoveCursorTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::SetAgentCursorEnabledTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::SetAgentCursorMotionTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::SetAgentCursorStyleTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::GetAgentCursorStateTool::new(state.clone())));
    registry.register(Box::new(check_permissions::CheckPermissionsTool));
    // `health_report` — single-call end-to-end diagnostics. Stable
    // schema_version="1" contract aimed at downstream consumers
    // (Hermes Agent's `hermes computer-use doctor`, NousResearch/
    // hermes-agent#47065) who must NOT have to know cua-driver
    // internals. Provider is platform-specific; tool plumbing is in
    // `cua_driver_core::health_report`.
    registry.register(Box::new(cua_driver_core::health_report::HealthReportTool::new(
        Arc::new(health_report::MacosHealthProvider),
    )));
    registry.register(Box::new(get_config::GetConfigTool::new(state.clone())));
    registry.register(Box::new(set_config::SetConfigTool::new(state.clone())));
    registry.register(Box::new(get_accessibility_tree::GetAccessibilityTreeTool::new(state.clone())));
    registry.register(Box::new(zoom::ZoomTool { state: state.clone() }));
    // `type_text_chars` is intentionally NOT registered — Swift treats it as
    // a deprecated alias for `type_text` resolved at invoke time in
    // mcp-server's `ToolRegistry::invoke`. Keeping it out of the registry
    // means it doesn't show up in `tools/list` either, matching Swift's
    // ToolRegistry.swift (`type_text_chars` not in `handlers`) and the
    // platform-windows::build_registry which uses the same convention.
    // Touch the struct so it stays in this crate for the alias resolver.
    let _: &type_text_chars::TypeTextCharsTool = &type_text_chars::TypeTextCharsTool::new(state.clone());
    // Cross-platform `page` tool definition lives in mcp-server; macOS plugs in
    // its Apple-Events / CDP / AX-tree backend here.
    registry.register(Box::new(cua_driver_core::page::PageTool::new(
        Arc::new(page::MacOsPageBackend::new(state.clone())),
    )));
    // Recording / replay + session-lifecycle tools are platform-independent.
    registry.register_recording_tools();
    registry.register_session_tools();
}

#[cfg(test)]
mod session_config_guard_tests {
    use super::*;
    use cua_driver_core::session::fire_session_end;

    fn overrides(mode: &str) -> ConfigOverrides {
        ConfigOverrides { capture_mode: Some(mode.to_owned()), capture_scope: None, max_image_dimension: None }
    }

    #[test]
    fn default_capture_scope_is_window() {
        assert_eq!(DriverConfig::default().capture_scope, "window");
    }

    #[test]
    fn effective_scope_uses_session_override_then_global() {
        let reg = SessionConfigRegistry::new();
        let global = DriverConfig::default();
        let sid = "wb-scope-live-C1D2E3";
        assert!(!cua_driver_core::session::is_session_ended(sid));

        // No override yet → falls back to global ("window").
        assert_eq!(reg.effective_scope(Some(sid), &global), "window");
        // Anonymous → global.
        assert_eq!(reg.effective_scope(None, &global), "window");

        // Live session override applies.
        reg.set(sid, ConfigOverrides {
            capture_mode: None,
            capture_scope: Some("desktop".to_owned()),
            max_image_dimension: None,
        });
        assert_eq!(reg.effective_scope(Some(sid), &global), "desktop");
        // Other sessions / anonymous still see global.
        assert_eq!(reg.effective_scope(None, &global), "window");
    }

    #[test]
    fn ended_session_config_set_is_noop() {
        // THE FIX (config side): an ended session id keys the overrides map, so
        // an in-flight set_config after session_end must not re-create the entry
        // the reaper's clear hook removed. effective() then falls back to global.
        let reg = SessionConfigRegistry::new();
        let global = DriverConfig::default();
        let sid = "wb-config-ended-Q9R8S7";
        fire_session_end(sid);
        assert!(cua_driver_core::session::is_session_ended(sid));

        reg.set(sid, overrides("raw"));
        let (mode, _) = reg.effective(Some(sid), &global);
        assert_eq!(mode, global.capture_mode, "ended session must not get an override entry");
    }

    #[test]
    fn live_session_config_set_takes_effect() {
        let reg = SessionConfigRegistry::new();
        let global = DriverConfig::default();
        let sid = "wb-config-live-T1U2V3";
        assert!(!cua_driver_core::session::is_session_ended(sid));
        reg.set(sid, overrides("raw"));
        let (mode, _) = reg.effective(Some(sid), &global);
        assert_eq!(mode, "raw", "live session override must apply");
    }
}

// RecordingSession lives in cua-driver-core, but its `start()` pulls in the
// macOS cursor sampler (CoreGraphics), so the start-guard test runs here in
// platform-macos where build.rs links the frameworks — the core crate's test
// binary has no CoreGraphics linkage.
#[cfg(test)]
mod recording_start_guard_tests {
    use cua_driver_core::recording::RecordingSession;
    use cua_driver_core::session::fire_session_end;

    #[test]
    fn start_refuses_for_ended_session_owner() {
        // THE FIX (recording side): an in-flight start_recording owned by a
        // session that already ended would leak an ffmpeg/SCStream process owned
        // by a dead session that is never reaped. start() must refuse.
        let rec = RecordingSession::new();
        let sid = "wb-recording-ended-W4X5Y6";
        fire_session_end(sid);
        assert!(cua_driver_core::session::is_session_ended(sid));

        let dir = std::env::temp_dir().join("wb-rec-ended");
        let err = rec.start(dir.to_str().unwrap(), false, Some(sid));
        assert!(err.is_err(), "start for an ended session owner must error");
        assert!(!rec.current_state().enabled, "no recording may start for a dead session");
    }

    #[test]
    fn start_succeeds_for_live_session_owner() {
        let rec = RecordingSession::new();
        let sid = "wb-recording-live-Z7A8B9";
        assert!(!cua_driver_core::session::is_session_ended(sid));
        let dir = std::env::temp_dir().join("wb-rec-live");
        // record_video=false avoids spawning ffmpeg in the test.
        let ok = rec.start(dir.to_str().unwrap(), false, Some(sid));
        assert!(ok.is_ok(), "start for a live session owner must succeed");
        assert!(rec.current_state().enabled);
        let _ = rec.stop_owner(Some(sid));
    }

    #[test]
    fn start_succeeds_for_anonymous_owner() {
        // owner = None (CLI one-shot / legacy shim) is never gated.
        let rec = RecordingSession::new();
        let dir = std::env::temp_dir().join("wb-rec-anon");
        let ok = rec.start(dir.to_str().unwrap(), false, None);
        assert!(ok.is_ok(), "anonymous start must never be gated");
        assert!(rec.current_state().enabled);
        let _ = rec.stop_owner(None);
    }
}
