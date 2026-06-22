//! Terminal-emulator detection for the Linux `type_text` path.
//!
//! Linux already routes terminal typing through pty injection
//! (`inject_terminal_input` in tools/impl_.rs), but that detection is
//! gated on a process-name match. Terminals like Ghostty don't always
//! resolve to a clean `process_name` (the binary is reported as
//! `ghostty` but the WM_CLASS may be the user-facing name), and the
//! WM_CLASS-based fallback below covers the gap.
//!
//! WM_CLASS substrings here are matched case-insensitively against
//! both the instance and class fields of `WM_CLASS`. Adding a new
//! terminal: append a substring to [`TERMINAL_WM_CLASS_SUBSTRINGS`]
//! and to [`TERMINAL_PROCESS_NAMES`] if the binary name is known.

/// Substrings that, when found in a window's `WM_CLASS` instance or
/// class field (case-insensitive), identify a terminal emulator.
///
/// Used by [`is_terminal_window`] to back up the per-pid
/// `is_terminal_process` check — handy when the terminal's binary
/// name doesn't appear in the limited list but its WM_CLASS does.
pub const TERMINAL_WM_CLASS_SUBSTRINGS: &[&str] = &[
    "alacritty",
    "ghostty",
    "gnome-terminal",
    "iterm",
    "kitty",
    "konsole",
    "terminal",   // catches "Terminal", "xfce4-terminal-window", etc.
    "tilix",
    "wezterm",
    "xterm",
];

/// Process names (as returned by `process_name(pid)`) for known
/// terminal emulators. Kept in sync with the inline list in
/// `tools/impl_.rs::is_terminal_process` — this constant is the
/// canonical source.
pub const TERMINAL_PROCESS_NAMES: &[&str] = &[
    "alacritty",
    "ghostty",
    "gnome-terminal-server",
    "kitty",
    "konsole",
    "tilix",
    "wezterm-gui",
    "xfce4-terminal",
    "xterm",
];

/// Returns `true` if `process_name` is a known terminal emulator
/// binary name.
pub fn is_terminal_process_name(process_name: &str) -> bool {
    TERMINAL_PROCESS_NAMES.iter().any(|n| *n == process_name)
}

/// Returns `true` if either the instance or class field of `WM_CLASS`
/// contains (case-insensitively) one of the known terminal-emulator
/// substrings.
pub fn wm_class_matches_terminal(instance: &str, class: &str) -> bool {
    let i = instance.to_ascii_lowercase();
    let c = class.to_ascii_lowercase();
    TERMINAL_WM_CLASS_SUBSTRINGS
        .iter()
        .any(|s| i.contains(s) || c.contains(s))
}

/// Returns `true` if the window with X11 id `xid` belongs to a
/// terminal emulator, based on its `WM_CLASS` property. On native
/// Wayland the dispatcher folds the foreign-toplevel `app_id` into
/// both fields so the substring match still works for terminals like
/// Ghostty / kitty / alacritty. Returns `false` when no connection is
/// available or the property is unset.
#[cfg(target_os = "linux")]
pub fn is_terminal_window(xid: u64) -> bool {
    match crate::wayland::wm_class_dispatch(xid) {
        Some((instance, class)) => wm_class_matches_terminal(&instance, &class),
        None => false,
    }
}

/// Non-Linux build: the WM_CLASS-based detector is a no-op (there are
/// no X11 windows). Lets the function be referenced unconditionally
/// without per-call `#[cfg]` guards at the call site.
#[cfg(not(target_os = "linux"))]
pub fn is_terminal_window(_xid: u64) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_names_match_documented_terminals() {
        for n in TERMINAL_PROCESS_NAMES {
            assert!(
                is_terminal_process_name(n),
                "documented terminal process {n:?} must match"
            );
        }
    }

    #[test]
    fn rejects_non_terminal_process_names() {
        for n in [
            "firefox",
            "code",
            "chrome",
            "nautilus",
            "gedit",
            "",
        ] {
            assert!(
                !is_terminal_process_name(n),
                "non-terminal {n:?} must not match"
            );
        }
    }

    #[test]
    fn wm_class_matches_documented_terminals() {
        // Real-world WM_CLASS pairs observed across distros.
        for (instance, class) in [
            ("alacritty", "Alacritty"),
            ("kitty", "kitty"),
            ("ghostty", "Ghostty"),
            ("Konsole", "konsole"),
            ("gnome-terminal-server", "Gnome-terminal"),
            ("xterm", "XTerm"),
            ("tilix", "Tilix"),
            ("iterm", "iTerm"),  // hypothetical port, but the substring matters
        ] {
            assert!(
                wm_class_matches_terminal(instance, class),
                "WM_CLASS=({instance:?}, {class:?}) must match a terminal"
            );
        }
    }

    #[test]
    fn wm_class_rejects_browsers_and_editors() {
        for (instance, class) in [
            ("firefox", "Firefox"),
            ("code", "Code"),
            ("chrome", "Google-chrome"),
            ("nautilus", "Org.gnome.Nautilus"),
            ("gedit", "Gedit"),
            ("", ""),
        ] {
            assert!(
                !wm_class_matches_terminal(instance, class),
                "WM_CLASS=({instance:?}, {class:?}) must not match a terminal"
            );
        }
    }

    #[test]
    fn ghostty_matches() {
        // Spot-check Ghostty across casing / field positions — that was
        // the report's primary regression.
        assert!(wm_class_matches_terminal("ghostty", "Ghostty"));
        assert!(wm_class_matches_terminal("Ghostty", ""));
        assert!(wm_class_matches_terminal("", "ghostty"));
        assert!(is_terminal_process_name("ghostty"));
    }
}
