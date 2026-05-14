use async_trait::async_trait;
use mcp_server::{protocol::{ToolResult, Content}, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct ScreenshotTool {
    pub state: Arc<ToolState>,
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "screenshot".into(),
        description:
            "Capture a screenshot. Returns base64-encoded image data in the requested format \
             (default png).\n\n\
             Without `window_id`, captures the full main display. With `window_id`, captures \
             just that window (pair with `list_windows` or `get_accessibility_tree` which return \
             window IDs).\n\n\
             Requires the Screen Recording TCC grant — call `check_permissions` first if unsure."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "window_id": {
                    "type": "integer",
                    "description": "Optional CGWindowID to capture just that window. Without it, captures the full main display."
                },
                "format": {
                    "type": "string",
                    "enum": ["png", "jpeg"],
                    "description": "Image format. Default: png."
                },
                "quality": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 95,
                    "description": "JPEG quality 1-95; ignored for png. Default: 95."
                }
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: false,
        open_world: false,
    })
}

#[async_trait]
impl Tool for ScreenshotTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let window_id = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);
        let format    = args.get("format").and_then(|v| v.as_str()).unwrap_or("png").to_owned();
        let quality   = args.get("quality").and_then(|v| v.as_u64()).unwrap_or(95) as u8;
        let use_jpeg  = format == "jpeg";
        let max_dim   = self.state.config.read().unwrap().max_image_dimension;

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, u32, u32, bool)> {
            let png_bytes = if let Some(wid) = window_id {
                crate::capture::screenshot_window_bytes(wid)?
            } else {
                crate::capture::screenshot_display_bytes()?
            };
            let png_bytes = crate::capture::resize_png_if_needed(&png_bytes, max_dim)?;
            let (w, h) = crate::capture::png_dimensions(&png_bytes)?;
            if use_jpeg {
                let jpeg = crate::capture::png_bytes_to_jpeg(&png_bytes, quality)?;
                let b64 = base64_encode(&jpeg);
                Ok((b64, w, h, true))
            } else {
                let b64 = base64_encode(&png_bytes);
                Ok((b64, w, h, false))
            }
        }).await;

        match result {
            Ok(Ok((b64, w, h, is_jpeg))) => {
                let format_str = if is_jpeg { "jpeg" } else { "png" };
                let target_str = if let Some(wid) = window_id {
                    format!("window {wid}")
                } else {
                    "main display".to_owned()
                };

                // List visible windows (matching Swift reference).
                let mut summary_lines = vec![
                    format!("Screenshot — {w}x{h} {format_str} ({target_str})")
                ];
                let visible = crate::windows::visible_windows();
                if !visible.is_empty() {
                    summary_lines.push("\nOn-screen windows:".to_owned());
                    for w_info in &visible {
                        let title = if w_info.title.is_empty() {
                            "(no title)".to_owned()
                        } else {
                            format!("\"{}\"", w_info.title)
                        };
                        summary_lines.push(format!(
                            "- {} (pid {}) {} [window_id: {}]",
                            w_info.app_name, w_info.pid, title, w_info.window_id
                        ));
                    }
                    summary_lines.push(
                        "→ Call get_window_state(pid, window_id) to inspect a window's UI.".to_owned()
                    );
                }
                let summary = summary_lines.join("\n");

                let image_content = if is_jpeg {
                    Content::image_jpeg(b64)
                } else {
                    Content::image_png(b64)
                };

                ToolResult {
                    content: vec![image_content, Content::text(summary)],
                    is_error: None,
                    structured_content: Some(serde_json::json!({
                        "width": w, "height": h, "format": format_str
                    })),
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Screenshot failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    BASE64.encode(data)
}
