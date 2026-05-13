//! macOS platform backend for cua-driver-rs.
//!
//! Provides background automation on macOS via:
//! - Accessibility (AX) API for UI tree walking and element interaction
//! - CGEvent / SkyLight SPI for background mouse and keyboard injection
//! - NSRunningApplication / NSWorkspace for app enumeration and lifecycle
//! - CGWindow / ScreenCaptureKit for window enumeration and screenshots

#[cfg(target_os = "macos")]
pub mod ax;
#[cfg(target_os = "macos")]
pub mod apps;
#[cfg(target_os = "macos")]
pub mod windows;
#[cfg(target_os = "macos")]
pub mod input;
#[cfg(target_os = "macos")]
pub mod cursor;
#[cfg(target_os = "macos")]
pub mod capture;
#[cfg(target_os = "macos")]
pub mod browser;
#[cfg(target_os = "macos")]
pub mod tools;

use mcp_server::tool::ToolRegistry;

/// Register all macOS tools.  For programs that don't restructure `main`
/// (e.g. test harnesses), the overlay is skipped.
pub fn register_tools() -> ToolRegistry {
    #[cfg(target_os = "macos")]
    {
        let mut r = ToolRegistry::new();
        tools::register_all(&mut r);
        r
    }
    #[cfg(not(target_os = "macos"))]
    ToolRegistry::new()
}

/// Register all macOS tools and initialise the cursor overlay channel.
///
/// After calling this, `main()` must call
/// `platform_macos::cursor::overlay::run_on_main_thread()` on the OS
/// main thread to actually display the overlay.
pub fn register_tools_with_cursor(cfg: cursor_overlay::CursorConfig) -> ToolRegistry {
    #[cfg(target_os = "macos")]
    {
        if cfg.enabled {
            cursor::overlay::init(cfg);
        }
        let mut r = ToolRegistry::new();
        tools::register_all(&mut r);
        r
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cfg;
        ToolRegistry::new()
    }
}
