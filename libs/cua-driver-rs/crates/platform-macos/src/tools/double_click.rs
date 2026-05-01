use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{
    copy_action_names, element_screen_center, perform_action, kAXErrorSuccess,
    AXUIElementRef,
};

use super::ToolState;

pub struct DoubleClickTool {
    state: Arc<ToolState>,
}

impl DoubleClickTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "double_click".into(),
        description:
            "Double-click at (x, y) or on an AX element identified by element_index + window_id.\n\n\
             AX path (element_index provided): performs `AXOpen` when the element advertises it \
             (Finder items, openable list rows/cells); otherwise resolves the element's on-screen \
             center and falls back to a pixel double-click there.\n\n\
             Pixel path (x, y provided): two down/up pairs ~80 ms apart at the given coordinates."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "pid":           { "type": "integer" },
                "x":             { "type": "number",  "description": "Screen X coordinate (pixel path)." },
                "y":             { "type": "number",  "description": "Screen Y coordinate (pixel path)." },
                "window_id":     { "type": "integer", "description": "CGWindowID. Required when element_index is used." },
                "element_index": { "type": "integer", "description": "Element index from last get_window_state. Uses AX path." }
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
impl Tool for DoubleClickTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let element_index = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let window_id     = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);

        // ── AX element path ──────────────────────────────────────────────────
        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            let element_ptr = match self.state.element_cache.get_element_ptr(pid, wid, idx) {
                Some(p) => p,
                None    => return ToolResult::error(format!(
                    "Element index {idx} not found. Call get_window_state first."
                )),
            };

            let result = tokio::task::spawn_blocking(move || {
                ax_double_click(pid, element_ptr, idx)
            }).await;

            return match result {
                Ok(Ok(msg))  => ToolResult::text(msg),
                Ok(Err(e))   => ToolResult::error(format!("double_click failed: {e}")),
                Err(e)       => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // ── Pixel path ───────────────────────────────────────────────────────
        let mut cx = match args.get("x").and_then(|v| v.as_f64()) {
            Some(v) => v,
            None => return ToolResult::error(
                "Either element_index + window_id or x + y must be provided."
            ),
        };
        let mut cy = match args.get("y").and_then(|v| v.as_f64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: y"),
        };

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

        let result = tokio::task::spawn_blocking(move || {
            if let Some(wid) = window_id {
                crate::input::mouse::click_at_xy_with_window_local(
                    pid, screen_x, screen_y, win_local_x, win_local_y, wid, 2, &[],
                )
            } else {
                crate::input::mouse::click_at_xy(pid, screen_x, screen_y, 2, &[])
            }
        }).await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!("✅ Double-clicked at ({screen_x:.1}, {screen_y:.1}).")),
            Ok(Err(e)) => ToolResult::error(format!("Double-click failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking AX path ─────────────────────────────────────────────────────────

fn ax_double_click(pid: i32, element_ptr: usize, idx: usize) -> anyhow::Result<String> {
    let element = element_ptr as AXUIElementRef;

    // Try AXOpen first (Finder items, openable list rows, document cells).
    let actions = unsafe { copy_action_names(element) };
    if actions.iter().any(|a| a == "AXOpen") {
        let err = unsafe { perform_action(element, "AXOpen") };
        if err == kAXErrorSuccess {
            return Ok(format!("AXOpen performed on element [{idx}]."));
        }
        tracing::debug!("AXOpen returned {err} for element [{idx}], falling back to pixel double-click");
    }

    // Resolve screen center and fall back to pixel double-click.
    let (cx, cy) = unsafe { element_screen_center(element) }
        .ok_or_else(|| anyhow::anyhow!("Cannot resolve screen center for element [{idx}]"))?;

    crate::cursor::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: cx, y: cy });
    crate::input::mouse::click_at_xy(pid, cx, cy, 2, &[])?;
    Ok(format!("✅ Double-clicked element [{idx}] at ({cx:.1}, {cy:.1})."))
}
