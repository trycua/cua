use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    snapshot_store::ZoomContext,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::ToolState;

#[cfg(test)]
use super::impl_::focus_by_pixel_click_args;

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
            // Description ported from Swift `ZoomTool.swift`.
            description: "Zoom into a rectangular region of a window screenshot at full (native) \
                resolution. Use this when `get_window_state` returned a resized image and you \
                need to read small text, identify icons, or verify UI details.\n\n\
                Corners, not a size: x1,y1 is the top-left and x2,y2 the bottom-right (x,y,width,height is also accepted and converted). \
                Coordinates `x1, y1, x2, y2` are in the same pixel space as the screenshot \
                returned by `get_window_state` (i.e. the resized image if `max_image_dimension` \
                is active). The maximum zoom region width is 500 px in scaled-image coordinates.\n\n\
                A 20% padding is automatically added on every side of the requested region so \
                the target remains visible even if the caller's coordinates are slightly off.\n\n\
                After a zoom, pass `from_zoom=true` to click/type_text to auto-translate \
                coordinates back to full-window space. Coordinate actions return \
                `screenshot_context_missing` when no current snapshot contains a \
                screenshot owned by this session. `from_zoom` actions return \
                `zoom_context_missing` when the zoom was never created or was replaced; call \
                `get_window_state`, then `zoom`, again on the same connection.\n\n\
                Windows-specific: `window_id` is the canonical addressing field (HWND). \
                `pid` is preferred when available; otherwise the driver resolves the unique \
                current snapshot owned by this session and window.".into(),
            input_schema: json!({
                "type":"object","required":["window_id","x1","y1","x2","y2"],"properties":{
                    "pid":{"type":"integer","description":"Optional target process ID. When omitted, the unique current snapshot for this session and HWND is used."},
                    "window_id":{"type":"integer","description":"HWND of the target window."},
                    "x1":{"type":"number","description":"Left edge of the region (resized-image pixels)."},
                    "y1":{"type":"number","description":"Top edge of the region (resized-image pixels)."},
                    "x2":{"type":"number","description":"Right edge of the region (resized-image pixels)."},
                    "y2":{"type":"number","description":"Bottom edge of the region (resized-image pixels)."}
                },"additionalProperties":false
            }),
            // Swift annotation: idempotent: false.
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let requested_pid = match args.get("pid") {
            None => None,
            Some(value) => match value.as_i64().and_then(|pid| i32::try_from(pid).ok()) {
                Some(pid) => Some(pid),
                None => return ToolResult::error("pid must be a 32-bit integer"),
            },
        };
        let hwnd = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required integer field window_id."),
        };
        use cua_driver_core::tool_args::ArgsExt;
        let coerce = |k: &str| {
            args.opt_f64(k)
                .or_else(|| args.opt_i64(k).map(|i| i as f64))
        };
        let (x1, y1, x2, y2) = match (coerce("x1"), coerce("y1"), coerce("x2"), coerce("y2")) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return ToolResult::error("Missing required region coordinates (x1, y1, x2, y2)."),
        };
        if x2 <= x1 || y2 <= y1 {
            return ToolResult::error("Invalid region: x2 must be > x1 and y2 must be > y1.");
        }
        if x2 - x1 > 500.0 {
            return ToolResult::error(format!(
                "Zoom region too wide: {} px > 500 px max. Use a narrower region (max 500 px in scaled-image coordinates).",
                (x2 - x1) as i64
            ));
        }

        // Issue #1880: the caller's coordinates are in the (possibly downscaled)
        // screenshot space of the last `get_window_state` call, but the crop below
        // runs on a fresh native-resolution capture. Scale by the stored resize
        // ratio so the crop lands on the intended region; the zoom context then
        // holds native-pixel values, which is what `from_zoom` clicks expect.
        let session_id = args.opt_str("_session_id");
        let (pid, screenshot) = match self.state.snapshots.screenshot_context_for_zoom(
            requested_pid,
            hwnd,
            session_id.as_deref(),
        ) {
            Ok(context) => context,
            Err(refusal) => return refusal,
        };
        let (nx1, ny1, nx2, ny2) = (
            x1 * screenshot.scale,
            y1 * screenshot.scale,
            x2 * screenshot.scale,
            y2 * screenshot.scale,
        );

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            let png = crate::capture::screenshot_window_bytes(hwnd)?;
            cursor_overlay::capture_utils::crop_png_to_jpeg(&png, nx1, ny1, nx2, ny2, 500)
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
                // Match Swift's text format 1:1.
                let summary = "✅ Zoomed region captured at native resolution. \
                    To click a target in this image, use \
                    `click(pid, x, y, from_zoom=true)` where x,y are pixel \
                    coordinates in THIS zoomed image — the driver maps them \
                    back automatically."
                    .to_owned();
                ToolResult {
                    content: vec![Content::image_jpeg(b64), Content::text(summary)],
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

#[cfg(test)]
mod snapshot_coordinate_tests {
    use super::{focus_by_pixel_click_args, ToolState, ZoomTool};
    use cua_driver_core::tool::Tool;

    #[test]
    fn schema_keeps_pid_optional_for_window_owned_lookup() {
        let tool = ZoomTool {
            state: ToolState::new(None),
        };
        let required = tool.def().input_schema["required"].as_array().unwrap();
        assert!(!required.iter().any(|field| field == "pid"));
        assert!(tool.def().input_schema["properties"].get("pid").is_some());
    }

    #[test]
    fn press_key_native_element_focus_skips_second_screenshot_scaling() {
        let native = focus_by_pixel_click_args(
            42,
            Some(7),
            120.0,
            80.0,
            false,
            None,
            Some("client-a".to_owned()),
            false,
            true,
        );
        assert_eq!(native["_native_coordinates"], true);

        let screenshot = focus_by_pixel_click_args(
            42,
            Some(7),
            60.0,
            40.0,
            false,
            None,
            Some("client-a".to_owned()),
            false,
            false,
        );
        assert!(screenshot.get("_native_coordinates").is_none());
    }
}
