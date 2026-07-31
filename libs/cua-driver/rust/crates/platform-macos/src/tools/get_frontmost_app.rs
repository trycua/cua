use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

pub struct GetFrontmostAppTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_frontmost_app".into(),
        description:
            "Return the current frontmost macOS application's pid, name, and bundle identifier. \
             This reads NSWorkspace directly and does not scan installed applications."
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
impl Tool for GetFrontmostAppTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        use objc2_app_kit::NSWorkspace;

        let Some(app) = (unsafe { NSWorkspace::sharedWorkspace().frontmostApplication() }) else {
            return ToolResult::error("macOS did not report a frontmost application");
        };
        let pid = unsafe { app.processIdentifier() };
        if pid <= 0 {
            return ToolResult::error("macOS reported an invalid frontmost application pid");
        }
        let name = unsafe {
            app.localizedName()
                .map(|value| value.to_string())
                .unwrap_or_default()
        };
        let bundle_id = unsafe { app.bundleIdentifier().map(|value| value.to_string()) };

        ToolResult::text(format!("Frontmost application: {name} (pid {pid})")).with_structured(
            serde_json::json!({
                "pid": pid,
                "name": name,
                "bundle_id": bundle_id,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_is_read_only_and_accepts_no_target() {
        let definition = def();
        assert!(definition.read_only);
        assert!(!definition.destructive);
        assert!(definition.idempotent);
        assert!(!definition.open_world);
        assert_eq!(
            definition.input_schema["additionalProperties"],
            serde_json::json!(false)
        );
        assert!(definition.input_schema["properties"]
            .as_object()
            .unwrap()
            .is_empty());
    }
}
