//! `capture_mode` — **deprecated and ignored.**
//!
//! `get_window_state` is now perception-mode-agnostic: it always returns BOTH
//! the element tree AND a screenshot, so a vision-capable agent grounds on both
//! and cross-checks one against the other (the element tree lies often enough —
//! Electron echo-confirms, Catalyst null values, virtualized off-viewport
//! `h:1` frames — that a grounding screenshot should always be present).
//!
//! The modality choice is no longer made at *capture* time. It is expressed at
//! *action* time by how you address the target: an **element ax action**
//! (`element_index` / `element_token` → the accessibility rung) or an **element
//! px action** (`x` / `y` → the pixel rung).
//!
//! `capture_mode` is still *accepted* on `get_window_state` so older MCP/CLI
//! callers don't hard-break under `additionalProperties:false`, but it has no
//! effect — both tree and screenshot come back regardless of its value.

/// The `capture_mode` JSON-schema fragment, shared by every platform's
/// `get_window_state` so the (deprecated) param advertises an identical shape
/// across macOS / Linux / Windows. Retained for back-compat only — see the
/// module docs. The enum keeps `ax` / `vision` so a client that still sends one
/// validates, but the value is ignored.
pub fn capture_mode_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "enum": ["ax", "vision"],
        "description": "DEPRECATED and ignored. get_window_state always returns \
            BOTH the element tree and a screenshot — ground on both. The modality \
            is chosen at action time by how you address the target: an element ax \
            action (element_index/element_token) or an element px action (x,y). \
            Any value (including the old \"som\"/\"screenshot\" aliases) is \
            accepted but has no effect."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_marked_deprecated_and_keeps_enum_for_back_compat() {
        let schema = capture_mode_schema();
        // Enum retained so an old client sending ax/vision still validates.
        let modes = schema["enum"].as_array().unwrap();
        assert!(modes.iter().any(|m| m == "ax"));
        assert!(modes.iter().any(|m| m == "vision"));
        // Description must signal it's a no-op so nobody relies on it.
        let desc = schema["description"].as_str().unwrap();
        assert!(desc.contains("DEPRECATED"), "must flag deprecation: {desc}");
        assert!(desc.contains("both"), "must say both are returned: {desc}");
    }
}
