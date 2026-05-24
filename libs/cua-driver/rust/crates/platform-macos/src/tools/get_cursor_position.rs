use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct GetCursorPositionTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_cursor_position".into(),
        description: "Return the current mouse cursor position in screen points (origin top-left).".into(),
        input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for GetCursorPositionTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, _args: Value) -> ToolResult {
        // Create a null event and query its location to get cursor position.
        use core_graphics::{
            event::CGEvent,
            event_source::{CGEventSource, CGEventSourceStateID},
        };

        let result = tokio::task::spawn_blocking(|| {
            let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
                .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
            let event = CGEvent::new(source)
                .map_err(|_| anyhow::anyhow!("CGEvent::new failed"))?;
            let loc = event.location();
            Ok::<(f64, f64), anyhow::Error>((loc.x, loc.y))
        }).await;

        match result {
            Ok(Ok((x, y))) => {
                // Truncate to integers and use Swift's text format 1:1
                // (`GetCursorPositionTool.swift`: `Int(pos.x)`, `Int(pos.y)`,
                // `"✅ Cursor at (X, Y)"`).
                let (xi, yi) = (x as i64, y as i64);
                ToolResult::text(format!("✅ Cursor at ({xi}, {yi})"))
                    .with_structured(serde_json::json!({ "x": xi, "y": yi }))
            }
            Ok(Err(e)) => ToolResult::error(format!("Failed to get cursor position: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
