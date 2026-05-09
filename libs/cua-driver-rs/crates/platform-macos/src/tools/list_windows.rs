use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct ListWindowsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "list_windows".into(),
        description: "List all layer-0 top-level windows currently known to WindowServer. \
            Includes off-screen windows (minimized, on another Space, hidden-launched). \
            Use this to find a window_id before calling get_window_state.\n\n\
            Per-record fields: window_id, pid, app_name, title, bounds \
            (x/y/width/height, top-left origin), z_index (higher = frontmost), \
            is_on_screen, on_current_space.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "Optional pid filter. When set, only this pid's windows are returned."
                },
                "on_screen_only": {
                    "type": "boolean",
                    "description": "When true, drop windows not on the current Space. Default false."
                }
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for ListWindowsTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid_filter: Option<i32> = args.get("pid").and_then(|v| v.as_i64()).map(|v| v as i32);
        let on_screen_only = args.get("on_screen_only").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut windows = if on_screen_only {
            crate::windows::visible_windows()
        } else {
            crate::windows::all_windows()
        };

        if let Some(pid) = pid_filter {
            windows.retain(|w| w.pid == pid);
        }

        let windows_json: Vec<Value> = windows.iter().map(|w| serde_json::json!({
            "window_id": w.window_id,
            "pid": w.pid,
            "app_name": w.app_name,
            "title": w.title,
            "bounds": {
                "x": w.bounds.x,
                "y": w.bounds.y,
                "width": w.bounds.width,
                "height": w.bounds.height
            },
            "layer": w.layer,
            "z_index": w.z_index,
            "is_on_screen": w.is_on_screen,
            "on_current_space": w.on_current_space,
            "space_ids": w.space_ids,
        })).collect();

        ToolResult::text(format!("Found {} window(s).", windows_json.len()))
            .with_structured(serde_json::json!({
                "windows": windows_json,
                "current_space_id": null
            }))
    }
}
