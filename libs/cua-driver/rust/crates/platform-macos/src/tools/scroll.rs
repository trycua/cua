use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;

use super::ToolState;

pub struct ScrollTool {
    state: Arc<ToolState>,
}

impl ScrollTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
const DEFAULT_KEY_AMOUNT: u64 = 3;
const MAX_KEY_AMOUNT: f64 = 20_000.0;
const DEFAULT_PIXEL_AMOUNT: f64 = 120.0;

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "scroll".into(),
        description: "Scroll the target pid's focused region.\n\n\
            Default mode preserves the historical keystroke path: by='page' → PageDown/PageUp × amount; \
            by='line' → DownArrow/UpArrow × amount. Horizontal variants use Left/Right arrow keys.\n\n\
            For canvas, video, WebGL, and infinite-canvas apps that ignore key scrolling, use by='pixel' \
            or provide delta_x / delta_y. That path posts native pixel scroll-wheel events to the target \
            pid, can anchor the wheel event at x/y in window-local screenshot pixels, and can hold modifier \
            keys across the gesture.\n\n\
            Optional element_index + window_id pre-focuses the element before keystroke scrolling.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "pid": { "type": "integer" },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction. Required unless delta_x or delta_y is supplied."
                },
                "by": {
                    "type": "string",
                    "enum": ["line", "page", "pixel"],
                    "description": "Scroll granularity. Default: line. Use pixel for native wheel events."
                },
                "amount": {
                    "type": "number",
                    "minimum": 1,
                    "maximum": 20000,
                    "description": "Keystroke repetitions for line/page; pixel distance for by=pixel. Defaults: 3 for keys, 120 for pixels."
                },
                "delta_x": {
                    "type": "number",
                    "description": "Native wheel path only. Positive values scroll/pan the viewport right."
                },
                "delta_y": {
                    "type": "number",
                    "description": "Native wheel path only. Positive values scroll/pan the viewport down."
                },
                "x": {
                    "type": "number",
                    "description": "Native wheel path only. Window-local screenshot X coordinate to anchor the event."
                },
                "y": {
                    "type": "number",
                    "description": "Native wheel path only. Window-local screenshot Y coordinate to anchor the event."
                },
                "from_zoom": {
                    "type": "boolean",
                    "description": "When true, x/y are in the last zoom image for this pid; driver translates back to full-window coordinates."
                },
                "modifier": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Native wheel path only. Modifier keys held across the gesture: cmd, shift, option/alt, ctrl."
                },
                "steps": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Native wheel path only. Split the pixel delta into this many wheel events. Default: 1."
                },
                "delay_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 1000,
                    "description": "Native wheel path only. Delay between split wheel events. Default: 8."
                },
                "window_id": { "type": "integer" },
                "element_index": { "type": "integer" }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    })
}

#[async_trait]
impl Tool for ScrollTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") { Ok(v) => v, Err(e) => return e };
        let by = args.str_or("by", "line");
        if by != "line" && by != "page" && by != "pixel" {
            return ToolResult::error("by must be one of: line, page, pixel");
        }

        if by == "pixel" || has_number(&args, "delta_x") || has_number(&args, "delta_y") {
            return self.invoke_pixel_scroll(&args, pid).await;
        }

        self.invoke_key_scroll(&args, pid, &by).await
    }
}

impl ScrollTool {
    async fn invoke_key_scroll(&self, args: &Value, pid: i32, by: &str) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let direction = match args.require_str("direction") { Ok(v) => v, Err(e) => return e };
        let amount = coerce_number(args, "amount").unwrap_or(DEFAULT_KEY_AMOUNT as f64);
        if !amount.is_finite() || amount <= 0.0 {
            return ToolResult::error("amount must be a positive finite number");
        }
        if amount > MAX_KEY_AMOUNT {
            return ToolResult::error("amount must be <= 20000");
        }
        let amount = amount.round().clamp(1.0, MAX_KEY_AMOUNT) as usize;
        let element_index = args.opt_u64("element_index").map(|v| v as usize);
        let window_id = args.opt_u64("window_id").map(|v| v as u32);

        // Resolve the pre-focus element pointer (if requested) outside
        // the suppression closure — only the focus_element() write itself
        // needs to run under suppression, the cache lookup does not.
        let pre_focus_ptr: Option<usize> = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            self.state.element_cache.get_element_ptr(pid, wid, idx)
        } else {
            None
        };

        let key = match (by, direction.as_str()) {
            ("page", "down")  | (_, "down") if by == "page"  => "pagedown",
            ("page", "up")    | (_, "up")   if by == "page"  => "pageup",
            ("line", "down")  | (_, "down")                  => "down",
            ("line", "up")    | (_, "up")                    => "up",
            (_, "left")                                       => "left",
            (_, "right")                                      => "right",
            _                                                 => "down",
        };
        let key = key.to_owned();

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Scroll keystrokes (PageDown / arrow) into search-box autocomplete
        // can spawn floating helper windows; rare but real. Wrap for parity
        // with the other action tools.
        //
        // The AX focus_element() pre-write also runs inside the closure so
        // any reflex activations it triggers are caught by both the wildcard
        // snapshot suppressor and the targeted FocusGuard lease.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "scroll.CGEvent",
            || async move {
                // Pre-focus the element under suppression so its
                // side-effects are captured by the snapshot + lease.
                if let Some(element_ptr) = pre_focus_ptr {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::input::ax_actions::focus_element(element_ptr)
                    }).await;
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }

                tokio::task::spawn_blocking(move || {
                    for _ in 0..amount {
                        if let Err(e) = crate::input::keyboard::press_key(pid, &key, &[]) {
                            return Err(e);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Ok(())
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Scrolled {direction} by {by} × {amount}.{}",
                changes.result_suffix()
            )),
            Ok(Err(e)) => ToolResult::error(format!("Scroll failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }

    async fn invoke_pixel_scroll(&self, args: &Value, pid: i32) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;

        let amount = coerce_number(args, "amount").unwrap_or(DEFAULT_PIXEL_AMOUNT);
        if !amount.is_finite() || amount <= 0.0 {
            return ToolResult::error("amount must be a positive finite number");
        }

        let explicit_dx = coerce_number(args, "delta_x");
        let explicit_dy = coerce_number(args, "delta_y");
        let (delta_x, delta_y) = match (explicit_dx, explicit_dy) {
            (Some(dx), Some(dy)) => (dx, dy),
            (Some(dx), None) => (dx, 0.0),
            (None, Some(dy)) => (0.0, dy),
            (None, None) => {
                let direction = match args.require_str("direction") { Ok(v) => v, Err(e) => return e };
                match direction_to_delta(&direction, amount) {
                    Some(delta) => delta,
                    None => return ToolResult::error("direction must be one of: up, down, left, right"),
                }
            }
        };

        if !delta_x.is_finite() || !delta_y.is_finite() {
            return ToolResult::error("delta_x and delta_y must be finite numbers");
        }
        if delta_x == 0.0 && delta_y == 0.0 {
            return ToolResult::error("delta_x and delta_y cannot both be zero");
        }

        let mut x = coerce_number(args, "x");
        let mut y = coerce_number(args, "y");
        if x.is_some() ^ y.is_some() {
            return ToolResult::error("x and y must be provided together");
        }

        let window_id = args.opt_u64("window_id").map(|v| v as u32);
        let from_zoom = args.bool_or("from_zoom", false);
        let modifiers: Vec<String> = args.str_array("modifier");
        let steps = args.u64_or("steps", 1).clamp(1, 100) as usize;
        let delay_ms = args.u64_or("delay_ms", 8).clamp(0, 1000);
        let cursor_key = super::cursor_tools::resolve_cursor_key(args);

        if from_zoom {
            let (Some(cx), Some(cy)) = (x, y) else {
                return ToolResult::error("from_zoom=true requires x and y");
            };
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(cx, cy);
                    x = Some(wx);
                    y = Some(wy);
                }
                None => return ToolResult::error(format!(
                    "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                )),
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            if let (Some(cx), Some(cy)) = (x, y) {
                x = Some(cx * ratio);
                y = Some(cy * ratio);
            }
        }
        if (x.is_some() || y.is_some()) && window_id.is_none() {
            return ToolResult::error(
                "window_id is required when x/y are provided; x/y are window-local screenshot coordinates."
            );
        }

        let anchor = resolve_scroll_anchor(window_id, x, y).await;
        let (screen_point, window_local) = match anchor {
            Ok(anchor) => anchor,
            Err(e) => return ToolResult::error(e),
        };

        if let (Some(wid), Some((screen_x, screen_y))) = (window_id, screen_point) {
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::PinAbove(wid as u64),
            );
            crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), screen_x, screen_y).await;
            self.state.cursor_registry.update_position(&cursor_key, screen_x, screen_y);
        }

        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);
        let mods_owned = modifiers.clone();

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "scroll.CGWheel",
            || async move {
                tokio::task::spawn_blocking(move || {
                    let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                    for i in 0..steps {
                        let step_dx = split_delta(delta_x, steps, i);
                        let step_dy = split_delta(delta_y, steps, i);
                        crate::input::mouse::scroll_wheel_at_xy(
                            pid,
                            screen_point,
                            window_local,
                            window_id,
                            step_dx,
                            step_dy,
                            &m,
                        )?;
                        if delay_ms > 0 && i + 1 < steps {
                            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                        }
                    }
                    Ok(())
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;
        let mod_suffix = if modifiers.is_empty() {
            String::new()
        } else {
            format!(" with {}", modifiers.join("+"))
        };
        let anchor_suffix = match screen_point {
            Some((sx, sy)) => format!(" at screen ({}, {})", sx as i64, sy as i64),
            None => String::new(),
        };

        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Posted pixel scroll{mod_suffix} to pid {pid} \
                 delta ({}, {}){anchor_suffix} in {steps} step(s).{}",
                delta_x.round() as i64,
                delta_y.round() as i64,
                changes.result_suffix(),
            )),
            Ok(Err(e)) => ToolResult::error(format!("Pixel scroll failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

fn has_number(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_f64()).is_some()
}

fn coerce_number(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

fn direction_to_delta(direction: &str, amount: f64) -> Option<(f64, f64)> {
    match direction {
        "up" => Some((0.0, -amount)),
        "down" => Some((0.0, amount)),
        "left" => Some((-amount, 0.0)),
        "right" => Some((amount, 0.0)),
        _ => None,
    }
}

fn split_delta(total: f64, steps: usize, index: usize) -> f64 {
    let steps = steps.max(1) as f64;
    let prev = (total * index as f64 / steps).round();
    let next = (total * (index + 1) as f64 / steps).round();
    next - prev
}

async fn resolve_scroll_anchor(
    window_id: Option<u32>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(Option<(f64, f64)>, Option<(f64, f64)>), String> {
    let (Some(cx), Some(cy)) = (x, y) else {
        if let Some(wid) = window_id {
            let bounds = tokio::task::spawn_blocking(move || crate::windows::window_bounds_by_id(wid))
                .await
                .unwrap_or(None);
            return match bounds {
                Some(b) => {
                    let local = (b.width / 2.0, b.height / 2.0);
                    Ok((Some((b.x + local.0, b.y + local.1)), Some(local)))
                }
                None => Err(format!("No window bounds found for window_id={wid}. Provide x and y.")),
            };
        }
        return Ok((None, None));
    };

    if let Some(wid) = window_id {
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
            let local = (cx / scale, cy / scale);
            Ok((Some((b.x + local.0, b.y + local.1)), Some(local)))
        } else {
            Err(format!("No window bounds found for window_id={wid}."))
        }
    } else {
        Ok((Some((cx, cy)), None))
    }
}

#[cfg(test)]
mod tests {
    use super::{direction_to_delta, split_delta};

    #[test]
    fn direction_to_delta_uses_viewport_semantics() {
        assert_eq!(direction_to_delta("up", 120.0), Some((0.0, -120.0)));
        assert_eq!(direction_to_delta("down", 120.0), Some((0.0, 120.0)));
        assert_eq!(direction_to_delta("left", 80.0), Some((-80.0, 0.0)));
        assert_eq!(direction_to_delta("right", 80.0), Some((80.0, 0.0)));
        assert_eq!(direction_to_delta("sideways", 80.0), None);
    }

    #[test]
    fn split_delta_preserves_rounded_total() {
        let parts: Vec<f64> = (0..4).map(|i| split_delta(121.0, 4, i)).collect();
        assert_eq!(parts, vec![30.0, 30.0, 31.0, 30.0]);
        assert_eq!(parts.iter().sum::<f64>(), 121.0);

        let parts: Vec<f64> = (0..4).map(|i| split_delta(-121.0, 4, i)).collect();
        assert_eq!(parts, vec![-30.0, -30.0, -31.0, -30.0]);
        assert_eq!(parts.iter().sum::<f64>(), -121.0);
    }
}
