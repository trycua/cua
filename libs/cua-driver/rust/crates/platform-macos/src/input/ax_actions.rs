//! AX action dispatch — the preferred click/interaction path for indexed elements.

use crate::ax::bindings::*;

/// Perform an AX action on a cached element.
pub fn perform_ax_action(element_ptr: usize, action: &str) -> anyhow::Result<()> {
    let ax_action = map_action(action);
    let err = unsafe {
        perform_action(element_ptr as AXUIElementRef, ax_action)
    };

    if err == kAXErrorSuccess {
        Ok(())
    } else {
        anyhow::bail!("AXUIElementPerformAction({action}) failed with error {err}")
    }
}

/// Perform a native AX scroll action when the target element advertises one.
pub fn perform_ax_scroll_action_if_supported(
    element_ptr: usize,
    direction: &str,
    by: &str,
    amount: usize,
) -> anyhow::Result<Option<&'static str>> {
    let actions = unsafe { copy_action_names(element_ptr as AXUIElementRef) };
    let Some(candidates) = scroll_action_candidates(direction, by) else {
        return Ok(None);
    };
    let Some(action) = candidates
        .iter()
        .copied()
        .find(|candidate| actions.iter().any(|advertised| advertised.as_str() == *candidate))
    else {
        return Ok(None);
    };

    for _ in 0..amount {
        let err = unsafe { perform_action(element_ptr as AXUIElementRef, action) };
        if err != kAXErrorSuccess {
            anyhow::bail!("AXUIElementPerformAction({action}) failed with error {err}");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    Ok(Some(action))
}

fn map_action(action: &str) -> &'static str {
    match action.to_lowercase().as_str() {
        "press" | "click" => "AXPress",
        "show_menu" | "right_click" | "rightclick" => "AXShowMenu",
        "pick" => "AXPick",
        "confirm" => "AXConfirm",
        "cancel" => "AXCancel",
        "open" => "AXOpen",
        _ => "AXPress",
    }
}

fn scroll_action_candidates(direction: &str, by: &str) -> Option<[&'static str; 2]> {
    Some(match (direction, by) {
        ("up", "page") => ["AXScrollUpByPage", "AXScrollUp"],
        ("down", "page") => ["AXScrollDownByPage", "AXScrollDown"],
        ("left", "page") => ["AXScrollLeftByPage", "AXScrollLeft"],
        ("right", "page") => ["AXScrollRightByPage", "AXScrollRight"],
        ("up", _) => ["AXScrollUp", "AXScrollUpByPage"],
        ("down", _) => ["AXScrollDown", "AXScrollDownByPage"],
        ("left", _) => ["AXScrollLeft", "AXScrollLeftByPage"],
        ("right", _) => ["AXScrollRight", "AXScrollRightByPage"],
        _ => return None,
    })
}

/// Set AXFocused=true on an element (for pre-focusing before key press).
pub fn focus_element(element_ptr: usize) -> anyhow::Result<()> {
    let err = unsafe {
        set_bool_attr_true(element_ptr as AXUIElementRef, "AXFocused")
    };
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        // Focus errors are often benign (element doesn't support focus).
        tracing::warn!("AXSetAttribute(AXFocused) returned {err}");
        Ok(())
    }
}

/// Set the AXValue of an element (for dropdowns, text fields, etc.).
pub fn set_ax_value(element_ptr: usize, value: &str) -> anyhow::Result<()> {
    let err = unsafe {
        set_string_attr(element_ptr as AXUIElementRef, "AXValue", value)
    };
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
    }
}
