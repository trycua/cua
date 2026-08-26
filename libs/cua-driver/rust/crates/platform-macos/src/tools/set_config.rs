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

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "set_config".into(),
        description: "Update cua-driver-rs configuration. Changes to \
            max_image_dimension take effect immediately. The \
            agent_view keys are persisted to ~/.cua-driver/config.json and \
            take effect on the next daemon restart (Agent View is \
            initialised once at startup).\n\nNote: capture_mode is a per-call \
            param (on get_window_state / click), not a stored setting. Capture \
            modality is selected by each action's target; the old \
            capture_scope config key is retired.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Name of a single config field to write ({key, value} shape, \
                        matching the CLI `config set` and the Windows/Linux tools). Pair with `value`. \
                        Equivalent to passing the field directly."
                },
                "value": {
                    "description": "New value for `key`. JSON type depends on the key."
                },
                "max_image_dimension": {
                    "type": "integer",
                    "description": "Max dimension for screenshot resizing (0 = no limit)."
                },
                "agent_view": {
                    "type": "boolean",
                    "description": "Enable the multi-target Agent View. Exact native \
                        windows and Chrome tabs are grouped automatically by session; no target \
                        claiming is involved. Applies on next daemon restart."
                },
                "agent_view_geometry": {
                    "type": "string",
                    "description": "Agent View size + optional position in `WxH` or `WxH+X+Y` \
                        form (e.g. `640x420+24+24`). Applies on next daemon restart."
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
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("capture_scope").is_some()
            || args.get("key").and_then(Value::as_str) == Some("capture_scope")
        {
            return ToolResult::error(
                "config key 'capture_scope' is retired; select a window or desktop target on each action",
            )
            .with_structured(serde_json::json!({
                "code": "config_key_retired",
                "key": "capture_scope",
                "replacement": "action.target",
            }));
        }
        // The daemon injects `_session_id` for non-anonymous MCP sessions.
        // Absent => anonymous/global session (CLI one-shot, legacy proxy) =>
        // today's behavior: write the shared global DriverConfig + persist to
        // disk. Present => session-scoped in-memory override only, never
        // touching the global config or the on-disk default, so two concurrent
        // sessions don't clobber each other or the persisted default.
        let session_id = args.opt_str("_session_id");

        // Accept BOTH the direct field and {key,value} shapes.
        let kv: Option<(String, Value)> = args
            .opt_str("key")
            .and_then(|k| args.get("value").map(|v| (k, v.clone())));
        let kv_u64 = |name: &str| -> Option<u64> {
            kv.as_ref()
                .filter(|(k, _)| k == name)
                .and_then(|(_, v)| v.as_u64())
        };
        let kv_bool = |name: &str| -> Option<bool> {
            kv.as_ref()
                .filter(|(k, _)| k == name)
                .and_then(|(_, v)| v.as_bool())
        };
        let kv_str = |name: &str| -> Option<String> {
            kv.as_ref()
                .filter(|(k, _)| k == name)
                .and_then(|(_, v)| v.as_str())
                .map(ToOwned::to_owned)
        };

        // Validate max_image_dimension up front so both branches share the
        // u32 check and we never half-apply.
        let max_dim: Option<u32> = match args
            .opt_u64("max_image_dimension")
            .or_else(|| kv_u64("max_image_dimension"))
        {
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
        // Agent View keys persist to the same config.json but take effect only on
        // next daemon restart — the backend is initialised once at startup.
        let mut agent_view_note = String::new();
        if let Some(enabled) = args
            .get("agent_view")
            .and_then(|v| v.as_bool())
            .or_else(|| kv_bool("agent_view"))
        {
            if let Err(e) = pip_preview::write_config_key("agent_view", Value::Bool(enabled)) {
                return ToolResult::error(format!("failed to persist agent_view: {e}"));
            }
            agent_view_note =
                format!(" — restart cua-driver for agent_view={enabled} to take effect");
        }
        if let Some(geom) = args
            .opt_str("agent_view_geometry")
            .or_else(|| kv_str("agent_view_geometry"))
        {
            // Validate before persisting so the user gets an immediate error.
            if pip_preview::PipGeometry::parse(&geom).is_none() {
                return ToolResult::error(format!(
                    "agent_view_geometry `{geom}` is not a valid WxH or WxH+X+Y string"
                ));
            }
            if let Err(e) =
                pip_preview::write_config_key("agent_view_geometry", Value::String(geom.clone()))
            {
                return ToolResult::error(format!("failed to persist agent_view_geometry: {e}"));
            }
            if agent_view_note.is_empty() {
                agent_view_note =
                    format!(" — restart cua-driver for agent_view_geometry={geom} to take effect");
            }
        }
        let scope_note = if session_id.is_some() {
            " (session-scoped; persisted default unchanged)"
        } else {
            ""
        };
        ToolResult::text(format!(
            "Config updated: max_image_dimension={}{}{}",
            effective_dim, scope_note, agent_view_note
        ))
        .with_structured(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "platform": "macos",
            "max_image_dimension": effective_dim,
        }))
    }
}
