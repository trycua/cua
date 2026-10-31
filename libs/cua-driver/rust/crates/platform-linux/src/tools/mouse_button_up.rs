use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::ArgsExt as _,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    clear_mouse_hold_after_release, coordinate_frame_schema, desktop_frame_requested,
    desktop_to_window_local, held_target_mismatch, mouse_button_name, mouse_hold_json,
    overlay_glide_to_for, overlay_snap_to_for, resolve_cursor_key, screenshot_scale,
    window_local_to_screen, MouseHoldState, ToolState,
};

#[cfg(test)]
#[test]
fn coordinate_less_mouse_button_up_survives_snapshot_replacement() {
    let state = ToolState::new();
    let cursor_id = "held-after-replacement";
    let hold = MouseHoldState {
        pid: 42,
        xid: 7,
        button: 1,
        x: 120.0,
        y: 80.0,
    };
    state.snapshots.publish_for_session(
        hold.pid as i32,
        hold.xid,
        crate::atspi::snapshot::AtspiSnapshot::from_nodes(&[]),
        Some("press-owner"),
        Some(2.0),
    );
    state
        .mouse_hold
        .lock()
        .unwrap()
        .insert(cursor_id.to_owned(), hold.clone());

    state.snapshots.publish_for_session(
        hold.pid as i32,
        hold.xid,
        crate::atspi::snapshot::AtspiSnapshot::from_nodes(&[]),
        Some("replacement-owner"),
        Some(1.0),
    );

    let release_args = json!({"_session_id": "press-owner"});
    assert_eq!(
        mouse_button_up_coordinates(&state, &release_args, &hold).unwrap(),
        (hold.x, hold.y),
        "coordinate-less release must use the stored native hold point"
    );

    let stale_coordinate_args = json!({"_session_id": "press-owner", "x": 60.0, "y": 40.0});
    let refusal = mouse_button_up_coordinates(&state, &stale_coordinate_args, &hold)
        .expect_err("coordinate-bearing release must still reject a replaced snapshot");
    assert_eq!(
        refusal.structured_content.as_ref().unwrap()["code"],
        "screenshot_context_missing"
    );

    clear_mouse_hold_after_release(&state, cursor_id);
    assert!(state
        .mouse_hold
        .lock()
        .unwrap()
        .insert(cursor_id.to_owned(), hold)
        .is_none());
}

fn mouse_button_up_coordinates(
    state: &ToolState,
    args: &Value,
    hold: &MouseHoldState,
) -> Result<(f64, f64), ToolResult> {
    if args.get("x").is_none() && args.get("y").is_none() {
        return Ok((hold.x, hold.y));
    }

    let mut x = args.opt_f64("x").unwrap_or(hold.x);
    let mut y = args.opt_f64("y").unwrap_or(hold.y);
    if args.bool_or("from_zoom", false) {
        let context = state.zoom_context(args, hold.pid, Some(hold.xid))?;
        return Ok(context.zoom_to_window(x, y));
    }

    let ratio = screenshot_scale(state, args, hold.pid, Some(hold.xid))?;
    x *= ratio;
    y *= ratio;
    Ok((x, y))
}

pub struct MouseButtonUpTool {
    pub(crate) state: Arc<ToolState>,
}
static MUP_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MouseButtonUpTool {
    fn def(&self) -> &ToolDef {
        MUP_DEF.get_or_init(|| ToolDef {
            name: "mouse_button_up".into(),
            description: "Release a previously-held mouse button via background X11 delivery. \
                If x/y are omitted, releases at the last held position. Returns the current held-button state.".into(),
            input_schema: json!({"type":"object","properties":{
                "session": cua_driver_core::tool_schema::session_schema_with(
                    "When both are present, session takes precedence over cursor_id."
                ),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer","description":"Optional process ID; must match the held button's target."},
                "window_id":{"type":"integer","description":"Optional window id; must match the held button's target."},
                "x":{"type":"number","description":"Window-local pixel X of the target window's own get_window_state screenshot (0..screenshot_width). For get_desktop_state pixels pass scope:\"desktop\" (or coordinate_frame:\"desktop\")."},
                "y":{"type":"number","description":"Window-local pixel Y of the target window's own get_window_state screenshot (0..screenshot_height); see x."},
                "coordinate_frame": coordinate_frame_schema(),
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let Some(hold) = self
            .state
            .mouse_hold
            .lock()
            .unwrap()
            .get(&cursor_id)
            .cloned()
        else {
            return ToolResult::error(format!(
                "No mouse button is currently held for cursor '{cursor_id}'."
            ))
            .with_structured(mouse_hold_json(&cursor_id, None));
        };
        if let Some(err) = held_target_mismatch(&args, &cursor_id, &hold) {
            return err;
        }

        let xid = hold.xid;

        let desktop_frame = args.opt_f64("x").is_some()
            && args.opt_f64("y").is_some()
            && desktop_frame_requested(&args);
        let (mut x, mut y) = if desktop_frame {
            // Desktop pixels: no zoom/screenshot scaling; translated below.
            (args.f64_or("x", hold.x), args.f64_or("y", hold.y))
        } else {
            match mouse_button_up_coordinates(&self.state, &args, &hold) {
                Ok(point) => point,
                Err(refusal) => return refusal,
            }
        };
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
                    .with_structured(mouse_hold_json(&cursor_id, Some(&hold)))
                }
                Err(e) => {
                    return ToolResult::error(format!("Task error: {e}"))
                        .with_structured(mouse_hold_json(&cursor_id, Some(&hold)))
                }
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
        }

        let button = hold.button;
        let xi = x as i32;
        let yi = y as i32;
        // Native Wayland: release through the persistent virtual-pointer so
        // the same vptr device that emitted the press also emits the release
        // (single logical drag rather than a click pair).
        let result = if crate::wayland::is_wayland() {
            let cid = cursor_id.clone();
            tokio::task::spawn_blocking(move || {
                crate::wayland::persistent_vptr::release(&cid, button)
            })
            .await
        } else {
            tokio::task::spawn_blocking(move || crate::input::send_button_up(xid, xi, yi, button))
                .await
        };
        match result {
            Ok(Ok(())) => {
                if let Ok(Ok((sx, sy))) =
                    tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
                {
                    self.state
                        .cursor_registry
                        .update_position(&cursor_id, sx, sy);
                    overlay_snap_to_for(&cursor_id, sx, sy, None);
                }
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::OverlayCommand::SetPressed(false),
                );
                clear_mouse_hold_after_release(&self.state, &cursor_id);
                let cleared = mouse_hold_json(&cursor_id, None);
                ToolResult::text(format!(
                    "✅ Cursor '{cursor_id}' released held {} button at ({x:.1}, {y:.1}).",
                    mouse_button_name(button),
                ))
                .with_structured(cleared)
            }
            Ok(Err(e)) => ToolResult::error(e.to_string())
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold))),
            Err(e) => ToolResult::error(format!("Task error: {e}"))
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold))),
        }
    }
}
