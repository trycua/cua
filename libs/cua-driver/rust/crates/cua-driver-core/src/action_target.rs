//! Per-call target normalization for lifecycle-independent capture modality.

use crate::protocol::ToolResult;
use serde_json::{json, Value};

const TARGETED_TOOLS: &[&str] = &[
    "move_cursor",
    "click",
    "drag",
    "scroll",
    "type_text",
    "press_key",
    "hotkey",
];

pub fn supports_typed_target(tool_name: &str) -> bool {
    TARGETED_TOOLS.contains(&tool_name)
}

fn invalid_target(message: impl Into<String>) -> ToolResult {
    ToolResult::error(message.into()).with_structured(json!({
        "code": "invalid_action_target",
    }))
}

/// Normalize the generated tagged union into the legacy flat fields consumed
/// by the current thin platform adapters. This runs once at the canonical
/// dispatch boundary before authorization, so policy/resource checks and the
/// platform worker see the same exact target.
pub fn normalize_action_target(tool_name: &str, args: &mut Value) -> Result<(), ToolResult> {
    let Some(object) = args.as_object_mut() else {
        return Ok(());
    };
    let Some(target) = object.remove("target") else {
        if object.get("scope").and_then(Value::as_str) == Some("desktop")
            && (object.contains_key("pid") || object.contains_key("window_id"))
        {
            return Err(invalid_target(
                "desktop scope cannot be combined with pid or window_id",
            ));
        }
        return Ok(());
    };
    if !supports_typed_target(tool_name) {
        return Err(invalid_target(format!(
            "{tool_name} does not accept a per-call target"
        )));
    }
    if object.contains_key("scope")
        || object.contains_key("pid")
        || object.contains_key("window_id")
    {
        return Err(invalid_target(
            "target cannot be combined with legacy scope, pid, or window_id fields",
        ));
    }
    let Some(target) = target.as_object() else {
        return Err(invalid_target("target must be an object"));
    };
    match target.get("kind").and_then(Value::as_str) {
        Some("window") => {
            let pid = target
                .get("pid")
                .and_then(Value::as_u64)
                .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
                .ok_or_else(|| invalid_target("window target requires a positive 32-bit pid"))?;
            let window_id = target
                .get("window_id")
                .and_then(Value::as_u64)
                .filter(|window_id| *window_id > 0)
                .ok_or_else(|| invalid_target("window target requires a positive window_id"))?;
            if target.len() != 3 {
                return Err(invalid_target(
                    "window target accepts only kind, pid, and window_id",
                ));
            }
            object.insert("scope".into(), Value::String("window".into()));
            object.insert("pid".into(), Value::Number(pid.into()));
            object.insert("window_id".into(), Value::Number(window_id.into()));
        }
        Some("desktop") => {
            let display_id = target
                .get("display_id")
                .and_then(Value::as_str)
                .filter(|display_id| !display_id.is_empty())
                .ok_or_else(|| invalid_target("desktop target requires display_id"))?;
            if target.len() != 2 {
                return Err(invalid_target(
                    "desktop target accepts only kind and display_id",
                ));
            }
            if display_id != "primary" {
                return Err(invalid_target(
                    "this release supports only display_id='primary'",
                ));
            }
            object.insert("scope".into(), Value::String("desktop".into()));
        }
        Some(_) => return Err(invalid_target("target.kind must be window or desktop")),
        None => return Err(invalid_target("target.kind is required")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_targets_normalize_to_one_unambiguous_legacy_shape() {
        let mut window = json!({
            "x": 1,
            "y": 2,
            "target": {"kind": "window", "pid": 7, "window_id": 9}
        });
        normalize_action_target("click", &mut window).unwrap();
        assert_eq!(window["scope"], "window");
        assert_eq!(window["pid"], 7);
        assert_eq!(window["window_id"], 9);
        assert!(window.get("target").is_none());

        let mut desktop = json!({
            "x": 1,
            "y": 2,
            "target": {"kind": "desktop", "display_id": "primary"}
        });
        normalize_action_target("click", &mut desktop).unwrap();
        assert_eq!(desktop["scope"], "desktop");
        assert!(desktop.get("pid").is_none());
    }

    #[test]
    fn ambiguous_or_unsupported_targets_fail_closed() {
        for mut args in [
            json!({
                "scope": "desktop",
                "target": {"kind": "desktop", "display_id": "primary"}
            }),
            json!({"scope": "desktop", "pid": 7}),
            json!({"target": {"kind": "desktop", "display_id": "secondary"}}),
            json!({"target": {"kind": "window", "pid": 7, "window_id": 0}}),
        ] {
            assert!(normalize_action_target("click", &mut args).is_err());
        }
    }
}
