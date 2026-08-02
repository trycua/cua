//! Cross-platform window-snapshot coverage signals.
//!
//! A browser permission bubble is browser chrome, not page content. Chromium
//! may composite that chrome in a popup/child surface which is visible in a
//! desktop capture but absent from a capture of the requested native window.
//! A single-window backend therefore cannot prove that no browser-owned blocker
//! exists. Keep that limitation explicit and machine-readable.

use serde_json::{json, Value};

pub const BROWSER_CHROME_COVERAGE_STATUS: &str = "not_observable_in_window_scope";

/// Describe the browser-chrome coverage limit of a Chromium-family window
/// snapshot.
///
/// This deliberately does **not** claim that a prompt is present. Presence is
/// not observable from a single native-window surface on every supported
/// platform. It also carries no prompt text or choices, so routine traces can
/// retain the recovery signal without retaining permission content.
pub fn mark_browser_chrome_capture_coverage(structured: &mut Value, chromium_family_window: bool) {
    if !chromium_family_window {
        return;
    }

    structured["capture_coverage"] = json!({
        "browser_chrome": {
            "status": BROWSER_CHROME_COVERAGE_STATUS
        },
        "recovery": {
            "when": "verified_window_action_ineffective",
            "escalate": {
                "tool": "escalate_session",
                "reason": "foreground_ineffective"
            },
            "inspect": "get_desktop_state",
            "act_scope": "desktop",
            "verify": "get_desktop_state"
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_window_declares_capture_coverage_without_claiming_presence() {
        let mut before = json!({
            "window_id": 7,
            "pid": 42,
            "tree_markdown": "page=unchanged",
            "screenshot_width": 900,
            "screenshot_height": 640
        });
        let mut after_visible_browser_blocker = before.clone();

        mark_browser_chrome_capture_coverage(&mut before, true);
        mark_browser_chrome_capture_coverage(&mut after_visible_browser_blocker, true);

        // Even when the page-owned state is byte-for-byte unchanged, callers
        // receive an explicit recovery branch instead of treating the window
        // snapshot as proof that the action was ignored.
        assert_eq!(before, after_visible_browser_blocker);
        assert_eq!(
            before["capture_coverage"]["browser_chrome"]["status"],
            BROWSER_CHROME_COVERAGE_STATUS
        );
        assert_eq!(
            before["capture_coverage"]["recovery"]["when"],
            "verified_window_action_ineffective"
        );
        assert_eq!(
            before["capture_coverage"]["recovery"]["escalate"],
            json!({
                "tool": "escalate_session",
                "reason": "foreground_ineffective"
            })
        );
        assert_eq!(
            before["capture_coverage"]["recovery"]["inspect"],
            "get_desktop_state"
        );
        assert!(before.get("browser_chrome_prompt").is_none());
    }

    #[test]
    fn signal_is_privacy_minimal_and_does_not_change_non_browser_snapshots() {
        let mut ordinary = json!({"window_id": 9, "pid": 3});
        let unchanged = ordinary.clone();
        mark_browser_chrome_capture_coverage(&mut ordinary, false);
        assert_eq!(ordinary, unchanged);

        let mut browser = unchanged;
        mark_browser_chrome_capture_coverage(&mut browser, true);
        let public = browser.to_string();
        for sensitive_key in ["text", "message", "choice", "allow", "deny"] {
            assert!(
                !public.contains(sensitive_key),
                "coverage signal leaked a prompt-content field: {public}"
            );
        }
    }
}
