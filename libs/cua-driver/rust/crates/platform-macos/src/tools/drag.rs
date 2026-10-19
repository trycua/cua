//! drag tool — matches the Swift reference DragTool.swift.
//!
//! Press-drag-release gesture: mouseDown at (from_x, from_y), N interpolated
//! mouseDragged events along the path, mouseUp at (to_x, to_y).
//!
//! All coordinates are in window-local screenshot pixels (same space as
//! `get_window_state` returns). `from_zoom=true` translates from the most
//! recent zoom crop context, same as click/double_click.

use async_trait::async_trait;
use cua_driver_contract::{ClickButton, DragInput};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;
use crate::apps;
use crate::focus_guard;
use crate::input::mouse::DragButton;
use crate::window_change_detector::WindowChangeDetector;

pub struct DragTool {
    pub state: Arc<ToolState>,
}

impl DragTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

const DELIVERY_MODE_DESCRIPTION: &str = "Window-scoped drag on macOS: \"background\" (default) \
     posts the press, drag and release to the exact window (window_id required) without moving \
     the pointer, raising the window or taking the user's focus. \"foreground\" briefly fronts \
     the window, drives the physical pointer through the gesture, then restores the prior \
     frontmost app; use it only when a background drag had no effect. Desktop scope ignores \
     this field.";

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "drag".into(),
        description:
            "Press-drag-release gesture from (from_x, from_y) to (to_x, to_y) in \
             window-local screenshot pixels — the same space get_window_state returns. \
             Top-left origin of the target's window.\n\n\
             Use for: marquee/lasso selection, drag-and-drop, resizing via a handle, \
             scrubbing a slider, repositioning a panel.\n\n\
             `duration_ms` (default 500) is the wall-clock budget for the path between \
             mouse-down and mouse-up; `steps` (default 20) is the number of intermediate \
             mouseDragged events linearly interpolated along the path. Increase both for \
             slower, more human drags; decrease for snap gestures.\n\n\
             `modifier` keys (cmd/shift/option/ctrl) are held across the entire gesture.\n\n\
             When `from_zoom` is true, coordinates are in the last zoom image for this \
             pid; the driver maps them back to window coordinates before dispatching.\n\n\
             A window-scoped drag (pid + window_id) runs in the background by default: \
             the gesture is posted to that window, the real pointer does not move, and the \
             user's frontmost window keeps focus. It works for controls that follow the \
             drag events: sliders, canvases, and pointer-event widgets on web pages in \
             Chrome. In an Electron window the driver sends the gesture from the app's \
             main process, so pointer-capture widgets and HTML5 drag and drop also drop \
             there. Elsewhere, a drag that starts a macOS drag-and-drop session (reordering a list \
             by drag and drop, moving a file, HTML5 draggable content) cannot drop in the \
             background, because macOS tracks those with the real pointer; the driver \
             detects it, releases at once and returns background_unavailable with reason \
             drag_and_drop_session. For a page in a browser \
             tab already bound with get_browser_state, browser_pointer action:\"drag\" \
             drives the same gesture through CDP with page refs. The result is not \
             driver-verified: confirm with a screenshot or the accessibility tree, and only \
             if nothing changed retry with delivery_mode:\"foreground\", which briefly \
             fronts the window and moves the physical pointer."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["from_x", "from_y", "to_x", "to_y"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid": { "type": "integer", "description": "Target process ID." },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID for the window the pixel coordinates were measured against. Optional only when pid owns exactly one eligible top-level window; otherwise the action refuses with ambiguous_window_target."
                },
                "from_x": { "type": "number", "description": "Drag-start X in window-local screenshot pixels. Top-left origin." },
                "from_y": { "type": "number", "description": "Drag-start Y in window-local screenshot pixels. Top-left origin." },
                "to_x": { "type": "number", "description": "Drag-end X in window-local screenshot pixels." },
                "to_y": { "type": "number", "description": "Drag-end Y in window-local screenshot pixels." },
                "duration_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 10000,
                    "description": "Wall-clock duration of the drag path between mouseDown and mouseUp. Default: 500."
                },
                "steps": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Number of intermediate mouseDragged events linearly interpolated along the path. Default: 20."
                },
                "modifier": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Modifier keys held across the entire gesture: cmd/shift/option/ctrl."
                },
                "button": {
                    "type": "string",
                    "enum": ["left", "right", "middle"],
                    "description": "Mouse button used for the drag. Default: left."
                },
                "from_zoom": {
                    "type": "boolean",
                    "description": "When true, coordinates are in the last zoom image for this pid; driver maps back to window coordinates."
                },
                "scope": { "type": "string", "enum": ["window", "desktop"], "default": "window", "description": "Use desktop with no pid/window_id for native get_desktop_state screenshot coordinates." },
                "delivery_mode": cua_driver_core::tool_schema::delivery_mode_schema_with(DELIVERY_MODE_DESCRIPTION)
            },
            "additionalProperties": false
        }),
        read_only:   false,
        destructive: true,
        idempotent:  false,
        open_world:  true,
    })
}

#[async_trait]
impl Tool for DragTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<DragInput>("drag", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let (from_x, from_y, to_x, to_y) = (input.from_x, input.from_y, input.to_x, input.to_y);
            let (from_x, from_y) = super::desktop_screenshot_point(from_x, from_y);
            let (to_x, to_y) = super::desktop_screenshot_point(to_x, to_y);
            if let Some(refusal) = super::desktop_pointer_guard::check(
                "drag",
                &args,
                vec![(from_x, from_y), (to_x, to_y)],
            )
            .await
            {
                return refusal;
            }
            let duration_ms = input.duration_ms.unwrap_or(500).min(10_000);
            let steps = input.steps.unwrap_or(20).clamp(1, 200) as usize;
            let modifiers = input.modifier.unwrap_or_default();
            let button = match input.button.unwrap_or(ClickButton::Left) {
                ClickButton::Left => DragButton::Left,
                ClickButton::Right => DragButton::Right,
                ClickButton::Middle => DragButton::Middle,
            };
            let cursor_for_drag = cursor_key.clone();
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::SetPressed(true),
            );
            let result = tokio::task::spawn_blocking(move || {
                let modifier_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                crate::input::mouse::drag_at_xy_foreground_observed(
                    from_x,
                    from_y,
                    to_x,
                    to_y,
                    duration_ms,
                    steps,
                    &modifier_refs,
                    button,
                    move |x, y| {
                        crate::cursor::overlay::send_command(
                            cursor_for_drag.clone(),
                            cursor_overlay::track_pointer_command(x, y),
                        );
                    },
                )
            })
            .await;
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::SetPressed(false),
            );
            if matches!(&result, Ok(Ok(()))) {
                self.state
                    .cursor_registry
                    .update_position(&cursor_key, to_x, to_y);
            }
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Dragged desktop from ({from_x:.1}, {from_y:.1}) to ({to_x:.1}, {to_y:.1})."
                ))
                .with_structured(serde_json::json!({
                    "scope": "desktop",
                    "path": "hid",
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(format!("desktop drag failed: {error}")),
                Err(error) => ToolResult::error(format!("desktop drag task failed: {error}")),
            };
        }
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Background (default): post the gesture to the exact window without
        // moving the pointer or raising it, after the focus-without-raise
        // prologue background clicks use. Foreground: briefly front the
        // window and drive the physical pointer (the explicit last resort for
        // surfaces that drop background events). Both need the exact window.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        let fg = delivery_mode.is_foreground();
        // Coerce integer or float from JSON for coordinate fields.
        let coerce = |key: &str| -> Option<f64> {
            args.opt_f64(key)
                .or_else(|| args.opt_i64(key).map(|i| i as f64))
        };

        let mut from_x = match coerce("from_x") {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: from_x"),
        };
        let mut from_y = match coerce("from_y") {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: from_y"),
        };
        let mut to_x = match coerce("to_x") {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: to_x"),
        };
        let mut to_y = match coerce("to_y") {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: to_y"),
        };

        let Some(wid) = args.opt_u64("window_id").map(|v| v as u32) else {
            return window_id_required(fg);
        };
        let window_id = Some(wid);
        let duration_ms = args.u64_or("duration_ms", 500).min(10_000);
        let steps = (args.u64_or("steps", 20) as usize).clamp(1, 200);
        let from_zoom = args.bool_or("from_zoom", false);
        let button_str = args.str_or("button", "left");
        let modifiers: Vec<String> = args.str_array("modifier");

        let button = match button_str.to_lowercase().as_str() {
            "left" => DragButton::Left,
            "right" => DragButton::Right,
            "middle" => DragButton::Middle,
            other => {
                return ToolResult::error(format!(
                    "Unknown button \"{other}\" — expected left, right, or middle."
                ))
            }
        };

        // from_zoom: translate from last zoom crop context.
        if from_zoom {
            match super::zoom_context(&self.state, &args, pid, window_id) {
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
            let ratio = match super::screenshot_scale(&self.state, &args, pid, window_id) {
                Ok(ratio) => ratio,
                Err(refusal) => return refusal,
            };
            from_x *= ratio;
            from_y *= ratio;
            to_x *= ratio;
            to_y *= ratio;
        }

        // Translate window-local screenshot pixels → screen coordinates, and
        // window-local logical coords for the foreground HID stamp. Both ends
        // of the drag resolve against ONE frame, so a window that closed
        // mid-call refuses rather than dragging across the desktop behind it.
        let (from_sx, from_sy, from_lx, from_ly, to_sx, to_sy, to_lx, to_ly) =
            match super::px_frame::resolve_or_refuse(wid).await {
                Ok(frame) => {
                    let (fsx, fsy, flx, fly) = frame.to_screen(from_x, from_y);
                    let (tsx, tsy, tlx, tly) = frame.to_screen(to_x, to_y);
                    (fsx, fsy, flx, fly, tsx, tsy, tlx, tly)
                }
                Err(refusal) => return refusal,
            };

        // A toolkit that reads the hardware pointer (Tk) or drops routed
        // mouse events (LibreOffice VCL) cannot take a background gesture;
        // refuse before any activation or dispatch.
        if !fg {
            if let Err(refusal) =
                super::pixel_route::resolve(pid, false, window_id, "mouse_drag").await
            {
                return refusal;
            }
            // Chromium reads a mouse event's held buttons from the hardware
            // state, which a window-routed press does not change, so an
            // Electron page sees the drag moves with buttons == 0, releases
            // its pointer capture and loses the drop. Send the gesture from
            // the app's main process instead, where the held button is part
            // of the event; refuse when that route is unavailable.
            if crate::browser::ElectronJs::is_electron(pid) {
                return self
                    .electron_background_drag(
                        &cursor_key,
                        pid,
                        wid,
                        (from_lx, from_ly),
                        (to_lx, to_ly),
                        (from_sx, from_sy),
                        (to_sx, to_sy),
                        duration_ms,
                        steps,
                        &modifiers,
                        button,
                    )
                    .await;
            }
        }

        // Animate agent cursor along drag path (start → end).
        crate::cursor::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::PinAbove(wid as u64),
        );
        crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), from_sx, from_sy).await;

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Drags can trigger drag-and-drop side-effects that spawn helper
        // windows (drop on Dock, drop on background app icon). In the
        // background the target itself may become AppKit-active without
        // raising; anything else that takes focus is reported and undone.
        let prior_front = apps::frontmost_pid();
        let snapshot = if fg {
            WindowChangeDetector::snapshot(prior_front)
        } else {
            WindowChangeDetector::snapshot_allowing_activation(prior_front, pid)
        };

        // Background prologue, as for a background pixel click: keep the
        // user's key focus if the gesture opens a menu, remember their key
        // window, and make the target window AppKit-key without raising it.
        let _key_focus = if fg {
            None
        } else {
            tokio::task::spawn_blocking(move || crate::key_focus_guard::protect(pid))
                .await
                .ok()
                .flatten()
        };
        let prior_key_wid = match prior_front {
            Some(prior) if !fg && prior != pid => tokio::task::spawn_blocking(move || {
                crate::input::skylight::key_window_of_pid(prior)
            })
            .await
            .ok()
            .flatten(),
            _ => None,
        };
        let focus_without_raise = if fg {
            false
        } else {
            match tokio::task::spawn_blocking(move || {
                crate::input::mouse::prepare_background_pixel_click(pid, wid)
            })
            .await
            {
                Ok(activated) => {
                    if !activated && crate::input::skylight::windowserver_connection_invalid() {
                        return super::windowserver_connection_refusal(pid, Some(wid));
                    }
                    crate::cursor::overlay::send_command(
                        cursor_key.clone(),
                        cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                    );
                    activated
                }
                Err(error) => {
                    return ToolResult::error(format!(
                        "Background drag activation task failed: {error}"
                    ))
                }
            }
        };

        // Dispatch blocking drag synthesis.
        let mods_owned = modifiers.clone();
        let cursor_for_drag = cursor_key.clone();
        crate::cursor::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::SetPressed(true),
        );
        let result = focus_guard::with_focus_suppressed(
            // Neither mode suppresses the target: foreground deliberately
            // activates it for the HID gesture, and background lets the
            // no-raise prologue make it active; the restore below hands focus
            // back. Other apps that take focus are still suppressed.
            None,
            prior_front,
            "drag.CGEvent",
            || async move {
                tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
                    let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                    let observe = move |x: f64, y: f64| {
                        crate::cursor::overlay::send_command(
                            cursor_for_drag.clone(),
                            cursor_overlay::track_pointer_command(x, y),
                        );
                    };
                    if fg {
                        // Keep the exact target window active for the
                        // complete HID gesture, then restore the prior
                        // front process.
                        return crate::input::skylight::with_foreground_hid_activation(
                            pid as libc::pid_t,
                            wid,
                            || {
                                crate::input::mouse::drag_at_xy_observed(
                                    pid,
                                    from_sx,
                                    from_sy,
                                    to_sx,
                                    to_sy,
                                    Some((from_lx, from_ly)),
                                    Some((to_lx, to_ly)),
                                    Some(wid),
                                    duration_ms,
                                    steps,
                                    &m,
                                    button,
                                    true,
                                    observe,
                                )
                            },
                        )
                        .map(|()| false);
                    }
                    // A drag that starts a system drag-and-drop session
                    // (list reordering, file drags, HTML5 draggable) is
                    // tracked by macOS with the hardware pointer, so the
                    // drop cannot land at the requested point. AppKit shows
                    // the drag image in a window of the target at the
                    // dragging level; when one appears, release at once so
                    // the session ends where the user's pointer is not
                    // moving, and report it.
                    let before = drag_image_windows(pid);
                    let mut session_seen = false;
                    let outcome = crate::input::mouse::drag_at_xy_background(
                        pid,
                        (from_sx, from_sy),
                        (to_sx, to_sy),
                        wid,
                        duration_ms,
                        steps,
                        &m,
                        button,
                        observe,
                        || {
                            session_seen = !drag_image_windows(pid).is_subset(&before);
                            !session_seen
                        },
                    )?;
                    let session = session_seen
                        || !drag_image_windows(pid).is_subset(&before)
                        || matches!(
                            outcome,
                            crate::input::mouse::BackgroundDragOutcome::StoppedEarly { .. }
                        );
                    Ok(session)
                })
                .await
            },
        )
        .await;
        crate::cursor::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::SetPressed(false),
        );
        if matches!(&result, Ok(Ok(false))) {
            self.state
                .cursor_registry
                .update_position(&cursor_key, to_sx, to_sy);
        }

        if !fg {
            super::click::restore_user_focus_after_background_pixel(
                pid,
                prior_front,
                prior_key_wid,
                window_id,
                focus_without_raise,
            )
            .await;
        }

        let changes = super::finish_window_observation(snapshot).await;

        crate::cursor::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::PinAbove(wid as u64),
        );

        let mod_suffix = if modifiers.is_empty() {
            String::new()
        } else {
            format!(" with {}", modifiers.join("+"))
        };
        let btn_suffix = if button_str == "left" {
            String::new()
        } else {
            format!(" ({button_str} button)")
        };
        let (route_note, path) = if fg {
            (
                "delivery_mode:foreground; window fronted, hardware pointer moved, HID event tap",
                "cgevent_hid",
            )
        } else {
            (
                "background: routed to the window's pid, pointer not moved, window not raised",
                "cgevent",
            )
        };
        match result {
            Ok(Ok(true))
                if cua_driver_core::delivery::brief_foreground_allowed()
                    && !cua_driver_core::delivery::background_requested_explicitly(&args) =>
            {
                self.brief_foreground_drag(&args, pid, wid, prior_front)
                    .await
            }
            Ok(Ok(true)) => drag_and_drop_session_result(pid, wid, changes.result_suffix()),
            Ok(Ok(false)) => ToolResult::text(format!(
                "✅ Posted drag{btn_suffix}{mod_suffix} to pid {pid} window {wid} \
                 from window-pixel ({}, {}) → ({}, {}), \
                 screen ({}, {}) → ({}, {}) \
                 in {duration_ms}ms / {steps} steps ({route_note}; not driver-verified — \
                 confirm via screenshot).{}",
                from_x as i64,
                from_y as i64,
                to_x as i64,
                to_y as i64,
                from_sx as i64,
                from_sy as i64,
                to_sx as i64,
                to_sy as i64,
                changes.result_suffix(),
            ))
            .with_structured(serde_json::json!({
                "path": path,
                "delivery_mode": if fg { "foreground" } else { "background" },
                "verified": false,
                "effect": "unverifiable",
                "focus_without_raise": focus_without_raise
            })),
            Ok(Err(e)) => ToolResult::error(format!("drag failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

impl DragTool {
    /// The operator allowed a brief foreground step (trycua/cua#5478) and the
    /// background drag started a native drag-and-drop session, which macOS
    /// tracks with the real pointer. Repeat only this drag in the foreground,
    /// which fronts the window, drives the pointer, then hands the frontmost
    /// app and the pointer back. Check both and report the step in the result.
    async fn brief_foreground_drag(
        &self,
        args: &Value,
        pid: i32,
        wid: u32,
        prior_front: Option<i32>,
    ) -> ToolResult {
        let pointer_before = tokio::task::spawn_blocking(pointer_location)
            .await
            .ok()
            .flatten();
        tracing::info!(
            pid,
            window_id = wid,
            ?prior_front,
            "drag: background drop impossible (drag-and-drop session); \
             operator allowed a brief foreground step"
        );
        let mut foreground_args = args.clone();
        foreground_args["delivery_mode"] = serde_json::json!("foreground");
        let mut result = self.invoke(foreground_args).await;

        // The foreground drag leaves the pointer on the drop point; this step
        // promised to hand it back, so put it where the user left it.
        if let Some((x, y)) = pointer_before {
            let _ =
                tokio::task::spawn_blocking(move || crate::input::mouse::move_cursor_desktop(x, y))
                    .await;
        }
        let pointer_after = tokio::task::spawn_blocking(pointer_location)
            .await
            .ok()
            .flatten();
        let front_after = apps::frontmost_pid();
        let restored_frontmost = prior_front.is_none() || front_after == prior_front;
        let restored_pointer = match (pointer_before, pointer_after) {
            (Some((bx, by)), Some((ax, ay))) => Some((bx - ax).hypot(by - ay) <= 1.0),
            _ => None,
        };
        tracing::info!(
            pid,
            window_id = wid,
            restored_frontmost,
            ?restored_pointer,
            failed = result.is_error == Some(true),
            "drag: brief foreground step finished"
        );

        let step = brief_foreground_step(restored_frontmost, restored_pointer);
        let note = brief_foreground_note(restored_frontmost, restored_pointer);
        let mut structured = result
            .structured_content
            .take()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(fields) = structured.as_object_mut() {
            fields.insert("foreground_step".to_owned(), step);
        }
        result.structured_content = Some(structured);
        result
            .content
            .insert(0, cua_driver_core::protocol::Content::text(note));
        result
    }

    /// Background drag in an Electron window through its main process
    /// ([`crate::input::electron_drag`]). Refuses when the app's inspector
    /// is unavailable (for example the inspector fuse is off).
    #[allow(clippy::too_many_arguments)]
    async fn electron_background_drag(
        &self,
        cursor_key: &str,
        pid: i32,
        wid: u32,
        from_local: (f64, f64),
        to_local: (f64, f64),
        from_screen: (f64, f64),
        to_screen: (f64, f64),
        duration_ms: u64,
        steps: usize,
        modifiers: &[String],
        button: DragButton,
    ) -> ToolResult {
        use crate::input::electron_drag::{
            electron_drag_js, parse_electron_drag_reply, ElectronDragButton, ElectronDragReply,
            ElectronDragSpec,
        };
        let spec = ElectronDragSpec {
            window_id: wid,
            from: from_local,
            to: to_local,
            steps,
            duration_ms,
            button: match button {
                DragButton::Left => ElectronDragButton::Left,
                DragButton::Right => ElectronDragButton::Right,
                DragButton::Middle => ElectronDragButton::Middle,
            },
            modifiers: modifiers.to_vec(),
        };
        let cursor_key = cursor_key.to_owned();
        crate::cursor::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::PinAbove(wid as u64),
        );
        crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), from_screen.0, from_screen.1)
            .await;
        crate::cursor::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::SetPressed(true),
        );
        let reply = crate::browser::ElectronJs::execute_main(&electron_drag_js(&spec), pid).await;
        let outcome = match &reply {
            Ok(text) => parse_electron_drag_reply(text),
            Err(_) => None,
        };
        if matches!(outcome, Some(ElectronDragReply::Delivered { .. })) {
            crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), to_screen.0, to_screen.1)
                .await;
            self.state
                .cursor_registry
                .update_position(&cursor_key, to_screen.0, to_screen.1);
        }
        crate::cursor::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::SetPressed(false),
        );
        match outcome {
            Some(ElectronDragReply::Delivered {
                route,
                drag_intercepted,
                html5_drop,
            }) => {
                let dnd = if html5_drop {
                    " The gesture started an HTML5 drag and drop; the driver kept it out of the \
                     native drag session and dropped it at the end point."
                } else {
                    ""
                };
                ToolResult::text(format!(
                    "✅ Posted drag to Electron pid {pid} window {wid} from window-point \
                     ({:.0}, {:.0}) → ({:.0}, {:.0}) through the app's main process \
                     ({route}); pointer not moved, window not raised, focus kept.{dnd} \
                     Not driver-verified — confirm via screenshot.",
                    from_local.0, from_local.1, to_local.0, to_local.1,
                ))
                .with_structured(serde_json::json!({
                    // Chromium trusted input (CDP Input or sendInputEvent), the
                    // same public route as browser_pointer.
                    "path": "trusted",
                    "electron_route": route,
                    "drag_intercepted": drag_intercepted,
                    "html5_drop": html5_drop,
                    "delivery_mode": "background",
                    "verified": false,
                    "effect": "unverifiable",
                }))
            }
            other => {
                let reason = match (other, &reply) {
                    (Some(ElectronDragReply::Refused { reason }), _) => reason,
                    (_, Err(error)) => error.to_string(),
                    (_, Ok(text)) => format!("unexpected inspector reply: {text}"),
                };
                cua_driver_core::delivery::background_unavailable_result(
                    format!("{ELECTRON_BACKGROUND_DRAG_UNAVAILABLE} ({reason})"),
                    "background_unavailable",
                    "the Electron app's main process could not take the drag",
                    serde_json::json!({
                        "effect": "refused",
                        "event_kind": "mouse_drag",
                        "reason": "electron_main_process_unavailable",
                        "requires": ["delivery_mode:foreground", "window_id"],
                    }),
                )
            }
        }
    }
}

/// Current hardware pointer location in global display points.
fn pointer_location() -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let location = CGEvent::new(source).ok()?.location();
    Some((location.x, location.y))
}

/// Structured record of a brief foreground step, for the agent and the log.
fn brief_foreground_step(restored_frontmost: bool, restored_pointer: Option<bool>) -> Value {
    serde_json::json!({
        "used": true,
        "reason": "drag_and_drop_session",
        "allowed_by": cua_driver_core::delivery::BRIEF_FOREGROUND_ENV,
        "background_attempt": "no_drop",
        "restored_frontmost": restored_frontmost,
        "restored_pointer": restored_pointer,
    })
}

fn brief_foreground_note(restored_frontmost: bool, restored_pointer: Option<bool>) -> String {
    let mut note = String::from(
        "Brief foreground step: the background drag started a macOS drag-and-drop session, \
         which follows the real pointer, so the driver repeated this one drag in the \
         foreground because the operator allowed it. The window came forward and the \
         pointer moved for the drag only.",
    );
    note.push_str(if restored_frontmost {
        " The previous frontmost app is back in front."
    } else {
        " The previous frontmost app did NOT come back to the front; tell the user."
    });
    note.push_str(match restored_pointer {
        Some(true) => " The pointer is back where it was.",
        Some(false) => " The pointer did NOT return to where it was; tell the user.",
        None => " The pointer position could not be checked.",
    });
    note
}

/// `kCGDraggingWindowLevel`: AppKit shows the image of a live (or sliding
/// back) drag-and-drop session in a window of the source app at this level.
const DRAGGING_WINDOW_LEVEL: i32 = 500;

const ELECTRON_BACKGROUND_DRAG_UNAVAILABLE: &str = "Background drag is unavailable for this \
     Electron window on macOS: a window-routed drag cannot hold the page's mouse button \
     (Chromium reads it from the hardware), and the app's main process did not take the drag. \
     Use the app's own controls (a button, menu item or keyboard route) or, if the user allows \
     the window to come forward, retry with delivery_mode:\"foreground\".";

fn drag_image_windows(pid: i32) -> std::collections::HashSet<u32> {
    crate::windows::pid_on_screen_windows_any_layer(pid)
        .into_iter()
        .filter(|window| window.layer == DRAGGING_WINDOW_LEVEL)
        .map(|window| window.window_id)
        .collect()
}

fn drag_and_drop_session_result(pid: i32, wid: u32, suffix: String) -> ToolResult {
    cua_driver_core::delivery::background_unavailable_result(
        format!(
            "The background drag in pid {pid} window {wid} started a macOS drag-and-drop \
             session (for example reordering a list, moving a file or dragging page content). \
             macOS tracks drag-and-drop with the real pointer, so the drop did not land at the \
             requested point; the driver released at once. Look for a route that is not a drag \
             (a Move Up/Down button or menu item, a context-menu command, a keyboard shortcut, \
             cut and paste). If the drag is the only route and the user allows the window to \
             come forward, retry with delivery_mode:\"foreground\".{suffix}"
        ),
        "background_unavailable",
        "drag-and-drop sessions follow the hardware pointer on macOS",
        serde_json::json!({
            "effect": "no_drop",
            "event_kind": "mouse_drag",
            "reason": "drag_and_drop_session",
            "requires": ["delivery_mode:foreground", "window_id"],
        }),
    )
}

fn window_id_required(foreground: bool) -> ToolResult {
    let mode = if foreground {
        "foreground"
    } else {
        "background"
    };
    ToolResult::error(format!(
        "A {mode} drag on macOS needs window_id: the gesture is addressed to one exact \
         window. Read it from list_windows or get_window_state and retry."
    ))
    .with_structured(serde_json::json!({
        "code": "window_id_required",
        "effect": "refused"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn window_drag_without_window_id_is_refused_in_both_modes() {
        let tool = DragTool::new(Arc::new(ToolState::new(false, false, None)));
        for delivery_mode in [None, Some("background"), Some("foreground")] {
            let mut args = serde_json::json!({
                "pid": 4242,
                "from_x": 1, "from_y": 2, "to_x": 3, "to_y": 4,
            });
            if let Some(mode) = delivery_mode {
                args["delivery_mode"] = serde_json::json!(mode);
            }
            let result = tool.invoke(args).await;
            assert_eq!(result.is_error, Some(true));
            let structured = result.structured_content.expect("structured refusal");
            assert_eq!(structured["code"], "window_id_required");
            assert_eq!(structured["effect"], "refused");
        }
    }

    #[test]
    fn brief_foreground_step_reports_what_was_restored() {
        let step = super::brief_foreground_step(true, Some(true));
        assert_eq!(step["used"], true);
        assert_eq!(step["reason"], "drag_and_drop_session");
        assert_eq!(step["allowed_by"], "CUA_DRIVER_ALLOW_BRIEF_FOREGROUND");
        assert_eq!(step["restored_frontmost"], true);
        assert_eq!(step["restored_pointer"], true);
        let unknown = super::brief_foreground_step(false, None);
        assert_eq!(unknown["restored_frontmost"], false);
        assert!(unknown["restored_pointer"].is_null());

        let note = super::brief_foreground_note(true, Some(true));
        assert!(note.contains("Brief foreground step"));
        assert!(note.contains("frontmost app is back"));
        assert!(note.contains("pointer is back"));
        let failed = super::brief_foreground_note(false, Some(false));
        assert!(failed.contains("did NOT come back"));
        assert!(failed.contains("did NOT return"));
    }

    #[test]
    fn schema_says_macos_drag_runs_in_the_background() {
        let def = def();
        assert!(def
            .description
            .contains("runs in the background by default"));
        assert!(def.description.contains("browser_pointer"));
        assert!(!def.description.contains("no background drag"));
        let mode = def.input_schema["properties"]["delivery_mode"]["description"]
            .as_str()
            .unwrap();
        assert!(mode.contains("\"background\" (default)"));
        assert!(mode.contains("without moving the pointer"));
    }
}
