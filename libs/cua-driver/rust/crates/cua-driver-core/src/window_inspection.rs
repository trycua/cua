//! Cross-platform window-snapshot coverage signals.
//!
//! A browser permission bubble is browser chrome, not page content. Chromium
//! may composite that chrome in a popup/child surface which is visible in a
//! desktop capture but absent from a capture of the requested native window.
//! A single-window backend therefore cannot prove that no browser-owned blocker
//! exists. Keep that limitation explicit and machine-readable.

use serde_json::{json, Value};

pub const BROWSER_CHROME_DESKTOP_REASON: &str = "browser_chrome_may_be_outside_window_capture";

/// Mark a Chromium-family window snapshot as insufficient to rule out
/// browser-owned chrome.
///
/// This deliberately does **not** claim that a prompt is present. Presence is
/// not observable from a single native-window surface on every supported
/// platform. It also carries no prompt text or choices, so routine traces can
/// retain the recovery signal without retaining permission content.
pub fn mark_browser_chrome_desktop_inspection(
    structured: &mut Value,
    chromium_family_window: bool,
) {
    if !chromium_family_window {
        return;
    }

    structured["desktop_inspection_required"] = json!(true);
    structured["desktop_inspection_reason"] = json!(BROWSER_CHROME_DESKTOP_REASON);
    structured["browser_chrome_prompt"] = json!({
        "status": "not_observable_in_window_scope",
        "recovery": {
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
    fn browser_window_requires_desktop_inspection_without_claiming_presence() {
        let mut before = json!({
            "window_id": 7,
            "pid": 42,
            "tree_markdown": "page=unchanged",
            "screenshot_width": 900,
            "screenshot_height": 640
        });
        let mut after_visible_browser_blocker = before.clone();

        mark_browser_chrome_desktop_inspection(&mut before, true);
        mark_browser_chrome_desktop_inspection(&mut after_visible_browser_blocker, true);

        // Even when the page-owned state is byte-for-byte unchanged, callers
        // receive an explicit recovery branch instead of treating the window
        // snapshot as proof that the action was ignored.
        assert_eq!(before, after_visible_browser_blocker);
        assert_eq!(before["desktop_inspection_required"], true);
        assert_eq!(
            before["desktop_inspection_reason"],
            BROWSER_CHROME_DESKTOP_REASON
        );
        assert_eq!(
            before["browser_chrome_prompt"]["status"],
            "not_observable_in_window_scope"
        );
        assert!(before["browser_chrome_prompt"].get("present").is_none());
    }

    #[test]
    fn signal_is_privacy_minimal_and_does_not_change_non_browser_snapshots() {
        let mut ordinary = json!({"window_id": 9, "pid": 3});
        let unchanged = ordinary.clone();
        mark_browser_chrome_desktop_inspection(&mut ordinary, false);
        assert_eq!(ordinary, unchanged);

        let mut browser = unchanged;
        mark_browser_chrome_desktop_inspection(&mut browser, true);
        let public = browser.to_string();
        for sensitive_key in ["text", "message", "choice", "allow", "deny"] {
            assert!(
                !public.contains(sensitive_key),
                "coverage signal leaked a prompt-content field: {public}"
            );
        }
    }
}
