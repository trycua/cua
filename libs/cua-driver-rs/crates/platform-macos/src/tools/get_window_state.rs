use async_trait::async_trait;
use mcp_server::{protocol::{ToolResult, Content}, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct GetWindowStateTool {
    state: Arc<ToolState>,
}

impl GetWindowStateTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_window_state".into(),
        description: "Walk a running app's AX tree and return a Markdown rendering of its UI, \
            tagging every actionable element with [element_index N]. Pass those indices to \
            click, type_text, press_key, etc.\n\n\
            INVARIANT: call get_window_state once per turn per (pid, window_id) before any \
            element-indexed action. The index map is replaced by the next snapshot.\n\n\
            Also captures a PNG screenshot of the specified window.\n\n\
            Optional `query` filters the tree_markdown to matching lines plus their ancestor \
            chain (case-insensitive substring). The element_index values are unchanged — \
            filtering only trims the rendered Markdown.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "window_id"],
            "properties": {
                "pid": { "type": "integer", "description": "Target process ID." },
                "window_id": { "type": "integer", "description": "Target window ID from list_windows." },
                "query": { "type": "string", "description": "Case-insensitive filter for tree_markdown." },
                "capture_mode": {
                    "type": "string",
                    "enum": ["som", "vision", "ax"],
                    "description": "som=AX+screenshot (default), vision=screenshot only (no AX walk), ax=AX only (no screenshot)."
                },
                "screenshot_out_file": {
                    "type": "string",
                    "description": "When set, write the PNG to this file path (~ expanded) instead of embedding base64 in the response. The structured output will contain screenshot_file_path instead."
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
impl Tool for GetWindowStateTool {
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
        let query = args.get("query").and_then(|v| v.as_str()).map(str::to_owned);
        let screenshot_out_file = args.get("screenshot_out_file").and_then(|v| v.as_str()).map(|s| {
            // Expand ~ prefix.
            if s.starts_with("~/") {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/{}", &s[2..])
            } else {
                s.to_owned()
            }
        });
        let default_mode = self.state.config.read().unwrap().capture_mode.clone();
        let capture_mode = args.get("capture_mode").and_then(|v| v.as_str()).unwrap_or(&default_mode);

        // Walk AX tree (unless vision-only mode). Accept "tree" as deprecated alias for "ax".
        let capture_mode = if capture_mode == "tree" { "ax" } else { capture_mode };
        let tree_result = if capture_mode != "vision" {
            let q = query.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::ax::tree::walk_tree(pid, Some(window_id), q.as_deref())
            }).await;
            match result {
                Ok(r) => Some(r),
                Err(e) => return ToolResult::error(format!("AX tree walk failed: {e}")),
            }
        } else {
            None
        };

        // Update element cache.
        if let Some(ref r) = tree_result {
            self.state.element_cache.update(pid, window_id, &r.nodes);
        }

        // Screenshot (unless ax-only mode).
        let max_dim = self.state.config.read().unwrap().max_image_dimension;
        // Returns (b64_or_path, final_w, final_h, Option<original_w>, is_file_path)
        let screenshot = if capture_mode != "ax" {
            let out_file = screenshot_out_file.clone();
            let res = tokio::task::spawn_blocking(move || -> anyhow::Result<(Option<String>, Option<String>, u32, u32, Option<u32>)> {
                use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                let raw = crate::capture::screenshot_window_bytes(window_id)?;
                let (orig_w, _orig_h) = crate::capture::png_dimensions(&raw)?;
                let png = crate::capture::resize_png_if_needed(&raw, max_dim)?;
                let (w, h) = crate::capture::png_dimensions(&png)?;
                let original_w = if w < orig_w { Some(orig_w) } else { None };
                if let Some(ref path) = out_file {
                    std::fs::write(path, &png)?;
                    Ok((None, Some(path.clone()), w, h, original_w))
                } else {
                    Ok((Some(BASE64.encode(&png)), None, w, h, original_w))
                }
            }).await;
            match res {
                Ok(Ok((b64, file_path, w, h, orig_w))) => {
                    // Record resize ratio so ClickTool can scale coordinates back up.
                    if let Some(ow) = orig_w {
                        if w > 0 {
                            self.state.resize_registry.set_ratio(pid, ow as f64 / w as f64);
                        }
                    } else {
                        self.state.resize_registry.clear_ratio(pid);
                    }
                    Some((b64, file_path, w, h))
                }
                Ok(Err(e)) => {
                    tracing::warn!("Screenshot failed for window {window_id}: {e}");
                    None
                }
                Err(e) => {
                    tracing::warn!("Screenshot task error for window {window_id}: {e}");
                    None
                }
            }
        } else {
            None
        };

        // Capture screenshot dimensions before consuming.
        let screenshot_dims = screenshot.as_ref().map(|(_, _, w, h)| (*w, *h));
        let screenshot_file_path = screenshot.as_ref().and_then(|(_, fp, _, _)| fp.clone());

        // Build response.
        let mut content: Vec<Content> = Vec::new();

        if let Some((b64_opt, _file_path, w, h)) = screenshot {
            if let Some(b64) = b64_opt {
                content.push(Content::image_png(b64));
            }

            // Summary text line (matching Swift reference format).
            let element_count = self.state.element_cache.element_count(pid, window_id);
            let summary = if let Some(ref r) = tree_result {
                format!(
                    "window_id={window_id} pid={pid} size={}x{} elements={element_count}\n\n{}",
                    w, h, r.tree_markdown
                )
            } else {
                format!("window_id={window_id} pid={pid} size={}x{}", w, h)
            };
            content.push(Content::text(summary));
        } else if let Some(ref r) = tree_result {
            let element_count = self.state.element_cache.element_count(pid, window_id);
            content.push(Content::text(format!(
                "window_id={window_id} pid={pid} elements={element_count}\n\n{}",
                r.tree_markdown
            )));
        }

        if content.is_empty() {
            return ToolResult::error("No content produced (neither AX tree nor screenshot succeeded)");
        }

        let element_count = self.state.element_cache.element_count(pid, window_id);
        let tree_md = tree_result.as_ref().map(|r| r.tree_markdown.clone()).unwrap_or_default();
        let mut structured = serde_json::json!({
            "window_id": window_id,
            "pid": pid,
            "element_count": element_count,
            "tree_markdown": tree_md
        });
        if let Some((sw, sh)) = screenshot_dims {
            structured["screenshot_width"] = serde_json::json!(sw);
            structured["screenshot_height"] = serde_json::json!(sh);
        }
        if let Some(ref fp) = screenshot_file_path {
            structured["screenshot_file_path"] = serde_json::json!(fp);
        }
        ToolResult { content, is_error: None, structured_content: Some(structured) }
    }
}
