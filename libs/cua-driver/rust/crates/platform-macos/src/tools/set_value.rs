//! set_value tool — matches the Swift reference in SetValueTool.swift.
//!
//! Two modes, determined by the element's AXRole:
//!
//! * **AXPopUpButton**: Find the option (a child, or an `AXMenuItem` under the
//!   popup's `AXMenu`) whose AXTitle or AXValue matches `value`
//!   (case-insensitive) and AXPress it.  AppKit and Chromium popups publish
//!   their items only while the menu is open, so the menu is opened for the
//!   selection and closed again.  Safari `<select>` elements are set through
//!   `osascript do JavaScript` instead.
//!
//! * **Everything else**: Write `AXValue` directly (sliders, steppers, native
//!   text fields that expose a settable AXValue).

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_bool_attr, copy_children, copy_number_attr, copy_string_attr, copy_url_attr,
    kAXErrorSuccess, perform_action, set_number_attr, set_string_attr, AXUIElementRef,
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
        description: "Set an element's value by `element_token`. A popup button / select dropdown: selects the menu item whose title or value matches `value` (case-insensitive). Safari `<select>` is set through the DOM; AppKit and Chromium popups list items only while open, so the menu is opened, the item pressed and the menu closed again (in the background the user's front window keeps key focus). An unknown value lists the options. Any other element: writes AXValue (sliders, steppers, date pickers, native text fields).\n\
            \n\
            A Finder file name cell (list or icon view, not being edited) is renamed through Finder's scripting interface with Finder left in the background, and the new name is read back from disk; the value is the full new name with its extension. macOS asks the user once to allow Cua Driver to control Finder. Finder's Get Info `Name` field is refused with `file_name_needs_rename`. For free-form text in web inputs use `type_text_chars`: WebKit ignores AXValue writes.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it." },
                "pid": { "type": "integer", "description": "Target process ID. Optional with element_token (the token carries the pid)." },
                "window_id": {
                    "type": "integer",
                    "description": "Window ID. Omit with element_token."
                },
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "value": {
                    "type": "string",
                    "description": "New value, coerced to the element's type."
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
        let pid = match super::target_pid(&self.state, &args) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let value = match args.require_str("value") {
            Ok(v) => v,
            Err(e) => return e,
        };

        let (element_index, window_id, element_guard) =
            match self.state.snapshots.resolve(pid, &args) {
                Ok(cua_driver_core::element_token::ResolvedElement::None) => {
                    return ToolResult::error(
                        "set_value requires element_token to address the target element.",
                    )
                }
                Ok(cua_driver_core::element_token::ResolvedElement::Element {
                    window_id,
                    element_index,
                    element,
                }) => match u32::try_from(window_id) {
                    Ok(window_id) => (element_index, window_id, element),
                    Err(_) => return ToolResult::error("window_id is out of range for macOS."),
                },
                Err(refusal) => return refusal,
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

        // A file's name as Finder lists it takes an AXValue write and reads it
        // back, but the file is never renamed. Refuse before anything moves.
        // Finder's Get Info Name field does the same.
        // In Finder, the file is renamed through Finder's scripting interface
        // instead, with Finder left in the background, and read back from
        // disk.
        let name_guard = element_guard.clone();
        let name_target = tokio::task::spawn_blocking(move || unsafe {
            let element = name_guard.as_ptr() as AXUIElementRef;
            if file_name_cell(element) {
                let finder =
                    crate::apps::bundle_id_for_pid(pid).as_deref() == Some("com.apple.finder");
                match copy_url_attr(element)
                    .as_deref()
                    .and_then(crate::apple_events::file_url_to_path)
                {
                    Some(path) if finder => Some(NameTarget::FinderItem(path)),
                    _ => Some(NameTarget::Refuse(LIST_RENAME_ROUTE)),
                }
            } else if get_info_name_field(pid, element) {
                Some(NameTarget::Refuse(GET_INFO_RENAME_ROUTE))
            } else {
                None
            }
        })
        .await
        .ok()
        .flatten();
        match name_target {
            Some(NameTarget::Refuse(reason)) => {
                return file_name_needs_rename(pid, window_id, reason)
            }
            Some(NameTarget::FinderItem(path)) => {
                let new_name = value.clone();
                let rename_path = path.clone();
                let renamed = tokio::task::spawn_blocking(move || {
                    crate::apple_events::finder_rename(&rename_path, &new_name)
                })
                .await;
                return finder_rename_result(pid, window_id, &path, &value, renamed);
            }
            None => {}
        }

        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
        let center_guard = element_guard.clone();
        if let Ok((Some((screen_x, screen_y)), target_rect)) =
            tokio::task::spawn_blocking(move || unsafe {
                let el = center_guard.as_ptr() as AXUIElementRef;
                (
                    crate::ax::bindings::element_screen_center(el),
                    crate::ax::bindings::element_screen_rect(el),
                )
            })
            .await
        {
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::PinAbove(window_id as u64),
            );
            crate::cursor::overlay::animate_cursor_to_target(
                cursor_key.clone(),
                screen_x,
                screen_y,
                target_rect,
            )
            .await;
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
                    set_value_blocking(element_guard.as_ptr(), element_index, pid, &value)
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;

        match result {
            Ok(Ok(outcome)) if outcome.not_applied && !ax_echo_surface => {
                let vcl = crate::input::pointer_toolkit::detect(pid)
                    == Some(crate::input::pointer_toolkit::PointerReadingToolkit::Vcl);
                not_applied_result(element_index, vcl)
            }
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
                if let Some(edited) = outcome.document_edited {
                    structured["document_key_edit_sent"] = serde_json::json!(edited);
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

// ── File name cells ──────────────────────────────────────────────────────────

const FILE_NAME_NEEDS_RENAME: &str = "file_name_needs_rename";

/// Whether `element` is a file's name as a list shows it (Finder's list and
/// icon views): a text field that names a file and is not being edited.
/// Finder's inline rename editor is focused, so it stays writable.
unsafe fn file_name_cell(element: AXUIElementRef) -> bool {
    copy_string_attr(element, "AXRole").as_deref() == Some("AXTextField")
        && is_file_name_cell(
            copy_string_attr(element, "AXFilename").as_deref(),
            copy_url_attr(element).as_deref(),
            copy_bool_attr(element, "AXFocused"),
        )
}

fn is_file_name_cell(filename: Option<&str>, url: Option<&str>, focused: Option<bool>) -> bool {
    filename.is_some_and(|name| !name.is_empty())
        && url.is_some_and(|url| url.starts_with("file://"))
        && focused != Some(true)
}

/// Whether `element` is the Name & Extension field of Finder's Get Info
/// window. It names no file through AXFilename or AXURL, so `file_name_cell`
/// misses it, yet an AXValue write there renames nothing either.
unsafe fn get_info_name_field(pid: i32, element: AXUIElementRef) -> bool {
    is_get_info_name_field(
        crate::apps::bundle_id_for_pid(pid).as_deref(),
        copy_string_attr(element, "AXRole").as_deref(),
        copy_string_attr(element, "AXIdentifier").as_deref(),
        copy_bool_attr(element, "AXFocused"),
    )
}

fn is_get_info_name_field(
    bundle_id: Option<&str>,
    role: Option<&str>,
    identifier: Option<&str>,
    focused: Option<bool>,
) -> bool {
    bundle_id == Some("com.apple.finder")
        && role == Some("AXTextField")
        && identifier == Some("Name")
        && focused != Some(true)
}

/// What a write to a file-name element turns into.
enum NameTarget {
    /// A Finder item at this path: rename it through Finder.
    FinderItem(std::path::PathBuf),
    /// Refuse with this route.
    Refuse(&'static str),
}

/// A rename read back from disk: the new name is listed, the old one gone.
fn rename_record() -> cua_driver_core::action_record::ActionExecutionRecord {
    use cua_driver_core::action_record::{
        ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
        EvidenceKind, RequestedDelivery,
    };
    ActionExecutionRecord::builder(
        ActionEffect::Confirmed,
        ActionTransport::MacosAppleEvent,
        RequestedDelivery::Background,
    )
    .actual_delivery(ActualDelivery::Background)
    .evidence(ActionEvidence {
        kind: EvidenceKind::ValueReadback,
        detail: "The folder on disk lists the new name and not the old one".into(),
    })
    .build()
    .expect("rename record is valid")
}

fn finder_rename_result(
    pid: i32,
    window_id: u32,
    path: &std::path::Path,
    new_name: &str,
    renamed: Result<
        Result<std::path::PathBuf, crate::apple_events::ScriptRefusal>,
        tokio::task::JoinError,
    >,
) -> ToolResult {
    let old_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match renamed {
        Ok(Ok(new_path)) => ToolResult::text(format!(
            "✅ Renamed \"{old_name}\" to \"{new_name}\" through Finder's scripting interface, \
             with Finder left in the background (not activated). Verified on disk: {} exists \
             and the old name is gone. Take a fresh get_window_state before addressing the \
             item again.",
            new_path.display()
        ))
        .with_structured(serde_json::json!({
            "path": "apple_events",
            "verified": true,
            "effect": "confirmed",
            "renamed_from": path.display().to_string(),
            "renamed_to": new_path.display().to_string(),
        }))
        .with_action_record(rename_record()),
        Ok(Err(refusal)) => ToolResult::error(format!(
            "set_value refused ({FILE_NAME_NEEDS_RENAME}): renaming \"{old_name}\" through \
             Finder did not complete ({}: {}). The file keeps its name; nothing else was sent.",
            refusal.code, refusal.reason
        ))
        .with_structured(serde_json::json!({
            "code": FILE_NAME_NEEDS_RENAME,
            "effect": "refused",
            "path": "apple_events",
            "pid": pid,
            "window_id": window_id,
            "reason": refusal.reason,
            "cause": refusal.code,
        })),
        Err(error) => ToolResult::error(format!("set_value rename task failed: {error}")),
    }
}

const LIST_RENAME_ROUTE: &str = "This is a file's name as the list shows it. Writing its AXValue \
    changes only what the list shows, never the file, so nothing was written. To rename the \
    file: click this element to select the item, then with Finder frontmost send press_key \
    return, hotkey cmd+a (Finder selects the name without its extension), type_text the full \
    new name, and press_key return, each with scope:\"desktop\". Then check the new name in a \
    fresh get_window_state.";

const GET_INFO_RENAME_ROUTE: &str = "This is the Name & Extension field of Finder's Get Info \
    window. Writing its AXValue changes only what the field shows, never the file, so nothing \
    was written. To rename the file in the background, call set_value with the new name on the \
    file's name cell in a Finder window that lists it. Otherwise take a fresh get_window_state \
    of this window and click the field's centre in its screenshot pixels (pass its capture_id), then hotkey cmd+a, type_text \
    the full new name with its extension, and press_key return, each with \
    delivery_mode:\"foreground\" on this window. A changed extension makes Finder ask for \
    confirmation in a dialog first. Then check the new name in a fresh listing of the folder; \
    this window's title changes with it.";

fn file_name_needs_rename(pid: i32, window_id: u32, reason: &str) -> ToolResult {
    ToolResult::error(format!(
        "set_value refused ({FILE_NAME_NEEDS_RENAME}): {reason}"
    ))
    .with_structured(serde_json::json!({
        "code": FILE_NAME_NEEDS_RENAME,
        "effect": "refused",
        "path": "ax",
        "pid": pid,
        "window_id": window_id,
        "reason": reason,
    }))
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
    /// The read-back still showed the old value after a short settle: the app
    /// took the accessibility write but did not apply it.
    not_applied: bool,
    /// The document's edited mark after the write; `None` outside a
    /// document or when the value did not move.
    document_edited: Option<bool>,
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
                not_applied: false,
                document_edited: None,
            }
        })
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
            let mut after = unsafe { copy_string_attr(element, "AXValue") };
            let (mut verified, mut changed) = classify_write(
                before.as_deref(),
                after.as_deref(),
                value,
                numeric_target.is_some(),
            );
            // Some apps apply the write a moment later; give it a short
            // settle before calling it not applied.
            let secure = unsafe { copy_string_attr(element, "AXSubrole") }.as_deref()
                == Some("AXSecureTextField");
            for _ in 0..NOT_APPLIED_REREADS {
                if secure || !write_not_applied(verified, changed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                after = unsafe { copy_string_attr(element, "AXValue") };
                (verified, changed) = classify_write(
                    before.as_deref(),
                    after.as_deref(),
                    value,
                    numeric_target.is_some(),
                );
            }
            let suffix = match (verified, changed) {
                (Some(true), Some(false)) => " Value already matched; write was idempotent.",
                (Some(true), _) => "",
                (Some(false), _) => " Read-back did not confirm the value; verify via screenshot.",
                (None, _) => " Value is not readable through AX; could not confirm.",
            };
            // An AX write bypasses NSTextView's edit model, so a document is
            // not marked edited and would never save the new text (DS-07).
            let document = if changed == Some(true) {
                unsafe { crate::document_edit::ensure_marked_after_ax_write(pid, element, true) }
            } else {
                crate::document_edit::DocumentEdited::NotADocument
            };
            Ok(SetValueOutcome {
                detail: format!(
                    "✅ Set AXValue on [{element_index}] {role}.{suffix}{}",
                    document.note()
                ),
                verified,
                changed,
                // A secure text field reads back empty or masked, so its
                // read-back proves nothing either way.
                not_applied: !secure && write_not_applied(verified, changed),
                document_edited: document.structured(),
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
                    not_applied: false,
                    document_edited: None,
                })
            } else {
                anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
            }
        } else {
            anyhow::bail!("AXUIElementSetAttributeValue(AXValue) failed with error {err}")
        }
    }
}

/// Re-reads (100 ms apart) before an unchanged value counts as not applied.
const NOT_APPLIED_REREADS: usize = 3;

/// The read-back did not match and the value did not move: the app accepted
/// the accessibility write (LibreOffice Calc cells do) but did not apply it.
fn write_not_applied(verified: Option<bool>, changed: Option<bool>) -> bool {
    verified == Some(false) && changed == Some(false)
}

/// The refusal for a write the app did not apply. Nothing changed, so a
/// script must stop here instead of carrying on as if it had.
fn not_applied_result(element_index: usize, vcl: bool) -> ToolResult {
    let route = if vcl {
        "LibreOffice ignores accessibility value writes and drops background pointer \
         input, so its cells cannot be edited in the background. Editing them needs the \
         window in front: bring_to_front if the user allows it, then type_text into the \
         cell. Otherwise stop and report that the edit is blocked."
    } else {
        "Try the control's own input route instead: click it, then type_text, or use a \
         keyboard shortcut. If that needs the window in front, bring_to_front only if the \
         user allows it."
    };
    ToolResult::error(format!(
        "set_value did not take: element [{element_index}] still shows its old value after \
         the write. The app accepted the accessibility write but did not apply it, so nothing \
         changed. {route}"
    ))
    .with_structured(serde_json::json!({
        "code": "write_not_applied",
        "path": "ax",
        "verified": false,
        "effect": "not_applied",
    }))
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

/// How long to wait for a popup's menu to publish its items after AXPress.
const POPUP_OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(2500);
/// Consecutive unchanged polls that mean the menu has finished filling in.
const POPUP_STABLE_POLLS: u32 = 3;
/// How long a dismissed menu may take to close before Escape is sent.
const POPUP_CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);
const POPUP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
/// AX messaging timeout while opening the menu. The target app enters its
/// menu-tracking loop inside the AXPress, so the call would otherwise block
/// for the full default timeout (~1.5 s) with the menu already open.
const POPUP_PRESS_TIMEOUT_SECONDS: f32 = 0.5;

/// One selectable entry of a popup: the retained AX element plus the title
/// and value it reports.
struct PopupOption {
    element: AXUIElementRef,
    title: String,
    value: String,
}

/// Release every retained option.
fn release_options(options: &[PopupOption]) {
    for option in options {
        unsafe { CFRelease(option.element as _) };
    }
}

/// The popup's options as AX exposes them: its direct children, or, when a
/// child is an `AXMenu` (AppKit `NSPopUpButton`, Chromium `<select>`), that
/// menu's `AXMenuItem` children. Menus without items yield nothing, so an
/// unopened popup reads as empty instead of as one untitled option.
fn popup_options(popup: AXUIElementRef) -> Vec<PopupOption> {
    let mut options = Vec::new();
    for child in unsafe { copy_children(popup) } {
        let role = unsafe { copy_string_attr(child, "AXRole") }.unwrap_or_default();
        if role == "AXMenu" {
            for item in unsafe { copy_children(child) } {
                options.push(describe_option(item));
            }
            unsafe { CFRelease(child as _) };
        } else {
            options.push(describe_option(child));
        }
    }
    options.retain(|option| !(option.title.is_empty() && option.value.is_empty()));
    options
}

fn describe_option(element: AXUIElementRef) -> PopupOption {
    PopupOption {
        element,
        title: unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default(),
        value: unsafe { copy_string_attr(element, "AXValue") }.unwrap_or_default(),
    }
}

/// Index of the option whose title or value equals `value`, ignoring case and
/// surrounding whitespace.
fn matching_option(options: &[(String, String)], value: &str) -> Option<usize> {
    let wanted = value.trim().to_lowercase();
    options.iter().position(|(title, option_value)| {
        title.trim().to_lowercase() == wanted || option_value.trim().to_lowercase() == wanted
    })
}

fn option_pairs(options: &[PopupOption]) -> Vec<(String, String)> {
    options
        .iter()
        .map(|option| (option.title.clone(), option.value.clone()))
        .collect()
}

fn describe_available(options: &[PopupOption]) -> String {
    options
        .iter()
        .map(|option| {
            let label = if option.title.is_empty() {
                &option.value
            } else {
                &option.title
            };
            format!("\"{label}\"")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether the popup's menu is open right now: an `AXMenu` child that already
/// lists items. A closed AppKit popup has an empty `AXMenu` (or none), and a
/// closed Chromium popup lists only its selected item directly.
fn popup_menu_is_open(popup: AXUIElementRef) -> bool {
    let mut open = false;
    for child in unsafe { copy_children(popup) } {
        if unsafe { copy_string_attr(child, "AXRole") }.as_deref() == Some("AXMenu") {
            let items = unsafe { copy_children(child) };
            open |= !items.is_empty();
            for item in items {
                unsafe { CFRelease(item as _) };
            }
        }
        unsafe { CFRelease(child as _) };
    }
    open
}

/// Close a menu this call opened, without choosing anything. `AXCancel` is
/// enough for AppKit; Chromium's menu ignores it, so fall back to Escape sent
/// to the app (the open menu is what receives it).
fn dismiss_popup_menu(popup: AXUIElementRef, pid: i32) {
    for child in unsafe { copy_children(popup) } {
        let role = unsafe { copy_string_attr(child, "AXRole") }.unwrap_or_default();
        if role == "AXMenu" {
            let _ = unsafe { perform_action(child, "AXCancel") };
        }
        unsafe { CFRelease(child as _) };
    }
    let deadline = std::time::Instant::now() + POPUP_CLOSE_TIMEOUT;
    while popup_menu_is_open(popup) {
        if std::time::Instant::now() >= deadline {
            let _ = crate::input::keyboard::press_key_no_auth(pid, "escape", &[]);
            std::thread::sleep(POPUP_CLOSE_TIMEOUT);
            return;
        }
        std::thread::sleep(POPUP_POLL_INTERVAL);
    }
}

fn select_popup_option(
    element: AXUIElementRef,
    element_index: usize,
    pid: i32,
    value: &str,
    element_title: &str,
) -> anyhow::Result<String> {
    let mut options = popup_options(element);
    let mut opened_menu = false;
    let mut press_thread = None;

    if matching_option(&option_pairs(&options), value).is_none() {
        // Safari/WebKit: no AX children while the popup is closed. Set the
        // <select> through the DOM instead of opening a menu.
        if options.is_empty() {
            let app_name = crate::apps::get_app_name_for_pid(pid).unwrap_or_default();
            if app_name == "Safari" {
                return set_select_via_js(element_index, element_title, value);
            }
        }
        // AppKit NSPopUpButton and Chromium <select> publish their items only
        // while the menu is open (a closed Chromium popup lists just the
        // selected one). Open it, wait for the full list, press the match,
        // and close the menu again whatever happens. The menu's tracking
        // steals key focus; in the background the key-focus guard armed by
        // the caller hands it straight back to the user's front window.
        //
        // The AXPress returns only once the app's menu tracking lets it (or
        // the messaging timeout fires), so run it on its own thread and read
        // the items as soon as they appear: the menu, and with it the key
        // focus it holds, stays open for the shortest time.
        let already_open = popup_menu_is_open(element);
        let closed_titles = if already_open {
            Vec::new()
        } else {
            option_pairs(&options)
        };
        release_options(&options);
        opened_menu = true;
        // A menu left open by an earlier click is already showing its items;
        // pressing again would close it.
        if !already_open {
            let popup_address = element as usize;
            press_thread = Some(std::thread::spawn(move || unsafe {
                let popup = popup_address as AXUIElementRef;
                crate::ax::bindings::AXUIElementSetMessagingTimeout(
                    popup,
                    POPUP_PRESS_TIMEOUT_SECONDS,
                );
                let _ = perform_action(popup, "AXPress");
                crate::ax::bindings::AXUIElementSetMessagingTimeout(popup, 0.0);
            }));
        }
        let deadline = std::time::Instant::now() + POPUP_OPEN_TIMEOUT;
        let mut last_seen: Vec<(String, String)> = Vec::new();
        let mut stable_polls = 0;
        loop {
            options = popup_options(element);
            let seen = option_pairs(&options);
            if matching_option(&seen, value).is_some() {
                break;
            }
            // The menu is open once the list differs from the closed one; it
            // is complete once the list stops changing.
            if !seen.is_empty() && seen != closed_titles {
                stable_polls = if seen == last_seen {
                    stable_polls + 1
                } else {
                    0
                };
                if stable_polls >= POPUP_STABLE_POLLS {
                    break;
                }
            }
            last_seen = seen;
            if std::time::Instant::now() >= deadline {
                break;
            }
            release_options(&options);
            std::thread::sleep(POPUP_POLL_INTERVAL);
        }
        if options.is_empty() {
            dismiss_popup_menu(element, pid);
            if let Some(press) = press_thread.take() {
                let _ = press.join();
            }
            anyhow::bail!(
                "AXPopUpButton [{element_index}] \"{element_title}\" exposed no options even \
                 after its menu was opened, so nothing was selected. Click the popup, then \
                 choose the option with press_key (down, return) or a pixel click on the item."
            )
        }
    }

    let result = match matching_option(&option_pairs(&options), value) {
        Some(i) => {
            let option = &options[i];
            let err = unsafe { perform_action(option.element, "AXPress") };
            if err == kAXErrorSuccess {
                let label = if option.title.is_empty() {
                    &option.value
                } else {
                    &option.title
                };
                Ok(format!(
                    "✅ Selected '{label}' in AXPopUpButton [{element_index}] \
                     \"{element_title}\" via AX menu item AXPress{}.",
                    if opened_menu {
                        " (the popup's menu was opened for the selection and closed again)"
                    } else {
                        ""
                    }
                ))
            } else {
                if opened_menu {
                    dismiss_popup_menu(element, pid);
                }
                Err(anyhow::anyhow!(
                    "AXPress on menu item failed with error {err}"
                ))
            }
        }
        None => {
            if opened_menu {
                dismiss_popup_menu(element, pid);
            }
            Err(anyhow::anyhow!(
                "No option matching '{value}' in AXPopUpButton [{element_index}] \
                 \"{element_title}\". Available: [{}]",
                describe_available(&options)
            ))
        }
    };
    // The opening AXPress returns once the menu is closed or its timeout
    // fires; wait for it so no AX call outlives this tool call.
    if let Some(press) = press_thread {
        let _ = press.join();
    }
    release_options(&options);
    result
}

#[cfg(test)]
mod popup_option_tests {
    use super::matching_option;

    fn pairs(titles: &[&str]) -> Vec<(String, String)> {
        titles
            .iter()
            .map(|title| ((*title).to_owned(), String::new()))
            .collect()
    }

    #[test]
    fn matches_title_ignoring_case_and_whitespace() {
        let options = pairs(&["Open", "Duplicate", "Closed"]);
        assert_eq!(matching_option(&options, "duplicate"), Some(1));
        assert_eq!(matching_option(&options, "  Closed "), Some(2));
        assert_eq!(matching_option(&options, "Pending"), None);
    }

    #[test]
    fn matches_value_when_title_differs() {
        let options = vec![("Duplicate of".to_owned(), "dup".to_owned())];
        assert_eq!(matching_option(&options, "DUP"), Some(0));
    }
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
        apply_surface_trust, apply_verification_label, classify_write, file_name_needs_rename,
        is_file_name_cell, is_get_info_name_field, not_applied_result, write_not_applied,
        SetValueOutcome, GET_INFO_RENAME_ROUTE, LIST_RENAME_ROUTE,
    };

    #[test]
    fn a_listed_file_name_is_a_file_name_cell() {
        let url = Some("file:///Users/me/lab/charlie.bin");
        assert!(is_file_name_cell(Some("charlie.bin"), url, Some(false)));
        assert!(is_file_name_cell(Some("charlie.bin"), url, None));
    }

    #[test]
    fn a_finder_rename_publishes_a_confirmed_system_api_record() {
        let record = super::rename_record();
        assert!(record.validate().is_ok());
        let path = std::path::Path::new("/tmp/a.txt");
        let ok = super::finder_rename_result(7, 42, path, "b.txt", Ok(Ok("/tmp/b.txt".into())));
        assert_eq!(ok.is_error, None);
        assert!(ok.action_record.is_some());
        let refused = super::finder_rename_result(
            7,
            42,
            path,
            "b.txt",
            Ok(Err(crate::apple_events::ScriptRefusal {
                code: "automation_denied",
                reason: "off".into(),
            })),
        );
        assert_eq!(refused.is_error, Some(true));
        let data = refused.structured_content.unwrap();
        assert_eq!(data["code"], "file_name_needs_rename");
        assert_eq!(data["cause"], "automation_denied");
    }

    #[test]
    fn rename_editor_and_ordinary_fields_stay_writable() {
        let url = Some("file:///Users/me/lab/charlie.bin");
        // Finder's inline rename editor is focused while editing.
        assert!(!is_file_name_cell(Some("charlie.bin"), url, Some(true)));
        // A plain text field names no file.
        assert!(!is_file_name_cell(None, None, Some(false)));
        assert!(!is_file_name_cell(Some(""), url, Some(false)));
        assert!(!is_file_name_cell(Some("charlie.bin"), None, Some(false)));
        assert!(!is_file_name_cell(
            Some("page"),
            Some("https://example.com/page"),
            None
        ));
    }

    #[test]
    fn get_info_name_field_is_refused_and_its_neighbours_are_not() {
        const FINDER: Option<&str> = Some("com.apple.finder");
        // (bundle id, role, AXIdentifier, AXFocused, refused). Identifiers are
        // the ones Finder reported on macOS 26.4.
        let cases = [
            (FINDER, "AXTextField", Some("Name"), Some(false), true),
            (FINDER, "AXTextField", Some("Name"), None, true),
            // A real click starts an edit session; Return then commits an
            // AXValue write, so the focused field stays writable.
            (FINDER, "AXTextField", Some("Name"), Some(true), false),
            // The "Name & Extension" disclosure triangle shares the identifier.
            (FINDER, "AXDisclosureTriangle", Some("Name"), None, false),
            // Tags field, list inline rename editor, list name cell.
            (FINDER, "AXTextField", Some("_NS:34"), None, false),
            (
                FINDER,
                "AXTextField",
                Some("ShrinkToFit Text Field"),
                Some(true),
                false,
            ),
            (FINDER, "AXTextField", None, Some(false), false),
            (FINDER, "AXTextArea", Some("Comments"), None, false),
            // The same field shape in another app.
            (
                Some("com.example.notes"),
                "AXTextField",
                Some("Name"),
                None,
                false,
            ),
            (None, "AXTextField", Some("Name"), None, false),
        ];
        for (bundle, role, identifier, focused, refused) in cases {
            assert_eq!(
                is_get_info_name_field(bundle, Some(role), identifier, focused),
                refused,
                "{bundle:?} {role} {identifier:?} {focused:?}"
            );
        }
    }

    #[test]
    fn get_info_refusal_names_the_foreground_route() {
        let result = file_name_needs_rename(7, 42, GET_INFO_RENAME_ROUTE);
        let data = result.structured_content.unwrap();
        assert_eq!(data["code"], "file_name_needs_rename");
        let reason = data["reason"].as_str().unwrap();
        for needed in [
            "Get Info",
            "nothing was written",
            "screenshot pixels",
            "capture_id",
            "cmd+a",
            "type_text",
            "return",
            "delivery_mode:\"foreground\"",
        ] {
            assert!(reason.contains(needed), "missing {needed:?}: {reason}");
        }
    }

    #[test]
    fn an_unchanged_read_back_is_not_applied_but_an_idempotent_one_is_fine() {
        // Read-back still shows the old value: not applied.
        let (verified, changed) =
            classify_write(Some("pending"), Some("pending"), "matched", false);
        assert!(write_not_applied(verified, changed));
        // Already held the requested value: idempotent, applied.
        let (verified, changed) =
            classify_write(Some("matched"), Some("matched"), "matched", false);
        assert!(!write_not_applied(verified, changed));
        // The app reformatted the value: it moved, so it is not "not applied".
        let (verified, changed) = classify_write(Some(""), Some("1,248.60"), "1248.6", false);
        assert!(!write_not_applied(verified, changed));
        // Unreadable before or after: cannot tell.
        let (verified, changed) = classify_write(None, Some("x"), "y", false);
        assert!(!write_not_applied(verified, changed));
        let refusal = not_applied_result(4, true);
        assert_eq!(refusal.is_error, Some(true));
        let data = refusal.structured_content.unwrap();
        assert_eq!(data["code"], "write_not_applied");
        assert_eq!(data["effect"], "not_applied");
    }

    #[test]
    fn file_name_refusal_names_the_rename_route() {
        let result = file_name_needs_rename(7, 42, LIST_RENAME_ROUTE);
        assert_eq!(result.is_error, Some(true));
        let data = result.structured_content.unwrap();
        assert_eq!(data["code"], "file_name_needs_rename");
        assert_eq!(data["effect"], "refused");
        assert_eq!(
            (data["pid"].as_i64(), data["window_id"].as_u64()),
            (Some(7), Some(42))
        );
        let reason = data["reason"].as_str().unwrap();
        for needed in [
            "never the file",
            "nothing was written",
            "return",
            "cmd+a",
            "type_text",
            "desktop",
        ] {
            assert!(reason.contains(needed), "missing {needed:?}: {reason}");
        }
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
        let mut outcome = SetValueOutcome {
            detail: "Set value.".to_owned(),
            verified: Some(true),
            changed: Some(true),
            not_applied: false,
            document_edited: None,
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
            not_applied: false,
            document_edited: None,
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
            not_applied: false,
            document_edited: None,
        };
        apply_verification_label(&mut outcome);
        assert_eq!(
            outcome.detail,
            "📨 Sent (unverified) AXValue on [4] AXTextField."
        );
    }
}
