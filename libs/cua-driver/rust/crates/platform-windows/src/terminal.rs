//! Terminal-emulator detection for the Windows `type_text` path.
//!
//! Terminal emulators (Windows Terminal, mintty / Git-Bash, Vim, NeoVim,
//! legacy `cmd.exe`) consume keyboard input through console / conpty
//! channels, not through the GUI's WM_CHAR queue. PostMessage(WM_CHAR)
//! against those hosts is silently dropped — the user reports
//! "type was acknowledged but nothing appeared". We detect the target
//! by window class name and route through SendInput (Unicode key
//! events) via the existing `inject_text_cloaked` primitive instead.
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

/// Returns `true` if the HWND at `hwnd` belongs to a known terminal
/// host. Uses the existing `input::dispatch::read_class_name` helper
/// (cheap: one `GetClassNameW` call into a 256-WCHAR buffer).
#[cfg(target_os = "windows")]
pub fn is_terminal_hwnd(hwnd: u64) -> bool {
    if hwnd == 0 {
        return false;
    }
    let class = crate::input::dispatch::read_class_name(hwnd);
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
            "CASCADIA_HOSTING_WINDOW_CLASS_0",   // suffixed instance
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
            "HwndWrapper[default;;abc-123]",  // WPF
            "gdkWindowToplevel",              // GTK
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
}
