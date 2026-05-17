//! drag tool — matches the Swift reference DragTool.swift.
//!
//! Press-drag-release gesture: mouseDown at (from_x, from_y), N interpolated
//! mouseDragged events along the path, mouseUp at (to_x, to_y).
//!
//! All coordinates are in window-local screenshot pixels (same space as
//! `get_window_state` returns). `from_zoom=true` translates from the most
//! recent zoom crop context, same as click/double_click.

use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::focus_guard;
use crate::input::mouse::DragButton;
use crate::window_change_detector::WindowChangeDetector;
use super::ToolState;

pub struct DragTool {
    pub state: Arc<ToolState>,
}

impl DragTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
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
            "required": ["pid", "from_x", "from_y", "to_x", "to_y"],
            "properties": {
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
                }
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
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };

        // Coerce integer or float from JSON for coordinate fields.
        let coerce = |key: &str| -> Option<f64> {
            args.get(key).and_then(|v| v.as_f64())
                .or_else(|| args.get(key).and_then(|v| v.as_i64()).map(|i| i as f64))
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

        let window_id   = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);
        let duration_ms = args.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(500);
        let steps       = args.get("steps").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let from_zoom   = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
        let button_str  = args.get("button").and_then(|v| v.as_str()).unwrap_or("left");
        let modifiers: Vec<String> = args.get("modifier")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();

        let button = match button_str.to_lowercase().as_str() {
            "left"   => DragButton::Left,
            "right"  => DragButton::Right,
            "middle" => DragButton::Middle,
            other    => return ToolResult::error(format!(
                "Unknown button \"{other}\" — expected left, right, or middle."
            )),
        };

        // from_zoom: translate from last zoom crop context.
        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(from_x, from_y);
                    let (wx2, wy2) = ctx.zoom_to_window(to_x, to_y);
                    from_x = wx; from_y = wy;
                    to_x   = wx2; to_y   = wy2;
                }
                None => return ToolResult::error(format!(
                    "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                )),
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            from_x *= ratio; from_y *= ratio;
            to_x   *= ratio; to_y   *= ratio;
        }

        // Translate window-local screenshot pixels → screen coordinates.
        // Also compute window-local logical coords for CGEventSetWindowLocation.
        let (from_sx, from_sy, from_lx, from_ly,
             to_sx,   to_sy,   to_lx,   to_ly) = if let Some(wid) = window_id {
            let result = tokio::task::spawn_blocking(move || {
                let bounds = crate::windows::window_bounds_by_id(wid);
                let scale: f64 = if let Some(ref b) = bounds {
                    if let Ok(png) = crate::capture::screenshot_window_bytes(wid) {
                        if png.len() >= 24 {
                            let pw = u32::from_be_bytes([png[16], png[17], png[18], png[19]]) as f64;
                            let lw = b.width;
                            if lw > 0.0 && pw > lw { pw / lw } else { 1.0 }
                        } else { 1.0 }
                    } else { 1.0 }
                } else { 1.0 };
                (bounds, scale)
            }).await.unwrap_or((None, 1.0));

            if let (Some(b), scale) = result {
                let flx = from_x / scale; let fly = from_y / scale;
                let tlx = to_x   / scale; let tly = to_y   / scale;
                (b.x + flx, b.y + fly, flx, fly,
                 b.x + tlx, b.y + tly, tlx, tly)
            } else {
                (from_x, from_y, from_x, from_y, to_x, to_y, to_x, to_y)
            }
        } else {
            (from_x, from_y, from_x, from_y, to_x, to_y, to_x, to_y)
        };

        // Animate agent cursor along drag path (start → end).
        if let Some(wid) = window_id {
            crate::cursor::overlay::send_command(
                cursor_overlay::OverlayCommand::PinAbove(wid as u64)
            );
        }
        crate::cursor::overlay::animate_cursor_to(from_sx, from_sy).await;

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Drags can trigger drag-and-drop side-effects that spawn helper
        // windows (drop on Dock, drop on background app icon) and the
        // mouseDown half-event alone can activate the target app on some
        // Chromium builds. Wrap to catch + report both.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        // Dispatch blocking drag synthesis.
        let mods_owned = modifiers.clone();
        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "drag.CGEvent",
            || async move {
                tokio::task::spawn_blocking(move || {
                    let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                    crate::input::mouse::drag_at_xy(
                        pid,
                        from_sx, from_sy,
                        to_sx,   to_sy,
                        Some((from_lx, from_ly)),
                        Some((to_lx,   to_ly)),
                        window_id,
                        duration_ms,
                        steps,
                        &m,
                        button,
                    )
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        // Animate cursor to end position.
        crate::cursor::overlay::animate_cursor_to(to_sx, to_sy).await;
        if let Some(wid) = window_id {
            crate::cursor::overlay::send_command(
                cursor_overlay::OverlayCommand::PinAbove(wid as u64)
            );
        }

        let mod_suffix = if modifiers.is_empty() {
            String::new()
        } else {
            format!(" with {}", modifiers.join("+"))
        };
        let btn_suffix = if button_str == "left" { String::new() } else {
            format!(" ({button_str} button)")
        };

        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Posted drag{btn_suffix}{mod_suffix} to pid {pid} \
                 from window-pixel ({}, {}) → ({}, {}), \
                 screen ({}, {}) → ({}, {}) \
                 in {duration_ms}ms / {steps} steps.{}",
                from_x as i64, from_y as i64,
                to_x   as i64, to_y   as i64,
                from_sx as i64, from_sy as i64,
                to_sx   as i64, to_sy   as i64,
                changes.result_suffix(),
            )),
            Ok(Err(e)) => ToolResult::error(format!("drag failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
