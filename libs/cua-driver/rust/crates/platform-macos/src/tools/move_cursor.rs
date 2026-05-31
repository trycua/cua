use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct MoveCursorTool {
    state: Arc<ToolState>,
}

impl MoveCursorTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "move_cursor".into(),
        description: "Move the agent cursor overlay to (x, y). Does NOT move the real mouse \
            cursor — the user's cursor stays where it is. Useful for showing the agent's \
            attention without interrupting the user.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["x", "y"],
            "properties": {
                "x": { "type": "number" },
                "y": { "type": "number" },
                "cursor_id": { "type": "string", "description": "Cursor instance to move. Default: 'default'." }
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
impl Tool for MoveCursorTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let x = match args.require_f64("x") { Ok(v) => v, Err(e) => return e };
        let y = match args.require_f64("y") { Ok(v) => v, Err(e) => return e };
        let cursor_id = super::cursor_tools::resolve_cursor_key(&args);

        self.state.cursor_registry.update_position(&cursor_id, x, y);
        // Drive the visual overlay for THIS session's cursor (no-op when the
        // overlay is disabled). End pointing upper-left (45°) — matches Swift's
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention.
        crate::cursor::overlay::send_command(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::MoveTo { x, y, end_heading_radians: std::f64::consts::FRAC_PI_4 }
        );
        ToolResult::text(format!("Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1})."))
    }
}
