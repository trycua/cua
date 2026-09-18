//! set_value tool — matches the Swift reference in SetValueTool.swift.
//!
//! Two modes, determined by the element's AXRole:
//!
//! * **AXPopUpButton**: Find the child option whose AXTitle or AXValue matches
//!   `value` (case-insensitive) and AXPress it directly.  The native macOS popup
//!   menu is never opened, so focus is never stolen.  Falls back to Safari
//!   `osascript do JavaScript` for WebKit `<select>` elements that expose no AX
//!   children when the popup is closed.
//!
//! * **Native single-line text roles**: Replace the text through the keystroke
//!   rung. An `AXValue` write neither starts nor ends an editing session, so a
//!   control whose value is a binding target keeps its own value while every
//!   read-back the driver can perform returns the written string.
//!
//! * **Everything else**: Write `AXValue` directly (sliders, steppers, search
//!   fields whose `AXConfirm` is the commit, web inputs).

use async_trait::async_trait;
use cua_driver_contract::ActionCommit;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_action_names, copy_children, copy_number_attr, copy_string_attr, kAXErrorSuccess,
    perform_action, set_number_attr, set_range_attr, set_string_attr, AXUIElementRef,
};
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;
use core_foundation::base::CFRelease;

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
        description:
            "Set a value on a UI element. Two modes depending on element role:\n\
             \n\
             - **AXPopUpButton / select dropdown**: finds the child option whose \
             title or value matches `value` (case-insensitive) and AXPresses it \
             directly — the native macOS popup menu is never opened, so focus \
             is never stolen. Use this for HTML <select> elements in Safari or \
             any native NSPopUpButton.\n\
             \n\
             - **All other elements**: writes AXValue directly (sliders, steppers, \
             date pickers, native text fields that expose settable AXValue).\n\
             \n\
             For free-form text entry into web inputs, prefer `type_text_chars` \
             which synthesises key events — AXValue writes are ignored by WebKit.\n\
             \n\
             Commits the write on text roles. A native single-line text field \
             is retyped through the keystroke rung and the edit is ended, so \
             the value reaches the binding behind the control; everything else \
             takes an AXValue write ended by the element's own AXConfirm or by \
             a tab/return keystroke. `committed` reports what was observed of \
             that end-of-edit: `committed` only for a typed edit session that \
             ended with the value in place, `not_committed` when the gesture \
             was refused or the app replaced the value, and `unproven` when \
             the value survived but the only evidence is an accessibility \
             read-back, which a bound control echoes whether or not the app \
             took the value. Web content is never committed this way (the \
             renderer never observes an AXValue write), and a multi-line \
             AXTextArea has no end-of-edit gesture.\n\
             \n\
             A committed edit is STILL NOT ON DISK: measured on TextEdit, an \
             AXTextArea write appeared in the AX tree while the document stayed \
             unmarked, with no undo entry and a byte-identical file. Make the \
             change durable with the app's own save action (`press_key` cmd+s, \
             or `invoke_menu` File > Save) before treating the text as written."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "value"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid": { "type": "integer" },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID for the window whose get_window_state produced the element_index. Required when element_index is used; optional when element_token is supplied (the token carries it)."
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
        let resolved = match self.state.element_cache.resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg,
            "set_value",
        ) {
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
        if let Ok(Some((screen_x, screen_y))) = tokio::task::spawn_blocking(move || unsafe {
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
        let ax_echo_surface = super::type_text::target_in_web_area(
            pid,
            Some((element_ptr, Some(element_index))),
            Some(window_id),
        );

        let plan = resolve_write_plan(
            &_mutation_lease,
            window_id,
            element_ptr,
            &value,
            ax_echo_surface,
        )
        .await;

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
                tokio::task::spawn_blocking(move || {
                    set_value_blocking(
                        element_guard.as_ptr(),
                        element_index,
                        pid,
                        window_id,
                        &value,
                        plan,
                    )
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
                let committed = outcome.committed;
                let mut msg = outcome.detail;
                msg.push_str(&changes.result_suffix());
                let verified = outcome.verified.unwrap_or(false);
                let mut structured = serde_json::json!({
                    "path": outcome.path,
                    "verified": verified,
                    "effect": if verified { "confirmed" } else { "unverifiable" },
                });
                if let Some(committed) = committed {
                    structured["committed"] = serde_json::json!(committed.as_wire());
                }
                if let Some(delivered) = outcome.delivered {
                    structured["delivered_chars"] = serde_json::json!(delivered);
                }
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
            Ok(Err(e)) => ToolResult::error(format!("set_value failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// How the written value reaches the app's own editing pipeline.
#[derive(Debug, PartialEq, Eq)]
enum WritePlan {
    /// Not a text control: the `AXValue` write is the whole edit.
    ValueOnly,
    /// `AXValue`, ended by the control's own advertised `AXConfirm`. For a
    /// search field that action *is* the commit: it runs the search.
    ValueThenConfirm,
    /// `AXValue`, ended by focusing the control and pressing this key.
    ValueThenKey(&'static str),
    /// Replace the control's text through the keystroke rung, then end the
    /// edit with this key.
    ///
    /// An `AXValue` write sets a control's string without starting or ending
    /// an editing session, so a plain `NSTextField` — a save sheet's name
    /// field, an action parameter — never hands the value to the binding
    /// behind it, while every read-back the driver can perform returns the
    /// written string. Keystrokes go through the field editor the binding
    /// reads on end-of-edit, which is the route a person uses.
    Retype(&'static str),
    /// No route exists, or one was refused before dispatch.
    Blocked(String),
}

/// Settle time between the commit gesture and the read-back that judges it.
const COMMIT_SETTLE: std::time::Duration = std::time::Duration::from_millis(120);

/// Settle time between an `AXFocused` write and the read-back that proves it.
const FOCUS_SETTLE: std::time::Duration = std::time::Duration::from_millis(60);

/// Native single-line text roles whose value AppKit carries in a field editor.
/// `AXSearchField` is deliberately absent: its `AXConfirm` is the commit, and
/// an `AXValue` write plus that action is measurably enough.
pub(super) fn is_binding_target_role(role: &str, subrole: &str) -> bool {
    matches!(role, "AXTextField" | "AXSecureTextField") && subrole != "AXSearchField"
}

/// Whether keystrokes can carry this value into a single-line control. Tab
/// moves focus and Return ends the edit, so a value holding either can only
/// ever be written losslessly through `AXValue`.
fn is_typeable(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

fn write_plan(role: &str, subrole: &str, advertised: &[String], value: &str) -> WritePlan {
    match role {
        "AXTextArea" => WritePlan::Blocked(
            "a multi-line AXTextArea has no end-of-edit gesture, so the app may never \
             register the write"
                .to_owned(),
        ),
        _ if is_binding_target_role(role, subrole) && is_typeable(value) => {
            WritePlan::Retype("tab")
        }
        "AXTextField" | "AXSecureTextField" | "AXSearchField" | "AXComboBox" | "AXDateField"
        | "AXTimeField" => {
            if advertised.iter().any(|action| action == "AXConfirm") {
                WritePlan::ValueThenConfirm
            } else if role == "AXSearchField" || subrole == "AXSearchField" {
                WritePlan::ValueThenKey("return")
            } else {
                WritePlan::ValueThenKey("tab")
            }
        }
        _ => WritePlan::ValueOnly,
    }
}

/// Resolve the write route for the addressed element, re-gating the keystroke
/// rung before it is promised: a process-scoped key is a stricter route than
/// the semantic AX write this tool already holds a lease for.
async fn resolve_write_plan(
    lease: &super::BackgroundMutationLease,
    window_id: u32,
    element_ptr: usize,
    value: &str,
    ax_echo_surface: bool,
) -> WritePlan {
    if ax_echo_surface {
        return WritePlan::Blocked(
            "web content never observes an AXValue write, so there is nothing to commit".to_owned(),
        );
    }
    let Ok((role, subrole, advertised)) = tokio::task::spawn_blocking(move || unsafe {
        let element = element_ptr as AXUIElementRef;
        (
            copy_string_attr(element, "AXRole").unwrap_or_default(),
            copy_string_attr(element, "AXSubrole").unwrap_or_default(),
            copy_action_names(element),
        )
    })
    .await
    else {
        return WritePlan::Blocked(
            "the element stopped answering AX reads before the write".to_owned(),
        );
    };
    let plan = write_plan(&role, &subrole, &advertised, value);
    if matches!(plan, WritePlan::ValueThenKey(_) | WritePlan::Retype(_)) {
        if let Err(refusal) = lease
            .gate_again(
                window_id,
                Some(element_ptr),
                cua_driver_core::background_input::BackgroundAction::GenericKey,
            )
            .await
        {
            return refused_keystroke_plan(&plan, &advertised, &refusal);
        }
    }
    plan
}

/// What is left when the keystroke rung is refused before dispatch.
///
/// The gate refuses process-scoped keys whenever the application owns another
/// eligible top-level window, and an open save panel is exactly that: its own
/// top-level window beside the document. `set_value` has no `delivery_mode`,
/// so the refusal is final for this call. An advertised `AXConfirm` is then
/// the only end-of-edit left, and on a save panel it is the panel's own
/// confirm — measured on Automator: the write is saved to disk under the
/// written name. Keeping the `AXValue` route in that case preserves a control
/// the typed route cannot reach; the verdict stays `unproven`, because a
/// read-back is all the evidence there is.
fn refused_keystroke_plan(
    plan: &WritePlan,
    advertised: &[String],
    refusal: &ToolResult,
) -> WritePlan {
    let confirms = advertised.iter().any(|action| action == "AXConfirm");
    if matches!(plan, WritePlan::Retype(_)) && confirms {
        return WritePlan::ValueThenConfirm;
    }
    WritePlan::Blocked(keystroke_refusal_reason(refusal))
}

fn keystroke_refusal_reason(refusal: &ToolResult) -> String {
    let detail = refusal
        .structured_content
        .as_ref()
        .and_then(|structured| structured.get("reason"))
        .and_then(serde_json::Value::as_str);
    match detail {
        Some(detail) => format!("the commit keystroke was refused before dispatch — {detail}"),
        None => "the commit keystroke was refused before dispatch".to_owned(),
    }
}

/// Run the commit gesture for an `AXValue` write and report what was observed
/// of the app's end-of-edit.
fn commit_written_value(
    element: AXUIElementRef,
    pid: i32,
    requested: &str,
    numeric: bool,
    plan: WritePlan,
) -> (Option<ActionCommit>, String) {
    match plan {
        WritePlan::ValueOnly => (None, String::new()),
        WritePlan::Blocked(reason) => (
            Some(ActionCommit::NotCommitted),
            format!(" Not committed: {reason}."),
        ),
        WritePlan::Retype(_) => unreachable!("the retype route does not write AXValue"),
        WritePlan::ValueThenConfirm => {
            let dispatched = unsafe { perform_action(element, "AXConfirm") } == kAXErrorSuccess;
            let survived = dispatched && {
                std::thread::sleep(COMMIT_SETTLE);
                let after = unsafe { copy_string_attr(element, "AXValue") };
                value_matches(after.as_deref(), requested, numeric)
            };
            // An `AXConfirm` is not an end-of-edit the driver can read, and
            // the value never went through the field editor, so the read-back
            // is the only evidence there is.
            judge_commit(dispatched, survived, false, "AXConfirm")
        }
        WritePlan::ValueThenKey(key) => {
            let focused = crate::input::ax_actions::focus_element(element as usize).is_ok();
            let dispatched = focused && crate::input::keyboard::press_key(pid, key, &[]).is_ok();
            let survived = dispatched && {
                std::thread::sleep(COMMIT_SETTLE);
                let after = unsafe { copy_string_attr(element, "AXValue") };
                value_matches(after.as_deref(), requested, numeric)
            };
            // The key ends the edit session, but the value in it arrived
            // through `AXValue` rather than the field editor, so the ended
            // session is not evidence the app took it.
            judge_commit(dispatched, survived, false, key)
        }
    }
}

/// What the driver may claim about the app's end-of-edit.
///
/// An accessibility read-back is echoed by a bound control whether or not the
/// application took the value, so it can only ever refute a commit. The one
/// positive signal available is a *typed* edit session ending — which is what
/// an AppKit binding commits on — observed as the control losing the app's own
/// keyboard focus after a gesture that had established it.
///
/// `typed` is what makes the ended edit mean anything, and it is measured, not
/// assumed. On Automator's "Save as:" action parameter (2026-09-14): an
/// `AXValue` write, a proven `AXFocused` write and a Tab that demonstrably
/// moved focus left the written string in the control's read-back and the
/// application still kept its own value in `document.wflow`. An ended edit
/// session over a value the field editor never saw proves nothing, so an
/// `AXValue` route can reach `unproven` at best; the same sequence with the
/// value typed through the keystroke rung committed.
fn judge_commit(
    dispatched: bool,
    survived: bool,
    typed_edit_ended: bool,
    gesture: &str,
) -> (Option<ActionCommit>, String) {
    if !dispatched {
        return (
            Some(ActionCommit::NotCommitted),
            format!(" Not committed: the {gesture} commit gesture could not be dispatched."),
        );
    }
    if !survived {
        return (
            Some(ActionCommit::NotCommitted),
            " Not committed: the value did not survive the app's end-of-edit, so the app \
             still holds its own value."
                .to_owned(),
        );
    }
    if typed_edit_ended {
        return (
            Some(ActionCommit::Committed),
            format!(
                " Committed via {gesture}: the typed edit session ended with this value in \
                 place."
            ),
        );
    }
    (
        Some(ActionCommit::Unproven),
        format!(
            " Commit unproven: the value survived {gesture}, but the app's own model was not \
             observed — an accessibility read-back is echoed by a bound control whether or not \
             the app took the value. Check the app's own output."
        ),
    )
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
    /// What was observed of the app's own end-of-edit. `None` for paths that
    /// have no commit gesture (menu selection, non-text controls).
    committed: Option<ActionCommit>,
    /// The transport the value actually travelled on, published as
    /// `structured.path`.
    path: &'static str,
    /// Characters the keystroke rung proved delivered, for the retype route.
    delivered: Option<usize>,
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
        for prefix in ["✅ Set", "✅ Typed", "✅ Cleared"] {
            if let Some(rest) = outcome.detail.strip_prefix(prefix) {
                outcome.detail = format!("📨 Sent (unverified){rest}");
                return;
            }
        }
    }
}

fn set_value_blocking(
    element_ptr: usize,
    element_index: usize,
    pid: i32,
    window_id: u32,
    value: &str,
    plan: WritePlan,
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
                committed: None,
                path: "ax",
                delivered: None,
            }
        })
    } else if let WritePlan::Retype(end) = plan {
        retype_blocking(
            element,
            element_ptr,
            element_index,
            pid,
            window_id,
            value,
            end,
            &role,
        )
    } else {
        // Default path: write AXValue directly. Numeric controls (AXSlider /
        // AXStepper) reject a CFString with -25201 and need a CFNumber; text
        // fields take a CFString. Try numeric first when the value parses as a
        // number, then fall back to a string write.
        // Numeric target carried through so we can step toward it if the
        // direct writes are rejected (SwiftUI AXSlider rejects every AXValue
        // write with -25200 yet exposes a readable AXValue + increment/decrement
        // actions).
        let numeric_target = value.trim().parse::<f64>().ok();
        // Read the value before writing so an unchanged field can be reported as
        // idempotent rather than silently indistinguishable from a fresh write.
        let before = unsafe { copy_string_attr(element, "AXValue") };
        let err = match numeric_target {
            Some(n) => {
                let e = unsafe { set_number_attr(element, "AXValue", n) };
                if e == kAXErrorSuccess {
                    e
                } else {
                    unsafe { set_string_attr(element, "AXValue", value) }
                }
            }
            None => unsafe { set_string_attr(element, "AXValue", value) },
        };
        if err == kAXErrorSuccess {
            let after = unsafe { copy_string_attr(element, "AXValue") };
            let (verified, changed) = classify_write(
                before.as_deref(),
                after.as_deref(),
                value,
                numeric_target.is_some(),
            );
            let (committed, commit_detail) =
                commit_written_value(element, pid, value, numeric_target.is_some(), plan);
            let suffix = match (verified, changed) {
                (Some(true), Some(false)) => " Value already matched; write was idempotent.",
                (Some(true), _) => "",
                (Some(false), _) => " Read-back did not confirm the value; verify via screenshot.",
                (None, _) => " Value is not readable through AX; could not confirm.",
            };
            Ok(SetValueOutcome {
                detail: format!(
                    "✅ Set AXValue on [{element_index}] {role}.{suffix}{commit_detail}"
                ),
                verified,
                changed,
                committed,
                path: "ax",
                delivered: None,
            })
        } else if let Some(target) = numeric_target {
            // Both direct writes failed for a numeric target — fall back to
            // stepping the control via AXIncrement / AXDecrement actions.
            if step_to_value(element, target) {
                let after = unsafe { copy_string_attr(element, "AXValue") };
                let (verified, changed) =
                    classify_write(before.as_deref(), after.as_deref(), value, true);
                Ok(SetValueOutcome {
                    detail: format!(
                        "✅ Set AXValue on [{element_index}] {role} via AXIncrement/AXDecrement stepping."
                    ),
                    verified,
                    changed,
                    committed: None,
                    path: "ax",
                    delivered: None,
                })
            } else {
                anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
            }
        } else {
            anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
        }
    }
}

/// Replace a bound control's text through the keystroke rung.
///
/// The order is the one AppKit requires: focus installs the field editor, the
/// selection write makes the first keystroke a replacement rather than an
/// insertion, the keystrokes land in the editor the binding reads, and the
/// end-of-edit key is what makes the binding read it.
#[allow(clippy::too_many_arguments)]
fn retype_blocking(
    element: AXUIElementRef,
    element_ptr: usize,
    element_index: usize,
    pid: i32,
    window_id: u32,
    value: &str,
    end: &'static str,
    role: &str,
) -> anyhow::Result<SetValueOutcome> {
    let before = unsafe { copy_string_attr(element, "AXValue") };

    crate::input::ax_actions::focus_element(element_ptr)
        .map_err(|error| anyhow::anyhow!("could not focus [{element_index}] {role}: {error}"))?;
    std::thread::sleep(FOCUS_SETTLE);
    if !crate::input::ax_actions::is_element_focused(pid, element_ptr) {
        // AppKit can install the window's remembered first responder just
        // after the write. One re-apply covers that.
        let _ = crate::input::ax_actions::focus_element(element_ptr);
        std::thread::sleep(FOCUS_SETTLE);
    }
    if !crate::input::ax_actions::is_element_focused(pid, element_ptr) {
        anyhow::bail!(
            "[{element_index}] {role} did not take keyboard focus, so typed keystrokes cannot \
             be proven to reach it; click the control first, or retry with \
             delivery_mode \"foreground\""
        );
    }

    let existing = before.as_deref().unwrap_or_default().chars().count();
    if existing > 0 && !select_all_text(element, existing) {
        anyhow::bail!(
            "[{element_index}] {role} refused both an AXSelectedTextRange selection and an \
             AXValue clear, so typing would append to its current value"
        );
    }

    // A keystroke has to make the change for the editor to notice it, so an
    // empty value is delivered as a deletion of the selection.
    let (delivered_all, delivered) = if value.is_empty() {
        crate::input::keyboard::press_key(pid, "delete", &[])?;
        std::thread::sleep(COMMIT_SETTLE);
        let after = unsafe { copy_string_attr(element, "AXValue") };
        (after.as_deref() == Some(""), Some(0))
    } else {
        super::type_text::type_and_drain(
            pid,
            value,
            0,
            Some(""),
            Some((element_ptr, Some(element_index))),
            Some(window_id),
        )?
    };

    let dispatched = crate::input::keyboard::press_key(pid, end, &[]).is_ok();
    let (survived, edit_ended) = if dispatched {
        std::thread::sleep(COMMIT_SETTLE);
        let after = unsafe { copy_string_attr(element, "AXValue") };
        (
            value_matches(after.as_deref(), value, false),
            !crate::input::ax_actions::is_element_focused(pid, element_ptr),
        )
    } else {
        (false, false)
    };
    let (committed, commit_detail) = judge_commit(dispatched, survived, edit_ended, end);

    let after = unsafe { copy_string_attr(element, "AXValue") };
    let (verified, changed) = classify_write(before.as_deref(), after.as_deref(), value, false);
    let delivery = match (delivered_all, delivered) {
        (true, _) => String::new(),
        (false, Some(count)) => format!(
            " Only {count} of {} character(s) were observed in the control.",
            value.chars().count()
        ),
        (false, None) => " Delivery is not readable through AX.".to_owned(),
    };
    let gesture = if value.is_empty() {
        format!("✅ Cleared [{element_index}] {role}")
    } else {
        format!(
            "✅ Typed {} character(s) into [{element_index}] {role}",
            value.chars().count()
        )
    };
    Ok(SetValueOutcome {
        detail: format!(
            "{gesture} through the app's own editing pipeline.{delivery}{commit_detail}"
        ),
        verified,
        changed,
        committed,
        path: "key_events",
        delivered,
    })
}

/// Select the control's whole value so the first keystroke replaces it.
/// A control that refuses a selection write still accepts an `AXValue` clear;
/// without one of the two, typing would append to the current value.
fn select_all_text(element: AXUIElementRef, length: usize) -> bool {
    if unsafe { set_range_attr(element, "AXSelectedTextRange", 0, length as isize) }
        == kAXErrorSuccess
    {
        return true;
    }
    (unsafe { set_string_attr(element, "AXValue", "") }) == kAXErrorSuccess
}

/// Whether an observed AXValue string equals `expected`. Numeric controls are
/// compared numerically so `"25"` matches a slider that reports `"25.0"`.
fn value_equal(observed: &str, expected: &str, numeric: bool) -> bool {
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
}

/// An unreadable AXValue never counts as a match: the write is unproven, not
/// confirmed.
fn value_matches(observed: Option<&str>, expected: &str, numeric: bool) -> bool {
    observed.is_some_and(|observed| value_equal(observed, expected, numeric))
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
    let verified = value_equal(after, requested, numeric);
    let changed = before.map(|before| !value_equal(after, before, numeric));
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
fn step_to_value(element: AXUIElementRef, target: f64) -> bool {
    // Can't target precisely without feedback — bail if AXValue is unreadable.
    let mut current = match unsafe { copy_number_attr(element, "AXValue") } {
        Some(v) => v,
        None => return false,
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
            return true;
        }

        let action = if current < target {
            "AXIncrement"
        } else {
            "AXDecrement"
        };
        let _ = unsafe { perform_action(element, action) };

        let next = match unsafe { copy_number_attr(element, "AXValue") } {
            Some(v) => v,
            None => return false,
        };

        // The action didn't move the value — the control can't be stepped (or
        // has hit a min/max bound short of target). Stop to avoid looping.
        if next == current {
            return false;
        }

        // Refine the stop threshold to half of the actual step the control took.
        let step = (next - current).abs();
        if step > 0.0 {
            step_radius = step / 2.0;
        }
        current = next;
    }

    // Exhausted the iteration cap without converging.
    (current - target).abs() <= step_radius
}

// ── AXPopUpButton path ───────────────────────────────────────────────────────

fn select_popup_option(
    element: AXUIElementRef,
    element_index: usize,
    pid: i32,
    value: &str,
    element_title: &str,
) -> anyhow::Result<String> {
    let children = unsafe { copy_children(element) };

    if !children.is_empty() {
        // Strategy 1: AX children (native AppKit NSPopUpButton).
        let value_lower = value.to_lowercase();
        let mut matched_idx: Option<usize> = None;
        let mut available: Vec<String> = Vec::with_capacity(children.len());

        for (i, &child) in children.iter().enumerate() {
            let child_title = unsafe { copy_string_attr(child, "AXTitle") }.unwrap_or_default();
            let child_value = unsafe { copy_string_attr(child, "AXValue") }.unwrap_or_default();
            available.push(child_title.clone());
            if child_title.to_lowercase() == value_lower
                || child_value.to_lowercase() == value_lower
            {
                matched_idx = Some(i);
                break;
            }
        }

        let result = if let Some(i) = matched_idx {
            let child = children[i];
            let opt_title =
                unsafe { copy_string_attr(child, "AXTitle") }.unwrap_or_else(|| value.to_string());
            let err = unsafe { perform_action(child, "AXPress") };
            if err == kAXErrorSuccess {
                Ok(format!(
                    "✅ Selected '{opt_title}' in AXPopUpButton [{element_index}] \
                     \"{element_title}\" via AX child AXPress."
                ))
            } else {
                anyhow::bail!("AXPress on child option failed with error {err}")
            }
        } else {
            let avail = available
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "No AX child matching '{value}' in AXPopUpButton [{element_index}] \
                 \"{element_title}\". Available: [{avail}]"
            )
        };

        // Release children (copy_children retains each one).
        for &child in &children {
            unsafe {
                CFRelease(child as _);
            }
        }

        return result;
    }

    // Strategy 2: Safari/WebKit — no AX children when popup is closed.
    // Use osascript do JavaScript to set the <select> element's DOM value.
    let app_name = crate::apps::get_app_name_for_pid(pid).unwrap_or_default();

    if app_name != "Safari" {
        anyhow::bail!(
            "AXPopUpButton [{element_index}] '{element_title}' has no AX children and \
             target is '{app_name}' (not Safari) — no fallback available."
        )
    }

    set_select_via_js(element_index, element_title, value)
}

// ── Safari JavaScript fallback ───────────────────────────────────────────────

/// Set an HTML `<select>` value in Safari via `osascript do JavaScript`.
/// Searches all `<select>` elements for an `<option>` whose text or value matches
/// `value` (case-insensitive), then sets it and dispatches a `change` event.
fn set_select_via_js(
    element_index: usize,
    element_title: &str,
    value: &str,
) -> anyhow::Result<String> {
    // Percent-encode the lowercased value using only unreserved URL characters
    // as the allowed set, matching the Swift reference's percent-encoding approach.
    // This makes the string safe to embed in both a JS single-quoted string
    // (via decodeURIComponent) and an AppleScript double-quoted string.
    let v_low = value.to_lowercase();
    let v_encoded = percent_encode_unreserved(&v_low);

    // JavaScript that matches the Swift reference verbatim.
    let js = format!(
        "(function(){{\
         var v=decodeURIComponent('{v_encoded}');\
         var ss=document.querySelectorAll('select'),opts=[];\
         for(var i=0;i<ss.length;i++){{\
         for(var j=0;j<ss[i].options.length;j++){{\
         var t=ss[i].options[j].text.toLowerCase(),\
         u=ss[i].options[j].value.toLowerCase();\
         opts.push(t+'|'+u);\
         if(t===v||u===v){{\
         ss[i].value=ss[i].options[j].value;\
         ss[i].dispatchEvent(new Event('change',{{bubbles:true}}));\
         return 'SET:'+ss[i].value;}}}}\
         }}return 'NOTFOUND:'+opts.join(',');\
         }})()"
    );

    let apple_script =
        format!("tell application \"Safari\" to do JavaScript \"{js}\" in front document");

    // Spawn osascript with a 10-second deadline. A stuck Safari permission
    // prompt or unresponsive renderer can cause wait() to block indefinitely,
    // which would stall the MCP tool handler permanently.
    let mut child = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&apple_script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("osascript launch failed: {e}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    anyhow::bail!("osascript timed out after 10 seconds");
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => anyhow::bail!("osascript wait error: {e}"),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| anyhow::anyhow!("osascript output error: {e}"))?;

    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();

    if let Some(dom_val) = raw.strip_prefix("SET:") {
        Ok(format!(
            "✅ Set select [{element_index}] '{element_title}' to '{value}' via \
             Safari JavaScript (DOM value: \"{dom_val}\")."
        ))
    } else if let Some(available) = raw.strip_prefix("NOTFOUND:") {
        anyhow::bail!(
            "No <option> matching '{value}' found in any <select>. \
             Available (text|value): {available}"
        )
    } else if raw.is_empty() && !out.status.success() {
        let err_text = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("osascript failed: {}", err_text.trim())
    } else {
        anyhow::bail!(
            "JavaScript returned unexpected output: {}",
            &raw[..raw.len().min(200)]
        )
    }
}

// ── Percent-encoding helper ──────────────────────────────────────────────────

/// Percent-encode a string, leaving only unreserved URL characters (`-._~` +
/// alphanumerics) unencoded.  Matches the Swift reference's approach.
fn percent_encode_unreserved(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_' || b == b'~' {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0xF));
        }
    }
    out
}

fn hex_digit(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + n - 10) as char,
        _ => '0',
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_surface_trust, apply_verification_label, classify_write, commit_written_value,
        judge_commit, refused_keystroke_plan, write_plan, SetValueOutcome, ToolResult, WritePlan,
    };
    use cua_driver_contract::ActionCommit;

    fn outcome(
        detail: &str,
        verified: Option<bool>,
        committed: Option<ActionCommit>,
    ) -> SetValueOutcome {
        SetValueOutcome {
            detail: detail.to_owned(),
            verified,
            changed: Some(true),
            committed,
            path: "ax",
            delivered: None,
        }
    }

    /// The Automator "Save as:" field advertises `AXConfirm`, and an `AXValue`
    /// write plus that action left the app holding `Untitled.txt`. A plain
    /// `NSTextField` has to be typed into, whatever it advertises.
    #[test]
    fn a_binding_target_field_is_written_by_typing() {
        assert_eq!(
            write_plan("AXTextField", "", &["AXConfirm".to_owned()], "name.txt"),
            WritePlan::Retype("tab")
        );
        assert_eq!(
            write_plan("AXTextField", "", &[], "name.txt"),
            WritePlan::Retype("tab")
        );
        assert_eq!(
            write_plan("AXSecureTextField", "", &[], "name.txt"),
            WritePlan::Retype("tab")
        );
    }

    /// Tab moves focus and Return ends the edit, so a value holding either can
    /// only be written losslessly through `AXValue`.
    #[test]
    fn a_value_keystrokes_cannot_carry_keeps_the_ax_value_route() {
        for value in ["\n", " \tΩ café\n", "a\rb"] {
            assert_eq!(
                write_plan("AXTextField", "", &["AXConfirm".to_owned()], value),
                WritePlan::ValueThenConfirm,
                "{value:?}"
            );
        }
        assert_eq!(
            write_plan("AXTextField", "", &[], ""),
            WritePlan::Retype("tab"),
            "an empty value is delivered as a deletion, which is typeable"
        );
    }

    /// A search field's `AXConfirm` runs the search, which is the commit, and
    /// the Automator library search field is measurably filtered by it.
    #[test]
    fn a_search_field_keeps_the_ax_value_route() {
        assert_eq!(
            write_plan(
                "AXTextField",
                "AXSearchField",
                &["AXConfirm".to_owned()],
                "name.txt"
            ),
            WritePlan::ValueThenConfirm
        );
        assert_eq!(
            write_plan("AXSearchField", "", &[], "name.txt"),
            WritePlan::ValueThenKey("return")
        );
        assert_eq!(
            write_plan("AXComboBox", "", &[], "name.txt"),
            WritePlan::ValueThenKey("tab")
        );
    }

    #[test]
    fn controls_without_an_editing_pipeline_report_no_commit() {
        assert_eq!(
            write_plan("AXSlider", "", &[], "name.txt"),
            WritePlan::ValueOnly
        );
        assert_eq!(
            write_plan("AXCheckBox", "", &[], "name.txt"),
            WritePlan::ValueOnly
        );
        let (committed, detail) = commit_written_value(
            std::ptr::null_mut(),
            0,
            "value",
            false,
            WritePlan::ValueOnly,
        );
        assert_eq!(committed, None, "a slider write has nothing to commit");
        assert!(detail.is_empty());
    }

    #[test]
    fn a_text_area_write_is_reported_as_uncommitted() {
        // Measured on TextEdit: the text appeared in the AX tree while the
        // document stayed unmarked. There is no end-of-edit gesture to run.
        let plan = write_plan("AXTextArea", "", &[], "name.txt");
        assert!(matches!(plan, WritePlan::Blocked(_)), "{plan:?}");
        let (committed, detail) =
            commit_written_value(std::ptr::null_mut(), 0, "value", false, plan);
        assert_eq!(committed, Some(ActionCommit::NotCommitted));
        assert!(detail.contains("Not committed"), "{detail}");
    }

    /// A read-back is echoed by a bound control whether or not the app took
    /// the value, so surviving the gesture alone can never say `committed` —
    /// and neither can an ended edit session over a value the field editor
    /// never saw. Measured on Automator's "Save as:" action parameter: an
    /// `AXValue` write plus a Tab that moved focus survived the read-back and
    /// the app still kept its own value on disk.
    #[test]
    fn only_a_typed_ended_edit_is_a_commit() {
        let (committed, detail) = judge_commit(true, true, false, "AXConfirm");
        assert_eq!(committed, Some(ActionCommit::Unproven));
        assert!(detail.contains("Commit unproven"), "{detail}");

        // The AXValue + Tab route: dispatched, survived, and the edit really
        // ended — but untyped, so the only honest verdict is unproven.
        assert_eq!(
            judge_commit(true, true, false, "tab").0,
            Some(ActionCommit::Unproven)
        );
        assert_eq!(
            judge_commit(true, true, true, "tab").0,
            Some(ActionCommit::Committed)
        );
        assert_eq!(
            judge_commit(true, false, true, "tab").0,
            Some(ActionCommit::NotCommitted)
        );
        assert_eq!(
            judge_commit(false, false, false, "tab").0,
            Some(ActionCommit::NotCommitted)
        );
    }

    /// `set_value` has no `delivery_mode`, so a refused keystroke is final for
    /// the call. A save panel is always a sibling top-level window, which is
    /// exactly what the gate refuses on — dropping to the control's own
    /// advertised `AXConfirm` keeps that control reachable instead of turning
    /// the whole write into a refusal.
    #[test]
    fn a_refused_retype_falls_back_to_an_advertised_confirm() {
        let refusal = ToolResult::error("refused".to_owned());
        assert_eq!(
            refused_keystroke_plan(
                &WritePlan::Retype("tab"),
                &["AXShowMenu".to_owned(), "AXConfirm".to_owned()],
                &refusal,
            ),
            WritePlan::ValueThenConfirm
        );
        assert!(matches!(
            refused_keystroke_plan(&WritePlan::Retype("tab"), &[], &refusal),
            WritePlan::Blocked(_)
        ));
        // The AXValue routes have no second gesture to fall back to.
        assert!(matches!(
            refused_keystroke_plan(
                &WritePlan::ValueThenKey("tab"),
                &["AXConfirm".to_owned()],
                &refusal,
            ),
            WritePlan::Blocked(_)
        ));
    }

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
        let mut result = outcome("Set value.", Some(true), Some(ActionCommit::Committed));
        apply_surface_trust(&mut result, true);
        assert_eq!(result.verified, Some(false));
        assert_eq!(result.changed, None);
        assert!(result.detail.contains("not trusted for web content"));
    }

    #[test]
    fn native_read_back_remains_trusted() {
        let mut result = outcome("Set value.", Some(true), Some(ActionCommit::Committed));
        apply_surface_trust(&mut result, false);
        assert_eq!(result.verified, Some(true));
        assert_eq!(result.changed, Some(true));
        assert_eq!(result.detail, "Set value.");
    }

    #[test]
    fn unverified_result_does_not_keep_a_success_checkmark() {
        let mut result = outcome(
            "✅ Set AXValue on [4] AXTextField.",
            Some(false),
            Some(ActionCommit::NotCommitted),
        );
        apply_verification_label(&mut result);
        assert_eq!(
            result.detail,
            "📨 Sent (unverified) AXValue on [4] AXTextField."
        );

        let mut typed = outcome(
            "✅ Typed 4 character(s) into [4] AXTextField.",
            Some(false),
            Some(ActionCommit::Unproven),
        );
        apply_verification_label(&mut typed);
        assert_eq!(
            typed.detail,
            "📨 Sent (unverified) 4 character(s) into [4] AXTextField."
        );
    }
}
