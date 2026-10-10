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
//! Derived in part from Interface-Agent (MIT) and trope-cua (MIT); see
//! THIRD_PARTY_NOTICES.md.

// Off Windows the crate still builds its stubs (so workspace-wide checks
// compile on every host) but the Win32 code is cfg'd out, which strands the
// shared helpers and parameters it uses. They are live on Windows, where these
// lints stay enforced.
#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_variables))]
// Tool helpers return `Result<_, ToolResult>`: the error arm is the finished
// tool reply (see the note in cua-driver-core), not a propagated error.
#![allow(clippy::result_large_err)]

use cua_driver_core::tool::ToolRegistry;

pub mod diagnostics;
pub mod health_report;
pub mod overlay;
pub mod pip;
pub mod recording_hooks;
pub mod terminal;
pub mod tools;

// Cross-platform: pure math for MOUSEEVENTF_VIRTUALDESK absolute-coord
// normalization. Lives outside the Windows-only `input` module so its
// unit tests run on any host (no Win32 runtime needed) — see issue #1979,
// where the multi-monitor normalization bug was diagnosable by pure math.
pub mod virtualdesk;

#[cfg(any(target_os = "windows", test))]
mod keycodes;

// Cross-platform: pure math for packing `(x, y)` into the `LPARAM` payload
// every `WM_MOUSE*` / `WM_*BUTTON*` message carries. Lives outside the
// Windows-only `input` module for the same reason as `virtualdesk` — the
// receiver-side `GET_X_LPARAM` sign-extension contract is a pure-math
// property we want to pin with cross-platform unit tests (the #1979 audit
// pass that lives in this commit).
pub mod lparam;

/// UIA / `WM_NCHITTEST` / `IDC_*` -> cursor shape tables for presence (pure,
/// every host).
pub mod pointer_shape_map;

#[cfg(target_os = "windows")]
pub mod pointer_shape;

/// Install the Windows presence pointer-shape backend (UIA hit-test,
/// `GetCursorInfo` readout, `SetCursorPos` warp) into
/// `cua_driver_core::pointer_shape`. Returns false off Windows or when a
/// backend was already installed.
pub fn install_pointer_shape_backend() -> bool {
    #[cfg(target_os = "windows")]
    {
        pointer_shape::install()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

#[cfg(target_os = "windows")]
pub mod win32;

#[cfg(target_os = "windows")]
pub mod history;

#[cfg(target_os = "windows")]
pub mod browser_platform;

// Pure isolated-browser selection decision; cfg-independent so its unit
// tests run on any host.
#[cfg(any(target_os = "windows", test))]
mod browser_isolated_selection;

// Pure path normalization and reparse-point resolution for the isolated
// browser installation check; cfg-independent so its unit tests run on any
// host.
#[cfg(any(target_os = "windows", test))]
mod browser_installation_path;

// Pure launch-token decision and command-line quoting for isolated browsers;
// cfg-independent so its unit tests run on any host.
#[cfg(any(target_os = "windows", test))]
mod browser_launch_token;

#[cfg(target_os = "windows")]
mod browser_standard_user;

// De-elevated `launch_app` for an elevated Driver (#3607).
#[cfg(target_os = "windows")]
pub mod standard_user_launch;

#[cfg(target_os = "windows")]
mod browser_consent_ui;
#[cfg(target_os = "windows")]
mod browser_setup_ui;

#[cfg(target_os = "windows")]
pub mod uia;

#[cfg(target_os = "windows")]
pub mod msaa;

#[cfg(target_os = "windows")]
pub mod input;

#[cfg(target_os = "windows")]
pub mod capture;
mod capture_admission;
#[cfg(target_os = "windows")]
mod clipboard;

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
    register_tools_with_cursor_and_provider(None, cfg, compat)
}

pub fn register_tools_with_cursor_and_provider(
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
    cfg: cursor_overlay::CursorConfig,
    compat: bool,
) -> ToolRegistry {
    if cfg.enabled {
        overlay::init(cfg.clone());
        overlay::run_on_thread();
    }
    tools::build_registry_with_provider(compat, provider)
}
