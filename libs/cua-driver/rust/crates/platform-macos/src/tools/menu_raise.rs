// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Background actions never open a menu in a window another app covers.
//!
//! When an app that is not frontmost opens a menu (a popup button's menu, a
//! context menu), WindowServer orders the menu's window group to the front,
//! and the group includes the window the menu belongs to. The key-focus guard
//! puts the user's window back about 20 ms later, but the raise is on the
//! user's screen for a frame or two first. No accessibility route chooses an
//! AppKit menu item without the menu, so a background action that would open
//! one is refused with `background_unavailable` and nothing is sent.

use cua_driver_core::protocol::ToolResult;

use crate::ax::bindings::{copy_action_names, copy_string_attr, AXUIElementRef};

/// `reason` of a refused popup (popup button, `<select>`).
pub(super) const POPUP_MENU_WOULD_RAISE: &str = "popup_menu_would_raise_window";
/// `reason` of a refused context menu.
pub(super) const CONTEXT_MENU_WOULD_RAISE: &str = "context_menu_would_raise_window";

/// Which menu a refused action would have opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Menu {
    Popup,
    Context,
}

/// The element's title, or its accessibility label when it has no title (an
/// AppKit control labelled by `setAccessibilityLabel`).
pub(super) fn label(element: AXUIElementRef) -> String {
    let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
    if !title.is_empty() {
        return title;
    }
    unsafe { copy_string_attr(element, "AXDescription") }.unwrap_or_default()
}

/// The refusal for an action on `target` (e.g. `[3] AXPopUpButton "Team"`)
/// that would open `menu` in `window_id` while another app's window covers it.
/// `doing` names the action ("choosing \"High\"", "opening it").
pub(super) fn refusal(
    pid: i32,
    window_id: u32,
    target: &str,
    menu: Menu,
    doing: &str,
    current: Option<&str>,
) -> ToolResult {
    let (reason, what, route) = match menu {
        Menu::Popup => (
            POPUP_MENU_WOULD_RAISE,
            "needs its menu open",
            "If the user agrees to this window coming forward, call bring_to_front for it, \
             then set_value again.",
        ),
        Menu::Context => (
            CONTEXT_MENU_WOULD_RAISE,
            "opens a context menu",
            "Use the same command from the app's menu bar (invoke_menu) or its keyboard \
             shortcut instead. If the user agrees to this window coming forward, call \
             bring_to_front for it first.",
        ),
    };
    let current_note = current
        .map(|value| format!(" Its current value is \"{value}\"."))
        .unwrap_or_default();
    let mut fields = serde_json::json!({
        "reason": reason,
        "effect": "refused",
        "pid": pid,
        "window_id": window_id,
    });
    if let Some(value) = current {
        fields["current_value"] = serde_json::json!(value);
    }
    cua_driver_core::delivery::background_unavailable_result_with_suggestion(
        format!(
            "Not changed: {doing} on {target} {what}, and while this app is behind the user's \
             window, macOS brings the window above theirs as the menu opens. Nothing was \
             sent.{current_note} {route}"
        ),
        "background_unavailable",
        "a menu opened in the background raises its window above the user's window",
        fields,
        route,
    )
}

/// `[3] AXButton "Copy"`, or `the AXButton "Copy" at that point` without an
/// index.
pub(super) fn describe(element_index: Option<usize>, role: &str, label: &str) -> String {
    match element_index {
        Some(index) => format!("[{index}] {role} \"{label}\""),
        None => format!("the {role} \"{label}\" at that point"),
    }
}

/// Refusal for a background right-click, show-menu, or press on `element`
/// that would open a menu raising `window_id` over another app's window: the
/// menu of a popup button, or the context menu of an element that advertises
/// `AXShowMenu`. `context` is true for a right-click or show-menu.
pub(super) fn background_menu_refusal(
    pid: i32,
    window_id: u32,
    element_index: Option<usize>,
    element: AXUIElementRef,
    context: bool,
) -> Option<ToolResult> {
    if let Some(refusal) =
        super::set_value::background_popup_open_refusal(pid, window_id, element_index, element)
    {
        return Some(refusal);
    }
    if !context
        || !unsafe { copy_action_names(element) }
            .iter()
            .any(|action| action == "AXShowMenu")
        || !crate::key_focus_guard::menu_open_would_raise(pid, window_id)
    {
        return None;
    }
    let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
    Some(refusal(
        pid,
        window_id,
        &describe(element_index, &role, &label(element)),
        Menu::Context,
        "right-clicking",
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(result: &ToolResult) -> String {
        result
            .content
            .iter()
            .find_map(|block| match block {
                cua_driver_core::protocol::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .expect("text")
    }

    #[test]
    fn a_refused_popup_is_background_unavailable_and_names_the_route() {
        let refused = refusal(
            7,
            42,
            &describe(Some(3), "AXPopUpButton", "Priority"),
            Menu::Popup,
            "choosing \"High\"",
            Some("Low"),
        );
        assert_eq!(refused.is_error, Some(true));
        let message = text(&refused);
        assert!(
            message.contains("[3] AXPopUpButton \"Priority\""),
            "{message}"
        );
        assert!(message.contains("\"Low\""), "{message}");
        assert!(message.contains("bring_to_front"), "{message}");
        let data = refused.structured_content.expect("structured");
        assert_eq!(data["code"], "background_unavailable");
        assert_eq!(data["reason"], POPUP_MENU_WOULD_RAISE);
        assert_eq!(data["effect"], "refused");
        assert_eq!(data["current_value"], "Low");
        assert_eq!(data["escalation"]["recommended"], "foreground");
    }

    #[test]
    fn a_refused_context_menu_points_at_the_menu_bar() {
        let refused = refusal(
            7,
            42,
            &describe(None, "AXButton", "Context"),
            Menu::Context,
            "right-clicking",
            None,
        );
        let message = text(&refused);
        assert!(
            message.contains("the AXButton \"Context\" at that point"),
            "{message}"
        );
        assert!(message.contains("invoke_menu"), "{message}");
        let data = refused.structured_content.expect("structured");
        assert_eq!(data["reason"], CONTEXT_MENU_WOULD_RAISE);
        assert!(data.get("current_value").is_none());
    }
}
