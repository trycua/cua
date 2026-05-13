use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct GetScreenSizeTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        // Matches `GetScreenSizeTool.swift` description verbatim.
        name: "get_screen_size".into(),
        description: "Return the logical size of the main display in points plus its backing \
            scale factor. Agents click in points; Retina displays have scale_factor 2.0. \
            Requires no TCC permissions.".into(),
        input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for GetScreenSizeTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, _args: Value) -> ToolResult {
        match main_screen_size() {
            Some((w, h, scale)) => {
                // Matches Swift text format 1:1.
                ToolResult::text(format!("✅ Main display: {w}x{h} points @ {scale}x"))
                    .with_structured(serde_json::json!({
                        "width": w, "height": h, "scale_factor": scale,
                    }))
            }
            None => ToolResult::error("No main display detected."),
        }
    }
}

/// Returns `(width_points, height_points, backing_scale_factor)` from
/// CoreGraphics — safe to call from any thread (no AppKit main-thread requirement).
///
/// The previous NSScreen-based implementation required `MainThreadMarker::new()`
/// which always returns `None` on async tokio threads, causing the tool to
/// return an error even when a display is attached.
fn main_screen_size() -> Option<(i64, i64, f64)> {
    use core_graphics::display::{CGMainDisplayID, CGDisplayBounds};

    // SAFETY: CGMainDisplayID / CGDisplayBounds are thread-safe CG APIs.
    let display_id = unsafe { CGMainDisplayID() };
    if display_id == 0 {
        return None;
    }
    let bounds = unsafe { CGDisplayBounds(display_id) };
    let w = bounds.size.width as i64;
    let h = bounds.size.height as i64;
    if w == 0 || h == 0 {
        return None;
    }

    let scale = get_backing_scale(display_id, w);
    Some((w, h, scale))
}

/// Estimate backing scale by comparing the display's pixel mode width to its
/// logical (CoreGraphics) bounds width.
fn get_backing_scale(display_id: u32, logical_w: i64) -> f64 {
    use core_graphics::display::CGDisplayPixelsWide;
    let pixel_w = unsafe { CGDisplayPixelsWide(display_id) } as i64;
    if pixel_w > 0 && logical_w > 0 {
        let ratio = pixel_w as f64 / logical_w as f64;
        // Round to nearest 0.5 to avoid floating point noise.
        (ratio * 2.0).round() / 2.0
    } else {
        1.0
    }
}
