use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::ArgsExt as _,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    drag_steps, foreground_budget, held_target_mismatch, mouse_button_name, mouse_hold_json,
    overlay_glide_to_for, overlay_move_to_target_for, overlay_snap_to_for,
    persistent_pointer_coordinates, require_point_args, resolve_cursor_key, screenshot_scale,
    spawn_blocking_bounded, window_local_to_screen, AimedPoint, ToolState,
};

fn overlay_move_to_for(cursor_id: &str, sx: f64, sy: f64, heading: Option<f64>) {
    overlay_move_to_target_for(cursor_id, sx, sy, heading, None);
}

pub struct MouseDragTool {
    pub(crate) state: Arc<ToolState>,
}
static MDRAG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MouseDragTool {
    fn def(&self) -> &ToolDef {
        MDRAG_DEF.get_or_init(|| ToolDef {
            name: "mouse_drag".into(),
            description: "Move a previously-held mouse button to a new point via background X11 delivery. \
                Requires an active mouse_button_down state; does not release the button. \
                Returns the updated held-button state.".into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema_with(
                    "When both are present, session takes precedence over cursor_id."
                ),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer","description":"Optional process ID; must match the held button's target."},
                "window_id":{"type":"integer","description":"Optional window id; must match the held button's target."},
                "x":{"type":"number","description":"Window-local pixel X of the target window's own get_window_state screenshot (0..screenshot_width). For get_desktop_state pixels pass scope:\"desktop\" (or coordinate_frame:\"desktop\")."},
                "y":{"type":"number","description":"Window-local pixel Y of the target window's own get_window_state screenshot (0..screenshot_height); see x."},
                "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Total drag duration. Default: 500."},
                "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Intermediate MotionNotify events. Default: 20."},
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let Some(mut hold) = self
            .state
            .mouse_hold
            .lock()
            .unwrap()
            .get(&cursor_id)
            .cloned()
        else {
            return ToolResult::error(format!(
                "No mouse button is currently held for cursor '{cursor_id}'. Call mouse_button_down first."
            ))
            .with_structured(mouse_hold_json(&cursor_id, None));
        };
        if let Some(err) = held_target_mismatch(&args, &cursor_id, &hold) {
            return err;
        }

        if let Some(refusal) = require_point_args("mouse_drag", &args) {
            return refusal;
        }
        let mut to_x = args.f64_or("x", 0.0);
        let mut to_y = args.f64_or("y", 0.0);
        if args.bool_or("from_zoom", false) {
            match self.state.zoom_context(&args, hold.pid, Some(hold.xid)) {
                Ok(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(to_x, to_y);
                    to_x = wx;
                    to_y = wy;
                }
                Err(refusal) => return refusal,
            }
        } else {
            let ratio = match screenshot_scale(&self.state, &args, hold.pid, Some(hold.xid)) {
                Ok(ratio) => ratio,
                Err(refusal) => return refusal,
            };
            to_x *= ratio;
            to_y *= ratio;
        }

        let xid = hold.xid;

        let from_x = hold.x;
        let from_y = hold.y;
        let duration_ms = args.u64_or("duration_ms", 500);
        let steps = drag_steps(&args);
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Ok(Ok((sx, sy))) =
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, from_x, from_y)).await
        {
            overlay_glide_to_for(&cursor_id, sx, sy).await;
            self.state
                .cursor_registry
                .update_position(&cursor_id, sx, sy);
            overlay_snap_to_for(&cursor_id, sx, sy, None);
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetPressed(true),
            );
        }

        let button = hold.button;
        let step_delay_ms = if steps > 1 {
            duration_ms / steps as u64
        } else {
            duration_ms
        };
        let mut result: anyhow::Result<()> = Ok(());
        let mut moved = 0;
        let mut prev_x = from_x;
        let mut prev_y = from_y;
        let is_wl = crate::wayland::is_wayland();
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let ix = from_x + (to_x - from_x) * t;
            let iy = from_y + (to_y - from_y) * t;
            // Native Wayland: route motion through the persistent virtual-
            // pointer so the held button stays held across the entire drag;
            // X11 keeps the existing input::send_motion path.
            let cid_inner = cursor_id.clone();
            let move_result = if is_wl {
                match persistent_pointer_coordinates(
                    is_wl,
                    xid,
                    ix.round() as i32,
                    iy.round() as i32,
                    crate::wayland::window_local_to_output,
                ) {
                    Ok(AimedPoint {
                        point: (output_x, output_y),
                        ..
                    }) => {
                        spawn_blocking_bounded("mouse_drag", foreground_budget(0), move || {
                            crate::wayland::persistent_vptr::move_to(&cid_inner, output_x, output_y)
                        })
                        .await
                    }
                    Err(error) => Ok(Err(error)),
                }
            } else {
                spawn_blocking_bounded("mouse_drag", foreground_budget(0), move || {
                    crate::input::send_motion(
                        xid,
                        ix.round() as i32,
                        iy.round() as i32,
                        Some(button),
                    )
                })
                .await
            };
            match move_result {
                Ok(Ok(())) => {
                    if let Ok(Ok((sx, sy))) =
                        tokio::task::spawn_blocking(move || window_local_to_screen(xid, ix, iy))
                            .await
                    {
                        let heading = if (ix - prev_x).abs() > f64::EPSILON
                            || (iy - prev_y).abs() > f64::EPSILON
                        {
                            Some((iy - prev_y).atan2(ix - prev_x))
                        } else {
                            None
                        };
                        self.state
                            .cursor_registry
                            .update_position(&cursor_id, sx, sy);
                        overlay_move_to_for(&cursor_id, sx, sy, heading);
                    }
                    prev_x = ix;
                    prev_y = iy;
                    moved = i;
                    if step_delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(step_delay_ms)).await;
                    }
                }
                Ok(Err(e)) => {
                    result = Err(e);
                    break;
                }
                Err(e) => {
                    result = Err(anyhow::anyhow!("Task error: {e}"));
                    break;
                }
            }
        }

        match result {
            Ok(()) => {
                hold.x = to_x;
                hold.y = to_y;
                self.state
                    .mouse_hold
                    .lock()
                    .unwrap()
                    .insert(cursor_id.clone(), hold.clone());
                if let Ok(Ok((sx, sy))) =
                    tokio::task::spawn_blocking(move || window_local_to_screen(xid, to_x, to_y))
                        .await
                {
                    self.state
                        .cursor_registry
                        .update_position(&cursor_id, sx, sy);
                    overlay_snap_to_for(
                        &cursor_id,
                        sx,
                        sy,
                        Some((to_y - from_y).atan2(to_x - from_x)),
                    );
                    crate::overlay::send_command_for(
                        cursor_id.clone(),
                        cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
                    );
                }
                ToolResult::text(format!(
                    "✅ Cursor '{cursor_id}' dragged held {} button to ({to_x:.1}, {to_y:.1}).",
                    mouse_button_name(hold.button),
                ))
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold)))
            }
            Err(e) if e.is::<crate::wayland::persistent_vptr::HeldPointerEnded>() => {
                // The persistent pointer released the button and discarded
                // the cursor: it holds nothing any more.
                self.state.mouse_hold.lock().unwrap().remove(&cursor_id);
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::OverlayCommand::SetPressed(false),
                );
                let mut structured = mouse_hold_json(&cursor_id, None);
                structured["effect"] = json!("partial");
                ToolResult::error(format!(
                    "mouse_drag stopped after {moved} of {steps} steps: {e}"
                ))
                .with_structured(structured)
            }
            Err(e) => ToolResult::error(e.to_string())
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold))),
        }
    }
}
