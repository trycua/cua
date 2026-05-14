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
