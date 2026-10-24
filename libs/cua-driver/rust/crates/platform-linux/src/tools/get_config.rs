use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::ToolState;

// ── get_config ────────────────────────────────────────────────────────────────

pub struct GetConfigTool {
    pub(crate) state: Arc<ToolState>,
}
static GCFG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetConfigTool {
    fn def(&self) -> &ToolDef {
        GCFG_DEF.get_or_init(|| ToolDef {
            name: "get_config".into(),
            description: "Return current cua-driver-rs configuration.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let cfg = self.state.config.read().unwrap();
        let (pip_enabled, pip_geometry) = pip_preview::read_pip_keys_from_file();
        ToolResult::text("cua-driver-rs configuration").with_structured(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "source_sha": option_env!("CUA_DRIVER_SOURCE_SHA"),
            "platform": "linux",
            "capture_mode": cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "cursor": {
                "motion": cursor_overlay::motion_defaults::read_saved().config_json(),
                "badges": cursor_overlay::badge_mode::read_saved().as_str(),
            },
            "experimental_pip": pip_enabled,
            "experimental_pip_geometry": pip_geometry
        }))
    }
}
