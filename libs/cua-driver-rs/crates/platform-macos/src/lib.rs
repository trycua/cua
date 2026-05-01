//! macOS platform backend for cua-driver-rs.
//!
//! Provides background automation on macOS via:
//! - Accessibility (AX) API for UI tree walking and element interaction
//! - CGEvent / SkyLight SPI for background mouse and keyboard injection
//! - NSRunningApplication / NSWorkspace for app enumeration and lifecycle
//! - CGWindow / ScreenCaptureKit for window enumeration and screenshots

pub mod ax;
pub mod apps;
pub mod windows;
pub mod input;
pub mod cursor;
pub mod capture;
pub mod tools;

use mcp_server::tool::ToolRegistry;

/// Register all macOS tools.  For programs that don't restructure `main`
/// (e.g. test harnesses), the overlay is skipped.
pub fn register_tools() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    tools::register_all(&mut r);
    r
}

/// Register all macOS tools and initialise the cursor overlay channel.
///
/// After calling this, `main()` must call
/// `platform_macos::cursor::overlay::run_on_main_thread()` on the OS
/// main thread to actually display the overlay.
pub fn register_tools_with_cursor(cfg: cursor_overlay::CursorConfig) -> ToolRegistry {
    if cfg.enabled {
        cursor::overlay::init(cfg);
    }
    let mut r = ToolRegistry::new();
    tools::register_all(&mut r);
    r
}
