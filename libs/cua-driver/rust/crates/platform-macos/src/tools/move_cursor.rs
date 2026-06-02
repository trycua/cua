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
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
                "x": { "type": "number" },
                "y": { "type": "number" },
                "cursor_id": { "type": "string", "description": "Cursor instance to move. Default: 'default'." }
            },
            "additionalProperties": false
        }),
        // read-only: move_cursor only nudges the agent-cursor overlay, never the
        // target app — so it's safe to run concurrently. The `readOnlyHint` this
        // emits lets MCP clients (e.g. Claude Code's isConcurrencySafe) parallelize
        // cursor moves. (Mutating tools like click stay read_only:false on purpose.)
        read_only: true,
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
        // Drive the DRAWN cursor via the same path as click's animation. A raw
        // `MoveTo` doesn't reliably bring a brand-new session cursor on-screen —
        // it sits at the off-screen sentinel until a click seeds it, so the
        // visible cursor wouldn't move (the reported position would, but the
        // overlay wouldn't). `animate_cursor_to` seeds the sentinel on-screen
        // then glides in, identical to `click`. No-op for an empty (anonymous)
        // key or when the overlay is disabled for this cursor.
        crate::cursor::overlay::animate_cursor_to(cursor_id.clone(), x, y).await;
        ToolResult::text(format!("Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1})."))
    }
}
