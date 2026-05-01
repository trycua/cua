use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct ListAppsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "list_apps".into(),
        description: "List running macOS applications with pid and bundle identifier. \
            Only includes apps with NSApplicationActivationPolicyRegular (no helpers/agents). \
            Use this to find a pid before calling list_windows or get_window_state.".into(),
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
        let apps = crate::apps::list_running_apps();
        let text = crate::apps::format_app_list(&apps);
        let structured = serde_json::json!({
            "apps": apps.iter().map(|a| serde_json::json!({
                "pid": a.pid, "name": a.name, "bundle_id": a.bundle_id, "active": a.active
            })).collect::<Vec<_>>()
        });
        ToolResult::text(text).with_structured(structured)
    }
}
