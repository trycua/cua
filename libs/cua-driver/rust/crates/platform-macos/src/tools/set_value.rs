// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! set_value tool — matches the Swift reference in SetValueTool.swift.
//!
//! Two modes, determined by the element's AXRole:
//!
//! * **AXPopUpButton**: Find the option (a child, or an `AXMenuItem` under the
//!   popup's `AXMenu`) whose AXTitle or AXValue matches `value`
//!   (case-insensitive) and AXPress it.  AppKit and Chromium popups publish
//!   their items only while the menu is open, so the menu is opened for the
//!   selection and closed again.  Safari `<select>` elements are set through
//!   `osascript do JavaScript` instead.  In the background no menu is opened
//!   while another app's window covers the popup's window: the menu would
//!   bring the window above the user's, so the call is refused.
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
        description: "Set an element's value by `element_token`. A popup button / select dropdown: selects the menu item whose title or value matches `value` (case-insensitive). Safari `<select>` is set through the DOM; a background Chromium `<select>` is chosen by typing the option's label into the closed control. AppKit popups list items only while open, so the menu is opened, the item pressed and the menu closed again. In the background a menu is never opened while another app's window covers the popup's window, because opening it brings the window above the user's. There the item is chosen by typing its title into the closed popup (AppKit type-select, `popup_route:\"type_select\"`, read back). AppKit applies type-select only in an app it counts as active, so an app that does not is made to count itself active for the length of the typing by an in-app activation (the user's app stays frontmost and keeps the keyboard; nothing is raised). Otherwise the call is refused with `background_unavailable` (reason `popup_menu_would_raise_window`, `type_select` says why) and nothing changes. If the app raises its window or activates itself anyway, the driver puts the user's window back and reports it in `user_disturbance`. An unknown value lists the options. Any other element: writes AXValue (sliders, steppers, date pickers, native text fields).\n\
            \n\
            A Finder file name cell (list or icon view, not being edited) is renamed through Finder's scripting interface with Finder left in the background, and the new name is read back from disk; the value is the full new name with its extension. macOS may prompt for Automation consent; with serve --no-automation-prompts, ungranted consent refuses with automation_consent_required. Finder's Get Info `Name` field is refused with `file_name_needs_rename`. For free-form text in web inputs use `type_text_chars`: WebKit ignores AXValue writes.".into(),
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
        if let Some(refusal) =
            super::refuse_if_disabled(pid, window_id, &element_guard, "set_value", false).await
        {
            return refusal;
        }

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
        let undone_before = crate::key_focus_guard::undone();
        let value_for_refusal = value.clone();

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
                    )
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async().await;
        let disturbance = disturbance_note(undone_before, crate::key_focus_guard::undone());

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
                if let Some(route) = outcome.popup_route {
                    structured["popup_route"] = serde_json::json!(route);
                }
                if let Some((note, report)) = disturbance {
                    msg.push_str(note);
                    structured["user_disturbance"] = report;
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
            Ok(Err(e)) => match e.downcast_ref::<PopupMenuWouldRaise>() {
                Some(refused) => {
                    let mut result = super::menu_raise::refusal(
                        pid,
                        window_id,
                        &super::menu_raise::describe(
                            Some(refused.element_index),
                            "AXPopUpButton",
                            &refused.title,
                        ),
                        super::menu_raise::Menu::Popup,
                        &format!("choosing \"{value_for_refusal}\""),
                        Some(&refused.current),
                    );
                    if let (Some(code), Some(data)) =
                        (refused.type_select, result.structured_content.as_mut())
                    {
                        data["type_select"] = serde_json::json!(code);
                    }
                    result
                }
                None => ToolResult::error(format!("set_value failed: {e}")),
            },
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// What the caller is told when the app raised its window over the user's, or
/// activated itself, during the call and the driver put the user's window
/// back. The counters are daemon-wide, so another action finishing at the
/// same moment can be counted too; the report says "during this call".
fn disturbance_note(
    before: crate::key_focus_guard::Undone,
    after: crate::key_focus_guard::Undone,
) -> Option<(&'static str, serde_json::Value)> {
    let raised = after.raises > before.raises;
    let activated = after.activations > before.activations;
    if !raised && !activated {
        return None;
    }
    let note = match (raised, activated) {
        (true, false) => {
            " Note: the app brought its window above the user's window during this call; \
             Cua Driver put the user's window back on top."
        }
        (false, true) => {
            " Note: the app made itself the active app during this call; Cua Driver made the \
             user's app active again."
        }
        _ => {
            " Note: the app came to the front during this call; Cua Driver put the user's \
             window back on top and made their app active again."
        }
    };
    Some((
        note,
        serde_json::json!({
            "window_raised": raised,
            "app_activated": activated,
            "restored": true,
        }),
    ))
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
        Ok(Err(refusal)) => {
            let code = if refusal.code == "automation_consent_required" {
                refusal.code
            } else {
                FILE_NAME_NEEDS_RENAME
            };
            ToolResult::error(format!(
                "set_value refused ({code}): renaming \"{old_name}\" through \
             Finder did not complete ({}: {}). The file keeps its name; nothing else was sent.",
                refusal.code, refusal.reason
            ))
            .with_structured(serde_json::json!({
                "code": code,
                "effect": "refused",
                "path": "apple_events",
                "pid": pid,
                "window_id": window_id,
                "reason": refusal.reason,
                "cause": refusal.code,
            }))
        }
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
    /// How a popup item was chosen when it was not by pressing a menu item
    /// (`"type_select"`).
    popup_route: Option<&'static str>,
}

/// A popup item chosen by `select_popup_option`.
struct PopupChoice {
    detail: String,
    /// `Some(true)` when the popup was read back showing the item.
    verified: Option<bool>,
    changed: Option<bool>,
    route: Option<&'static str>,
}

impl PopupChoice {
    /// Chosen through a route that reads nothing back.
    fn sent(detail: String) -> Self {
        Self {
            detail,
            verified: None,
            changed: None,
            route: None,
        }
    }
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
    window_id: u32,
    value: &str,
) -> anyhow::Result<SetValueOutcome> {
    let element = element_ptr as AXUIElementRef;

    let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();

    if role == "AXPopUpButton" {
        let element_title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
        // Menu-item selection, not an AXValue write — no read-back to report.
        select_popup_option(
            element,
            element_index,
            pid,
            window_id,
            value,
            &element_title,
        )
        .map(|choice| SetValueOutcome {
            detail: choice.detail,
            verified: choice.verified,
            changed: choice.changed,
            not_applied: false,
            document_edited: None,
            popup_route: choice.route,
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
                popup_route: None,
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
                    popup_route: None,
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
pub(super) fn popup_menu_is_open(popup: AXUIElementRef) -> bool {
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

/// Whether a popup belongs to a Chromium renderer (Chrome-family browsers and
/// Electron apps), whose `<select>` menu is a native popup.
fn is_chromium_popup_owner(pid: i32) -> bool {
    let bundle_id = crate::apps::bundle_id_for_pid(pid).unwrap_or_default();
    super::type_text::is_chromium_identity(&bundle_id)
        || crate::browser::electron_js::ElectronJs::is_electron(pid)
}

/// Whether pressing `element` would open a Chromium or Electron `<select>`
/// menu: an `AXPopUpButton` inside web content of a Chromium renderer. The
/// browser's own toolbar popups have no web-area ancestor and do not match.
pub(super) fn is_chromium_web_select(pid: i32, window_id: u32, element: AXUIElementRef) -> bool {
    unsafe { copy_string_attr(element, "AXRole") }.as_deref() == Some("AXPopUpButton")
        && is_chromium_popup_owner(pid)
        && super::type_text::target_in_web_area(
            pid,
            Some((element as usize, None)),
            Some(window_id),
        )
}

/// Refusal for a background press that would open a Chromium `<select>` menu
/// (CUA-1795). Opening it orders the browser window above the user's window
/// for at least a frame, which undoing afterwards cannot hide; `set_value`
/// selects through the page's type-ahead and never opens the menu.
pub(super) fn chromium_select_open_refusal(
    pid: i32,
    window_id: u32,
    element_index: Option<usize>,
    element: AXUIElementRef,
) -> cua_driver_core::protocol::ToolResult {
    let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
    let current = unsafe { copy_string_attr(element, "AXValue") }.unwrap_or_default();
    let target = match element_index {
        Some(index) => format!("[{index}] AXPopUpButton \"{title}\""),
        None => format!("the AXPopUpButton \"{title}\" at that point"),
    };
    cua_driver_core::protocol::ToolResult::error(format!(
        "Not opened: {target} is a web dropdown (<select>). In the background, opening its \
         menu brings the browser window above the user's window, so nothing was sent. Its \
         current value is \"{current}\". Choose an option in one call with \
         set_value(element_token, \"<option text>\"): it types the option's text into the \
         closed control and the menu never opens. Open the menu with \
         delivery_mode:\"foreground\" only if the user agreed to the browser coming forward."
    ))
    .with_structured(serde_json::json!({
        "code": "select_menu_would_raise_window",
        "effect": "refused",
        "pid": pid,
        "window_id": window_id,
        "current_value": current,
        "escalation": {
            "recommended": "set_value",
            "reason": "a background <select> is chosen with set_value; opening its menu would raise the browser window",
        },
    }))
}

/// Why a background popup was left unchanged: choosing in it would open its
/// menu, and the menu raises its window over the user's window.
#[derive(Debug)]
struct PopupMenuWouldRaise {
    element_index: usize,
    title: String,
    current: String,
    /// Why choosing the item by typing its title did not work, when tried.
    type_select: Option<&'static str>,
}

impl std::fmt::Display for PopupMenuWouldRaise {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "popup menu would raise its window")
    }
}

impl std::error::Error for PopupMenuWouldRaise {}

/// Refusal for a background press or show-menu on `element` when it would
/// open a popup menu that raises the window: a Chromium `<select>` always
/// (its window stays raised), any other popup button while another app's
/// window covers its window.
pub(super) fn background_popup_open_refusal(
    pid: i32,
    window_id: u32,
    element_index: Option<usize>,
    element: AXUIElementRef,
) -> Option<cua_driver_core::protocol::ToolResult> {
    if unsafe { copy_string_attr(element, "AXRole") }.as_deref() != Some("AXPopUpButton") {
        return None;
    }
    if is_chromium_web_select(pid, window_id, element) {
        return Some(chromium_select_open_refusal(
            pid,
            window_id,
            element_index,
            element,
        ));
    }
    if popup_menu_is_open(element) || !crate::key_focus_guard::menu_open_would_raise(pid, window_id)
    {
        return None;
    }
    let current = unsafe { copy_string_attr(element, "AXValue") }.unwrap_or_default();
    Some(super::menu_raise::refusal(
        pid,
        window_id,
        &super::menu_raise::describe(
            element_index,
            "AXPopUpButton",
            &super::menu_raise::label(element),
        ),
        super::menu_raise::Menu::PopupOpen,
        "clicking",
        Some(&current),
    ))
}

/// What a popup shows: its `AXValue` (the selected item's title), or its
/// `AXTitle` when it reports no value.
fn popup_shown_value(popup: AXUIElementRef) -> String {
    match unsafe { copy_string_attr(popup, "AXValue") } {
        Some(value) if !value.is_empty() => value,
        _ => unsafe { copy_string_attr(popup, "AXTitle") }.unwrap_or_default(),
    }
}

/// Whether a popup showing `shown` shows the item `wanted` (case and
/// surrounding whitespace ignored, as `matching_option` compares).
fn shows_item(shown: &str, wanted: &str) -> bool {
    shown.trim().to_lowercase() == wanted.trim().to_lowercase()
}

/// AppKit applies a type-select once typing pauses for its type-select
/// interval (about one second); wait this long after the last key.
const TYPE_SELECT_COMMIT: std::time::Duration = std::time::Duration::from_millis(1800);
const TYPE_SELECT_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// The keys that type-select `title` in a closed popup: its characters up to
/// the first whitespace or control character. A space (like Return or an
/// arrow key) opens the menu of a focused popup, and the words before it
/// usually pick the item already. Empty when the title starts with one.
fn type_select_keys(title: &str) -> Vec<char> {
    title
        .trim()
        .chars()
        .take_while(|ch| !ch.is_whitespace() && !ch.is_control())
        .collect()
}

/// Why typing an item's title into a closed popup did not choose it.
#[derive(Debug, PartialEq, Eq)]
enum TypeSelectMiss {
    /// The title begins with a key that would open the menu.
    NoKeys,
    /// AppKit does not count the app as active, so it ignores type-select,
    /// and the in-app activation records did not change that; nothing was
    /// typed.
    AppNotActive,
    /// Counting itself active put another of the app's windows on screen (a
    /// panel AppKit shows only in an active app: it went away again when the
    /// activation was undone), so the activation was undone at once; nothing
    /// was typed. A window the app showed by itself meanwhile (it stays on
    /// screen while the app is inactive) is not this.
    ActivationShowedWindows,
    /// The popup could not take keyboard focus, or the window could not be
    /// made the app's key window; nothing was typed.
    NotFocused,
    /// Keys were typed but the popup does not show the item. `shown` is what
    /// it shows afterwards.
    NotChosen { shown: String },
}

impl TypeSelectMiss {
    /// The `type_select` field of the refusal.
    fn code(&self) -> &'static str {
        match self {
            Self::NoKeys => "title_starts_with_a_menu_key",
            Self::AppNotActive => "app_not_active",
            Self::ActivationShowedWindows => "activation_showed_windows",
            Self::NotFocused => "popup_not_focused",
            Self::NotChosen { .. } => "item_not_chosen",
        }
    }
}

/// Type `keys` with `press` and wait, reading the popup with `shown`, until it
/// shows `wanted` or `commit` has passed since the last key.
fn run_type_select(
    wanted: &str,
    keys: &[char],
    mut press: impl FnMut(char) -> anyhow::Result<()>,
    mut shown: impl FnMut() -> String,
    commit: std::time::Duration,
    poll: std::time::Duration,
) -> Result<String, TypeSelectMiss> {
    if keys.is_empty() {
        return Err(TypeSelectMiss::NoKeys);
    }
    for &key in keys {
        if press(key).is_err() {
            return Err(TypeSelectMiss::NotChosen { shown: shown() });
        }
    }
    let deadline = std::time::Instant::now() + commit;
    loop {
        let now_shown = shown();
        if shows_item(&now_shown, wanted) {
            return Ok(now_shown);
        }
        if std::time::Instant::now() >= deadline {
            return Err(TypeSelectMiss::NotChosen { shown: now_shown });
        }
        std::thread::sleep(poll);
    }
}

/// How long an app may take to handle the in-app activation records before it
/// reports itself active, or the deactivation record before it reports itself
/// inactive again.
const ACTIVATION_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
const ACTIVATION_POLL: std::time::Duration = std::time::Duration::from_millis(25);
/// How long a window that appeared during the activation must stay on screen
/// after the app counts itself inactive again to be the app's own window
/// rather than a panel AppKit shows only in an active app.
const ACTIVATION_HIDE_GRACE: std::time::Duration = std::time::Duration::from_millis(100);
/// How often the activation is tried when windows the app showed by itself
/// (not because of the activation) appeared while it was being posted.
const ACTIVATION_ATTEMPTS: usize = 2;

/// The timing of [`ensure_counts_itself_active`].
#[derive(Clone, Copy)]
struct ActivationTiming {
    settle: std::time::Duration,
    poll: std::time::Duration,
    hide_grace: std::time::Duration,
}

const ACTIVATION_TIMING: ActivationTiming = ActivationTiming {
    settle: ACTIVATION_SETTLE,
    poll: ACTIVATION_POLL,
    hide_grace: ACTIVATION_HIDE_GRACE,
};

/// Have AppKit count the app as active before anything is typed. An app that
/// already does needs nothing (`Ok(None)`). Otherwise `activate` posts the
/// in-app activation and returns its guard, and `is_active` is read until it
/// is true or `timing.settle` has passed. The guard undoes the activation when
/// it is dropped, which happens here when the app never reports itself active
/// or a window it showed has to be undone.
///
/// An app that shows another window once active (a panel AppKit hides while
/// the app is inactive) would change the user's screen, so that activation is
/// undone. But a window can also appear on its own while the records are in
/// flight: an app answers an earlier action with a banner, an alert or a
/// sheet a moment later, whether it is active or not. Comparing the app's
/// on-screen windows (`on_screen`) before and after cannot tell the two
/// apart, so the activation is undone and the new windows are read again
/// once the app counts itself inactive: a window the activation showed goes
/// away with it, a window the app showed by itself stays. Only the first is
/// refused; for the second the activation is tried again with the window
/// counted as already there.
fn ensure_counts_itself_active<G>(
    mut is_active: impl FnMut() -> bool,
    mut activate: impl FnMut() -> Option<G>,
    mut on_screen: impl FnMut() -> Vec<u32>,
    timing: ActivationTiming,
) -> Result<Option<G>, TypeSelectMiss> {
    if is_active() {
        return Ok(None);
    }
    let mut before = on_screen();
    for _ in 0..ACTIVATION_ATTEMPTS {
        let Some(guard) = activate() else {
            return Err(TypeSelectMiss::AppNotActive);
        };
        let deadline = std::time::Instant::now() + timing.settle;
        while !is_active() {
            if std::time::Instant::now() >= deadline {
                drop(guard);
                return Err(TypeSelectMiss::AppNotActive);
            }
            std::thread::sleep(timing.poll);
        }
        let shown: Vec<u32> = on_screen()
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        if shown.is_empty() {
            return Ok(Some(guard));
        }
        drop(guard);
        if !shown_while_inactive(&shown, &mut is_active, &mut on_screen, timing) {
            return Err(TypeSelectMiss::ActivationShowedWindows);
        }
        before = on_screen();
    }
    Err(TypeSelectMiss::ActivationShowedWindows)
}

/// After the activation was undone: whether every window in `shown` is still
/// on screen `timing.hide_grace` after the app counts itself inactive again,
/// so the app showed it by itself. `false` as soon as one of them is gone (the
/// activation showed it), and when the app still counts itself active after
/// `timing.settle` (it cannot be told).
fn shown_while_inactive(
    shown: &[u32],
    is_active: &mut impl FnMut() -> bool,
    on_screen: &mut impl FnMut() -> Vec<u32>,
    timing: ActivationTiming,
) -> bool {
    let deadline = std::time::Instant::now() + timing.settle;
    let mut inactive_since: Option<std::time::Instant> = None;
    loop {
        let now_on_screen = on_screen();
        if shown.iter().any(|id| !now_on_screen.contains(id)) {
            return false;
        }
        if is_active() {
            inactive_since = None;
        } else if inactive_since
            .get_or_insert_with(std::time::Instant::now)
            .elapsed()
            >= timing.hide_grace
        {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(timing.poll);
    }
}

/// Whether AppKit counts the app as active (`NSApp.isActive`), which its
/// application element reports as `AXFrontmost`. AppKit applies a popup's
/// type-select only then, when its type-select interval ends. An app in the
/// background normally is not active; one whose own activation (an alert or
/// sheet answered in the background) was handed back to the user's app by
/// re-fronting that app can still count itself active while the user's app
/// is frontmost and keeps the keyboard.
fn app_counts_itself_active(pid: i32) -> bool {
    let app = unsafe { crate::ax::bindings::AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return false;
    }
    let active = unsafe { copy_bool_attr(app, "AXFrontmost") } == Some(true);
    unsafe { CFRelease(app as _) };
    active
}

/// Type one character to `pid`: its own key on the current layout when it has
/// one (type-select reads the key's characters, case-insensitively), its
/// Unicode text otherwise.
fn press_type_select_key(pid: i32, key: char) -> anyhow::Result<()> {
    let name = key.to_lowercase().to_string();
    if key.is_ascii_alphanumeric() {
        if let Ok(()) = crate::input::keyboard::press_key(pid, &name, &[]) {
            return Ok(());
        }
    }
    crate::input::keyboard::type_text(pid, &key.to_string())
}

/// Choose `value` in a closed AppKit popup by typing its title, without
/// opening the menu, so the window order does not change.
///
/// A focused, closed `NSPopUpButton` takes AppKit's type-select: typed
/// characters select the first item whose title starts with them once typing
/// pauses. The popup is focused through accessibility and the keys are posted
/// to the app with its window made the app's key window, the route `press_key`
/// with an `element_token` uses; nothing is raised. The result is read back
/// from the popup. AppKit applies type-select only while it counts the app as
/// active (it ignores the keys otherwise). An app that does not is made to
/// count itself active by AppKit's application-activated record, posted to it
/// alone, for the length of the typing: WindowServer's front process, the
/// window order and the user's key focus do not change. When even that leaves
/// the app inactive, nothing is typed and the caller refuses as before.
fn select_appkit_popup_by_type_select(
    popup: AXUIElementRef,
    element_index: usize,
    pid: i32,
    window_id: u32,
    value: &str,
) -> Result<PopupChoice, TypeSelectMiss> {
    let wanted = value.trim();
    // An AppKit popup reports no AXTitle; name it by its label.
    let element_title = super::menu_raise::label(popup);
    if shows_item(&popup_shown_value(popup), wanted) {
        return Ok(PopupChoice {
            detail: format!(
                "✅ '{wanted}' is already selected in AXPopUpButton [{element_index}] \
                 \"{element_title}\"; nothing was changed."
            ),
            verified: Some(true),
            changed: Some(false),
            route: None,
        });
    }
    let keys = type_select_keys(wanted);
    if keys.is_empty() {
        return Err(TypeSelectMiss::NoKeys);
    }
    // AppKit applies type-select only in an app it counts as active. One that
    // does not gets the application-activated record (to it alone; the user's
    // app stays frontmost and keeps the keyboard) and is handed back with the
    // deactivated record once the item is chosen or the attempt ends.
    let on_screen = || -> Vec<u32> {
        crate::windows::pid_on_screen_windows_any_layer(pid)
            .into_iter()
            .map(|window| window.window_id)
            .collect()
    };
    let activation = ensure_counts_itself_active(
        || app_counts_itself_active(pid),
        || crate::input::skylight::InAppActivation::enter(pid, window_id),
        on_screen,
        ACTIVATION_TIMING,
    )?;
    let popup_address = popup as usize;
    let previous_focus =
        unsafe { crate::ax::exact_target::focused_element_in_window(pid, window_id) };
    let restore_focus = |previous: Option<AXUIElementRef>| {
        if let Some(previous) = previous {
            if previous as usize != popup_address {
                let _ = crate::input::ax_actions::focus_element(previous as usize);
            }
            unsafe { CFRelease(previous as _) };
        }
    };
    // Keys go to the window's focused element, so the window must be the
    // app's key window (in-app records only; the app is not activated).
    let scope = match super::key_scope(pid, Some(window_id)) {
        Ok(scope) => scope,
        Err(_) => {
            restore_focus(previous_focus);
            return Err(TypeSelectMiss::NotFocused);
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    loop {
        let _ = crate::input::ax_actions::focus_element(popup_address);
        if crate::input::ax_actions::element_has_keyboard_focus(pid, popup_address) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            drop(scope);
            restore_focus(previous_focus);
            return Err(TypeSelectMiss::NotFocused);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result = run_type_select(
        wanted,
        &keys,
        |key| press_type_select_key(pid, key),
        || popup_shown_value(popup),
        TYPE_SELECT_COMMIT,
        TYPE_SELECT_POLL,
    );
    drop(scope);
    drop(activation);
    match result {
        Ok(shown) => {
            if let Some(previous) = previous_focus {
                unsafe { CFRelease(previous as _) };
            }
            Ok(PopupChoice {
                detail: format!(
                    "✅ Selected '{shown}' in AXPopUpButton [{element_index}] \
                     \"{element_title}\" by typing its title into the closed popup \
                     (type-select; the menu was not opened, so the window order did not change)."
                ),
                verified: Some(true),
                changed: Some(true),
                route: Some("type_select"),
            })
        }
        Err(miss) => {
            restore_focus(previous_focus);
            Err(miss)
        }
    }
}

/// How long the page may take to apply a type-ahead selection.
const TYPEAHEAD_SETTLE: std::time::Duration = std::time::Duration::from_millis(800);

/// Select a background Chromium `<select>` without opening its menu.
///
/// Chromium shows a `<select>` menu as a native popup. Opening it orders the
/// whole browser window above the user's window, and the window stays there
/// after the menu closes; re-activating the user's app does not reorder it
/// (Bench v2: WF-02 to WF-08, IR-05, IR-06, MA-02, CDB-G04). A focused closed
/// `<select>` takes the page's own type-ahead instead: typing an option's
/// label selects it and fires `input` and `change`, and the menu never opens.
///
/// Returns `None` when this route does not apply or did not land on `value`;
/// the caller then falls back to opening the menu.
fn select_chromium_popup_by_typeahead(
    popup: AXUIElementRef,
    element_index: usize,
    pid: i32,
    window_id: u32,
    value: &str,
    element_title: &str,
) -> Option<String> {
    let wanted = value.trim();
    if wanted.is_empty()
        || crate::apps::frontmost_pid() == Some(pid)
        || popup_menu_is_open(popup)
        || !is_chromium_popup_owner(pid)
    {
        return None;
    }
    let selected = || unsafe { copy_string_attr(popup, "AXValue") }.unwrap_or_default();
    let matches = |current: &str| current.trim().to_lowercase() == wanted.to_lowercase();
    if matches(&selected()) {
        return Some(format!(
            "✅ '{wanted}' is already selected in AXPopUpButton [{element_index}] \
             \"{element_title}\"; nothing was changed."
        ));
    }
    // Focus moves inside the page only: no activation, no window order change.
    unsafe { crate::ax::bindings::set_bool_attr_true(popup, "AXFocused") };
    std::thread::sleep(POPUP_POLL_INTERVAL);
    if unsafe { copy_bool_attr(popup, "AXFocused") } != Some(true) {
        return None;
    }
    // Keys go to the window's focused element, so the window must be the
    // app's key window (in-app records only; the app is not activated).
    let _scope = super::key_scope(pid, Some(window_id)).ok()?;
    crate::input::keyboard::type_text(pid, wanted).ok()?;
    let deadline = std::time::Instant::now() + TYPEAHEAD_SETTLE;
    loop {
        let current = selected();
        if matches(&current) {
            return Some(format!(
                "✅ Selected '{current}' in AXPopUpButton [{element_index}] \
                 \"{element_title}\" by typing its label into the focused control \
                 (the menu was not opened, so the window order did not change)."
            ));
        }
        if std::time::Instant::now() >= deadline {
            tracing::debug!(
                pid,
                wanted,
                current,
                "type-ahead did not select the option; opening the menu"
            );
            return None;
        }
        std::thread::sleep(POPUP_POLL_INTERVAL);
    }
}

fn select_popup_option(
    element: AXUIElementRef,
    element_index: usize,
    pid: i32,
    window_id: u32,
    value: &str,
    element_title: &str,
) -> anyhow::Result<PopupChoice> {
    if let Some(detail) = select_chromium_popup_by_typeahead(
        element,
        element_index,
        pid,
        window_id,
        value,
        element_title,
    ) {
        return Ok(PopupChoice::sent(detail));
    }
    let mut options = popup_options(element);
    let mut opened_menu = false;
    let mut press_thread = None;

    if matching_option(&option_pairs(&options), value).is_none() {
        // Safari/WebKit: no AX children while the popup is closed. Set the
        // <select> through the DOM instead of opening a menu.
        if options.is_empty() {
            let app_name = crate::apps::get_app_name_for_pid(pid).unwrap_or_default();
            if app_name == "Safari" {
                return set_select_via_js(element_index, element_title, value)
                    .map(PopupChoice::sent);
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
        // Opening the menu of a background app raises the popup's window over
        // the user's window until the guard puts it back, a frame or two
        // later. Choose the item by typing its title into the closed popup
        // instead; when AppKit does not take it, leave the popup as it is
        // and say why.
        if !already_open && crate::key_focus_guard::menu_open_would_raise(pid, window_id) {
            release_options(&options);
            if !is_chromium_popup_owner(pid) {
                match select_appkit_popup_by_type_select(
                    element,
                    element_index,
                    pid,
                    window_id,
                    value,
                ) {
                    Ok(choice) => return Ok(choice),
                    Err(miss) => {
                        tracing::debug!(pid, value, ?miss, "type-select did not choose the item");
                        return Err(anyhow::Error::new(PopupMenuWouldRaise {
                            element_index,
                            title: super::menu_raise::label(element),
                            current: popup_shown_value(element),
                            type_select: Some(miss.code()),
                        }));
                    }
                }
            }
            return Err(anyhow::Error::new(PopupMenuWouldRaise {
                element_index,
                title: super::menu_raise::label(element),
                current: popup_shown_value(element),
                type_select: None,
            }));
        }
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
                Ok(PopupChoice::sent(format!(
                    "✅ Selected '{label}' in AXPopUpButton [{element_index}] \
                     \"{element_title}\" via AX menu item AXPress{}.",
                    if opened_menu {
                        " (the popup's menu was opened for the selection and closed again)"
                    } else {
                        ""
                    }
                )))
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
mod type_select_tests {
    use super::{run_type_select, type_select_keys, TypeSelectMiss};
    use std::cell::{Cell, RefCell};
    use std::time::Duration;

    /// A closed popup that applies type-select the way AppKit does: typed
    /// characters accumulate, and once typing pauses it shows the first item
    /// whose title starts with them, or keeps its item when none does. `active`
    /// false is an app AppKit does not count as active: keys change nothing.
    struct FakePopup {
        items: Vec<&'static str>,
        shown: RefCell<String>,
        typed: RefCell<String>,
        active: bool,
    }

    impl FakePopup {
        fn new(items: &[&'static str], shown: &str, active: bool) -> Self {
            Self {
                items: items.to_vec(),
                shown: RefCell::new(shown.into()),
                typed: RefCell::new(String::new()),
                active,
            }
        }

        fn press(&self, key: char) -> anyhow::Result<()> {
            self.typed.borrow_mut().push(key);
            Ok(())
        }

        fn read(&self) -> String {
            let typed = self.typed.borrow().to_lowercase();
            if self.active && !typed.is_empty() {
                if let Some(item) = self
                    .items
                    .iter()
                    .find(|item| item.to_lowercase().starts_with(&typed))
                {
                    *self.shown.borrow_mut() = item.to_string();
                }
            }
            self.shown.borrow().clone()
        }
    }

    fn choose(popup: &FakePopup, wanted: &str) -> Result<String, TypeSelectMiss> {
        run_type_select(
            wanted,
            &type_select_keys(wanted),
            |key| popup.press(key),
            || popup.read(),
            Duration::from_millis(30),
            Duration::from_millis(5),
        )
    }

    #[test]
    fn typing_the_title_chooses_the_item() {
        let popup = FakePopup::new(
            &[
                "Choose a team",
                "Design",
                "Finance",
                "Operations",
                "Research",
            ],
            "Choose a team",
            true,
        );
        assert_eq!(choose(&popup, "Operations"), Ok("Operations".into()));
        assert_eq!(*popup.typed.borrow(), "Operations");
    }

    #[test]
    fn a_shared_prefix_is_told_apart_by_the_rest_of_the_title() {
        let popup = FakePopup::new(
            &["Hardware", "Software", "Services", "Travel", "Training"],
            "Hardware",
            true,
        );
        assert_eq!(choose(&popup, "services"), Ok("Services".into()));
    }

    #[test]
    fn typing_stops_before_a_space_which_would_open_the_menu() {
        assert_eq!(
            type_select_keys(" Office Supplies "),
            vec!['O', 'f', 'f', 'i', 'c', 'e']
        );
        assert!(type_select_keys(" ").is_empty());
        let popup = FakePopup::new(&["Hardware", "Office Supplies"], "Hardware", true);
        assert_eq!(
            choose(&popup, "Office Supplies"),
            Ok("Office Supplies".into())
        );
        assert!(!popup.typed.borrow().contains(' '));
    }

    #[test]
    fn a_title_whose_first_word_names_another_item_is_not_chosen() {
        // "High" is a prefix of "High priority" and comes first: the popup
        // shows "High", which is not what was asked for.
        let popup = FakePopup::new(&["Low", "High", "High priority"], "Low", true);
        assert_eq!(
            choose(&popup, "High priority"),
            Err(TypeSelectMiss::NotChosen {
                shown: "High".into()
            })
        );
    }

    #[test]
    fn an_app_that_ignores_type_select_is_a_miss_and_keeps_its_item() {
        let popup = FakePopup::new(&["Low", "Medium", "High"], "Low", false);
        assert_eq!(
            choose(&popup, "High"),
            Err(TypeSelectMiss::NotChosen {
                shown: "Low".into()
            })
        );
    }

    #[test]
    fn a_title_with_no_typeable_start_sends_nothing() {
        let popup = FakePopup::new(&["Low"], "Low", true);
        assert_eq!(choose(&popup, "   "), Err(TypeSelectMiss::NoKeys));
        assert!(popup.typed.borrow().is_empty());
    }

    #[test]
    fn a_failed_key_post_stops_typing() {
        let popup = FakePopup::new(&["Low", "High"], "Low", true);
        let result = run_type_select(
            "High",
            &type_select_keys("High"),
            |_| anyhow::bail!("post failed"),
            || popup.read(),
            Duration::from_millis(30),
            Duration::from_millis(5),
        );
        assert_eq!(
            result,
            Err(TypeSelectMiss::NotChosen {
                shown: "Low".into()
            })
        );
    }

    /// A fake app for [`super::ensure_counts_itself_active`]. Once an
    /// activation is posted it counts itself active after `reads_until_active`
    /// reads (never with `None`); dropping the activation makes it inactive
    /// again. `panel` is a window it shows only while active; `own_after` puts
    /// a window of its own on screen after that many window reads, active or
    /// not (a banner in answer to an earlier action).
    struct FakeApp {
        state: std::rc::Rc<FakeState>,
        reads_until_active: Option<u32>,
        panel: bool,
        own_after: Option<u32>,
        posts: bool,
    }

    #[derive(Default)]
    struct FakeState {
        active: Cell<bool>,
        pending: Cell<Option<u32>>,
        posted: Cell<u32>,
        undone: Cell<u32>,
        window_reads: Cell<u32>,
    }

    struct FakeActivation(std::rc::Rc<FakeState>);

    impl Drop for FakeActivation {
        fn drop(&mut self) {
            self.0.undone.set(self.0.undone.get() + 1);
            self.0.pending.set(None);
            self.0.active.set(false);
        }
    }

    const MAIN: u32 = 1;
    const PANEL: u32 = 7;
    const OWN: u32 = 9;

    impl FakeApp {
        fn new(active: bool, reads_until_active: Option<u32>) -> Self {
            let state = FakeState::default();
            state.active.set(active);
            Self {
                state: std::rc::Rc::new(state),
                reads_until_active,
                panel: false,
                own_after: None,
                posts: true,
            }
        }

        fn is_active(&self) -> bool {
            match self.state.pending.get() {
                Some(0) => {
                    self.state.pending.set(None);
                    self.state.active.set(true);
                }
                Some(n) => self.state.pending.set(Some(n - 1)),
                None => {}
            }
            self.state.active.get()
        }

        fn on_screen(&self) -> Vec<u32> {
            let reads = self.state.window_reads.get() + 1;
            self.state.window_reads.set(reads);
            let mut windows = vec![MAIN];
            if self.panel && self.state.active.get() {
                windows.push(PANEL);
            }
            if self.own_after.is_some_and(|after| reads > after) {
                windows.push(OWN);
            }
            windows
        }

        fn ensure(&self) -> Result<Option<FakeActivation>, TypeSelectMiss> {
            super::ensure_counts_itself_active(
                || self.is_active(),
                || {
                    self.state.posted.set(self.state.posted.get() + 1);
                    if !self.posts {
                        return None;
                    }
                    self.state.pending.set(self.reads_until_active);
                    Some(FakeActivation(self.state.clone()))
                },
                || self.on_screen(),
                super::ActivationTiming {
                    settle: Duration::from_millis(40),
                    poll: Duration::from_millis(2),
                    hide_grace: Duration::from_millis(6),
                },
            )
        }
    }

    #[test]
    fn an_app_that_counts_itself_active_gets_no_activation() {
        let app = FakeApp::new(true, None);
        assert!(matches!(app.ensure(), Ok(None)));
        assert_eq!(app.state.posted.get(), 0);
        assert_eq!(app.state.undone.get(), 0);
    }

    #[test]
    fn an_inactive_app_is_activated_in_app_and_handed_back_after() {
        let app = FakeApp::new(false, Some(2));
        let guard = app.ensure().expect("activated");
        assert!(guard.is_some());
        assert_eq!(app.state.posted.get(), 1);
        // Still active while the guard (the typing) lasts.
        assert_eq!(app.state.undone.get(), 0);
        drop(guard);
        assert_eq!(app.state.undone.get(), 1);
    }

    #[test]
    fn an_app_that_stays_inactive_is_handed_back_and_refused() {
        let app = FakeApp::new(false, None);
        assert!(matches!(app.ensure(), Err(TypeSelectMiss::AppNotActive)));
        assert_eq!(app.state.posted.get(), 1);
        assert_eq!(app.state.undone.get(), 1);
    }

    #[test]
    fn records_that_cannot_be_posted_are_refused() {
        let mut app = FakeApp::new(false, Some(0));
        app.posts = false;
        assert!(matches!(app.ensure(), Err(TypeSelectMiss::AppNotActive)));
        assert_eq!(app.state.undone.get(), 0);
    }

    #[test]
    fn an_activation_that_shows_a_window_is_undone_at_once() {
        let mut app = FakeApp::new(false, Some(0));
        app.panel = true;
        let result = app.ensure();
        assert!(matches!(
            result,
            Err(TypeSelectMiss::ActivationShowedWindows)
        ));
        // Undone once; not tried again, since the window went away with it.
        assert_eq!(app.state.posted.get(), 1);
        assert_eq!(app.state.undone.get(), 1);
        assert!(!app.state.active.get());
        assert_eq!(
            TypeSelectMiss::ActivationShowedWindows.code(),
            "activation_showed_windows"
        );
    }

    /// The IR-02 race: the app puts a banner of its own on screen (in answer
    /// to the field set just before) while the activation is posted. The
    /// banner stays once the app is inactive again, so it is not the
    /// activation's: the activation is tried again and goes through.
    #[test]
    fn a_window_the_app_shows_by_itself_meanwhile_is_not_refused() {
        let mut app = FakeApp::new(false, Some(1));
        // Read 1 is the baseline; the banner is up from read 2 on.
        app.own_after = Some(1);
        let guard = app.ensure().expect("activated on the second try");
        assert!(guard.is_some());
        assert_eq!(app.state.posted.get(), 2);
        assert_eq!(app.state.undone.get(), 1);
        assert!(app.state.active.get());
        drop(guard);
        assert_eq!(app.state.undone.get(), 2);
    }

    /// A panel the activation shows is still refused when a window of the
    /// app's own appears in the same moment.
    #[test]
    fn an_activation_panel_is_refused_beside_a_window_of_the_apps_own() {
        let mut app = FakeApp::new(false, Some(0));
        app.own_after = Some(1);
        app.panel = true;
        assert!(matches!(
            app.ensure(),
            Err(TypeSelectMiss::ActivationShowedWindows)
        ));
        assert_eq!(app.state.posted.get(), 1);
        assert!(!app.state.active.get());
    }
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
    crate::apple_events::ensure_permitted("com.apple.Safari", "Safari")
        .map_err(|r| anyhow::anyhow!("{}: {}", r.code, r.reason))?;
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
        let refused = super::finder_rename_result(
            7,
            42,
            path,
            "b.txt",
            Ok(Err(crate::apple_events::ScriptRefusal {
                code: "automation_consent_required",
                reason: "Grant Finder access in System Settings > Privacy & Security > Automation"
                    .into(),
            })),
        );
        assert_eq!(refused.is_error, Some(true));
        let data = refused.structured_content.unwrap();
        assert_eq!(data["code"], "automation_consent_required");
        assert_eq!(data["cause"], "automation_consent_required");
        assert_eq!(data["effect"], "refused");
        assert_eq!(
            data["reason"],
            "Grant Finder access in System Settings > Privacy & Security > Automation"
        );
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
            popup_route: None,
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
            popup_route: None,
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
            popup_route: None,
        };
        apply_verification_label(&mut outcome);
        assert_eq!(
            outcome.detail,
            "📨 Sent (unverified) AXValue on [4] AXTextField."
        );
    }

    #[test]
    fn a_raise_put_back_during_the_call_is_reported() {
        use crate::key_focus_guard::Undone;
        let before = Undone {
            raises: 4,
            activations: 2,
        };
        assert!(super::disturbance_note(before, before).is_none());
        let (note, report) = super::disturbance_note(
            before,
            Undone {
                raises: 5,
                activations: 2,
            },
        )
        .expect("raise reported");
        assert!(note.contains("above the user's window"), "{note}");
        assert_eq!(report["window_raised"], true);
        assert_eq!(report["app_activated"], false);
        assert_eq!(report["restored"], true);
        let (_, report) = super::disturbance_note(
            before,
            Undone {
                raises: 4,
                activations: 3,
            },
        )
        .expect("activation reported");
        assert_eq!(report["app_activated"], true);
    }
}
