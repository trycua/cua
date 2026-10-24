use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::ToolState;

// ── set_config ────────────────────────────────────────────────────────────────

fn with_cursor_motion_config_properties(mut schema: Value) -> Value {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.extend(cursor_overlay::motion_defaults::config_schema_properties());
        properties.extend(cursor_overlay::badge_mode::config_schema_properties());
    }
    schema
}

pub struct SetConfigTool {
    pub(crate) state: Arc<ToolState>,
}
static SCFG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetConfigTool {
    fn def(&self) -> &ToolDef {
        SCFG_DEF.get_or_init(|| ToolDef {
            name: "set_config".into(),
            description: "Write a setting into the persistent driver config. Values take \
                effect immediately. Pass each setting as its own field.\n\n\
                Known keys:\n\
                - `capture_mode` (string: `vision` | `ax` | `som`) — DEPRECATED and ignored; \
                  `get_window_state` always returns both the UIA tree and a screenshot. Still \
                  accepted/persisted for back-compat but has no effect.\n\
                - `max_image_dimension` (integer)\n\
                - `experimental_pip` (boolean; persisted to config.json, applies on next daemon restart — Windows backend stubbed today, see issue #1729)\n\
                - `experimental_pip_geometry` (string `WxH` or `WxH+X+Y`; persisted; applies on next daemon restart)\n\n\
                - `cursor.motion.style`, `cursor.motion.timing`, `cursor.motion.effects.<name>` (`on`, `off`, or `default`) — saved default cursor motion for sessions started afterwards. `cursor.motion: \"default\"` clears every saved cursor motion default.\n\
                - `cursor.badges` (`minimal` default, `verbose`, or `off`) — agent cursor badge; applies to every cursor at once.\n\n\
                Returns the full updated config in the same shape as `get_config`.".into(),
            input_schema: with_cursor_motion_config_properties(json!({"type":"object","properties":{
                "capture_mode":{"type":"string","enum":["ax","vision"],"description":"DEPRECATED and ignored — get_window_state always returns both the UIA tree and a screenshot. Still accepted/persisted for back-compat but has no effect. (\"som\"/\"screenshot\" still decode as deprecated aliases.)"},
                "max_image_dimension":{"type":"integer","description":"Max dimension for screenshot resizing (0 = no limit)."},
                "experimental_pip":{"type":"boolean","description":"Enables PiP preview (applies next restart)."},
                "experimental_pip_geometry":{"type":"string","description":"PiP window size + optional position."}
            },"additionalProperties":false})),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        if args.get("capture_scope").is_some() {
            return ToolResult::error(
                "config key 'capture_scope' is retired; select a window or desktop target on each action",
            )
            .with_structured(json!({
                "code": "config_key_retired",
                "key": "capture_scope",
                "replacement": "action.target",
            }));
        }
        let motion_keys = match cursor_overlay::motion_defaults::apply_config_args(&args) {
            Ok(keys) => keys,
            Err(message) => return ToolResult::error(message),
        };
        let mut cfg = self.state.config.write().unwrap();
        let badge_key = match cursor_overlay::badge_mode::apply_config_args(&args) {
            Ok(key) => key,
            Err(message) => return ToolResult::error(message),
        };
        let mut applied = !motion_keys.is_empty() || badge_key.is_some();
        if let Some(mode) = args.get("capture_mode").and_then(|v| v.as_str()) {
            cfg.capture_mode = mode.to_owned();
            if let Err(e) =
                pip_preview::write_config_key("capture_mode", Value::String(mode.to_owned()))
            {
                tracing::warn!("set_config: failed to persist capture_mode: {e}");
            }
            applied = true;
        }
        if let Some(dim) = args.get("max_image_dimension").and_then(|v| v.as_u64()) {
            cfg.max_image_dimension = dim as u32;
            if let Err(e) = pip_preview::write_config_key("max_image_dimension", Value::from(dim)) {
                tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
            }
            applied = true;
        }
        if let Some(enabled) = args.get("experimental_pip").and_then(|v| v.as_bool()) {
            if let Err(e) = pip_preview::write_config_key("experimental_pip", Value::Bool(enabled))
            {
                return ToolResult::error(format!("failed to persist experimental_pip: {e}"));
            }
            applied = true;
        }
        if let Some(geom) = args
            .get("experimental_pip_geometry")
            .and_then(|v| v.as_str())
        {
            if pip_preview::PipGeometry::parse(geom).is_none() {
                return ToolResult::error(format!(
                    "experimental_pip_geometry `{geom}` is not a valid WxH or WxH+X+Y string"
                ));
            }
            if let Err(e) = pip_preview::write_config_key(
                "experimental_pip_geometry",
                Value::String(geom.to_owned()),
            ) {
                return ToolResult::error(format!(
                    "failed to persist experimental_pip_geometry: {e}"
                ));
            }
            applied = true;
        }
        if !applied {
            return ToolResult::error("set_config did not contain a known field.");
        }
        // Emit the same pretty-JSON payload as `get_config` (matches Swift's
        // `set_config` return shape — both tools echo the full config after).
        let cursor_enabled = self
            .state
            .cursor_registry
            .all_states()
            .first()
            .map(|s| s.config.enabled)
            .unwrap_or(true);
        let (pip_enabled, pip_geometry) = pip_preview::read_pip_keys_from_file();
        let payload = json!({
            "schema_version":      1,
            "version":             env!("CARGO_PKG_VERSION"),
            "source_sha":          option_env!("CUA_DRIVER_SOURCE_SHA"),
            "platform":            "windows",
            "capture_mode":        cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "agent_cursor":        { "enabled": cursor_enabled },
            "experimental_pip":    pip_enabled,
            "experimental_pip_geometry": pip_geometry,
        });
        let pretty = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
        ToolResult::text(format!("✅ {pretty}")).with_structured(payload)
    }
}
