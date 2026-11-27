// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Shared contract for read-only accessibility surfaces without native window identity.

use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::tool::ToolDef;

pub const TOOL_NAME: &str = "get_accessibility_surfaces";
pub const CAPABILITY: &str = "accessibility.observation.ax_window";
pub const SURFACE_KIND: &str = "ax_window";
pub const MAX_ELEMENTS: usize = 2_000;
pub const MAX_DEPTH: usize = 25;

pub fn tool_def() -> &'static ToolDef {
    static DEF: OnceLock<ToolDef> = OnceLock::new();
    DEF.get_or_init(|| ToolDef {
        name: TOOL_NAME.into(),
        description: "Observe the bounded semantic trees of top-level accessibility windows for \
            one process that have no native window id. Results preserve each AXWindow boundary \
            and expose no screenshot, element_index, element_token, or action-cache side effect. \
            Native windows with an exact window_id remain observable through get_window_state. \
            Supported on macOS; unsupported on Windows and Linux."
            .into(),
        input_schema: json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "session": crate::tool_schema::session_schema(),
                "pid": { "type": "integer", "minimum": 1, "description": "Process ID to observe." },
                "max_elements": { "type": "integer", "minimum": 1, "maximum": MAX_ELEMENTS, "default": MAX_ELEMENTS, "description": "Maximum number of accessibility elements to return." },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": MAX_DEPTH, "default": MAX_DEPTH, "description": "Maximum accessibility tree depth to traverse." }
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

pub fn capability(status: &str, platform: &str) -> Value {
    json!({
        "name": CAPABILITY,
        "status": status,
        "platform": platform,
        "surface_kind": SURFACE_KIND,
        "actions_supported": false,
        "screenshot_supported": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_has_no_window_or_action_identity() {
        let schema = &tool_def().input_schema;
        assert_eq!(schema["required"], json!(["pid"]));
        for forbidden in [
            "window_id",
            "surface_token",
            "query",
            "element_index",
            "element_token",
            "snapshot_id",
        ] {
            assert!(schema["properties"].get(forbidden).is_none());
        }
        assert!(schema.get("x-cua-platform-support").is_none());
        for parameter in schema["properties"].as_object().unwrap().values() {
            assert!(!parameter["description"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn discovery_does_not_replace_capture_backed_observations() {
        assert_eq!(
            crate::tool::default_capabilities_for(TOOL_NAME),
            vec![CAPABILITY.to_owned()]
        );
        let window = crate::tool::default_capabilities_for("get_window_state");
        assert!(window.iter().any(|cap| cap == "accessibility.tree.bounded"));
        assert!(window.iter().any(|cap| cap == "screen.capture.window"));
    }
}
