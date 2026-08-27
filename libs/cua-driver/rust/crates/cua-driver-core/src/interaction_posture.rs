//! Runtime-owned limits for interaction modes that promise not to disturb the
//! foreground desktop.
//!
//! This policy deliberately lives at the canonical tool-registry boundary,
//! rather than in Agent View's presentation hook. A view may disappear or fail
//! to start, but the runtime that was constructed as background-only must keep
//! the same fail-closed mutation contract for its entire lifetime.

use serde_json::{json, Value};

use crate::protocol::ToolResult;

/// Registry-injected, caller-unforgeable instruction for the macOS click
/// implementation. Strict semantic clicks may use AXPress or AXSelected, but
/// must not cross internally to an element-center coordinate click.
pub const AX_ONLY_CLICK_ARG: &str = "_interaction_posture_ax_only";

/// Immutable interaction posture selected by a trusted runtime owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InteractionPosture {
    /// Preserve the ordinary Cua Driver action surface.
    #[default]
    Normal,
    /// Admit only mutations with a reviewed, exact, background-safe route.
    BackgroundOnly,
}

impl InteractionPosture {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::BackgroundOnly => "background_only",
        }
    }
}

/// Whether a tool has at least one call shape available in this posture.
/// Mixed tools such as `click` remain listed, while their per-call policy is
/// still enforced at dispatch. Unknown future tools are intentionally omitted
/// until their route receives an explicit review.
pub(crate) fn tool_is_available(
    posture: InteractionPosture,
    tool_name: &str,
    read_only: bool,
) -> bool {
    posture == InteractionPosture::Normal
        || is_certified_background_only_observation(tool_name, read_only)
        || matches!(
            tool_name,
            "start_session"
                | "end_session"
                | "check_permissions"
                | "click"
                | "browser_click"
                | "browser_type"
        )
}

/// Observation tools in the assembled macOS registry whose implementations
/// have been reviewed not to foreground an app, synthesize input, or mutate
/// shared host state. Keep this list explicit: `readOnlyHint` is discovery
/// metadata, not sufficient proof that a future tool is safe in this posture.
fn is_certified_background_only_observation(tool_name: &str, read_only: bool) -> bool {
    read_only
        && matches!(
            tool_name,
            // macOS desktop/window observation.
            "list_apps"
                | "list_windows"
                | "get_window_state"
                | "verify_state"
                | "get_screen_size"
                | "get_desktop_state"
                | "get_cursor_position"
                | "get_accessibility_tree"
                | "zoom"
                // Protected reads and status/configuration observation.
                | "clipboard_read"
                | "health_report"
                | "get_config"
                | "get_agent_cursor_state"
                // Exact browser observation; setup remains a separate,
                // mutable tool and is not admitted here.
                | "get_browser_state"
                // Daemon-local recording/session state.
                | "get_recording_state"
                | "get_session"
                | "list_sessions"
                | "get_session_state"
                // Optional encrypted-history reads retain their ordinary
                // protected-resource authorization downstream.
                | "history_status"
                | "history_query"
        )
}

/// Remove call shapes that are read-only only in the UI sense but still write
/// shared host state. Dispatch remains the authority; this projection keeps
/// strict tool discovery from inviting a call that will be refused.
pub(crate) fn project_tool_entry(posture: InteractionPosture, mut entry: Value) -> Value {
    if posture == InteractionPosture::BackgroundOnly {
        entry
            .pointer_mut("/inputSchema/properties")
            .and_then(Value::as_object_mut)
            .map(|properties| properties.remove("screenshot_out_file"));
    }
    entry
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundOnlyViolation {
    ForegroundDelivery,
    ExactNativeTargetRequired,
    DriverTarget,
    RawNativeInput,
    ExplicitBackgroundDeliveryRequired,
    NativeTargetNotCertified,
    BrowserTargetNotCertified,
    MutationNotCertified,
}

impl BackgroundOnlyViolation {
    fn code(self) -> &'static str {
        match self {
            Self::ForegroundDelivery => "agent_view_foreground_input_refused",
            Self::ExactNativeTargetRequired => "agent_view_exact_window_required",
            Self::DriverTarget => "agent_view_driver_window_refused",
            Self::RawNativeInput => "agent_view_raw_input_refused",
            Self::ExplicitBackgroundDeliveryRequired => "agent_view_background_delivery_required",
            Self::NativeTargetNotCertified => "agent_view_native_target_not_certified",
            Self::BrowserTargetNotCertified => "agent_view_browser_target_not_certified",
            Self::MutationNotCertified => "agent_view_operation_unavailable",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::ForegroundDelivery => {
                "background-only interaction refuses delivery_mode=foreground"
            }
            Self::ExactNativeTargetRequired => {
                "background-only native input requires an exact pid and window_id"
            }
            Self::DriverTarget => {
                "background-only interaction cannot target the Cua Driver process"
            }
            Self::RawNativeInput => {
                "background-only interaction refuses raw coordinate and synthetic-event input"
            }
            Self::ExplicitBackgroundDeliveryRequired => {
                "background-only native input requires delivery_mode=background"
            }
            Self::NativeTargetNotCertified => {
                "background-only native input requires a fresh, session-owned element snapshot"
            }
            Self::BrowserTargetNotCertified => {
                "background-only browser input requires an exact, session-owned embedded page binding"
            }
            Self::MutationNotCertified => {
                "this mutation has no certified background-only interaction route"
            }
        }
    }
}

fn positive_pid(args: &Value) -> Option<u64> {
    args.get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid <= i32::MAX as u64)
}

fn has_exact_native_window(args: &Value) -> bool {
    positive_pid(args).is_some()
        && args
            .get("window_id")
            .and_then(Value::as_u64)
            .is_some_and(|window_id| window_id > 0 && window_id <= u64::from(u32::MAX))
        && args
            .get("scope")
            .and_then(Value::as_str)
            .is_none_or(|scope| scope == "window")
}

fn has_exact_semantic_element(args: &Value) -> bool {
    let token = args
        .get("element_token")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.is_empty());
    let indexed_snapshot = args.get("element_index").and_then(Value::as_u64).is_some()
        && args
            .get("snapshot_id")
            .and_then(Value::as_str)
            .is_some_and(|snapshot| !snapshot.is_empty());
    token || indexed_snapshot
}

fn is_certified_native_ax_press(tool_name: &str, args: &Value) -> bool {
    tool_name == "click"
        && args.get("delivery_mode").and_then(Value::as_str) == Some("background")
        && has_exact_native_window(args)
        && has_exact_semantic_element(args)
        && !args.get("x").is_some_and(|value| !value.is_null())
        && !args.get("y").is_some_and(|value| !value.is_null())
        && args
            .get("action")
            .and_then(Value::as_str)
            .is_none_or(|action| action == "press")
        && args
            .get("button")
            .and_then(Value::as_str)
            .is_none_or(|button| button == "left")
        && args
            .get("count")
            .and_then(Value::as_u64)
            .is_none_or(|count| count == 1)
        && args
            .get("modifier")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
        && !args
            .get("from_zoom")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        && args.get("debug_image_out").is_none()
}

fn has_exact_browser_shape(tool_name: &str, args: &Value) -> bool {
    let exact_ids = ["target_id", "tab_id", "ref"].into_iter().all(|key| {
        args.get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    });
    if !exact_ids {
        return false;
    }
    match tool_name {
        "browser_click" => {
            !args.get("x").is_some_and(|value| !value.is_null())
                && !args.get("y").is_some_and(|value| !value.is_null())
                && args
                    .get("input_route")
                    .and_then(Value::as_str)
                    .is_none_or(|route| route == "trusted")
        }
        "browser_type" => {
            args.get("text").and_then(Value::as_str).is_some()
                && args
                    .get("mode")
                    .and_then(Value::as_str)
                    .is_none_or(|mode| mode == "insert_text")
                && !args
                    .get("replace")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        }
        _ => false,
    }
}

fn background_only_violation(
    tool_name: &str,
    args: &Value,
    read_only: bool,
    exact_native_element: bool,
    exact_embedded_browser_target: bool,
) -> Option<BackgroundOnlyViolation> {
    // Several observation tools are marked read-only because they do not
    // change the desktop, but an explicit output path writes shared host
    // state. Treat that argument shape as a mutation before the read-only
    // exemption below.
    if args
        .get("screenshot_out_file")
        .and_then(Value::as_str)
        .is_some_and(|path| !path.trim().is_empty())
    {
        return Some(BackgroundOnlyViolation::MutationNotCertified);
    }

    // Only explicitly reviewed observations may use the read-only route.
    // Unknown tools fail closed even if their discovery metadata claims
    // `readOnlyHint=true`.
    if is_certified_background_only_observation(tool_name, read_only) {
        return None;
    }

    if args.get("delivery_mode").and_then(Value::as_str) == Some("foreground") {
        return Some(BackgroundOnlyViolation::ForegroundDelivery);
    }

    if positive_pid(args) == Some(u64::from(std::process::id())) {
        return Some(BackgroundOnlyViolation::DriverTarget);
    }

    // Lifecycle bookkeeping is an explicitly reviewed in-process mutation. It
    // is needed to own exact target capabilities and to clean them up, and it
    // never dispatches input or changes another application.
    if matches!(tool_name, "start_session" | "end_session") {
        return None;
    }

    // The permission-status shape is observational even though its mixed tool
    // definition is mutable (prompt=true can raise system UI). Keep the exact
    // non-prompting call used by startup/E2E available, but do not admit its
    // prompt-capable variant.
    if tool_name == "check_permissions"
        && !args.get("prompt").and_then(Value::as_bool).unwrap_or(false)
    {
        return None;
    }

    if tool_name == "click" {
        if !has_exact_native_window(args) {
            return Some(BackgroundOnlyViolation::ExactNativeTargetRequired);
        }
        if args.get("x").is_some_and(|value| !value.is_null())
            || args.get("y").is_some_and(|value| !value.is_null())
        {
            return Some(BackgroundOnlyViolation::RawNativeInput);
        }
        if args.get("delivery_mode").and_then(Value::as_str) != Some("background") {
            return Some(BackgroundOnlyViolation::ExplicitBackgroundDeliveryRequired);
        }
        if !exact_native_element {
            return Some(BackgroundOnlyViolation::NativeTargetNotCertified);
        }
        return (!is_certified_native_ax_press(tool_name, args))
            .then_some(BackgroundOnlyViolation::MutationNotCertified);
    }

    if matches!(tool_name, "browser_click" | "browser_type") {
        return (!(has_exact_browser_shape(tool_name, args) && exact_embedded_browser_target))
            .then_some(BackgroundOnlyViolation::BrowserTargetNotCertified);
    }

    // Default deny is intentional. New mutable tools do not silently join the
    // posture merely because their schema happens to accept pid/window fields.
    Some(BackgroundOnlyViolation::MutationNotCertified)
}

/// Enforce the runtime posture before ordinary authorization, recording,
/// cursor animation, presentation hooks, or platform dispatch.
pub(crate) fn enforce(
    posture: InteractionPosture,
    tool_name: &str,
    args: &Value,
    read_only: bool,
    exact_native_element: bool,
    exact_embedded_browser_target: bool,
) -> Option<ToolResult> {
    if posture == InteractionPosture::Normal {
        return None;
    }
    let violation = background_only_violation(
        tool_name,
        args,
        read_only,
        exact_native_element,
        exact_embedded_browser_target,
    )?;
    Some(
        ToolResult::error(violation.reason()).with_structured(json!({
            "status": "refused",
            "code": violation.code(),
            "effect": "refused",
            "interaction_posture": "background_only",
            "agent_view_mode": "background_only",
            "tool": tool_name,
            "reason": violation.reason(),
            "refusal": {
                "code": violation.code(),
                "posture": "background_only",
                "message": violation.reason(),
            },
        })),
    )
}

/// Add implementation constraints only after public arguments have been
/// sanitized, admitted, and copied for recording. Public callers cannot set
/// underscore-prefixed fields, so the platform can trust this marker.
pub(crate) fn apply_trusted_constraints(
    posture: InteractionPosture,
    tool_name: &str,
    args: &mut Value,
) {
    if posture != InteractionPosture::BackgroundOnly || tool_name != "click" {
        return;
    }
    if let Some(arguments) = args.as_object_mut() {
        arguments.insert(AX_ONLY_CLICK_ARG.to_owned(), Value::Bool(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_posture_does_not_change_mutation_admission() {
        assert!(enforce(
            InteractionPosture::Normal,
            "click",
            &json!({"x": 10, "y": 20}),
            false,
            false,
            false,
        )
        .is_none());
    }

    #[test]
    fn background_only_admits_the_proven_native_ax_press_shape() {
        let args = json!({
            "pid": 41,
            "window_id": 9,
            "element_index": 2,
            "snapshot_id": "s00000001",
            "action": "press",
            "delivery_mode": "background",
        });
        assert!(enforce(
            InteractionPosture::BackgroundOnly,
            "click",
            &args,
            false,
            true,
            false,
        )
        .is_none());
    }

    #[test]
    fn background_only_refuses_raw_coordinates_even_with_an_exact_window() {
        let refusal = enforce(
            InteractionPosture::BackgroundOnly,
            "click",
            &json!({
                "pid": 41,
                "window_id": 9,
                "x": 10,
                "y": 20,
                "delivery_mode": "background",
            }),
            false,
            false,
            false,
        )
        .expect("raw input must be refused");
        assert_eq!(
            refusal.structured_content.as_ref().unwrap()["code"],
            "agent_view_raw_input_refused"
        );
    }

    #[test]
    fn semantic_native_press_requires_explicit_background_delivery() {
        let refusal = enforce(
            InteractionPosture::BackgroundOnly,
            "click",
            &json!({
                "pid": 41,
                "window_id": 9,
                "element_index": 2,
                "snapshot_id": "s00000001",
                "action": "press",
            }),
            false,
            true,
            false,
        )
        .expect("an implicit platform delivery default is not certified");
        assert_eq!(
            refusal.structured_content.as_ref().unwrap()["refusal"]["code"],
            "agent_view_background_delivery_required"
        );
    }

    #[test]
    fn browser_mutations_require_both_typed_shape_and_runtime_binding_proof() {
        let args = json!({
            "target_id": "bt-1",
            "tab_id": "tab-1",
            "ref": "p1:2",
        });
        assert!(enforce(
            InteractionPosture::BackgroundOnly,
            "browser_click",
            &args,
            false,
            false,
            true,
        )
        .is_none());
        let refusal = enforce(
            InteractionPosture::BackgroundOnly,
            "browser_click",
            &args,
            false,
            false,
            false,
        )
        .expect("unproved browser binding must be refused");
        assert_eq!(
            refusal.structured_content.as_ref().unwrap()["code"],
            "agent_view_browser_target_not_certified"
        );
    }

    #[test]
    fn unknown_and_legacy_mutations_fail_closed() {
        for tool in [
            "future_mutating_tool",
            "page",
            "clipboard_write",
            "move_cursor",
            "set_window_frame",
        ] {
            let refusal = enforce(
                InteractionPosture::BackgroundOnly,
                tool,
                &json!({}),
                false,
                false,
                false,
            )
            .expect("mutable tool must be explicitly certified");
            assert_eq!(
                refusal.structured_content.as_ref().unwrap()["code"],
                "agent_view_operation_unavailable",
                "tool: {tool}"
            );
        }
    }

    #[test]
    fn permission_status_is_allowed_but_permission_prompts_are_not() {
        for args in [json!({}), json!({"prompt": false})] {
            assert!(enforce(
                InteractionPosture::BackgroundOnly,
                "check_permissions",
                &args,
                false,
                false,
                false,
            )
            .is_none());
        }

        let refusal = enforce(
            InteractionPosture::BackgroundOnly,
            "check_permissions",
            &json!({"prompt": true}),
            false,
            false,
            false,
        )
        .expect("prompt-capable permission check must be refused");
        assert_eq!(
            refusal.structured_content.as_ref().unwrap()["refusal"]["code"],
            "agent_view_operation_unavailable"
        );
    }

    #[test]
    fn read_only_observation_cannot_write_a_screenshot_file() {
        let refusal = enforce(
            InteractionPosture::BackgroundOnly,
            "get_window_state",
            &json!({"screenshot_out_file": "/tmp/agent-view.png"}),
            true,
            false,
            false,
        )
        .expect("shared file output is still a mutation");
        assert_eq!(
            refusal.structured_content.as_ref().unwrap()["refusal"]["code"],
            "agent_view_operation_unavailable"
        );
    }

    #[test]
    fn strict_click_gets_a_caller_unforgeable_ax_only_constraint() {
        let mut args = json!({"pid": 41, "window_id": 9});
        apply_trusted_constraints(InteractionPosture::BackgroundOnly, "click", &mut args);
        assert_eq!(args[AX_ONLY_CLICK_ARG], true);

        let mut normal = json!({"pid": 41, "window_id": 9});
        apply_trusted_constraints(InteractionPosture::Normal, "click", &mut normal);
        assert!(normal.get(AX_ONLY_CLICK_ARG).is_none());
    }
}
