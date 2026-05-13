use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct GetConfigTool {
    state: Arc<ToolState>,
}

impl GetConfigTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_config".into(),
        description: "Return the current cua-driver-rs configuration.".into(),
        input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for GetConfigTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let cfg = self.state.config.read().unwrap();
        let cursor_enabled = self.state.cursor_registry.all_states()
            .first()
            .map(|s| s.config.enabled)
            .unwrap_or(true);
        ToolResult::text("cua-driver-rs configuration")
            .with_structured(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "platform": "macos",
                "capture_mode": cfg.capture_mode,
                "max_image_dimension": cfg.max_image_dimension,
                "agent_cursor": {
                    "enabled": cursor_enabled,
                },
            }))
    }
}
