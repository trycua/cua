//! macOS `bring_to_front` stub.
//!
//! `bring_to_front` exists to let agents pay a one-shot foreground swap on
//! Windows before driving Chromium-content or GTK-button targets through
//! the SendInput-required code path. On macOS, equivalent functionality is
//! offered by `NSRunningApplication.activate` / Cocoa's window-ordering
//! APIs, which the macOS input tools never need internally — every
//! `CGEvent.postToPid` based dispatch reaches the target without
//! foreground manipulation. So the macOS stub returns an actionable
//! "Windows-only" error.

use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct BringToFrontTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "bring_to_front".into(),
        description:
            "Activate a window so subsequent input tools with `dispatch:\"foreground\"` \
             land on it without a per-call SetForegroundWindow flash. **Windows-only:** \
             on macOS this tool returns an error pointing to the platform-native \
             `NSRunningApplication.activate` (which the macOS input tools don't need \
             because CGEvent.postToPid reaches backgrounded windows). On Linux this \
             tool also stubs out; use `wmctrl -a` or `xdotool windowactivate` if \
             you need explicit activation."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "pid": { "type": "integer" },
                "window_id": { "type": "integer" }
            },
            "additionalProperties": false,
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for BringToFrontTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::error(
            "bring_to_front is Windows-only. On macOS, input tools do not need \
             explicit foreground activation: every CGEvent.postToPid dispatch \
             reaches backgrounded windows. If you need to bring the app \
             visually forward for your own UX reasons, use \
             NSRunningApplication.activate via your own AppleScript / shell \
             call — cua-driver intentionally does not expose that path because \
             it violates the no-foreground contract."
                .to_string(),
        )
        .with_structured(serde_json::json!({
            "code": "bring_to_front_unsupported_on_platform",
            "platform": "macos",
            "suggestion":
                "macOS input tools deliver via CGEvent.postToPid which is \
                 inherently background-safe; no bring_to_front equivalent needed.",
        }))
    }
}
