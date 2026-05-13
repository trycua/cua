use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct CheckPermissionsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        // Matches Swift `CheckPermissionsTool.swift` description verbatim.
        name: "check_permissions".into(),
        description: "Report TCC permission status for Accessibility and Screen Recording. \
            By default also raises the system permission dialogs for any missing grants — \
            Apple's request APIs are no-ops when the grant is already active, so this is \
            safe to call repeatedly. Pass {\"prompt\": false} for a purely read-only \
            status check.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "boolean",
                    "description": "Raise the system permission prompts for missing grants. Default true.",
                }
            },
            "additionalProperties": false,
        }),
        // Not read_only because the default path may raise a modal dialog
        // (mirrors Swift annotation `readOnlyHint: false`).
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for CheckPermissionsTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Default to prompting — same default + rationale as Swift.
        let should_prompt = args.get("prompt").and_then(|v| v.as_bool()).unwrap_or(true);
        if should_prompt {
            let _ = request_accessibility();
            let _ = request_screen_recording();
        }
        let accessibility = unsafe { crate::ax::bindings::AXIsProcessTrusted() };
        let screen_recording = probe_screen_recording();

        // Text format mirrors Swift 1:1:
        //   "✅ Accessibility: granted.\n✅ Screen Recording: granted."
        let ax_prefix  = if accessibility   { "✅" } else { "❌" };
        let sr_prefix  = if screen_recording { "✅" } else { "❌" };
        let ax_state   = if accessibility   { "granted" } else { "NOT granted" };
        let sr_state   = if screen_recording { "granted" } else { "NOT granted" };
        let summary = format!(
            "{ax_prefix} Accessibility: {ax_state}.\n{sr_prefix} Screen Recording: {sr_state}."
        );

        ToolResult::text(summary)
            .with_structured(serde_json::json!({
                "accessibility":    accessibility,
                "screen_recording": screen_recording,
            }))
    }
}

/// Raise the Accessibility TCC prompt if not yet granted. No-op when active.
/// Mirrors Swift `Permissions.requestAccessibility()`.
fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let val = CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), val.as_CFType())]);
    unsafe {
        crate::ax::bindings::AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

/// Raise the Screen Recording TCC prompt if not yet granted.
/// Mirrors Swift `Permissions.requestScreenRecording()` (`CGRequestScreenCaptureAccess`).
fn request_screen_recording() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}

/// Real probe — Swift uses `SCShareableContent.excludingDesktopWindows`,
/// which is unavailable from Rust without large ScreenCaptureKit bindings.
/// `CGPreflightScreenCaptureAccess` returns false negatives for
/// subprocess-launched apps but is the closest stable C API.  When that
/// returns false, fall back to the existing window-enumeration heuristic
/// (returns content only when Screen Recording is actually granted).
fn probe_screen_recording() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    if unsafe { CGPreflightScreenCaptureAccess() } { return true; }
    let windows = crate::windows::all_windows();
    !windows.is_empty()
}
