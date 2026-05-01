use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct TypeTextCharsTool {
    state: Arc<ToolState>,
}

impl TypeTextCharsTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "type_text_chars".into(),
        description: "Type text character-by-character with a configurable inter-character delay. \
            Useful for apps that miss keystrokes when characters arrive too quickly. \
            Otherwise identical to type_text (CGEvent, no focus steal).".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "text"],
            "properties": {
                "pid":           { "type": "integer", "description": "Target process ID." },
                "text":          { "type": "string",  "description": "Text to type." },
                "delay_ms":      { "type": "integer", "description": "Milliseconds between characters (default 30)." },
                "window_id":     { "type": "integer", "description": "Window ID for element focus." },
                "element_index": { "type": "integer", "description": "Element to focus before typing." },
                "type_chars_only": { "type": "boolean", "description": "Skip AX focus, type directly. Default false." }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true,
    })
}

#[async_trait]
impl Tool for TypeTextCharsTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None => return ToolResult::error("Missing required parameter: text"),
        };
        let delay_ms = args.get("delay_ms").and_then(|v| v.as_u64()).unwrap_or(30);
        let element_index = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let window_id = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);
        let type_chars_only = args.get("type_chars_only").and_then(|v| v.as_bool()).unwrap_or(false);

        // Pre-focus element if requested.
        if !type_chars_only {
            if let (Some(idx), Some(wid)) = (element_index, window_id) {
                if let Some(element_ptr) = self.state.element_cache.get_element_ptr(pid, wid, idx) {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::input::ax_actions::focus_element(element_ptr)
                    }).await;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }

        let text_len = text.chars().count();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::keyboard::type_text_with_delay(pid, &text, delay_ms)
        }).await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!("Typed {text_len} character(s) with {delay_ms}ms delay.")),
            Ok(Err(e)) => ToolResult::error(format!("Type text chars failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
