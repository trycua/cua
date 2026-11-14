// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use cua_driver_core::{
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef},
};
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
            500 px wide. Corners, not a size: x1,y1 is the top-left and x2,y2 the bottom-right (x,y,width,height is also accepted and converted).\n\n\
            After a zoom, pass `from_zoom=true` to click/type_text to auto-translate coordinates \
            back to full-window space. Coordinate actions return `screenshot_context_missing` \
            when no current snapshot contains a screenshot owned by this session. \
            `from_zoom` actions return `zoom_context_missing` when the zoom was never created or \
            was replaced; call `get_window_state`, then `zoom`, again on the same connection.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["window_id", "x1", "y1", "x2", "y2"],
            "properties": {
                "window_id": { "type": "integer", "description": "CGWindowID from list_windows." },
                "pid":       { "type": "integer", "description": "Optional target pid. When omitted, the driver resolves the unique current snapshot for this session and window." },
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
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let mut args = args;
        super::default_window_for_pid(&mut args);
        if ["x1", "y1", "x2", "y2"]
            .iter()
            .all(|key| args.get(*key).is_none())
        {
            return ToolResult::error(
                "zoom needs a region of the window's screenshot: x1,y1 (top-left) and x2,y2 \
                 (bottom-right) in screenshot pixels, or x,y,width,height. To see the whole \
                 window, call get_window_state (it returns the screenshot).",
            );
        }
        let window_id = match args.require_u32("window_id") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let requested_pid = match args.get("pid") {
            None => None,
            Some(value) => match value.as_i64().and_then(|pid| i32::try_from(pid).ok()) {
                Some(pid) => Some(pid),
                None => return ToolResult::error("pid must be a 32-bit integer"),
            },
        };
        let session_id = args.opt_str("_session_id");
        let (pid, screenshot) = match self.state.snapshots.screenshot_context_for_zoom(
            requested_pid,
            u64::from(window_id),
            session_id.as_deref(),
        ) {
            Ok(context) => context,
            Err(refusal) => return refusal,
        };
        let x1 = match args.require_f64("x1") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let y1 = match args.require_f64("y1") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let x2 = match args.require_f64("x2") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let y2 = match args.require_f64("y2") {
            Ok(v) => v,
            Err(e) => return e,
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
        let frame_scale = screenshot.scale;
        // Every zoom captures the window now, so it shows the window as it is
        // after any earlier action, never an older frame.
        let result = tokio::task::spawn_blocking(move || {
            let png_bytes = crate::capture::screenshot_window_bytes(window_id)?;
            let (width, height) = image::ImageReader::new(std::io::Cursor::new(&png_bytes))
                .with_guessed_format()?
                .into_dimensions()?;
            if let Some(message) = region_outside_frame(x1, y1, width, height, frame_scale) {
                anyhow::bail!(message);
            }
            cursor_overlay::capture_utils::crop_png_to_jpeg(&png_bytes, x1, y1, x2, y2, 500)
        })
        .await;

        match result {
            Ok(Ok(crop)) => {
                // Store zoom context so from_zoom clicks can translate back.
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
                let (w, h) = (crop.out_w, crop.out_h);
                let b64 = BASE64.encode(&crop.jpeg_bytes);
                ToolResult {
                    content: vec![
                        Content::image_jpeg(b64),
                        Content::text(format!(
                            "Zoom region ({x1:.0},{y1:.0})–({x2:.0},{y2:.0}) → {w}×{h} px JPEG, \
                             captured now."
                        )),
                    ],
                    is_error: None,
                    structured_content: Some(serde_json::json!({
                        // `format` stays for back-compat. `mime_type` is the
                        // Surface-7 addition that mirrors the MCP image part's
                        // `mimeType` onto the structured payload, so consumers
                        // don't have to translate "jpeg" → "image/jpeg" or
                        // sniff base64 magic bytes.
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

/// Why a region whose top-left corner (native capture pixels) lies outside a
/// `width` x `height` capture cannot be zoomed: its coordinates came from a
/// different frame than the window's latest screenshot. `scale` converts
/// native pixels back to that screenshot's pixels.
fn region_outside_frame(x1: f64, y1: f64, width: u32, height: u32, scale: f64) -> Option<String> {
    if x1 < f64::from(width) && y1 < f64::from(height) {
        return None;
    }
    let scale = if scale > 0.0 { scale } else { 1.0 };
    Some(format!(
        "the region starts outside the window's latest screenshot ({}x{} screenshot pixels). \
         zoom takes coordinates in the frame of the latest get_window_state read of this window; \
         a read with another max_dimension or a resized window changes that frame",
        (f64::from(width) / scale).round(),
        (f64::from(height) / scale).round()
    ))
}

#[cfg(test)]
mod tests {
    use super::{def, region_outside_frame};

    #[test]
    fn a_region_inside_the_frame_is_kept() {
        assert_eq!(region_outside_frame(10.0, 10.0, 1600, 1200, 2.0), None);
    }

    #[test]
    fn a_region_from_another_frame_is_refused_with_the_frame_size() {
        let message = region_outside_frame(1700.0, 10.0, 1600, 1200, 2.0).expect("outside");
        assert!(message.contains("(800x600 screenshot pixels)"), "{message}");
    }

    #[test]
    fn schema_keeps_pid_optional_for_window_owned_zoom_lookup() {
        let required = def().input_schema["required"].as_array().unwrap();
        assert!(!required.iter().any(|field| field == "pid"));
        assert!(def().input_schema["properties"].get("pid").is_some());
    }
}
