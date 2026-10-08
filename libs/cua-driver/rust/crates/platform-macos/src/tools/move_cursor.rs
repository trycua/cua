use async_trait::async_trait;
use cua_driver_contract::MoveCursorInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct MoveCursorTool {
    state: Arc<ToolState>,
}

impl MoveCursorTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "move_cursor".into(),
        description: "Move a cursor to (x, y). In window scope (default), moves only the \
            agent cursor overlay. With scope=desktop, moves the real OS pointer in native \
            get_desktop_state screenshot coordinates.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["x", "y"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "x": { "type": "number", "description": "Destination X. Window scope: screen points for the agent cursor overlay. Desktop scope: native get_desktop_state screenshot pixels." },
                "y": { "type": "number", "description": "Destination Y, in the same space as x." },
                "scope": { "type": "string", "enum": ["window", "desktop"], "default": "window", "description": "\"window\" (default) moves only the agent cursor overlay; \"desktop\" moves the real OS pointer." },
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
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop") {
            let input = match parse_typed_projection::<MoveCursorInput>("move_cursor", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let (x, y) = (input.x, input.y);
            let (x, y) = super::desktop_screenshot_point(x, y).await;
            let result =
                tokio::task::spawn_blocking(move || crate::input::mouse::move_cursor_desktop(x, y))
                    .await;
            let note = tokio::task::spawn_blocking(move || inactive_app_under_point(x, y))
                .await
                .ok()
                .flatten()
                .map(|app| inactive_app_note(&app))
                .unwrap_or_default();
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Moved the real desktop pointer to ({x:.1}, {y:.1}).{note}"
                ))
                .with_structured(serde_json::json!({
                    "scope": "desktop",
                    "x": x,
                    "y": y,
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => {
                    ToolResult::error(format!("desktop pointer move failed: {error}"))
                }
                Err(error) => ToolResult::error(format!("desktop pointer task failed: {error}")),
            };
        }
        if !self.state.cursor_overlay_available {
            return super::cursor_overlay_unavailable();
        }
        let x = match args.require_f64("x") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let y = match args.require_f64("y") {
            Ok(v) => v,
            Err(e) => return e,
        };
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
        ToolResult::text(format!(
            "Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1})."
        ))
    }
}

/// The app that owns the frontmost ordinary window under a desktop point,
/// when that app is not the active one. The driver's own windows (cursor
/// overlay) are skipped.
fn inactive_app_under_point(x: f64, y: f64) -> Option<String> {
    let own = std::process::id() as i32;
    let under = crate::windows::visible_windows()
        .into_iter()
        .filter(|w| w.layer == 0 && w.is_on_screen && w.pid != own)
        .filter(|w| {
            let b = &w.bounds;
            x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height
        })
        .max_by_key(|w| w.z_index)?;
    (crate::apps::frontmost_pid() != Some(under.pid)).then_some(under.app_name)
}

/// Hover tracking reaches an inactive app, but AppKit shows native tooltips
/// only in the active one (VM check: 3/3 with the app active, 0/3 without).
fn inactive_app_note(app: &str) -> String {
    format!(
        " {app} is not the active app: hover effects update, but native tooltips only \
         appear in the active app. To read a tooltip, bring_to_front its window first, \
         move the pointer onto the control and wait about a second before looking."
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_inactive_app_note_names_the_route_to_a_tooltip() {
        let note = super::inactive_app_note("BenchLab");
        assert!(note.starts_with(" BenchLab is not the active app"));
        assert!(note.contains("bring_to_front"));
    }
}
