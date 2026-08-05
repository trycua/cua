//! macOS platform backend for cua-driver-rs.
//!
//! Provides background automation on macOS via:
//! - Accessibility (AX) API for UI tree walking and element interaction
//! - CGEvent / SkyLight SPI for background mouse and keyboard injection
//! - NSRunningApplication / NSWorkspace for app enumeration and lifecycle
//! - CGWindow / ScreenCaptureKit for window enumeration and screenshots

#[cfg(target_os = "macos")]
pub mod apps;
#[cfg(target_os = "macos")]
pub mod ax;
#[cfg(target_os = "macos")]
mod background_mutation;
#[cfg(target_os = "macos")]
pub mod browser;
#[cfg(target_os = "macos")]
pub mod capture;
#[cfg(target_os = "macos")]
pub mod cursor;
#[cfg(target_os = "macos")]
pub mod focus_guard;
#[cfg(target_os = "macos")]
pub mod focus_steal;
#[cfg(target_os = "macos")]
pub mod input;
#[cfg(target_os = "macos")]
pub mod permissions;
#[cfg(target_os = "macos")]
pub mod pip;
#[cfg(target_os = "macos")]
pub mod recording_hooks;
#[cfg(target_os = "macos")]
pub mod session;
#[cfg(target_os = "macos")]
pub mod terminal;
#[cfg(target_os = "macos")]
pub mod tools;
#[cfg(target_os = "macos")]
pub mod video_sckit;
#[cfg(target_os = "macos")]
pub mod window_change_detector;
#[cfg(target_os = "macos")]
pub mod windows;

use cua_driver_core::tool::ToolRegistry;

/// Register all macOS tools.  For programs that don't restructure `main`
/// (e.g. test harnesses), the overlay is skipped.
pub fn register_tools() -> ToolRegistry {
    register_tools_with_compat(false)
}

/// Same as `register_tools` but lets the caller pick the Claude Code
/// computer-use compat mode. `compat=true` swaps the regular `screenshot`
/// tool for the window-scoped variant (pid + window_id required,
/// JPEG @ 85%, text note pointing at pixel tools).
pub fn register_tools_with_compat(compat: bool) -> ToolRegistry {
    #[cfg(target_os = "macos")]
    {
        let mut r = ToolRegistry::new();
        tools::register_all(&mut r, compat, false, false, None);
        r
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = compat;
        ToolRegistry::new()
    }
}

/// Register all macOS tools and initialise the cursor overlay channel.
///
/// After calling this, `main()` must call
/// `platform_macos::cursor::overlay::run_on_main_thread()` on the OS
/// main thread to actually display the overlay.
///
/// `compat=true` enables Claude Code computer-use compatibility mode:
/// the regular `screenshot` tool is replaced by a window-scoped variant
/// (pid + window_id required, JPEG @ 85%, text note pointing at pixel
/// tools). See `tools::screenshot_compat`.
pub fn register_tools_with_cursor(
    cfg: cursor_overlay::CursorConfig,
    compat: bool,
    host_owns_permission_ux: bool,
    host_bundle_id: Option<String>,
) -> ToolRegistry {
    register_tools_with_cursor_and_provider(
        None,
        cfg,
        compat,
        host_owns_permission_ux,
        host_bundle_id,
    )
}

/// Register all macOS tools with a constructor-installed protected host.
///
/// This is used by the canonical SDK runtime. The original public constructor
/// remains unchanged for callers that do not host protected consent.
pub fn register_tools_with_cursor_and_provider(
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
    cfg: cursor_overlay::CursorConfig,
    compat: bool,
    host_owns_permission_ux: bool,
    host_bundle_id: Option<String>,
) -> ToolRegistry {
    #[cfg(target_os = "macos")]
    {
        let cursor_overlay_available =
            cursor_overlay_facility_available(cfg.enabled, session::has_graphic_access());
        if cursor_overlay_available {
            cursor::overlay::init(cfg);
        }
        let mut r = ToolRegistry::new_with_protected_consent_provider(provider);
        tools::register_all(
            &mut r,
            compat,
            cursor_overlay_available,
            host_owns_permission_ux,
            host_bundle_id,
        );
        r
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cfg;
        let _ = compat;
        let _ = host_owns_permission_ux;
        let _ = host_bundle_id;
        let _ = provider;
        ToolRegistry::new()
    }
}

#[cfg(target_os = "macos")]
fn cursor_overlay_facility_available(enabled: bool, graphic_access: bool) -> bool {
    enabled && graphic_access
}

#[cfg(all(test, target_os = "macos"))]
mod cursor_overlay_host_tests {
    use super::cursor_overlay_facility_available;

    #[test]
    fn overlay_requires_both_host_enablement_and_graphic_session_access() {
        assert!(cursor_overlay_facility_available(true, true));
        assert!(!cursor_overlay_facility_available(false, true));
        assert!(!cursor_overlay_facility_available(true, false));
        assert!(!cursor_overlay_facility_available(false, false));
    }
}
