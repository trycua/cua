//! macOS `bring_to_front`.
//!
//! This is the explicit persistent-foreground escape hatch for focus-proxy
//! applications.  A successful native request is only a request receipt: when
//! an exact `window_id` is supplied, the tool independently verifies the
//! process, semantic key window, and WindowServer layer-0 order before it says
//! `activated: true`.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use serde_json::{json, Value};

use crate::ax::bindings::{
    ax_get_window_id, copy_ax_windows, perform_action, set_bool_attr_true,
    AXUIElementCreateApplication,
};

pub struct BringToFrontTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

const VERIFY_TIMEOUT: Duration = Duration::from_millis(900);
const VERIFY_POLL: Duration = Duration::from_millis(20);
const VERIFY_STABLE: Duration = Duration::from_millis(100);

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "bring_to_front".into(),
        description: "Persistently activate an app and leave it in the foreground. Most input \
             does not need this; use it only for a focus-proxy surface that must remain \
             foreground across interactions. With window_id, success means the exact ordinary \
             macOS window was independently verified as the focused window and first in \
             WindowServer layer-0 order. Request acceptance alone is reported as a partial \
             result, never as activation. This DOES steal foreground."
            .into(),
        input_schema: json!({
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExactWindowObservation {
    workspace_frontmost_pid: Option<i32>,
    front_process_matches_target: Option<bool>,
    focused_window_id: Option<u32>,
    frontmost_ordinary_window_id: Option<u32>,
    target_visible_ordinary: bool,
}

impl ExactWindowObservation {
    fn frontmost_pid(self, pid: i32) -> Option<i32> {
        match self.front_process_matches_target {
            Some(true) => Some(pid),
            Some(false) => None,
            None => self.workspace_frontmost_pid,
        }
    }

    fn process_activated(self, pid: i32) -> bool {
        self.frontmost_pid(pid) == Some(pid)
    }

    fn exact_window_focused(self, window_id: u32) -> bool {
        self.focused_window_id == Some(window_id)
    }

    fn exact_window_frontmost_ordinary(self, window_id: u32) -> bool {
        self.target_visible_ordinary && self.frontmost_ordinary_window_id == Some(window_id)
    }

    fn exact_postcondition(self, pid: i32, window_id: u32) -> bool {
        self.process_activated(pid)
            && self.exact_window_focused(window_id)
            && self.exact_window_frontmost_ordinary(window_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExactOutcome {
    Activated,
    Partial,
    Failed,
}

fn classify_exact_outcome(
    request_accepted: bool,
    pid: i32,
    window_id: u32,
    observation: ExactWindowObservation,
) -> ExactOutcome {
    if observation.exact_postcondition(pid, window_id) {
        ExactOutcome::Activated
    } else if request_accepted
        || observation.process_activated(pid)
        || observation.exact_window_focused(window_id)
        || observation.exact_window_frontmost_ordinary(window_id)
    {
        ExactOutcome::Partial
    } else {
        ExactOutcome::Failed
    }
}

fn observe_exact_window(pid: i32, window_id: u32) -> ExactWindowObservation {
    let windows = crate::windows::visible_windows();
    let target_visible_ordinary = windows
        .iter()
        .any(|window| window.pid == pid && window.window_id == window_id && window.layer == 0);
    let frontmost_ordinary_window_id = windows
        .iter()
        .max_by_key(|window| window.z_index)
        .map(|window| window.window_id);
    ExactWindowObservation {
        workspace_frontmost_pid: crate::apps::frontmost_pid(),
        front_process_matches_target: crate::input::skylight::front_process_matches(pid, window_id),
        focused_window_id: crate::ax::bindings::focused_window_id_of_pid(pid),
        frontmost_ordinary_window_id,
        target_visible_ordinary,
    }
}

fn wait_for_exact_window(pid: i32, window_id: u32) -> ExactWindowObservation {
    let deadline = Instant::now() + VERIFY_TIMEOUT;
    let mut stable_since = None;
    let mut observation;
    loop {
        let now = Instant::now();
        observation = observe_exact_window(pid, window_id);
        if observation.exact_postcondition(pid, window_id) {
            let since = stable_since.get_or_insert(now);
            if now.duration_since(*since) >= VERIFY_STABLE {
                return observation;
            }
        } else {
            stable_since = None;
        }
        if now >= deadline {
            return observation;
        }
        std::thread::sleep(VERIFY_POLL);
    }
}

/// Best-effort completion of the one exact-window request. This addresses only
/// the requested AX window; it never orders every window owned by the process.
fn raise_exact_ax_window(pid: i32, window_id: u32) -> bool {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return false;
        }
        let mut target = None;
        for window in copy_ax_windows(app) {
            if target.is_none() && ax_get_window_id(window) == Some(window_id) {
                target = Some(window);
            } else {
                CFRelease(window as CFTypeRef);
            }
        }
        CFRelease(app as CFTypeRef);
        let Some(target) = target else {
            return false;
        };
        let raised = perform_action(target, "AXRaise") == 0;
        let main = set_bool_attr_true(target, "AXMain") == 0;
        let focused = set_bool_attr_true(target, "AXFocused") == 0;
        CFRelease(target as CFTypeRef);
        raised || main || focused
    }
}

fn exact_result(
    pid: i32,
    window_id: u32,
    path: &'static str,
    request_accepted: bool,
    observation: ExactWindowObservation,
) -> ToolResult {
    let outcome = classify_exact_outcome(request_accepted, pid, window_id, observation);
    let activated = outcome == ExactOutcome::Activated;
    let process_activated = observation.process_activated(pid);
    let frontmost_pid = observation.frontmost_pid(pid);
    let exact_window_focused = observation.exact_window_focused(window_id);
    let exact_window_frontmost_ordinary = observation.exact_window_frontmost_ordinary(window_id);
    let status = match outcome {
        ExactOutcome::Activated => "activated",
        ExactOutcome::Partial => "partial",
        ExactOutcome::Failed => "failed",
    };
    let structured = json!({
        "status": status,
        "code": if activated { "bring_to_front_exact_window_verified" } else { "bring_to_front_exact_window_unverified" },
        "pid": pid,
        "window_id": window_id,
        "activated": activated,
        "path": path,
        "request_accepted": request_accepted,
        "process_activated": process_activated,
        "exact_window_effect": {
            "verified": activated,
            "focused": exact_window_focused,
            "frontmost_ordinary": exact_window_frontmost_ordinary,
            "target_visible_ordinary": observation.target_visible_ordinary,
        },
        "observed": {
            "frontmost_pid": frontmost_pid,
            "workspace_frontmost_pid": observation.workspace_frontmost_pid,
            "front_process_matches_target": observation.front_process_matches_target,
            "focused_window_id": observation.focused_window_id,
            "frontmost_ordinary_window_id": observation.frontmost_ordinary_window_id,
        }
    });
    if activated {
        ToolResult::text(format!(
            "Brought exact window {window_id} for pid {pid} to the foreground."
        ))
        .with_structured(structured)
    } else {
        ToolResult::error(format!(
            "bring_to_front: exact window {window_id} for pid {pid} was not verified \
             as frontmost and focused (request_accepted={request_accepted}, \
             process_activated={process_activated}, focused={exact_window_focused}, \
             frontmost_ordinary={exact_window_frontmost_ordinary})."
        ))
        .with_structured(structured)
    }
}

#[async_trait]
impl Tool for BringToFrontTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(Value::as_i64) {
            Some(p) => match libc::pid_t::try_from(p) {
                Ok(pid) => pid,
                Err(_) => {
                    return ToolResult::error(format!(
                        "bring_to_front: `pid` {p} is out of range for a process identifier."
                    ))
                    .with_structured(json!({
                        "code": "bring_to_front_pid_out_of_range",
                        "pid": p,
                    }));
                }
            },
            None => return ToolResult::error("Missing required integer field: pid"),
        };
        let window_id = match args.get("window_id") {
            Some(value) => match value.as_i64().and_then(|value| u32::try_from(value).ok()) {
                Some(window_id) if window_id != 0 => Some(window_id),
                _ => {
                    return ToolResult::error("bring_to_front: window_id is out of range")
                        .with_structured(json!({
                            "code": "bring_to_front_window_id_out_of_range",
                            "pid": pid,
                            "window_id": value,
                            "activated": false,
                            "request_accepted": false,
                        }));
                }
            },
            None => None,
        };

        let Some(app) =
            (unsafe { NSRunningApplication::runningApplicationWithProcessIdentifier(pid) })
        else {
            return ToolResult::error(format!(
                "bring_to_front: no running application for pid {pid} (process not found or exited)."
            ))
            .with_structured(json!({
                "code": "bring_to_front_pid_not_found",
                "pid": pid,
                "activated": false,
                "request_accepted": false,
            }));
        };

        if let Some(window_id) = window_id {
            let Some(window) = crate::windows::window_info_by_id(window_id) else {
                return ToolResult::error(format!(
                    "bring_to_front: window_id {window_id} is stale or unknown."
                ))
                .with_structured(json!({
                    "code": "bring_to_front_window_not_found",
                    "pid": pid,
                    "window_id": window_id,
                    "activated": false,
                    "request_accepted": false,
                }));
            };
            if window.pid != pid {
                return ToolResult::error(format!(
                    "bring_to_front: window_id {window_id} is owned by pid {}, not pid {pid}.",
                    window.pid
                ))
                .with_structured(json!({
                    "code": "bring_to_front_window_pid_mismatch",
                    "pid": pid,
                    "window_id": window_id,
                    "owner_pid": window.pid,
                    "activated": false,
                    "request_accepted": false,
                }));
            }
            if window.layer != 0 {
                return ToolResult::error(format!(
                    "bring_to_front: window_id {window_id} is layer {}, not an ordinary layer-0 window.",
                    window.layer
                ))
                .with_structured(json!({
                    "code": "bring_to_front_window_not_ordinary",
                    "pid": pid,
                    "window_id": window_id,
                    "layer": window.layer,
                    "activated": false,
                    "request_accepted": false,
                }));
            }

            // The persistent kCPSNoWindows request owns process activation
            // without broadly ordering every application window. The separate
            // kCPSUserGenerated sequence then makes only the requested window
            // native-key. Pair both with public Cocoa activation and re-assert
            // only the exact AX window below; the three independent
            // postconditions remain authoritative over every request receipt.
            let skylight_process_accepted =
                crate::input::skylight::set_front_process_persistently(pid, window_id);
            let skylight_exact_accepted =
                crate::input::skylight::make_exact_window_key(pid, window_id);
            let cocoa_accepted = unsafe {
                app.activateWithOptions(
                    NSApplicationActivationOptions::NSApplicationActivateAllWindows,
                )
            };
            let ax_window_requested = raise_exact_ax_window(pid, window_id);
            let path = match (
                skylight_process_accepted,
                skylight_exact_accepted,
                cocoa_accepted,
                ax_window_requested,
            ) {
                (true, true, true, true) => "skylight_process_exact_cocoa_ax",
                (true, true, _, _) => "skylight_process_exact",
                (true, false, _, true) => "skylight_process_ax",
                (true, false, _, false) => "skylight_process",
                (false, true, true, true) => "skylight_exact_cocoa_ax",
                (false, true, _, _) => "skylight_exact",
                (false, false, true, true) => "cocoa_ax",
                (false, false, true, false) => "cocoa",
                (false, false, false, true) => "ax",
                (false, false, false, false) => "none",
            };
            let request_accepted = skylight_process_accepted
                || skylight_exact_accepted
                || cocoa_accepted
                || ax_window_requested;
            return exact_result(
                pid,
                window_id,
                path,
                request_accepted,
                wait_for_exact_window(pid, window_id),
            );
        }

        let request_accepted = unsafe {
            app.activateWithOptions(NSApplicationActivationOptions::NSApplicationActivateAllWindows)
        };
        let deadline = Instant::now() + VERIFY_TIMEOUT;
        let activated = loop {
            if crate::apps::frontmost_pid() == Some(pid) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(VERIFY_POLL);
        };
        let structured = json!({
            "status": if activated { "activated" } else if request_accepted { "partial" } else { "failed" },
            "code": if activated { "bring_to_front_process_verified" } else { "bring_to_front_process_unverified" },
            "pid": pid,
            "window_id": Value::Null,
            "activated": activated,
            "path": "cocoa",
            "request_accepted": request_accepted,
            "process_activated": activated,
        });
        if activated {
            ToolResult::text(format!("Brought pid {pid} to the foreground."))
                .with_structured(structured)
        } else {
            ToolResult::error(format!(
                "bring_to_front: pid {pid} did not become frontmost (request_accepted={request_accepted})."
            ))
            .with_structured(structured)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        workspace_frontmost_pid: Option<i32>,
        front_process_matches_target: Option<bool>,
        focused_window_id: Option<u32>,
        frontmost_ordinary_window_id: Option<u32>,
        target_visible_ordinary: bool,
    ) -> ExactWindowObservation {
        ExactWindowObservation {
            workspace_frontmost_pid,
            front_process_matches_target,
            focused_window_id,
            frontmost_ordinary_window_id,
            target_visible_ordinary,
        }
    }

    #[test]
    fn exact_success_requires_process_focus_and_layer_zero_order() {
        assert_eq!(
            classify_exact_outcome(
                true,
                42,
                7,
                observation(Some(9), Some(true), Some(7), Some(7), true),
            ),
            ExactOutcome::Activated
        );
        for incomplete in [
            observation(Some(42), Some(false), Some(7), Some(7), true),
            observation(Some(42), Some(true), Some(8), Some(7), true),
            observation(Some(42), Some(true), Some(7), Some(8), true),
            observation(Some(42), Some(true), Some(7), Some(7), false),
        ] {
            assert_eq!(
                classify_exact_outcome(true, 42, 7, incomplete),
                ExactOutcome::Partial
            );
        }
    }

    #[test]
    fn process_oracle_falls_back_to_workspace_only_when_skylight_is_unavailable() {
        assert_eq!(
            classify_exact_outcome(
                true,
                42,
                7,
                observation(Some(42), None, Some(7), Some(7), true),
            ),
            ExactOutcome::Activated
        );
        assert_eq!(
            classify_exact_outcome(
                true,
                42,
                7,
                observation(Some(9), None, Some(7), Some(7), true),
            ),
            ExactOutcome::Partial
        );
    }

    #[test]
    fn request_acceptance_or_z_order_alone_is_only_partial() {
        assert_eq!(
            classify_exact_outcome(
                true,
                42,
                7,
                observation(Some(42), Some(true), Some(8), Some(7), true),
            ),
            ExactOutcome::Partial
        );
        assert_eq!(
            classify_exact_outcome(
                false,
                42,
                7,
                observation(None, Some(false), None, Some(7), true),
            ),
            ExactOutcome::Partial
        );
    }

    #[test]
    fn no_request_and_no_observed_effect_is_failed() {
        assert_eq!(
            classify_exact_outcome(
                false,
                42,
                7,
                observation(Some(9), Some(false), Some(8), Some(8), true),
            ),
            ExactOutcome::Failed
        );
    }

    #[test]
    fn partial_result_keeps_request_process_and_exact_effect_distinct() {
        let result = exact_result(
            42,
            7,
            "skylight_ax",
            true,
            observation(Some(9), Some(true), Some(8), Some(7), true),
        );
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured result");
        assert_eq!(structured["status"], "partial");
        assert_eq!(structured["activated"], false);
        assert_eq!(structured["request_accepted"], true);
        assert_eq!(structured["process_activated"], true);
        assert_eq!(structured["observed"]["frontmost_pid"], 42);
        assert_eq!(structured["observed"]["workspace_frontmost_pid"], 9);
        assert_eq!(structured["observed"]["front_process_matches_target"], true);
        assert_eq!(structured["exact_window_effect"]["focused"], false);
        assert_eq!(
            structured["exact_window_effect"]["frontmost_ordinary"],
            true
        );
        assert_eq!(structured["exact_window_effect"]["verified"], false);
    }

    #[test]
    fn structured_frontmost_pid_never_echoes_stale_workspace_state_as_authoritative() {
        let definite_negative = exact_result(
            42,
            7,
            "skylight_ax",
            true,
            observation(Some(42), Some(false), Some(7), Some(7), true),
        )
        .structured_content
        .expect("structured negative result");
        assert_eq!(definite_negative["process_activated"], false);
        assert_eq!(definite_negative["observed"]["frontmost_pid"], Value::Null);
        assert_eq!(definite_negative["observed"]["workspace_frontmost_pid"], 42);

        let fallback = exact_result(
            42,
            7,
            "cocoa_ax",
            true,
            observation(Some(42), None, Some(7), Some(7), true),
        )
        .structured_content
        .expect("structured fallback result");
        assert_eq!(fallback["process_activated"], true);
        assert_eq!(fallback["observed"]["frontmost_pid"], 42);
        assert_eq!(fallback["observed"]["workspace_frontmost_pid"], 42);
    }

    #[test]
    fn verified_result_is_the_only_non_error_exact_mapping() {
        let result = exact_result(
            42,
            7,
            "skylight_ax",
            true,
            observation(Some(9), Some(true), Some(7), Some(7), true),
        );
        assert_eq!(result.is_error, None);
        let structured = result.structured_content.expect("structured result");
        assert_eq!(structured["status"], "activated");
        assert_eq!(structured["activated"], true);
        assert_eq!(structured["exact_window_effect"]["verified"], true);
    }
}
