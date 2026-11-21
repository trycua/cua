// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use cua_driver_contract::GetCursorPositionInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::{json, Value};

use crate::routes::{CursorReadout, LinuxCursorReadout as _};

// ── get_cursor_position ───────────────────────────────────────────────────────

pub struct GetCursorPositionTool;
static GCP_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetCursorPositionTool {
    fn def(&self) -> &ToolDef {
        GCP_DEF.get_or_init(|| ToolDef {
            name: "get_cursor_position".into(),
            description:
                "Return the current mouse cursor position in screen points (origin top-left)."
                    .into(),
            input_schema: json!({"type":"object","properties":{
                "session": cua_driver_core::tool_schema::session_schema()
            },"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        if let Err(result) =
            parse_typed_input::<GetCursorPositionInput>("get_cursor_position", args)
        {
            return result;
        }
        let result =
            tokio::task::spawn_blocking(|| crate::routes::routes().cursor_position()).await;
        match result {
            Ok(Ok(CursorReadout::Compositor(x, y))) => {
                ToolResult::text(format!("✅ Cursor at ({x}, {y}) in the desktop frame"))
                    .with_structured(json!({ "x": x, "y": y, "source": "compositor" }))
            }
            Ok(Ok(CursorReadout::Unsupported {
                last_synthetic: Some((x, y)),
            })) => ToolResult::text(format!(
                "✅ Cursor at ({x}, {y}) (synthetic — last move_cursor in this process)"
            ))
            .with_structured(json!({
                "x": x, "y": y, "source": "synthetic"
            })),
            Ok(Ok(CursorReadout::Unsupported {
                last_synthetic: None,
            })) => ToolResult::text(
                "Cursor position unknown on Wayland — no move_cursor has been issued in this process yet.".to_string()
            ).with_structured(json!({ "source": "synthetic", "available": false })),
            // Text format matches Swift `GetCursorPositionTool` 1:1.
            Ok(Ok(CursorReadout::X11(x, y))) => ToolResult::text(format!("✅ Cursor at ({x}, {y})"))
                .with_structured(json!({ "x": x, "y": y, "source": "x11" })),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
