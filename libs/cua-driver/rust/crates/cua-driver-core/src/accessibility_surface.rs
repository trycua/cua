//! Shared contract for read-only accessibility surfaces without native window identity.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

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
            Native windows with an exact window_id remain observable through get_window_state."
            .into(),
        input_schema: json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "session": { "type": "string" },
                "pid": { "type": "integer", "minimum": 1 },
                "max_elements": { "type": "integer", "minimum": 1, "maximum": MAX_ELEMENTS, "default": MAX_ELEMENTS },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": MAX_DEPTH, "default": MAX_DEPTH }
            },
            "x-cua-platform-support": {
                "macos": "supported",
                "windows": "unsupported",
                "linux": "unsupported"
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

pub struct UnsupportedAccessibilitySurfaceTool {
    platform: &'static str,
}

impl UnsupportedAccessibilitySurfaceTool {
    pub const fn new(platform: &'static str) -> Self {
        Self { platform }
    }
}

#[async_trait]
impl Tool for UnsupportedAccessibilitySurfaceTool {
    fn def(&self) -> &ToolDef {
        tool_def()
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::error(format!(
            "{TOOL_NAME}: accessibility-only window surfaces are unsupported on {}",
            self.platform
        ))
        .with_structured(json!({
            "code": "unsupported_capability",
            "effect": "refused",
            "capability": capability("unsupported", self.platform),
        }))
    }
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
        assert_eq!(schema["x-cua-platform-support"]["macos"], "supported");
        assert_eq!(schema["x-cua-platform-support"]["windows"], "unsupported");
    }

    #[tokio::test]
    async fn unsupported_platform_refuses_explicitly() {
        let result = UnsupportedAccessibilitySurfaceTool::new("windows")
            .invoke(json!({}))
            .await;
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured refusal");
        assert_eq!(structured["code"], "unsupported_capability");
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["capability"]["actions_supported"], false);
    }
}
