use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;
use libc;

use super::ToolState;

pub struct PressKeyTool {
    state: Arc<ToolState>,
}

impl PressKeyTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "press_key".into(),
        description: "Press and release a single key, delivered to the target pid via \
            CGEventPostToPid. No focus steal.\n\n\
            Two delivery paths:\n\
            • window_id + element_index: focuses the AX element first, then posts via the \
              auth-message path (Chromium-safe).\n\
            • window_id only (no element_index): NSMenu path — briefly activates the window \
              WindowServer-frontmost via SLPSSetFrontProcessWithOptions (kCPSNoWindows, < 1 ms), \
              posts WITHOUT the auth envelope so IOHIDPostEvent fires and NSApplication.sendEvent: \
              dispatches NSMenu key equivalents. Restores prior frontmost immediately.\n\
            • No window_id: standard auth-message path.\n\n\
            Key names: return, tab, escape, up/down/left/right, space, delete, home, end, \
            pageup, pagedown, f1-f12, plus any letter or digit.\n\
            Modifiers array: cmd, shift, option/alt, ctrl, fn.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "key"],
            "properties": {
                "pid": { "type": "integer" },
                "key": { "type": "string", "description": "Key name: return, tab, escape, up, down, etc." },
                "modifiers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Modifier keys: cmd, shift, option/alt, ctrl, fn."
                },
                "window_id": { "type": "integer" },
                "element_index": { "type": "integer" }
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
impl Tool for PressKeyTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let key_raw = match args.get("key").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None => return ToolResult::error("Missing required parameter: key"),
        };
        let mut modifiers: Vec<String> = args.get("modifiers")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let element_index = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let window_id = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);

        // Remap "+" / "plus" → "=" + Shift (same physical key on US layout).
        let key = if key_raw == "+" || key_raw == "plus" {
            if !modifiers.iter().any(|m| m.eq_ignore_ascii_case("shift")) {
                modifiers.push("shift".to_string());
            }
            "=".to_string()
        } else {
            key_raw.clone()
        };
        let display_key = key_raw.clone();

        // Pre-focus element if requested (element_index + window_id path).
        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            if let Some(element_ptr) = self.state.element_cache.get_element_ptr(pid, wid, idx) {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::input::ax_actions::focus_element(element_ptr)
                }).await;
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
        }

        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            if let Some(wid) = window_id {
                if element_index.is_none() {
                    // NSMenu path: window_id set but no element_index.
                    crate::input::skylight::with_menu_shortcut_activation(pid as libc::pid_t, wid, || {
                        crate::input::keyboard::press_key_no_auth(pid, &key, &m)
                    })?;
                    return Ok(());
                }
            }
            crate::input::keyboard::press_key(pid, &key, &m)
        }).await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!("✅ Pressed {display_key} on pid {pid}.")),
            Ok(Err(e)) => ToolResult::error(format!("press_key failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
