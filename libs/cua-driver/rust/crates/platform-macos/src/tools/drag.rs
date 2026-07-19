//! drag tool — matches the Swift reference DragTool.swift.
//!
//! Press-drag-release gesture: mouseDown at (from_x, from_y), N interpolated
//! mouseDragged events along the path, mouseUp at (to_x, to_y).
//!
//! All coordinates are in window-local screenshot pixels (same space as
//! `get_window_state` returns). `from_zoom=true` translates from the most
//! recent zoom crop context, same as click/double_click.

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
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
             pid; the driver maps them back to window coordinates before dispatching."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["from_x", "from_y", "to_x", "to_y"],
            "properties": {
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
                "pid": { "type": "integer", "description": "Target process ID." },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID for the window the pixel coordinates were measured against. Optional — when omitted the driver picks the frontmost window of pid."
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
                "delivery_mode": cua_driver_core::tool_schema::delivery_mode_schema()
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
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let coordinate = |key: &str| {
                args.opt_f64(key)
                    .or_else(|| args.opt_i64(key).map(|value| value as f64))
            };
            let Some(from_x) = coordinate("from_x") else {
                return ToolResult::error("Missing required parameter: from_x");
            };
            let Some(from_y) = coordinate("from_y") else {
                return ToolResult::error("Missing required parameter: from_y");
            };
            let Some(to_x) = coordinate("to_x") else {
                return ToolResult::error("Missing required parameter: to_x");
            };
            let Some(to_y) = coordinate("to_y") else {
                return ToolResult::error("Missing required parameter: to_y");
            };
            let (from_x, from_y) = super::desktop_screenshot_point(from_x, from_y).await;
            let (to_x, to_y) = super::desktop_screenshot_point(to_x, to_y).await;
            let duration_ms = args.u64_or("duration_ms", 500).min(10_000);
            let steps = args.u64_or("steps", 20).clamp(1, 200) as usize;
            let modifiers: Vec<String> = args.str_array("modifier");
            let button = match args.str_or("button", "left").to_lowercase().as_str() {
                "left" => DragButton::Left,
                "right" => DragButton::Right,
                "middle" => DragButton::Middle,
                other => {
                    return ToolResult::error(format!(
                        "Unknown button '{other}' — expected left, right, or middle."
                    ))
                }
            };
            let result = tokio::task::spawn_blocking(move || {
                let modifier_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                crate::input::mouse::drag_at_xy_foreground(
                    from_x,
                    from_y,
                    to_x,
                    to_y,
                    duration_ms,
                    steps,
                    &modifier_refs,
                    button,
                )
            })
            .await;
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
        // delivery_mode: foreground briefly fronts the window before the
        // press-drag-release gesture (the explicit last resort for surfaces
        // that drop background CGEvents), via the same skylight assist click
        // uses. Requires a window_id to have a window to front.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        if !delivery_mode.is_foreground() {
            return ToolResult::error(
                "Background drag is unavailable on macOS; use delivery_mode:\"foreground\"."
                    .to_owned(),
            )
            .with_structured(serde_json::json!({ "code": "background_unavailable" }));
        }
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);

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

        let window_id = args.opt_u64("window_id").map(|v| v as u32);
        let duration_ms = args.u64_or("duration_ms", 500);
        let steps = args.u64_or("steps", 20) as usize;
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
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(from_x, from_y);
                    let (wx2, wy2) = ctx.zoom_to_window(to_x, to_y);
                    from_x = wx;
                    from_y = wy;
                    to_x = wx2;
                    to_y = wy2;
                }
                None => {
                    return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                    ))
                }
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            from_x *= ratio;
            from_y *= ratio;
            to_x *= ratio;
            to_y *= ratio;
        }

        // Translate window-local screenshot pixels → screen coordinates.
        // Also compute window-local logical coords for CGEventSetWindowLocation.
        let (from_sx, from_sy, from_lx, from_ly, to_sx, to_sy, to_lx, to_ly) =
            if let Some(wid) = window_id {
                let result = tokio::task::spawn_blocking(move || {
                    let bounds = crate::windows::window_bounds_by_id(wid);
                    let scale: f64 = if let Some(ref b) = bounds {
                        if let Ok(png) = crate::capture::screenshot_window_bytes(wid) {
                            if png.len() >= 24 {
                                let pw =
                                    u32::from_be_bytes([png[16], png[17], png[18], png[19]]) as f64;
                                let lw = b.width;
                                if lw > 0.0 && pw > lw {
                                    pw / lw
                                } else {
                                    1.0
                                }
                            } else {
                                1.0
                            }
                        } else {
                            1.0
                        }
                    } else {
                        1.0
                    };
                    (bounds, scale)
                })
                .await
                .unwrap_or((None, 1.0));

                if let (Some(b), scale) = result {
                    let flx = from_x / scale;
                    let fly = from_y / scale;
                    let tlx = to_x / scale;
                    let tly = to_y / scale;
                    (
                        b.x + flx,
                        b.y + fly,
                        flx,
                        fly,
                        b.x + tlx,
                        b.y + tly,
                        tlx,
                        tly,
                    )
                } else {
                    (from_x, from_y, from_x, from_y, to_x, to_y, to_x, to_y)
                }
            } else {
                (from_x, from_y, from_x, from_y, to_x, to_y, to_x, to_y)
            };

        // Animate agent cursor along drag path (start → end).
        if let Some(wid) = window_id {
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::PinAbove(wid as u64),
            );
        }
        crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), from_sx, from_sy).await;

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Drags can trigger drag-and-drop side-effects that spawn helper
        // windows (drop on Dock, drop on background app icon) and the
        // mouseDown half-event alone can activate the target app on some
        // Chromium builds. Wrap to catch + report both.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        // Dispatch blocking drag synthesis.
        let mods_owned = modifiers.clone();
        let fg = delivery_mode.is_foreground() && window_id.is_some();
        let result = focus_guard::with_focus_suppressed(
            // Foreground drag deliberately activates the target so the global
            // HID stream carries the pressed-button state. A suppression lease
            // here would race that activation and restore the prior app before
            // Chromium receives the gesture.
            if fg { None } else { Some(pid) },
            prior_front,
            "drag.CGEvent",
            || async move {
                tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                    let do_it = move || -> anyhow::Result<()> {
                        let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                        if fg {
                            // HID delivery is global, so foreground mode must
                            // establish a real active application before the
                            // gesture begins. The SkyLight flash can be
                            // unavailable for Electron child windows; the
                            // documented Cocoa activation is the fallback.
                            apps::activate_pid(pid);
                            std::thread::sleep(std::time::Duration::from_millis(40));
                            return crate::input::mouse::drag_at_xy_foreground(
                                from_sx,
                                from_sy,
                                to_sx,
                                to_sy,
                                duration_ms,
                                steps,
                                &m,
                                button,
                            );
                        }
                        crate::input::mouse::drag_at_xy(
                            pid,
                            from_sx,
                            from_sy,
                            to_sx,
                            to_sy,
                            Some((from_lx, from_ly)),
                            Some((to_lx, to_ly)),
                            window_id,
                            duration_ms,
                            steps,
                            &m,
                            button,
                            fg,
                        )
                    };
                    // Foreground rung: activate for the complete HID gesture,
                    // then restore the prior app after pointer capture settles.
                    match (fg, window_id) {
                        (true, Some(_wid)) => {
                            let result = do_it();
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            if let Some(previous_pid) = prior_front {
                                if previous_pid != pid {
                                    apps::activate_pid(previous_pid);
                                }
                            }
                            result?;
                            Ok(())
                        }
                        _ => do_it(),
                    }
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        // Animate cursor to end position.
        crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), to_sx, to_sy).await;
        if let Some(wid) = window_id {
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::PinAbove(wid as u64),
            );
        }

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

        let mode_label = if fg {
            " (delivery_mode:foreground)"
        } else {
            ""
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Posted drag{btn_suffix}{mod_suffix} to pid {pid} \
                 from window-pixel ({}, {}) → ({}, {}), \
                 screen ({}, {}) → ({}, {}) \
                 in {duration_ms}ms / {steps} steps{mode_label} \
                 (background CGEvent; not driver-verified — confirm via screenshot).{}",
                from_x as i64, from_y as i64,
                to_x   as i64, to_y   as i64,
                from_sx as i64, from_sy as i64,
                to_sx   as i64, to_sy   as i64,
                changes.result_suffix(),
            ))
            .with_structured(serde_json::json!({
                "path": if fg { "cgevent_fg" } else { "cgevent" }, "verified": false, "effect": "unverifiable"
            })),
            Ok(Err(e)) => ToolResult::error(format!("drag failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
