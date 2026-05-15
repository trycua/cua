use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::{write_driver_config_key, ToolState};

pub struct SetConfigTool {
    state: Arc<ToolState>,
}

impl SetConfigTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "set_config".into(),
        description: "Update cua-driver-rs configuration. Changes take effect immediately.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "capture_mode": {
                    "type": "string",
                    "enum": ["som", "vision", "ax"],
                    "description": "Default capture mode for get_window_state."
                },
                "max_image_dimension": {
                    "type": "integer",
                    "description": "Max dimension for screenshot resizing (0 = no limit)."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for SetConfigTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let mut cfg = self.state.config.write().unwrap();
        if let Some(mode) = args.get("capture_mode").and_then(|v| v.as_str()) {
            cfg.capture_mode = mode.to_owned();
            if let Err(e) = write_driver_config_key("capture_mode", &Value::String(mode.to_owned())) {
                tracing::warn!("set_config: failed to persist capture_mode: {e}");
            }
        }
        if let Some(dim) = args.get("max_image_dimension").and_then(|v| v.as_u64()) {
            if let Ok(dim32) = u32::try_from(dim) {
                cfg.max_image_dimension = dim32;
                if let Err(e) = write_driver_config_key("max_image_dimension", &Value::Number(dim.into())) {
                    tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
                }
            } else {
                return ToolResult::error(format!("max_image_dimension {dim} exceeds u32::MAX"));
            }
        }
        ToolResult::text(format!(
            "Config updated: capture_mode={}, max_image_dimension={}",
            cfg.capture_mode, cfg.max_image_dimension
        ))
    }
}
