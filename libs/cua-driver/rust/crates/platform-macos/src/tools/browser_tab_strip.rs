//! Keep background clicks from switching the user's browser tab (#5397).
//!
//! A Chromium window's accessibility tree only exposes the web content of its
//! selected tab. An agent that wants another tab through `get_window_state`
//! therefore presses that tab in the browser's own tab strip, which changes
//! the tab the user is looking at. The typed browser tools (`get_browser_state`,
//! `browser_click`, `browser_type`, `browser_navigate`) reach an unselected tab
//! by `tab_id` without selecting it, so a background press on an unselected
//! tab-strip tab refuses and points there. `delivery_mode:"foreground"` still
//! selects the tab, and the result reports `side_effects.selected_browser_tab`.

use crate::ax::bindings::{copy_binary_attr, copy_element_attr, copy_string_attr, AXUIElementRef};
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::protocol::ToolResult;

/// Ancestors walked before giving up; a tab-strip tab sits a few levels under
/// its window, while web content sits under an `AXWebArea` much deeper down.
const MAX_ANCESTORS: usize = 40;
/// How close to the tab the `AXTabGroup` container must be.
const TAB_GROUP_MAX_DEPTH: usize = 3;

/// What the accessibility tree says about a pressed element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TabStripEvidence {
    pub chromium: bool,
    pub role: Option<String>,
    pub subrole: Option<String>,
    /// Depth of the nearest `AXTabGroup` ancestor (1 = parent), if any.
    pub tab_group_depth: Option<usize>,
    /// Whether an `AXWebArea` sits above the element: a page's own
    /// `role="tab"` widget, not the browser's tab strip.
    pub in_web_area: bool,
    /// The tab's `AXValue`: true when selected, false when not, `None` when
    /// unreadable.
    pub selected: Option<bool>,
}

/// Whether pressing this element would select a different tab in the
/// browser's own tab strip. Unknown selection state never refuses.
pub(super) fn selects_another_browser_tab(evidence: &TabStripEvidence) -> bool {
    let tab_role = matches!(
        evidence.role.as_deref(),
        Some("AXRadioButton") | Some("AXTab")
    );
    let in_tab_strip = evidence.subrole.as_deref() == Some("AXTabButton")
        || evidence
            .tab_group_depth
            .is_some_and(|depth| depth <= TAB_GROUP_MAX_DEPTH);
    evidence.chromium
        && tab_role
        && in_tab_strip
        && !evidence.in_web_area
        && evidence.selected == Some(false)
}

/// Read [`TabStripEvidence`] for `element` in a process `pid`.
///
/// # Safety
///
/// `element` must be a valid `AXUIElementRef` for the duration of the call.
pub(super) unsafe fn read_evidence(pid: i32, element: AXUIElementRef) -> TabStripEvidence {
    let bundle_id = crate::apps::bundle_id_for_pid(pid).unwrap_or_default();
    let mut evidence = TabStripEvidence {
        chromium: super::type_text::is_chromium_identity(&bundle_id),
        role: copy_string_attr(element, "AXRole"),
        subrole: copy_string_attr(element, "AXSubrole"),
        ..Default::default()
    };
    if !evidence.chromium
        || !matches!(
            evidence.role.as_deref(),
            Some("AXRadioButton") | Some("AXTab")
        )
    {
        return evidence;
    }
    evidence.selected = copy_binary_attr(element, "AXValue");

    let mut current = copy_element_attr(element, "AXParent");
    let mut depth = 1;
    while let Some(node) = current {
        let role = copy_string_attr(node, "AXRole");
        match role.as_deref() {
            Some("AXWebArea") => {
                evidence.in_web_area = true;
                CFRelease(node as CFTypeRef);
                break;
            }
            Some("AXTabGroup") if evidence.tab_group_depth.is_none() => {
                evidence.tab_group_depth = Some(depth);
            }
            Some("AXWindow") | Some("AXApplication") | None => {
                CFRelease(node as CFTypeRef);
                break;
            }
            _ => {}
        }
        if depth >= MAX_ANCESTORS {
            CFRelease(node as CFTypeRef);
            break;
        }
        let parent = copy_element_attr(node, "AXParent");
        CFRelease(node as CFTypeRef);
        current = parent;
        depth += 1;
    }
    evidence
}

/// Whether pressing `element` would switch the user's selected browser tab.
///
/// # Safety
///
/// `element` must be a valid `AXUIElementRef` for the duration of the call.
pub(super) unsafe fn press_selects_another_tab(pid: i32, element: AXUIElementRef) -> bool {
    selects_another_browser_tab(&read_evidence(pid, element))
}

/// Refusal for a background press on an unselected tab-strip tab.
///
/// # Safety
///
/// `element` must be a valid `AXUIElementRef` for the duration of the call.
pub(super) unsafe fn tab_switch_refusal(
    pid: i32,
    window_id: u32,
    element_index: Option<usize>,
    element: AXUIElementRef,
) -> ToolResult {
    let title = copy_string_attr(element, "AXTitle")
        .filter(|title| !title.is_empty())
        .or_else(|| copy_string_attr(element, "AXDescription"))
        .unwrap_or_default();
    let role = copy_string_attr(element, "AXRole").unwrap_or_else(|| "AXRadioButton".into());
    let target = match element_index {
        Some(index) => format!("[{index}] {role} \"{title}\""),
        None => format!("the {role} \"{title}\" at that point"),
    };
    ToolResult::error(format!(
        "Not selected: {target} is a tab in the browser's tab strip. Selecting it switches \
         the tab the user is looking at, so nothing was sent. Act on that tab without \
         selecting it: get_browser_state({{pid: {pid}, window_id: {window_id}}}) lists every \
         tab in this window, and browser_click, browser_type and browser_navigate take its \
         tab_id. Select the tab with delivery_mode:\"foreground\" only if the user agreed to \
         their visible tab changing."
    ))
    .with_structured(serde_json::json!({
        "code": "browser_tab_switch_refused",
        "effect": "refused",
        "pid": pid,
        "window_id": window_id,
        "tab_title": title,
        "hint": "Use get_browser_state and the typed browser tools with the tab's tab_id; they act on an unselected tab without selecting it.",
        "escalation": {
            "recommended": "get_browser_state",
            "reason": "the typed browser tools act on an unselected tab by tab_id without selecting it",
            "alternative_delivery_mode": "foreground",
        },
    }))
}

/// What a successful foreground press on another tab tells the caller.
pub(super) const SELECTED_TAB_NOTE: &str =
    " This selected another browser tab, so the tab the user sees changed.";

/// Record on a successful foreground press that it selected another tab, in
/// the summary text (which the public action result carries) and in the
/// legacy structured payload.
pub(super) fn report_selected_tab(mut result: ToolResult) -> ToolResult {
    if result.is_error == Some(true) {
        return result;
    }
    if let Some(cua_driver_core::protocol::Content::Text { text, .. }) = result.content.first_mut()
    {
        text.push_str(SELECTED_TAB_NOTE);
    }
    let structured = result
        .structured_content
        .get_or_insert_with(|| serde_json::json!({}));
    if let Some(object) = structured.as_object_mut() {
        let side_effects = object
            .entry("side_effects")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(side_effects) = side_effects.as_object_mut() {
            side_effects.insert("selected_browser_tab".into(), serde_json::json!(true));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome_tab(selected: Option<bool>) -> TabStripEvidence {
        TabStripEvidence {
            chromium: true,
            role: Some("AXRadioButton".into()),
            subrole: Some("AXTabButton".into()),
            tab_group_depth: Some(1),
            in_web_area: false,
            selected,
        }
    }

    #[test]
    fn unselected_browser_tab_refuses() {
        assert!(selects_another_browser_tab(&chrome_tab(Some(false))));
    }

    #[test]
    fn selected_or_unknown_tab_is_left_alone() {
        assert!(!selects_another_browser_tab(&chrome_tab(Some(true))));
        assert!(!selects_another_browser_tab(&chrome_tab(None)));
    }

    #[test]
    fn page_tab_widgets_are_not_the_tab_strip() {
        let mut page_tab = chrome_tab(Some(false));
        page_tab.in_web_area = true;
        assert!(!selects_another_browser_tab(&page_tab));
    }

    #[test]
    fn tab_group_ancestor_proves_the_tab_strip_without_a_subrole() {
        let mut tab = chrome_tab(Some(false));
        tab.subrole = None;
        assert!(selects_another_browser_tab(&tab));
        tab.tab_group_depth = Some(TAB_GROUP_MAX_DEPTH + 1);
        assert!(!selects_another_browser_tab(&tab));
        tab.tab_group_depth = None;
        assert!(!selects_another_browser_tab(&tab));
    }

    #[test]
    fn other_apps_and_roles_are_left_alone() {
        let mut other_app = chrome_tab(Some(false));
        other_app.chromium = false;
        assert!(!selects_another_browser_tab(&other_app));
        let mut button = chrome_tab(Some(false));
        button.role = Some("AXButton".into());
        assert!(!selects_another_browser_tab(&button));
    }

    #[test]
    fn foreground_report_adds_the_side_effect() {
        let result = report_selected_tab(
            ToolResult::text("clicked").with_structured(serde_json::json!({"path": "ax_fg"})),
        );
        let structured = result.structured_content.clone().unwrap();
        assert_eq!(structured["path"], "ax_fg");
        assert_eq!(structured["side_effects"]["selected_browser_tab"], true);
        assert!(matches!(
            result.content.first(),
            Some(cua_driver_core::protocol::Content::Text { text, .. })
                if text.ends_with(SELECTED_TAB_NOTE)
        ));
        let refused = report_selected_tab(ToolResult::error("nope"));
        assert!(refused.structured_content.is_none());
    }
}
