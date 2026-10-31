use async_trait::async_trait;
use cua_driver_contract::PressKeyInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    background_key_route, coordinate_frame_is_desktop, coordinate_frame_schema, focus_by_pixel,
    focus_hyprland_foreground, focus_nested_inject_target, foreground_budget,
    foreground_hyprland_action, foreground_hyprland_key, foreground_hyprland_refusal,
    hyprland_foreground, inject_terminal_input, input_error_result, invalid_pointer_arguments,
    isolated_hyprland_action, isolated_hyprland_background, isolated_hyprland_refusal,
    key_route_result, position_named_session_keyboard_cursor, prove_token_element,
    resolve_cursor_key, spawn_blocking_bounded, split_key_combo, stale_observed_element,
    unavailable_chromium_background, unavailable_gtk_keyboard_background,
    unavailable_wayland_focused_input_background, unavailable_webkit_keyboard_background,
    wm_chord_background, KeyRoute, ToolState,
};

/// True when `key` names the Enter key in the shared X keysym vocabulary
/// (`key_name_to_keysym`). The terminal pty short-circuit below applies to
/// every spelling of that physical key — `enter`, `return`, any case — so an
/// agent following the documented key names cannot silently lose the keypress
/// on a terminal window (terminals discard synthetic XSendEvent keys).
fn is_enter_key(key: &str) -> bool {
    crate::input::key_name_to_keysym(key).ok() == Some(0xFF0D)
}

// ── press_key ─────────────────────────────────────────────────────────────────

pub struct PressKeyTool {
    pub(crate) state: Arc<ToolState>,
}
static PRESS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

/// Turn a modified `press_key` request into the chord an underlying keyboard
/// path expects when it takes a single key and no separate modifier list.
///
/// Returns `None` when there is nothing to promote, so the caller keeps the
/// plain single-key route. Dropping the modifiers instead is not a silent
/// no-op: the bare key still reaches the app and is typed as a character.
fn press_key_chord(mods: &[String], key: &str) -> Option<Vec<String>> {
    if mods.is_empty() {
        return None;
    }
    let mut chord = mods.to_vec();
    chord.push(key.to_owned());
    Some(chord)
}

#[cfg(test)]
mod press_key_tests {
    use super::is_enter_key;
    use super::press_key_chord;

    #[test]
    fn unmodified_press_stays_on_the_single_key_route() {
        assert_eq!(press_key_chord(&[], "return"), None);
    }

    #[test]
    fn enter_key_gate_accepts_the_documented_enter_spelling() {
        assert!(is_enter_key("enter"));
        assert!(is_enter_key("Enter"));
        assert!(is_enter_key("ENTER"));
    }

    #[test]
    fn enter_key_gate_accepts_the_documented_return_spelling() {
        // `key_name_to_keysym` resolves "return" and "enter" to the same
        // physical key (XK_Return, 0xFF0D); the pty short-circuit must not
        // drop a documented spelling on terminals (they discard synthetic
        // XSendEvent keys, so a dropped short-circuit loses the keypress).
        assert!(is_enter_key("return"));
        assert!(is_enter_key("Return"));
        assert!(is_enter_key("RETURN"));
    }

    #[test]
    fn enter_key_gate_rejects_non_enter_keys() {
        assert!(!is_enter_key("tab"));
        assert!(!is_enter_key("escape"));
        assert!(!is_enter_key("space"));
        assert!(!is_enter_key(""));
    }

    #[test]
    fn modifiers_are_promoted_to_a_chord_in_order() {
        assert_eq!(
            press_key_chord(&["ctrl".to_owned()], "s"),
            Some(vec!["ctrl".to_owned(), "s".to_owned()])
        );
        assert_eq!(
            press_key_chord(&["ctrl".to_owned(), "shift".to_owned()], "t"),
            Some(vec!["ctrl".to_owned(), "shift".to_owned(), "t".to_owned()])
        );
    }
}

#[async_trait]
impl Tool for PressKeyTool {
    fn def(&self) -> &ToolDef {
        PRESS_DEF.get_or_init(|| ToolDef {
            name: "press_key".into(),
            description: "Press a key via XSendEvent to a window. No focus steal.".into(),
            input_schema: json!({
                "type":"object","required":["key"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID. Omit with scope \"desktop\" to send the key to the focused application."},
                    "window_id":{"type":"integer","description":"Window id from list_windows. Required with x/y or element_index; optional with element_token (the token carries it)."},
                    "key":{"type":"string","description":"Key name: enter/return, tab, escape, space, backspace, delete, insert, home, end, pageup, pagedown, up, down, left, right, f1-f12, or any single ASCII character."},
                    "modifiers":{"type":"array","items":{"type":"string"},"description":"Modifier keys held while the key is pressed: ctrl, shift, alt, super."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "x":{"type":"number","description":"Pixel X — the element px action form: pixel-click there to focus, then send the key. Use when the key must go to a Chromium/Electron surface the AX path can't focus. Pass with y, no element_token. Window-local pixels by default (same convention as click); for get_desktop_state pixels pass scope:\"desktop\" (or coordinate_frame:\"desktop\")."},
                    "y":{"type":"number","description":"Pixel Y (see x)."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"Use \"desktop\" with no pid to send the key to the focused application. Default \"window\"."},
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
            let input = match parse_typed_projection::<PressKeyInput>("press_key", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let key = input.key;
            let display = key.clone();
            let modifiers = input.modifiers.unwrap_or_default();
            position_named_session_keyboard_cursor(&self.state, &args, 0, 0, None, None, false)
                .await;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_focused" } else { "xtest" };
            let result = tokio::task::spawn_blocking(move || {
                if wayland && modifiers.is_empty() {
                    crate::wayland::press_key_focused(&key)
                } else if wayland {
                    let mut keys = modifiers;
                    keys.push(key);
                    crate::wayland::hotkey_focused(&keys)
                } else {
                    let refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                    crate::input::send_key_xtest(&key, &refs)
                }
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Pressed desktop key '{display}'."))
                    .with_structured(
                        json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                    ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let mut pid = args.u64_or("pid", 0) as u32;
        let key = match args.require_str("key") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mut mods: Vec<String> = args.str_array("modifiers");
        let (combo_mods, key) = split_key_combo(&key);
        mods.extend(combo_mods);

        // Resolve the element token into both its owning window and exact
        // child. Foreground delivery establishes child focus inside the
        // verified top-level activation transaction; background XSendEvent
        // retains the historical direct-window path.
        let window_id_arg = args.opt_u64("window_id");
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let token = match prove_token_element(pid, &resolved, "press_key").await {
            Ok(token) => token,
            Err(refusal) => return refusal,
        };
        let resolved_element_index = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let xid_opt = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => {
                Some(*window_id)
            }
            cua_driver_core::element_token::ResolvedElement::None => window_id_arg,
        };
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

        // ── px form: pixel-click to focus, then the key goes to the focused element ──
        // Reuses click's translation + delivery_mode; after it, deliver via the plain
        // background path (the focus-click already handled fronting when fg). Pass x,y
        // (no element_token) for Chromium/Electron surfaces the AX path can't focus.
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if isolated_hyprland_background(delivery) {
            if xid_opt.is_none() {
                return isolated_hyprland_refusal(
                    "an exact window_id is required for isolated keys",
                );
            }
            if resolved_element_index.is_some()
                || args.get("x").is_some()
                || args.get("y").is_some()
            {
                return isolated_hyprland_refusal(
                    "isolated keys address the exact top-level; first click the child explicitly",
                );
            }
            position_named_session_keyboard_cursor(&self.state, &args, pid, xid, None, None, false)
                .await;
            return isolated_hyprland_action(
                &args,
                pid,
                xid,
                crate::wayland::hyprland_input::Action::Key {
                    key,
                    modifiers: mods,
                },
            )
            .await;
        }
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }

        if let Some(refusal) = unavailable_webkit_keyboard_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_keyboard_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
            return refusal;
        }
        if let Some(result) = wm_chord_background(pid, xid, &key, &mods, delivery).await {
            return result;
        }

        let px = args.get("x").and_then(|value| value.as_f64());
        let py = args.get("y").and_then(|value| value.as_f64());
        if px.is_some() != py.is_some() {
            return invalid_pointer_arguments(
                "press_key",
                "x and y must be passed together (a pixel focus point), or neither",
            );
        }
        if px.is_some() && resolved_element_index.is_some() {
            return ToolResult::error(
                "Pass either element_token (ax) or x,y (px) to press_key, not both.",
            );
        }

        if hyprland_foreground(delivery) {
            if xid_opt.is_none() {
                return foreground_hyprland_refusal("an exact window_id is required");
            }
            let action = match foreground_hyprland_key(key, mods) {
                Ok(action) => action,
                Err(error) => return error,
            };
            if let Err(error) =
                focus_hyprland_foreground(&self.state, &args, pid, xid, token.clone(), px.zip(py))
                    .await
            {
                return error;
            }
            if px.is_none() {
                position_named_session_keyboard_cursor(
                    &self.state,
                    &args,
                    pid,
                    xid,
                    token.clone(),
                    None,
                    false,
                )
                .await;
            }
            return foreground_hyprland_action(&args, pid, xid, action).await;
        }

        position_named_session_keyboard_cursor(
            &self.state,
            &args,
            pid,
            xid,
            token.clone(),
            px.zip(py),
            false,
        )
        .await;

        // Nested cua-compositor addresses the owning Wayland client directly.
        // Preserve legacy modifiers by promoting the request to a chord.
        if crate::wayland::is_inject_mode() {
            if let Err(error) =
                focus_nested_inject_target(pid, xid, token.clone(), px.zip(py)).await
            {
                return error;
            }
            let result = match press_key_chord(&mods, &key) {
                None => {
                    let key_w = key.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::wayland::inject_press_key(pid, xid, &key_w)
                    })
                    .await
                }
                Some(chord) => {
                    tokio::task::spawn_blocking(move || {
                        crate::wayland::inject_hotkey(pid, xid, &chord)
                    })
                    .await
                }
            };
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Pressed key '{key}' (focus-free via cua-compositor)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let px_target = {
            if let (Some(cx), Some(cy)) = (px, py) {
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
                Some((cx.round() as i32, cy.round() as i32))
            } else {
                None
            }
        };

        // Native Wayland: send the key to the focused surface via virtual-keyboard.
        // `wayland::press_key` carries no modifier list, so a modified request has
        // to become a chord here — otherwise the modifiers are dropped and the
        // bare key is typed as a character (Ctrl+S inserts a literal "s").
        if crate::wayland::wayland_input_enabled() {
            let key_w = key.clone();
            let chord = press_key_chord(&mods, &key);
            let element = token.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::with_target_foreground(pid, xid, || {
                    if let Some(element) = &element {
                        if !crate::atspi::focus_token(pid, element)? {
                            anyhow::bail!(
                                "AT-SPI Component.GrabFocus returned false for element {}",
                                element.index
                            );
                        }
                    }
                    match chord {
                        Some(keys) => crate::wayland::hotkey_focused(&keys),
                        None => crate::wayland::press_key_focused(&key_w),
                    }
                })
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Pressed key '{key}' (via Wayland virtual-keyboard)."
                )),
                Ok(Err(e)) => resolved_element_index
                    .and_then(|idx| stale_observed_element("press_key", idx, &e))
                    .unwrap_or_else(|| ToolResult::error(e.to_string())),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let key_for_task = key.clone();
        let cursor_id = resolve_cursor_key(&args);
        // Foreground delivery is one atomic activate-and-XTest transaction.
        // A preceding PX click establishes internal widget focus, but that
        // click restores the prior top-level before returning.
        let deliver_fg = delivery.is_foreground();
        let result = spawn_blocking_bounded(
            "press_key",
            foreground_budget(1),
            move || -> anyhow::Result<KeyRoute> {
            if resolved_element_index.is_none()
                && mods.is_empty()
                && is_enter_key(&key_for_task)
                && inject_terminal_input(pid, xid, "\n")?
            {
                return Ok(KeyRoute::Terminal);
            }
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            // foreground: activate the window first, then inject a REAL key via
            // XTest. Synthetic XSendEvent keys (`send_key`) are dropped by
            // GTK/Qt/Chromium/Firefox, so the foreground rung must use XTest —
            // it delivers to the now-focused window. background = real key
            // events through the XI2 virtual master keyboard (no focus steal),
            // else direct XSendEvent for apps that accept it.
            if deliver_fg {
                return crate::input::with_x11_foreground_opts(
                    xid,
                    crate::input::ForegroundOptions::keyboard(),
                    || {
                        if let Some(element) = &token {
                            let element_index = element.index;
                            if !crate::atspi::focus_token(pid, element)? {
                                anyhow::bail!(
                                    "AT-SPI Component.GrabFocus returned false for element {element_index}"
                                );
                            }
                        }
                        crate::input::send_key_xtest(&key_for_task, &m)
                    },
                )
                .map(|((), report)| KeyRoute::Foreground(report));
            }
            background_key_route(
                &cursor_id,
                pid,
                xid,
                px_target,
                &key_for_task,
                &m,
                token.as_ref(),
            )
        })
        .await;
        let mode_label = if deliver_fg {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok(route)) => key_route_result(&format!("Pressed key '{key}'"), route, mode_label),
            Ok(Err(e)) => input_error_result(e),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
