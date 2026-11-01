use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

use super::impl_::window_record_json;

// ── list_windows ─────────────────────────────────────────────────────────────

pub struct ListWindowsTool;
static LIST_WINDOWS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ListWindowsTool {
    fn def(&self) -> &ToolDef {
        LIST_WINDOWS_DEF.get_or_init(|| ToolDef {
            name: "list_windows".into(),
            description: "List top-level windows. Each record includes z_index (integer or null; \
                higher values are closer to the front; null means stacking order is unavailable \
                and callers must not infer one). To select a frontmost candidate, take the maximum \
                integer z_index; if every value is null, use an explicit fallback instead of \
                relying on array order. Filter with `app` or `pid`: an unfiltered call lists \
                every window on the host.".into(),
            input_schema: json!({"type":"object","properties":{
                "pid":{"type":"integer","description":"Only list windows owned by this process ID."},
                "app":{"type":"string","description":cua_driver_core::tool_args::LIST_WINDOWS_APP_FILTER_DESCRIPTION},
                "on_screen_only":{"type":"boolean","description":"When true, filter to visible windows only. Default false."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let filter_pid = args.opt_u64("pid").map(|v| v as u32);
        let on_screen_only = args.bool_or("on_screen_only", false);
        let mut windows =
            tokio::task::spawn_blocking(move || crate::wayland::list_windows_dispatch(filter_pid))
                .await
                .unwrap_or_default();
        // Exited applications can remain visible in AT-SPI/X11 as zombies;
        // never return those stale targets to callers.
        windows.retain(|window| window.pid.is_none_or(crate::proc_fs::is_process_live));
        if on_screen_only {
            windows.retain(|window| window.is_on_screen);
        }
        if let Some(filter) = args.opt_str("app") {
            windows.retain(|window| {
                cua_driver_core::tool_args::app_filter_matches(&filter, &window.app_name, None)
            });
        }
        if crate::wayland::is_wayland() {
            crate::wayland::remember_observed_window_origins(&windows);
        }
        let mut lines = vec![format!("Found {} windows:", windows.len())];
        for w in &windows {
            lines.push(format!(
                "  window_id={} pid={} \"{}\" {}x{}+{}+{}{}",
                w.xid,
                w.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                w.title,
                w.width,
                w.height,
                w.x,
                w.y,
                if w.app_name.is_empty() {
                    String::new()
                } else {
                    format!(" app={}", w.app_name)
                }
            ));
        }
        let structured =
            json!({ "windows": windows.iter().map(window_record_json).collect::<Vec<_>>() });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}
