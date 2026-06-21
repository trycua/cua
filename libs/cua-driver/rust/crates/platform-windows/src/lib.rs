//! Windows platform backend for cua-driver-rs.
//!
//! Background automation on Windows via:
//! - UI Automation (UIA / MSAA) for accessibility tree walking
//! - PostMessage(WM_LBUTTONDOWN/WM_LBUTTONUP) for background mouse injection
//! - PostMessage(WM_CHAR / WM_KEYDOWN/UP) for keyboard events
//! - Win32 EnumWindows / CreateToolhelp32Snapshot for enumeration
//! - PrintWindow / GDI BitBlt for screenshots
//!
//! ## Provenance
//!
//! The Windows accessibility-tree walk, app/window enumeration, UIA
//! InvokePattern click, and ValuePattern set-value all derive from prior art in
//! Interface-Agent (github.com/francedot/Interface-Agent, packages/windows, 2024).
//! trope-cua (MIT, github.com/voctory/trope-cua) was a useful cross-reference for
//! some of the details during this Rust implementation — thanks to Victor Vannara
//! for that work.

use cua_driver_core::tool::ToolRegistry;

pub mod tools;
pub mod overlay;
pub mod diagnostics;
pub mod recording_hooks;
pub mod pip;
pub mod terminal;

#[cfg(target_os = "windows")]
pub mod win32;

#[cfg(target_os = "windows")]
pub mod uia;

#[cfg(target_os = "windows")]
pub mod msaa;

#[cfg(target_os = "windows")]
pub mod input;

#[cfg(target_os = "windows")]
pub mod capture;

#[cfg(target_os = "windows")]
pub mod wgc;

#[cfg(target_os = "windows")]
pub mod launch_uwp;

pub fn register_tools() -> ToolRegistry {
    tools::build_registry(false)
}

/// `compat=true` enables Claude Code computer-use compatibility mode:
/// the regular `screenshot` tool is replaced by a window-scoped variant
/// (pid + window_id required, JPEG @ 85%, text note pointing at pixel
/// tools). See `tools::impl_::ScreenshotCompatTool`.
pub fn register_tools_with_cursor(cfg: cursor_overlay::CursorConfig, compat: bool) -> ToolRegistry {
    if cfg.enabled {
        overlay::init(cfg.clone());
        overlay::run_on_thread();
    }
    tools::build_registry(compat)
}
