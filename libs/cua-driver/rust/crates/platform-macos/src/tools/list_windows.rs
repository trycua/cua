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
        description: "List layer-0 top-level windows known to WindowServer, including off-screen ones (minimized, other Space, hidden). Use it to find a `window_id` for get_window_state. AppKit-internal helper windows are omitted.\n\
            \n\
            Per record: window_id, pid, app_name, title, bounds, z_index, is_on_screen, space_ids, current_space_id, on_current_space. For the frontmost window take the maximum integer z_index; null means stacking is unavailable, so do not infer it from array order.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "Only this pid's windows."
                },
                "on_screen_only": {
                    "type": "boolean",
                    "description": "Drop windows not on the current Space."
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

        let enumeration = if on_screen_only {
            crate::windows::visible_windows_with_space_snapshot()
        } else {
            crate::windows::all_windows_with_space_snapshot()
        };
        let current_space_id = enumeration.current_space_id;
        let mut windows = enumeration.windows;

        if let Some(pid) = pid_filter {
            windows.retain(|w| w.pid == pid);
        }
        crate::windows::retain_ax_reachable(&mut windows, current_space_id);

        let windows_json: Vec<Value> = windows.iter().map(window_record_json).collect();

        ToolResult::text(format_window_list(&windows)).with_structured(
            serde_json::json!({
                "windows": windows_json,
                "current_space_id": current_space_id
            }),
        )
    }
}

/// One compact line per window after the count header, so text-only MCP
/// clients can discover `window_id`s without reading `structuredContent`.
/// Same row shape as the Windows tool (`- app (pid) "title" [window_id]`,
/// `(no title)` fallback, ` [off-screen]` tag); the header line is unchanged.
pub(super) fn format_window_list(windows: &[crate::windows::WindowInfo]) -> String {
    let mut lines = vec![format!("Found {} window(s).", windows.len())];
    for w in windows {
        let title = if w.title.is_empty() {
            "(no title)".to_owned()
        } else {
            format!("\"{}\"", w.title)
        };
        let tag = if w.is_on_screen { "" } else { " [off-screen]" };
        lines.push(format!(
            "- {} (pid {}) {} [window_id: {}]{tag}",
            w.app_name, w.pid, title, w.window_id
        ));
    }
    lines.join("\n")
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
        "current_space_id": w.current_space_id,
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
            current_space_id: Some(1),
            on_current_space: Some(true),
            space_ids: Some(vec![1]),
        };

        assert_eq!(window_record_json(&window)["z_index"], serde_json::json!(7));
        assert_eq!(
            window_record_json(&window)["current_space_id"],
            serde_json::json!(1)
        );
        assert_eq!(
            window_record_json(&window)["on_current_space"],
            serde_json::json!(true)
        );
    }

    fn window(
        window_id: u32,
        pid: i32,
        app: &str,
        title: &str,
        on_screen: bool,
    ) -> crate::windows::WindowInfo {
        crate::windows::WindowInfo {
            window_id,
            pid,
            app_name: app.into(),
            title: title.into(),
            bounds: crate::windows::WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            layer: 0,
            z_index: 1,
            is_on_screen: on_screen,
            current_space_id: Some(1),
            on_current_space: Some(true),
            space_ids: Some(vec![1]),
        }
    }

    #[test]
    fn empty_list_keeps_the_count_only_header() {
        assert_eq!(format_window_list(&[]), "Found 0 window(s).");
    }

    #[test]
    fn text_lists_one_row_per_window_with_ids_and_screen_state() {
        let text = format_window_list(&[
            window(10700, 65305, "Calculator", "Calculator", true),
            window(10701, 65305, "Calculator", "", false),
        ]);
        assert_eq!(
            text,
            "Found 2 window(s).\n\
             - Calculator (pid 65305) \"Calculator\" [window_id: 10700]\n\
             - Calculator (pid 65305) (no title) [window_id: 10701] [off-screen]"
        );
    }
}
