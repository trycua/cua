//! Shared contract for read-only application-composite accessibility observation.
//!
//! This scope is deliberately separate from exact-window state. Its observation
//! indices are not action handles, and platform adapters must not place them in
//! an exact `(pid, window_id)` element cache.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};

pub const TOOL_NAME: &str = "get_application_state";
pub const CAPABILITY: &str = "accessibility.observation.application_composite";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationObservationScope {
    ApplicationComposite,
}

impl ApplicationObservationScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApplicationComposite => "application_composite",
        }
    }
}

pub fn tool_def() -> &'static ToolDef {
    static DEF: OnceLock<ToolDef> = OnceLock::new();
    DEF.get_or_init(|| ToolDef {
        name: TOOL_NAME.into(),
        description: "Observe a bounded accessibility tree rooted at one application process. \
            This tool requires explicit scope:\"application_composite\" and is read-only: \
            observation_index values are not accepted by element actions, no element tokens are \
            minted, no exact-window action cache is updated, and no screenshot is implied. Use \
            get_window_state for exact (pid, window_id) observation and actions. Platforms without \
            application-composite observation return unsupported_capability explicitly."
            .into(),
        input_schema: json!({
            "type": "object",
            "required": ["pid", "scope"],
            "properties": {
                "session": { "type": "string" },
                "pid": { "type": "integer", "minimum": 1, "description": "Exact target process ID." },
                "scope": {
                    "type": "string",
                    "const": "application_composite",
                    "description": "Explicit application-level AX observation scope. Never inferred from an exact-window failure."
                },
                "query": { "type": "string", "maxLength": 500, "description": "Optional case-insensitive semantic filter." },
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
        idempotent: false,
        open_world: false,
    })
}

pub fn scope_identity(pid: i32, scope_id: &str) -> Value {
    json!({
        "kind": ApplicationObservationScope::ApplicationComposite.as_str(),
        "pid": pid,
        "scope_id": scope_id,
    })
}

pub fn capability(status: &str, platform: &str) -> Value {
    json!({
        "name": CAPABILITY,
        "status": status,
        "platform": platform,
        "actions_supported": false,
        "screenshot_supported": false,
    })
}

pub fn unsupported_result(platform: &str) -> ToolResult {
    ToolResult::error(format!(
        "{TOOL_NAME}: application-composite accessibility observation is unsupported on {platform}"
    ))
    .with_structured(json!({
        "code": "unsupported_capability",
        "effect": "refused",
        "scope": { "kind": ApplicationObservationScope::ApplicationComposite.as_str() },
        "capability": capability("unsupported", platform),
    }))
}

pub struct UnsupportedApplicationStateTool {
    platform: &'static str,
}

impl UnsupportedApplicationStateTool {
    pub const fn new(platform: &'static str) -> Self {
        Self { platform }
    }
}

#[async_trait]
impl Tool for UnsupportedApplicationStateTool {
    fn def(&self) -> &ToolDef {
        tool_def()
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        unsupported_result(self.platform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_requires_explicit_pid_and_composite_scope() {
        let schema = &tool_def().input_schema;
        assert_eq!(schema["required"], json!(["pid", "scope"]));
        assert_eq!(
            schema["properties"]["scope"]["const"],
            "application_composite"
        );
        assert!(schema["properties"].get("window_id").is_none());
        assert!(schema["properties"].get("element_index").is_none());
        assert!(schema["properties"].get("element_token").is_none());
        assert_eq!(schema["x-cua-platform-support"]["macos"], "supported");
        assert_eq!(schema["x-cua-platform-support"]["windows"], "unsupported");
        assert!(crate::tool::default_capabilities_for(TOOL_NAME)
            .iter()
            .any(|capability| capability == CAPABILITY));
    }

    #[tokio::test]
    async fn unsupported_platform_is_typed_and_fail_closed() {
        let result = UnsupportedApplicationStateTool::new("windows")
            .invoke(json!({"pid": 7, "scope": "application_composite"}))
            .await;
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured refusal");
        assert_eq!(structured["code"], "unsupported_capability");
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["capability"]["status"], "unsupported");
        assert_eq!(structured["capability"]["actions_supported"], false);
    }
}
