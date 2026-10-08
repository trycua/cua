//! `get_window_state.display_only`: a public, side-effect-free capture for
//! live previews (picture-in-picture) that poll a window several times a
//! second. It maps onto each backend's internal observation-only read: no
//! snapshot is published, refreshed or removed, and no action capture is
//! registered, so the agent's element tokens and pixel frame for the window
//! stay valid.
//!
//! Hosts detect it by the property in `get_window_state`'s input schema
//! (T3 Code checks `"display_only" in input_schema.properties`), so the
//! description, refusal and response fields match T3 Code's Linux patch to
//! Cua Driver 0.34.0 word for word (pingdotgg/t3code#16975, MIT,
//! Copyright (c) 2026 T3 Tools Inc.). The typed contract
//! (`cua-driver-contract`) is unchanged.
//!
//! On main this lives in `window_state_view`; the 0.34 line has no such
//! module, so the same items live here.

use crate::protocol::ToolResult;
use serde_json::{json, Value};

const DISPLAY_ONLY_DESCRIPTION: &str = "Default false. With include_accessibility_tree:false, returns pixels for display without registering an action capture or replacing the agent snapshot. The image cannot ground input actions.";

/// Refusal when `display_only` is asked for together with a tree walk.
pub const DISPLAY_ONLY_NEEDS_NO_TREE: &str =
    "display_only requires include_accessibility_tree:false";

/// `frame_note` on a display-only response.
pub const DISPLAY_ONLY_FRAME_NOTE: &str = "Display-only preview; not registered for input actions. Obtain a normal get_window_state screenshot before acting.";

/// The `display_only` input-schema property every backend advertises.
pub fn schema() -> Value {
    json!({
        "type": "boolean",
        "description": DISPLAY_ONLY_DESCRIPTION
    })
}

/// Read `display_only`. A display-only read must skip the tree walk, because
/// a walk is what produces element rows and tokens.
pub fn display_only(args: &Value) -> Result<bool, ToolResult> {
    if args.get("display_only").and_then(Value::as_bool) != Some(true) {
        return Ok(false);
    }
    if args
        .get("include_accessibility_tree")
        .and_then(Value::as_bool)
        != Some(false)
    {
        return Err(ToolResult::error(DISPLAY_ONLY_NEEDS_NO_TREE));
    }
    Ok(true)
}

/// Mark a display-only response.
pub fn mark_display_only(structured: &mut Value) {
    structured["display_only"] = json!(true);
    structured["frame_note"] = json!(DISPLAY_ONLY_FRAME_NOTE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_only_schema_is_a_boolean() {
        assert_eq!(schema()["type"], "boolean");
    }

    #[test]
    fn display_only_requires_a_screenshot_only_read() {
        assert_eq!(display_only(&json!({})).ok(), Some(false));
        assert_eq!(
            display_only(&json!({"display_only": false})).ok(),
            Some(false)
        );
        assert_eq!(
            display_only(&json!({"display_only": true, "include_accessibility_tree": false})).ok(),
            Some(true)
        );
        for args in [
            json!({"display_only": true}),
            json!({"display_only": true, "include_accessibility_tree": true}),
        ] {
            let refusal = display_only(&args).unwrap_err();
            assert_eq!(
                serde_json::to_value(refusal).unwrap(),
                serde_json::to_value(ToolResult::error(DISPLAY_ONLY_NEEDS_NO_TREE)).unwrap()
            );
        }
    }

    #[test]
    fn display_only_response_carries_the_frame_note() {
        let mut structured = json!({"screenshot_width": 800});
        mark_display_only(&mut structured);
        assert_eq!(structured["display_only"], true);
        assert_eq!(structured["frame_note"], DISPLAY_ONLY_FRAME_NOTE);
    }
}
