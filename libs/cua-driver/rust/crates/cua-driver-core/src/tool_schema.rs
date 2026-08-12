//! Shared tool-parameter schema fragments + a cross-platform consistency gate.
//!
//! Every platform crate hand-writes its tool `input_schema` as an inline
//! `json!` block. Before this module those blocks drifted: the same logical
//! param (`session`, `delivery_mode`, `modifier`, …) was defined slightly
//! differently per platform, and `additionalProperties:false` turned that drift
//! into hard client breakage (a client that sends `session` to a Windows tool
//! that never declared it gets *rejected*, not ignored).
//!
//! Two halves:
//!  1. **Fragments** — canonical `serde_json::Value` builders for each shared
//!     param. Platforms compose their schemas from these instead of re-typing.
//!  2. **The gate** — [`shared_schema_violations`] compares a tool's declared
//!     params against the canon (structurally, ignoring prose) plus a per-tool
//!     required-set spec. Each platform calls it from a test against its own
//!     registry, so CI fails the moment a platform drifts.
//!
//! Descriptions are generally not compared because prose can legitimately vary
//! per tool (a `delivery_mode` blurb mentioning "PIXEL click" vs "AX insert").
//! The `session` guidance is the exception: action tools must tell callers that
//! public labels are preferred for multi-call work and must be repeated on each
//! call. Lifecycle-management tools keep their resource-specific wording.

use serde_json::{json, Value};

// ── Fragments ────────────────────────────────────────────────────────────────

/// `session` is an optional public lifecycle label. Uniform across every tool.
pub fn session_schema() -> Value {
    json!({
        "type": "string",
        "description": cua_driver_contract::MULTI_CALL_SESSION_DESCRIPTION
    })
}

/// Canonical lifecycle guidance plus a tool-specific ownership or precedence note.
pub fn session_schema_with(note: &str) -> Value {
    let mut schema = session_schema();
    schema["description"] = Value::String(format!(
        "{} {note}",
        cua_driver_contract::MULTI_CALL_SESSION_DESCRIPTION
    ));
    schema
}

/// `delivery_mode` — the best-effort-background ladder rung. The prose varies by
/// tool (the surface it injects through differs), so callers may pass their own
/// `description`; the shape is fixed here.
pub fn delivery_mode_schema_with(description: &str) -> Value {
    json!({ "type": "string", "enum": ["background", "foreground"], "description": description })
}

/// `delivery_mode` with the generic, tool-agnostic blurb.
pub fn delivery_mode_schema() -> Value {
    delivery_mode_schema_with(
        "Best-effort-background ladder rung (default \"background\"). \
         \"background\": inject without fronting or raising the target — no focus \
         steal. \"foreground\": briefly front the target, act, then restore the \
         prior frontmost — the explicit last resort when a background attempt \
         didn't land. Re-call with \"foreground\" only for the action that needs it.",
    )
}

/// `modifier` — held modifier keys for a pointer action.
pub fn modifier_schema() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": "Modifier keys held during the action: cmd, shift, option/alt, ctrl."
    })
}

/// `button` — which mouse button.
pub fn button_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["left", "right", "middle"],
        "description": "Mouse button. Default \"left\"."
    })
}

/// `scope` — window-local vs desktop-absolute coordinate frame for windowless
/// pointer actions.
pub fn scope_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["window", "desktop"],
        "description": "Coordinate frame (default \"window\"). Pass \"desktop\" \
            with x,y and NO pid/window_id for a windowless screen-absolute action \
            (coordinates read from get_desktop_state). Per-call; not a setting."
    })
}

/// `element_index` — the integer handle from the last get_window_state.
pub fn element_index_schema() -> Value {
    json!({
        "type": "integer",
        "description": "Element index from get_window_state. Cua Driver 0.17 \
            requires the matching `snapshot_id` alongside it. Prefer \
            `element_token`, which carries both values."
    })
}

/// `snapshot_id` — the snapshot handle paired with a numeric element index.
pub fn snapshot_id_schema() -> Value {
    json!({
        "type": "string",
        "pattern": "^s[0-9a-f]{8}$",
        "description": "Snapshot handle from get_window_state. Required when \
            targeting by element_index; stale snapshots fail closed."
    })
}

/// `element_token` — the opaque, validity-checked handle from get_window_state.
pub fn element_token_schema() -> Value {
    json!({
        "type": "string",
        "description": "Opaque per-snapshot element handle from \
            `structuredContent.elements[].element_token`. If element_index, \
            snapshot_id, or window_id are also supplied they must agree. Returns \
            an explicit stale error once a newer snapshot supersedes it."
    })
}

// ── The gate ─────────────────────────────────────────────────────────────────

/// The canonical *shape* of each shared param (description stripped). `None` for
/// any param name that is not part of the shared, cross-platform contract —
/// genuinely platform-specific params (`bundle_id`, `aumid`, …) are simply not
/// listed and the gate ignores them.
fn shared_param_canonical(name: &str) -> Option<Value> {
    let v = match name {
        "session" => session_schema(),
        "delivery_mode" => delivery_mode_schema(),
        "modifier" => modifier_schema(),
        "button" => button_schema(),
        "scope" => scope_schema(),
        "element_index" => element_index_schema(),
        "element_token" => element_token_schema(),
        "snapshot_id" => snapshot_id_schema(),
        "capture_mode" => crate::capture_mode::capture_mode_schema(),
        _ => return None,
    };
    Some(structural(&v))
}

/// The canonical `required` array for tools whose required-set drifted across
/// platforms. `None` = not pinned (the gate ignores that tool's required list).
/// Conditionally-required params (e.g. `pid`, required at runtime unless
/// `scope:"desktop"`) are intentionally NOT in `required` — the schema can't
/// express the condition, so it's validated in code with a clear error.
fn required_canonical(tool: &str) -> Option<&'static [&'static str]> {
    Some(match tool {
        // `pid` is conditionally required (validated in code: needed unless a
        // windowless desktop-scope call), so it is intentionally NOT pinned here.
        "click" => &[],
        "scroll" => &["direction"],
        "zoom" => &["window_id", "x1", "y1", "x2", "y2"],
        // double_click / right_click are already consistent across platforms —
        // not pinned, so we don't force a behavior change on them.
        _ => return None,
    })
}

/// Reduce a param schema to the parts that govern client compatibility —
/// `type`, `enum`, and (recursively) `items` — dropping `description` and any
/// other prose so per-tool wording differences don't trip the gate.
fn structural(schema: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(t) = schema.get("type") {
        out.insert("type".into(), t.clone());
    }
    if let Some(e) = schema.get("enum") {
        out.insert("enum".into(), e.clone());
    }
    if let Some(items) = schema.get("items") {
        out.insert("items".into(), structural(items));
    }
    Value::Object(out)
}

fn session_guidance_is_exempt(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "start_session"
            | "end_session"
            | "get_session"
            | "get_session_state"
            | "list_sessions"
            | "escalate_session"
            | "set_agent_cursor_enabled"
            | "set_agent_cursor_motion"
            | "set_agent_cursor_theme"
            | "get_agent_cursor_state"
    )
}

fn session_description_has_multi_call_guidance(schema: &Value) -> bool {
    schema
        .get("description")
        .and_then(Value::as_str)
        .map(|description| description.split_whitespace().collect::<Vec<_>>().join(" "))
        .is_some_and(|description| {
            description.contains("prefer a short public session label")
                && description.contains("repeat it on every call that accepts it")
        })
}

/// Check one tool's `input_schema` against the shared canon. Returns a list of
/// human-readable violation strings (empty == consistent). Each platform calls
/// this for every tool in its registry from a test.
pub fn shared_schema_violations(tool_name: &str, input_schema: &Value) -> Vec<String> {
    let mut violations = Vec::new();

    // 1. Every declared param that IS a shared param must match the canon shape.
    if let Some(props) = input_schema.get("properties").and_then(|p| p.as_object()) {
        for (pname, pschema) in props {
            if let Some(canon) = shared_param_canonical(pname) {
                let got = structural(pschema);
                if got != canon {
                    violations.push(format!(
                        "{tool_name}.{pname}: shape {got} diverges from shared canon {canon}"
                    ));
                }
            }
            if pname == "session"
                && !session_guidance_is_exempt(tool_name)
                && !session_description_has_multi_call_guidance(pschema)
            {
                violations.push(format!(
                    "{tool_name}.session: description must prefer a short public session label \
                     for multi-call work and tell callers to repeat it on every accepting call"
                ));
            }
        }
    }

    // 2. Pinned tools must declare exactly the canonical required-set.
    if let Some(want) = required_canonical(tool_name) {
        let got: Vec<String> = input_schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let mut got_sorted = got.clone();
        got_sorted.sort();
        let mut want_sorted: Vec<String> = want.iter().map(|s| (*s).to_owned()).collect();
        want_sorted.sort();
        if got_sorted != want_sorted {
            violations.push(format!(
                "{tool_name}.required: {got_sorted:?} diverges from canon {want_sorted:?}"
            ));
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_strips_description_keeps_type_and_enum() {
        let with_prose = json!({ "type": "string", "enum": ["a", "b"], "description": "x" });
        let s = structural(&with_prose);
        assert_eq!(s, json!({ "type": "string", "enum": ["a", "b"] }));
    }

    #[test]
    fn session_description_prefers_named_multi_call_runs() {
        let schema = session_schema();
        let description = schema["description"]
            .as_str()
            .expect("session schema should carry agent guidance");
        assert!(description.contains("prefer a short public session label"));
        assert!(description.contains("repeat it on every call that accepts it"));
        assert!(description.contains("implicit lifecycle session"));
    }

    #[test]
    fn action_session_description_without_repeat_guidance_is_flagged() {
        let tool = json!({
            "type": "object",
            "properties": {
                "session": { "type": "string", "description": "Optional session id." }
            }
        });
        let violations = shared_schema_violations("clipboard_write", &tool);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("repeat it on every accepting call"));
    }

    #[test]
    fn lifecycle_session_tools_keep_resource_specific_descriptions() {
        let tool = json!({
            "type": "object",
            "properties": {
                "session": { "type": "string", "description": "Public label to end." }
            }
        });
        assert!(shared_schema_violations("end_session", &tool).is_empty());
    }

    #[test]
    fn matching_param_with_different_prose_passes() {
        // Same shape, different description → no violation.
        let tool = json!({
            "type": "object",
            "properties": {
                "delivery_mode": delivery_mode_schema_with("a totally different blurb")
            }
        });
        assert!(shared_schema_violations("click", &tool).is_empty());
    }

    #[test]
    fn wrong_enum_is_flagged() {
        // The exact capture_mode regression we just fixed: a stale 3-mode enum.
        let tool = json!({
            "type": "object",
            "properties": {
                "capture_mode": { "type": "string", "enum": ["som", "vision", "ax"] }
            }
        });
        let v = shared_schema_violations("get_window_state", &tool);
        assert_eq!(v.len(), 1, "stale capture_mode enum must be flagged: {v:?}");
    }

    #[test]
    fn non_shared_param_is_ignored() {
        let tool = json!({ "type": "object", "properties": { "bundle_id": { "type": "string" } } });
        assert!(shared_schema_violations("launch_app", &tool).is_empty());
    }

    #[test]
    fn required_set_drift_is_flagged() {
        // click pinned to [] — a stray required `pid` must trip the gate.
        let tool = json!({ "type": "object", "required": ["pid"], "properties": {} });
        let v = shared_schema_violations("click", &tool);
        assert!(v.iter().any(|s| s.contains("required")), "got {v:?}");
    }

    #[test]
    fn canonical_click_required_passes() {
        let tool = json!({ "type": "object", "required": [], "properties": {} });
        assert!(shared_schema_violations("click", &tool).is_empty());
    }
}
