use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
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
            (x/y/width/height, top-left origin), z_index (integer or null; higher values are \
            closer to the front; null means stacking order is unavailable and callers must not \
            infer one), is_on_screen, on_current_space. To select a frontmost candidate, take the \
            maximum integer z_index; if every value is null, use an explicit fallback instead of \
            relying on array order.".into(),
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
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid_filter: Option<i32> = args.opt_i64("pid").map(|v| v as i32);
        let on_screen_only = args.bool_or("on_screen_only", false);

        let mut windows = if on_screen_only {
            crate::windows::visible_windows()
        } else {
            crate::windows::all_windows()
        };

        if let Some(pid) = pid_filter {
            windows.retain(|w| w.pid == pid);
        }

        let windows_json: Vec<Value> = windows.iter().map(window_record_json).collect();

        ToolResult::text(format!("Found {} window(s).", windows_json.len())).with_structured(
            serde_json::json!({
                "windows": windows_json,
                "current_space_id": null
            }),
        )
    }
}

pub(super) fn window_record_json(w: &crate::windows::WindowInfo) -> Value {
    serde_json::json!({
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_record_includes_observed_z_index() {
        let window = crate::windows::WindowInfo {
            window_id: 42,
            pid: 123,
            app_name: "Example".into(),
            title: "Document".into(),
            bounds: crate::windows::WindowBounds {
                x: 1.0,
                y: 2.0,
                width: 300.0,
                height: 200.0,
            },
            layer: 0,
            z_index: 7,
            is_on_screen: true,
            on_current_space: Some(true),
            space_ids: Some(vec![1]),
        };

        assert_eq!(window_record_json(&window)["z_index"], serde_json::json!(7));
    }
}
