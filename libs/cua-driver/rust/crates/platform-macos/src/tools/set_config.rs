use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use super::{write_driver_config_key, ConfigOverrides, ToolState};

pub struct SetConfigTool {
    state: Arc<ToolState>,
}

impl SetConfigTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn with_cursor_motion_properties(mut schema: Value) -> Value {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.extend(cursor_overlay::motion_defaults::config_schema_properties());
    }
    schema
}

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "set_config".into(),
        description: "Update cua-driver-rs configuration. Changes to \
            max_image_dimension take effect immediately. The \
            experimental_pip keys are persisted to ~/.cua-driver/config.json and \
            take effect on the next daemon restart (the PiP backend is \
            initialised once at startup).\n\nNote: capture_mode is a per-call \
            param (on get_window_state / click), not a stored setting. Capture \
            modality is selected by each action's target; the old \
            capture_scope config key is retired.\n\nCursor motion defaults: \
            cursor.motion.style, cursor.motion.timing and \
            cursor.motion.effects.<trail|glow|magnet|ripple|squish> are saved to \
            ~/.cua-driver/config.json and seed sessions started afterwards. \
            start_session cursor_motion and set_agent_cursor_motion override them; \
            reduced motion always wins."
            .into(),
        input_schema: with_cursor_motion_properties(serde_json::json!({
            "type": "object",
            "properties": {
                "max_image_dimension": {
                    "type": "integer",
                    "description": "Max dimension for screenshot resizing (0 = no limit)."
                },
                "experimental_pip": {
                    "type": "boolean",
                    "description": "Enable the experimental picture-in-picture preview window. \
                        Applies on next daemon restart."
                },
                "experimental_pip_geometry": {
                    "type": "string",
                    "description": "PiP window size + optional position in `WxH` or `WxH+X+Y` \
                        form (e.g. `320x200+24+24`). Applies on next daemon restart."
                }
            },
            "additionalProperties": false
        })),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for SetConfigTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("capture_scope").is_some() {
            return ToolResult::error(
                "config key 'capture_scope' is retired; select a window or desktop target on each action",
            )
            .with_structured(serde_json::json!({
                "code": "config_key_retired",
                "key": "capture_scope",
                "replacement": "action.target",
            }));
        }
        // Whether run_script is offered is the operator's choice, made in
        // config.json or the environment; a model cannot change it.
        if let Some(key) = args
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| matches!(*key, "disable_run_script" | "experimental_script"))
        {
            return ToolResult::error(format!(
                "config key '{key}' is set by whoever runs cua-driver, in \
                 ~/.cua-driver/config.json or the environment, not through set_config"
            ))
            .with_structured(serde_json::json!({
                "code": "config_key_operator_only",
                "key": key,
            }));
        }
        // The daemon injects `_session_id` for non-anonymous MCP sessions.
        // Absent => anonymous/global session (CLI one-shot, legacy proxy) =>
        // today's behavior: write the shared global DriverConfig + persist to
        // disk. Present => session-scoped in-memory override only, never
        // touching the global config or the on-disk default, so two concurrent
        // sessions don't clobber each other or the persisted default.
        let session_id = args.opt_str("_session_id");

        // Cursor motion defaults are a driver-wide saved setting, so they are
        // validated and persisted like the other config.json keys.
        let motion_keys = match cursor_overlay::motion_defaults::apply_config_args(&args) {
            Ok(keys) => keys,
            Err(message) => return ToolResult::error(message),
        };

        let max_dim: Option<u32> = match args.opt_u64("max_image_dimension") {
            Some(dim) => match u32::try_from(dim) {
                Ok(d) => Some(d),
                Err(_) => {
                    return ToolResult::error(format!("max_image_dimension {dim} exceeds u32::MAX"))
                }
            },
            None => None,
        };

        let effective_dim = if let Some(sid) = session_id.as_deref() {
            // Session-scoped override: in-memory only, no global write, no disk.
            self.state.session_config.set(
                sid,
                ConfigOverrides {
                    max_image_dimension: max_dim,
                },
            );
            self.state
                .session_config
                .effective_max_image_dimension(Some(sid), &self.state.config.read().unwrap())
        } else {
            // Anonymous/global session: write the shared global + persist.
            let mut cfg = self.state.config.write().unwrap();
            if let Some(dim32) = max_dim {
                cfg.max_image_dimension = dim32;
                if let Err(e) = write_driver_config_key(
                    "max_image_dimension",
                    &Value::Number(u64::from(dim32).into()),
                ) {
                    tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
                }
            }
            cfg.max_image_dimension
        };
        // PiP keys persist to the same config.json but take effect only on
        // next daemon restart — the backend is initialised once at startup.
        let mut pip_note = String::new();
        if let Some(enabled) = args.get("experimental_pip").and_then(|v| v.as_bool()) {
            if let Err(e) = pip_preview::write_config_key("experimental_pip", Value::Bool(enabled))
            {
                return ToolResult::error(format!("failed to persist experimental_pip: {e}"));
            }
            pip_note =
                format!(" — restart cua-driver for experimental_pip={enabled} to take effect");
        }
        if let Some(geom) = args.opt_str("experimental_pip_geometry") {
            // Validate before persisting so the user gets an immediate error.
            if pip_preview::PipGeometry::parse(&geom).is_none() {
                return ToolResult::error(format!(
                    "experimental_pip_geometry `{geom}` is not a valid WxH or WxH+X+Y string"
                ));
            }
            if let Err(e) = pip_preview::write_config_key(
                "experimental_pip_geometry",
                Value::String(geom.clone()),
            ) {
                return ToolResult::error(format!(
                    "failed to persist experimental_pip_geometry: {e}"
                ));
            }
            if pip_note.is_empty() {
                pip_note = format!(
                    " — restart cua-driver for experimental_pip_geometry={geom} to take effect"
                );
            }
        }
        if !motion_keys.is_empty() {
            pip_note.push_str(&format!(
                " — saved {} (applies to sessions started from now on)",
                motion_keys.join(", ")
            ));
        }
        let scope_note = if session_id.is_some() {
            " (session-scoped; persisted default unchanged)"
        } else {
            ""
        };
        ToolResult::text(format!(
            "Config updated: max_image_dimension={}{}{}",
            effective_dim, scope_note, pip_note
        ))
        .with_structured(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "platform": "macos",
            "max_image_dimension": effective_dim,
        }))
    }
}
