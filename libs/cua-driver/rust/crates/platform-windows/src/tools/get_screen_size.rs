use async_trait::async_trait;
use cua_driver_contract::GetScreenSizeInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::{json, Value};

use super::impl_::physical_screen_size;

// ── get_screen_size ───────────────────────────────────────────────────────────

pub struct GetScreenSizeTool;
static GSS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetScreenSizeTool {
    fn def(&self) -> &ToolDef {
        GSS_DEF.get_or_init(|| ToolDef {
            name: "get_screen_size".into(),
            description: "Return the size of the main display in physical pixels plus its display \
                scale factor. On Windows, screenshots and pixel clicks use this same physical-pixel \
                coordinate space. Requires no special permissions.".into(),
            input_schema: json!({"type":"object","properties":{
                "session": cua_driver_core::tool_schema::session_schema()
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        if let Err(error) = parse_typed_input::<GetScreenSizeInput>("get_screen_size", args) {
            return error;
        }
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        // With permonitorv2 DPI awareness (set in cua-driver.manifest),
        // SM_CXSCREEN/SM_CYSCREEN return PHYSICAL pixels — the same
        // coordinate space screenshots and pixel clicks use on Windows.
        // Report these as-is, along with the scale factor for reference.
        let (w, h) = physical_screen_size();
        let dpi = unsafe { GetDpiForSystem() };
        let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
        ToolResult::text(format!("✅ Main display: {w}x{h} pixels @ {scale}x"))
            .with_structured(json!({ "width": w, "height": h, "scale_factor": scale }))
    }
}
