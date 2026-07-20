use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct TypeTextCharsTool {
    state: Arc<ToolState>,
}

impl TypeTextCharsTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
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
                "window_id":     { "type": "integer", "description": "Window ID for element focus. Optional when element_token is supplied." },
                "element_index": { "type": "integer", "description": "Element to focus before typing." },
                "element_token": { "type": "string",  "description": "Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded." },
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
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Same trailing-protocol-tag scrub as TypeTextTool — see
        // cua_driver_core::text_sanitize for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        let delay_ms = args.u64_or("delay_ms", 30);
        // Surface 6: element_token / element_index precedence resolution.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match super::resolve_element_target(
            &self.state,
            pid,
            window_id_arg,
            element_index_arg,
            element_token_arg.as_deref(),
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let token_targeted = resolved.via_token;
        let element_index = resolved.element_index;
        let window_id = resolved.window_id;
        let token_guard = resolved.retained;
        let type_chars_only = args.bool_or("type_chars_only", false);
        if token_targeted && type_chars_only {
            return ToolResult::error(
                "type_chars_only cannot be used with element_token because it would discard \
                 the exact-node focus binding.",
            );
        }

        // Pre-focus element if requested.
        if !type_chars_only {
            if let (Some(idx), Some(wid)) = (element_index, window_id) {
                // Retain so a concurrent get_window_state can't free the element
                // during the focus call (use-after-free → daemon crash). The
                // guard outlives the awaited spawn_blocking below.
                let element_guard = match token_guard
                    .or_else(|| self.state.element_cache.get_element_retained(pid, wid, idx))
                {
                    Some(element) => element,
                    None => {
                        return ToolResult::error(format!(
                            "Element index {idx} not found. Call get_window_state first."
                        ))
                    }
                };
                let element_ptr = element_guard.as_ptr();
                let focus = tokio::task::spawn_blocking(move || {
                    if token_targeted {
                        crate::input::ax_actions::focus_element_strict(element_ptr)
                    } else {
                        crate::input::ax_actions::focus_element(element_ptr)
                    }
                })
                .await;
                match focus {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        return ToolResult::error(format!(
                            "type_text_chars exact-node focus failed: {error}"
                        ))
                    }
                    Err(error) => return ToolResult::error(format!("Focus task failed: {error}")),
                }
                drop(element_guard);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }

        let text_len = text.chars().count();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::keyboard::type_text_with_delay(pid, &text, delay_ms)
        })
        .await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Typed {text_len} character(s) with {delay_ms}ms delay."
            )),
            Ok(Err(e)) => ToolResult::error(format!("Type text chars failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
