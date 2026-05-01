use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct CheckPermissionsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "check_permissions".into(),
        description: "Check macOS Accessibility and Screen Recording permissions required by \
            cua-driver-rs. Returns the current status of each permission.".into(),
        input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for CheckPermissionsTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let ax_trusted = unsafe { crate::ax::bindings::AXIsProcessTrusted() };
        let screen_recording = check_screen_recording();

        let status_text = format!(
            "Accessibility API: {}\nScreen Recording: {}",
            if ax_trusted { "✅ granted" } else { "❌ not granted — go to System Settings → Privacy & Security → Accessibility" },
            if screen_recording { "✅ granted" } else { "❌ not granted — go to System Settings → Privacy & Security → Screen Recording" }
        );

        ToolResult::text(status_text)
            .with_structured(serde_json::json!({
                "accessibility": ax_trusted,
                "screen_recording": screen_recording
            }))
    }
}

fn check_screen_recording() -> bool {
    // CGWindowListCopyWindowInfo with a window ID will fail without screen recording.
    // A simpler heuristic: try to get the window list and see if we get content.
    let windows = crate::windows::all_windows();
    !windows.is_empty()
}
