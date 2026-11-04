use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    snapshot_store::ZoomContext,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::ToolState;

// ── zoom ──────────────────────────────────────────────────────────────────────

pub struct ZoomTool {
    pub(crate) state: Arc<ToolState>,
}
static ZOOM_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ZoomTool {
    fn def(&self) -> &ToolDef {
        ZOOM_DEF.get_or_init(|| ToolDef {
            name: "zoom".into(),
            description: "Capture a cropped JPEG of a window region (x1,y1)–(x2,y2) in \
                screenshot pixels, with 20% padding. Output is at most 500 px wide. \
                Corners, not a size: x1,y1 is the top-left and x2,y2 the bottom-right (x,y,width,height is also accepted and converted).\n\n\
                After a zoom, pass from_zoom=true to click/type_text to auto-translate \
                coordinates back to full-window space. Coordinate actions return \
                `screenshot_context_missing` when no current snapshot contains a \
                screenshot owned by this session. `from_zoom` actions return \
                `zoom_context_missing` when the zoom was never created or was replaced; call \
                `get_window_state`, then `zoom`, again on the same connection.".into(),
            input_schema: json!({
                "type":"object","required":["window_id","x1","y1","x2","y2"],"properties":{
                    "window_id":{"type":"integer","description":"Window id of the window captured by get_window_state."},
                    "pid":{"type":"integer","description":"Optional target pid. When omitted, the driver resolves the unique current snapshot for this session and window."},
                    "x1":{"type":"number","description":"Left edge of the region in window-local screenshot pixels."},
                    "y1":{"type":"number","description":"Top edge of the region in window-local screenshot pixels."},
                    "x2":{"type":"number","description":"Right edge of the region in window-local screenshot pixels."},
                    "y2":{"type":"number","description":"Bottom edge of the region in window-local screenshot pixels."}
                },"additionalProperties":false
            }),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let xid = match args.require_u64("window_id") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let requested_pid = match args.get("pid") {
            None => None,
            Some(value) => match value.as_u64().and_then(|pid| i32::try_from(pid).ok()) {
                Some(pid) => Some(pid),
                None => return ToolResult::error("pid must be a positive 32-bit integer"),
            },
        };
        let session_id = args.opt_str("_session_id");
        let (pid, screenshot) = match self.state.snapshots.screenshot_context_for_zoom(
            requested_pid,
            xid,
            session_id.as_deref(),
        ) {
            Ok(context) => context,
            Err(refusal) => return refusal,
        };
        let x1 = match args.opt_f64("x1") {
            Some(v) => v,
            None => return ToolResult::error("Missing x1"),
        };
        let y1 = match args.opt_f64("y1") {
            Some(v) => v,
            None => return ToolResult::error("Missing y1"),
        };
        let x2 = match args.opt_f64("x2") {
            Some(v) => v,
            None => return ToolResult::error("Missing x2"),
        };
        let y2 = match args.opt_f64("y2") {
            Some(v) => v,
            None => return ToolResult::error("Missing y2"),
        };
        if x2 <= x1 || y2 <= y1 {
            return ToolResult::error("x2 must be > x1 and y2 must be > y1");
        }

        let (x1, y1, x2, y2) = (
            x1 * screenshot.scale,
            y1 * screenshot.scale,
            x2 * screenshot.scale,
            y2 * screenshot.scale,
        );
        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Route through the Wayland-aware window capture dispatcher so
            // pure-Wayland sessions surface a typed "per-window capture not
            // supported yet" error instead of accidentally calling the
            // X11-only path with a foreign-toplevel id.
            let png = crate::wayland::screenshot_window_dispatch(xid)?;
            cursor_overlay::capture_utils::crop_png_to_jpeg(&png, x1, y1, x2, y2, 500)
        })
        .await;

        match result {
            Ok(Ok(crop)) => {
                if let Err(refusal) = state.snapshots.set_zoom(
                    pid,
                    session_id.as_deref(),
                    ZoomContext {
                        screenshot,
                        origin_x: crop.origin_x,
                        origin_y: crop.origin_y,
                        scale_inv: crop.scale_inv,
                    },
                ) {
                    return refusal;
                }
                use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                let b64 = B64.encode(&crop.jpeg_bytes);
                let (w, h) = (crop.out_w, crop.out_h);
                use cua_driver_core::protocol::Content;
                ToolResult {
                    content: vec![
                        Content::image_jpeg(b64),
                        Content::text(format!(
                            "Zoom ({x1:.0},{y1:.0})–({x2:.0},{y2:.0}) → {w}×{h} px JPEG."
                        )),
                    ],
                    is_error: None,
                    // Surface 7: `mime_type` mirrors the MCP image part's `mimeType`
                    // onto the structured payload (additive — `format` stays).
                    structured_content: Some(json!({
                        "width": w, "height": h, "format": "jpeg",
                        "mime_type": "image/jpeg"
                    })),
                    action_record: None,
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Zoom failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
