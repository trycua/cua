// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
        properties.extend(cursor_overlay::cursor_options::config_schema_properties());
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
            description: "Update cua-driver-rs configuration. capture_mode / \
                max_image_dimension take effect immediately.\n\n\
                Pass each setting as its own field: `{\"capture_mode\": \"som\", \"max_image_dimension\": 0}`.\n\n\
                `cursor.motion.style`, `cursor.motion.timing` and `cursor.motion.effects.<name>` \
                (`on`, `off`, or `default`) save the default cursor motion for sessions started afterwards. \
                `cursor.motion: \"default\"` clears every saved cursor motion default.\n\n\
                `cursor.badges` (`minimal` default, `verbose`, or `off`) sets the agent cursor badge \
                for every cursor at once.\n\n\
                `cursor.float` (float at rest and while moving) and `cursor.halo` (soft glow) are \
                `on` (default), `off` or `default`; `cursor.marks` (action marks) is `on`, \
                `off` (default) or `default`; `cursor.rotate` is `heading` (default), `lean`, \
                `off` or `default`. All apply to every cursor at once.".into(),
            input_schema: with_cursor_motion_config_properties(json!({"type":"object","properties":{
                "capture_mode":{"type":"string","enum":["ax","vision"],"description":"Default capture mode for get_window_state. (\"som\"/\"screenshot\" still decode as deprecated aliases.)"},
                "max_image_dimension":{"type":"integer","description":"Max dimension for screenshot resizing (0 = no limit)."},
                "fast_path": cua_driver_core::fast_path::wire::config_schema()
            },"additionalProperties":false})),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
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
        let badge_key = match cursor_overlay::badge_mode::apply_config_args(&args) {
            Ok(key) => key,
            Err(message) => return ToolResult::error(message),
        };
        let option_keys = match cursor_overlay::cursor_options::apply_config_args(&args) {
            Ok(keys) => keys,
            Err(message) => return ToolResult::error(message),
        };
        let mut cfg = self.state.config.write().unwrap();
        let mut parts: Vec<String> = motion_keys
            .iter()
            .map(|key| format!("{key} (applies to sessions started from now on)"))
            .collect();
        parts.extend(
            badge_key
                .into_iter()
                .chain(option_keys)
                .map(|key| format!("{key} (applies now)")),
        );
        if let Some(mode) = args.opt_str("capture_mode") {
            if let Err(e) = cua_driver_core::config_file::write_config_key(
                "capture_mode",
                Value::String(mode.clone()),
            ) {
                tracing::warn!("set_config: failed to persist capture_mode: {e}");
            }
            parts.push(format!("capture_mode={mode}"));
            cfg.capture_mode = mode;
        }
        match cua_driver_core::fast_path::wire::apply_config_args(&args) {
            Ok(Some(mode)) => parts.push(format!(
                "fast_path={} (applies now, not saved)",
                mode.as_str()
            )),
            Ok(None) => {}
            Err(message) => return ToolResult::error(message),
        }
        if let Some(dim) = args.opt_u64("max_image_dimension") {
            cfg.max_image_dimension = dim as u32;
            if let Err(e) = cua_driver_core::config_file::write_config_key(
                "max_image_dimension",
                Value::from(dim),
            ) {
                tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
            }
            parts.push(format!("max_image_dimension={dim}"));
        }
        let msg = if parts.is_empty() {
            "Config unchanged (no known parameters).".to_owned()
        } else {
            format!("Config updated: {}", parts.join(", "))
        };
        ToolResult::text(msg).with_structured(json!({
            "capture_mode": cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "fast_path": cua_driver_core::fast_path::wire::config_value()
        }))
    }
}
