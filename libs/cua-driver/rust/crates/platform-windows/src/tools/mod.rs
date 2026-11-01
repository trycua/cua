//! Windows tool implementations.
//!
//! On Windows, delegates to real win32/uia/input/capture implementations.
//! On other platforms, returns "not implemented" stubs so the crate still compiles.

use cua_driver_core::tool::ToolRegistry;

#[cfg(target_os = "windows")]
mod bring_to_front;
#[cfg(target_os = "windows")]
mod check_permissions;
#[cfg(target_os = "windows")]
mod click;
#[cfg(target_os = "windows")]
mod cursor_tools;
#[cfg(target_os = "windows")]
mod debug_window_info;
#[cfg(target_os = "windows")]
mod double_click;
#[cfg(target_os = "windows")]
mod drag;
#[cfg(target_os = "windows")]
mod get_accessibility_tree;
#[cfg(target_os = "windows")]
mod get_config;
#[cfg(target_os = "windows")]
mod get_cursor_position;
#[cfg(target_os = "windows")]
mod get_desktop_state;
#[cfg(target_os = "windows")]
mod get_screen_size;
#[cfg(target_os = "windows")]
mod get_window_state;
#[cfg(target_os = "windows")]
mod hotkey;
#[cfg(target_os = "windows")]
mod impl_;
#[cfg(target_os = "windows")]
mod invoke_menu;
#[cfg(target_os = "windows")]
mod kill_app;
#[cfg(target_os = "windows")]
mod launch_app;
#[cfg(target_os = "windows")]
mod list_apps;
#[cfg(target_os = "windows")]
mod list_windows;
#[cfg(target_os = "windows")]
mod move_cursor;
#[cfg(target_os = "windows")]
pub(crate) mod page;
#[cfg(target_os = "windows")]
pub(crate) mod page_bookmark;
// Pure title parsing for `page_bookmark`; ungated so its tests run on any host.
pub(crate) mod page_title_marker;
#[cfg(target_os = "windows")]
mod press_key;
#[cfg(target_os = "windows")]
mod right_click;
#[cfg(target_os = "windows")]
mod scroll;
mod scroll_coordinates;
#[cfg(target_os = "windows")]
mod set_config;
#[cfg(target_os = "windows")]
mod set_value;
#[cfg(target_os = "windows")]
mod set_window_frame;
#[cfg(target_os = "windows")]
mod type_text;
#[cfg(target_os = "windows")]
mod type_text_chars;
#[cfg(target_os = "windows")]
mod zoom;

#[cfg(not(target_os = "windows"))]
mod stubs;

pub fn build_registry(compat: bool) -> ToolRegistry {
    build_registry_with_provider(compat, None)
}

pub fn build_registry_with_provider(
    compat: bool,
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
) -> ToolRegistry {
    #[cfg(target_os = "windows")]
    return impl_::build_registry_with_provider(compat, provider);

    #[cfg(not(target_os = "windows"))]
    {
        let _ = compat;
        let _ = provider;
        stubs::build_registry()
    }
}
