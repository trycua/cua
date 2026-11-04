use async_trait::async_trait;
use cua_driver_contract::GetCursorPositionInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::{json, Value};

// ── get_cursor_position ───────────────────────────────────────────────────────

pub struct GetCursorPositionTool;
static GCP_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetCursorPositionTool {
    fn def(&self) -> &ToolDef {
        GCP_DEF.get_or_init(|| ToolDef {
            name: "get_cursor_position".into(),
            description:
                "Return the current mouse cursor position in screen points (origin top-left)."
                    .into(),
            input_schema: json!({"type":"object","properties":{
                "session": cua_driver_core::tool_schema::session_schema()
            },"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        if let Err(error) = parse_typed_input::<GetCursorPositionInput>("get_cursor_position", args)
        {
            return error;
        }
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        // Text format matches Swift `GetCursorPositionTool` 1:1.
        ToolResult::text(format!("✅ Cursor at ({}, {})", pt.x, pt.y))
            .with_structured(json!({ "x": pt.x, "y": pt.y }))
    }
}
