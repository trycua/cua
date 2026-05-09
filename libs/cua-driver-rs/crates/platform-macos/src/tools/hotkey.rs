use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;
use libc;

use super::ToolState;

pub struct HotkeyTool {
    state: Arc<ToolState>,
}

impl HotkeyTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "hotkey".into(),
        description:
            "Press a combination of keys simultaneously — e.g. `[\"cmd\", \"c\"]` for Copy, \
             `[\"cmd\", \"shift\", \"4\"]` for screenshot selection. The combo is posted directly \
             to the target pid's event queue; the target does NOT need to be frontmost.\n\n\
             Two delivery paths:\n\
             • Default (no window_id): auth-message envelope — Chromium/Electron apps accept \
               the keystrokes as trusted live input on macOS 14+.\n\
             • With window_id: NSMenu path — briefly activates the target WindowServer-frontmost \
               via SLPSSetFrontProcessWithOptions (kCPSNoWindows, < 1 ms), posts WITHOUT the auth \
               envelope so IOHIDPostEvent fires and NSApplication.sendEvent: dispatches NSMenu key \
               equivalents (e.g. Cmd+Z undo, Cmd+W close). Restores prior frontmost immediately. \
               Use this path when you need native menu-bar actions on non-Chromium apps.\n\n\
             Recognized modifiers: cmd/command, shift, option/alt, ctrl/control, fn. \
             Non-modifier keys use the same vocabulary as `press_key` (return, tab, escape, \
             up/down/left/right, space, delete, home, end, pageup, pagedown, f1-f12, letters, \
             digits). Order: modifiers first, one non-modifier last."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "keys"],
            "properties": {
                "pid": { "type": "integer", "description": "Target process ID." },
                "keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 2,
                    "description": "Modifier(s) and one non-modifier key, e.g. [\"cmd\", \"c\"]."
                },
                "window_id": {
                    "type": "integer",
                    "description": "When set, uses NSMenu path: briefly activates the window for menu key dispatch, then restores prior frontmost."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true,
    })
}

/// Modifier key names — split the keys array into modifiers + base key.
fn is_modifier(k: &str) -> bool {
    matches!(
        k.to_lowercase().as_str(),
        "cmd" | "command" | "shift" | "option" | "alt" | "ctrl" | "control" | "fn"
    )
}

#[async_trait]
impl Tool for HotkeyTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let _ = &self.state;

        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };

        let raw_keys = match args.get("keys").and_then(|v| v.as_array()) {
            Some(arr) => arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>(),
            None => return ToolResult::error("Missing required parameter: keys"),
        };

        if raw_keys.is_empty() {
            return ToolResult::error("keys must be a non-empty array of strings.");
        }

        // Split: modifiers are all entries that are modifier names; base key is everything else.
        // Typically: last non-modifier is the key; all others are modifiers.
        let modifiers: Vec<String> = raw_keys.iter()
            .filter(|k| is_modifier(k))
            .cloned()
            .collect();
        let non_modifiers: Vec<String> = raw_keys.iter()
            .filter(|k| !is_modifier(k))
            .cloned()
            .collect();

        if non_modifiers.is_empty() {
            return ToolResult::error(
                "keys must include at least one non-modifier key (e.g. \"c\" in [\"cmd\", \"c\"])."
            );
        }

        // Use the last non-modifier key; if there are multiple, treat earlier ones as extra keys.
        let key = non_modifiers.last().unwrap().clone();
        let key_display = raw_keys.join("+");
        let window_id = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);

        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            if let Some(wid) = window_id {
                crate::input::skylight::with_menu_shortcut_activation(pid as libc::pid_t, wid, || {
                    crate::input::keyboard::hotkey_no_auth(pid, &key, &m)
                })?;
                Ok(())
            } else {
                crate::input::keyboard::hotkey(pid, &key, &m)
            }
        }).await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!("Pressed {key_display} on pid {pid}.")),
            Ok(Err(e)) => ToolResult::error(format!("hotkey failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
