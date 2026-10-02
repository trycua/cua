use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

pub struct ListAppsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "list_apps".into(),
        description: "List running and installed apps with `running`, `active`, pid, bundle_id \
            and `launch_path`. Not required before launch_app; use list_windows for window state."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for ListAppsTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let apps = tokio::task::spawn_blocking(crate::apps::list_all_apps)
            .await
            .unwrap_or_default();
        let text = crate::apps::format_app_list(&apps);
        // Single flat array. Each entry is the unified shape — existing
        // fields (`pid`, `name`, `bundle_id`, `running`, `active`) are
        // unchanged for backwards compatibility; the new fields
        // (`launch_path`, `kind`, `last_used`, `windows`) are additive.
        let structured = serde_json::json!({
            "apps": apps.iter().map(|a| serde_json::json!({
                "pid": a.pid,
                "name": a.name,
                "bundle_id": a.bundle_id,
                "active": a.active,
                "running": a.running,
                "launch_path": a.launch_path,
                "kind": a.kind,
                "last_used": a.last_used,
                "windows": Vec::<serde_json::Value>::new(),
            })).collect::<Vec<_>>()
        });
        ToolResult::text(text).with_structured(structured)
    }
}
