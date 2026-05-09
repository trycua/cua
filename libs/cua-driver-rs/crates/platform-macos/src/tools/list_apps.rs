use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct ListAppsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "list_apps".into(),
        description: "List macOS applications — both running apps (with pid) and \
            installed-but-not-running apps (pid=0, running=false). \
            Running apps are those with NSApplicationActivationPolicyRegular (no helpers/agents). \
            Installed apps are scanned from /Applications, /System/Applications, and ~/Applications. \
            Use this to find a pid before calling list_windows or get_window_state, \
            or to discover installed apps before launching them.".into(),
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
