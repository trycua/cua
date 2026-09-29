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
        // Legacy `scope:"desktop"` together with a pid/window_id means
        // "desktop-frame (get_desktop_state) coordinates against this named
        // window". It is rewritten to `coordinate_frame:"desktop"` where the
        // adapter translates the point (Linux) and refused elsewhere.
        if object.get("scope").and_then(Value::as_str) == Some("desktop")
            && (object.contains_key("pid") || object.contains_key("window_id"))
        {
            return normalize_desktop_frame_for_window(object);
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

/// `scope: "desktop"` together with `pid` / `window_id` means "these x/y are
/// desktop (full-screen) pixels — the ones `get_desktop_state` returns — but
/// deliver to THIS window". Agents that ground on a desktop screenshot and
/// then target the window that owns the pixel need exactly this; refusing it
/// made every such click fail. The Linux adapter translates the point into
/// the window's local frame (`coordinate_frame: "desktop"`); backends that do
/// not implement the translation keep the explicit refusal rather than
/// silently reading desktop pixels as window-local ones.
fn normalize_desktop_frame_for_window(
    object: &mut serde_json::Map<String, Value>,
) -> Result<(), ToolResult> {
    if !desktop_frame_for_window_supported() {
        return Err(invalid_target(
            "desktop scope cannot be combined with pid or window_id",
        ));
    }
    object.remove("scope");
    object.insert("coordinate_frame".into(), Value::String("desktop".into()));
    Ok(())
}

/// Which backends translate desktop-frame pixels for a window target.
pub fn desktop_frame_for_window_supported() -> bool {
    cfg!(target_os = "linux")
}

/// Reject the delivery combination that cannot preserve background posture.
/// Call this only after delivery aliases and typed targets have been normalized.
pub fn enforce_delivery_target(tool_name: &str, args: &Value) -> Result<(), ToolResult> {
    if tool_name != "click"
        || args.get("scope").and_then(Value::as_str) != Some("desktop")
        || args.get("delivery_mode").and_then(Value::as_str) != Some("background")
    {
        return Ok(());
    }

    let message = cua_driver_contract::ClickInput::DESKTOP_BACKGROUND_MESSAGE;
    Err(crate::delivery::background_unavailable_result(
        message,
        "background_unavailable",
        message,
        json!({ "effect": "refused" }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_scope_with_pid_becomes_a_desktop_coordinate_frame_where_supported() {
        let mut args = json!({"pid": 7, "x": 100, "y": 200, "scope": "desktop"});
        let result = normalize_action_target("click", &mut args);
        if desktop_frame_for_window_supported() {
            result.unwrap();
            assert!(args.get("scope").is_none());
            assert_eq!(args["coordinate_frame"], "desktop");
            assert_eq!(args["pid"], 7);
        } else {
            assert!(result.is_err());
        }
        // Windowless desktop scope is untouched.
        let mut bare = json!({"x": 1, "y": 2, "scope": "desktop"});
        normalize_action_target("click", &mut bare).unwrap();
        assert_eq!(bare["scope"], "desktop");
        assert!(bare.get("coordinate_frame").is_none());
    }

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
            json!({"target": {"kind": "desktop", "display_id": "secondary"}}),
            json!({"target": {"kind": "window", "pid": 7, "window_id": 0}}),
        ] {
            assert!(normalize_action_target("click", &mut args).is_err());
        }
        // desktop scope with a pid is a desktop-frame window target where the
        // backend translates it, and a refusal everywhere else.
        let mut args = json!({"scope": "desktop", "pid": 7});
        assert_eq!(
            normalize_action_target("click", &mut args).is_ok(),
            desktop_frame_for_window_supported()
        );
    }

    #[test]
    fn desktop_scope_with_pid_is_platform_gated() {
        let mut args = json!({"scope": "desktop", "pid": 7, "x": 1, "y": 2});
        let result = normalize_action_target("click", &mut args);
        if cfg!(target_os = "linux") {
            assert!(
                result.is_ok(),
                "Linux maps desktop-frame points into the window"
            );
            assert!(args.get("scope").is_none());
            assert_eq!(args["coordinate_frame"], "desktop");
            assert_eq!(args["pid"], 7);
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn desktop_background_guard_is_click_specific() {
        let desktop_background = json!({
            "scope": "desktop",
            "delivery_mode": "background"
        });
        let refusal = enforce_delivery_target("click", &desktop_background).unwrap_err();
        assert_eq!(refusal.is_error, Some(true));
        assert_eq!(
            refusal.structured_content,
            Some(json!({
                "code": "background_unavailable",
                "effect": "refused",
                "suggestion": "Retry this action with delivery_mode:\"foreground\".",
                "escalation": {
                    "recommended": "foreground",
                    "reason": cua_driver_contract::ClickInput::DESKTOP_BACKGROUND_MESSAGE,
                },
            }))
        );

        for (tool, args) in [
            (
                "click",
                json!({"scope": "desktop", "delivery_mode": "foreground"}),
            ),
            (
                "click",
                json!({"scope": "window", "delivery_mode": "background"}),
            ),
            ("drag", desktop_background),
        ] {
            assert!(
                enforce_delivery_target(tool, &args).is_ok(),
                "{tool}: {args}"
            );
        }
    }
}
