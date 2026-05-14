//! Claude Code computer-use compatibility screenshot tool.
//!
//! Replaces the regular `screenshot` tool in `--claude-code-computer-use-compat` mode.
//! Differences from the regular tool:
//! - `pid` and `window_id` are both required (regular tool makes window_id optional).
//! - Returns JPEG at 85% quality (Claude Code vision flows prefer smaller images).
//! - Validates that the target window is on-screen and layer-0 before capturing.
//! - Includes a short text note directing the caller to use pixel-addressed tools.

use async_trait::async_trait;
use mcp_server::{protocol::{ToolResult, Content}, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct ClaudeCodeCompatScreenshotTool {
    pub state: Arc<ToolState>,
}

impl ClaudeCodeCompatScreenshotTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "screenshot".into(),
        description:
            "Capture a target window and return a JPEG image. Coordinates accepted by \
             CuaDriver's pixel tools are pixels in this window screenshot's coordinate space.\n\n\
             This is the compatibility anchor for Claude Code vision flows: CuaDriver remains \
             window-scoped, and all other tools are the normal CuaDriver tools."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "window_id"],
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "Target process ID from list_windows or launch_app."
                },
                "window_id": {
                    "type": "integer",
                    "description": "Target CGWindowID from list_windows or launch_app."
                }
            },
            "additionalProperties": false
        }),
        read_only:   true,
        destructive: false,
        idempotent:  false,
        open_world:  false,
    })
}

#[async_trait]
impl Tool for ClaudeCodeCompatScreenshotTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let window_id = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: window_id"),
        };

        // Validate: window must be visible and layer-0.
        let window = {
            let wid = window_id;
            let p = pid;
            tokio::task::spawn_blocking(move || {
                crate::windows::all_windows()
                    .into_iter()
                    .find(|w| w.window_id == wid && w.pid == p && w.layer == 0
                          && w.is_on_screen
                          && w.bounds.width > 1.0
                          && w.bounds.height > 1.0)
            }).await.unwrap_or(None)
        };

        let window = match window {
            Some(w) => w,
            None => return ToolResult::error(format!(
                "No visible layer-0 window {window_id} found for pid {pid}. \
                 Use list_windows to choose an on-screen target window."
            )),
        };

        let max_dim = self.state.config.read().unwrap().max_image_dimension;

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, u32, u32)> {
            let png_bytes = crate::capture::screenshot_window_bytes(window_id)?;
            let png_bytes = crate::capture::resize_png_if_needed(&png_bytes, max_dim)?;
            let (w, h) = crate::capture::png_dimensions(&png_bytes)?;
            let jpeg = crate::capture::png_bytes_to_jpeg(&png_bytes, 85)?;
            use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
            Ok((BASE64.encode(&jpeg), w, h))
        }).await;

        match result {
            Ok(Ok((b64, w, h))) => {
                let app_name = window.app_name;
                let summary = format!(
                    "Captured window screenshot {w}x{h} for {app_name} \
                     [pid: {pid}, window_id: {window_id}]. \
                     Use CuaDriver pixel tools with this window-local coordinate space."
                );
                ToolResult {
                    content: vec![
                        Content::image_jpeg(b64),
                        Content::text(summary),
                    ],
                    is_error: None,
                    structured_content: Some(serde_json::json!({
                        "pid": pid,
                        "window_id": window_id,
                        "width": w,
                        "height": h,
                        "format": "jpeg"
                    })),
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Screenshot failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
