use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::ToolState;

// ── type_text_chars ───────────────────────────────────────────────────────────

pub struct TypeTextCharsTool {
    pub(crate) _state: Arc<ToolState>,
}
static TYPE_CHARS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for TypeTextCharsTool {
    fn def(&self) -> &ToolDef {
        TYPE_CHARS_DEF.get_or_init(|| ToolDef {
            name: "type_text_chars".into(),
            description: "Type text character-by-character with a configurable inter-character \
                delay (default 30 ms). Useful for apps that miss keystrokes. \
                Otherwise identical to type_text (WM_CHAR, no focus steal).".into(),
            input_schema: json!({
                "type":"object","required":["pid","text"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID."},
                    "window_id":{"type":"integer","description":"HWND of the target window. Required with element_index."},
                    "text":{"type":"string","description":"Text to type, one character at a time."},
                    "delay_ms":{"type":"integer","description":"Milliseconds between chars (default 30)."},
                    "type_chars_only":{"type":"boolean","description":"Skip element focus, type directly. Default false."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Same trailing-protocol-tag scrub as the main TypeTextTool — see
        // cua_driver_core::text_sanitize for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        let delay_ms = args.u64_or("delay_ms", 30);
        let hwnd_opt = args.opt_u64("window_id");
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };
        let text_len = text.chars().count();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::post_type_text_with_delay(hwnd, &text, delay_ms)
        })
        .await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Typed {text_len} character(s) with {delay_ms}ms delay."
            )),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
