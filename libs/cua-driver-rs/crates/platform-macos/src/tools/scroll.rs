use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct ScrollTool {
    state: Arc<ToolState>,
}

impl ScrollTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "scroll".into(),
        description: "Scroll the target pid's focused region by synthesized keystrokes.\n\n\
            Mapping: by='page' → PageDown/PageUp × amount; by='line' → DownArrow/UpArrow × amount. \
            Horizontal variants use Left/Right arrow keys.\n\n\
            Optional element_index + window_id pre-focuses the element before scrolling.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "direction"],
            "properties": {
                "pid": { "type": "integer" },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction."
                },
                "by": {
                    "type": "string",
                    "enum": ["line", "page"],
                    "description": "Scroll granularity. Default: line."
                },
                "amount": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "Number of keystroke repetitions. Default: 3."
                },
                "window_id": { "type": "integer" },
                "element_index": { "type": "integer" }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    })
}

#[async_trait]
impl Tool for ScrollTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let direction = match args.get("direction").and_then(|v| v.as_str()) {
            Some(d) => d.to_owned(),
            None => return ToolResult::error("Missing required parameter: direction"),
        };
        let by = args.get("by").and_then(|v| v.as_str()).unwrap_or("line").to_owned();
        let amount = args.get("amount").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
        let element_index = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let window_id = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);

        // Pre-focus element if requested.
        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            if let Some(element_ptr) = self.state.element_cache.get_element_ptr(pid, wid, idx) {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::input::ax_actions::focus_element(element_ptr)
                }).await;
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
        }

        let key = match (by.as_str(), direction.as_str()) {
            ("page", "down")  | (_, "down") if by == "page"  => "pagedown",
            ("page", "up")    | (_, "up")   if by == "page"  => "pageup",
            ("line", "down")  | (_, "down")                  => "down",
            ("line", "up")    | (_, "up")                    => "up",
            (_, "left")                                       => "left",
            (_, "right")                                      => "right",
            _                                                 => "down",
        };
        let key = key.to_owned();

        let result = tokio::task::spawn_blocking(move || {
            for _ in 0..amount {
                if let Err(e) = crate::input::keyboard::press_key(pid, &key, &[]) {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(())
        }).await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!("Scrolled {direction} by {by} × {amount}.")),
            Ok(Err(e)) => ToolResult::error(format!("Scroll failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
