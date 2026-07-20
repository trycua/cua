//! AX action dispatch — the preferred click/interaction path for indexed elements.

use crate::ax::bindings::*;

/// Perform an AX action on a cached element.
pub fn perform_ax_action(element_ptr: usize, action: &str) -> anyhow::Result<()> {
    let ax_action = map_action(action);
    let err = unsafe { perform_action(element_ptr as AXUIElementRef, ax_action) };

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
    let err = unsafe { set_bool_attr_true(element_ptr as AXUIElementRef, "AXFocused") };
    focus_result(err, false)
}

pub fn focus_element_strict(element_ptr: usize) -> anyhow::Result<()> {
    let err = unsafe { set_bool_attr_true(element_ptr as AXUIElementRef, "AXFocused") };
    focus_result(err, true)
}

fn focus_result(err: i32, strict: bool) -> anyhow::Result<()> {
    if err == kAXErrorSuccess {
        Ok(())
    } else if strict {
        anyhow::bail!("AXSetAttribute(AXFocused) failed with error {err}")
    } else {
        // Focus errors are often benign for legacy index targeting.
        tracing::warn!("AXSetAttribute(AXFocused) returned {err}");
        Ok(())
    }
}

/// Set the AXValue of an element (for dropdowns, text fields, etc.).
pub fn set_ax_value(element_ptr: usize, value: &str) -> anyhow::Result<()> {
    let err = unsafe { set_string_attr(element_ptr as AXUIElementRef, "AXValue", value) };
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::focus_result;

    #[test]
    fn token_focus_miss_fails_closed_while_legacy_focus_remains_best_effort() {
        let focus_miss = -25205;
        assert!(focus_result(focus_miss, true).is_err());
        assert!(focus_result(focus_miss, false).is_ok());
    }
}
