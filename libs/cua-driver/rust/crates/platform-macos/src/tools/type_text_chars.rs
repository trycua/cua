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
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
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
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg,
            "type_text_chars",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id) = match resolved {
            cua_driver_core::element_token::ResolvedElement::None => (None, window_id_arg),
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: wid,
                element_index: idx,
                via_token: _,
            } => (Some(idx), wid),
        };
        let type_chars_only = args.bool_or("type_chars_only", false);

        // Retain the addressed element (if any) so a concurrent
        // get_window_state can't free it during the gate/focus below
        // (use-after-free → daemon crash). Guard lives past the focus call.
        let element_guard = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            match self.state.element_cache.get_element_retained(pid, wid, idx) {
                Some(guard) => Some(guard),
                None => {
                    return ToolResult::error(format!(
                        "Element index {idx} not found. Call get_window_state first."
                    ));
                }
            }
        } else {
            None
        };
        let element_ptr: Option<usize> = element_guard.as_ref().map(|g| g.as_ptr());

        // ── Exact-target background gate (macOS background input v1) ──
        // This tool is always background CGEvent keystrokes — process-scoped
        // transport with no semantic AX rung. A window-addressed request must
        // prove exact delivery before any event is posted; there is no
        // semantic fallback here, so a refusal is final.
        let _mutation_lease = if let Some(wid) = window_id {
            match super::gate_background_window_action(
                pid,
                wid,
                element_ptr,
                cua_driver_core::background_input::BackgroundAction::InsertText,
            )
            .await
            {
                Ok(lease) => Some(lease),
                Err(refusal_result) => return refusal_result,
            }
        } else {
            None
        };

        // Pre-focus element if requested.
        if !type_chars_only {
            if let Some(element_ptr) = element_ptr {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::input::ax_actions::focus_element(element_ptr)
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
        drop(element_guard);

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
