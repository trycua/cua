//! MCP tool implementations for macOS.

mod list_apps;
mod list_windows;
mod get_window_state;
mod launch_app;
mod click;
mod double_click;
mod right_click;
mod drag;
mod type_text;
mod press_key;
mod hotkey;
mod set_value;
mod scroll;
mod screenshot;
mod get_screen_size;
mod get_cursor_position;
mod move_cursor;
mod cursor_tools;
mod check_permissions;
mod get_config;
mod set_config;
mod get_accessibility_tree;
mod zoom;
mod type_text_chars;
mod page;

use mcp_server::tool::ToolRegistry;
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
    /// Max screenshot dimension (0 = no limit). Applied during screenshot/zoom.
    pub max_image_dimension: u32,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            capture_mode: "som".to_owned(),
            max_image_dimension: 0,
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

/// Shared state passed to all tools.
pub struct ToolState {
    pub element_cache: Arc<ElementCache>,
    pub cursor_registry: Arc<CursorRegistry>,
    pub zoom_registry: Arc<ZoomRegistry>,
    pub resize_registry: Arc<ResizeRegistry>,
    pub config: Arc<std::sync::RwLock<DriverConfig>>,
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
        }
    }
}

/// Register all macOS tools into the registry.
pub fn register_all(registry: &mut ToolRegistry) {
    let state = Arc::new(ToolState::default());

    registry.register(Box::new(list_apps::ListAppsTool));
    registry.register(Box::new(list_windows::ListWindowsTool));
    registry.register(Box::new(get_window_state::GetWindowStateTool::new(state.clone())));
    registry.register(Box::new(launch_app::LaunchAppTool));
    registry.register(Box::new(click::ClickTool::new(state.clone())));
    registry.register(Box::new(double_click::DoubleClickTool::new(state.clone())));
    registry.register(Box::new(right_click::RightClickTool::new(state.clone())));
    registry.register(Box::new(drag::DragTool::new(state.clone())));
    registry.register(Box::new(type_text::TypeTextTool::new(state.clone())));
    registry.register(Box::new(press_key::PressKeyTool::new(state.clone())));
    registry.register(Box::new(hotkey::HotkeyTool::new(state.clone())));
    registry.register(Box::new(set_value::SetValueTool::new(state.clone())));
    registry.register(Box::new(scroll::ScrollTool::new(state.clone())));
    registry.register(Box::new(screenshot::ScreenshotTool { state: state.clone() }));
    registry.register(Box::new(get_screen_size::GetScreenSizeTool));
    registry.register(Box::new(get_cursor_position::GetCursorPositionTool));
    registry.register(Box::new(move_cursor::MoveCursorTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::SetAgentCursorEnabledTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::SetAgentCursorMotionTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::SetAgentCursorStyleTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::GetAgentCursorStateTool::new(state.clone())));
    registry.register(Box::new(check_permissions::CheckPermissionsTool));
    registry.register(Box::new(get_config::GetConfigTool::new(state.clone())));
    registry.register(Box::new(set_config::SetConfigTool::new(state.clone())));
    registry.register(Box::new(get_accessibility_tree::GetAccessibilityTreeTool::new(state.clone())));
    registry.register(Box::new(zoom::ZoomTool { state: state.clone() }));
    registry.register(Box::new(type_text_chars::TypeTextCharsTool::new(state.clone())));
    registry.register(Box::new(page::PageTool::new(state.clone())));
    // Recording / replay tools are platform-independent — live in mcp-server.
    registry.register_recording_tools();
}
