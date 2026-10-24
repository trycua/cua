use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::ArgsExt as _,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    coordinate_frame_schema, desktop_frame_requested, desktop_to_window_local, mouse_button_name,
    mouse_hold_json, overlay_glide_to_for, overlay_snap_to_for, parse_mouse_button,
    persistent_pointer_coordinates, resolve_cursor_key, screenshot_scale, window_local_to_screen,
    MouseHoldState, ToolState,
};

// ── mouse_button_down / mouse_drag / mouse_button_up ────────────────────────

pub struct MouseButtonDownTool {
    pub(crate) state: Arc<ToolState>,
}
static MDOWN_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MouseButtonDownTool {
    fn def(&self) -> &ToolDef {
        MDOWN_DEF.get_or_init(|| ToolDef {
            name: "mouse_button_down".into(),
            description: "Press and hold a mouse button at (x,y) via background X11 delivery. \
                Does not release the button; pair with mouse_drag / mouse_button_up. \
                Returns the current held-button state.".into(),
            input_schema: json!({"type":"object","required":["pid","window_id","x","y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema_with(
                    "When both are present, session takes precedence over cursor_id."
                ),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"Window id from list_windows."},
                "x":{"type":"number","description":"Window-local pixel X of the target window's own get_window_state screenshot (0..screenshot_width). For get_desktop_state pixels pass scope:\"desktop\" (or coordinate_frame:\"desktop\")."},
                "y":{"type":"number","description":"Window-local pixel Y of the target window's own get_window_state screenshot (0..screenshot_height); see x."},
                "coordinate_frame": coordinate_frame_schema(),
                "button": cua_driver_core::tool_schema::button_schema(),
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        if let Some(held) = self
            .state
            .mouse_hold
            .lock()
            .unwrap()
            .get(&cursor_id)
            .cloned()
        {
            return ToolResult::error(format!(
                "Cursor '{cursor_id}' already has a held mouse button. Call mouse_button_up first."
            ))
            .with_structured(mouse_hold_json(&cursor_id, Some(&held)));
        }

        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let xid = match args.opt_u64("window_id") {
            Some(v) => v,
            None => return ToolResult::error("window_id is required on Linux."),
        };
        let button_name = args.str_or("button", "left");
        let button = parse_mouse_button(button_name.as_str());
        let mut x = args.f64_or("x", 0.0);
        let mut y = args.f64_or("y", 0.0);
        let desktop_frame = desktop_frame_requested(&args);
        if desktop_frame {
            // Desktop pixels: no zoom/screenshot scaling; translated once below.
        } else if args.bool_or("from_zoom", false) {
            match self.state.zoom_context(&args, pid, Some(xid)) {
                Ok(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(x, y);
                    x = wx;
                    y = wy;
                }
                Err(refusal) => return refusal,
            }
        } else {
            let ratio = match screenshot_scale(&self.state, &args, pid, Some(xid)) {
                Ok(ratio) => ratio,
                Err(refusal) => return refusal,
            };
            x *= ratio;
            y *= ratio;
        }
        if desktop_frame {
            match tokio::task::spawn_blocking(move || desktop_to_window_local(xid, x, y)).await {
                Ok(Ok((lx, ly))) => {
                    x = lx;
                    y = ly;
                }
                Ok(Err(e)) => {
                    return ToolResult::error(format!(
                        "desktop-frame coordinates could not be mapped into window {xid}: {e}"
                    ))
                }
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            }
        }

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Ok(Ok((sx, sy))) =
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
        {
            overlay_glide_to_for(&cursor_id, sx, sy).await;
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
            );
        }

        let xi = x as i32;
        let yi = y as i32;
        let is_wl = crate::wayland::is_wayland();
        // Native Wayland: route through the persistent virtual-pointer module
        // so the held button survives across tool calls; the X11 path keeps
        // the existing input::send_button_down behaviour.
        let result = if is_wl {
            let cid = cursor_id.clone();
            let (output_x, output_y) = persistent_pointer_coordinates(
                is_wl,
                xid,
                xi,
                yi,
                crate::wayland::window_local_to_output,
            );
            tokio::task::spawn_blocking(move || {
                crate::wayland::persistent_vptr::press(&cid, xid, output_x, output_y, button)
            })
            .await
        } else {
            tokio::task::spawn_blocking(move || crate::input::send_button_down(xid, xi, yi, button))
                .await
        };
        match result {
            Ok(Ok(())) => {
                let hold = MouseHoldState {
                    pid,
                    xid,
                    button,
                    x,
                    y,
                };
                self.state
                    .mouse_hold
                    .lock()
                    .unwrap()
                    .insert(cursor_id.clone(), hold.clone());
                if let Ok(Ok((sx, sy))) =
                    tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
                {
                    self.state
                        .cursor_registry
                        .update_position(&cursor_id, sx, sy);
                    overlay_snap_to_for(&cursor_id, sx, sy, None);
                    crate::overlay::send_command_for(
                        cursor_id.clone(),
                        cursor_overlay::OverlayCommand::SetPressed(true),
                    );
                }
                ToolResult::text(format!(
                    "✅ Cursor '{cursor_id}' held {} button down at ({x:.1}, {y:.1}).",
                    mouse_button_name(button),
                ))
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold)))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()).with_structured(mouse_hold_json(
                &cursor_id,
                self.state.mouse_hold.lock().unwrap().get(&cursor_id),
            )),
            Err(e) => {
                ToolResult::error(format!("Task error: {e}")).with_structured(mouse_hold_json(
                    &cursor_id,
                    self.state.mouse_hold.lock().unwrap().get(&cursor_id),
                ))
            }
        }
    }
}
