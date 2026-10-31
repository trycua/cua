use async_trait::async_trait;
use cua_driver_contract::MoveCursorInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{resolve_cursor_key, ToolState};

// ── move_cursor ───────────────────────────────────────────────────────────────

pub struct MoveCursorTool {
    pub(crate) state: Arc<ToolState>,
}

static MCURSOR_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MoveCursorTool {
    fn def(&self) -> &ToolDef {
        MCURSOR_DEF.get_or_init(|| ToolDef {
            name: "move_cursor".into(),
            description:
                "Move the agent cursor overlay to (x, y). Does NOT move the real mouse cursor."
                    .into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "scope":{"type":"string","enum":["window","desktop"],"description":"desktop moves the real OS pointer; window moves only the agent overlay."},
                "x":{"type":"number","description":"Destination X. Window scope: screen coordinates of the agent cursor overlay. Desktop scope: get_desktop_state screenshot pixels."},
                "y":{"type":"number","description":"Destination Y, in the same space as x."},
                "cursor_id":{"type":"string","description":"Cursor instance to move. Default: 'default'."}
            },"additionalProperties":false}),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("scope").and_then(Value::as_str) == Some("desktop") {
            let input = match parse_typed_projection::<MoveCursorInput>("move_cursor", &args) {
                Ok(input) => input,
                Err(error) => return error,
            };
            let x = input.x;
            let y = input.y;
            return match crate::input::mouse::move_cursor_desktop(x as i32, y as i32) {
                Ok(()) => ToolResult::text(format!(
                    "Moved the OS pointer to ({x:.1}, {y:.1}) (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SetCursorPos", "x": x, "y": y
                })),
                Err(error) => ToolResult::error(error.to_string()),
            };
        }
        let x = args.f64_or("x", 0.0);
        let y = args.f64_or("y", 0.0);
        // The trusted dispatch boundary always supplies a lifecycle key,
        // including for an unnamed implicit session.
        let cursor_key = resolve_cursor_key(&args);
        if !cursor_key.is_empty() {
            self.state
                .cursor_registry
                .update_position(&cursor_key, x, y);
        }
        // End pointing upper-left (45°) — matches Swift's
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention so
        // the cursor settles to the natural macOS-style pose.
        // Use the acknowledged animation path so a first-ever move seeds and
        // displays the session cursor just as reliably as a coordinate click.
        crate::overlay::animate_cursor_to(cursor_key.clone(), x, y).await;
        let shown = if cursor_key.is_empty() {
            "default"
        } else {
            cursor_key.as_str()
        };
        ToolResult::text(format!("Agent cursor '{shown}' moved to ({x:.1}, {y:.1})."))
    }
}
