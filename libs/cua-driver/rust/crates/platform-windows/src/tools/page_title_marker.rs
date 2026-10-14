//! Parse the `CUA:` / `CUA_ERR:` result marker that the bookmark
//! JavaScript fallback in [`super::page_bookmark`] writes into the active
//! tab's title.
//!
//! This is pure string handling with no Win32 dependency, so it compiles
//! and its tests run on every host.

/// Browser names that Chromium-family browsers append to a tab or window
/// title as ` - <name>`. Compared after [`normalize_suffix`].
const BROWSER_TITLE_SUFFIXES: &[&str] = &[
    "microsoft edge",
    "edge",
    "google chrome",
    "chrome",
    "chromium",
    "brave",
    "brave browser",
    "arc",
    "vivaldi",
    "opera",
    "opera gx",
];

/// Release channels that can follow a browser name, e.g.
/// `Google Chrome Canary` or `Microsoft Edge Beta`.
const BROWSER_CHANNELS: &[&str] = &["beta", "dev", "canary", "nightly"];

/// Split `s` at the prefix and strip Chromium's trailing
/// ` - <browser name>` only. Payloads that legitimately contain ` - ` in
/// their JSON content (e.g. `CUA:"a - b"`) must stay intact.
///
/// We rsplit from the END (so only the last `" - "` is a candidate
/// separator) and strip the suffix only when it is exactly a known
/// browser name, optionally followed by a release channel. A suffix
/// that merely contains a browser name (`search` contains `arc`,
/// `operation` contains `opera`) stays in the payload verbatim.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn extract_marker(s: &str, prefix: &str) -> String {
    let start = s.find(prefix).unwrap_or(0);
    let after = &s[start..];
    if let Some((left, right)) = after.rsplit_once(" - ") {
        if is_browser_title_suffix(right) {
            return left.to_owned();
        }
    }
    after.to_owned()
}

fn is_browser_title_suffix(suffix: &str) -> bool {
    let normalized = normalize_suffix(suffix);
    let name = match normalized.rsplit_once(' ') {
        Some((base, channel)) if BROWSER_CHANNELS.contains(&channel) => base,
        _ => normalized.as_str(),
    };
    BROWSER_TITLE_SUFFIXES.contains(&name)
}

/// Lowercase, drop zero-width characters (Edge titles use
/// `Microsoft\u{200B} Edge`), and collapse runs of whitespace.
fn normalize_suffix(suffix: &str) -> String {
    let cleaned: String = suffix
        .chars()
        .filter(|c| !matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}'))
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_marker_strips_chromium_suffix() {
        assert_eq!(extract_marker("CUA:42", "CUA:"), "CUA:42");
        assert_eq!(extract_marker("CUA:42 - Microsoft Edge", "CUA:"), "CUA:42");
        assert_eq!(extract_marker("CUA:42 - Google Chrome", "CUA:"), "CUA:42");
        assert_eq!(extract_marker("Foo CUA:42 - Edge", "CUA:"), "CUA:42");
        assert_eq!(
            extract_marker("CUA_ERR:not defined - Edge", "CUA_ERR:"),
            "CUA_ERR:not defined"
        );
    }

    #[test]
    fn extract_marker_strips_every_known_browser_suffix() {
        for suffix in [
            "Microsoft Edge",
            "Microsoft\u{200B} Edge",
            "Microsoft Edge Beta",
            "Google Chrome",
            "Google Chrome Canary",
            "  google chrome  ",
            "Chrome",
            "Chromium",
            "Brave",
            "Brave Browser",
            "Arc",
            "Vivaldi",
            "Opera",
            "Opera GX",
        ] {
            let title = format!("CUA:42 - {suffix}");
            assert_eq!(extract_marker(&title, "CUA:"), "CUA:42", "{title:?}");
        }
    }

    #[test]
    fn extract_marker_preserves_dash_in_payload() {
        // Payload "a - b" must NOT be truncated at the embedded " - ".
        // (The previous `find(" - ")` implementation would return
        // `CUA:"a` here — wrong.)
        assert_eq!(
            extract_marker("CUA:\"a - b\" - Microsoft Edge", "CUA:"),
            "CUA:\"a - b\""
        );
        // No browser suffix present → keep the whole payload, even with
        // a stray " - " in the middle.
        assert_eq!(
            extract_marker("CUA:\"foo - bar\"", "CUA:"),
            "CUA:\"foo - bar\""
        );
        // The suffix must be a known browser to be stripped — an
        // arbitrary trailing " - X" stays in the payload.
        assert_eq!(
            extract_marker("CUA:result - notabrowser", "CUA:"),
            "CUA:result - notabrowser"
        );
    }

    #[test]
    fn extract_marker_keeps_suffix_that_only_contains_a_browser_name() {
        // "search" contains "arc", "operation" contains "opera",
        // "knowledge" contains "edge". None is a browser suffix.
        for (title, expected) in [
            ("CUA:\"x - search\"", "CUA:\"x - search\""),
            ("CUA:\"x - operation\"", "CUA:\"x - operation\""),
            ("CUA:\"x - knowledge\"", "CUA:\"x - knowledge\""),
            ("CUA_ERR:x - chromebook", "CUA_ERR:x - chromebook"),
            ("CUA:\"x - search\" - Microsoft Edge", "CUA:\"x - search\""),
        ] {
            let prefix = if title.starts_with("CUA_ERR:") {
                "CUA_ERR:"
            } else {
                "CUA:"
            };
            assert_eq!(extract_marker(title, prefix), expected, "{title:?}");
        }
    }
}
