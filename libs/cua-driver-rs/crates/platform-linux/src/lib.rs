//! Linux platform backend for cua-driver-rs.
//!
//! Background automation on Linux via:
//! - /proc filesystem for process enumeration
//! - X11 _NET_CLIENT_LIST for window enumeration
//! - X11 XSendEvent for background mouse/keyboard injection (no focus steal)
//! - AT-SPI D-Bus protocol for accessibility tree (via subprocess bridge)
//! - xwd/scrot/import for window screenshots
//!
//! Wayland: falls back to xdg-output and virtual-keyboard protocols when
//! running under a Wayland compositor with XWayland support.

use mcp_server::tool::ToolRegistry;

pub mod tools;
pub mod overlay;

#[cfg(target_os = "linux")]
pub mod x11;

#[cfg(target_os = "linux")]
pub mod input;

#[cfg(target_os = "linux")]
pub mod proc_fs;

#[cfg(target_os = "linux")]
pub mod capture;

#[cfg(target_os = "linux")]
pub mod atspi;

pub fn register_tools() -> ToolRegistry {
    tools::build_registry()
}

pub fn register_tools_with_cursor(cfg: cursor_overlay::CursorConfig) -> ToolRegistry {
    if cfg.enabled {
        overlay::init(cfg.clone());
        overlay::run_on_thread();
    }
    tools::build_registry()
}
