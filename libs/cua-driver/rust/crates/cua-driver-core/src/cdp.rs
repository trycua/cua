// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cross-platform CDP (Chrome DevTools Protocol) helper.
//!
//! The legacy `Runtime.evaluate` path of the `page` tool was removed with that
//! tool's page actions; what remains is the shared page-target selection used
//! by the macOS CDP client.

use serde_json::Value;

/// Pick the unique page whose `url` contains `hint` (case-insensitive), or
/// the first page when no hint is given.
///
/// Explicit hints fail closed when zero or multiple pages match: CDP target
/// ids carry no relationship to the caller's window, so a browser with more
/// than one tab open would otherwise be picked non-deterministically.
pub fn pick_page<'a>(
    pages: impl IntoIterator<Item = &'a Value>,
    hint: Option<&str>,
) -> Option<&'a Value> {
    let mut pages = pages.into_iter();
    match hint {
        None => pages.next(),
        Some(hint) => {
            let hint_lower = hint.to_ascii_lowercase();
            let mut matches = pages.filter(|page| {
                page.get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|url| url.to_ascii_lowercase().contains(&hint_lower))
            });
            let page = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            Some(page)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pick_page;
    use serde_json::json;

    fn pages() -> Vec<serde_json::Value> {
        vec![
            json!({ "url": "app://fixture/#window-a", "webSocketDebuggerUrl": "ws://a" }),
            json!({ "url": "app://fixture/#window-b", "webSocketDebuggerUrl": "ws://b" }),
        ]
    }

    #[test]
    fn targeted_page_selection_is_unique_and_case_insensitive() {
        let pages = pages();
        assert_eq!(
            pick_page(&pages, Some("#WINDOW-B"))
                .and_then(|page| page["webSocketDebuggerUrl"].as_str()),
            Some("ws://b")
        );
        assert!(pick_page(&pages, Some("#missing")).is_none());
        assert!(pick_page(&pages, Some("app://fixture/")).is_none());
    }

    #[test]
    fn omitted_page_hint_preserves_first_page_behavior() {
        let pages = pages();
        assert_eq!(
            pick_page(&pages, None).and_then(|page| page["webSocketDebuggerUrl"].as_str()),
            Some("ws://a")
        );
    }
}
