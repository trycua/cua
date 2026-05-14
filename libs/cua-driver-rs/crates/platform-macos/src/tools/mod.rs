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
            config: Arc::new(std::sync::RwLock::new(DriverConfig::default())),
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
