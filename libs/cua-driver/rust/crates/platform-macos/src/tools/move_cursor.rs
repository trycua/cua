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
            there; in a background app it also hovers through a route that leaves the real \
            pointer alone where one exists (AXShowAlternateUI on the element under the point, \
            or an Electron page) and says so. Without a target, moves only the agent cursor \
            overlay, in screen points. Other macOS hover and tooltips follow the real pointer, \
            so most background apps see no hover; for a web page in Chrome, Edge or Brave use \
            browser_pointer hover instead. With scope=desktop, moves the user's real OS pointer \
            in native get_desktop_state screenshot coordinates; over a window of an app that is \
            not in front it refuses unless delivery_mode is \"foreground\".".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["x", "y"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "x": { "type": "number", "description": "Destination X. Window target: window screenshot pixels. No target: screen points for the agent cursor overlay. Desktop scope: native get_desktop_state screenshot pixels." },
                "y": { "type": "number", "description": "Destination Y, in the same space as x." },
                "scope": { "type": "string", "enum": ["window", "desktop"], "default": "window", "description": "\"window\" (default) moves only the agent cursor overlay; \"desktop\" moves the real OS pointer." },
                "cursor_id": { "type": "string", "description": "Cursor instance to move. Default: 'default'." },
                "delivery_mode": cua_driver_core::tool_schema::delivery_mode_schema_with("Desktop scope only. Default \"background\": refuses, before moving anything, when the point is over a window of an app that is not in front, since that would move the user's pointer. \"foreground\": move the real pointer anyway; only when nobody is using this screen or the user agreed.")
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
            if let Some(refusal) =
                super::desktop_pointer_guard::check("move_cursor", &args, vec![(x, y)]).await
            {
                return refusal;
            }
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
        if crate::apps::frontmost_pid() == Some(pid) {
            return ToolResult::text(format!(
                "Agent cursor '{cursor_id}' moved over window {wid} at ({x:.0}, {y:.0}); the real \
                 pointer did not move."
            ))
            .with_structured(serde_json::json!({
                "scope": "window",
                "window_id": wid,
                "screen_point": [screen_x, screen_y],
                "hover_delivered": false,
                "effect": "unverifiable"
            }));
        }
        // Background app: try the hover routes that leave the real pointer alone.
        let chromium = tokio::task::spawn_blocking(move || {
            super::type_text::is_chromium_identity(
                &crate::apps::bundle_id_for_pid(pid).unwrap_or_default(),
            )
        })
        .await
        .unwrap_or(false);
        let ax = tokio::task::spawn_blocking(move || {
            crate::input::hover::reveal_alternate_ui(pid, wid, screen_x, screen_y)
        })
        .await
        .ok()
        .flatten();
        if let Some(reveal) = ax {
            let label = if reveal.label.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", reveal.label)
            };
            let revealed = reveal.revealed_new_controls();
            if !revealed && chromium {
                return browser_hover_result(&cursor_id, pid, wid, x, y, screen_x, screen_y);
            }
            let outcome = if revealed {
                format!(
                    "Revealed the hover-only controls of {}{label} through AXShowAlternateUI \
                     ({} new element(s)), without the real pointer, focus or raise. Call \
                     get_window_state to see them; hovering elsewhere hides them again.",
                    reveal.role,
                    reveal.elements_after.saturating_sub(reveal.elements_before)
                )
            } else {
                format!(
                    "Sent AXShowAlternateUI to {}{label}, the background hover route, but no new \
                     control appeared in its accessibility tree, so it may have no hover-only \
                     controls. The real pointer did not move.",
                    reveal.role
                )
            };
            return ToolResult::text(format!(
                "Agent cursor '{cursor_id}' moved over window {wid} at ({x:.0}, {y:.0}). {outcome}"
            ))
            .with_structured(serde_json::json!({
                "scope": "window",
                "window_id": wid,
                "screen_point": [screen_x, screen_y],
                "hover_delivered": true,
                "hover_route": "ax_alternate_ui",
                "alternate_ui_visible": reveal.visible,
                "new_elements": reveal.elements_after.saturating_sub(reveal.elements_before),
                "effect": if revealed { "confirmed" } else { "unverifiable" }
            }));
        }
        if tokio::task::spawn_blocking(move || crate::browser::ElectronJs::is_electron(pid))
            .await
            .unwrap_or(false)
        {
            let js = crate::input::hover::electron_hover_js(wid, local_x, local_y);
            if let Ok(reply) = crate::browser::ElectronJs::execute_main(&js, pid).await {
                if crate::input::hover::electron_hover_delivered(&reply) {
                    return ToolResult::text(format!(
                        "Agent cursor '{cursor_id}' moved over window {wid} at ({x:.0}, {y:.0}). \
                         Hovered the Electron page there with webContents.sendInputEvent, without \
                         the real pointer, focus or raise. Hover effects in the page update; \
                         native tooltips do not show."
                    ))
                    .with_structured(serde_json::json!({
                        "scope": "window",
                        "window_id": wid,
                        "screen_point": [screen_x, screen_y],
                        "hover_delivered": true,
                        "hover_route": "electron_input",
                        "effect": "unverifiable"
                    }));
                }
            }
        }
        if chromium {
            return browser_hover_result(&cursor_id, pid, wid, x, y, screen_x, screen_y);
        }
        ToolResult::text(format!(
            "Agent cursor '{cursor_id}' moved over window {wid} at ({x:.0}, {y:.0}); the real \
             pointer did not move.{BACKGROUND_HOVER_NOTE}"
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

/// A window-target move over a background Chromium browser: the page saw no
/// hover, and the browser tools are the route that delivers one without the
/// user's pointer. Bench v2 PR-03 agents read the generic note's desktop
/// option as the way forward and moved the user's pointer.
fn browser_hover_result(
    cursor_id: &str,
    pid: i32,
    wid: u32,
    x: f64,
    y: f64,
    screen_x: f64,
    screen_y: f64,
) -> ToolResult {
    ToolResult::text(format!(
        "Agent cursor '{cursor_id}' moved over window {wid} at ({x:.0}, {y:.0}); the real \
         pointer did not move and the page saw no hover. {BROWSER_HOVER_NOTE}"
    ))
    .with_structured(serde_json::json!({
        "scope": "window",
        "window_id": wid,
        "screen_point": [screen_x, screen_y],
        "hover_delivered": false,
        "effect": "unverifiable",
        "next_call": {
            "tool": "get_browser_state",
            "arguments": { "pid": pid, "window_id": wid },
        },
    }))
}

/// The browser route for hover on a background web page.
const BROWSER_HOVER_NOTE: &str = "This is a web browser page: hover it with the browser tools, \
     which reach the page without the user's pointer. Call get_browser_state with this pid and \
     window_id, then browser_pointer with action \"hover\" and the element's ref (or viewport \
     x,y) from that snapshot. If get_browser_state refuses, make the browser_prepare call in its \
     detail.next_call. A desktop-scope move_cursor would move the user's pointer and refuses \
     over a background app.";

/// What a window-target move does not do for a background app.
const BACKGROUND_HOVER_NOTE: &str = " The app is in the background, so it sees no hover: \
     macOS apps track hover and show tooltips from the real pointer, and nothing under this \
     point offers a background route (no AXShowAlternateUI, not an Electron page). For a web \
     page in Chrome use browser_pointer hover. A control that only appears on hover, or a \
     tooltip, cannot be revealed without the user's pointer (scope:\"desktop\") or the app in \
     front (bring_to_front); use either only if the user allows it. Look first for the same \
     command elsewhere: a menu item, a context menu, a keyboard shortcut, or help text in \
     get_window_state.";

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
    fn the_browser_hover_note_routes_to_the_browser_tools_not_the_desktop() {
        let result = super::browser_hover_result("default", 8601, 659, 344.0, 450.0, 384.0, 490.0);
        let value = serde_json::to_value(&result).unwrap();
        let text = value["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("the page saw no hover"));
        assert!(text.contains("get_browser_state"));
        assert!(text.contains("browser_pointer with action \"hover\""));
        assert!(text.contains("detail.next_call"));
        assert!(!text.contains("scope:\"desktop\""));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["hover_delivered"], false);
        assert_eq!(structured["next_call"]["tool"], "get_browser_state");
        assert_eq!(structured["next_call"]["arguments"]["pid"], 8601);
    }

    #[test]
    fn desktop_scope_advertises_the_foreground_opt_in() {
        let def = super::def();
        assert!(def
            .description
            .contains("refuses unless delivery_mode is \"foreground\""));
        assert!(def.description.contains("browser_pointer hover"));
        let mode = &def.input_schema["properties"]["delivery_mode"];
        assert_eq!(
            mode["enum"],
            serde_json::json!(["background", "foreground"])
        );
    }

    #[test]
    fn the_inactive_app_note_names_the_route_to_a_tooltip() {
        let note = super::inactive_app_note("BenchLab");
        assert!(note.starts_with(" BenchLab is not the active app"));
        assert!(note.contains("bring_to_front"));
    }
}
