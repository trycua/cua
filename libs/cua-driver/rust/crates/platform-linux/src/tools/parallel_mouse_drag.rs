use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{linux_input_error, parse_mouse_button, window_local_to_screen, ToolState};

pub struct ParallelMouseDragTool {
    pub(crate) state: Arc<ToolState>,
}
static PMDRAG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

/// cua-compositor path for parallel_mouse_drag: build window-local drag paths
/// and run them as concurrent multi-cursor injections over the control socket.
/// Coordinates stay window-local (the compositor maps them per app_id), so no
/// X11 geometry/MPX is needed — the X11 path's hard blocker on Wayland.
async fn parallel_drag_inject(args: &Value) -> ToolResult {
    let Some(items) = args.get("drags").and_then(|v| v.as_array()) else {
        return ToolResult::error("drags[] is required.");
    };
    if items.len() < 2 {
        return ToolResult::error("parallel_mouse_drag requires at least two drag items.");
    }
    let mut drags = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let Some(xid) = item.get("window_id").and_then(|v| v.as_u64()) else {
            return ToolResult::error("each drag item requires window_id.");
        };
        let local: Vec<(f64, f64)> = if let Some(pts) = item.get("path").and_then(|v| v.as_array())
        {
            pts.iter()
                .filter_map(|p| {
                    let a = p.as_array()?;
                    Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
                })
                .collect()
        } else {
            let g = |k: &str| item.get(k).and_then(|v| v.as_f64());
            match (g("from_x"), g("from_y"), g("to_x"), g("to_y")) {
                (Some(fx), Some(fy), Some(tx), Some(ty)) => vec![(fx, fy), (tx, ty)],
                _ => {
                    return ToolResult::error(
                        "each drag item requires path[] or from_x/from_y/to_x/to_y.",
                    )
                }
            }
        };
        if local.len() < 2 {
            return ToolResult::error("drag path needs at least 2 points.");
        }
        let steps = item
            .get("steps")
            .and_then(|v| v.as_u64())
            .unwrap_or(60)
            .clamp(1, 300) as usize;
        let x_button = parse_mouse_button(
            item.get("button")
                .and_then(|v| v.as_str())
                .unwrap_or("left"),
        ) as u32;
        let app = match tokio::task::spawn_blocking(move || {
            crate::wayland::inject_target_for_window(xid)
        })
        .await
        {
            Ok(Ok(target)) => target,
            Ok(Err(error)) => return ToolResult::error(error.to_string()),
            Err(error) => return ToolResult::error(format!("Task error: {error}")),
        };
        drags.push(crate::wayland::InjectDrag {
            app_id: app,
            idx: i,
            x_button,
            path: local,
            steps,
        });
    }
    let n = drags.len();
    match tokio::task::spawn_blocking(move || crate::wayland::inject_parallel_drags(&drags)).await {
        Ok(Ok(())) => ToolResult::text(format!(
            "Ran {n} concurrent drags (multi-cursor via cua-compositor)."
        )),
        Ok(Err(e)) => ToolResult::error(e.to_string()),
        Err(e) => ToolResult::error(format!("Task error: {e}")),
    }
}

#[async_trait]
impl Tool for ParallelMouseDragTool {
    fn def(&self) -> &ToolDef {
        PMDRAG_DEF.get_or_init(|| ToolDef {
            name: "parallel_mouse_drag".into(),
            description: "Run multiple mouse drag gestures concurrently via Linux MPX/XI2 virtual master pointers. \
                Each drag item runs on its own session-scoped master pointer (true same-window concurrent draws on X11). \
                Each item presses once, glides continuously through its whole path, and releases once — one smooth held \
                drag, not a chain of clicks. A path is given either as a straight segment (from_x/from_y → to_x/to_y) or \
                as a function `fn` = y(x) sampled over [x_from, x_to] in window-local pixels (e.g. fn:\"x\" is a diagonal, \
                fn:\"300+120*sin(x/40)\" a sine wave). Functions support + - * / ^, sin/cos/tan, sqrt, abs, exp, ln, pi, e.".into(),
            input_schema: json!({"type":"object","required":["drags"],"properties":{
                "drags":{"type":"array","minItems":2,"description":"Two or more drag gestures to run concurrently, each on its own virtual master pointer.","items":{"type":"object","required":["session","window_id"],"properties":{
                    "session":{"type":"string","description":"Session/cursor id; also keys the virtual master pointer."},
                    "window_id":{"type":"integer","description":"Window id from list_windows."},
                    "path":{"type":"array","items":{"type":"array","items":{"type":"number"}},"description":"Explicit window-local waypoints [[x,y],...] (>=2); pressed once, glided through, released once. Takes precedence over fn/from-to."},
                    "fn":{"type":"string","description":"Expression y(x) in window-local pixels; sampled over [x_from,x_to]. Mutually exclusive with from_x/to_x."},
                    "x_from":{"type":"number","description":"Domain start (window-local x) when `fn` is used."},
                    "x_to":{"type":"number","description":"Domain end (window-local x) when `fn` is used."},
                    "samples":{"type":"integer","minimum":2,"maximum":400,"description":"Waypoints sampled along `fn`. Default: 80."},
                    "from_x":{"type":"number","description":"Straight-segment start X (window-local pixels)."},
                    "from_y":{"type":"number","description":"Straight-segment start Y (window-local pixels)."},
                    "to_x":{"type":"number","description":"Straight-segment end X (window-local pixels)."},
                    "to_y":{"type":"number","description":"Straight-segment end Y (window-local pixels)."},
                    "button": cua_driver_core::tool_schema::button_schema(),
                    "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Default: 1500 for fn paths, 500 for straight."},
                    "steps":{"type":"integer","minimum":1,"maximum":300,"description":"Motion sub-steps along the whole path. Default: scaled to path length."}
                },"additionalProperties":false}}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Nested cua-compositor: run the drags as concurrent multi-cursor
        // injections (window-local, no X11 MPX/geometry needed).
        if crate::wayland::is_inject_mode() {
            return parallel_drag_inject(&args).await;
        }
        // Native Wayland without the inject socket: MPX/XI2 + uinput master
        // pointers don't exist on Wayland. Surface a typed error instead of
        // silently calling the X11 path that's guaranteed to fail.
        if crate::wayland::is_wayland() {
            return ToolResult::error(
                "parallel_mouse_drag requires the cua-compositor inject socket on Wayland \
                 (set CUA_INJECT_SOCKET to the cua-compositor control socket), \
                 or run the target under X11.",
            );
        }
        match tokio::task::spawn_blocking(crate::input::check_parallel_pointer_support).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return ToolResult::error(e.to_string()),
            Err(e) => return ToolResult::error(format!("Task error: {e}")),
        }

        let Some(items) = args.get("drags").and_then(|v| v.as_array()) else {
            return ToolResult::error("drags[] is required.");
        };
        if items.len() < 2 {
            return ToolResult::error("parallel_mouse_drag requires at least two drag items.");
        }

        let mut drags = Vec::with_capacity(items.len());
        for item in items {
            let Some(session) = item.get("session").and_then(|v| v.as_str()) else {
                return ToolResult::error("each drag item requires session.");
            };
            let Some(xid) = item.get("window_id").and_then(|v| v.as_u64()) else {
                return ToolResult::error("each drag item requires window_id.");
            };

            // Build the window-local waypoint path from one of: an explicit
            // `path` of [x,y] points, a function `fn` (y = f(x) sampled over
            // [x_from, x_to]), or a straight from→to segment.
            let is_fn = item.get("fn").and_then(|v| v.as_str()).is_some();
            let local: Vec<(f64, f64)> = if let Some(pts) =
                item.get("path").and_then(|v| v.as_array())
            {
                let mut out = Vec::with_capacity(pts.len());
                for p in pts {
                    let a = p.as_array();
                    let (Some(px), Some(py)) = (
                        a.and_then(|a| a.first()).and_then(|v| v.as_f64()),
                        a.and_then(|a| a.get(1)).and_then(|v| v.as_f64()),
                    ) else {
                        return ToolResult::error("each `path` entry must be [x, y].");
                    };
                    out.push((px, py));
                }
                if out.len() < 2 {
                    return ToolResult::error("`path` needs at least 2 points.");
                }
                out
            } else if let Some(expr_str) = item.get("fn").and_then(|v| v.as_str()) {
                let Some(x_from) = item.get("x_from").and_then(|v| v.as_f64()) else {
                    return ToolResult::error("`fn` requires x_from.");
                };
                let Some(x_to) = item.get("x_to").and_then(|v| v.as_f64()) else {
                    return ToolResult::error("`fn` requires x_to.");
                };
                let samples = item
                    .get("samples")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(80)
                    .clamp(2, 400);
                match crate::input::sample_function(expr_str, x_from, x_to, samples) {
                    Ok(pts) => pts,
                    Err(e) => return ToolResult::error(e.to_string()),
                }
            } else {
                let coerce = |k: &str| item.get(k).and_then(|v| v.as_f64());
                match (coerce("from_x"), coerce("from_y"), coerce("to_x"), coerce("to_y")) {
                    (Some(fx), Some(fy), Some(tx), Some(ty)) => vec![(fx, fy), (tx, ty)],
                    _ => return ToolResult::error("each drag item requires either `fn`+x_from+x_to, or from_x/from_y/to_x/to_y."),
                }
            };

            let button = parse_mouse_button(
                item.get("button")
                    .and_then(|v| v.as_str())
                    .unwrap_or("left"),
            );
            let duration_ms = item
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(if is_fn { 1500 } else { 500 });

            // One translate gives the window origin; the path is a pure offset.
            let origin =
                match tokio::task::spawn_blocking(move || window_local_to_screen(xid, 0.0, 0.0))
                    .await
                {
                    Ok(Ok(o)) => o,
                    Ok(Err(e)) => return ToolResult::error(e.to_string()),
                    Err(e) => return ToolResult::error(format!("Task error: {e}")),
                };
            let path: Vec<(i32, i32)> = local
                .iter()
                .map(|(lx, ly)| {
                    (
                        (origin.0 + lx).round() as i32,
                        (origin.1 + ly).round() as i32,
                    )
                })
                .collect();

            // Default sub-step count scaled to path length (smooth glide),
            // overridable via `steps`.
            let total_len: f64 = path
                .windows(2)
                .map(|w| {
                    (((w[1].0 - w[0].0) as f64).powi(2) + ((w[1].1 - w[0].1) as f64).powi(2)).sqrt()
                })
                .sum();
            let steps = item
                .get("steps")
                .and_then(|v| v.as_u64())
                .map(|s| (s as usize).clamp(1, 300))
                .unwrap_or_else(|| ((total_len / 3.0).round() as usize).clamp(24, 300));

            let start = path[0];
            self.state
                .cursor_registry
                .update_position(session, start.0 as f64, start.1 as f64);
            crate::overlay::send_command_for(
                session.to_owned(),
                cursor_overlay::OverlayCommand::PinAbove(xid),
            );
            crate::overlay::send_command_for(
                session.to_owned(),
                cursor_overlay::OverlayCommand::SnapTo {
                    x: start.0 as f64,
                    y: start.1 as f64,
                    heading_radians: None,
                },
            );
            crate::overlay::send_command_for(
                session.to_owned(),
                cursor_overlay::OverlayCommand::SetPressed(true),
            );

            drags.push((
                session.to_owned(),
                crate::input::VirtualPointerDrag {
                    target_window: xid,
                    button,
                    path,
                    duration_ms,
                    steps,
                },
            ));
        }

        let drags_for_task = drags.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::send_parallel_virtual_pointer_drags(&drags_for_task)
        })
        .await;
        match result {
            Ok(Ok(())) => {
                for (session, drag) in &drags {
                    let n = drag.path.len();
                    let end = drag.path[n - 1];
                    let prev = drag.path[n.saturating_sub(2)];
                    self.state
                        .cursor_registry
                        .update_position(session, end.0 as f64, end.1 as f64);
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::SnapTo {
                            x: end.0 as f64,
                            y: end.1 as f64,
                            heading_radians: Some(
                                ((end.1 - prev.1) as f64).atan2((end.0 - prev.0) as f64),
                            ),
                        },
                    );
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::SetPressed(false),
                    );
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::ClickPulse {
                            x: end.0 as f64,
                            y: end.1 as f64,
                        },
                    );
                }
                ToolResult::text(format!(
                    "✅ Ran {} MPX drag gesture(s) concurrently.",
                    drags.len()
                ))
                .with_structured(json!({"count": drags.len()}))
            }
            Ok(Err(e)) => {
                for (session, _) in &drags {
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::SetPressed(false),
                    );
                }
                linux_input_error(e)
            }
            Err(e) => {
                for (session, _) in &drags {
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::SetPressed(false),
                    );
                }
                ToolResult::error(format!("Task error: {e}"))
            }
        }
    }
}
