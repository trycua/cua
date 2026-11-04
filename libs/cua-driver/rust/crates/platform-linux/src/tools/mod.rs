//! Linux tool implementations.
//!
//! On Linux: delegates to real x11/atspi/input/capture implementations.
//! On other platforms: returns "not implemented" stubs so the crate compiles.

use cua_driver_core::tool::ToolRegistry;

#[cfg(target_os = "linux")]
mod bring_to_front;
#[cfg(target_os = "linux")]
mod check_permissions;
#[cfg(target_os = "linux")]
mod click;
#[cfg(target_os = "linux")]
mod cursor_tools;
#[cfg(target_os = "linux")]
mod double_click;
#[cfg(target_os = "linux")]
mod drag;
#[cfg(target_os = "linux")]
mod get_accessibility_tree;
#[cfg(target_os = "linux")]
mod get_config;
#[cfg(target_os = "linux")]
mod get_cursor_position;
#[cfg(target_os = "linux")]
mod get_desktop_state;
#[cfg(target_os = "linux")]
mod get_screen_size;
#[cfg(target_os = "linux")]
mod get_window_state;
#[cfg(target_os = "linux")]
mod hotkey;
#[cfg(target_os = "linux")]
mod impl_;
#[cfg(target_os = "linux")]
mod invoke_menu;
#[cfg(target_os = "linux")]
mod kill_app;
#[cfg(target_os = "linux")]
mod launch_app;
#[cfg(target_os = "linux")]
mod list_apps;
#[cfg(target_os = "linux")]
mod list_windows;
#[cfg(target_os = "linux")]
mod mouse_button_down;
#[cfg(target_os = "linux")]
mod mouse_button_up;
#[cfg(target_os = "linux")]
mod mouse_drag;
#[cfg(target_os = "linux")]
mod move_cursor;
#[cfg(target_os = "linux")]
pub(crate) mod page;
#[cfg(target_os = "linux")]
mod parallel_mouse_drag;
#[cfg(target_os = "linux")]
mod press_key;
#[cfg(target_os = "linux")]
mod right_click;
#[cfg(target_os = "linux")]
mod scroll;
#[cfg(target_os = "linux")]
mod set_config;
#[cfg(target_os = "linux")]
mod set_value;
#[cfg(target_os = "linux")]
mod set_window_frame;
#[cfg(target_os = "linux")]
mod type_text;
#[cfg(target_os = "linux")]
mod type_text_chars;
#[cfg(target_os = "linux")]
mod zoom;

#[cfg(not(target_os = "linux"))]
mod stubs;

pub fn build_registry(compat: bool) -> ToolRegistry {
    build_registry_with_provider(compat, None)
}

pub fn build_registry_with_provider(
    compat: bool,
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
) -> ToolRegistry {
    #[cfg(target_os = "linux")]
    #[allow(clippy::needless_return)] // a cfg-gated early return, not a tail
    {
        // Visibly targeted pointer actions push cursor-hook events (see
        // `impl_::emit_cursor_hook`), so an embedder such as cua-spacesd can
        // publish the agent's cursor as presence without polling.
        cua_driver_core::cursor_hook::declare_cursor_hook_emitter();
        return impl_::build_registry_with_provider(compat, provider);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = compat;
        let _ = provider;
        stubs::build_registry()
    }
}

// Keep register_all as alias for backwards compat.
pub fn register_all() -> ToolRegistry {
    build_registry(false)
}
