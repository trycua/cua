//! Windows platform backend for cua-driver-rs.
//!
//! Background automation on Windows via:
//! - UI Automation (UIA / MSAA) for accessibility tree walking
//! - PostMessage(WM_LBUTTONDOWN/WM_LBUTTONUP) for background mouse injection
//! - PostMessage(WM_CHAR / WM_KEYDOWN/UP) for keyboard events
//! - Win32 EnumWindows / CreateToolhelp32Snapshot for enumeration
//! - PrintWindow / GDI BitBlt for screenshots
//!
//! Reference: /tmp/trope-cua/src/CuaDriver.Win/

use mcp_server::tool::ToolRegistry;

pub mod tools;
pub mod overlay;

#[cfg(target_os = "windows")]
pub mod win32;

#[cfg(target_os = "windows")]
pub mod uia;

#[cfg(target_os = "windows")]
pub mod input;

#[cfg(target_os = "windows")]
pub mod capture;

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
