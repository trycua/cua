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
        description: "Capture a JPEG crop (max 500 px wide, 20% padding) of window region \
            (x1,y1)-(x2,y2) in screenshot pixels. Then pass `from_zoom: true` to click/drag to \
            use coordinates read off the crop.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["window_id", "x1", "y1", "x2", "y2"],
            "properties": {
                "window_id": { "type": "integer", "description": "Window ID from list_windows." },
                "pid":       { "type": "integer", "description": "Target pid; inferred from the current snapshot if omitted." },
                "x1": { "type": "number", "description": "Left edge." },
                "y1": { "type": "number", "description": "Top edge." },
                "x2": { "type": "number", "description": "Right edge." },
                "y2": { "type": "number", "description": "Bottom edge." }
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
        let result = tokio::task::spawn_blocking(move || {
            let png_bytes = crate::capture::screenshot_window_bytes(window_id)?;
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
                            "Zoom region ({x1:.0},{y1:.0})–({x2:.0},{y2:.0}) → {w}×{h} px JPEG."
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

#[cfg(test)]
mod tests {
    use super::def;

    #[test]
    fn schema_keeps_pid_optional_for_window_owned_zoom_lookup() {
        let required = def().input_schema["required"].as_array().unwrap();
        assert!(!required.iter().any(|field| field == "pid"));
        assert!(def().input_schema["properties"].get("pid").is_some());
    }
}
