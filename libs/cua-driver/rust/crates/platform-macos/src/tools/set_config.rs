use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::{write_driver_config_key, ConfigOverrides, ToolState};

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
        description: "Update cua-driver-rs configuration. Changes to capture_mode \
            and max_image_dimension take effect immediately. The experimental_pip \
            keys are persisted to ~/.cua-driver/config.json and take effect on \
            the next daemon restart (the PiP backend is initialised once at \
            startup).".into(),
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
        use cua_driver_core::tool_args::ArgsExt;
        // The daemon injects `_session_id` for non-anonymous MCP sessions.
        // Absent => anonymous/global session (CLI one-shot, legacy proxy) =>
        // today's behavior: write the shared global DriverConfig + persist to
        // disk. Present => session-scoped in-memory override only, never
        // touching the global config or the on-disk default, so two concurrent
        // sessions don't clobber each other or the persisted default.
        let session_id = args.opt_str("_session_id");

        // Validate max_image_dimension up front so both branches share the
        // u32 check and we never half-apply.
        let max_dim: Option<u32> = match args.opt_u64("max_image_dimension") {
            Some(dim) => match u32::try_from(dim) {
                Ok(d) => Some(d),
                Err(_) => return ToolResult::error(format!("max_image_dimension {dim} exceeds u32::MAX")),
            },
            None => None,
        };
        let capture_mode = args.opt_str("capture_mode");

        let (effective_mode, effective_dim) = if let Some(sid) = session_id.as_deref() {
            // Session-scoped override: in-memory only, no global write, no disk.
            self.state.session_config.set(sid, ConfigOverrides {
                capture_mode: capture_mode.clone(),
                max_image_dimension: max_dim,
            });
            self.state.session_config.effective(Some(sid), &self.state.config.read().unwrap())
        } else {
            // Anonymous/global session: write the shared global + persist,
            // exactly as before this change.
            let mut cfg = self.state.config.write().unwrap();
            if let Some(mode) = capture_mode.clone() {
                cfg.capture_mode = mode.clone();
                if let Err(e) = write_driver_config_key("capture_mode", &Value::String(mode)) {
                    tracing::warn!("set_config: failed to persist capture_mode: {e}");
                }
            }
            if let Some(dim32) = max_dim {
                cfg.max_image_dimension = dim32;
                if let Err(e) = write_driver_config_key("max_image_dimension", &Value::Number(u64::from(dim32).into())) {
                    tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
                }
            }
            (cfg.capture_mode.clone(), cfg.max_image_dimension)
        };
        // PiP keys persist to the same config.json but take effect only on
        // next daemon restart — the backend is initialised once at startup.
        let mut pip_note = String::new();
        if let Some(enabled) = args.get("experimental_pip").and_then(|v| v.as_bool()) {
            if let Err(e) = pip_preview::write_config_key("experimental_pip", Value::Bool(enabled)) {
                return ToolResult::error(format!("failed to persist experimental_pip: {e}"));
            }
            pip_note = format!(" — restart cua-driver for experimental_pip={enabled} to take effect");
        }
        if let Some(geom) = args.opt_str("experimental_pip_geometry") {
            // Validate before persisting so the user gets an immediate error.
            if pip_preview::PipGeometry::parse(&geom).is_none() {
                return ToolResult::error(format!(
                    "experimental_pip_geometry `{geom}` is not a valid WxH or WxH+X+Y string"
                ));
            }
            if let Err(e) = pip_preview::write_config_key("experimental_pip_geometry", Value::String(geom.clone())) {
                return ToolResult::error(format!("failed to persist experimental_pip_geometry: {e}"));
            }
            if pip_note.is_empty() {
                pip_note = format!(" — restart cua-driver for experimental_pip_geometry={geom} to take effect");
            }
        }
        let scope_note = if session_id.is_some() {
            " (session-scoped; persisted default unchanged)"
        } else {
            ""
        };
        ToolResult::text(format!(
            "Config updated: capture_mode={}, max_image_dimension={}{}{}",
            effective_mode, effective_dim, scope_note, pip_note
        ))
    }
}
