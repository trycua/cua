//! Agent View event hook, registered once by `main.rs` when `--agent-view`
//! is enabled.
//!
//! The trait + factory live in the `pip-preview` crate so the platform
//! backends can implement them without depending on `cua-driver-core`.
//! What lives here is just the per-process callback that the tool
//! dispatcher uses to push frames after each successful tool call —
//! a thin shim so `tool.rs` doesn't need to know about `pip-preview`
//! directly and we keep the dependency graph one-directional.
//!
//! The PNG bytes pushed through here come from the existing
//! `SCREENSHOT_FN` callback (the same source `screenshot.png` uses in
//! the recording pipeline), so PiP shows exactly what the recorder
//! captures.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use serde_json::Value;

/// Synthesized per-call frame payload. Kept structurally identical
/// to `pip_preview::PipFrame` — duplicated here to keep `cua-driver-core`
/// from importing `pip-preview` (the dependency would be circular once
/// platform backends pull both crates in).
#[derive(Clone, Copy)]
pub enum PipHookTargetKind {
    NativeWindow,
    BrowserTab,
}

pub struct PipHookTarget {
    pub workspace_id: String,
    pub workspace_label: String,
    pub target_id: String,
    pub identity_key: String,
    pub target_kind: PipHookTargetKind,
    pub target_label: String,
    pub native_container: Option<PipHookNativeContainer>,
}

#[derive(Clone, Copy)]
pub struct PipHookNativeContainer {
    pub pid: i64,
    pub window_id: u64,
}

pub struct PipHookFrame {
    pub target: PipHookTarget,
    pub png_bytes: Vec<u8>,
    pub action_label: String,
    pub timestamp_ms: u64,
    pub cursor_position: Option<(f64, f64)>,
}

pub enum PipHookEvent {
    Upsert(PipHookFrame),
    SetInputPassthrough {
        passthrough: bool,
    },
    RemoveTarget {
        workspace_id: String,
        identity_key: String,
    },
    RemoveWorkspace {
        workspace_id: String,
    },
}

/// Restores Agent View interactivity even when an action exits early or
/// unwinds. Physical desktop actions are serialized by the dispatcher, so one
/// process-global passthrough scope is sufficient.
pub struct PipInputPassthroughGuard;

impl Drop for PipInputPassthroughGuard {
    fn drop(&mut self) {
        if let Some(f) = PIP_EVENT_FN.get() {
            if let Err(error) = f(PipHookEvent::SetInputPassthrough { passthrough: false }) {
                tracing::error!(%error, "failed to restore Agent View input handling");
            }
        }
    }
}

type PipEventFnBox = Box<dyn Fn(PipHookEvent) -> Result<(), String> + Send + Sync>;
static PIP_EVENT_FN: OnceLock<PipEventFnBox> = OnceLock::new();
static BACKGROUND_ONLY: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundOnlyViolation {
    ForegroundDelivery,
    ExactWindowRequired,
    DriverWindowTarget,
    OperationUnavailable,
}

impl BackgroundOnlyViolation {
    fn code(self) -> &'static str {
        match self {
            Self::ForegroundDelivery => "agent_view_foreground_input_refused",
            Self::ExactWindowRequired => "agent_view_exact_window_required",
            Self::DriverWindowTarget => "agent_view_driver_window_refused",
            Self::OperationUnavailable => "agent_view_operation_unavailable",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::ForegroundDelivery => {
                "background-only Agent View refuses delivery_mode=foreground"
            }
            Self::ExactWindowRequired => {
                "background-only Agent View requires an exact pid and window_id"
            }
            Self::DriverWindowTarget => {
                "background-only Agent View cannot target the Cua Driver process"
            }
            Self::OperationUnavailable => {
                "this operation can activate, move, close, or globally control main-session UI"
            }
        }
    }
}

fn background_only_violation(tool_name: &str, args: &Value) -> Option<BackgroundOnlyViolation> {
    if args.get("delivery_mode").and_then(Value::as_str) == Some("foreground") {
        return Some(BackgroundOnlyViolation::ForegroundDelivery);
    }

    if matches!(
        tool_name,
        "bring_to_front"
            | "browser_prepare"
            | "drag"
            | "invoke_menu"
            | "kill_app"
            | "launch_app"
            | "mouse_button_down"
            | "mouse_button_up"
            | "mouse_drag"
            | "move_cursor"
            | "parallel_mouse_drag"
            | "replay_trajectory"
            | "set_agent_cursor_enabled"
            | "set_agent_cursor_motion"
            | "set_agent_cursor_theme"
            | "set_window_frame"
    ) {
        return Some(BackgroundOnlyViolation::OperationUnavailable);
    }
    if tool_name == "check_permissions"
        && args.get("prompt").and_then(Value::as_bool).unwrap_or(false)
    {
        return Some(BackgroundOnlyViolation::OperationUnavailable);
    }
    if tool_name == "page"
        && args.get("action").and_then(Value::as_str) == Some("enable_javascript_apple_events")
    {
        // This legacy compatibility action quits and relaunches the selected
        // browser, so exact target arguments cannot make it background-only.
        return Some(BackgroundOnlyViolation::OperationUnavailable);
    }

    if args.get("pid").and_then(Value::as_u64) == Some(u64::from(std::process::id())) {
        return Some(BackgroundOnlyViolation::DriverWindowTarget);
    }

    let has_exact_native_target = args
        .get("pid")
        .and_then(Value::as_u64)
        .is_some_and(|pid| pid > 0 && pid <= u64::from(u32::MAX))
        && args
            .get("window_id")
            .and_then(Value::as_u64)
            .is_some_and(|window_id| window_id > 0);
    if matches!(
        tool_name,
        "click"
            | "double_click"
            | "hotkey"
            | "press_key"
            | "right_click"
            | "scroll"
            | "set_value"
            | "type_text"
    ) && !has_exact_native_target
    {
        return Some(BackgroundOnlyViolation::ExactWindowRequired);
    }

    None
}

/// Enable the macOS Agent View launch contract that admits only exact-window,
/// background desktop input. This is process-immutable once selected by the
/// trusted daemon launcher.
pub fn enable_background_only() {
    BACKGROUND_ONLY.store(true, Ordering::Release);
}

/// Refuse an action before authorization, recording, cursor animation, or its
/// platform actuator can observe the request.
pub fn enforce_background_only(
    tool_name: &str,
    args: &Value,
) -> Option<crate::protocol::ToolResult> {
    if !BACKGROUND_ONLY.load(Ordering::Acquire) {
        return None;
    }
    let violation = background_only_violation(tool_name, args)?;
    Some(
        crate::protocol::ToolResult::error(violation.reason()).with_structured(serde_json::json!({
            "code": violation.code(),
            "effect": "refused",
            "agent_view_mode": "background_only",
            "tool": tool_name,
            "reason": violation.reason(),
        })),
    )
}

/// Register the platform-side push callback. `main.rs` calls this
/// once after starting the PiP backend.
pub fn set_pip_event_fn(f: impl Fn(PipHookEvent) -> Result<(), String> + Send + Sync + 'static) {
    let _ = PIP_EVENT_FN.set(Box::new(f));
}

/// True when a PiP backend is wired up. Tool dispatcher uses this to
/// skip the screenshot-bytes path when nothing would consume the
/// frame (avoiding wasted capture work when Agent View is disabled).
pub fn pip_enabled() -> bool {
    PIP_EVENT_FN.get().is_some()
}

/// Temporarily make Agent View input-transparent. The registered backend
/// applies the native state synchronously before this returns.
pub fn begin_pip_input_passthrough() -> Result<Option<PipInputPassthroughGuard>, String> {
    let Some(f) = PIP_EVENT_FN.get() else {
        return Ok(None);
    };
    f(PipHookEvent::SetInputPassthrough { passthrough: true })?;
    Ok(Some(PipInputPassthroughGuard))
}

/// Upsert one exact-target frame. No-op when no backend is registered.
pub fn push_pip_frame(frame: PipHookFrame) {
    if let Some(f) = PIP_EVENT_FN.get() {
        let _ = f(PipHookEvent::Upsert(frame));
    }
}

/// Remove one exact target card. Presentation only; never closes the target.
pub fn remove_pip_target(workspace_id: impl Into<String>, identity_key: impl Into<String>) {
    if let Some(f) = PIP_EVENT_FN.get() {
        let _ = f(PipHookEvent::RemoveTarget {
            workspace_id: workspace_id.into(),
            identity_key: identity_key.into(),
        });
    }
}

/// Remove presentation state for an ended workspace. This never closes the
/// workspace's native windows or browser tabs.
pub fn remove_pip_workspace(workspace_id: impl Into<String>) {
    if let Some(f) = PIP_EVENT_FN.get() {
        let _ = f(PipHookEvent::RemoveWorkspace {
            workspace_id: workspace_id.into(),
        });
    }
}

#[cfg(test)]
mod background_only_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refuses_foreground_delivery_before_platform_dispatch() {
        assert_eq!(
            background_only_violation(
                "click",
                &json!({"pid": 41, "window_id": 9, "delivery_mode": "foreground"}),
            ),
            Some(BackgroundOnlyViolation::ForegroundDelivery)
        );
    }

    #[test]
    fn requires_an_exact_window_for_native_input() {
        assert_eq!(
            background_only_violation("type_text", &json!({"pid": 41})),
            Some(BackgroundOnlyViolation::ExactWindowRequired)
        );
        assert_eq!(
            background_only_violation("type_text", &json!({"pid": -1, "window_id": 9})),
            Some(BackgroundOnlyViolation::ExactWindowRequired)
        );
        assert_eq!(
            background_only_violation("type_text", &json!({"pid": 41, "window_id": 0})),
            Some(BackgroundOnlyViolation::ExactWindowRequired)
        );
        assert_eq!(
            background_only_violation(
                "type_text",
                &json!({"pid": 41, "window_id": 9, "delivery_mode": "background"}),
            ),
            None
        );
    }

    #[test]
    fn refuses_global_and_window_management_operations() {
        for tool in [
            "bring_to_front",
            "drag",
            "invoke_menu",
            "launch_app",
            "move_cursor",
            "set_agent_cursor_enabled",
            "set_window_frame",
        ] {
            assert_eq!(
                background_only_violation(tool, &json!({})),
                Some(BackgroundOnlyViolation::OperationUnavailable),
                "tool: {tool}"
            );
        }
        assert_eq!(
            background_only_violation(
                "page",
                &json!({
                    "action": "enable_javascript_apple_events",
                    "bundle_id": "com.google.Chrome"
                }),
            ),
            Some(BackgroundOnlyViolation::OperationUnavailable)
        );
    }

    #[test]
    fn refuses_the_agent_view_process_as_an_input_target() {
        assert_eq!(
            background_only_violation("click", &json!({"pid": std::process::id(), "window_id": 9}),),
            Some(BackgroundOnlyViolation::DriverWindowTarget)
        );
    }

    #[test]
    fn leaves_exact_browser_and_read_only_calls_available() {
        assert_eq!(
            background_only_violation(
                "browser_click",
                &json!({"target_id": "target", "tab_id": "tab"}),
            ),
            None
        );
        assert_eq!(
            background_only_violation("get_window_state", &json!({"pid": 41, "window_id": 9})),
            None
        );
    }
}
