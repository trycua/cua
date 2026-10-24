use async_trait::async_trait;
use cua_driver_contract::MoveCursorInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{resolve_cursor_key, reveal_pointer_action_for, ToolState};

// ── move_cursor ───────────────────────────────────────────────────────────────

pub struct MoveCursorTool {
    pub(crate) state: Arc<ToolState>,
}

static MCURSOR_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorControlScope {
    Agent,
    Desktop,
}

fn cursor_control_scope(args: &Value) -> CursorControlScope {
    match args.get("scope").and_then(Value::as_str) {
        Some("desktop") => CursorControlScope::Desktop,
        _ => CursorControlScope::Agent,
    }
}

#[async_trait]
impl Tool for MoveCursorTool {
    fn def(&self) -> &ToolDef {
        MCURSOR_DEF.get_or_init(|| ToolDef {
            name: "move_cursor".into(),
            description: "Move the synthetic agent cursor without changing the user's pointer. Only an explicit scope=desktop request moves the real OS pointer in get_desktop_state coordinates.".into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "x":{"type":"number","description":"Destination X. Window scope: screen coordinates of the agent cursor overlay. Desktop scope: get_desktop_state screenshot pixels."},
                "y":{"type":"number","description":"Destination Y, in the same space as x."},
                "session": cua_driver_core::tool_schema::session_schema(),
                "cursor_id":{"type":"string","description":"Cursor instance to move. Default: 'default'."},
                "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"\"window\" (default) moves only the agent cursor overlay; \"desktop\" moves the real OS pointer."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if cursor_control_scope(&args) == CursorControlScope::Desktop {
            let input = match parse_typed_projection::<MoveCursorInput>("move_cursor", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let (x, y) = (input.x, input.y);
            let xi = x.round() as i32;
            let yi = y.round() as i32;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland {
                "wayland_desktop"
            } else {
                "xtest_desktop"
            };
            let result = if wayland {
                tokio::task::spawn_blocking(move || {
                    let space = crate::wayland::DesktopInputSpace::current()?;
                    crate::wayland::move_cursor_desktop(&space, xi, yi)
                })
                .await
            } else {
                tokio::task::spawn_blocking(move || crate::input::send_move_xtest_desktop(xi, yi))
                    .await
            };
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Moved the real desktop pointer to ({xi}, {yi})."
                ))
                .with_structured(
                    json!({"scope":"desktop","path":path,"x":xi,"y":yi,"effect":"unverifiable"}),
                ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let x = args.f64_or("x", 0.0);
        let y = args.f64_or("y", 0.0);
        let cursor_id = resolve_cursor_key(&args);
        // End pointing upper-left (45°) — matches Swift's
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention so the
        // overlay arrow settles to the natural macOS-style pose.
        // Use the acknowledged animation path so a first-ever move seeds and
        // displays the session cursor just as reliably as a coordinate click.
        reveal_pointer_action_for(&self.state, &cursor_id, x, y, false).await;
        ToolResult::text(format!(
            "Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1}); the user pointer was unchanged."
        ))
    }
}

#[cfg(test)]
mod session_cursor_target_tests;
