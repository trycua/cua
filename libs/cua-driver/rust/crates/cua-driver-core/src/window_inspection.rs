//! Cross-platform window-snapshot coverage signals.
//!
//! A browser permission bubble is browser chrome, not page content. Chromium
//! may composite that chrome in the requested window or in a popup/child
//! surface which is visible only in a desktop capture. A single-window backend
//! therefore cannot always prove that no browser-owned blocker exists. Keep
//! that platform-specific limitation explicit and machine-readable.

use serde_json::{json, Value};

pub const BROWSER_CHROME_COVERAGE_STATUS: &str = "not_observable_in_window_scope";
pub const BROWSER_CHROME_MAY_BE_INCOMPLETE_STATUS: &str = "may_be_incomplete_in_window_scope";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserChromeCaptureCoverage {
    NotObservable,
    MayBeIncomplete,
}

impl BrowserChromeCaptureCoverage {
    fn status(self) -> &'static str {
        match self {
            Self::NotObservable => BROWSER_CHROME_COVERAGE_STATUS,
            Self::MayBeIncomplete => BROWSER_CHROME_MAY_BE_INCOMPLETE_STATUS,
        }
    }
}

/// Describe the browser-chrome coverage limit of a Chromium-family window
/// snapshot.
///
/// This deliberately does **not** claim that a prompt is present. Presence is
/// not reliably complete from a single native-window surface on every
/// supported platform. It also carries no prompt text or choices, so routine
/// traces can retain the recovery signal without retaining permission content.
pub fn mark_browser_chrome_capture_coverage(
    structured: &mut Value,
    coverage: Option<BrowserChromeCaptureCoverage>,
) {
    let Some(coverage) = coverage else { return };

    structured["capture_coverage"] = json!({
        "browser_chrome": {
            "status": coverage.status()
        },
        "recovery": {
            "when": "verified_window_action_ineffective",
            "inspect": "get_desktop_state",
            "act_target": {
                "kind": "desktop",
                "display_id": "primary"
            },
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

        mark_browser_chrome_capture_coverage(
            &mut before,
            Some(BrowserChromeCaptureCoverage::NotObservable),
        );
        mark_browser_chrome_capture_coverage(
            &mut after_visible_browser_blocker,
            Some(BrowserChromeCaptureCoverage::NotObservable),
        );

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
        assert!(before["capture_coverage"]["recovery"]
            .get("escalate")
            .is_none());
        assert_eq!(
            before["capture_coverage"]["recovery"]["inspect"],
            "get_desktop_state"
        );
        assert_eq!(
            before["capture_coverage"]["recovery"]["act_target"],
            json!({"kind": "desktop", "display_id": "primary"})
        );
        assert!(before.get("browser_chrome_prompt").is_none());
    }

    #[test]
    fn signal_is_privacy_minimal_and_does_not_change_non_browser_snapshots() {
        let mut ordinary = json!({"window_id": 9, "pid": 3});
        let unchanged = ordinary.clone();
        mark_browser_chrome_capture_coverage(&mut ordinary, None);
        assert_eq!(ordinary, unchanged);

        let mut browser = unchanged;
        mark_browser_chrome_capture_coverage(
            &mut browser,
            Some(BrowserChromeCaptureCoverage::MayBeIncomplete),
        );
        assert_eq!(
            browser["capture_coverage"]["browser_chrome"]["status"],
            BROWSER_CHROME_MAY_BE_INCOMPLETE_STATUS
        );
        let public = browser.to_string();
        for sensitive_key in ["text", "message", "choice", "allow", "deny"] {
            assert!(
                !public.contains(sensitive_key),
                "coverage signal leaked a prompt-content field: {public}"
            );
        }
    }
}
