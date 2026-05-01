use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct GetScreenSizeTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_screen_size".into(),
        description: "Return the main screen's width and height in points.".into(),
        input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for GetScreenSizeTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let (w, h) = get_main_screen_size();
        ToolResult::text(format!("Screen: {}×{}", w as i64, h as i64))
            .with_structured(serde_json::json!({ "width": w, "height": h }))
    }
}

fn get_main_screen_size() -> (f64, f64) {
    use core_graphics::display::{CGDisplay, CGRect};
    let display = CGDisplay::main();
    let bounds = display.bounds();
    (bounds.size.width, bounds.size.height)
}
