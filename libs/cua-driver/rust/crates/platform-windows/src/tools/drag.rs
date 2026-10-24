use async_trait::async_trait;
use cua_driver_contract::{ClickButton, DragInput};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    bitmap_to_screen, overlay_glide_to, pin_overlay_above, resolve_cursor_key,
    restore_foreground_polling_best_effort, screenshot_scale, winui3_background_gesture, ToolState,
};

async fn track_overlay_drag(
    key: String,
    from: (f64, f64),
    to: (f64, f64),
    duration_ms: u64,
    steps: usize,
) {
    if key.is_empty() || !crate::overlay::is_enabled(&key) {
        return;
    }
    crate::overlay::send_command(
        key.clone(),
        cursor_overlay::OverlayCommand::SetPressed(true),
    );
    let steps = steps.max(1);
    let step_delay = std::time::Duration::from_millis(duration_ms / steps as u64);
    for index in 0..=steps {
        let t = index as f64 / steps as f64;
        let x = from.0 + (to.0 - from.0) * t;
        let y = from.1 + (to.1 - from.1) * t;
        crate::overlay::send_command(key.clone(), cursor_overlay::track_pointer_command(x, y));
        if index < steps && !step_delay.is_zero() {
            tokio::time::sleep(step_delay).await;
        }
    }
    crate::overlay::send_command(key, cursor_overlay::OverlayCommand::SetPressed(false));
}

// ── drag ─────────────────────────────────────────────────────────────────────

pub struct DragTool {
    pub(crate) state: Arc<ToolState>,
}

static DRAG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for DragTool {
    fn def(&self) -> &ToolDef {
        DRAG_DEF.get_or_init(|| ToolDef {
            name: "drag".into(),
            description: "Press-drag-release gesture from (from_x, from_y) to (to_x, to_y) in window-local screenshot pixels. \
                          duration_ms (default 500) is the wall-clock budget; steps (default 20) interpolates intermediate \
                          WM_MOUSEMOVE events along the path. Background delivery does not steal focus; delivery_mode:\"foreground\" \
                          activates the target and refuses with foreground_unavailable, sending nothing, when another window still \
                          covers the start point. After a zoom call, pass from_zoom=true. \
                          Drags that start on a window caption / title bar or resize border (i.e. moving or resizing the \
                          window itself) cannot be delivered in the background — the OS move/resize loop needs real pointer \
                          input — so they return background_unavailable; re-issue those with delivery_mode:\"foreground\".".into(),
            input_schema: json!({"type":"object","required":["from_x","from_y","to_x","to_y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "scope":{"type":"string","enum":["window","desktop"],"description":"Use desktop with no pid/window_id for screen-absolute coordinates."},
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"Target window handle (HWND). Optional — driver picks frontmost window of pid when omitted."},
                "from_x":{"type":"number","description":"Drag-start X in window-local screenshot pixels."},
                "from_y":{"type":"number","description":"Drag-start Y in window-local screenshot pixels."},
                "to_x":{"type":"number","description":"Drag-end X in window-local screenshot pixels."},
                "to_y":{"type":"number","description":"Drag-end Y in window-local screenshot pixels."},
                "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Wall-clock duration of drag path. Default: 500."},
                "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Number of intermediate WM_MOUSEMOVE events. Default: 20."},
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "button": cua_driver_core::tool_schema::button_schema(),
                "from_zoom":{"type":"boolean","description":"When true, coordinates are in the last zoom image for this pid."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_key = resolve_cursor_key(&args);
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<DragInput>("drag", &args) {
                Ok(input) => input,
                Err(error) => return error,
            };
            let from_x = input.from_x as i32;
            let from_y = input.from_y as i32;
            let to_x = input.to_x as i32;
            let to_y = input.to_y as i32;
            let duration_ms = input.duration_ms.unwrap_or(500);
            let steps = input.steps.unwrap_or(20) as usize;
            let button = input.button.unwrap_or(ClickButton::Left).as_str();
            let hwnd = match crate::input::mouse::foreground_window() {
                Ok(hwnd) => hwnd,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            let native_drag = tokio::task::spawn_blocking(move || {
                crate::input::mouse::send_drag_synthesized(
                    hwnd,
                    from_x,
                    from_y,
                    to_x,
                    to_y,
                    duration_ms,
                    steps,
                    &button,
                )
            });
            let visual_drag = track_overlay_drag(
                cursor_key.clone(),
                (f64::from(from_x), f64::from(from_y)),
                (f64::from(to_x), f64::from(to_y)),
                duration_ms,
                steps,
            );
            let (native_drag, ()) = tokio::join!(native_drag, visual_drag);
            return match native_drag {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Dragged from ({from_x}, {from_y}) to ({to_x}, {to_y}) (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SendInput",
                    "from": {"x": from_x, "y": from_y}, "to": {"x": to_x, "y": to_y},
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("desktop drag task failed: {error}")),
            };
        }
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let delivery = DeliveryMode::from_args(&args);

        // Accepts numeric JSON as either float or integer — coerce both to f64.
        let coerce = |key: &str| -> Option<f64> {
            args.opt_f64(key)
                .or_else(|| args.opt_i64(key).map(|i| i as f64))
        };
        let from_x_opt = coerce("from_x");
        let from_y_opt = coerce("from_y");
        let to_x_opt = coerce("to_x");
        let to_y_opt = coerce("to_y");
        let (mut from_x, mut from_y, mut to_x, mut to_y) =
            match (from_x_opt, from_y_opt, to_x_opt, to_y_opt) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => {
                    return ToolResult::error(
                        "from_x, from_y, to_x, and to_y are all required (window-local pixels).",
                    )
                }
            };

        let hwnd_opt = args.opt_u64("window_id");
        let duration_ms = args.u64_or("duration_ms", 500);
        let steps = args.u64_or("steps", 20) as usize;
        let button = args.str_or("button", "left");
        let from_zoom = args.bool_or("from_zoom", false);

        if from_zoom {
            match self.state.zoom_context(&args, pid, hwnd_opt) {
                Ok(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(from_x, from_y);
                    let (wx2, wy2) = ctx.zoom_to_window(to_x, to_y);
                    from_x = wx;
                    from_y = wy;
                    to_x = wx2;
                    to_y = wy2;
                }
                Err(refusal) => return refusal,
            }
        } else {
            let ratio = match screenshot_scale(&self.state, &args, pid, hwnd_opt) {
                Ok(ratio) => ratio,
                Err(refusal) => return refusal,
            };
            from_x *= ratio;
            from_y *= ratio;
            to_x *= ratio;
            to_y *= ratio;
        }

        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // Compute screen-coord endpoints. Same correction as the click
        // tools — bitmap pixels are anchored to the DWM-frame top-left,
        // not the client area top-left (see `bitmap_to_screen` doc).
        let (sx_from, sy_from) = bitmap_to_screen(hwnd, from_x as i32, from_y as i32);
        let (sx_to, sy_to) = bitmap_to_screen(hwnd, to_x as i32, to_y as i32);

        // delivery_mode:"background" on WinUI3: a pointer drag can't both land and
        // hold the contract — the content island only consumes real
        // system-queue pointer input (which the pen/touch injector delivers but
        // at the cost of click-activating the frame: 8/8 foreground steals) and
        // ignores posted WM_* drag messages; UIA has no drag analogue. Return
        // the structured error so the caller retries with delivery_mode:foreground
        // (a WinUI3 Slider can also be set with the set_value tool, which uses
        // UIA RangeValuePattern and holds the contract).
        if delivery == DeliveryMode::Background {
            if let Some(r) = winui3_background_gesture(&None, pid, hwnd, None, 1, &button).await {
                return r;
            }
        }

        // delivery_mode:"background" — if a PostMessage drag would silently drop
        // (Chromium/WPF/GTK canvas content reads mouse from the system input
        // queue, not the per-window queue), fall back to coordinate-routed
        // synthetic-pen drag injection instead of refusing. No foreground swap,
        // no cursor move; the target is held non-activatable + cloaked for the
        // stroke (mirrors the click pen path).
        if delivery == DeliveryMode::Background
            && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseMove)
        {
            let target = hwnd;
            let btn = button.clone();
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, sx_from as f64, sy_from as f64).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: sx_from as f64,
                    y: sy_from as f64,
                },
            );
            let inj = tokio::task::spawn_blocking(move || {
                crate::input::inject::inject_drag_screen(
                    target,
                    sx_from,
                    sy_from,
                    sx_to,
                    sy_to,
                    steps.max(8),
                    &btn,
                )
            });
            let visual_drag = track_overlay_drag(
                cursor_key.clone(),
                (f64::from(sx_from), f64::from(sy_from)),
                (f64::from(sx_to), f64::from(sy_to)),
                duration_ms,
                steps,
            );
            let (inj, ()) = tokio::join!(inj, visual_drag);
            return match inj {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent drag via synthetic-pen injection on pid {raw_pid} \
                         from screen ({sx_from},{sy_from}) → ({sx_to},{sy_to}) \
                         (delivery_mode:background, PostMessage would have been dropped)."
                )),
                Ok(Err(e)) => crate::input::delivery::background_unavailable_error_with_cause(
                    hwnd,
                    EventKind::MouseMove,
                    e.to_string(),
                ),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        // delivery_mode:"foreground" — SendInput-based drag. Required for WPF
        // Slider thumbs (and any framework that polls GetKeyState during
        // its drag handler — PostMessage doesn't update per-thread input
        // state, so those targets see a button-up world during the drag
        // and never start tracking). Like the foreground click, it activates
        // the target and refuses rather than drag in a window that covers it.
        if delivery == DeliveryMode::Foreground {
            let btn_fg = button.clone();
            pin_overlay_above(&cursor_key, hwnd);
            let prev_fg_addr = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
            };
            let send_result = tokio::task::spawn_blocking(move || {
                crate::input::mouse::send_drag_synthesized_active(
                    hwnd,
                    sx_from,
                    sy_from,
                    sx_to,
                    sy_to,
                    duration_ms,
                    steps,
                    &btn_fg,
                )
            });
            let visual_drag = track_overlay_drag(
                cursor_key.clone(),
                (f64::from(sx_from), f64::from(sy_from)),
                (f64::from(sx_to), f64::from(sy_to)),
                duration_ms,
                steps,
            );
            let (send_result, ()) = tokio::join!(send_result, visual_drag);
            tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
            let button_suffix = if button == "left" {
                String::new()
            } else {
                format!(" ({} button)", button)
            };
            return match send_result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent drag{button_suffix} via SendInput on pid {raw_pid} \
                     from screen ({sx_from},{sy_from}) → ({sx_to},{sy_to}) \
                     in {duration_ms}ms / {steps} steps (delivery_mode:foreground)."
                )),
                Ok(Err(e)) => {
                    let message = e.to_string();
                    if message.starts_with("foreground_unavailable:") {
                        // Nothing was sent; give callers the structured code
                        // and its recovery hint rather than bare text.
                        ToolResult::error(message).with_structured(json!({
                            "code": "foreground_unavailable",
                            "effect": "refused",
                            "pid": raw_pid,
                            "window_id": hwnd,
                        }))
                    } else {
                        ToolResult::error(message)
                    }
                }
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Background posted-message drags cannot move or resize a top-level
        // window: a caption/border drag is handled by the OS's interactive
        // move/resize modal loop, which only real pointer input can drive —
        // the posted WM_MOUSEMOVE stream is discarded and the "success" is a
        // verified no-op. Hit-test the start point and refuse with the
        // structured background_unavailable error so callers escalate to
        // delivery_mode:"foreground" per the documented ladder instead of
        // trusting a drag that did nothing.
        let nc_hit = tokio::task::spawn_blocking(move || {
            crate::input::mouse::non_client_move_resize_hit(hwnd, sx_from, sy_from)
        })
        .await
        .ok()
        .flatten();
        if let Some(region) = nc_hit {
            return crate::input::delivery::background_unavailable_error_with_cause(
                hwnd,
                EventKind::MouseMove,
                format!(
                    "the drag starts on the window's {region} (WM_NCHITTEST), and moving \
                     or resizing a top-level window requires the OS interactive \
                     move/resize loop, which posted mouse messages cannot drive. \
                     Re-issue the same drag with delivery_mode:\"foreground\"."
                ),
            );
        }

        // Pin the agent-cursor overlay above the drag target so the synthetic
        // cursor stays sandwiched at z+1 of the dragged window for the full
        // path. Drag stays within a single HWND, so one pin at the start is
        // sufficient — the 80 ms z-order tick keeps it asserted thereafter.
        pin_overlay_above(&cursor_key, hwnd);

        // Move the agent cursor to the drag-start, show the press state, and
        // track the same interpolated step count and duration while the
        // posted-message drag runs.
        overlay_glide_to(&cursor_key, sx_from as f64, sy_from as f64).await;
        crate::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::ClickPulse {
                x: sx_from as f64,
                y: sy_from as f64,
            },
        );

        let button_c = button.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Screen-coord, deepest-child variant: routes the gesture to the
            // child control under the start point (e.g. a WinForms Panel),
            // not the top-level frame that would ignore it.
            crate::input::mouse::post_drag_screen(
                hwnd,
                sx_from,
                sy_from,
                sx_to,
                sy_to,
                duration_ms,
                steps,
                &button_c,
            )
        });
        let visual_drag = track_overlay_drag(
            cursor_key.clone(),
            (f64::from(sx_from), f64::from(sy_from)),
            (f64::from(sx_to), f64::from(sy_to)),
            duration_ms,
            steps,
        );
        let (result, ()) = tokio::join!(result, visual_drag);

        match result {
            Ok(Ok(())) => {
                // Match Swift's text format verbatim.
                let button_suffix = if button == "left" {
                    String::new()
                } else {
                    format!(" ({} button)", button)
                };
                ToolResult::text(format!(
                    "✅ Posted drag{button_suffix} to pid {raw_pid} \
                     from window-pixel ({}, {}) → ({}, {}), \
                     screen ({sx_from}, {sy_from}) → ({sx_to}, {sy_to}) \
                     in {duration_ms}ms / {steps} steps.",
                    from_x as i32, from_y as i32, to_x as i32, to_y as i32,
                ))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
