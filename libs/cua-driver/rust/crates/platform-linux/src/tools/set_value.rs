// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::atspi::native::{OptionPick, OptionPickSent};

use super::impl_::{
    attach_focus_guard, background_virtual_key, background_virtual_text,
    checked_element_local_coords, element_screen_center, foreground_budget, input_error_result,
    non_ax_escalation, position_named_session_keyboard_cursor, prove_token_element,
    refuse_if_disabled, resolve_cursor_key, resolve_element_local_coords, spawn_blocking_bounded,
    stale_observed_element, values_agree, with_cleanup_warning, x11_pixel_click_no_focus_steal,
    ToolState,
};

// ── set_value ─────────────────────────────────────────────────────────────────

pub struct SetValueTool {
    pub(crate) state: Arc<ToolState>,
}
static SV_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetValueTool {
    fn def(&self) -> &ToolDef {
        SV_DEF.get_or_init(|| ToolDef {
            name: "set_value".into(),
            description: "Set value of an AT-SPI element via SetValue action.".into(),
            input_schema: json!({
                "type":"object","required":["value"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID. Optional with element_token (the token carries the pid)."},
                    "window_id":{"type":"integer","description":"Omit when element_token is supplied (the token carries it)."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "value":{"type":"string","description":"New value. Written through AT-SPI EditableText/Value when the element exposes them; otherwise (GTK2 spin scales, VCL spin buttons) the field is clicked with the session's real pointer, its text selected and replaced through the virtual keyboard and committed with Tab, then read back."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let value = match args.get("value") {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            _ => {
                return ToolResult::error("set_value: 'value' must be a string or a number.")
                    .with_structured(json!({ "code": "invalid_arguments" }))
            }
        };
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let token = match prove_token_element(pid, &resolved, "set_value").await {
            Ok(token) => token,
            Err(refusal) => return refusal,
        };
        if let Some(refusal) = refuse_if_disabled(pid, &resolved, "set_value").await {
            return refusal;
        }
        let (token, xid) = match (token, &resolved) {
            (
                Some(token),
                cua_driver_core::element_token::ResolvedElement::Element { window_id, .. },
            ) => (token, *window_id),
            _ => {
                return ToolResult::error(
                    "set_value requires element_token to address the target element.",
                )
            }
        };
        let idx = token.index;
        let value_for_task = value.clone();
        let xid_opt = Some(xid);
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let cursor_id = resolve_cursor_key(&args);
        position_named_session_keyboard_cursor(
            &self.state,
            &args,
            pid,
            xid,
            Some(token.clone()),
            None,
            true,
        )
        .await;
        // 1. Focus-free accessibility write on the exact snapshot object.
        let target = token.clone();
        let ax = spawn_blocking_bounded("set_value", foreground_budget(0), move || {
            crate::atspi::set_value_token(pid, &target, &value_for_task)
        })
        .await;
        let ax_error = match ax {
            Ok(Ok(())) => {
                let target = token.clone();
                let readback =
                    tokio::task::spawn_blocking(move || crate::atspi::read_value_token(&target))
                        .await
                        .ok()
                        .flatten();
                return set_value_result(idx, &value, "ax", readback, None);
            }
            Ok(Err(error)) if crate::atspi::native::is_no_value_route(&error) => error,
            Ok(Err(error)) if crate::atspi::native::is_no_such_option(&error) => {
                return no_such_option_result(idx, &error)
            }
            Ok(Err(error)) if error.is::<OptionPickSent>() => {
                let readback = read_back(token).await;
                return pick_sent_result(idx, &value, "ax", readback, &error);
            }
            Ok(Err(error)) => {
                return stale_observed_element("set_value", idx, &error)
                    .unwrap_or_else(|| input_error_result(error))
            }
            Err(error) => return ToolResult::error(format!("Task error: {error}")),
        };
        // 2. No accessibility write route. A combo box is opened with a click
        //    and the option picked from its popup (whose items only exist and
        //    act while it is open), then the popup is closed. Anything else is
        //    typed like a user would: click the field (real pointer), select
        //    all, type, commit with Tab.
        let is_combo = token.observed.role.eq_ignore_ascii_case("combo box");
        let wayland = crate::wayland::is_wayland();
        let background_route = !delivery.is_foreground()
            && !wayland
            && crate::input::real_pointer_input_available()
            && crate::input::real_keyboard_input_available();
        let foreground_route = delivery.is_foreground() && !wayland;
        if !background_route && !foreground_route {
            let detail = format!(
                "{ax_error}; the keyboard fallback needs {}",
                if wayland {
                    "an X11 session"
                } else {
                    "the focus-free real-input route (a writable /dev/uinput), or delivery_mode:\"foreground\""
                }
            );
            let mut structured = json!({
                "code": "set_value_unavailable",
                "detail": detail,
                "effect": "none",
            });
            // Only X11 can run the foreground keyboard fallback.
            if !wayland {
                structured["escalation"] = json!({ "recommended": "foreground", "reason": "click the field and type the value with delivery_mode:\"foreground\"" });
            }
            return ToolResult::error(format!(
                "set_value: element [{idx}] has no accessibility write route and no keyboard \
                 fallback is available: {detail}."
            ))
            .with_structured(structured);
        }
        let value_for_keys = value.clone();
        let target = token.clone();
        let typed = spawn_blocking_bounded(
            "set_value keyboard fallback",
            foreground_budget(value.len()),
            move || -> anyhow::Result<(Option<crate::input::FocusGuardReport>, Option<String>)> {
                if foreground_route {
                    ensure_element_showing(&target)?;
                    let (win, _, _) = checked_element_local_coords(pid, &target, xid_opt)
                        .map_err(map_bounds_lookup_error)?;
                    let (sx, sy) = element_screen_center(pid, &target, Some(win))
                        .map_err(map_bounds_lookup_error)?;
                    let (sx_i, sy_i) = (sx.round() as i32, sy.round() as i32);
                    ensure_element_owns_point(pid, win, &target, sx_i, sy_i)?;
                    if is_combo {
                        return pick_from_popup(
                            &cursor_id,
                            pid,
                            &target,
                            win,
                            PopupClickTarget::ForegroundScreen(sx_i, sy_i),
                            &value_for_keys,
                        );
                    }
                    crate::interaction::with_x11_foreground(
                        win,
                        crate::input::ForegroundOptions::pointer(),
                        crate::interaction::session_policy().menu,
                        || {
                            ensure_element_owns_point(pid, win, &target, sx_i, sy_i)?;
                            ensure_window_owns_screen_point(win, sx_i, sy_i)?;
                            crate::input::send_click_xtest_desktop(sx_i, sy_i, 1, 1)?;
                            std::thread::sleep(std::time::Duration::from_millis(80));
                            crate::input::send_key_xtest("a", &["ctrl"])?;
                            crate::input::send_type_text_xtest(&value_for_keys)?;
                            crate::input::send_key_xtest("Tab", &[])
                        },
                    )?;
                    return Ok((None, None));
                }
                let (win, lx, ly) = resolve_element_local_coords(pid, &target, xid_opt)?;
                if is_combo {
                    return pick_from_popup(
                        &cursor_id,
                        pid,
                        &target,
                        win,
                        PopupClickTarget::BackgroundLocal(lx, ly),
                        &value_for_keys,
                    );
                }
                let (_, guard) = crate::input::focus_guard::guarded(Some(pid), || {
                    x11_pixel_click_no_focus_steal(&cursor_id, win, lx as i32, ly as i32, 1, 1)?;
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    background_virtual_key(&cursor_id, pid, win, "a", &["ctrl"])?;
                    background_virtual_text(
                        &cursor_id,
                        pid,
                        win,
                        &value_for_keys,
                        crate::input::TypePacing::default(),
                    )?;
                    background_virtual_key(&cursor_id, pid, win, "Tab", &[])?;
                    Ok(())
                })?;
                Ok((guard, None))
            },
        )
        .await;
        match typed {
            Ok(Ok((guard, warning))) => {
                // Let the toolkit commit on focus-out before reading back.
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                let readback =
                    tokio::task::spawn_blocking(move || crate::atspi::read_value_token(&token))
                        .await
                        .ok()
                        .flatten();
                let result = set_value_result(
                    idx,
                    &value,
                    match (is_combo, foreground_route) {
                        (true, true) => "click_select_fg",
                        (true, false) => "click_select_mpx",
                        (false, true) => "click_type_fg",
                        (false, false) => "click_type_mpx",
                    },
                    readback,
                    guard.as_ref(),
                );
                with_cleanup_warning(result, warning)
            }
            Ok(Err(error)) if crate::atspi::native::is_no_such_option(&error) => {
                no_such_option_result(idx, &error)
            }
            Ok(Err(error)) if error.is::<OptionPickSent>() => {
                let path = if foreground_route {
                    "click_select_fg"
                } else {
                    "click_select_mpx"
                };
                let readback = read_back(token).await;
                pick_sent_result(idx, &value, path, readback, &error)
            }
            Ok(Err(error)) => keyboard_fallback_refusal(idx, pid, &error)
                .unwrap_or_else(|| input_error_result(error)),
            Err(error) => ToolResult::error(format!("Task error: {error}")),
        }
    }
}

/// `set_value` named an option the combo box or list does not have.
fn no_such_option_result(idx: usize, error: &anyhow::Error) -> ToolResult {
    let detail = error.to_string();
    let detail = detail
        .strip_prefix(crate::atspi::native::NO_SUCH_OPTION)
        .map(|rest| rest.trim_start_matches(':').trim())
        .unwrap_or(&detail)
        .to_owned();
    ToolResult::error(format!(
        "set_value: element [{idx}] has {detail}; nothing was changed."
    ))
    .with_structured(json!({ "code": "option_not_found", "detail": detail, "effect": "none" }))
}

/// How long `set_value` waits for an opened combo box popup to list its
/// options.
const POPUP_OPTIONS_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Pre-click refusal on foreground keyboard fallback routes: either the cached
/// element vanished before any input (`StaleElement`), or it is not on screen,
/// its point lies outside the target window, another window covers the point,
/// or `point_ownership` reports a different control there (`BoundsUnavailable`).
#[derive(Debug)]
enum PreClickRefusal {
    BoundsUnavailable {
        reason: String,
        owner: Option<String>,
    },
    StaleElement(anyhow::Error),
}

impl std::fmt::Display for PreClickRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BoundsUnavailable { reason, .. } => f.write_str(reason),
            Self::StaleElement(error) => std::fmt::Display::fmt(error, f),
        }
    }
}
impl std::error::Error for PreClickRefusal {}

fn ensure_element_showing(target: &crate::atspi::TokenElement) -> anyhow::Result<()> {
    match crate::atspi::element_showing_observed(&target.observed) {
        Ok(true) => Ok(()),
        Ok(false) => Err(PreClickRefusal::BoundsUnavailable {
            reason: format!(
                "the {} is not showing (scrolled out of view or hidden)",
                target.observed.role
            ),
            owner: None,
        }
        .into()),
        Err(error) if error.is::<crate::atspi::native::CachedElementGone>() => {
            Err(PreClickRefusal::StaleElement(error).into())
        }
        Err(error) => {
            tracing::debug!(
                "set_value element [{}] showing-state unavailable: {error:#}",
                target.index
            );
            Ok(())
        }
    }
}

/// Reuse the click tool's `point_ownership` hit test before clicking a non-web
/// element: if another control owns the cached centre, refuse before sending
/// pointer or keyboard input.
fn ensure_element_owns_point(
    pid: u32,
    win: u64,
    target: &crate::atspi::TokenElement,
    sx: i32,
    sy: i32,
) -> anyhow::Result<()> {
    if target.observed.in_web_content {
        return Ok(());
    }
    let Some(oref) = target.observed.object_ref.as_ref() else {
        return Ok(());
    };
    match crate::atspi::native::hit_test::point_ownership(pid, win, oref, sx, sy) {
        crate::atspi::native::hit_test::PointOwnership::Mismatch { owner }
        | crate::atspi::native::hit_test::PointOwnership::Redirect { owner, .. } => {
            Err(PreClickRefusal::BoundsUnavailable {
                reason: "point_owned_by_another_element".to_owned(),
                owner: Some(owner.describe()),
            }
            .into())
        }
        _ => Ok(()),
    }
}

fn map_bounds_lookup_error(error: anyhow::Error) -> anyhow::Error {
    if error.is::<crate::atspi::native::CachedElementGone>() {
        return PreClickRefusal::StaleElement(error).into();
    }
    PreClickRefusal::BoundsUnavailable {
        reason: error.to_string(),
        owner: None,
    }
    .into()
}

/// Verify on live X11 state right before an XTest click that `(sx, sy)` is
/// inside `win`'s visible bounds and that `win` owns that screen point.
fn ensure_window_owns_screen_point(win: u64, sx: i32, sy: i32) -> anyhow::Result<()> {
    use crate::x11::WindowPointOwnership;
    let ownership = crate::x11::point_owned_by_window(win, sx, sy).map_err(|error| {
        anyhow::Error::new(PreClickRefusal::BoundsUnavailable {
            reason: format!("could not verify X11 window ownership ({error})"),
            owner: None,
        })
    })?;
    match ownership {
        WindowPointOwnership::Owned => Ok(()),
        WindowPointOwnership::NotViewable => Err(PreClickRefusal::BoundsUnavailable {
            reason: format!("window {win} is not viewable"),
            owner: None,
        }
        .into()),
        WindowPointOwnership::OutsideWindow {
            bounds: (wx, wy, ww, wh),
        } => Err(PreClickRefusal::BoundsUnavailable {
            reason: format!(
                "point ({sx}, {sy}) lies outside window {win} visible bounds ({wx}, {wy}, {ww}x{wh})"
            ),
            owner: None,
        }
        .into()),
        WindowPointOwnership::Occluded { covering } => Err(PreClickRefusal::BoundsUnavailable {
            reason: match covering {
                Some(other) => format!(
                    "point ({sx}, {sy}) is covered by topmost window {other} instead of target window {win}"
                ),
                None => format!(
                    "point ({sx}, {sy}) is over bare desktop instead of target window {win}"
                ),
            },
            owner: None,
        }
        .into()),
    }
}

/// Map a foreground pre-click refusal (`anyhow::Error::downcast_ref` reaches
/// through the focus-restore `.context(...)` wrapper added by
/// `with_x11_foreground`). Errors raised after input was sent, or on background
/// MPX routes, are never wrapped in `PreClickRefusal` and fall through to
/// `input_error_result`.
fn keyboard_fallback_refusal(idx: usize, pid: u32, error: &anyhow::Error) -> Option<ToolResult> {
    let refusal = error.downcast_ref::<PreClickRefusal>()?;
    let (reason, owner) = match refusal {
        PreClickRefusal::StaleElement(inner) => {
            return stale_observed_element("set_value", idx, inner);
        }
        PreClickRefusal::BoundsUnavailable { reason, owner } => (reason, owner),
    };
    let mut structured = json!({
        "code": "element_bounds_unavailable",
        "effect": "none",
        "element_index": idx,
        "reason": reason,
    });
    if let Some(owner) = owner {
        structured["owner"] = json!(owner);
    }
    let detail = if let Some(owner) = owner {
        let context = if error.source().is_some() {
            format!(" ({error})")
        } else {
            String::new()
        };
        format!(
            "set_value: element [{idx}] (pid {pid}) screen point is owned by {owner} rather \
             than the observed element{context}; no pointer or keyboard input was sent. \
             Re-snapshot with get_window_state or scroll the element into view."
        )
    } else {
        format!(
            "set_value: element [{idx}] (pid {pid}) cannot be clicked safely ({error}); \
             no pointer or keyboard input was sent. Re-snapshot with get_window_state or \
             scroll the element into view."
        )
    };
    Some(ToolResult::error(detail).with_structured(structured))
}

enum PopupClickTarget {
    ForegroundScreen(i32, i32),
    BackgroundLocal(f64, f64),
}

/// Keyboard-fallback `set_value` on a combo box whose options cannot be
/// selected while it is closed: click it open (the real pointer: XTest in
/// the foreground for `ForegroundScreen`, the session's MPX pointer in the
/// background for `BackgroundLocal`), activate the option `value` names from
/// the popup, and press Escape if the popup is still open afterwards (an
/// unknown option, or a toolkit that keeps it open), so no popup is left
/// behind. Returns the focus guard report and a warning when the popup could
/// not be closed after the pick.
fn pick_from_popup(
    cursor_id: &str,
    pid: u32,
    token: &crate::atspi::TokenElement,
    win: u64,
    click_target: PopupClickTarget,
    value: &str,
) -> anyhow::Result<(Option<crate::input::FocusGuardReport>, Option<String>)> {
    let foreground = matches!(click_target, PopupClickTarget::ForegroundScreen(..));
    let close_popup = || -> anyhow::Result<()> {
        if !crate::atspi::options_showing_token(token) {
            return Ok(());
        }
        if foreground {
            // Inside the click's transaction: the target still has the focus.
            crate::input::send_key_xtest("Escape", &[])
        } else {
            background_virtual_key(cursor_id, pid, win, "Escape", &[]).map(|_| ())
        }
    };
    let pick = || {
        pick_listed_option(value, POPUP_OPTIONS_WAIT, || {
            crate::atspi::activate_option_token(token, value)
        })
    };
    let open_and_pick = || pick_then_close(pick, close_popup);
    match click_target {
        PopupClickTarget::ForegroundScreen(sx_i, sy_i) => {
            // Open, pick and close in one transaction, so the restore runs once
            // the popup is gone instead of closing it before the pick.
            let (warning, _report) = crate::interaction::with_x11_foreground(
                win,
                crate::input::ForegroundOptions::pointer(),
                crate::interaction::session_policy().menu,
                || {
                    ensure_element_owns_point(pid, win, token, sx_i, sy_i)?;
                    ensure_window_owns_screen_point(win, sx_i, sy_i)?;
                    crate::input::send_click_xtest_desktop(sx_i, sy_i, 1, 1)?;
                    open_and_pick()
                },
            )?;
            Ok((None, warning))
        }
        PopupClickTarget::BackgroundLocal(lx, ly) => {
            let (warning, guard) = crate::input::focus_guard::guarded(Some(pid), || {
                x11_pixel_click_no_focus_steal(cursor_id, win, lx as i32, ly as i32, 1, 1)?;
                open_and_pick()
            })?;
            Ok((guard, warning))
        }
    }
}

/// Pick the option, then close the popup if it is still open. The pick's
/// error wins. A close that fails after a successful pick is a warning, not
/// an error: the option was set, and an error would invite picking again.
fn pick_then_close(
    pick: impl FnOnce() -> anyhow::Result<String>,
    close: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<Option<String>> {
    let picked = pick();
    // Let the toolkit close the popup on activation before checking.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let closed = close();
    picked?;
    Ok(closed.err().map(|error| {
        format!("closing the combo box popup after the pick failed ({error:#}), so it may still be open")
    }))
}

#[cfg(test)]
#[test]
fn a_failed_popup_close_never_turns_a_pick_into_an_error() {
    let closing_fails = || Err(anyhow::anyhow!("virtual keyboard unavailable"));
    let warning =
        pick_then_close(|| Ok("Medium".into()), closing_fails).expect("the option was picked");
    assert!(
        warning.is_some_and(|warning| warning.contains("virtual keyboard unavailable")),
        "a failed close is reported beside the pick"
    );
    assert_eq!(
        pick_then_close(|| Ok("Medium".into()), || Ok(())).unwrap(),
        None
    );
    let failed = pick_then_close(
        || Err(anyhow::anyhow!("no option named Huge")),
        closing_fails,
    );
    assert_eq!(failed.unwrap_err().to_string(), "no option named Huge");
}

/// Retry `activate` until the opened popup answers with its options or
/// `wait` runs out, then pick the option `value` names. A popup whose every
/// read answered and that still lists no options has no option `value`
/// names: the same `no_such_option` refusal as an unknown option. One whose
/// options could not be read stays an error, since a populated popup can be
/// unreadable too.
fn pick_listed_option(
    value: &str,
    wait: std::time::Duration,
    mut activate: impl FnMut() -> anyhow::Result<OptionPick>,
) -> anyhow::Result<String> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        let pick = activate()?;
        let waiting = std::time::Instant::now() < deadline;
        match pick {
            OptionPick::Picked { name } => return Ok(name),
            OptionPick::NoMatch { options } => {
                return Err(crate::atspi::no_such_option(value, &options))
            }
            OptionPick::NoOptions | OptionPick::Unavailable if waiting => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            OptionPick::NoOptions => return Err(crate::atspi::no_such_option(value, &[])),
            OptionPick::Unavailable => {
                anyhow::bail!("the combo box popup's options could not be read")
            }
        }
    }
}

async fn read_back(token: crate::atspi::TokenElement) -> Option<String> {
    tokio::task::spawn_blocking(move || crate::atspi::read_value_token(&token))
        .await
        .ok()
        .flatten()
}

/// `set_value` result for a combo box pick that was sent but not answered
/// ([`OptionPickSent`]): the option may be set, so it is reported like a
/// write with its readback, never as a failure that invites picking again.
/// Without a matching readback the effect is unknown, not a no-op, and no
/// escalation is offered.
fn pick_sent_result(
    idx: usize,
    value: &str,
    path: &str,
    readback: Option<String>,
    error: &anyhow::Error,
) -> ToolResult {
    let mut result = set_value_result(idx, value, path, readback, None);
    let note = format!("the option pick was sent, but {error}; it may have taken effect");
    if let Some(structured) = result.structured_content.as_mut() {
        if structured["verified"] != true {
            structured["effect"] = json!("unverifiable");
            if let Some(fields) = structured.as_object_mut() {
                fields.remove("escalation");
            }
        }
        structured["note"] = json!(note);
    }
    if let Some(cua_driver_core::protocol::Content::Text { text, .. }) = result.content.first_mut()
    {
        text.push_str(&format!(
            " Note: {note}. Read it back before picking again."
        ));
    }
    result
}

/// `set_value` result with an honest verification: `verified` only when the
/// element read back the requested value.
fn set_value_result(
    idx: usize,
    value: &str,
    path: &str,
    readback: Option<String>,
    guard: Option<&crate::input::FocusGuardReport>,
) -> ToolResult {
    let normalised_path = readback
        .as_deref()
        .is_some_and(|seen| path_normalised_to_basename(value, seen));
    let verified = readback
        .as_deref()
        .is_some_and(|seen| values_agree(value, seen))
        || normalised_path;
    let mut structured = json!({
        "path": path,
        "verified": verified,
        "effect": if verified { "confirmed" } else if readback.is_some() { "suspected_noop" } else { "unverifiable" },
    });
    let mut text = format!("Set value of element [{idx}] to '{value}' (path={path})");
    match readback {
        Some(seen) if normalised_path => {
            structured["readback"] = json!(seen);
            text.push_str(&format!(
                "; the field reads back '{seen}': a file chooser navigated to the path's \
                 directory and kept the file name (the write committed)."
            ));
        }
        Some(seen) if verified => text.push_str(&format!("; read back '{seen}'.")),
        Some(seen) => {
            structured["readback"] = json!(seen);
            structured["escalation"] = non_ax_escalation();
            text.push_str(&format!(
                "; but the element reads back '{seen}' — the write may not have committed. \
                 Confirm with get_window_state or a screenshot."
            ));
        }
        None => {
            structured["readback"] = Value::Null;
            text.push_str("; not verified — confirm with get_window_state.");
        }
    }
    attach_focus_guard(ToolResult::text(text).with_structured(structured), guard)
}

/// A Qt/GTK file chooser given an absolute path in its "File name" field
/// moves to that directory and shows only the final component: the readback
/// is the basename, and the write committed.
fn path_normalised_to_basename(value: &str, seen: &str) -> bool {
    let value = value.trim();
    let seen = seen.trim();
    value.contains('/')
        && !seen.is_empty()
        && value
            .rsplit('/')
            .next()
            .is_some_and(|base| !base.is_empty() && base == seen)
}

#[cfg(test)]
mod set_value_tests {
    use super::*;

    #[test]
    fn a_file_chooser_keeping_the_basename_counts_as_committed() {
        assert!(path_normalised_to_basename(
            "/home/user/Desktop/tone.wav",
            "tone.wav"
        ));
        assert!(!path_normalised_to_basename(
            "/home/user/Desktop/tone.wav",
            "other.wav"
        ));
        assert!(!path_normalised_to_basename("tone.wav", "tone.wav"));
        let ok = set_value_result(
            3,
            "/home/user/Desktop/tone.wav",
            "ax",
            Some("tone.wav".into()),
            None,
        );
        let s = ok.structured_content.unwrap();
        assert_eq!(s["verified"], true);
        assert_eq!(s["effect"], "confirmed");
    }

    #[test]
    fn readback_agreement_is_numeric_aware() {
        assert!(values_agree("40", "40.0"));
        assert!(values_agree("40", " 40 "));
        assert!(values_agree("abc", "abc"));
        assert!(!values_agree("40", "41"));
        assert!(!values_agree("40", "forty"));
    }

    #[test]
    fn set_value_result_is_verified_only_on_matching_readback() {
        let ok = set_value_result(3, "40", "click_type_mpx", Some("40.0".into()), None);
        let s = ok.structured_content.unwrap();
        assert_eq!(s["verified"], true);
        assert_eq!(s["effect"], "confirmed");
        let noop = set_value_result(3, "40", "ax", Some("10.0".into()), None);
        let s = noop.structured_content.unwrap();
        assert_eq!(s["verified"], false);
        assert_eq!(s["effect"], "suspected_noop");
        assert_eq!(s["readback"], "10.0");
        let blind = set_value_result(3, "40", "ax", None, None);
        assert_eq!(blind.structured_content.unwrap()["effect"], "unverifiable");
    }

    #[test]
    fn keyboard_fallback_refusal_preserves_bounds_unavailable_when_wrapped() {
        let wrapped = anyhow::Error::new(PreClickRefusal::BoundsUnavailable {
            reason: "point_owned_by_another_element".into(),
            owner: Some("push button \"Other\"".into()),
        })
        .context("the prior focus also could not be restored");
        let refused = keyboard_fallback_refusal(29, 7, &wrapped).expect("mapped refusal");
        assert_eq!(refused.is_error, Some(true));
        let s = refused.structured_content.unwrap();
        assert_eq!(s["code"], "element_bounds_unavailable");
        assert_eq!(s["effect"], "none");
        assert_eq!(s["owner"], "push button \"Other\"");
        let Some(cua_driver_core::protocol::Content::Text { text, .. }) = refused.content.first()
        else {
            panic!("expected text content");
        };
        assert!(
            text.contains("the prior focus also could not be restored"),
            "{text}"
        );

        let unwrapped = anyhow::Error::new(PreClickRefusal::BoundsUnavailable {
            reason: "window 9 is not viewable".into(),
            owner: None,
        });
        let refused = keyboard_fallback_refusal(29, 7, &unwrapped).expect("mapped refusal");
        assert_eq!(refused.is_error, Some(true));
        let s = refused.structured_content.unwrap();
        assert_eq!(s["code"], "element_bounds_unavailable");
        assert_eq!(s["effect"], "none");
        assert!(s.get("owner").is_none(), "{s}");

        let preclick_stale = map_bounds_lookup_error(anyhow::Error::new(
            crate::atspi::native::CachedElementGone("gone before click".into()),
        ));
        let refused =
            keyboard_fallback_refusal(29, 7, &preclick_stale).expect("mapped pre-click stale");
        assert_eq!(refused.is_error, Some(true));
        let s = refused.structured_content.unwrap();
        assert_eq!(s["code"], "stale_element_token");
        assert_eq!(s["effect"], "none");

        // A raw CachedElementGone raised after a click (e.g. click_select_fg
        // popup pick) or on MPX routes must NOT map to effect: "none".
        let raw_stale = anyhow::Error::new(crate::atspi::native::CachedElementGone(
            "gone after click".into(),
        ));
        assert!(keyboard_fallback_refusal(29, 7, &raw_stale).is_none());
    }

    #[test]
    fn a_combo_box_listing_no_options_refuses_the_value_and_changes_nothing() {
        let mut attempts = 0;
        let error = pick_listed_option("Blue", std::time::Duration::ZERO, || {
            attempts += 1;
            Ok(OptionPick::NoOptions)
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(crate::atspi::native::is_no_such_option(&error), "{error}");
        let refused = no_such_option_result(3, &error);
        assert_eq!(refused.is_error, Some(true));
        let s = refused.structured_content.unwrap();
        assert_eq!(s["code"], "option_not_found");
        assert_eq!(s["effect"], "none");
        assert_eq!(s["detail"], "no option \"Blue\"; it lists no options");
    }

    #[test]
    fn an_unreadable_popup_is_never_reported_as_having_no_such_option() {
        let error = pick_listed_option("Blue", std::time::Duration::ZERO, || {
            Ok(OptionPick::Unavailable)
        })
        .unwrap_err();
        assert!(!crate::atspi::native::is_no_such_option(&error), "{error}");
    }

    #[test]
    fn a_popup_that_lists_its_options_late_still_gets_the_pick() {
        let mut attempts = 0;
        let picked = pick_listed_option("Blue", std::time::Duration::from_secs(5), || {
            attempts += 1;
            Ok(match attempts {
                1 => OptionPick::Unavailable,
                2 => OptionPick::NoOptions,
                _ => OptionPick::Picked {
                    name: "Blue".to_owned(),
                },
            })
        })
        .unwrap();
        assert_eq!(picked, "Blue");
        assert_eq!(attempts, 3);
    }

    #[test]
    fn a_sent_pick_is_never_reported_as_failed_or_a_noop() {
        let sent = anyhow::Error::new(OptionPickSent("the option's action did not answer".into()));
        // The combo box already shows the option: the pick landed.
        let landed = pick_sent_result(3, "Medium", "ax", Some("Medium".into()), &sent);
        assert_ne!(landed.is_error, Some(true));
        let s = landed.structured_content.unwrap();
        assert_eq!(s["verified"], true);
        assert_eq!(s["effect"], "confirmed");
        // It shows the old option, or nothing: unknown, with no escalation
        // that would send the pick again.
        for readback in [Some("Small".to_owned()), None] {
            let unknown = pick_sent_result(3, "Medium", "click_select_mpx", readback, &sent);
            assert_ne!(unknown.is_error, Some(true));
            let s = unknown.structured_content.unwrap();
            assert_eq!(s["verified"], false);
            assert_eq!(s["effect"], "unverifiable");
            assert!(s.get("escalation").is_none(), "{s}");
            assert!(s["note"]
                .as_str()
                .unwrap()
                .contains("may have taken effect"));
        }
    }
}
