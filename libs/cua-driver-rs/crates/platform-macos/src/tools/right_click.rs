use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{
    copy_action_names, perform_action, kAXErrorSuccess, AXUIElementRef,
    copy_string_attr,
};

use super::ToolState;

pub struct RightClickTool {
    state: Arc<ToolState>,
}

impl RightClickTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "right_click".into(),
        description:
            "Right-click against a target pid. Two addressing modes:\n\n\
             - `element_index` + `window_id` (from the last `get_window_state` snapshot) — \
               performs `AXShowMenu` on the cached element. Pure AX RPC, works on backgrounded / \
               hidden windows, no cursor move or focus steal. Requires a prior \
               `get_window_state(pid, window_id)` in this turn.\n\n\
             - `x`, `y` — synthesizes `rightMouseDown` / `rightMouseUp` CGEvent pair posted \
               to the pid. Driver converts image-pixel → screen-point internally. \
               `modifier` forces the CGEvent path (AX actions don't propagate modifier keys).\n\n\
             Exactly one of `element_index` or (`x` AND `y`) must be provided. `pid` always \
             required. `window_id` required when `element_index` is used."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "pid": { "type": "integer", "description": "Target process ID." },
                "element_index": {
                    "type": "integer",
                    "description": "Element index from last get_window_state. Routes through AXShowMenu. Requires window_id."
                },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID. Required when element_index is used."
                },
                "x": {
                    "type": "number",
                    "description": "X in window-local screenshot pixels. Must be provided together with y."
                },
                "y": {
                    "type": "number",
                    "description": "Y in window-local screenshot pixels. Must be provided together with x."
                },
                "modifier": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Modifier keys held during the right-click: cmd/shift/option/ctrl. Pixel path only."
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
impl Tool for RightClickTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };

        let element_index = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let window_id     = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);
        let x             = args.get("x").and_then(|v| v.as_f64());
        let y             = args.get("y").and_then(|v| v.as_f64());
        let has_xy        = x.is_some() && y.is_some();
        let partial_xy    = x.is_some() != y.is_some();
        let modifiers: Vec<String> = args.get("modifier")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();

        if partial_xy {
            return ToolResult::error("Provide both x and y together, not just one.");
        }
        if element_index.is_some() && has_xy {
            return ToolResult::error("Provide either element_index or (x, y), not both.");
        }
        if element_index.is_none() && !has_xy {
            return ToolResult::error(
                "Provide element_index or (x, y) to address the right-click target."
            );
        }
        if element_index.is_some() && window_id.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used."
            );
        }

        // ── AX element path ──────────────────────────────────────────────────
        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            let element_ptr = match self.state.element_cache.get_element_ptr(pid, wid, idx) {
                Some(p) => p,
                None    => return ToolResult::error(format!(
                    "Element index {idx} not found. Call get_window_state first."
                )),
            };

            let result = tokio::task::spawn_blocking(move || {
                ax_show_menu(element_ptr, idx)
            }).await;

            return match result {
                Ok(Ok(msg))  => ToolResult::text(msg),
                Ok(Err(e))   => ToolResult::error(format!("AXShowMenu failed: {e}")),
                Err(e)       => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // ── Pixel path ───────────────────────────────────────────────────────
        let (mut cx, mut cy) = (x.unwrap(), y.unwrap());
        // Scale back from downscaled-image space to native pixels when needed.
        if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            cx *= ratio;
            cy *= ratio;
        }

        // Window-local → screen coordinate translation + win-local logical coords
        // for CGEventSetWindowLocation (matches click.rs enhancement).
        let (screen_x, screen_y, win_local_x, win_local_y) = if let Some(wid) = window_id {
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
                let wx = cx / scale;
                let wy = cy / scale;
                (b.x + wx, b.y + wy, wx, wy)
            } else {
                (cx, cy, cx, cy)
            }
        } else {
            (cx, cy, cx, cy)
        };

        // Pin overlay above the target window before animating.
        if let Some(wid) = window_id {
            crate::cursor::overlay::send_command(
                cursor_overlay::OverlayCommand::PinAbove(wid as u64)
            );
        }
        // Animate cursor to the click point; wait for arrival before firing.
        crate::cursor::overlay::animate_cursor_to(screen_x, screen_y).await;
        crate::cursor::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: screen_x, y: screen_y });

        let mod_suffix = if modifiers.is_empty() {
            String::new()
        } else {
            format!(" with {}", modifiers.join("+"))
        };

        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            if window_id.is_some() {
                crate::input::mouse::right_click_at_xy_with_window_local(
                    pid, screen_x, screen_y, win_local_x, win_local_y, &m,
                )
            } else {
                crate::input::mouse::right_click_at_xy(pid, screen_x, screen_y, &m)
            }
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Right-clicked{mod_suffix} at ({screen_x:.1}, {screen_y:.1}).")),
            Ok(Err(e)) => ToolResult::error(format!("Right-click failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking AX path ─────────────────────────────────────────────────────────

fn ax_show_menu(element_ptr: usize, idx: usize) -> anyhow::Result<String> {
    let element = element_ptr as AXUIElementRef;

    let role  = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
    let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

    let advertised = unsafe { copy_action_names(element) };
    let err = unsafe { perform_action(element, "AXShowMenu") };

    if err != kAXErrorSuccess {
        anyhow::bail!("AXUIElementPerformAction(AXShowMenu) returned {err}");
    }

    let mut summary = format!("Shown menu for [{idx}] {role} \"{title}\".");
    if !advertised.iter().any(|a| a == "AXShowMenu") {
        let list = if advertised.is_empty() {
            "none".to_owned()
        } else {
            advertised.join(", ")
        };
        summary.push_str(&format!(
            "\n⚠️ Element does not advertise AXShowMenu (actions: {list}). \
             Action may have been a no-op. Retry with a pixel right-click \
             (right_click(pid, x, y)) if no menu appeared."
        ));
    }
    Ok(summary)
}
