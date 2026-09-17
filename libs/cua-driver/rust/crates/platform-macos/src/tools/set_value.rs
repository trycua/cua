use async_trait::async_trait;
use cua_driver_core::tool::spawn_native;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_number_attr, copy_string_attr, kAXErrorSuccess, perform_action, set_number_attr,
    set_string_attr, AXUIElementRef,
};
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;

use super::ToolState;

pub struct SetValueTool {
    state: Arc<ToolState>,
}

impl SetValueTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "set_value".into(),
        description: "Set a native value through an observed element_token. Popup selection requires one matching, enabled native option; unavailable or ambiguous options refuse. Other controls use their native value type, or advertised numeric stepping when AXValue is not settable. A failed native attempt is not retried through another setter, JavaScript, or keyboard input. Read fresh state to verify the result.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "value"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid": { "type": "integer" },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID. Must match element_token when both are supplied."
                },
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                "value": {
                    "type": "string",
                    "description": "New value. AX will coerce to the element's native type."
                }
            },
            "additionalProperties": false
        }),
        read_only:   false,
        destructive: true,
        idempotent:  true,
        open_world:  true,
    })
}

#[async_trait]
impl Tool for SetValueTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let value = match args.require_str("value") {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Surface 6: element_token / element_index precedence. Neither
        // is now schema-required so the resolver can centralize the
        // "missing addressing" error message.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id");
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match crate::ax::element_resolver::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg,
            "set_value",
        )
        .await
        {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id, element_guard) = match resolved {
            cua_driver_core::element_token::ResolvedElement::None => {
                return ToolResult::error(
                    "set_value requires element_index (+ window_id) or element_token to \
                     address the target element.",
                )
            }
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: Some(wid),
                element_index: idx,
                element,
                ..
            } => match u32::try_from(wid) {
                Ok(wid) => (idx, wid, element),
                Err(_) => return ToolResult::error("window_id is out of range for macOS."),
            },
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: None, ..
            } => {
                return ToolResult::error(
                    "set_value requires window_id when element_index is used \
                 (omit only when supplying element_token, which carries it).",
                )
            }
        };

        let element_ptr = element_guard.as_ptr();

        // set_value is an always-background semantic AX mutation. Re-prove
        // that the retained element still belongs to the requested exact
        // window immediately before any cursor or AX work; a cache hit alone
        // is not delivery proof after a window lifecycle or Space change.
        let _mutation_lease = match super::gate_background_window_action(
            pid,
            window_id,
            Some(element_ptr),
            cua_driver_core::background_input::BackgroundAction::AxSemantic,
        )
        .await
        {
            Ok(lease) => lease,
            Err(refusal_result) => return refusal_result,
        };

        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
        let center_guard = element_guard.clone();
        if let Ok(Some((screen_x, screen_y))) = spawn_native(move || unsafe {
            crate::ax::bindings::element_screen_center(center_guard.as_ptr() as AXUIElementRef)
        })
        .await
        {
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::PinAbove(window_id as u64),
            );
            crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), screen_x, screen_y).await;
            self.state
                .cursor_registry
                .update_position(&cursor_key, screen_x, screen_y);
        }
        // An AXValue read-back is not ground truth for web content. Chromium,
        // WebKit, and Electron can echo the write through accessibility while
        // the renderer never observes it. Reuse type_text's bounded ancestor
        // check so native browser chrome stays trusted but rendered content is
        // always reported as unverified.
        let ax_echo_surface = element_guard.in_web_content();

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // AXValue writes on popups / sliders can cause reflex activations
        // in Chromium-based apps; the AXPopUpButton path also AXPresses a
        // child option which can trigger app activation in some setups.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "set_value.AXValue",
            || async move {
                spawn_native(move || {
                    set_value_blocking(element_guard.checked_ptr()?, element_index, pid, &value)
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        match result {
            Ok(Ok(mut outcome)) => {
                apply_surface_trust(&mut outcome, ax_echo_surface);
                apply_verification_label(&mut outcome);
                let mut msg = outcome.detail;
                msg.push_str(&changes.result_suffix());
                let verified = outcome.verified.unwrap_or(false);
                let mut structured = serde_json::json!({
                    "path": "ax",
                    "verified": verified,
                    "effect": if verified { "confirmed" } else { "unverifiable" },
                });
                if ax_echo_surface {
                    structured["escalation"] = serde_json::json!({
                        "recommended": "px",
                        "reason": "AXValue read-back is not trusted for web content. Verify \
                                   through the renderer; use browser page tools for a tab or \
                                   manipulate the control through its pixel action."
                    });
                }
                ToolResult::text(msg).with_structured(structured)
            }
            Ok(Err(e)) => ToolResult::from_native_error(
                e,
                cua_driver_core::action_record::RequestedDelivery::Background,
            ),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking implementation (runs on spawn_blocking thread) ─────────────────

/// Outcome of a `set_value` write.
///
/// `verified` is `None` for paths that do not perform a value read-back (the
/// AXPopUpButton path drives menu items rather than writing AXValue), and
/// `Some(false)` when a read-back ran but could not confirm the write. A
/// successful `AXUIElementSetAttributeValue` return code is not by itself
/// evidence that the value landed: web content behind an AXWebArea accepts the
/// write and echoes it back through AXValue while the renderer never observes
/// it — the same trap `type_text` already documents.
struct SetValueOutcome {
    detail: String,
    verified: Option<bool>,
    /// `Some(false)` when the element already held the requested value, so the
    /// write was a no-op. Lets callers distinguish "idempotent" from "applied".
    changed: Option<bool>,
}

fn apply_surface_trust(outcome: &mut SetValueOutcome, ax_echo_surface: bool) {
    if ax_echo_surface && outcome.verified == Some(true) {
        outcome.verified = Some(false);
        outcome.changed = None;
        outcome.detail.push_str(
            " AXValue read-back is not trusted for web content; verify the \
             renderer via screenshot or use the browser page tools.",
        );
    }
}

fn apply_verification_label(outcome: &mut SetValueOutcome) {
    if outcome.verified != Some(true) {
        if let Some(rest) = outcome.detail.strip_prefix("✅ Set") {
            outcome.detail = format!("📨 Sent (unverified){rest}");
        }
    }
}

fn set_value_blocking(
    element_ptr: usize,
    element_index: usize,
    pid: i32,
    value: &str,
) -> anyhow::Result<SetValueOutcome> {
    let element = element_ptr as AXUIElementRef;

    let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();

    if role == "AXPopUpButton" {
        let element_title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
        // Menu-item selection, not an AXValue write — no read-back to report.
        select_popup_option(element, element_index, pid, value, &element_title).map(|detail| {
            SetValueOutcome {
                detail,
                verified: None,
                changed: None,
            }
        })
    } else {
        let before_number =
            unsafe { crate::ax::bindings::copy_number_attr_checked(element, "AXValue") }
                .map_err(|code| anyhow::anyhow!("native value type read failed: {code}"))?;
        let numeric = before_number.is_some();
        let numeric_target = if numeric {
            Some(
                value
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| anyhow::anyhow!("numeric control requires a finite number"))?,
            )
        } else {
            None
        };
        let before = before_number
            .map(|value| value.to_string())
            .or_else(|| unsafe { copy_string_attr(element, "AXValue") });
        let settable =
            unsafe { crate::ax::bindings::is_attribute_settable_checked(element, "AXValue") }
                .map_err(|code| anyhow::anyhow!("value capability read failed: {code}"))?;
        if !settable {
            let target = numeric_target.ok_or_else(|| {
                anyhow::anyhow!("target does not support native value replacement")
            })?;
            if !step_to_value(element, target)? {
                return Err(cua_driver_core::protocol::ToolResult::native_action_error(format!("native stepping did not confirm the requested value; inspect fresh state and do not replay"), cua_driver_core::action_record::ActionTransport::MacosAxValue));
            }
        } else {
            let status = match numeric_target {
                Some(number) => unsafe { set_number_attr(element, "AXValue", number) },
                None => unsafe { set_string_attr(element, "AXValue", value) },
            };
            if status != kAXErrorSuccess {
                return Err(cua_driver_core::protocol::ToolResult::native_action_error(format!("native value outcome is unknown ({status}); inspect fresh state and do not replay"), cua_driver_core::action_record::ActionTransport::MacosAxValue));
            }
        }
        let after = unsafe { copy_number_attr(element, "AXValue") }
            .map(|value| value.to_string())
            .or_else(|| unsafe { copy_string_attr(element, "AXValue") });
        let (verified, changed) =
            classify_write(before.as_deref(), after.as_deref(), value, numeric);
        Ok(SetValueOutcome {
            detail: format!("Set native value on [{element_index}] {role}; inspect fresh state before retrying."),
            verified,
            changed,
        })
    }
}

/// Decide what a post-write AXValue read proves.
///
/// Returns `(verified, changed)`:
/// - `verified = None` when AXValue is not readable at all, so the write can be
///   neither confirmed nor denied.
/// - `verified = Some(true)` when the read-back equals the requested value.
///   Numeric controls are compared numerically so `"25"` matches a slider that
///   reports `"25.0"`.
/// - `changed = Some(false)` when the read-back equals what was there before,
///   i.e. the element's value did not move. Combined with `verified` this
///   separates "already had the requested value" (verified + unchanged) from
///   "the write did not take" (unverified + unchanged).
fn classify_write(
    before: Option<&str>,
    after: Option<&str>,
    requested: &str,
    numeric: bool,
) -> (Option<bool>, Option<bool>) {
    let Some(after) = after else {
        return (None, None);
    };
    let matches = |observed: &str, expected: &str| -> bool {
        if observed == expected {
            return true;
        }
        if !numeric {
            return false;
        }
        match (
            observed.trim().parse::<f64>(),
            expected.trim().parse::<f64>(),
        ) {
            (Ok(a), Ok(b)) => {
                let scale = a.abs().max(b.abs()).max(1.0);
                (a - b).abs() <= 1e-9 * scale
            }
            _ => false,
        }
    };
    let verified = matches(after, requested);
    let changed = before.map(|before| !matches(after, before));
    (Some(verified), changed)
}

// ── AXIncrement / AXDecrement stepping fallback ──────────────────────────────

/// Step a numeric control toward `target` using its `AXIncrement` /
/// `AXDecrement` actions. Used only when direct `AXValue` writes are rejected
/// (notably SwiftUI's `AXSlider`, which exposes a readable-but-unsettable
/// `AXValue` plus increment/decrement actions).
///
/// Returns `true` once the control's value lands within half of the last
/// observed step of `target`, `false` if it can't be read or can't be moved.
fn step_to_value(element: AXUIElementRef, target: f64) -> anyhow::Result<bool> {
    // Can't target precisely without feedback — bail if AXValue is unreadable.
    let mut current = match unsafe { copy_number_attr(element, "AXValue") } {
        Some(v) => v,
        None => return Ok(false),
    };

    // Half of the last observed step. Start near-zero so we never declare the
    // target "reached" before performing (and observing) a real
    // AXIncrement/AXDecrement — otherwise a slider at 0.0 targeting 0.5 would
    // report success without ever moving. The radius widens only after we learn
    // the control's actual step size from an observed value change.
    let mut step_radius = f64::EPSILON;

    // Hard cap to prevent runaway on a control that never quite converges.
    for _ in 0..500 {
        if (current - target).abs() <= step_radius {
            return Ok(true);
        }

        let action = if current < target {
            "AXIncrement"
        } else {
            "AXDecrement"
        };
        let actions = unsafe { crate::ax::bindings::copy_action_names_checked(element) }
            .map_err(|code| anyhow::anyhow!("native action read failed: {code}"))?;
        if !actions.iter().any(|name| name == action) {
            anyhow::bail!("target does not advertise {action}");
        }
        let status = unsafe { perform_action(element, action) };
        if status != kAXErrorSuccess {
            return Err(cua_driver_core::protocol::ToolResult::native_action_error(format!("native stepping outcome is unknown ({status}); inspect fresh state and do not replay"), cua_driver_core::action_record::ActionTransport::MacosAxValue));
        }

        let next = match unsafe { copy_number_attr(element, "AXValue") } {
            Some(v) => v,
            None => return Ok(false),
        };

        // The action didn't move the value — the control can't be stepped (or
        // has hit a min/max bound short of target). Stop to avoid looping.
        if next == current {
            return Ok(false);
        }

        // Refine the stop threshold to half of the actual step the control took.
        let step = (next - current).abs();
        if step > 0.0 {
            step_radius = step / 2.0;
        }
        current = next;
    }

    // Exhausted the iteration cap without converging.
    Ok((current - target).abs() <= step_radius)
}

// ── AXPopUpButton path ───────────────────────────────────────────────────────

fn select_popup_option(
    element: AXUIElementRef,
    element_index: usize,
    _pid: i32,
    value: &str,
    element_title: &str,
) -> anyhow::Result<String> {
    use crate::ax::bindings::{
        copy_action_names_checked, copy_bool_attr_checked, copy_element_array,
        copy_string_attr_checked,
    };
    let children = unsafe { copy_element_array(element, "AXChildren") }
        .map_err(|code| anyhow::anyhow!("native option traversal failed: {code}"))?;
    let owned = crate::ax::element_resolver::FreshAxElements {
        elements: children.iter().map(|child| *child as usize).collect(),
    };
    if children.is_empty() || children.len() > 2000 {
        anyhow::bail!("native options are unavailable or incomplete; global document fallback is not supported");
    }
    let mut selected = None;
    for child in children {
        let title = unsafe { copy_string_attr_checked(child, "AXTitle") }
            .map_err(|code| anyhow::anyhow!("native option title read failed: {code}"))?;
        let child_value = unsafe { copy_string_attr_checked(child, "AXValue") }
            .map_err(|code| anyhow::anyhow!("native option value read failed: {code}"))?;
        if title
            .iter()
            .chain(child_value.iter())
            .any(|text| text.to_lowercase() == value.to_lowercase())
        {
            if selected.replace(child).is_some() {
                anyhow::bail!("native option is ambiguous");
            }
        }
    }
    let selected = selected.ok_or_else(|| anyhow::anyhow!("native option was not found"))?;
    let enabled = unsafe { copy_bool_attr_checked(selected, "AXEnabled") }
        .map_err(|code| anyhow::anyhow!("native option state read failed: {code}"))?;
    let actions = unsafe { copy_action_names_checked(selected) }
        .map_err(|code| anyhow::anyhow!("native option action read failed: {code}"))?;
    if enabled == Some(false) || !actions.iter().any(|action| action == "AXPress") {
        anyhow::bail!("native option is disabled or unsupported");
    }
    let status = unsafe { perform_action(selected, "AXPress") };
    drop(owned);
    if status != kAXErrorSuccess {
        return Err(cua_driver_core::protocol::ToolResult::native_action_error(format!("native option outcome is unknown ({status}); inspect fresh state and do not replay"), cua_driver_core::action_record::ActionTransport::MacosAxValue));
    }
    Ok(format!(
        "Selected '{value}' in native popup [{element_index}] '{element_title}'."
    ))
}

#[cfg(test)]
mod tests {
    use super::{apply_surface_trust, apply_verification_label, classify_write, SetValueOutcome};

    #[test]
    fn unreadable_value_reports_neither_verified_nor_changed() {
        // AXValue is not exposed: the write can be neither confirmed nor denied,
        // so the tool must not claim success on the return code alone.
        assert_eq!(
            classify_write(Some("old"), None, "new", false),
            (None, None)
        );
    }

    #[test]
    fn matching_read_back_verifies_the_write() {
        assert_eq!(
            classify_write(Some("old"), Some("new"), "new", false),
            (Some(true), Some(true))
        );
    }

    #[test]
    fn echoed_but_wrong_value_fails_verification() {
        // Web content behind an AXWebArea accepts the write and echoes a value
        // the renderer never took. A success return code must not be reported
        // as a verified write.
        assert_eq!(
            classify_write(Some("old"), Some("old"), "new", false),
            (Some(false), Some(false))
        );
    }

    #[test]
    fn idempotent_write_is_verified_but_unchanged() {
        assert_eq!(
            classify_write(Some("same"), Some("same"), "same", false),
            (Some(true), Some(false))
        );
    }

    #[test]
    fn numeric_controls_compare_numerically() {
        // AXSlider reports "25.0" for a requested "25".
        assert_eq!(
            classify_write(Some("10"), Some("25.000000001"), "25", true),
            (Some(true), Some(true))
        );
    }

    #[test]
    fn numeric_text_is_not_normalised_on_a_text_target() {
        assert_eq!(
            classify_write(Some("old"), Some("7"), "007", false),
            (Some(false), Some(true))
        );
    }

    #[test]
    fn missing_before_still_verifies_numeric_after() {
        assert_eq!(
            classify_write(None, Some("25.0"), "25", true),
            (Some(true), None)
        );
    }

    #[test]
    fn web_content_ax_echo_is_never_reported_as_verified() {
        let mut outcome = SetValueOutcome {
            detail: "Set value.".to_owned(),
            verified: Some(true),
            changed: Some(true),
        };
        apply_surface_trust(&mut outcome, true);
        assert_eq!(outcome.verified, Some(false));
        assert_eq!(outcome.changed, None);
        assert!(outcome.detail.contains("not trusted for web content"));
    }

    #[test]
    fn native_read_back_remains_trusted() {
        let mut outcome = SetValueOutcome {
            detail: "Set value.".to_owned(),
            verified: Some(true),
            changed: Some(true),
        };
        apply_surface_trust(&mut outcome, false);
        assert_eq!(outcome.verified, Some(true));
        assert_eq!(outcome.changed, Some(true));
        assert_eq!(outcome.detail, "Set value.");
    }

    #[test]
    fn unverified_result_does_not_keep_a_success_checkmark() {
        let mut outcome = SetValueOutcome {
            detail: "✅ Set AXValue on [4] AXTextField.".to_owned(),
            verified: Some(false),
            changed: Some(false),
        };
        apply_verification_label(&mut outcome);
        assert_eq!(
            outcome.detail,
            "📨 Sent (unverified) AXValue on [4] AXTextField."
        );
    }
}
