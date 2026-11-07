// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! JSON shapes for the fast path: the config key, refusal payloads, and the
//! check summary attached to action results. Kept apart from the checks so
//! those stay std-only.

use super::classify::TargetClass;
use super::pixel::GrayImage;
use super::reason::{Reason, Verdict};
use super::{mode, set_mode, FastPathMode};
use crate::protocol::ToolResult;
use serde_json::{json, Value};

pub const CONFIG_KEY: &str = "fast_path";

/// The `set_config` schema property.
pub fn config_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["off", "shadow", "guard", "on"],
        "description": "Experimental fast path (default off). shadow: check cached \
            targets at act time and report would-refuse verdicts; guard: refuse an \
            action whose target changed since it was read (target_changed); on: guard, \
            plus a small delta in each action result and tree-only re-reads. Runtime \
            only, not saved; env CUA_DRIVER_FAST_PATH seeds it at start."
    })
}

/// Add the `fast_path` property to a `set_config` schema.
pub fn with_config_property(mut schema: Value) -> Value {
    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert(CONFIG_KEY.into(), config_schema());
    }
    schema
}

/// Apply `fast_path` from `set_config` args. `Ok(None)` when absent.
pub fn apply_config_args(args: &Value) -> Result<Option<FastPathMode>, String> {
    let Some(raw) = args.get(CONFIG_KEY) else {
        return Ok(None);
    };
    let parsed = match raw {
        Value::String(s) => FastPathMode::parse(s),
        Value::Bool(true) => Some(FastPathMode::On),
        Value::Bool(false) => Some(FastPathMode::Off),
        _ => None,
    };
    let Some(m) = parsed else {
        return Err(format!(
            "fast_path must be one of off, shadow, guard, on (got {raw})"
        ));
    };
    set_mode(m);
    Ok(Some(m))
}

/// The `get_config` field.
pub fn config_value() -> Value {
    Value::String(mode().as_str().into())
}

/// The summary attached to an action that passed (or, in shadow mode,
/// would have failed) its checks.
pub fn check_summary(
    verdict: &Verdict,
    class: Option<TargetClass>,
    verified: &str,
    check_ms: f64,
) -> Value {
    let mut out = json!({
        "mode": mode().as_str(),
        "verified": verified,
        "check_ms": (check_ms * 10.0).round() / 10.0,
    });
    if let Some(class) = class {
        out["target_class"] = json!(class.as_str());
    }
    match verdict {
        Verdict::Ok => {}
        Verdict::Relocated { dx, dy } => {
            out["relocated"] = json!(true);
            out["offset"] = json!([dx, dy]);
        }
        Verdict::Refuse { reason, detail } => {
            out["would_refuse"] = json!(reason.as_str());
            out["detail"] = json!(detail);
        }
    }
    out
}

/// A refusal: `target_changed` (or `window_changed`) with the reason, the
/// detail and a pointer to the fresh state.
pub fn refusal(
    tool: &str,
    reason: Reason,
    detail: &str,
    class: Option<TargetClass>,
    pid: i32,
    window_id: Option<u64>,
    check_ms: f64,
) -> ToolResult {
    let mut next = json!({ "tool": "get_window_state", "args": { "pid": pid } });
    if let Some(w) = window_id {
        next["args"]["window_id"] = json!(w);
    }
    if matches!(
        reason,
        Reason::RegionChanged | Reason::WindowChanged | Reason::Occluded | Reason::Animating
    ) {
        next["args"]["include_screenshot"] = json!(true);
    }
    let mut structured = json!({
        "code": reason.code(),
        "reason": reason.as_str(),
        "detail": detail,
        "effect": "refused",
        "fast_path": {
            "mode": mode().as_str(),
            "check_ms": (check_ms * 10.0).round() / 10.0,
        },
        "next_call": next,
    });
    if let Some(class) = class {
        structured["fast_path"]["target_class"] = json!(class.as_str());
    }
    ToolResult::error(format!(
        "{tool}: {} ({}: {detail}). Nothing was sent.",
        reason.message(),
        reason.as_str()
    ))
    .with_structured(structured)
}

/// Merge `extra` into the result's structured content under `fast_path`.
pub fn annotate(result: &mut ToolResult, summary: Value) {
    match result.structured_content.as_mut() {
        Some(Value::Object(map)) => {
            map.insert("fast_path".into(), summary);
        }
        _ => {
            result.structured_content = Some(json!({ "fast_path": summary }));
        }
    }
}

/// Greyscale copy of a capture, for the read-time image and act-time crops.
pub fn gray_from_captured(img: &crate::image_utils::CapturedImage) -> Option<GrayImage> {
    use crate::image_utils::CapturedImage;
    match img {
        CapturedImage::Rgba(f) => {
            GrayImage::from_rgba(f.width as usize, f.height as usize, &f.rgba)
        }
        CapturedImage::Png(bytes) => {
            let decoded = image::load_from_memory(bytes).ok()?.to_luma8();
            let (w, h) = decoded.dimensions();
            GrayImage::new(w as usize, h as usize, decoded.into_raw())
        }
    }
}

/// Design (e): with the mode `on`, a re-read (`since` given) that did not
/// ask for a screenshot is tree-only, unless the window's tree is sparse
/// (under 10 elements or mostly 1-pt frames), where the picture is the
/// only reliable state.
pub fn skip_reread_screenshot(args: &Value, pid: i32, window_id: u64) -> bool {
    if !mode().deltas()
        || args.get("include_screenshot").is_some()
        || args.get("screenshot_out_file").is_some()
        || args.get("since").is_none_or(Value::is_null)
    {
        return false;
    }
    match super::store::global().latest_stats(pid, window_id) {
        Some((elements, thin)) => elements >= 10 && thin * 2 <= elements,
        None => false,
    }
}

/// Tools whose direct (unbatched) result gets a delta when the mode is `on`.
pub fn returns_delta(tool: &str) -> bool {
    matches!(
        tool,
        "click"
            | "double_click"
            | "right_click"
            | "type_text"
            | "press_key"
            | "hotkey"
            | "set_value"
            | "scroll"
            | "drag"
    )
}

/// The window a direct action's delta is read from, when the mode is `on`.
/// From `pid`/`window_id`, or from the snapshot an `element_token` names.
pub fn delta_target(args: &Value) -> Option<(i32, u64)> {
    if !mode().deltas() {
        return None;
    }
    let pid = args.get("pid").and_then(Value::as_i64).map(|p| p as i32);
    let window = args.get("window_id").and_then(Value::as_u64);
    if let (Some(pid), Some(window)) = (pid, window) {
        return Some((pid, window));
    }
    let (snapshot, index) = args
        .get("element_token")
        .and_then(Value::as_str)
        .and_then(crate::element_token::parse_token)?;
    super::store::global()
        .element(snapshot, index)
        .map(|(pid, window, _)| (pid, window))
}

/// Append a delta read to an action result: its text after the action's,
/// and its structured content under `fast_path.delta`.
pub fn attach_delta(result: &mut ToolResult, delta: ToolResult) {
    if delta.is_error == Some(true) {
        annotate_delta(result, json!({ "error": true }));
        return;
    }
    let structured = delta.structured_content.clone().unwrap_or(Value::Null);
    let summary = json!({
        "snapshot_id": structured.get("snapshot_id").cloned().unwrap_or(Value::Null),
        "since_status": structured.get("since_status").cloned().unwrap_or(Value::Null),
    });
    result.content.extend(delta.content);
    annotate_delta(result, summary);
}

fn annotate_delta(result: &mut ToolResult, delta: Value) {
    let map = match result.structured_content.as_mut() {
        Some(Value::Object(map)) => map,
        _ => {
            result.structured_content = Some(json!({}));
            result
                .structured_content
                .as_mut()
                .unwrap()
                .as_object_mut()
                .unwrap()
        }
    };
    let entry = map
        .entry("fast_path")
        .or_insert_with(|| json!({ "mode": mode().as_str() }));
    if !entry.is_object() {
        *entry = json!({ "mode": mode().as_str() });
    }
    entry["delta"] = delta;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip_and_rejects_junk() {
        let _guard = crate::fast_path::tests_lock();
        assert_eq!(apply_config_args(&json!({})).unwrap(), None);
        assert_eq!(
            apply_config_args(&json!({"fast_path": "guard"})).unwrap(),
            Some(FastPathMode::Guard)
        );
        assert_eq!(config_value(), json!("guard"));
        assert!(apply_config_args(&json!({"fast_path": 3})).is_err());
        apply_config_args(&json!({"fast_path": "off"})).unwrap();
        assert_eq!(mode(), FastPathMode::Off);
    }

    #[test]
    fn refusal_payload_names_reason_and_fresh_read() {
        let r = refusal(
            "click",
            Reason::Moved,
            "moved 40 pt",
            None,
            7,
            Some(3),
            1.23,
        );
        assert_eq!(r.is_error, Some(true));
        let s = r.structured_content.unwrap();
        assert_eq!(s["code"], "target_changed");
        assert_eq!(s["reason"], "moved");
        assert_eq!(s["next_call"]["args"]["window_id"], 3);
        let r = refusal(
            "click",
            Reason::RegionChanged,
            "x",
            Some(TargetClass::Sparse),
            7,
            Some(3),
            0.0,
        );
        let s = r.structured_content.unwrap();
        assert_eq!(s["next_call"]["args"]["include_screenshot"], true);
        assert_eq!(s["fast_path"]["target_class"], "sparse");
    }

    #[test]
    fn delta_is_attached_under_fast_path() {
        let mut result = ToolResult::text("clicked").with_structured(json!({"path": "ax"}));
        let delta = ToolResult::text("~ [3] AXButton \"Save\"")
            .with_structured(json!({"snapshot_id": "s00000002", "since_status": "delta"}));
        attach_delta(&mut result, delta);
        let s = result.structured_content.unwrap();
        assert_eq!(s["path"], "ax");
        assert_eq!(s["fast_path"]["delta"]["snapshot_id"], "s00000002");
        assert_eq!(result.content.len(), 2);
    }

    #[test]
    fn summary_reports_relocation_and_shadow_verdicts() {
        let s = check_summary(
            &Verdict::Relocated { dx: 4.0, dy: 0.0 },
            Some(TargetClass::Rich),
            "ax",
            2.0,
        );
        assert_eq!(s["relocated"], true);
        let s = check_summary(&Verdict::refuse(Reason::Gone, "gone"), None, "ax", 2.0);
        assert_eq!(s["would_refuse"], "gone");
    }
}
