use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    attach_focus_guard, background_virtual_key, background_virtual_text, foreground_budget,
    input_error_result, non_ax_escalation, position_named_session_keyboard_cursor,
    prove_token_element, resolve_cursor_key, resolve_element_local_coords, spawn_blocking_bounded,
    stale_observed_element, values_agree, window_local_to_screen, x11_pixel_click_no_focus_steal,
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
        let wayland = crate::wayland::wayland_input_enabled();
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
            move || -> anyhow::Result<Option<crate::input::FocusGuardReport>> {
                let (win, lx, ly) = resolve_element_local_coords(pid, &target, xid_opt)?;
                if is_combo {
                    return pick_from_popup(
                        &cursor_id,
                        pid,
                        &target,
                        (win, lx, ly),
                        &value_for_keys,
                        foreground_route,
                    );
                }
                if foreground_route {
                    let (sx, sy) = window_local_to_screen(win, lx, ly)?;
                    crate::input::with_x11_foreground_opts(
                        win,
                        crate::input::ForegroundOptions::pointer(),
                        || {
                            crate::input::send_click_xtest_desktop(
                                sx.round() as i32,
                                sy.round() as i32,
                                1,
                                1,
                            )?;
                            std::thread::sleep(std::time::Duration::from_millis(80));
                            crate::input::send_key_xtest("a", &["ctrl"])?;
                            crate::input::send_type_text_xtest(&value_for_keys)?;
                            crate::input::send_key_xtest("Tab", &[])
                        },
                    )?;
                    return Ok(None);
                }
                let (_, guard) = crate::input::focus_guard::guarded(Some(pid), || {
                    x11_pixel_click_no_focus_steal(&cursor_id, win, lx as i32, ly as i32, 1, 1)?;
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    background_virtual_key(&cursor_id, pid, win, "a", &["ctrl"])?;
                    background_virtual_text(&cursor_id, pid, win, &value_for_keys)?;
                    background_virtual_key(&cursor_id, pid, win, "Tab", &[])?;
                    Ok(())
                })?;
                Ok(guard)
            },
        )
        .await;
        match typed {
            Ok(Ok(guard)) => {
                // Let the toolkit commit on focus-out before reading back.
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                let readback =
                    tokio::task::spawn_blocking(move || crate::atspi::read_value_token(&token))
                        .await
                        .ok()
                        .flatten();
                set_value_result(
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
                )
            }
            Ok(Err(error)) if crate::atspi::native::is_no_such_option(&error) => {
                no_such_option_result(idx, &error)
            }
            Ok(Err(error)) => input_error_result(error),
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

/// Keyboard-fallback `set_value` on a combo box whose options cannot be
/// selected while it is closed: click it open (the real pointer: XTest in
/// the foreground, the session's MPX pointer in the background), activate
/// the option `value` names from the popup, and press Escape if the popup
/// is still open afterwards (an unknown option, or a toolkit that keeps it
/// open), so no popup is left behind.
fn pick_from_popup(
    cursor_id: &str,
    pid: u32,
    token: &crate::atspi::TokenElement,
    (win, lx, ly): (u64, f64, f64),
    value: &str,
    foreground: bool,
) -> anyhow::Result<Option<crate::input::FocusGuardReport>> {
    let close_popup = || -> anyhow::Result<()> {
        if !crate::atspi::options_showing_token(token) {
            return Ok(());
        }
        if foreground {
            crate::input::with_x11_foreground_opts(
                win,
                crate::input::ForegroundOptions::keyboard(),
                || crate::input::send_key_xtest("Escape", &[]),
            )
            .map(|_| ())
        } else {
            background_virtual_key(cursor_id, pid, win, "Escape", &[]).map(|_| ())
        }
    };
    let pick = || -> anyhow::Result<String> {
        let deadline = std::time::Instant::now() + POPUP_OPTIONS_WAIT;
        loop {
            match crate::atspi::activate_option_token(token, value) {
                Ok(Some(name)) => return Ok(name),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Ok(None) => anyhow::bail!("the combo box popup listed no options"),
                Err(error) => return Err(error),
            }
        }
    };
    let open_and_pick = || -> anyhow::Result<()> {
        let picked = pick();
        // Let the toolkit close the popup on activation before checking.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let closed = close_popup();
        picked?;
        closed
    };
    if foreground {
        let (sx, sy) = window_local_to_screen(win, lx, ly)?;
        crate::input::with_x11_foreground_opts(
            win,
            crate::input::ForegroundOptions::pointer(),
            || crate::input::send_click_xtest_desktop(sx.round() as i32, sy.round() as i32, 1, 1),
        )?;
        open_and_pick()?;
        return Ok(None);
    }
    let (_, guard) = crate::input::focus_guard::guarded(Some(pid), || {
        x11_pixel_click_no_focus_steal(cursor_id, win, lx as i32, ly as i32, 1, 1)?;
        open_and_pick()
    })?;
    Ok(guard)
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
mod set_value_tests;
