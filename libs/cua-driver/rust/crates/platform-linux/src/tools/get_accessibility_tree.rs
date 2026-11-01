use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

use super::impl_::{visible_live_desktop_windows, window_record_json};

// ── get_accessibility_tree ────────────────────────────────────────────────────

pub struct GetAccessibilityTreeTool;

static GAX_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetAccessibilityTreeTool {
    fn def(&self) -> &ToolDef {
        GAX_DEF.get_or_init(|| ToolDef {
            name: "get_accessibility_tree".into(),
            description: "Return a lightweight snapshot of the desktop: running processes and \
                on-screen visible windows with their bounds and owner pid.\n\n\
                For the full AT-SPI subtree of a single window (with interactive element indices \
                you can click by), use get_window_state instead — this is a fast discovery read."
                .into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let (procs, windows) = tokio::task::spawn_blocking(|| {
            (
                crate::proc_fs::list_processes(),
                visible_live_desktop_windows(crate::wayland::list_windows_dispatch(None)),
            )
        })
        .await
        .unwrap_or_default();

        if crate::wayland::is_wayland() {
            crate::wayland::remember_observed_window_origins(&windows);
        }

        let mut lines = vec![format!(
            "{} running process(es), {} visible window(s)",
            procs.len(),
            windows.len()
        )];
        for p in &procs {
            let cmd = if p.cmdline.is_empty() {
                p.name.clone()
            } else {
                p.cmdline.clone()
            };
            lines.push(format!("- {} (pid {})", cmd, p.pid));
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
                    "- pid={} window_id={} {} {}x{}+{}+{}",
                    w.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                    w.xid,
                    title,
                    w.width,
                    w.height,
                    w.x,
                    w.y
                ));
            }
            lines.push(
                "→ Call get_window_state(pid, window_id) to inspect a window's UI.".to_owned(),
            );
        }

        let structured = json!({
            "processes": procs.iter().map(|p| json!({"pid":p.pid,"name":p.name})).collect::<Vec<_>>(),
            "windows": windows.iter().map(window_record_json).collect::<Vec<_>>()
        });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}
