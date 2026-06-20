use async_trait::async_trait;
use cua_driver_core::{
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;
use crate::ax::AXNode;
use crate::windows::WindowBounds;

pub struct GetWindowStateTool {
    state: Arc<ToolState>,
}

impl GetWindowStateTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
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
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
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

fn build_structured_elements(
    nodes: &[AXNode],
    bounds: &WindowBounds,
    screenshot_width: u32,
    screenshot_height: u32,
) -> Vec<Value> {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    let scale_x = screenshot_width as f64 / bounds.width;
    let scale_y = screenshot_height as f64 / bounds.height;

    nodes
        .iter()
        .filter_map(|node| {
            let idx = node.element_index?;
            let [x, y, width, height] =
                intersect_screen_rect_with_bounds(node.screen_rect?, bounds)?;
            Some(serde_json::json!({
                "element_index": idx,
                "x": ((x - bounds.x) * scale_x).round() as i64,
                "y": ((y - bounds.y) * scale_y).round() as i64,
                "width": (width * scale_x).round() as i64,
                "height": (height * scale_y).round() as i64,
            }))
        })
        .collect()
}

fn intersect_screen_rect_with_bounds(
    [x, y, width, height]: [f64; 4],
    bounds: &WindowBounds,
) -> Option<[f64; 4]> {
    let left = x.max(bounds.x);
    let top = y.max(bounds.y);
    let right = (x + width).min(bounds.x + bounds.width);
    let bottom = (y + height).min(bounds.y + bounds.height);
    let clipped_width = right - left;
    let clipped_height = bottom - top;

    if clipped_width <= 0.0 || clipped_height <= 0.0 {
        None
    } else {
        Some([left, top, clipped_width, clipped_height])
    }
}

#[async_trait]
impl Tool for GetWindowStateTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let window_id = match args.require_u32("window_id") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let query = args.opt_str("query");
        let screenshot_out_file = args.opt_str("screenshot_out_file").map(|s| {
            // Expand ~ prefix.
            if s.starts_with("~/") {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/{}", &s[2..])
            } else {
                s
            }
        });
        // Effective config resolves call-arg > session-override > global. The
        // daemon injects `_session_id` for named MCP sessions; absent => global.
        let session_id = args.opt_str("_session_id");
        let (default_mode, effective_max_dim) = {
            let cfg = self.state.config.read().unwrap();
            self.state
                .session_config
                .effective(session_id.as_deref(), &cfg)
        };
        let capture_mode = args.opt_str("capture_mode").unwrap_or(default_mode);

        // Walk AX tree (unless vision-only mode). Accept "tree" as deprecated alias for "ax".
        let capture_mode = if capture_mode == "tree" {
            "ax".to_owned()
        } else {
            capture_mode
        };
        let tree_result = if capture_mode != "vision" {
            let q = query.clone();
            // Wrap the blocking AX walk in a 30-second timeout. Heavy webview apps
            // (Arc, Safari with many tabs, Electron) can block
            // AXUIElementCopyAttributeValue indefinitely via XPC — without a
            // deadline the MCP server hangs forever (issue #1537).
            let walk_future = tokio::task::spawn_blocking(move || {
                crate::ax::tree::walk_tree(pid, Some(window_id), q.as_deref())
            });
            match tokio::time::timeout(std::time::Duration::from_secs(30), walk_future).await {
                Ok(Ok(r)) => Some(r),
                Ok(Err(e)) => return ToolResult::error(format!("AX tree walk failed: {e}")),
                Err(_elapsed) => {
                    return ToolResult::error(format!(
                        "AX tree walk for pid={pid} timed out after 30 s. \
                         The app (likely Arc, Electron, or Safari with many tabs) has a \
                         pathologically large accessibility tree. \
                         Workarounds: switch to capture_mode=vision for pixel-click \
                         workflows, or use capture_mode=ax with a depth-limited scan."
                    ));
                }
            }
        } else {
            None
        };

        // Update element cache.
        if let Some(ref r) = tree_result {
            self.state.element_cache.update(pid, window_id, &r.nodes);
        }

        // Screenshot (unless ax-only mode). Uses the session-effective max
        // dimension resolved above.
        let max_dim = effective_max_dim;
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
                            self.state
                                .resize_registry
                                .set_ratio(pid, ow as f64 / w as f64);
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
            return ToolResult::error(
                "No content produced (neither AX tree nor screenshot succeeded)",
            );
        }

        let element_count = self.state.element_cache.element_count(pid, window_id);
        let tree_md = tree_result
            .as_ref()
            .map(|r| r.tree_markdown.clone())
            .unwrap_or_default();
        let structured_elements = match (tree_result.as_ref(), screenshot_dims) {
            (Some(result), Some((sw, sh))) => crate::windows::window_bounds_by_id(window_id)
                .map(|bounds| build_structured_elements(&result.nodes, &bounds, sw, sh)),
            _ => None,
        };
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
        if let Some(elements) = structured_elements {
            structured["elements"] = serde_json::json!(elements);
        }
        if let Some(ref fp) = screenshot_file_path {
            structured["screenshot_file_path"] = serde_json::json!(fp);
        }
        ToolResult {
            content,
            is_error: None,
            structured_content: Some(structured),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(element_index: Option<usize>, screen_rect: Option<[f64; 4]>) -> AXNode {
        AXNode {
            element_index,
            role: "AXButton".to_owned(),
            title: None,
            value: None,
            description: None,
            identifier: None,
            help: None,
            actions: Vec::new(),
            screen_rect,
            element_ptr: 0,
        }
    }

    #[test]
    fn build_structured_elements_converts_to_window_local_screenshot_pixels() {
        let bounds = WindowBounds {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        let elements = build_structured_elements(
            &[node(Some(7), Some([20.0, 24.0, 30.0, 10.0]))],
            &bounds,
            150,
            100,
        );

        assert_eq!(
            elements,
            vec![serde_json::json!({
                "element_index": 7,
                "x": 15,
                "y": 8,
                "width": 45,
                "height": 20,
            })]
        );
    }

    #[test]
    fn build_structured_elements_clips_to_window_bounds() {
        let bounds = WindowBounds {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        let elements = build_structured_elements(
            &[
                node(Some(1), Some([0.0, 10.0, 30.0, 20.0])),
                node(Some(2), Some([200.0, 20.0, 10.0, 10.0])),
            ],
            &bounds,
            150,
            100,
        );

        assert_eq!(
            elements,
            vec![serde_json::json!({
                "element_index": 1,
                "x": 0,
                "y": 0,
                "width": 30,
                "height": 20,
            })]
        );
    }

    #[test]
    fn build_structured_elements_skips_non_actionable_or_missing_geometry_nodes() {
        let bounds = WindowBounds {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let elements = build_structured_elements(
            &[
                node(Some(0), None),
                node(None, Some([10.0, 10.0, 5.0, 5.0])),
                node(Some(1), Some([10.0, 10.0, 5.0, 5.0])),
            ],
            &bounds,
            100,
            100,
        );

        assert_eq!(
            elements,
            vec![serde_json::json!({
                "element_index": 1,
                "x": 10,
                "y": 10,
                "width": 5,
                "height": 5,
            })]
        );
    }

    #[test]
    fn build_structured_elements_returns_empty_for_non_positive_window_bounds() {
        let bounds = WindowBounds {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 120.0,
        };
        let elements = build_structured_elements(
            &[node(Some(7), Some([10.0, 10.0, 5.0, 5.0]))],
            &bounds,
            100,
            100,
        );

        assert!(elements.is_empty());
    }
}
