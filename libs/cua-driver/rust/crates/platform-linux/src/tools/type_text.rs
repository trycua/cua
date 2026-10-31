use async_trait::async_trait;
use cua_driver_contract::TypeTextInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::{fs, sync::Arc};

use super::impl_::{
    announce_keyboard_target, atspi_up, background_keyboard_refusal, background_virtual_text,
    coordinate_frame_is_desktop, coordinate_frame_schema, focus_by_pixel,
    focus_hyprland_foreground, focus_nested_inject_target, foreground_hyprland_refusal,
    foreground_structured, hyprland_foreground, hyprland_input_result, inject_terminal_input,
    input_error_result, invalid_pointer_arguments, is_chromium_embedder, is_popup_keyboard_grab,
    is_terminal_process, is_webkitgtk_embedder, isolated_hyprland_background,
    isolated_hyprland_refusal, isolated_hyprland_result, isolated_hyprland_task_error,
    named_session_cursor_key, position_named_session_keyboard_cursor, prove_token_element,
    resolve_cursor_key, spawn_blocking_bounded, spawn_isolated_hyprland, stale_observed_element,
    typing_budget, unavailable_chromium_background, unavailable_wayland_focused_input_background,
    unavailable_webkit_keyboard_background, values_agree, KeyRoute, ToolState,
};

/// Structured payload for a `type_text` response. Keeps the legacy
/// `path`/`characters`/`verified` fields for back-compat and adds the cross-tool
/// `effect` tri-state. Linux's AT-SPI `insertText` return value acknowledges
/// the method call but does not read the widget value back, so it and every
/// keystroke / XSendEvent / XTest / Wayland rung are `"unverifiable"` (the
/// caller confirms through a separate observation). No escalation is
/// attached to a successful delivery: the public projection renders one as
/// `delivery_failed`, which is wrong for text that landed but was not read
/// back. Mirrors the macOS `type_text` contract.
fn type_text_structured(path: &str, characters: usize, verified: bool) -> Value {
    let s = json!({
        "path": path,
        "characters": characters,
        "verified": verified,
        "effect": if verified { "confirmed" } else { "unverifiable" },
    });
    // No escalation on a successful-but-unverified delivery: the public
    // projection renders a foreground escalation as `delivery_failed`, which
    // made models re-type text that had already landed. Refusals carry their own.
    let _ = verified;
    s
}

/// Structured payload for the Electron/Chromium AX-echo case: the AT-SPI
/// `insertText` rung returned success on a Chromium embedder, but on those
/// surfaces the a11y bridge can accept and echo the write while the renderer
/// never observes it — so a path=="ax" "confirm" is a shim echo, not ground
/// truth. Mirrors the macOS `type_text` `ax_echo_surface` branch: report
/// effect:"unverifiable" + escalation:{recommended:"px"} (a renderer-focus
/// problem — pixel-focus the field, not foreground-activate it).
fn type_text_structured_electron(text_len: usize) -> Value {
    json!({
        "path": "ax",
        "characters": text_len,
        "verified": false,
        "effect": "unverifiable",
        "escalation": {
            "recommended": "px",
            "reason": "Electron/web surface — the AX write was echoed but the \
                       renderer may not have observed it. Confirm via the screenshot; \
                       if it didn't land, re-type with the element px action (pass x,y \
                       to pixel-focus the field, then type)."
        }
    })
}

/// [`type_text_ax_result`] for an element-addressed insert: the snapshot
/// object is read back (as `set_value` does) and, when the typed text is now
/// part of its value, the result is `effect: confirmed` with a
/// `value_readback` evidence item instead of `unverifiable`.
async fn type_text_ax_result_verified(
    pid: u32,
    token: crate::atspi::TokenElement,
    text: &str,
    route: &str,
) -> ToolResult {
    let text_len = text.chars().count();
    if is_chromium_embedder(pid) {
        return type_text_ax_result(pid, text_len, route);
    }
    let idx = token.index;
    let readback = tokio::task::spawn_blocking(move || crate::atspi::read_value_token(&token))
        .await
        .ok()
        .flatten();
    type_text_readback_result(pid, idx, text, route, readback)
}

/// The typed text is confirmed when the element's fresh value contains it
/// (an insert at the caret) or agrees with it numerically (a spin button
/// that reformats "-40" as "-40.0").
fn type_text_readback_result(
    pid: u32,
    idx: usize,
    text: &str,
    route: &str,
    readback: Option<String>,
) -> ToolResult {
    let text_len = text.chars().count();
    match readback {
        Some(seen)
            if !text.trim().is_empty() && (seen.contains(text) || values_agree(text, &seen)) =>
        {
            let shown: String = seen.chars().take(200).collect();
            ToolResult::text(format!(
                "Typed {text_len} character(s) into element [{idx}] ({route}); it reads back '{shown}'."
            ))
            .with_structured(json!({
                "path": "ax",
                "characters": text_len,
                "verified": true,
                "effect": "confirmed",
                "readback": shown,
                "evidence": [{
                    "kind": "value_readback",
                    "detail": format!("element [{idx}] reads back the typed text"),
                }],
            }))
        }
        Some(seen) => {
            let mut result = type_text_ax_result(pid, text_len, route);
            let shown: String = seen.chars().take(200).collect();
            if let Some(structured) = result.structured_content.as_mut() {
                structured["readback"] = json!(shown);
            }
            result
                .content
                .push(cua_driver_core::protocol::Content::text(format!(
                    "The element reads back '{shown}', which does not contain the typed text; \
                 confirm with get_window_state."
                )));
            result
        }
        None => type_text_ax_result(pid, text_len, route),
    }
}

#[cfg(test)]
mod type_text_readback_tests {
    use super::*;

    #[test]
    fn typed_text_is_confirmed_only_when_read_back() {
        let confirmed =
            type_text_readback_result(1, 3, "-40", "via targeted AT-SPI", Some("-40.0".into()));
        let s = confirmed.structured_content.clone().unwrap();
        assert_eq!(s["effect"], "confirmed");
        assert_eq!(s["verified"], true);
        assert_eq!(s["evidence"][0]["kind"], "value_readback");
        let contained = type_text_readback_result(1, 3, "ab", "r", Some("xxabyy".into()));
        assert_eq!(contained.structured_content.unwrap()["effect"], "confirmed");
        let mismatch = type_text_readback_result(1, 3, "ab", "r", Some("zz".into()));
        let s = mismatch.structured_content.unwrap();
        assert_eq!(s["effect"], "unverifiable");
        assert_eq!(s["readback"], "zz");
        let blind = type_text_readback_result(1, 3, "ab", "r", None);
        assert_eq!(blind.structured_content.unwrap()["effect"], "unverifiable");
    }
}

/// Build the success `ToolResult` for an AT-SPI insert. The EditableText
/// method's boolean is a delivery acknowledgement, not a fresh value readback,
/// so native widgets remain `unverifiable`. Chromium embedders additionally
/// recommend the pixel rung because their accessibility bridge can acknowledge
/// a write the renderer never observes.
fn type_text_ax_result(pid: u32, text_len: usize, route: &str) -> ToolResult {
    if is_chromium_embedder(pid) {
        return ToolResult::text(format!(
            "📨 Sent (unverified) {text_len} character(s) ({route}). — Electron/web \
             surface: the AX layer accepts and echoes the write but the renderer may \
             not have observed it, so the driver cannot confirm via AX. Verify via the \
             screenshot; if it didn't land, re-type with the px form (pass x,y to \
             pixel-focus the field)."
        ))
        .with_structured(type_text_structured_electron(text_len));
    }
    ToolResult::text(format!("Typed {text_len} character(s) ({route})."))
        .with_structured(type_text_structured("ax", text_len, false))
}

/// Qt5 maps its own GUI library; only then is the synthetic-FocusIn bridge
/// workaround in `type_text` worth two extra AT-SPI walks.
fn is_qt5_process(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/maps"))
        .map(|maps| maps.contains("libQt5Gui") || maps.contains("libQt5Widgets"))
        .unwrap_or(false)
}

#[cfg(test)]
#[test]
fn type_text_routes_isolated_background_before_generic_wayland_refusal() {
    let source = include_str!("type_text.rs");
    let invoke = source
        .rsplit_once("impl Tool for TypeTextTool {")
        .unwrap()
        .1
        .split_once("async fn invoke")
        .unwrap()
        .1;
    let isolated = invoke
        .find("isolated_hyprland_background(delivery)")
        .expect("type_text must select isolated Hyprland background delivery");
    let generic = invoke
        .find("unavailable_wayland_focused_input_background")
        .expect("type_text must retain the generic Wayland refusal");
    assert!(
        isolated < generic,
        "the plugin route must run before the generic focused-input refusal"
    );
    assert!(invoke.contains("execute_background_text"));
}

// ── type_text ─────────────────────────────────────────────────────────────────

pub struct TypeTextTool {
    pub(crate) state: Arc<ToolState>,
}
static TYPE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for TypeTextTool {
    fn def(&self) -> &ToolDef {
        TYPE_DEF.get_or_init(|| ToolDef {
            name: "type_text".into(),
            description: "Type text to a window via XSendEvent (KeyPress/KeyRelease). No focus steal.".into(),
            input_schema: json!({
                "type":"object","required":["text"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID. Omit with scope \"desktop\" to type into the focused application."},
                    "window_id":{"type":"integer","description":"Window id from list_windows. Required with x/y; optional with element_token (the token carries it)."},
                    "text":{"type":"string","description":"Text to type."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "x":{"type":"number","description":"Pixel X of the field to type into — the element px action form. Pass x,y (no element_token) and the tool pixel-clicks there to establish real renderer focus, then types. Use for Chromium/Electron inputs the AX path can't reach. Window-local pixels of the target window's own get_window_state screenshot by default (same convention as click); for get_desktop_state pixels pass scope:\"desktop\" (or coordinate_frame:\"desktop\") — a bare pid+x/y is otherwise misinterpreted as window-local and can focus the wrong widget."},
                    "y":{"type":"number","description":"Pixel Y of the field (see x)."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"Use \"desktop\" with no pid to type into the focused application. Default \"window\"."},
                    "coordinate_frame": coordinate_frame_schema(),
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if (args.opt_str("scope").as_deref() == Some("desktop")
            || coordinate_frame_is_desktop(&args))
            && args.get("pid").is_none()
        {
            let input = match parse_typed_projection::<TypeTextInput>("type_text", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let text =
                cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&input.text)
                    .into_owned();
            position_named_session_keyboard_cursor(&self.state, &args, 0, 0, None, None, false)
                .await;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_focused" } else { "xtest" };
            let result = spawn_blocking_bounded(
                "desktop type_text",
                typing_budget(text.chars().count(), crate::wayland::niri::is_session()),
                move || {
                    if wayland {
                        crate::wayland::type_text_focused(&text)
                    } else {
                        crate::input::send_type_text_xtest(&text)
                    }
                },
            )
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text("Typed text into the focused desktop application.")
                    .with_structured(
                        json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                    ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let mut pid = args.u64_or("pid", 0) as u32;
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Strip trailing agent-protocol closing tags — see
        // cua_driver_core::text_sanitize docs for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let token = match prove_token_element(pid, &resolved, "type_text").await {
            Ok(token) => token,
            Err(refusal) => return refusal,
        };
        let (resolved_elem_idx, resolved_window_id) = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element {
                element_index,
                window_id,
                ..
            } => (Some(*element_index), Some(*window_id)),
            cua_driver_core::element_token::ResolvedElement::None => (None, None),
        };
        let xid_opt = resolved_window_id.or(args.opt_u64("window_id"));

        // Resolve XID: use window_id if given, else first window for pid.
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows = tokio::task::spawn_blocking(move || {
                    crate::x11::list_windows(if pid == 0 { None } else { Some(pid) })
                })
                .await
                .unwrap_or_default();
                // pid omitted: the keys go to the active window, like a
                // physical keyboard would, and the action adopts its pid.
                let chosen = if pid == 0 {
                    let active = crate::x11::active_window();
                    windows
                        .iter()
                        .find(|w| Some(w.xid) == active && w.pid.is_some())
                        .or_else(|| windows.iter().find(|w| w.is_on_screen && w.pid.is_some()))
                } else {
                    // The pid's active window, else its largest on-screen
                    // toplevel: LibreOffice and GIMP own hidden/utility
                    // toplevels that a plain `first()` could pick, and an
                    // unmapped window cannot take the virtual keyboard focus.
                    let active = crate::x11::active_window();
                    windows
                        .iter()
                        .find(|w| Some(w.xid) == active)
                        .or_else(|| {
                            windows
                                .iter()
                                .filter(|w| w.is_on_screen)
                                .max_by_key(|w| u64::from(w.width) * u64::from(w.height))
                        })
                        .or_else(|| windows.first())
                };
                match chosen {
                    Some(w) => {
                        if pid == 0 {
                            pid = w.pid.unwrap_or(0);
                        }
                        w.xid
                    }
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if isolated_hyprland_background(delivery)
            && resolved_elem_idx.is_none()
            && args.get("x").is_none()
            && args.get("y").is_none()
        {
            if xid_opt.is_none() {
                return isolated_hyprland_refusal(
                    "an exact window_id is required for isolated text",
                );
            }
            match crate::wayland::hyprland_input::text_actions(&text) {
                Ok(actions) if !actions.is_empty() => {}
                Ok(_) => return isolated_hyprland_refusal("background text must not be empty"),
                Err(error) => return isolated_hyprland_refusal(error.to_string()),
            }
            position_named_session_keyboard_cursor(&self.state, &args, pid, xid, None, None, true)
                .await;
            let owner = named_session_cursor_key(&args);
            let (_cancellation, dispatch) =
                match spawn_isolated_hyprland(&args, move |cancellation| {
                    crate::wayland::hyprland_input::execute_background_text(
                        owner,
                        pid,
                        xid,
                        &text,
                        cancellation,
                    )
                }) {
                    Ok(dispatch) => dispatch,
                    Err(refusal) => return refusal,
                };
            return match dispatch.await {
                Ok(result) => isolated_hyprland_result(result),
                Err(error) => isolated_hyprland_task_error(error, false),
            };
        }
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }
        if resolved_elem_idx.is_none() {
            if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
                return refusal;
            }
        }

        let px = args.get("x").and_then(|value| value.as_f64());
        let py = args.get("y").and_then(|value| value.as_f64());
        if px.is_some() != py.is_some() {
            return invalid_pointer_arguments(
                "type_text",
                "x and y must be passed together (a pixel focus point), or neither",
            );
        }
        if px.is_some() && resolved_elem_idx.is_some() {
            return ToolResult::error(
                "Pass either element_token (ax) or x,y (px) to type_text, not both.",
            );
        }

        if hyprland_foreground(delivery) {
            if xid_opt.is_none() {
                return foreground_hyprland_refusal("an exact window_id is required");
            }
            // Native editables retain their verifiable, addressed AT-SPI route.
            if !is_chromium_embedder(pid) && !is_webkitgtk_embedder(pid) {
                if let Some(element) = token.clone() {
                    let text_ax = text.clone();
                    if matches!(
                        tokio::task::spawn_blocking(move || crate::atspi::type_into_token(
                            pid, &element, &text_ax
                        ))
                        .await,
                        Ok(Ok(()))
                    ) {
                        position_named_session_keyboard_cursor(
                            &self.state,
                            &args,
                            pid,
                            xid,
                            token.clone(),
                            None,
                            true,
                        )
                        .await;
                        return type_text_ax_result(
                            pid,
                            text.chars().count(),
                            "via targeted AT-SPI",
                        );
                    }
                }
            }
            match crate::wayland::hyprland_input::text_actions(&text) {
                Ok(actions) if !actions.is_empty() => {}
                Ok(_) => return foreground_hyprland_refusal("foreground text must not be empty"),
                Err(error) => return foreground_hyprland_refusal(error.to_string()),
            }
            if let Err(error) =
                focus_hyprland_foreground(&self.state, &args, pid, xid, token.clone(), px.zip(py))
                    .await
            {
                return error;
            }
            if named_session_cursor_key(&args).is_some() {
                if px.is_none() {
                    position_named_session_keyboard_cursor(
                        &self.state,
                        &args,
                        pid,
                        xid,
                        token.clone(),
                        None,
                        true,
                    )
                    .await;
                }
            } else {
                announce_keyboard_target(&args, pid, xid, token.clone(), px.zip(py)).await;
            }
            let owner = named_session_cursor_key(&args);
            let (_cancellation, dispatch) =
                match spawn_isolated_hyprland(&args, move |cancellation| {
                    crate::wayland::hyprland_input::execute_foreground_text(
                        owner,
                        pid,
                        xid,
                        &text,
                        cancellation,
                    )
                }) {
                    Ok(dispatch) => dispatch,
                    Err(_) => {
                        return foreground_hyprland_refusal(
                            "authenticated admitted lifecycle required",
                        )
                    }
                };
            return hyprland_input_result(
                match dispatch.await {
                    Ok(result) => result,
                    Err(error) => Err(crate::wayland::hyprland_input::unknown_dispatch(
                        error.into(),
                        0,
                    )),
                },
                true,
            );
        }

        position_named_session_keyboard_cursor(
            &self.state,
            &args,
            pid,
            xid,
            token.clone(),
            px.zip(py),
            true,
        )
        .await;

        let text_len = text.chars().count();
        // Native toolkit editables have a stronger focus-free route than raw
        // compositor keyboard injection. Keep Chromium/WebKit on real key events
        // because their accessibility bridges may echo a write that never reaches
        // renderer-owned state.
        if let Some(element) = token.clone().filter(|_| {
            crate::wayland::is_inject_mode()
                && !is_chromium_embedder(pid)
                && !is_webkitgtk_embedder(pid)
        }) {
            let text_at = text.clone();
            let target = element.clone();
            let targeted = tokio::task::spawn_blocking(move || {
                crate::atspi::type_into_token(pid, &target, &text_at)
            })
            .await;
            if let Ok(Ok(())) = targeted {
                return type_text_ax_result_verified(pid, element, &text, "via targeted AT-SPI")
                    .await;
            }
        }
        // The private nested compositor can target the owning Wayland client
        // directly. Establish widget-local focus first, without changing the
        // compositor's focused toplevel, so keys reach the addressed control.
        if crate::wayland::is_inject_mode() {
            if let Err(error) =
                focus_nested_inject_target(pid, xid, token.clone(), px.zip(py)).await
            {
                return error;
            }
            let text_w = text.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::inject_type_text(pid, xid, &text_w)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (focus-free via cua-compositor)."
                ))
                .with_structured(type_text_structured(
                    "key_events",
                    text_len,
                    false,
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // ── px form: focus by pixel-click, then type into the now-focused element ──
        // Pass x,y (no element_token) for an *element px action*: pixel-click
        // the field to give the renderer the real keyboard focus the AT-SPI path
        // can't, then fall through to the focused-element type path below (it
        // escalates AT-SPI → key events and lands once focused). Reuses ClickTool's
        // exact coordinate translation + delivery_mode.
        if let (Some(cx), Some(cy)) = (px, py) {
            if let Some(refusal) = unavailable_webkit_keyboard_background(pid, delivery) {
                return refusal;
            }
            let from_zoom = args.bool_or("from_zoom", false);
            if let Err(e) = focus_by_pixel(
                &self.state,
                pid,
                Some(xid),
                (cx, cy),
                delivery.is_foreground(),
                &args,
                from_zoom,
            )
            .await
            {
                return e;
            }
            // resolved_elem_idx stays None → the type path below writes to the now-
            // focused element via the background key / AT-SPI rung.
        }

        if resolved_elem_idx.is_some() {
            if let Some(refusal) = unavailable_webkit_keyboard_background(pid, delivery) {
                return refusal;
            }
        }

        // Renderer EditableText writes can update the accessible value without
        // emitting the DOM input event. For an explicit foreground request on
        // native Wayland, focus the named field and send real keyboard input so
        // Chromium/WebKit observe the same event sequence as a user.
        if delivery.is_foreground()
            && crate::wayland::wayland_input_enabled()
            && (is_chromium_embedder(pid) || is_webkitgtk_embedder(pid))
        {
            if let Some(element) = token.clone() {
                let idx = element.index;
                let text_w = text.clone();
                let result = spawn_blocking_bounded(
                    "type_text",
                    typing_budget(text_w.chars().count(), crate::wayland::niri::is_session()),
                    move || {
                        crate::interaction::with_target_foreground(
                            pid,
                            xid,
                            crate::interaction::RestoreMode::RestorePrevious,
                            || {
                                if !crate::atspi::focus_token(pid, &element)? {
                                    anyhow::bail!(
                                    "AT-SPI Component.GrabFocus returned false for element {idx}"
                                );
                                }
                                crate::wayland::type_text_focused(&text_w)
                            },
                        )
                    },
                )
                .await;
                return match result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "Typed {text_len} character(s) (via Wayland virtual-keyboard)."
                    ))
                    .with_structured(type_text_structured(
                        "key_events",
                        text_len,
                        false,
                    )),
                    Ok(Err(error)) => stale_observed_element("type_text", idx, &error)
                        .unwrap_or_else(|| ToolResult::error(error.to_string())),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }
        }

        // AX addressing names one exact editable. Try this focus-free route
        // before native Wayland keyboard injection, which can only target the
        // compositor's globally focused surface.
        if let Some(element) = token.clone() {
            let text_at = text.clone();
            let target = element.clone();
            let targeted = tokio::task::spawn_blocking(move || {
                crate::atspi::type_into_token(pid, &target, &text_at)
            })
            .await;
            if let Ok(Err(error)) = &targeted {
                // The observed object is gone: nothing was written to it.
                if let Some(stale) = stale_observed_element("type_text", element.index, error) {
                    return stale;
                }
            }
            match targeted {
                Ok(Ok(())) => {
                    return type_text_ax_result_verified(
                        pid,
                        element,
                        &text,
                        "via targeted AT-SPI",
                    )
                    .await;
                }
                Ok(Err(_)) | Err(_)
                    if !delivery.is_foreground() && crate::wayland::wayland_input_enabled() =>
                {
                    return crate::input::delivery::background_unavailable_error(
                        crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                    );
                }
                _ => {}
            }
        }

        // Native Wayland: keys go to the *focused* surface (no pid/window
        // targeting in the protocol). Type via the virtual-keyboard tool; pair
        // with a prior `click`/`activate` to focus the intended window.
        if crate::wayland::wayland_input_enabled() {
            if !delivery.is_foreground() {
                return crate::input::delivery::background_unavailable_error(
                    crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                );
            }
            let text_w = text.clone();
            let element = token.clone();
            let result = spawn_blocking_bounded(
                "type_text",
                typing_budget(text_w.chars().count(), crate::wayland::niri::is_session()),
                move || {
                    crate::interaction::with_target_foreground(
                        pid,
                        xid,
                        crate::interaction::RestoreMode::RestorePrevious,
                        || {
                            if let Some(element) = &element {
                                if !crate::atspi::focus_token(pid, element)? {
                                    anyhow::bail!(
                                        "AT-SPI Component.GrabFocus returned false for element {}",
                                        element.index
                                    );
                                }
                            }
                            crate::wayland::type_text_focused(&text_w)
                        },
                    )
                },
            )
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (via Wayland virtual-keyboard)."
                ))
                .with_structured(type_text_structured(
                    "key_events",
                    text_len,
                    false,
                )),
                Ok(Err(e)) => resolved_elem_idx
                    .and_then(|idx| stale_observed_element("type_text", idx, &e))
                    .unwrap_or_else(|| ToolResult::error(e.to_string())),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Terminal short-circuit: when the target window's WM_CLASS marks it
        // as a terminal emulator (Ghostty / Alacritty / kitty / …), skip the
        // AT-SPI path entirely. Terminals expose an editable text area that
        // AT-SPI `insertText` would aim at, but the write never reaches the
        // pty, so the user sees "type acknowledged but nothing appeared".
        // Try pty-master injection first (most reliable), then XTest key
        // synthesis. Either way the structured response reports
        // `path: "key_events"` so callers can verify the route taken.
        let pid_is_terminal = is_terminal_process(pid);
        let wm_class_is_terminal =
            tokio::task::spawn_blocking(move || crate::terminal::is_terminal_window(xid))
                .await
                .unwrap_or(false);
        if pid_is_terminal || wm_class_is_terminal {
            let text_len = text.chars().count();
            let text_t = text.clone();
            let foreground = delivery.is_foreground();
            let cursor_id = resolve_cursor_key(&args);
            let result = spawn_blocking_bounded(
                "type_text terminal",
                typing_budget(text_len, crate::wayland::niri::is_session()),
                move || -> anyhow::Result<(&'static str, Option<crate::input::KeyboardDeliveryReport>)> {
                // pty-master injection is preferred — it skips the X event
                // queue entirely. Falls through to key events if the terminal
                // isn't reachable that way (descendant pty unresolvable).
                if inject_terminal_input(pid, xid, &text_t)? {
                    return Ok(("pty", None));
                }
                if foreground {
                    crate::interaction::with_x11_foreground(xid, crate::input::ForegroundOptions::pointer(), || {
                        crate::input::send_type_text_xtest(&text_t)
                    }).map(|(value, _report)| value)?;
                    return Ok(("key_events_fg", None));
                }
                // Background: real key events through the XI2 virtual master
                // keyboard (xterm/gnome-terminal accept them like physical input).
                match background_text_route(&cursor_id, pid, xid, &text_t)? {
                    Some(KeyRoute::Mpx(report)) => Ok((crate::input::MPX_UINPUT_PATH, Some(report))),
                    _ => Ok(("background_unavailable", None)),
                }
            })
            .await;
            return match result {
                Ok(Ok(("background_unavailable", _))) => background_keyboard_refusal(atspi_up()),
                Ok(Ok((_, Some(report)))) => type_text_mpx_result(text_len, report),
                Ok(Ok((path, None))) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (terminal emulator: pty/XTest key events)."
                ))
                .with_structured(type_text_structured(path, text_len, false)),
                Ok(Err(e)) => input_error_result(e),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        // Foreground means the caller explicitly permits activation: behave like
        // a user. Activate the window and confirm it holds the input focus,
        // GrabFocus the indexed element when one was given, then type REAL key
        // events via XTest. The AT-SPI EditableText probes below are skipped on
        // purpose: on large trees (LibreOffice) they stall for the full bus
        // timeout, Chromium/WebKitGTK echo them without the renderer seeing the
        // input, and for a spreadsheet cell / canvas they would target the
        // wrong widget anyway. The whole transaction is deadline-bounded.
        if delivery.is_foreground() {
            let text_f = text.clone();
            let element = token.clone();
            let result = spawn_blocking_bounded(
                "foreground type_text",
                typing_budget(text_len, crate::wayland::niri::is_session()),
                move || {
                    crate::interaction::with_x11_foreground(
                        xid,
                        crate::input::ForegroundOptions::keyboard(),
                        || {
                            if let Some(element) = &element {
                                if !crate::atspi::focus_token(pid, element)? {
                                    anyhow::bail!(
                                        "AT-SPI Component.GrabFocus returned false for element {}",
                                        element.index
                                    );
                                }
                            }
                            crate::input::send_type_text_xtest(&text_f)
                        },
                    )
                },
            )
            .await;
            return match result {
                Ok(Ok(((), report))) => {
                    let mut extra = serde_json::Map::new();
                    extra.insert("characters".into(), json!(text_len));
                    ToolResult::text(format!(
                        "Typed {text_len} character(s) as real key events \
                         (delivery_mode=foreground, focus_after={}); not verified — \
                         confirm with a screenshot.",
                        report.focus_after.as_str()
                    ))
                    .with_structured(foreground_structured(
                        "key_events_fg",
                        report,
                        extra,
                    ))
                }
                Ok(Err(e)) => input_error_result(e),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // With a focus-free real keyboard available, type where the widget
        // focus is — exactly what a physical keyboard does after the click that
        // focused the field. The blind AT-SPI editable search below can pick a
        // different editable of the app (it "succeeded" into GIMP's wrong entry
        // and VS Code's search box); it stays as the fallback for hosts without
        // the MPX route and for explicit element targets.
        if !delivery.is_foreground()
            && resolved_elem_idx.is_none()
            && crate::input::real_keyboard_input_available()
        {
            let text_b = text.clone();
            let cursor_id = resolve_cursor_key(&args);
            let result = spawn_blocking_bounded(
                "background type_text",
                typing_budget(text_len, crate::wayland::niri::is_session()),
                move || background_text_route(&cursor_id, pid, xid, &text_b),
            )
            .await;
            match result {
                Ok(Ok(Some(KeyRoute::Mpx(report)))) => {
                    return type_text_mpx_result(text_len, report)
                }
                Ok(Err(e)) => return input_error_result(e),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
                Ok(Ok(_)) => {}
            }
        }

        // Prefer the focused widget — the element the user just clicked. If a
        // NON-editable input holds keyboard focus (a spreadsheet cell, a
        // terminal, a canvas), the focus-free AT-SPI editable search below would
        // grab the wrong field (e.g. gnumeric's name box instead of the selected
        // cell, or skip a terminal entirely), so synth-type into the focused
        // widget instead: terminals via pty injection, everything else via
        // XSendEvent to the focused window. A focused *editable* (Some(true)) or
        // nothing focused (None) falls through to the existing AT-SPI-first flow.
        let focus_kind = tokio::task::spawn_blocking(move || {
            crate::atspi::focused_is_editable(pid).ok().flatten()
        })
        .await
        .ok()
        .flatten();
        if focus_kind == Some(false) {
            if !delivery.is_foreground() {
                // A spreadsheet cell / canvas / terminal holds the widget focus:
                // real key events aimed at this window are the only thing that
                // types there. The XI2 virtual master keyboard delivers them
                // without touching the user's focus; terminals take the pty.
                let text_b = text.clone();
                let cursor_id = resolve_cursor_key(&args);
                let result = spawn_blocking_bounded(
                    "background type_text",
                    typing_budget(text_len, crate::wayland::niri::is_session()),
                    move || -> anyhow::Result<Option<KeyRoute>> {
                        if inject_terminal_input(pid, xid, &text_b)? {
                            return Ok(Some(KeyRoute::Terminal));
                        }
                        background_text_route(&cursor_id, pid, xid, &text_b)
                    },
                )
                .await;
                return match result {
                    Ok(Ok(Some(KeyRoute::Terminal))) => ToolResult::text(format!(
                        "Typed {text_len} character(s) into the focused terminal (pty)."
                    ))
                    .with_structured(type_text_structured("key_events", text_len, false)),
                    Ok(Ok(Some(KeyRoute::Mpx(report)))) => type_text_mpx_result(text_len, report),
                    Ok(Ok(_)) => background_keyboard_refusal(atspi_up()),
                    Ok(Err(e)) => input_error_result(e),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            let text_f = text.clone();
            let result = spawn_blocking_bounded(
                "foreground type_text",
                typing_budget(text_len, crate::wayland::niri::is_session()),
                move || {
                    if inject_terminal_input(pid, xid, &text_f)? {
                        return Ok(());
                    }
                    // XTest (real input to the focused window), NOT XSendEvent: GTK/Qt
                    // drop synthetic key events, so a spreadsheet cell / canvas would
                    // stay empty. The click that gave this widget focus already put it
                    // under the X input focus, so XTest-to-focus lands correctly.
                    crate::interaction::with_x11_foreground(
                        xid,
                        crate::input::ForegroundOptions::pointer(),
                        || crate::input::send_type_text_xtest(&text_f),
                    )
                    .map(|(value, _report)| value)
                },
            )
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) into the focused widget."
                ))
                .with_structured(type_text_structured(
                    "key_events",
                    text_len,
                    false,
                )),
                Ok(Err(e)) => input_error_result(e),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Try AT-SPI EditableText first (focus-free, works for Qt6/GTK4).
        let text_clone = text.clone();
        let atspi_result =
            tokio::task::spawn_blocking(move || crate::atspi::type_into_editable(pid, &text_clone))
                .await;

        match atspi_result {
            Ok(Ok(())) => {
                // AT-SPI succeeded — focus-free typing worked (Qt6, GTK4, etc.)!
                // Electron/Chromium can echo this write without the renderer
                // observing it, so the confirm is suppressed there (mirrors macOS).
                return type_text_ax_result(pid, text_len, "via AT-SPI");
            }
            _ => {
                // AT-SPI failed (no editable exposed). Qt5 doesn't expose widgets
                // when unfocused, so try the synthetic-focus workaround.
            }
        }

        // Qt5 workaround: send synthetic FocusIn to make Qt5's AT-SPI bridge
        // expose the widget tree, type via AT-SPI, then send FocusOut.
        // This doesn't change the X11 active window, so the test's focus check passes.
        let text_clone2 = text.clone();
        let qt5_result = spawn_blocking_bounded(
            "Qt5 type_text",
            typing_budget(text_len, crate::wayland::niri::is_session()),
            move || {
                if !is_qt5_process(pid) {
                    anyhow::bail!("not a Qt5 process; synthetic-FocusIn bridge workaround skipped");
                }
                // Send FocusIn to trigger Qt5's bridge
                crate::input::send_focus_in(xid)?;
                std::thread::sleep(std::time::Duration::from_millis(100));

                // Try AT-SPI again now that widgets should be exposed
                let result = crate::atspi::type_into_editable(pid, &text_clone2);

                // Restore state with FocusOut
                crate::input::send_focus_out(xid)?;

                result
            },
        )
        .await;

        match qt5_result {
            Ok(Ok(())) => {
                return type_text_ax_result(pid, text_len, "via AT-SPI with focus workaround");
            }
            _ => {
                // AT-SPI still didn't work. Fall back to X11 XSendEvent.
            }
        }

        // Track which path the final fallback chain took, so the
        // structured response stays honest. The closure can't borrow
        // a local mutably across `spawn_blocking`, so funnel the
        // decision through the success type instead.
        // delivery_mode: background (default) = focus-free AT-SPI / XSendEvent;
        // foreground = activate the window (EWMH), then synthesize REAL key
        // events to it via XTest — the escalation when background didn't land
        // (e.g. a GTK dialog whose widget ignores synthetic XSendEvent keys).
        let cursor_id = resolve_cursor_key(&args);
        let result = spawn_blocking_bounded(
            "type_text",
            typing_budget(text_len, crate::wayland::niri::is_session()),
            move || -> anyhow::Result<(&'static str, Option<crate::input::KeyboardDeliveryReport>)> {
            // Terminals: write to the pty master (focus-free, below the toolkit).
            if inject_terminal_input(pid, xid, &text)? {
                return Ok(("key_events", None));
            }
            // GUI apps: X11 only routes keystrokes to the *focused* toplevel's
            // focused widget, so background XSendEvent typing doesn't land. Fill
            // the editable field via AT-SPI instead — focus-free and toolkit-
            // agnostic. Fall back to Tk send or XSendEvent when no a11y field is exposed.
            if crate::atspi::insert_text(pid, &text).unwrap_or(false) {
                return Ok(("ax", None));
            }
            // Tk apps: use Tk's `send` command (no AT-SPI bridge, so AT-SPI above
            // returned false). This is the Tk-specific override, like CDP for Chromium.
            if crate::input::inject_tk_send(&text).unwrap_or(false) {
                return Ok(("key_events", None));
            }
            if delivery.is_foreground() {
                crate::interaction::with_x11_foreground(xid, crate::input::ForegroundOptions::pointer(), || {
                    crate::input::send_type_text_xtest(&text)
                }).map(|(value, _report)| value)?;
                return Ok(("key_events_fg", None));
            }
            // No a11y editable, no pty: real key events through the XI2 virtual
            // master keyboard are the last focus-free route.
            match background_text_route(&cursor_id, pid, xid, &text)? {
                Some(KeyRoute::Mpx(report)) => Ok((crate::input::MPX_UINPUT_PATH, Some(report))),
                _ => Ok(("background_unavailable", None)),
            }
        })
        .await;
        let mode_label = if delivery.is_foreground() {
            "foreground"
        } else {
            "background"
        };
        match result {
            // AT-SPI's boolean acknowledges the EditableText call; it is not a
            // fresh value readback. Keep the result unverifiable and apply the
            // stricter Chromium escalation where appropriate.
            Ok(Ok(("ax", _))) => type_text_ax_result(
                pid,
                text_len,
                &format!("via X11, delivery_mode={mode_label}"),
            ),
            Ok(Ok(("background_unavailable", _))) => background_keyboard_refusal(atspi_up()),
            Ok(Ok((_, Some(report)))) => type_text_mpx_result(text_len, report),
            Ok(Ok((path, None))) => ToolResult::text(format!(
                "Typed {text_len} character(s) (via X11, delivery_mode={mode_label})."
            ))
            .with_structured(type_text_structured(path, text_len, false)),
            Ok(Err(e)) => input_error_result(e),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// Background text delivery through the XI2 virtual master keyboard. `None`
/// when the host has no such route (Xvfb, no `/dev/uinput`); a uinput
/// permission error propagates so the tool can attach the operator hint.
fn background_text_route(
    cursor_id: &str,
    pid: u32,
    xid: u64,
    text: &str,
) -> anyhow::Result<Option<KeyRoute>> {
    if !crate::input::real_keyboard_input_available() {
        return Ok(None);
    }
    match crate::input::focus_guard::guarded(Some(pid), || {
        background_virtual_text(cursor_id, pid, xid, text)
    }) {
        Ok((mut report, guard)) => {
            report.focus_guard = guard;
            Ok(Some(KeyRoute::Mpx(report)))
        }
        Err(error) if crate::input::is_uinput_unavailable(&error) => Err(error),
        Err(error) if is_popup_keyboard_grab(&error) => Err(error),
        Err(error) => {
            tracing::warn!(pid, "MPX keyboard text delivery failed: {error}");
            Err(error.context("virtual master keyboard delivery failed"))
        }
    }
}

/// `type_text` result for a delivery through the virtual master keyboard.
fn type_text_mpx_result(
    text_len: usize,
    report: crate::input::KeyboardDeliveryReport,
) -> ToolResult {
    let mut structured = type_text_structured(report.path, text_len, false);
    for (key, value) in report.to_json().as_object().into_iter().flatten() {
        structured[key] = value.clone();
    }
    structured["delivery_mode"] = json!("background");
    let outcome = if report.skipped_characters.is_empty() {
        format!("Typed {text_len} character(s)")
    } else {
        "Partial typing result".to_owned()
    };
    let mut text = format!(
        "{outcome} as real key events {} (delivery_mode=background); \
         not verified — confirm with a screenshot.",
        report.route_phrase()
    );
    if !report.skipped_characters.is_empty() {
        text.push_str(&format!(
            " {} character(s) had no keycode in the current keymap and were skipped: {:?}.",
            report.skipped_characters.len(),
            report.skipped_characters.iter().collect::<String>()
        ));
    }
    if !report.virtual_focus_held || !report.delivery_confirmed {
        structured["effect"] = json!("suspected_noop");
        text.push_str(
            " Warning: the target lost the virtual keyboard's focus or the server did \
             not confirm the last key; the text may not have landed.",
        );
    }
    text.push_str(&report.delivery_notes());
    let result = if report.skipped_characters.is_empty() {
        ToolResult::text(text)
    } else {
        ToolResult::error(text)
    };
    result.with_structured(structured)
}

#[cfg(test)]
#[test]
fn mpx_type_text_with_skipped_characters_is_an_error() {
    let report = |skipped_characters: Vec<char>| crate::input::KeyboardDeliveryReport {
        virtual_focus_held: true,
        core_focus_unchanged: true,
        delivery_confirmed: true,
        key_events: 2,
        skipped_characters,
        focus_guard: None,
        released_stuck: Vec::new(),
        path: crate::input::MPX_UINPUT_PATH,
        grab_popup: None,
    };
    assert_eq!(type_text_mpx_result(2, report(Vec::new())).is_error, None);
    let skipped = type_text_mpx_result(2, report(vec!['約']));
    assert_eq!(skipped.is_error, Some(true));
    assert!(!format!("{:?}", skipped.content).contains("Typed 2 character(s)"));
    assert_eq!(
        skipped.structured_content.unwrap()["skipped_characters"],
        "約"
    );
}
