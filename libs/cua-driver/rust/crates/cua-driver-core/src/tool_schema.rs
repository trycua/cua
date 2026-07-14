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
//! Descriptions are intentionally NOT compared — prose can legitimately vary
//! per tool (a `delivery_mode` blurb mentioning "PIXEL click" vs "AX insert").
//! The gate enforces *shape* (`type` / `enum` / `items`), which is what governs
//! client compatibility.

use serde_json::{json, Value};

// ── Fragments ────────────────────────────────────────────────────────────────

/// `session` — the per-run agent-cursor identity. Uniform across every tool.
pub fn session_schema() -> Value {
    json!({
        "type": "string",
        "description": "Optional session id: declares/uses the agent cursor and \
            per-session state for this run. The same id works over MCP, the CLI, \
            or the raw socket, and follows the run across apps/windows. Omit to \
            run cursor-less."
    })
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
        "description": "Element index from the last get_window_state. REQUIRES \
            `pid` and `window_id` alongside it — element_index alone (no pid) \
            fails fast, it is not a silent no-op."
    })
}

/// `element_token` — the opaque, validity-checked handle from get_window_state.
pub fn element_token_schema() -> Value {
    json!({
        "type": "string",
        "description": "Opaque per-snapshot element handle from \
            `structuredContent.elements[].element_token`. On macOS click/set_value \
            it is strictly bound to pid, window_id, generation, element_index, and \
            AX node identity. If element_index or window_id are also supplied they \
            must match. Unknown, cross-target, stale-generation, or identity-mismatch \
            tokens fail closed; call get_window_state again after a stale error."
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
