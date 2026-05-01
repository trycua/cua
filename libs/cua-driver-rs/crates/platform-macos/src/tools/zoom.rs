use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use mcp_server::{protocol::{ToolResult, Content}, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::{ToolState, ZoomContext};

pub struct ZoomTool {
    pub state: Arc<ToolState>,
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "zoom".into(),
        description: "Capture a cropped JPEG of a window region (x1,y1)–(x2,y2) in screenshot \
            pixel coordinates, with 20% padding added on each side. The output image is at most \
            500 px wide.\n\n\
            After a zoom, pass `from_zoom=true` to click/type_text to auto-translate coordinates \
            back to full-window space.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["window_id", "x1", "y1", "x2", "y2"],
            "properties": {
                "window_id": { "type": "integer", "description": "CGWindowID from list_windows." },
                "pid":       { "type": "integer", "description": "Target pid — required for from_zoom click/type translation." },
                "x1": { "type": "number", "description": "Left edge of region in screenshot pixels." },
                "y1": { "type": "number", "description": "Top edge of region in screenshot pixels." },
                "x2": { "type": "number", "description": "Right edge of region in screenshot pixels." },
                "y2": { "type": "number", "description": "Bottom edge of region in screenshot pixels." }
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for ZoomTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let window_id = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: window_id"),
        };
        let pid = args.get("pid").and_then(|v| v.as_i64()).map(|v| v as i32);
        let x1 = match args.get("x1").and_then(|v| v.as_f64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: x1"),
        };
        let y1 = match args.get("y1").and_then(|v| v.as_f64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: y1"),
        };
        let x2 = match args.get("x2").and_then(|v| v.as_f64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: x2"),
        };
        let y2 = match args.get("y2").and_then(|v| v.as_f64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: y2"),
        };

        if x2 <= x1 || y2 <= y1 {
            return ToolResult::error("x2 must be > x1 and y2 must be > y1");
        }

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            let png_bytes = crate::capture::screenshot_window_bytes(window_id)?;
            cursor_overlay::capture_utils::crop_png_to_jpeg(&png_bytes, x1, y1, x2, y2, 500)
        }).await;

        match result {
            Ok(Ok(crop)) => {
                // Store zoom context so from_zoom clicks can translate back.
                if let Some(p) = pid {
                    state.zoom_registry.set(p, ZoomContext {
                        origin_x: crop.origin_x,
                        origin_y: crop.origin_y,
                        scale_inv: crop.scale_inv,
                    });
                }
                let (w, h) = (crop.out_w, crop.out_h);
                let b64 = BASE64.encode(&crop.jpeg_bytes);
                ToolResult {
                    content: vec![
                        Content::image_jpeg(b64),
                        Content::text(format!(
                            "Zoom region ({x1:.0},{y1:.0})–({x2:.0},{y2:.0}) → {w}×{h} px JPEG."
                        )),
                    ],
                    is_error: None,
                    structured_content: Some(serde_json::json!({
                        "width": w, "height": h, "format": "jpeg"
                    })),
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Zoom failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
