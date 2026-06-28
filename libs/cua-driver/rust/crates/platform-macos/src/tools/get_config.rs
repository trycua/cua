use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
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

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        // Resolve effective values for the CALLING session: a named session
        // sees its own override layered over the global; the anonymous session
        // (absent `_session_id`) sees the raw global — today's behavior.
        let session_id = args.opt_str("_session_id");
        let max_image_dimension = {
            let cfg = self.state.config.read().unwrap();
            self.state.session_config.effective_max_image_dimension(session_id.as_deref(), &cfg)
        };
        // Report the CALLING session's own cursor enabled-state, not a
        // nondeterministic HashMap.first(). Resolve the same key the click /
        // cursor tools use (cursor_id > _session_id > "default"); fall back to
        // the seeded "default" cursor when this session hasn't materialised its
        // own cursor yet, and finally to `true` (the overlay default).
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
        let cursor_enabled = self.state.cursor_registry.get(&cursor_key)
            .or_else(|| self.state.cursor_registry.get("default"))
            .map(|s| s.config.enabled)
            .unwrap_or(true);
        // PiP values aren't in DriverConfig — they're file-only since the
        // backend is initialised once at startup. Read fresh so the
        // response reflects whatever set_config (or a direct JSON edit)
        // last wrote.
        let (pip_enabled, pip_geometry) = pip_preview::read_pip_keys_from_file();
        ToolResult::text("cua-driver-rs configuration")
            .with_structured(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "platform": "macos",
                // capture_mode / scope are per-call params now, not settings.
                "max_image_dimension": max_image_dimension,
                "agent_cursor": {
                    "enabled": cursor_enabled,
                },
                "experimental_pip": pip_enabled,
                "experimental_pip_geometry": pip_geometry,
            }))
    }
}
