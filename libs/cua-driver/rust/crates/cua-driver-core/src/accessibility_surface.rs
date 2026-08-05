//! Shared contract for accessibility surfaces without native window identity.
//!
//! An accessibility surface is narrower than an application: it names one
//! native accessibility window that cannot be attributed to a platform window
//! id. Surface tokens are observation handles, never element-action handles.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

pub const LIST_TOOL_NAME: &str = "list_accessibility_surfaces";
pub const GET_TOOL_NAME: &str = "get_accessibility_surface_state";
pub const DISCOVERY_CAPABILITY: &str = "accessibility.surface.discovery.ax_window";
pub const OBSERVATION_CAPABILITY: &str = "accessibility.observation.ax_window";
pub const SURFACE_KIND: &str = "ax_window";

pub fn list_tool_def() -> &'static ToolDef {
    static DEF: OnceLock<ToolDef> = OnceLock::new();
    DEF.get_or_init(|| ToolDef {
        name: LIST_TOOL_NAME.into(),
        description: "List top-level accessibility windows for one process that have no native \
            window id. Each result has a runtime-bound surface_token for read-only semantic \
            observation. The token is not accepted by get_window_state or element actions. \
            Native windows with an exact window_id remain discoverable through list_windows."
            .into(),
        input_schema: json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "session": { "type": "string" },
                "pid": { "type": "integer", "minimum": 1 }
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

pub fn get_tool_def() -> &'static ToolDef {
    static DEF: OnceLock<ToolDef> = OnceLock::new();
    DEF.get_or_init(|| ToolDef {
        name: GET_TOOL_NAME.into(),
        description: "Observe one accessibility-only window returned by \
            list_accessibility_surfaces. The bounded result contains semantic nodes only: it \
            has no screenshot, element_index, element_token, or action-cache side effect."
            .into(),
        input_schema: json!({
            "type": "object",
            "required": ["pid", "surface_token"],
            "properties": {
                "session": { "type": "string" },
                "pid": { "type": "integer", "minimum": 1 },
                "surface_token": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Opaque observation handle from list_accessibility_surfaces."
                },
                "query": { "type": "string", "maxLength": 500 },
                "max_elements": { "type": "integer", "minimum": 1, "maximum": 2000, "default": 2000 },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 25, "default": 25 }
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

pub fn capability(name: &str, status: &str, platform: &str) -> Value {
    json!({
        "name": name,
        "status": status,
        "platform": platform,
        "surface_kind": SURFACE_KIND,
        "actions_supported": false,
        "screenshot_supported": false,
    })
}

pub fn unsupported_result(tool_name: &str, capability_name: &str, platform: &str) -> ToolResult {
    ToolResult::error(format!(
        "{tool_name}: accessibility-only window surfaces are unsupported on {platform}"
    ))
    .with_structured(json!({
        "code": "unsupported_capability",
        "effect": "refused",
        "capability": capability(capability_name, "unsupported", platform),
    }))
}

pub struct UnsupportedAccessibilitySurfaceTool {
    platform: &'static str,
    list: bool,
}

impl UnsupportedAccessibilitySurfaceTool {
    pub const fn list(platform: &'static str) -> Self {
        Self {
            platform,
            list: true,
        }
    }

    pub const fn get(platform: &'static str) -> Self {
        Self {
            platform,
            list: false,
        }
    }
}

#[async_trait]
impl Tool for UnsupportedAccessibilitySurfaceTool {
    fn def(&self) -> &ToolDef {
        if self.list {
            list_tool_def()
        } else {
            get_tool_def()
        }
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        if self.list {
            unsupported_result(LIST_TOOL_NAME, DISCOVERY_CAPABILITY, self.platform)
        } else {
            unsupported_result(GET_TOOL_NAME, OBSERVATION_CAPABILITY, self.platform)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_keep_ax_surfaces_separate_from_window_and_action_identity() {
        let list = &list_tool_def().input_schema;
        let get = &get_tool_def().input_schema;
        assert_eq!(list["required"], json!(["pid"]));
        assert_eq!(get["required"], json!(["pid", "surface_token"]));
        for forbidden in ["window_id", "element_index", "element_token", "snapshot_id"] {
            assert!(get["properties"].get(forbidden).is_none());
        }
        assert_eq!(get["x-cua-platform-support"]["macos"], "supported");
        assert_eq!(get["x-cua-platform-support"]["windows"], "unsupported");
    }

    #[tokio::test]
    async fn unsupported_platform_refuses_both_operations() {
        for tool in [
            UnsupportedAccessibilitySurfaceTool::list("windows"),
            UnsupportedAccessibilitySurfaceTool::get("windows"),
        ] {
            let result = tool.invoke(json!({})).await;
            assert_eq!(result.is_error, Some(true));
            let structured = result.structured_content.expect("structured refusal");
            assert_eq!(structured["code"], "unsupported_capability");
            assert_eq!(structured["effect"], "refused");
            assert_eq!(structured["capability"]["actions_supported"], false);
        }
    }
}
