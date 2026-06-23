//! Terminal-emulator detection for the macOS `type_text` path.
//!
//! Terminal emulators expose an `AXTextArea` for their grid, but
//! `AXSelectedText` writes are dropped on the floor — the value-set
//! never reaches the pty, so `type_text` reports success while the
//! shell sees nothing. The user-visible symptom is "type was
//! acknowledged but nothing appeared", reported repeatedly against
//! Ghostty, iTerm2, and Apple's Terminal.app.
//!
//! Detection here is intentionally a small, explicit list — adding a
//! new terminal is a one-line append below. Match is done against the
//! target pid's bundle id (resolved via `NSRunningApplication`); the
//! call is cheap (one Objective-C method) and we only run it when
//! `type_text` is about to issue an AX value-set.
//!
//! Adding a new terminal: append the bundle id to [`TERMINAL_BUNDLE_IDS`]
//! and add a coverage line in the unit tests below.

/// Bundle identifiers of macOS terminal emulators where AX value-set is
/// known to be silently dropped — `type_text` skips the AX path and
/// goes straight to CGEvent key-event synthesis when the target pid's
/// bundle id is in this list.
///
/// Add new terminals here. Keep alphabetical so diffs stay sane.
pub const TERMINAL_BUNDLE_IDS: &[&str] = &[
    "co.zeit.hyper",            // Hyper
    "com.apple.Terminal",       // Apple Terminal.app
    "com.github.wez.wezterm",   // WezTerm
    "com.googlecode.iterm2",    // iTerm2
    "com.mitchellh.ghostty",    // Ghostty
    "dev.warp.Warp-Stable",     // Warp
    "dev.zed.Zed.Helper",       // Zed's embedded terminal helper
    "io.alacritty",             // Alacritty (newer bundle id)
    "net.kovidgoyal.kitty",     // kitty
    "org.alacritty",            // Alacritty (older bundle id)
];

/// Returns `true` when `bundle_id` matches a known terminal emulator
/// from [`TERMINAL_BUNDLE_IDS`]. Case-sensitive, exact match — bundle
/// ids are reverse-DNS and case-significant per Apple's convention.
pub fn is_terminal_bundle_id(bundle_id: &str) -> bool {
    TERMINAL_BUNDLE_IDS.iter().any(|b| *b == bundle_id)
}

/// Returns `true` when `pid` belongs to a known terminal emulator.
/// Resolves the bundle id via [`crate::apps::bundle_id_for_pid`]; if
/// resolution fails (unbundled process), returns `false` so the caller
/// keeps the existing AX path.
pub fn is_terminal_pid(pid: i32) -> bool {
    match crate::apps::bundle_id_for_pid(pid) {
        Some(bid) => is_terminal_bundle_id(&bid),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_documented_terminals() {
        for bid in TERMINAL_BUNDLE_IDS {
            assert!(
                is_terminal_bundle_id(bid),
                "documented terminal {bid:?} must match"
            );
        }
    }

    #[test]
    fn rejects_non_terminal_bundles() {
        for bid in [
            "com.apple.Safari",
            "com.apple.TextEdit",
            "com.apple.finder",
            "com.google.Chrome",
            "com.microsoft.VSCode",
            "com.tinyspeck.slackmacgap",
            "",
        ] {
            assert!(
                !is_terminal_bundle_id(bid),
                "non-terminal {bid:?} must not match"
            );
        }
    }

    #[test]
    fn ghostty_iterm_terminal_apple_all_present() {
        // Spot-check the trio called out in the bug report so the
        // regression that motivated this list can't quietly slip back
        // out of the list during a refactor.
        assert!(is_terminal_bundle_id("com.mitchellh.ghostty"));
        assert!(is_terminal_bundle_id("com.googlecode.iterm2"));
        assert!(is_terminal_bundle_id("com.apple.Terminal"));
    }

    #[test]
    fn case_sensitive_match() {
        // Bundle ids are case-significant — a capital-T variant must
        // not slip through if some path lower-cases the id.
        assert!(!is_terminal_bundle_id("COM.APPLE.TERMINAL"));
        assert!(!is_terminal_bundle_id("com.apple.terminal"));
    }
}
