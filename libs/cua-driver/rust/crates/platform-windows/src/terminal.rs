//! Terminal-emulator detection for the Windows `type_text` path.
//!
//! Terminal emulators (Windows Terminal, mintty / Git-Bash, Vim, NeoVim,
//! legacy `cmd.exe`) consume keyboard input through console / conpty
//! channels, not through the GUI's WM_CHAR queue. PostMessage(WM_CHAR)
//! against those hosts is silently dropped — the user reports
//! "type was acknowledged but nothing appeared". We detect the target
//! by window class name; in background, type_text to a terminal returns
//! `background_unavailable` (escalate to `delivery_mode:"foreground"`,
//! which delivers via SendInput Unicode) rather than fronting it. Native
//! ConsoleHost on ARM64 is the exception: it can accept those Unicode events
//! without delivering them, so both modes return a hard refusal.
//!
//! Adding a new terminal: append a class-name prefix to
//! [`TERMINAL_CLASS_PREFIXES`] and a coverage entry in the tests.

/// Window class names (or class-name prefixes) of Windows terminal
/// hosts. PostMessage(WM_CHAR) is unreliable against any class on
/// this list; the `type_text` tool falls back to SendInput Unicode
/// key events when the target HWND matches.
///
/// - `CASCADIA_HOSTING_WINDOW_CLASS`: Windows Terminal (wt.exe) /
///   modern conhost.
/// - `mintty`: Git Bash / Cygwin / MSYS2.
/// - `ConsoleWindowClass`: legacy console host (cmd.exe, powershell.exe).
/// - `Vim`, `nvim`: GVim and NeoVim host windows; both consume keys
///   through their own VT input pipeline, so WM_CHAR drops on the
///   floor.
///
/// Match is a `starts_with` against the HWND's class name (case-
/// sensitive — class names are registered case-significantly on
/// Windows).
pub const TERMINAL_CLASS_PREFIXES: &[&str] = &[
    "CASCADIA_HOSTING_WINDOW_CLASS",
    "ConsoleWindowClass",
    "mintty",
    "nvim",
    "Vim",
];

/// Whether `type_text` has a known honest delivery route for a window class.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum TypeTextPolicy {
    Supported,
    ForegroundRequired,
    Unsupported,
}

/// Returns `true` when `class_name` (typically the result of
/// `GetClassNameW`) matches a known terminal-host class via prefix.
pub fn class_matches_terminal(class_name: &str) -> bool {
    if class_name.is_empty() {
        return false;
    }
    TERMINAL_CLASS_PREFIXES
        .iter()
        .any(|p| class_name.starts_with(p))
}

/// Decide the terminal route before invoking either PostMessage or SendInput.
///
/// `ConsoleWindowClass` is native ConsoleHost. On Windows 11 ARM64 its input
/// queue can report every `KEYEVENTF_UNICODE` packet as accepted while the
/// prompt remains unchanged. Since the driver has no independent delivery
/// oracle for that surface, foreground delivery must refuse instead of
/// returning a false success.
pub(crate) fn type_text_policy_for_class(
    class_name: &str,
    foreground: bool,
    windows_arm64: bool,
) -> TypeTextPolicy {
    if windows_arm64 && class_name.starts_with("ConsoleWindowClass") {
        TypeTextPolicy::Unsupported
    } else if class_matches_terminal(class_name) && !foreground {
        TypeTextPolicy::ForegroundRequired
    } else {
        TypeTextPolicy::Supported
    }
}

/// Resolve [`TypeTextPolicy`] for a live HWND.
#[cfg(target_os = "windows")]
pub(crate) fn type_text_policy(hwnd: u64, foreground: bool) -> TypeTextPolicy {
    if hwnd == 0 {
        return TypeTextPolicy::Supported;
    }
    let class = crate::input::delivery::read_class_name(hwnd);
    type_text_policy_for_class(&class, foreground, cfg!(target_arch = "aarch64"))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn type_text_policy(_hwnd: u64, _foreground: bool) -> TypeTextPolicy {
    TypeTextPolicy::Supported
}

/// Hard refusal for native ConsoleHost, whose Unicode SendInput acceptance is
/// not evidence that the prompt received the requested text.
#[cfg(target_os = "windows")]
pub(crate) fn native_consolehost_type_text_error(
    hwnd: u64,
) -> cua_driver_core::protocol::ToolResult {
    let class = crate::input::delivery::read_class_name(hwnd);
    native_consolehost_type_text_error_for_class(class)
}

#[cfg(any(test, target_os = "windows"))]
fn native_consolehost_type_text_error_for_class(
    class: String,
) -> cua_driver_core::protocol::ToolResult {
    let suggestion = "Use a process or PTY execution channel for console setup, \
                      or launch a terminal host with a verifiable text-input route.";
    cua_driver_core::protocol::ToolResult::error(format!(
        "type_text is unavailable for native ConsoleHost window class '{class}': \
         Windows can accept synthesized Unicode input without delivering it to \
         the prompt, so Cua Driver refuses rather than reporting a false success. \
         {suggestion}"
    ))
    .with_structured(serde_json::json!({
        "code": "input_delivery_unavailable",
        "target_class": class,
        "event_kind": "text_input",
        "effect": "refused",
        "retryable": false,
        "alternative_route": "process_or_pty",
        "suggestion": suggestion,
    }))
}

/// Returns `true` if the HWND at `hwnd` belongs to a known terminal
/// host. Uses the existing `input::delivery::read_class_name` helper
/// (cheap: one `GetClassNameW` call into a 256-WCHAR buffer).
#[cfg(target_os = "windows")]
pub fn is_terminal_hwnd(hwnd: u64) -> bool {
    if hwnd == 0 {
        return false;
    }
    let class = crate::input::delivery::read_class_name(hwnd);
    class_matches_terminal(&class)
}

/// Non-Windows build: there are no HWNDs, so the detector is a
/// constant `false`. Lets cross-target unit tests for
/// [`class_matches_terminal`] live in the same module.
#[cfg(not(target_os = "windows"))]
pub fn is_terminal_hwnd(_hwnd: u64) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_documented_terminal_classes() {
        // Real-world class names observed in the wild.
        for name in [
            "CASCADIA_HOSTING_WINDOW_CLASS",
            "CASCADIA_HOSTING_WINDOW_CLASS_0", // suffixed instance
            "mintty",
            "mintty_window",
            "ConsoleWindowClass",
            "Vim",
            "nvim",
        ] {
            assert!(
                class_matches_terminal(name),
                "expected terminal class {name:?} to match"
            );
        }
    }

    #[test]
    fn rejects_non_terminal_classes() {
        for name in [
            "Chrome_WidgetWin_0",
            "Chrome_WidgetWin_1",
            "Notepad",
            "WordPadClass",
            "HwndWrapper[default;;abc-123]", // WPF
            "gdkWindowToplevel",             // GTK
            "Edit",
            "Static",
            "",
        ] {
            assert!(
                !class_matches_terminal(name),
                "expected non-terminal class {name:?} to NOT match"
            );
        }
    }

    #[test]
    fn windows_terminal_and_mintty_present() {
        // Spot-check the trio called out in the bug report so the
        // regression that motivated this list can't quietly slip back
        // out during a refactor.
        assert!(class_matches_terminal("CASCADIA_HOSTING_WINDOW_CLASS"));
        assert!(class_matches_terminal("mintty"));
        assert!(class_matches_terminal("ConsoleWindowClass"));
    }

    #[test]
    fn native_consolehost_refuses_both_delivery_modes() {
        assert_eq!(
            type_text_policy_for_class("ConsoleWindowClass", false, true),
            TypeTextPolicy::Unsupported
        );
        assert_eq!(
            type_text_policy_for_class("ConsoleWindowClass", true, true),
            TypeTextPolicy::Unsupported
        );
        assert_eq!(
            type_text_policy_for_class("ConsoleWindowClass", true, false),
            TypeTextPolicy::Supported
        );

        let refusal =
            native_consolehost_type_text_error_for_class("ConsoleWindowClass".to_string());
        assert_eq!(refusal.is_error, Some(true));
        let structured = refusal.structured_content.expect("structured refusal");
        assert_eq!(structured["code"], "input_delivery_unavailable");
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["retryable"], false);
        assert_eq!(structured["alternative_route"], "process_or_pty");
    }

    #[test]
    fn other_terminals_still_allow_foreground_delivery() {
        assert_eq!(
            type_text_policy_for_class("CASCADIA_HOSTING_WINDOW_CLASS", false, true),
            TypeTextPolicy::ForegroundRequired
        );
        assert_eq!(
            type_text_policy_for_class("CASCADIA_HOSTING_WINDOW_CLASS", true, true),
            TypeTextPolicy::Supported
        );
        assert_eq!(
            type_text_policy_for_class("Notepad", false, true),
            TypeTextPolicy::Supported
        );
    }
}
