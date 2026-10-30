use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{resolve_cursor_key, ToolState};

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
            // Description ported from Swift `GetConfigTool.swift` with the
            // Windows config path. Swift's `agent_cursor.*` subtree is not
            // mirrored on Windows yet (separate cursor-overlay config path).
            description: "Report the current persistent driver config.\n\n\
                Pure read-only. Returns defaults when the underlying state is unset — \
                same fallback the daemon uses at startup. Sibling to `set_config`.\n\n\
                Current schema (Windows):\n\n  \
                {\n    \"schema_version\": 1,\n    \"version\": \"<crate version>\",\n    \
                \"source_sha\": \"<maintainer build commit or null>\",\n    \"platform\": \"windows\",\n    \
                \"capture_mode\": \"ax\" | \"vision\" (DEPRECATED, ignored),\n    \
                \"max_image_dimension\": 0\n  }\n\n\
                `capture_mode` is deprecated and no longer affects behavior — \
                `get_window_state` always returns both the UIA tree and a screenshot.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cfg = self.state.config.read().unwrap();
        // Mirror the macOS agent's parity addition (commit adb9ecca):
        // nested `agent_cursor.enabled` block so Swift-shaped get_config
        // consumers can read the cursor's enabled state from one place.
        // Scope to the CALLING session's cursor (session > cursor_id > default)
        // and read it from the overlay deterministically — `all_states().first()`
        // was a nondeterministic HashMap read across sessions (macOS BUG 3).
        let cursor_enabled = crate::overlay::is_enabled(&resolve_cursor_key(&args));
        let (pip_enabled, pip_geometry) = pip_preview::read_pip_keys_from_file();
        let payload = json!({
            "schema_version":      1,
            "version":             env!("CARGO_PKG_VERSION"),
            "source_sha":          option_env!("CUA_DRIVER_SOURCE_SHA"),
            "platform":            "windows",
            "capture_mode":        cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "agent_cursor":        { "enabled": cursor_enabled },
            "cursor":              cursor_overlay::cursor_options::saved_config_json(),
            "experimental_pip":    pip_enabled,
            "experimental_pip_geometry": pip_geometry,
        });
        // Match Swift's text format 1:1: `"✅ <pretty JSON>"`.
        let pretty = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
        ToolResult::text(format!("✅ {pretty}")).with_structured(payload)
    }
}
