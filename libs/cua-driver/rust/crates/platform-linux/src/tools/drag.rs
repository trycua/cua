use async_trait::async_trait;
use cua_driver_contract::{ClickButton, DragInput};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::{parse_typed_projection, ArgsExt as _},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    coordinate_frame_schema, desktop_frame_requested, desktop_frame_unmapped, desktop_input_space,
    desktop_to_window_local, drag_steps, foreground_budget, foreground_hyprland_refusal,
    hyprland_foreground, hyprland_input_result, input_error_result, invalid_pointer_arguments,
    isolated_hyprland_background, isolated_hyprland_refusal, isolated_hyprland_result,
    isolated_hyprland_task_error, named_session_cursor_key, overlay_glide_to_for,
    overlay_snap_to_for, parse_mouse_button, press_x11_drag_start, resolve_cursor_key,
    screenshot_scale, spawn_blocking_bounded, spawn_isolated_hyprland,
    synthetic_pointer_is_dropped, unavailable_chromium_background,
    unavailable_gtk_pointer_background, unavailable_wayland_focused_input_background,
    unavailable_webkit_background, window_local_to_screen, CoordinateContext, PointerRoute,
    ToolState,
};

fn coordinate_drag_context(
    native_refusal: Option<ToolResult>,
    resolve_context: impl FnOnce() -> Result<CoordinateContext, ToolResult>,
) -> Result<CoordinateContext, ToolResult> {
    if let Some(refusal) = native_refusal {
        return Err(refusal);
    }
    resolve_context()
}

#[cfg(test)]
#[test]
fn coordinate_drag_keeps_wayland_background_refusal_ahead_of_screenshot_context() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let resolved = AtomicBool::new(false);
    let native_refusal = crate::input::delivery::background_unavailable_error(
        crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
    );
    let result = coordinate_drag_context(Some(native_refusal), || {
        resolved.store(true, Ordering::SeqCst);
        Ok(CoordinateContext::Screenshot(7.35))
    })
    .expect_err("unsupported Wayland background delivery must refuse drag");

    assert_eq!(
        result.structured_content.as_ref().unwrap()["code"],
        "background_unavailable"
    );
    assert!(
        !resolved.load(Ordering::SeqCst),
        "drag snapshot context must not be consulted before the native refusal"
    );
}

async fn track_overlay_drag_for(
    cursor_id: String,
    from: (f64, f64),
    to: (f64, f64),
    duration_ms: u64,
    steps: usize,
) {
    track_overlay_drag_with(
        |command| crate::overlay::send_command_for(cursor_id.clone(), command),
        from,
        to,
        duration_ms,
        steps,
    )
    .await;
}

async fn track_overlay_drag_with(
    send: impl Fn(cursor_overlay::OverlayCommand),
    from: (f64, f64),
    to: (f64, f64),
    duration_ms: u64,
    steps: usize,
) {
    send(cursor_overlay::OverlayCommand::SetEnabled(true));
    let _pressed = cursor_overlay::PressedVisualGuard::new(&send);
    let steps = steps.max(1);
    let step_delay = std::time::Duration::from_millis(duration_ms / steps as u64);
    for index in 0..=steps {
        let t = index as f64 / steps as f64;
        let x = from.0 + (to.0 - from.0) * t;
        let y = from.1 + (to.1 - from.1) * t;
        send(cursor_overlay::track_pointer_command(x, y));
        if index < steps && !step_delay.is_zero() {
            tokio::time::sleep(step_delay).await;
        }
    }
}

#[cfg(test)]
#[tokio::test]
async fn cancelled_overlay_drag_releases_visual_press_without_native_commands() {
    use std::{cell::RefCell, future::Future, task::Context};
    let commands = RefCell::new(Vec::new());
    let mut drag = Box::pin(track_overlay_drag_with(
        |command| commands.borrow_mut().push(command),
        (10.0, 20.0),
        (30.0, 40.0),
        2000,
        20,
    ));
    let mut context = Context::from_waker(std::task::Waker::noop());
    assert!(drag.as_mut().poll(&mut context).is_pending());
    assert!(matches!(
        commands.borrow()[1],
        cursor_overlay::OverlayCommand::SetPressed(true)
    ));
    drop(drag);
    let commands = commands.borrow();
    assert_eq!(commands.len(), 4);
    assert!(matches!(
        commands.last(),
        Some(cursor_overlay::OverlayCommand::SetPressed(false))
    ));
}

#[cfg(test)]
#[tokio::test]
async fn completed_overlay_drag_balances_visual_press_once() {
    use std::cell::RefCell;
    let pressed = RefCell::new(Vec::new());
    track_overlay_drag_with(
        |command| {
            if let cursor_overlay::OverlayCommand::SetPressed(value) = command {
                pressed.borrow_mut().push(value);
            }
        },
        (10.0, 20.0),
        (30.0, 40.0),
        0,
        2,
    )
    .await;
    assert_eq!(*pressed.borrow(), vec![true, false]);
}

// ── drag ─────────────────────────────────────────────────────────────────────

/// Typed refusal for a modified drag on native Wayland. The Wayland pointer
/// routes (virtual pointer, libei, and the nested cua-compositor socket) do not
/// hold keyboard modifier state across a pointer gesture, so the drag is
/// refused before any input is dispatched rather than degraded to an
/// unmodified drag. The stable `code` lets callers branch without parsing text.
fn wayland_modified_drag_refusal() -> ToolResult {
    const DETAIL: &str = "the pointer route cannot carry keyboard modifier state";
    ToolResult::error(format!(
        "modified drags are unavailable on native Wayland: {DETAIL}"
    ))
    .with_structured(json!({
        "code": "modified_pointer_unavailable",
        "effect": "refused",
        "verified": false,
        "detail": DETAIL,
    }))
}

pub struct DragTool {
    pub(crate) state: Arc<ToolState>,
}
static DRAG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for DragTool {
    fn has_independent_input_lane(&self, args: &Value) -> bool {
        // This opt-in branch below either uses its own leased synthetic seat
        // or refuses. It never falls back to primary-seat input. All ordinary
        // Linux routes retain the common process-wide input coordinator.
        // Socket availability can change between admission and invocation.
        // Native Hyprland window drags remain fail-closed on their own lane.
        args.opt_str("scope").as_deref() != Some("desktop")
            && !crate::input::delivery::DeliveryMode::from_args(args).is_foreground()
            && crate::wayland::wayland_input_enabled()
            && crate::wayland::hyprland::is_session()
    }

    fn def(&self) -> &ToolDef {
        DRAG_DEF.get_or_init(|| ToolDef {
            name: "drag".into(),
            description: "Press-drag-release gesture from (from_x, from_y) to (to_x, to_y) in \
                          window-local screenshot pixels. Background delivery on X11 is one real held \
                          gesture on the session's virtual master pointer (path mpx_pointer: press, \
                          interpolated motion, release) — it lands on GTK/VCL/Qt/Chromium; a point \
                          covered by another application's window is refused (target_occluded), one \
                          covered by this app's own dialog is retargeted to it. Headless servers fall \
                          back to XSendEvent. duration_ms (default 500), steps (default 20). No focus steal.".into(),
            input_schema: json!({"type":"object","required":["from_x","from_y","to_x","to_y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer","description":"Target process ID. Required unless scope is \"desktop\"."},
                "window_id":{"type":"integer","description":"Target window XID. Required."},
                "from_x":{"type":"number","description":"Drag start X: window-local pixels of the target window's own get_window_state screenshot; pass scope:\"desktop\" for get_desktop_state pixels."},
                "from_y":{"type":"number","description":"Drag start Y (same frame as from_x)."},
                "to_x":{"type":"number","description":"Drag end X (same frame as from_x)."},
                "to_y":{"type":"number","description":"Drag end Y (same frame as from_x)."},
                "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Total drag duration. Default: 500."},
                "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Intermediate MotionNotify events. Default: 20."},
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "button": cua_driver_core::tool_schema::button_schema(),
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."},
                "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"Use \"desktop\" with no pid/window_id for get_desktop_state screen coordinates. Default \"window\"."},
                "coordinate_frame": coordinate_frame_schema(),
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let modifiers: Vec<String> = args.str_array("modifier");
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<DragInput>("drag", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let (from_x, from_y) = (input.from_x, input.from_y);
            let (to_x, to_y) = (input.to_x, input.to_y);
            let button = parse_mouse_button(input.button.unwrap_or(ClickButton::Left).as_str());
            let duration_ms = input.duration_ms.unwrap_or(500).min(10_000);
            let steps = input.steps.unwrap_or(20).clamp(1, 200) as usize;
            let modifiers = input.modifier.unwrap_or_default();
            let wayland = crate::wayland::wayland_input_enabled();
            if wayland && !modifiers.is_empty() {
                return wayland_modified_drag_refusal();
            }
            let path = if wayland { "wayland_desktop" } else { "xtest" };
            let space = match desktop_input_space().await {
                Ok(space) => space,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            // The overlay draws in layout coordinates, like the input below.
            let overlay_point = |x: f64, y: f64| {
                let (lx, ly) = space.to_layout(x.round() as i32, y.round() as i32);
                (f64::from(lx), f64::from(ly))
            };
            let (overlay_from, overlay_to) =
                (overlay_point(from_x, from_y), overlay_point(to_x, to_y));
            let result = spawn_blocking_bounded(
                "desktop drag",
                foreground_budget(steps) + std::time::Duration::from_millis(duration_ms),
                move || {
                    if wayland {
                        crate::wayland::drag_desktop(
                            &space,
                            from_x.round() as i32,
                            from_y.round() as i32,
                            to_x.round() as i32,
                            to_y.round() as i32,
                            steps as u32,
                            duration_ms,
                            button,
                        )
                    } else {
                        let modifier_refs: Vec<&str> =
                            modifiers.iter().map(String::as_str).collect();
                        crate::input::send_drag_xtest_desktop_with_modifiers(
                            from_x.round() as i32,
                            from_y.round() as i32,
                            to_x.round() as i32,
                            to_y.round() as i32,
                            button,
                            duration_ms,
                            steps,
                            &modifier_refs,
                        )
                    }
                },
            );
            let visual_drag = track_overlay_drag_for(
                cursor_id.clone(),
                overlay_from,
                overlay_to,
                duration_ms,
                steps,
            );
            let (result, ()) = tokio::join!(result, visual_drag);
            if matches!(&result, Ok(Ok(()))) {
                self.state
                    .cursor_registry
                    .update_position(&cursor_id, overlay_to.0, overlay_to.1);
            }
            return match result {
                Ok(Ok(())) => ToolResult::text("Dragged on the desktop.").with_structured(
                    json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                ),
                Ok(Err(error)) if crate::input::foreground::error_code(&error).is_some() => {
                    input_error_result(error)
                }
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let xid = match args.opt_u64("window_id") {
            Some(v) => v,
            None => return ToolResult::error("window_id is required on Linux."),
        };
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let isolated_background = isolated_hyprland_background(delivery);
        let native_refusal = if !isolated_background {
            if crate::wayland::wayland_input_enabled() && !modifiers.is_empty() {
                return wayland_modified_drag_refusal();
            }
            if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
                return refusal;
            }
            if let Some(refusal) = unavailable_webkit_background(pid, delivery) {
                return refusal;
            }
            if let Some(refusal) = unavailable_gtk_pointer_background(pid, delivery) {
                return refusal;
            }
            unavailable_wayland_focused_input_background(delivery, true)
        } else {
            None
        };

        let from_zoom = args.bool_or("from_zoom", false);
        let desktop_frame = desktop_frame_requested(&args);
        let context = match coordinate_drag_context(native_refusal, || {
            if desktop_frame {
                // Desktop pixels: no zoom/screenshot scaling; translated below.
                Ok(CoordinateContext::Screenshot(1.0))
            } else if from_zoom {
                self.state
                    .zoom_context(&args, pid, Some(xid))
                    .map(CoordinateContext::Zoom)
            } else {
                screenshot_scale(&self.state, &args, pid, Some(xid))
                    .map(CoordinateContext::Screenshot)
            }
        }) {
            Ok(context) => context,
            Err(refusal) => return refusal,
        };

        let coerce = |key: &str| -> Option<f64> {
            args.opt_f64(key)
                .or_else(|| args.opt_i64(key).map(|i| i as f64))
        };
        let mut from_x = match coerce("from_x") {
            Some(v) => v,
            None => {
                return invalid_pointer_arguments(
                    "drag",
                    "from_x is missing (from_x/from_y/to_x/to_y are all required)",
                )
            }
        };
        let mut from_y = match coerce("from_y") {
            Some(v) => v,
            None => {
                return invalid_pointer_arguments(
                    "drag",
                    "from_y is missing (from_x/from_y/to_x/to_y are all required)",
                )
            }
        };
        let mut to_x = match coerce("to_x") {
            Some(v) => v,
            None => return ToolResult::error("Missing: to_x"),
        };
        let mut to_y = match coerce("to_y") {
            Some(v) => v,
            None => return ToolResult::error("Missing: to_y"),
        };

        let duration_ms = args.u64_or("duration_ms", 500);
        let steps = drag_steps(&args);
        let button_str = args.str_or("button", "left");
        let button = parse_mouse_button(button_str.as_str());
        match context {
            CoordinateContext::Zoom(context) => {
                (from_x, from_y) = context.zoom_to_window(from_x, from_y);
                (to_x, to_y) = context.zoom_to_window(to_x, to_y);
            }
            CoordinateContext::Screenshot(scale) => {
                from_x *= scale;
                from_y *= scale;
                to_x *= scale;
                to_y *= scale;
            }
        }
        if desktop_frame {
            let mapped = tokio::task::spawn_blocking(move || {
                Ok::<_, anyhow::Error>((
                    desktop_to_window_local(xid, from_x, from_y)?,
                    desktop_to_window_local(xid, to_x, to_y)?,
                ))
            })
            .await;
            match mapped {
                Ok(Ok(((fx, fy), (tx, ty)))) => {
                    from_x = fx;
                    from_y = fy;
                    to_x = tx;
                    to_y = ty;
                }
                Ok(Err(e)) => return desktop_frame_unmapped(xid, e),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            }
        }

        if hyprland_foreground(delivery) {
            if button_str != "left" || args.get("modifier").is_some_and(|value| !value.is_null()) {
                return foreground_hyprland_refusal(
                    "foreground drag supports only an unmodified left button",
                );
            }
            if !crate::wayland::hyprland_input::enabled() {
                return foreground_hyprland_refusal(
                    "production Hyprland input plugin is unavailable",
                );
            }
            let owner = named_session_cursor_key(&args);
            let points = crate::wayland::window_local_to_output(
                xid,
                from_x.round() as i32,
                from_y.round() as i32,
            )
            .and_then(|from| {
                crate::wayland::window_local_to_output(
                    xid,
                    to_x.round() as i32,
                    to_y.round() as i32,
                )
                .map(|to| (from.point, to.point))
            });
            let (from, to) = match points {
                Ok(points) => points,
                Err(error) => return input_error_result(error),
            };
            let (started, acknowledged) = tokio::sync::oneshot::channel();
            let (_cancellation, mut dispatch) =
                match spawn_isolated_hyprland(&args, move |cancellation| {
                    crate::wayland::hyprland_input::execute_foreground_with_started(
                        owner,
                        pid,
                        xid,
                        crate::wayland::hyprland_input::Action::Drag {
                            from: (from_x, from_y),
                            to: (to_x, to_y),
                            duration_ms,
                        },
                        Some(started),
                        cancellation,
                    )
                }) {
                    Ok(dispatch) => dispatch,
                    Err(_) => {
                        return foreground_hyprland_refusal(
                            "authenticated admitted lifecycle required",
                        )
                    }
                };
            let acknowledged = acknowledged.await.is_ok();
            let result = if acknowledged {
                overlay_snap_to_for(&cursor_id, from.0 as f64, from.1 as f64, None);
                tokio::select! {
                    result = &mut dispatch => result,
                    () = track_overlay_drag_for(cursor_id.clone(), (from.0 as f64, from.1 as f64),
                        (to.0 as f64, to.1 as f64), duration_ms, steps) => dispatch.await,
                }
            } else {
                dispatch.await
            };
            return hyprland_input_result(
                match result {
                    Ok(result) => result,
                    Err(error) => Err(crate::wayland::hyprland_input::unknown_dispatch(
                        error.into(),
                        u32::from(acknowledged),
                    )),
                },
                true,
            );
        }

        if isolated_background {
            if button_str != "left" || args.get("modifier").is_some_and(|value| !value.is_null()) {
                return isolated_hyprland_refusal(
                    "isolated drag supports only an unmodified left button",
                );
            }
            let owner = named_session_cursor_key(&args);
            let points = crate::wayland::window_local_to_output(
                xid,
                from_x.round() as i32,
                from_y.round() as i32,
            )
            .and_then(|from| {
                crate::wayland::window_local_to_output(
                    xid,
                    to_x.round() as i32,
                    to_y.round() as i32,
                )
                .map(|to| (from.point, to.point))
            });
            let (from, to) = match points {
                Ok(points) => points,
                Err(error) => return input_error_result(error),
            };
            let (started, acknowledged) = tokio::sync::oneshot::channel();
            let (_cancellation, mut dispatch) =
                match spawn_isolated_hyprland(&args, move |cancellation| {
                    crate::wayland::hyprland_input::execute_with_started(
                        owner,
                        pid,
                        xid,
                        crate::wayland::hyprland_input::Action::Drag {
                            from: (from_x, from_y),
                            to: (to_x, to_y),
                            duration_ms,
                        },
                        Some(started),
                        cancellation,
                    )
                }) {
                    Ok(dispatch) => dispatch,
                    Err(refusal) => return refusal,
                };
            // Do not animate a drag that was refused. The compositor starts
            // the overlay clock only after accepting and pressing the button.
            let acknowledged = acknowledged.await.is_ok();
            if acknowledged {
                overlay_snap_to_for(&cursor_id, from.0 as f64, from.1 as f64, None);
                tokio::select! {
                    result = &mut dispatch => {
                        return match result { Ok(result) => isolated_hyprland_result(result), Err(error) => isolated_hyprland_task_error(error, acknowledged) };
                    }
                    () = track_overlay_drag_for(cursor_id.clone(), (from.0 as f64, from.1 as f64),
                        (to.0 as f64, to.1 as f64), duration_ms, steps) => {}
                }
            }
            return match dispatch.await {
                Ok(result) => isolated_hyprland_result(result),
                Err(error) => isolated_hyprland_task_error(error, acknowledged),
            };
        }

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        let wayland_points = match crate::wayland::wayland_input_enabled()
            .then(|| {
                crate::wayland::window_local_to_output(
                    xid,
                    from_x.round() as i32,
                    from_y.round() as i32,
                )
                .and_then(|from| {
                    crate::wayland::window_local_to_output(
                        xid,
                        to_x.round() as i32,
                        to_y.round() as i32,
                    )
                    .map(|to| (from, to))
                })
            })
            .transpose()
        {
            Ok(points) => points,
            Err(error) => return input_error_result(error),
        };
        let screen_from = if let Some((from, _)) = wayland_points {
            Some((from.point.0 as f64, from.point.1 as f64))
        } else {
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, from_x, from_y))
                .await
                .ok()
                .and_then(|result| result.ok())
        };
        if let Some((sx_from, sy_from)) = screen_from {
            overlay_glide_to_for(&cursor_id, sx_from, sy_from).await;
            self.state
                .cursor_registry
                .update_position(&cursor_id, sx_from, sy_from);
            overlay_snap_to_for(&cursor_id, sx_from, sy_from, None);
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: sx_from,
                    y: sy_from,
                },
            );
        }
        // Native Wayland: emit press + interpolated motion + release as one
        // virtual-pointer (wlroots) or libei (GNOME/KDE) sequence, output-relative
        // coords. Returns early so we don't fall into the X11 XSendEvent loop below.
        if crate::wayland::wayland_input_enabled() {
            let steps_u32 = steps as u32;
            let inject = crate::wayland::is_inject_mode();
            let (from_aimed, to_aimed) = wayland_points.unwrap_or_else(|| {
                (
                    (from_x.round() as i32, from_y.round() as i32).into(),
                    (to_x.round() as i32, to_y.round() as i32).into(),
                )
            });
            let drag_result = spawn_blocking_bounded(
                "drag",
                foreground_budget(steps) + std::time::Duration::from_millis(duration_ms),
                move || {
                    if inject {
                        return crate::wayland::inject_drag(
                            pid,
                            xid,
                            (from_x, from_y),
                            (to_x, to_y),
                            steps,
                            button as u32,
                        );
                    }
                    crate::interaction::with_target_foreground(
                        pid,
                        xid,
                        crate::interaction::pointer_gesture_restore(),
                        || {
                            crate::wayland::drag(
                                xid,
                                from_aimed,
                                to_aimed,
                                steps_u32,
                                duration_ms,
                                button,
                            )
                        },
                    )
                },
            );
            let ((from_output_x, from_output_y), (to_output_x, to_output_y)) =
                (from_aimed.point, to_aimed.point);
            let visual_drag = track_overlay_drag_for(
                cursor_id.clone(),
                (f64::from(from_output_x), f64::from(from_output_y)),
                (f64::from(to_output_x), f64::from(to_output_y)),
                duration_ms,
                steps,
            );
            let (drag_result, ()) = tokio::join!(drag_result, visual_drag);
            if matches!(&drag_result, Ok(Ok(()))) {
                self.state.cursor_registry.update_position(
                    &cursor_id,
                    f64::from(to_output_x),
                    f64::from(to_output_y),
                );
            }
            return match drag_result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Posted drag ({button_str}) to pid {pid} \
                     from ({from_x:.0}, {from_y:.0}) → ({to_x:.0}, {to_y:.0}) \
                     in {duration_ms}ms / {steps} steps."
                )),
                Ok(Err(e)) => input_error_result(e),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        if delivery.is_foreground() {
            let screen_points = tokio::task::spawn_blocking(move || {
                Ok::<_, anyhow::Error>((
                    window_local_to_screen(xid, from_x, from_y)?,
                    window_local_to_screen(xid, to_x, to_y)?,
                ))
            })
            .await;
            let ((screen_from_x, screen_from_y), (screen_to_x, screen_to_y)) = match screen_points {
                Ok(Ok(points)) => points,
                Ok(Err(e)) => return ToolResult::error(e.to_string()),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            };
            let drag_result = spawn_blocking_bounded(
                "foreground drag",
                foreground_budget(steps) + std::time::Duration::from_millis(duration_ms),
                move || {
                    let modifier_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                    crate::interaction::with_x11_foreground(
                        xid,
                        crate::input::ForegroundOptions::pointer(),
                        || {
                            crate::input::send_drag_xtest_desktop_with_modifiers(
                                screen_from_x.round() as i32,
                                screen_from_y.round() as i32,
                                screen_to_x.round() as i32,
                                screen_to_y.round() as i32,
                                button,
                                duration_ms,
                                steps,
                                &modifier_refs,
                            )
                        },
                    )
                    .map(|(value, _report)| value)
                },
            );
            let visual_drag = track_overlay_drag_for(
                cursor_id.clone(),
                (screen_from_x, screen_from_y),
                (screen_to_x, screen_to_y),
                duration_ms,
                steps,
            );
            let (drag_result, ()) = tokio::join!(drag_result, visual_drag);
            if matches!(&drag_result, Ok(Ok(()))) {
                self.state
                    .cursor_registry
                    .update_position(&cursor_id, screen_to_x, screen_to_y);
            }
            return match drag_result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Dragged ({button_str}) to pid {pid} from ({from_x:.0}, {from_y:.0}) \
                     to ({to_x:.0}, {to_y:.0}) in {duration_ms}ms / {steps} steps \
                     (delivery_mode=foreground)."
                ))
                .with_structured(json!({
                    "path": "x11_xtest_fg",
                    "verified": false,
                    "delivery_mode": "foreground"
                })),
                Ok(Err(e)) => input_error_result(e),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Background X11 drag. With a real Xorg server and /dev/uinput the
        // gesture is a held press + interpolated motion + release on the
        // session's MPX virtual master pointer (GTK/VCL/Qt/Chromium drop the
        // synthetic XSendEvent gesture below); the target's screen points are
        // resolved once and the agent cursor tracks the glide.
        if crate::input::real_pointer_input_available() {
            let screen_points = tokio::task::spawn_blocking(move || {
                Ok::<_, anyhow::Error>((
                    window_local_to_screen(xid, from_x, from_y)?,
                    window_local_to_screen(xid, to_x, to_y)?,
                ))
            })
            .await;
            let ((sfx, sfy), (stx, sty)) = match screen_points {
                Ok(Ok(points)) => points,
                Ok(Err(e)) => return ToolResult::error(format!("{e:#}")),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            };
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetPressed(true),
            );
            let cursor_id_for_task = cursor_id.clone();
            let drag_result = spawn_blocking_bounded(
                "drag",
                foreground_budget(steps) + std::time::Duration::from_millis(duration_ms),
                move || {
                    let (outcome, guard) =
                        crate::input::focus_guard::guarded_settled(Some(pid), || {
                            Ok(crate::input::send_virtual_pointer_drag(
                                &cursor_id_for_task,
                                &crate::input::VirtualPointerDrag {
                                    target_window: xid,
                                    button,
                                    path: vec![
                                        (sfx.round() as i32, sfy.round() as i32),
                                        (stx.round() as i32, sty.round() as i32),
                                    ],
                                    duration_ms,
                                    steps,
                                },
                            ))
                        })?;
                    outcome.map(|mut effect| {
                        effect.focus_guard = guard;
                        effect
                    })
                },
            )
            .await;
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetPressed(false),
            );
            if matches!(&drag_result, Ok(Ok(_))) {
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::track_pointer_command(stx, sty),
                );
                self.state
                    .cursor_registry
                    .update_position(&cursor_id, stx, sty);
            }
            return match drag_result {
                Ok(Ok(effect)) => {
                    let route = PointerRoute::Mpx(effect);
                    ToolResult::text(format!(
                        "Dragged ({button_str}) pid {pid} from ({from_x:.0}, {from_y:.0}) to \
                         ({to_x:.0}, {to_y:.0}) in {duration_ms}ms / {steps} steps with a real \
                         held pointer gesture {}",
                        route.text_suffix("background")
                    ))
                    .with_structured(route.structured("background"))
                }
                Ok(Err(e)) => input_error_result(e),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Legacy target-addressed XSendEvent gesture (headless servers / no
        // uinput): only core-protocol toolkits accept it.
        if synthetic_pointer_is_dropped(pid) {
            return crate::input::delivery::background_unavailable_error(
                crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
            );
        }
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::SetPressed(true),
        );
        let press_result = press_x11_drag_start(
            foreground_budget(0),
            move || {
                crate::input::send_button_down(
                    xid,
                    from_x.round() as i32,
                    from_y.round() as i32,
                    button,
                )
            },
            move || {
                crate::input::send_button_up(
                    xid,
                    from_x.round() as i32,
                    from_y.round() as i32,
                    button,
                )
            },
        )
        .await;
        let mut result: anyhow::Result<()> = match press_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(anyhow::anyhow!("Task error: {e}")),
        };

        if result.is_ok() {
            let step_delay_ms = if steps > 1 {
                duration_ms / steps as u64
            } else {
                duration_ms
            };
            for i in 1..=steps {
                let t = i as f64 / steps.max(1) as f64;
                let ix = from_x + (to_x - from_x) * t;
                let iy = from_y + (to_y - from_y) * t;
                let motion_result =
                    spawn_blocking_bounded("drag", foreground_budget(0), move || {
                        crate::input::send_motion(
                            xid,
                            ix.round() as i32,
                            iy.round() as i32,
                            Some(button),
                        )
                    })
                    .await;
                match motion_result {
                    Ok(Ok(())) => {
                        if let Ok(Ok((sx, sy))) =
                            tokio::task::spawn_blocking(move || window_local_to_screen(xid, ix, iy))
                                .await
                        {
                            self.state
                                .cursor_registry
                                .update_position(&cursor_id, sx, sy);
                            crate::overlay::send_command_for(
                                cursor_id.clone(),
                                cursor_overlay::track_pointer_command(sx, sy),
                            );
                        }
                        if step_delay_ms > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(step_delay_ms))
                                .await;
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
        }

        // XSendEvent does not read the call's action, so the release still
        // goes out if the call ends first.
        let release_result = spawn_blocking_bounded("drag", foreground_budget(0), move || {
            crate::input::send_button_up(xid, to_x.round() as i32, to_y.round() as i32, button)
        })
        .await;
        if result.is_ok() {
            result = match release_result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(anyhow::anyhow!("Task error: {e}")),
            };
        }
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::SetPressed(false),
        );

        if result.is_ok() {
            if let Ok(Ok((sx_to, sy_to))) =
                tokio::task::spawn_blocking(move || window_local_to_screen(xid, to_x, to_y)).await
            {
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::track_pointer_command(sx_to, sy_to),
                );
                self.state
                    .cursor_registry
                    .update_position(&cursor_id, sx_to, sy_to);
            }
        }

        let route = PointerRoute::Synthetic;
        match result {
            Ok(()) => ToolResult::text(format!(
                "Posted drag ({button_str}) to pid {pid} \
                 from ({from_x:.0}, {from_y:.0}) → ({to_x:.0}, {to_y:.0}) \
                 in {duration_ms}ms / {steps} steps {}",
                route.text_suffix("background")
            ))
            .with_structured(route.structured("background")),
            Err(e) => input_error_result(e),
        }
    }
}
