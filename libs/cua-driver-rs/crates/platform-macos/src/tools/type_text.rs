//! type_text tool — matches the Swift reference TypeTextTool.swift.
//!
//! Inserts text via `AXSelectedText` attribute write — an atomic single-call
//! insertion at the current cursor position. This is the preferred path for
//! all standard Cocoa text views (NSTextField, NSTextView, WKWebView text
//! inputs in Safari, etc.) and is significantly faster than per-keystroke
//! CGEvent synthesis.
//!
//! For Chromium / Electron inputs that don't implement `kAXSelectedText`,
//! the tool falls back to character-by-character CGEvent keystrokes so the
//! caller doesn't need to detect the app type themselves.
//!
//! Use `type_text_chars` when you explicitly need per-character pacing
//! (e.g., to trigger live-search debounce handlers).

use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{
    copy_string_attr, focused_element_of_pid, kAXErrorSuccess, set_string_attr,
    AXUIElementRef,
};
use core_foundation::base::CFRelease;

use super::ToolState;

pub struct TypeTextTool {
    pub state: Arc<ToolState>,
}

impl TypeTextTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "type_text".into(),
        description:
            "Insert text into the target pid via `AXSetAttribute(kAXSelectedText)`. \
             Works for standard Cocoa text fields and text views. No keystrokes are \
             synthesized — special keys (Return / Escape / arrows) go through \
             `press_key` / `hotkey`. For Chromium / Electron inputs that don't \
             implement `kAXSelectedText`, the tool falls back to CGEvent \
             character synthesis automatically.\n\n\
             Optional `element_index` + `window_id` (from the last \
             `get_window_state` snapshot) directs the write to a specific field. \
             Without `element_index`, the write goes to the pid's currently \
             focused element."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "text"],
            "properties": {
                "pid":  { "type": "integer", "description": "Target process ID." },
                "text": { "type": "string",  "description": "Text to insert at the target's cursor." },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID. Required when element_index is used."
                },
                "element_index": {
                    "type": "integer",
                    "description": "Element index from last get_window_state. Directs the write to a specific field. Requires window_id."
                },
                "delay_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 200,
                    "description": "Milliseconds between characters in the CGEvent fallback path. Default 30. Ignored when the AX path succeeds."
                }
            },
            "additionalProperties": false
        }),
        read_only:   false,
        destructive: true,
        idempotent:  false,
        open_world:  true,
    })
}

#[async_trait]
impl Tool for TypeTextTool {
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
        let element_index = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let window_id     = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);
        let delay_ms      = args.get("delay_ms").and_then(|v| v.as_u64()).unwrap_or(30);

        // Validate element_index requires window_id.
        if element_index.is_some() && window_id.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used."
            );
        }

        // Resolve the element pointer (if element_index given).
        let element_ptr = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            match self.state.element_cache.get_element_ptr(pid, wid, idx) {
                Some(p) => Some((p, Some(idx))),
                None => return ToolResult::error(format!(
                    "Element index {idx} not found. Call get_window_state first."
                )),
            }
        } else {
            None
        };

        let text_clone  = text.clone();
        let char_count  = text.chars().count();

        let result = tokio::task::spawn_blocking(move || {
            type_text_blocking(pid, &text_clone, element_ptr, delay_ms)
        }).await;

        match result {
            Ok(Ok(detail)) => ToolResult::text(format!(
                "✅ Inserted {char_count} char(s){detail}."
            )),
            Ok(Err(e)) => ToolResult::error(format!("type_text failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking implementation ───────────────────────────────────────────────────

/// `element_ptr_and_idx` — `Some((ptr, idx))` if element_index was provided.
///
/// Returns a short detail string appended to the success message.
fn type_text_blocking(
    pid: i32,
    text: &str,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    delay_ms: u64,
) -> anyhow::Result<String> {
    // --- Path 1: caller provided an explicit element index ---
    if let Some((ptr, idx_opt)) = element_ptr_and_idx {
        let element = ptr as AXUIElementRef;
        let role  = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
        let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

        let err = unsafe { set_string_attr(element, "AXSelectedText", text) };
        if err == kAXErrorSuccess {
            // Verify the write was actually applied — Chromium web inputs silently
            // accept the call but never update their DOM value. Read AXValue back;
            // if it's still empty when we just wrote non-empty text the write was a
            // no-op and we must use CGEvent instead.
            let silent_accept = !text.is_empty() && unsafe {
                copy_string_attr(element, "AXValue")
                    .map(|v| v.is_empty())
                    .unwrap_or(false)
            };
            if !silent_accept {
                let idx_str = idx_opt.map(|i| format!(" [{i}]")).unwrap_or_default();
                return Ok(format!(" into{idx_str} {role} \"{title}\""));
            }
            tracing::debug!(
                "AXSelectedText silent-accept detected for {role} \"{title}\" \
                 (AXValue still empty), falling back to CGEvent keystrokes"
            );
        } else {
            // AXSelectedText not supported (Chromium/Electron) — fall through to CGEvent.
            tracing::debug!(
                "AXSelectedText write failed ({err}) for {role} \"{title}\", \
                 falling back to CGEvent keystrokes"
            );
        }
        return crate::input::keyboard::type_text_with_delay(pid, text, delay_ms)
            .map(|_| format!(" via CGEvent (AXSelectedText unsupported/silent-accept, {delay_ms}ms delay)"));
    }

    // --- Path 2: no element_index — target the pid's focused element ---
    let focused = unsafe { focused_element_of_pid(pid) };
    if let Some(element) = focused {
        let role  = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
        let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

        let err = unsafe { set_string_attr(element, "AXSelectedText", text) };

        let did_succeed = if err == kAXErrorSuccess {
            // Same silent-accept check: Chromium web inputs return success but
            // leave AXValue empty, meaning the write never reached the DOM.
            let silent_accept = !text.is_empty() && unsafe {
                copy_string_attr(element, "AXValue")
                    .map(|v| v.is_empty())
                    .unwrap_or(false)
            };
            if silent_accept {
                tracing::debug!(
                    "AXSelectedText silent-accept detected for focused {role} \"{title}\" \
                     (AXValue still empty), falling back to CGEvent keystrokes"
                );
            }
            !silent_accept
        } else {
            false
        };

        unsafe { CFRelease(element as _); }

        if did_succeed {
            return Ok(format!(" into focused {role} \"{title}\""));
        }
        if err != kAXErrorSuccess {
            // Fall through to CGEvent.
            tracing::debug!(
                "AXSelectedText write failed ({err}) for focused {role}, \
                 falling back to CGEvent keystrokes"
            );
        }
    } else {
        tracing::debug!(
            "No focused element found for pid {pid}, falling back to CGEvent keystrokes"
        );
    }

    crate::input::keyboard::type_text_with_delay(pid, text, delay_ms)
        .map(|_| format!(" via CGEvent (no focused element / AXSelectedText unsupported, {delay_ms}ms delay)"))
}
