use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::ArgsExt as _,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::{
    click::ClickTool,
    impl_::{
        checked_element_local_coords, coordinate_frame_schema, desktop_frame_requested,
        desktop_frame_unmapped, desktop_to_window_local, element_screen_center_and_rect,
        foreground_budget, hyprland_foreground, input_error_result, prove_token_element,
        require_point_args, resolve_cursor_key, reveal_pointer_action_for,
        reveal_pointer_action_target_for, screenshot_scale, spawn_blocking_bounded,
        unavailable_chromium_background, unavailable_gtk_pointer_background,
        unavailable_wayland_focused_input_background, unavailable_webkit_background,
        window_local_to_screen, with_pixel_points, x11_pixel_click_no_focus_steal, AimedPoint,
        PointerRoute, ToolState,
    },
};

// ── right_click ───────────────────────────────────────────────────────────────

pub struct RightClickTool {
    pub(crate) state: Arc<ToolState>,
}
static RCLICK_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for RightClickTool {
    fn def(&self) -> &ToolDef {
        RCLICK_DEF.get_or_init(|| ToolDef {
            name: "right_click".into(),
            description: "Right-click at (x,y) or an element_token (AT-SPI bounds). Background \
                delivery on X11 sends real button events from the session's virtual master pointer \
                (path mpx_pointer); a point covered by another application's window is refused \
                (target_occluded), one covered by this app's own dialog is retargeted to it \
                (retargeted_to), one outside the window is refused (point_outside_window; x/y are \
                window-local pixels, the result reports window_point / screen_point). No focus \
                steal. When a context menu opens, the result names it (`popup: {window_id, bounds, \
                title}`): call get_window_state(pid, window_id=<that id>) to index its menu items \
                and click them by element_token. Provide either (window_id + x/y) or (pid + element_token). \
                After a zoom call, pass from_zoom=true to auto-translate zoom-image coords.".into(),
            input_schema: json!({"type":"object","required":[],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer","description":"Target process ID. Optional with element_token (the token carries the pid)."},
                "window_id":{"type":"integer","description":"Window id from list_windows. Required with x/y or element_index; optional with element_token (the token carries it)."},
                "x":{"type":"number","description":"Window-local pixel X of the target window's own get_window_state screenshot (0..screenshot_width). For get_desktop_state pixels pass scope:\"desktop\" (or coordinate_frame:\"desktop\")."},
                "y":{"type":"number","description":"Window-local pixel Y of the target window's own get_window_state screenshot (0..screenshot_height); see x."},
                "coordinate_frame": coordinate_frame_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_webkit_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_pointer_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
            return refusal;
        }
        if hyprland_foreground(delivery) {
            // Share exact-target validation and the admitted native click lifecycle.
            let mut args = args;
            args["button"] = json!("right");
            args["count"] = json!(1);
            return ClickTool {
                state: self.state.clone(),
            }
            .invoke(args)
            .await;
        }
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let token = match prove_token_element(pid, &resolved, "right_click").await {
            Ok(token) => token,
            Err(refusal) => return refusal,
        };
        let window_id_resolved: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => {
                Some(*window_id)
            }
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        if let Some(token) = token {
            let idx = token.index;
            let xid_hint = window_id_resolved;
            let target = token.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, f64, f64)> {
                checked_element_local_coords(pid, &target, xid_hint)
            })
            .await;
            return match result {
                Ok(Ok((xid, lx, ly))) => {
                    if let Ok(Ok(((sx, sy), rect))) = tokio::task::spawn_blocking(move || {
                        element_screen_center_and_rect(pid, &token, Some(xid))
                    })
                    .await
                    {
                        crate::overlay::send_command_for(
                            cursor_id.clone(),
                            cursor_overlay::OverlayCommand::PinAbove(xid),
                        );
                        reveal_pointer_action_target_for(
                            &self.state,
                            &cursor_id,
                            sx,
                            sy,
                            true,
                            Some(rect),
                        )
                        .await;
                    }
                    let lxi = lx as i32;
                    let lyi = ly as i32;
                    let wayland_point = match crate::wayland::wayland_input_enabled()
                        .then(|| crate::wayland::window_local_to_output(xid, lxi, lyi))
                        .transpose()
                    {
                        Ok(point) => point,
                        Err(error) => return input_error_result(error),
                    };
                    let cursor_id_for_task = cursor_id.clone();
                    let click_result = spawn_blocking_bounded(
                        "right_click",
                        foreground_budget(0),
                        move || -> anyhow::Result<PointerRoute> {
                            if crate::wayland::is_inject_mode() {
                                return crate::wayland::inject_click(pid, xid, lx, ly, 1, 3)
                                    .map(|()| PointerRoute::Wayland);
                            }
                            if crate::wayland::wayland_input_enabled() {
                                let aimed = wayland_point.unwrap_or_else(|| (lxi, lyi).into());
                                return crate::wayland::click(xid, aimed, 1, 3)
                                    .map(|()| PointerRoute::Wayland);
                            }
                            if delivery.is_foreground() {
                                return crate::interaction::with_x11_foreground(
                                    xid,
                                    crate::input::ForegroundOptions::pointer(),
                                    || {
                                        let (sx, sy) =
                                            window_local_to_screen(xid, lxi as f64, lyi as f64)?;
                                        crate::input::send_click_xtest_desktop(
                                            sx.round() as i32,
                                            sy.round() as i32,
                                            3,
                                            1,
                                        )
                                    },
                                )
                                .map(|(value, _report)| value)
                                .map(|()| PointerRoute::Foreground);
                            }
                            x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, lxi, lyi, 3, 1)
                        },
                    )
                    .await;
                    let mode_label = if delivery.is_foreground() {
                        "foreground"
                    } else {
                        "background"
                    };
                    match click_result {
                        Ok(Ok(route)) => ToolResult::text(format!(
                            "Right-clicked element [{idx}] {}",
                            route.text_suffix(mode_label)
                        ))
                        .with_structured(route.structured(mode_label)),
                        Ok(Err(e)) => input_error_result(e),
                        Err(e) => ToolResult::error(format!("Task error: {e}")),
                    }
                }
                Ok(Err(e)) => input_error_result(e.context("AT-SPI bounds failed")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        if let Some(refusal) = require_point_args("right_click", &args) {
            return refusal;
        }
        let xid = match window_id_resolved {
            Some(v) => v,
            None => return ToolResult::error("Provide either element_token or window_id + x/y."),
        };
        let desktop_frame = desktop_frame_requested(&args);
        let from_zoom = args.bool_or("from_zoom", false);
        let mut x = args.f64_or("x", 0.0);
        let mut y = args.f64_or("y", 0.0);
        if desktop_frame {
            // Desktop pixels: no zoom/screenshot scaling; translated once below.
        } else if from_zoom {
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
                Ok(Err(e)) => return desktop_frame_unmapped(xid, e),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            }
        }
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        let wayland_output_point = if crate::wayland::wayland_input_enabled() {
            match crate::wayland::window_local_to_output(xid, x.round() as i32, y.round() as i32) {
                Ok(point) => Some(point),
                Err(error) => return input_error_result(error),
            }
        } else {
            None
        };
        let glide_target = if let Some(AimedPoint {
            point: (sx, sy), ..
        }) = wayland_output_point
        {
            Some((sx as f64, sy as f64))
        } else {
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y))
                .await
                .ok()
                .and_then(|r| r.ok())
        };
        if let Some((sx, sy)) = glide_target {
            reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, true).await;
        }
        let (xi, yi) = (x as i32, y as i32);
        let cursor_id_for_task = cursor_id.clone();
        let result = spawn_blocking_bounded(
            "right_click",
            foreground_budget(0),
            move || -> anyhow::Result<PointerRoute> {
                if crate::wayland::is_inject_mode() {
                    return crate::wayland::inject_click(pid, xid, x, y, 1, 3)
                        .map(|()| PointerRoute::Wayland);
                }
                if crate::wayland::wayland_input_enabled() {
                    let aimed = wayland_output_point.unwrap_or_else(|| (xi, yi).into());
                    return crate::wayland::click(xid, aimed, 1, 3).map(|()| PointerRoute::Wayland);
                }
                if delivery.is_foreground() {
                    return crate::interaction::with_x11_foreground(
                        xid,
                        crate::input::ForegroundOptions::pointer(),
                        || {
                            // Real XTest right-click at the screen point (synthetic
                            // XSendEvent is dropped by GTK/Qt; MPX needs /dev/uinput).
                            // Mirrors the single-click foreground path.
                            if let Ok((sx, sy)) = window_local_to_screen(xid, xi as f64, yi as f64)
                            {
                                crate::input::send_click_xtest_desktop(
                                    sx.round() as i32,
                                    sy.round() as i32,
                                    3,
                                    1,
                                )?;
                                return Ok(());
                            }
                            x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, xi, yi, 3, 1)
                                .map(|_| ())
                        },
                    )
                    .map(|(value, _report)| value)
                    .map(|()| PointerRoute::Foreground);
                }
                x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, xi, yi, 3, 1)
            },
        )
        .await;
        let mode_label = if delivery.is_foreground() {
            "foreground"
        } else {
            "background"
        };
        let outcome = match result {
            Ok(Ok(route)) => ToolResult::text(format!(
                "Right-clicked at ({x:.1}, {y:.1}) {}",
                route.text_suffix(mode_label)
            ))
            .with_structured(route.structured(mode_label)),
            Ok(Err(e)) => input_error_result(e),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        };
        with_pixel_points(
            outcome,
            glide_target.map(|(sx, sy)| (sx.round() as i32, sy.round() as i32)),
            (xi, yi),
        )
    }
}
