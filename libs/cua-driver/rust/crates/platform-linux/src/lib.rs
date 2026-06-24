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

use cua_driver_core::tool::ToolRegistry;

pub mod tools;
pub mod overlay;
pub mod pip;
pub mod health_report;

#[cfg(target_os = "linux")]
pub mod x11;

#[cfg(target_os = "linux")]
pub mod input;

#[cfg(target_os = "linux")]
pub mod tty;

#[cfg(target_os = "linux")]
pub mod proc_fs;

#[cfg(target_os = "linux")]
pub mod installed_apps;

#[cfg(target_os = "linux")]
pub mod capture;

#[cfg(target_os = "linux")]
pub mod atspi;

#[cfg(target_os = "linux")]
pub mod a11y;

#[cfg(target_os = "linux")]
pub mod wayland;

// `terminal` is OS-independent (pure string matching + a thin x11 hook).
// Keeping it un-gated lets the unit tests run on any host.
pub mod terminal;

#[cfg(target_os = "linux")]
pub mod xauth;

/// Pure WSL-detection predicate, split out from [`is_wsl`] so both branches
/// are unit-testable without the process-global env / `OnceLock` cache.
/// `wsl_env` mirrors `$WSL_DISTRO_NAME`/`$WSL_INTEROP` being set; `osrelease`
/// is the `/proc/sys/kernel/osrelease` contents.
fn detect_wsl(wsl_env_set: bool, osrelease: Option<&str>) -> bool {
    if wsl_env_set {
        return true;
    }
    osrelease
        .map(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("microsoft") || s.contains("wsl")
        })
        .unwrap_or(false)
}

/// True when this process is running under WSL (Windows Subsystem for Linux).
/// WSL leaks its identity through the kernel release string ("microsoft" /
/// "WSL") and `/proc/sys/kernel/osrelease`. Cheap, side-effect-free, cached
/// after the first probe.
pub fn is_wsl() -> bool {
    use std::sync::OnceLock;
    static IS_WSL: OnceLock<bool> = OnceLock::new();
    *IS_WSL.get_or_init(|| {
        let env_set = std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::env::var_os("WSL_INTEROP").is_some();
        let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();
        detect_wsl(env_set, osrelease.as_deref())
    })
}

/// Actionable hint appended to "no usable display" errors. Tailored for WSL,
/// where the common failure is "no real X/Wayland display, or WSLg geometry
/// isn't resolvable". Kept as a trailing fragment so callers can splice it
/// onto a more specific message. See issue #2005.
pub fn no_display_hint() -> String {
    let neither_display_set =
        std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none();
    if is_wsl() {
        " — no usable display detected (WSL). WSLg provides $DISPLAY/$WAYLAND_DISPLAY \
         under a recent Windows 11 + 'wsl --update'; on older setups install an X server \
         (e.g. VcXsrv) and export DISPLAY. cua-driver needs a real display to capture and \
         report screen size."
            .to_owned()
    } else if neither_display_set {
        " — no usable display detected: neither $DISPLAY (X11) nor $WAYLAND_DISPLAY is set. \
         Start an X server / Wayland compositor (or Xvfb for headless) before running cua-driver."
            .to_owned()
    } else {
        String::new()
    }
}

pub fn register_tools() -> ToolRegistry {
    #[cfg(target_os = "linux")]
    wayland::ensure_nested_session();
    tools::build_registry(false)
}

/// `compat=true` enables Claude Code computer-use compatibility mode:
/// the regular `screenshot` tool is replaced by a window-scoped variant
/// (pid + window_id required, JPEG @ 85%, text note pointing at pixel
/// tools). See `tools::impl_::ScreenshotCompatTool`.
pub fn register_tools_with_cursor(cfg: cursor_overlay::CursorConfig, compat: bool) -> ToolRegistry {
    #[cfg(target_os = "linux")]
    wayland::ensure_nested_session();
    if cfg.enabled {
        overlay::init(cfg.clone());
        overlay::run_on_thread();
    }
    tools::build_registry(compat)
}

#[cfg(test)]
mod display_detection_tests {
    use super::detect_wsl;

    #[test]
    fn wsl_env_var_short_circuits() {
        // $WSL_DISTRO_NAME / $WSL_INTEROP present → WSL regardless of osrelease.
        assert!(detect_wsl(true, None));
        assert!(detect_wsl(true, Some("6.1.0-generic")));
    }

    #[test]
    fn wsl2_osrelease_microsoft_tag_detected() {
        // Real WSL2 kernel release string (verified live on a WSL2 box).
        assert!(detect_wsl(false, Some("6.18.33.1-microsoft-standard-WSL2")));
    }

    #[test]
    fn wsl1_osrelease_microsoft_tag_detected() {
        // WSL1 kernels also carry the Microsoft tag, mixed-case.
        assert!(detect_wsl(false, Some("4.4.0-19041-Microsoft")));
    }

    #[test]
    fn plain_linux_is_not_wsl() {
        assert!(!detect_wsl(false, Some("6.8.0-39-generic")));
        assert!(!detect_wsl(false, None));
    }
}
