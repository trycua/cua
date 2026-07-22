//! MCP tool implementations for macOS.

mod bring_to_front;
mod click;
mod double_click;
mod drag;
mod get_window_state;
mod hotkey;
mod kill_app;
mod launch_app;
mod list_apps;
mod list_windows;
mod press_key;
mod right_click;
mod scroll;
mod set_value;
mod type_text;
// `screenshot` / `screenshot_compat` modules removed in PR #1692 —
// `get_window_state` capture_mode:"vision" is the canonical screenshot
// path. The capture functions they wrapped (ScreenCaptureKit, CGWindow,
// etc.) live elsewhere under CuaDriverCore::Capture and are reached
// through GetWindowStateTool.
mod check_permissions;
mod cursor_tools;
mod get_accessibility_tree;
mod get_config;
mod get_cursor_position;
mod get_desktop_state;
pub(crate) mod get_screen_size;
mod health_report;
mod move_cursor;
mod page;
mod set_config;
mod type_text_chars;
mod zoom;

use cua_driver_core::tool::ToolRegistry;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{ax::cache::ElementCache, cursor::state::CursorRegistry};

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

/// Input delivery modality — the agent-selected rung of the best-effort-background
/// ladder, passed per call (never a stored/config setting).
///
/// - `Background` (default): post synthetic input to the pid without fronting.
/// - `Foreground`: briefly front the target window, act, then restore the prior
///   frontmost (see [`crate::input::skylight::with_foreground_assist`]). The
///   agent's vision-driven last resort — and the only way `click` reaches a
///   foreground rung. Orthogonal to addressing (`element_index` vs `x/y`, which
///   selects AX vs pixel).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DeliveryMode {
    #[default]
    Background,
    Foreground,
}

impl DeliveryMode {
    /// Parse the per-call `delivery_mode` argument. Anything other than an
    /// explicit case-insensitive `"foreground"` resolves to `Background` — the
    /// correct default, so an omitted/garbage value never silently fronts.
    pub fn parse(arg: Option<&str>) -> Self {
        match arg {
            Some(s) if s.eq_ignore_ascii_case("foreground") => Self::Foreground,
            _ => Self::Background,
        }
    }

    pub fn is_foreground(self) -> bool {
        matches!(self, Self::Foreground)
    }
}

/// Finish the post-action observation window. Embedded interactive clients
/// that already observe the target continuously may opt out through the
/// private registry argument to avoid adding a one-second acknowledgement
/// delay to every input event. Regular MCP callers retain the full observer.
pub(crate) async fn finish_window_observation(
    snapshot: crate::window_change_detector::Snapshot,
    args: &serde_json::Value,
) -> crate::window_change_detector::Changes {
    if args
        .get("_skip_window_change_detection")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        drop(snapshot);
        crate::window_change_detector::Changes::no_change()
    } else {
        snapshot.detect_async().await
    }
}

#[cfg(test)]
mod interactive_observation_tests {
    use super::*;

    #[tokio::test]
    async fn embedded_interactive_input_can_finish_without_polling() {
        let snapshot = crate::window_change_detector::WindowChangeDetector::snapshot(None);
        let changes = finish_window_observation(
            snapshot,
            &serde_json::json!({"_skip_window_change_detection": true}),
        )
        .await;
        assert!(!changes.needs_restore());
    }
}

/// px-focus for the keyboard family (type_text / press_key / hotkey): focus the
/// element at (x,y) before a keystroke — the *element px action* form of a
/// keyboard tool. Prefer non-destructive AX focus so an existing selection is
/// retained; the foreground rung falls back to a real pixel click when needed.
/// Reuses ClickTool's exact coordinate translation and delivery mode.
/// `Ok(())` on success; `Err(ToolResult)` short-circuits the caller.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn focus_by_pixel(
    state: &Arc<ToolState>,
    pid: i32,
    window_id: Option<u32>,
    x: f64,
    y: f64,
    foreground: bool,
    session: Option<String>,
    session_id: Option<String>,
    from_zoom: bool,
) -> Result<(), cua_driver_core::protocol::ToolResult> {
    use cua_driver_core::tool::Tool;
    let mut click_args = serde_json::json!({
        "pid": pid, "x": x, "y": y,
        "delivery_mode": "background",
        "action": "focus",
    });
    if let Some(wid) = window_id {
        click_args["window_id"] = serde_json::json!(wid);
    }
    if let Some(ref s) = session {
        click_args["session"] = serde_json::json!(s);
    }
    if let Some(ref s) = session_id {
        click_args["_session_id"] = serde_json::json!(s);
    }
    if from_zoom {
        click_args["from_zoom"] = serde_json::json!(true);
    }
    let focus = click::ClickTool::new(state.clone())
        .invoke(click_args)
        .await;
    if focus.is_error != Some(true) {
        // AXFocused is non-destructive: unlike a second real click, it keeps a
        // Cmd+A selection intact before a follow-up type_text or Cmd+V.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        return Ok(());
    }

    if !foreground {
        return Err(cua_driver_core::protocol::ToolResult::error(format!(
            "focus pixel-click at ({x:.0},{y:.0}) failed."
        )));
    }

    // Some renderer surfaces do not expose a usable AX focus action. The
    // explicit foreground rung retains its real-click fallback for them.
    let mut click_args = serde_json::json!({
        "pid": pid, "x": x, "y": y,
        "delivery_mode": "foreground",
        "action": "press",
    });
    if let Some(wid) = window_id {
        click_args["window_id"] = serde_json::json!(wid);
    }
    if let Some(ref s) = session {
        click_args["session"] = serde_json::json!(s);
    }
    if let Some(ref s) = session_id {
        click_args["_session_id"] = serde_json::json!(s);
    }
    if from_zoom {
        click_args["from_zoom"] = serde_json::json!(true);
    }
    let focus = click::ClickTool::new(state.clone())
        .invoke(click_args)
        .await;
    if focus.is_error == Some(true) {
        return Err(cua_driver_core::protocol::ToolResult::error(format!(
            "focus pixel-click at ({x:.0},{y:.0}) failed."
        )));
    }
    // Brief settle so the renderer registers focus before the keystrokes.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    Ok(())
}

/// Thread-safe per-pid zoom context registry.
pub struct ZoomRegistry {
    inner: std::sync::Mutex<HashMap<i32, ZoomContext>>,
}

impl Default for ZoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ZoomRegistry {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

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

impl Default for ResizeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ResizeRegistry {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

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
    /// Max screenshot dimension (0 = no limit). Applied during screenshot/zoom.
    /// Default 1568 matches Swift's `CuaDriverConfig.defaultMaxImageDimension` —
    /// the long edge is downscaled to this before encoding.
    pub max_image_dimension: u32,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
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
        Err(_) => return cfg, // no file yet — use defaults
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return cfg, // malformed file — use defaults
    };
    // `capture_mode` is per-call now; old on-disk `capture_mode` and
    // `capture_scope` keys are intentionally inert.
    if let Some(v) = json.get("max_image_dimension").and_then(|v| v.as_u64()) {
        if let Ok(v32) = u32::try_from(v) {
            cfg.max_image_dimension = v32;
        }
    }
    cfg
}

/// Convert native pixels from `get_desktop_state` into the logical point space
/// used by CoreGraphics input APIs. Retina scaled modes cannot rely on the
/// nominal backing factor alone, so derive the ratio from the actual PNG.
pub async fn desktop_screenshot_point(x: f64, y: f64) -> (f64, f64) {
    let ratio = tokio::task::spawn_blocking(|| {
        let logical_w = get_screen_size::main_screen_size().map(|(w, _, _)| w as f64);
        let shot_w = crate::capture::screenshot_display_bytes()
            .ok()
            .and_then(|png| crate::capture::png_dimensions(&png).ok())
            .map(|(w, _)| w as f64);
        match (shot_w, logical_w) {
            (Some(sw), Some(lw)) if lw > 0.0 && sw > lw => sw / lw,
            _ => 1.0,
        }
    })
    .await
    .unwrap_or(1.0);
    (x / ratio, y / ratio)
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
    pub max_image_dimension: Option<u32>,
}

/// Thread-safe map of `session_id` → `ConfigOverrides`, mirroring
/// `CursorRegistry`'s registry shape. Cleared per session on `session_end`.
pub struct SessionConfigRegistry {
    inner: std::sync::Mutex<HashMap<String, ConfigOverrides>>,
}

impl SessionConfigRegistry {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

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
        if delta.max_image_dimension.is_some() {
            entry.max_image_dimension = delta.max_image_dimension;
        }
    }

    /// Resolve the effective `max_image_dimension` for `session`, layering its
    /// override over the global `DriverConfig`. `session = None` (anonymous)
    /// returns the global value verbatim.
    pub fn effective_max_image_dimension(
        &self,
        session: Option<&str>,
        global: &DriverConfig,
    ) -> u32 {
        let ov = session.and_then(|s| self.inner.lock().unwrap().get(s).cloned());
        match ov {
            Some(ov) => ov.max_image_dimension.unwrap_or(global.max_image_dimension),
            None => global.max_image_dimension,
        }
    }

    /// Drop `session`'s overrides. No-op for an unknown id (so `session_end`
    /// for an anonymous / never-set session is harmless).
    pub fn clear(&self, session: &str) {
        self.inner.lock().unwrap().remove(session);
    }
}

impl Default for SessionConfigRegistry {
    fn default() -> Self {
        Self::new()
    }
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
    /// Open CDP connections, one per port, reused across `insert_text` /
    /// `type_keystrokes` calls instead of reconnecting fresh every time —
    /// see `CdpSessionCache` for why (Chrome's "allow remote debugging"
    /// popup fires on every new connection, not once per session).
    pub cdp_sessions: Arc<crate::browser::CdpSessionCache>,
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
            cdp_sessions: Arc::new(crate::browser::CdpSessionCache::new()),
        }
    }
}

/// Register all macOS tools into the registry. `compat=true` swaps the
/// regular `screenshot` tool for the Claude Code computer-use compat
/// variant — same name, stricter args, window-scoped JPEG @ 85% + a text
/// note telling the caller to use pixel-addressed tools.
pub fn register_all(registry: &mut ToolRegistry, compat: bool) {
    let state = Arc::new(ToolState::default());
    {
        let cursor_registry = state.cursor_registry.clone();
        let _ = cua_driver_core::session::set_cursor_outcome_reader(std::sync::Arc::new(
            move |session_id| {
                let state = cursor_registry.get(session_id);
                let motion_customized = state.is_some()
                    && crate::cursor::overlay::current_motion(session_id)
                        != cursor_overlay::MotionConfig::default();
                let active_cursor_count = cursor_registry
                    .all_states()
                    .iter()
                    .filter(|state| state.config.cursor_id != "default")
                    .count()
                    .max(1);
                match state {
                    Some(state) => cua_driver_core::session::bounded_cursor_outcome(
                        true,
                        state.config.enabled,
                        state.config.cursor_icon.as_deref(),
                        state.config.cursor_color.as_deref(),
                        state.config.cursor_label.as_deref(),
                        motion_customized,
                        active_cursor_count,
                    ),
                    None => cua_driver_core::session::bounded_cursor_outcome(
                        false,
                        false,
                        None,
                        None,
                        None,
                        false,
                        active_cursor_count,
                    ),
                }
            },
        ));
    }
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
    registry.register(Box::new(get_window_state::GetWindowStateTool::new(
        state.clone(),
    )));
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
    // The standalone `screenshot` tool was removed (#1692). The pixel-grounding
    // screenshot the Claude Code computer-use compat loop relies on now comes
    // from `get_window_state` (which always returns BOTH the tree AND a
    // screenshot — perception is mode-agnostic; `capture_mode` is deprecated/
    // ignored) for a window, or `get_desktop_state` for the whole screen.
    // `compat` no longer gates a tool swap here — the flag's live purpose is to
    // register the MCP server under the `cua-computer-use` name, which is what
    // triggers Claude Code's computer-use beta-tool injection (see cli.rs).
    let _ = compat;
    registry.register(Box::new(get_screen_size::GetScreenSizeTool));
    registry.register(Box::new(get_desktop_state::GetDesktopStateTool));
    registry.register(Box::new(get_cursor_position::GetCursorPositionTool));
    registry.register(Box::new(move_cursor::MoveCursorTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::SetAgentCursorEnabledTool::new(
        state.clone(),
    )));
    registry.register(Box::new(cursor_tools::SetAgentCursorMotionTool::new(
        state.clone(),
    )));
    registry.register(Box::new(cursor_tools::SetAgentCursorStyleTool::new(
        state.clone(),
    )));
    registry.register(Box::new(cursor_tools::GetAgentCursorStateTool::new(
        state.clone(),
    )));
    registry.register(Box::new(check_permissions::CheckPermissionsTool));
    // `health_report` — single-call end-to-end diagnostics. Stable
    // schema_version="1" contract aimed at downstream consumers who must
    // not have to know cua-driver internals. Provider is platform-specific; tool plumbing is in
    // `cua_driver_core::health_report`.
    registry.register(Box::new(
        cua_driver_core::health_report::HealthReportTool::new(Arc::new(
            health_report::MacosHealthProvider,
        )),
    ));
    registry.register(Box::new(get_config::GetConfigTool::new(state.clone())));
    registry.register(Box::new(set_config::SetConfigTool::new(state.clone())));
    registry.register(Box::new(
        get_accessibility_tree::GetAccessibilityTreeTool::new(state.clone()),
    ));
    registry.register(Box::new(zoom::ZoomTool {
        state: state.clone(),
    }));
    // `type_text_chars` is intentionally NOT registered — Swift treats it as
    // a deprecated alias for `type_text` resolved at invoke time in
    // mcp-server's `ToolRegistry::invoke`. Keeping it out of the registry
    // means it doesn't show up in `tools/list` either, matching Swift's
    // ToolRegistry.swift (`type_text_chars` not in `handlers`) and the
    // platform-windows::build_registry which uses the same convention.
    // Touch the struct so it stays in this crate for the alias resolver.
    let _: &type_text_chars::TypeTextCharsTool =
        &type_text_chars::TypeTextCharsTool::new(state.clone());
    // Cross-platform `page` tool definition lives in mcp-server; macOS plugs in
    // its Apple-Events / CDP / AX-tree backend here.
    registry.register(Box::new(cua_driver_core::page::PageTool::new(Arc::new(
        page::MacOsPageBackend::new(state.clone()),
    ))));
    let browser_engine = cua_driver_core::browser::BrowserEngine::new(Arc::new(
        crate::browser::MacOsBrowserPlatform,
    ));
    cua_driver_core::browser::register_browser_tools(&browser_engine, registry);
    // Recording / replay + session-lifecycle tools are platform-independent.
    registry.register_recording_tools();
    registry.register_session_tools();
}

#[cfg(test)]
mod session_config_guard_tests {
    use super::*;
    use cua_driver_core::session::fire_session_end;

    fn overrides(max_dim: u32) -> ConfigOverrides {
        ConfigOverrides {
            max_image_dimension: Some(max_dim),
        }
    }

    #[test]
    fn ended_session_config_set_is_noop() {
        // THE FIX (config side): an ended session id keys the overrides map, so
        // an in-flight set_config after session_end must not re-create the entry
        // the reaper's clear hook removed. effective then falls back to global.
        let reg = SessionConfigRegistry::new();
        let global = DriverConfig::default();
        let sid = "wb-config-ended-Q9R8S7";
        fire_session_end(sid);
        assert!(cua_driver_core::session::is_session_ended(sid));

        reg.set(sid, overrides(800));
        let dim = reg.effective_max_image_dimension(Some(sid), &global);
        assert_eq!(
            dim, global.max_image_dimension,
            "ended session must not get an override entry"
        );
    }

    #[test]
    fn live_session_config_set_takes_effect() {
        let reg = SessionConfigRegistry::new();
        let global = DriverConfig::default();
        let sid = "wb-config-live-T1U2V3";
        assert!(!cua_driver_core::session::is_session_ended(sid));
        reg.set(sid, overrides(800));
        let dim = reg.effective_max_image_dimension(Some(sid), &global);
        assert_eq!(dim, 800, "live session override must apply");
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
        assert!(
            !rec.current_state().enabled,
            "no recording may start for a dead session"
        );
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
