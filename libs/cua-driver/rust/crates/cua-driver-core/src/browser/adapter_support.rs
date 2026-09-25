//! Parsing, classification, and cursor bookkeeping shared by the platform
//! browser adapters.
//!
//! Every platform adapter reads Chromium's `DevToolsActivePort` file,
//! re-proves that a discovered websocket stays on the attested loopback
//! listener, recognizes Firefox, and keeps one visible browser cursor per
//! native window. Those semantics are platform-independent, so they live here
//! and the adapters stay thin.

use std::collections::HashMap;

/// Parse Chromium's `DevToolsActivePort` file.
///
/// The file must hold exactly two non-empty lines: the listener port and one
/// `/devtools/browser/<id>` path whose id is ASCII alphanumeric, `-`, or `_`.
/// Anything else, including page paths and trailing lines, is refused so the
/// caller never attaches to an endpoint the file does not name exactly.
pub fn parse_devtools_active_port(text: &str) -> Option<(u16, &str)> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let port = lines.next()?.parse::<u16>().ok()?;
    let path = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    let instance = path.strip_prefix("/devtools/browser/")?;
    (!instance.is_empty()
        && instance
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then_some((port, path))
}

/// Return the port of a loopback DevTools websocket URL.
///
/// Accepts `127.0.0.1`, `localhost`, and `[::1]` hosts only, so a
/// discovered URL that points anywhere else can never match the attested
/// listener port.
pub fn loopback_websocket_port(url: &str) -> Option<u16> {
    ["ws://127.0.0.1:", "ws://localhost:", "ws://[::1]:"]
        .iter()
        .find_map(|prefix| {
            url.strip_prefix(prefix)?
                .split('/')
                .next()?
                .parse::<u16>()
                .ok()
        })
}

/// Whether a process identity names Firefox.
///
/// `identity` is whatever the adapter has for the process: an executable
/// name, an executable path, or an app name joined with its bundle id. The
/// identity is split on every non-alphanumeric character and matches when one
/// token is exactly `firefox`, so `firefox.exe`, `/usr/lib/firefox-esr/firefox-esr`
/// and `org.mozilla.firefox` match while `FirefoxHelper` and `waterfox` do
/// not.
pub fn is_firefox(identity: &str) -> bool {
    identity
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token == "firefox")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserCursorBinding {
    window_id: u64,
    cdp_target_id: String,
}

/// Session-to-tab bindings for browser cursors.
///
/// An active tab owns the sole visible browser cursor in its native window.
/// An inactive tab hides only its own cursor and never disturbs the cursor for
/// the selected tab.
#[derive(Debug, Default)]
pub struct BrowserCursorTracker {
    bindings: HashMap<String, BrowserCursorBinding>,
}

impl BrowserCursorTracker {
    /// Record the session-to-tab binding and return the exact overlay
    /// visibility changes, as `(session, visible)` pairs, needed for this
    /// action.
    pub fn update(
        &mut self,
        session: &str,
        window_id: u64,
        cdp_target_id: &str,
        tab_is_active: bool,
    ) -> Vec<(String, bool)> {
        self.bindings.insert(
            session.to_owned(),
            BrowserCursorBinding {
                window_id,
                cdp_target_id: cdp_target_id.to_owned(),
            },
        );

        if !tab_is_active {
            return vec![(session.to_owned(), false)];
        }

        self.bindings
            .iter()
            .filter(|(_, binding)| binding.window_id == window_id)
            .map(|(key, binding)| {
                (
                    key.clone(),
                    key == session && binding.cdp_target_id == cdp_target_id,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_port_parser_requires_one_exact_browser_path() {
        assert_eq!(
            parse_devtools_active_port(
                "9222\n/devtools/browser/f1d991b4-2694-4b28-b63a-1f2a8da3a435\n"
            ),
            Some((
                9222,
                "/devtools/browser/f1d991b4-2694-4b28-b63a-1f2a8da3a435"
            ))
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/abc_123\n"),
            Some((9222, "/devtools/browser/abc_123"))
        );
        for refused in [
            "9222\n/devtools/browser\n",
            "9222\n/devtools/browser/\n",
            "9222\n/devtools/page/id\n",
            "9222\n/devtools/browser/id\nextra\n",
            "9222\n/devtools/browser/../page\n",
            "not-a-port\n/devtools/browser/id\n",
            "9222\n",
        ] {
            assert_eq!(parse_devtools_active_port(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn websocket_url_must_keep_the_attested_listener_port() {
        for (url, port) in [
            ("ws://127.0.0.1:9222/devtools/browser/id", Some(9222)),
            ("ws://localhost:9333/devtools/browser/id", Some(9333)),
            ("ws://[::1]:9444/devtools/browser/id", Some(9444)),
            ("ws://0.0.0.0:9222/devtools", None),
            ("ws://192.0.2.1:9222/devtools", None),
            ("wss://127.0.0.1:9222/devtools", None),
        ] {
            assert_eq!(loopback_websocket_port(url), port, "{url}");
        }
    }

    #[test]
    fn firefox_classifier_uses_product_tokens() {
        for identity in [
            "firefox.exe",
            "Mozilla Firefox.exe",
            "Firefox org.mozilla.firefox",
            "/usr/lib/firefox/firefox",
            "/usr/lib/firefox-esr/firefox-esr",
        ] {
            assert!(is_firefox(identity), "{identity}");
        }
        for identity in [
            "FirefoxHelper.exe",
            "FirefoxHelper com.example.FirefoxHelper",
            "waterfox",
            "Waterfox net.waterfox.current",
        ] {
            assert!(!is_firefox(identity), "{identity}");
        }
    }

    #[test]
    fn browser_cursor_tracker_shows_only_the_active_tabs_session_per_window() {
        let mut tracker = BrowserCursorTracker::default();
        assert_eq!(
            tracker.update("session-red", 77, "tab-A", false),
            vec![("session-red".to_owned(), false)]
        );

        let first_active = tracker.update("session-red", 77, "tab-A", true);
        assert_eq!(first_active, vec![("session-red".to_owned(), true)]);

        let second_active = tracker
            .update("session-blue", 77, "tab-B", true)
            .into_iter()
            .collect::<HashMap<_, _>>();
        assert_eq!(second_active.get("session-red"), Some(&false));
        assert_eq!(second_active.get("session-blue"), Some(&true));

        let mut back_to_red = tracker.update("session-red", 77, "tab-A", true);
        back_to_red.sort();
        assert_eq!(
            back_to_red,
            vec![
                ("session-blue".to_owned(), false),
                ("session-red".to_owned(), true)
            ]
        );

        let other_window = tracker.update("session-green", 88, "tab-C", true);
        assert_eq!(
            other_window,
            vec![("session-green".to_owned(), true)],
            "an active tab in another native window must not hide this window"
        );
    }
}
