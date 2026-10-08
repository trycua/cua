//! `display_only` on `get_window_state`: pixels for a human-facing preview.
//!
//! A host that shows the agent's window to the user (a picture-in-picture, a
//! live preview) polls `get_window_state` on the same daemon the agent uses.
//! A normal capture registers an action frame and replaces the session's
//! snapshot, so each preview frame would invalidate the agent's element tokens
//! and pixel frames. `display_only:true` takes the same path as the internal
//! `_observation_only` mode instead: no capture binding, no snapshot change.
//!
//! The flag requires `include_accessibility_tree:false`. A host feature-detects
//! it from the tool's input schema, so the schema property is the capability
//! advertisement on every platform.
//!
//! The shape matches T3 Code's Linux patch to Cua Driver 0.34.0
//! (pingdotgg/t3code#16975, MIT, Copyright (c) 2026 T3 Tools Inc.), which
//! T3 Code already feature-detects.

use serde_json::{json, Value};

/// The argument name.
pub const ARG: &str = "display_only";

/// The error for `display_only:true` without `include_accessibility_tree:false`.
pub const REQUIRES_NO_TREE: &str = "display_only requires include_accessibility_tree:false";

/// The `frame_note` a display-only result carries in place of the usual
/// coordinate-frame note.
pub const FRAME_NOTE: &str = "Display-only preview; not registered for input actions. \
    Obtain a normal get_window_state screenshot before acting.";

/// The `display_only` JSON-schema fragment, shared by every platform's
/// `get_window_state` so the capability advertises an identical shape.
pub fn schema() -> Value {
    json!({
        "type": "boolean",
        "description": "Default false. With include_accessibility_tree:false, returns \
            pixels for display without registering an action capture or replacing \
            the agent snapshot. The image cannot ground input actions."
    })
}

/// Whether the call asked for a display-only capture.
///
/// Errs when `display_only:true` comes without an explicit
/// `include_accessibility_tree:false`: a display-only call never walks or
/// publishes an element tree.
pub fn requested(args: &Value) -> Result<bool, &'static str> {
    if args.get(ARG).and_then(Value::as_bool) != Some(true) {
        return Ok(false);
    }
    if args
        .get("include_accessibility_tree")
        .and_then(Value::as_bool)
        != Some(false)
    {
        return Err(REQUIRES_NO_TREE);
    }
    Ok(true)
}

/// Mark a result as display-only.
pub fn annotate(structured: &mut Value) {
    structured["display_only"] = json!(true);
    structured["frame_note"] = json!(FRAME_NOTE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_or_false_is_not_requested() {
        assert_eq!(requested(&json!({"pid": 1})), Ok(false));
        assert_eq!(
            requested(&json!({"pid": 1, "display_only": false})),
            Ok(false)
        );
        // A tree walk without display_only is the normal path.
        assert_eq!(
            requested(&json!({"pid": 1, "include_accessibility_tree": true})),
            Ok(false)
        );
    }

    #[test]
    fn requires_an_explicit_tree_opt_out() {
        for args in [
            json!({"pid": 1, "display_only": true}),
            json!({"pid": 1, "display_only": true, "include_accessibility_tree": true}),
        ] {
            assert_eq!(requested(&args), Err(REQUIRES_NO_TREE));
        }
        assert_eq!(
            requested(&json!({
                "pid": 1,
                "display_only": true,
                "include_accessibility_tree": false
            })),
            Ok(true)
        );
    }

    #[test]
    fn annotation_replaces_the_action_frame_note() {
        let mut structured = json!({"frame_note": "x/y are pixels of THIS screenshot"});
        annotate(&mut structured);
        assert_eq!(structured["display_only"], json!(true));
        assert_eq!(structured["frame_note"], json!(FRAME_NOTE));
    }

    #[test]
    fn schema_is_a_boolean_that_says_it_cannot_ground_actions() {
        let schema = schema();
        assert_eq!(schema["type"], "boolean");
        assert!(schema["description"]
            .as_str()
            .unwrap()
            .contains("cannot ground input actions"));
    }
}
