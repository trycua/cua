//! get_accessibility_tree — lightweight desktop snapshot: running apps + visible windows.
//!
//! This is the fast, no-TCC-grant discovery tool. For the full per-window AX subtree
//! with interactive element indices, use `get_window_state` (or
//! `get_window_state` with `capture_mode="ax"` to skip the screenshot).

use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct GetAccessibilityTreeTool {
    state: Arc<ToolState>,
}

impl GetAccessibilityTreeTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_accessibility_tree".into(),
        description:
            "Return a lightweight snapshot of the desktop: running regular apps and \
             on-screen visible windows with their bounds, z-order, and owner pid.\n\n\
             For the full AX subtree of a single window (with interactive element indices \
             you can click by), use `get_window_state` instead — that's the heavy per-window \
             tool. This one is a fast discovery read that needs no TCC grants."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        read_only:   true,
        destructive: false,
        idempotent:  true,
        open_world:  false,
    })
}

#[async_trait]
impl Tool for GetAccessibilityTreeTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let _ = &self.state; // state not needed for this tool

        let apps    = crate::apps::list_running_apps();
        let windows = crate::windows::visible_windows();

        let mut lines = vec![format!(
            "{} running app(s), {} visible window(s)",
            apps.len(), windows.len()
        )];

        for app in &apps {
            let bid = app.bundle_id.as_deref()
                .map(|b| format!(" [{b}]"))
                .unwrap_or_default();
            lines.push(format!("- {} (pid {}){}", app.name, app.pid, bid));
        }

        if !windows.is_empty() {
            lines.push(String::new());
            lines.push("Windows:".to_owned());
            for w in &windows {
                let title = if w.title.is_empty() {
                    "(no title)".to_owned()
                } else {
                    format!("\"{}\"", w.title)
                };
                lines.push(format!(
                    "- {} (pid {}) {} [window_id: {}]",
                    w.app_name, w.pid, title, w.window_id
                ));
            }
            lines.push(
                "→ Call get_window_state(pid, window_id) to inspect a window's UI.".to_owned()
            );
        }

        let structured = serde_json::json!({
            "apps": apps.iter().map(|a| serde_json::json!({
                "pid": a.pid,
                "name": a.name,
                "bundle_id": a.bundle_id
            })).collect::<Vec<_>>(),
            "windows": windows.iter().map(|w| serde_json::json!({
                "window_id": w.window_id,
                "pid": w.pid,
                "app_name": w.app_name,
                "title": w.title
            })).collect::<Vec<_>>()
        });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}
