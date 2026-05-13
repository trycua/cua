use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct ListAppsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "list_apps".into(),
        // Description matches Swift `ListAppsTool.swift` verbatim.
        description: "List macOS apps — both currently running and installed-but-not-running — \
            with per-app state flags:\n\n\
            - running: is a process for this app live? (pid is 0 when false)\n\
            - active: is it the system-frontmost app? (implies running)\n\n\
            Only apps with NSApplicationActivationPolicyRegular are included — \
            background helpers and system UI agents are filtered out. Installed \
            apps come from scanning /Applications, /Applications/Utilities, \
            ~/Applications, /System/Applications, and /System/Applications/Utilities.\n\n\
            Use this for \"is X installed?\" as well as \"is X running?\". For \
            per-window state — on-screen, on-current-Space, minimized, \
            window titles — call list_windows instead. For just opening an \
            app — running or not — call launch_app({bundle_id: ...}) directly; \
            list_apps is not a prerequisite.".into(),
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
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let apps = tokio::task::spawn_blocking(crate::apps::list_all_apps).await
            .unwrap_or_default();
        let text = crate::apps::format_app_list(&apps);
        let structured = serde_json::json!({
            "apps": apps.iter().map(|a| serde_json::json!({
                "pid": a.pid,
                "name": a.name,
                "bundle_id": a.bundle_id,
                "active": a.active,
                "running": a.running
            })).collect::<Vec<_>>()
        });
        ToolResult::text(text).with_structured(structured)
    }
}
