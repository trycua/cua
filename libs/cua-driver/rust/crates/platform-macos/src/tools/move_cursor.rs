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
        description: "Move a cursor to (x, y). With target {kind: \"window\", pid, window_id}, \
            x/y are window screenshot pixels (as for click) and only the agent cursor moves \
            there. Without a target, moves only the agent cursor overlay, in screen points. \
            Neither sends input: macOS apps track hover and show tooltips from the real pointer, \
            so a background app sees no hover. With scope=desktop, moves the real OS pointer in \
            native get_desktop_state screenshot coordinates.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["x", "y"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "x": { "type": "number", "description": "Destination X. Window target: window screenshot pixels. No target: screen points for the agent cursor overlay. Desktop scope: native get_desktop_state screenshot pixels." },
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
        if let (Some(pid), Some(wid)) = (args.opt_i64("pid"), args.opt_u64("window_id")) {
            return self.move_over_window(&args, pid as i32, wid as u32).await;
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

impl MoveCursorTool {
    /// `target: {kind: "window", pid, window_id}`: x/y are window-local
    /// screenshot pixels, as for `click`. Moves the agent cursor there. It
    /// sends no input: AppKit derives hover (tracking areas, tooltips) from
    /// the real pointer, and pointer-move events posted to a background app
    /// do not reach its tracking areas (VM check, 11 Oct 2026: BenchLab saw no
    /// hover through the SkyLight, public or both routes).
    async fn move_over_window(&self, args: &Value, pid: i32, wid: u32) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if !self.state.cursor_overlay_available {
            return super::cursor_overlay_unavailable();
        }
        let (x, y) = match (args.require_f64("x"), args.require_f64("y")) {
            (Ok(x), Ok(y)) => (x, y),
            (Err(e), _) | (_, Err(e)) => return e,
        };
        let ratio = match super::screenshot_scale(&self.state, args, pid, Some(wid)) {
            Ok(ratio) => ratio,
            Err(refusal) => return refusal,
        };
        let frame = match super::px_frame::resolve_or_refuse(wid).await {
            Ok(frame) => frame,
            Err(refusal) => return refusal,
        };
        let (screen_x, screen_y, local_x, local_y) = frame.to_screen(x * ratio, y * ratio);
        if local_x < 0.0
            || local_y < 0.0
            || local_x > frame.bounds.width
            || local_y > frame.bounds.height
        {
            return ToolResult::error(format!(
                "move_cursor: window-local point ({local_x:.1}, {local_y:.1}) pt lies outside \
                 window {wid}'s {:.0}×{:.0} pt frame. Re-read coordinates from a fresh \
                 get_window_state screenshot.",
                frame.bounds.width, frame.bounds.height
            ));
        }
        let cursor_id = super::cursor_tools::resolve_cursor_key(args);
        crate::cursor::overlay::send_command(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(wid as u64),
        );
        crate::cursor::overlay::animate_cursor_to(cursor_id.clone(), screen_x, screen_y).await;
        self.state
            .cursor_registry
            .update_position(&cursor_id, screen_x, screen_y);
        let note = if crate::apps::frontmost_pid() == Some(pid) {
            ""
        } else {
            BACKGROUND_HOVER_NOTE
        };
        ToolResult::text(format!(
            "Agent cursor '{cursor_id}' moved over window {wid} at ({x:.0}, {y:.0}); the real \
             pointer did not move.{note}"
        ))
        .with_structured(serde_json::json!({
            "scope": "window",
            "window_id": wid,
            "screen_point": [screen_x, screen_y],
            "hover_delivered": false,
            "effect": "unverifiable"
        }))
    }
}

/// What a window-target move does not do for a background app.
const BACKGROUND_HOVER_NOTE: &str = " The app is in the background, so it sees no hover: \
     macOS apps track hover and show tooltips from the real pointer, and no background route \
     reaches them. A control that only appears on hover, or a tooltip, cannot be revealed \
     without the user's pointer (scope:\"desktop\") or the app in front (bring_to_front); use \
     either only if the user allows it. Look first for the same command elsewhere: a menu \
     item, a context menu, a keyboard shortcut, or help text in get_window_state.";

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
    fn the_background_hover_note_names_both_routes_and_the_alternatives() {
        let note = super::BACKGROUND_HOVER_NOTE;
        assert!(note.contains("scope:\"desktop\""));
        assert!(note.contains("bring_to_front"));
        assert!(note.contains("only if the user allows it"));
    }

    #[test]
    fn the_inactive_app_note_names_the_route_to_a_tooltip() {
        let note = super::inactive_app_note("BenchLab");
        assert!(note.starts_with(" BenchLab is not the active app"));
        assert!(note.contains("bring_to_front"));
    }
}
