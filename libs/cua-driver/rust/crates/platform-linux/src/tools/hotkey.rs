use async_trait::async_trait;
use cua_driver_contract::HotkeyInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    background_key_route, focus_by_pixel, focus_hyprland_foreground, focus_nested_inject_target,
    foreground_budget, foreground_hyprland_action, foreground_hyprland_key,
    foreground_hyprland_refusal, hyprland_foreground, input_error_result,
    invalid_pointer_arguments, is_chromium_embedder, is_modifier, isolated_hyprland_action,
    isolated_hyprland_background, isolated_hyprland_refusal, key_route_result,
    position_named_session_keyboard_cursor, prove_token_element, resolve_cursor_key,
    resolve_element_local_coords, spawn_blocking_bounded, stale_observed_element,
    unavailable_chromium_background, unavailable_gtk_keyboard_background,
    unavailable_wayland_focused_input_background, unavailable_webkit_keyboard_background,
    wm_chord_background, KeyRoute, ToolState,
};

// ── hotkey ────────────────────────────────────────────────────────────────────

pub struct HotkeyTool {
    pub(crate) state: Arc<ToolState>,
}
static HOTKEY_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for HotkeyTool {
    fn def(&self) -> &ToolDef {
        HOTKEY_DEF.get_or_init(|| ToolDef {
            name: "hotkey".into(),
            description: "Press a combination of keys simultaneously, e.g. [\"ctrl\",\"c\"] for Copy. \
                Delivered to the target window without focus steal (real key events from a virtual \
                master keyboard on X11); target does NOT need to be frontmost. Window-manager chords \
                cannot reach the WM that way: alt+F4 on a window target closes that window through \
                the WM (_NET_CLOSE_WINDOW, effect confirmed when it unmaps); super+*, alt+tab, \
                ctrl+alt+* and other WM bindings are refused with code wm_chord_unavailable — use \
                bring_to_front / list_windows / kill_app or delivery_mode:\"foreground\" instead.".into(),
            input_schema: json!({
                "type":"object","required":["keys"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID. Omit with scope \"desktop\" to send the chord to the focused application."},
                    "window_id":{"type":"integer","description":"Window id from list_windows. Required with x/y or element_index; optional with element_token (the token carries it)."},
                    "keys":{"type":"array","items":{"type":"string"},"minItems":2,
                        "description":"Modifier(s) + one non-modifier key, e.g. [\"ctrl\",\"c\"]."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "x":{"type":"number","description":"Screenshot-pixel X — the element px action form: pixel-click there to focus, then send the combo (so e.g. Ctrl+V pastes into that field). Pass with y. Use for Chromium/Electron surfaces the background combo can't reach."},
                    "y":{"type":"number","description":"Screenshot-pixel Y (see x)."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"Use \"desktop\" with no pid to send the chord to the focused application. Default \"window\"."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop") && args.get("pid").is_none() {
            let input = match parse_typed_projection::<HotkeyInput>("hotkey", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let keys = input.keys;
            if keys.len() < 2 {
                return ToolResult::error("hotkey.keys must contain at least two keys.")
                    .with_structured(json!({ "code": "invalid_arguments" }));
            }
            let modifiers: Vec<String> = keys
                .iter()
                .filter(|key| is_modifier(key))
                .cloned()
                .collect();
            let Some(key) = keys.iter().rev().find(|key| !is_modifier(key)).cloned() else {
                return ToolResult::error("keys must include at least one non-modifier key.");
            };
            let display = keys.join("+");
            position_named_session_keyboard_cursor(&self.state, &args, 0, 0, None, None, false)
                .await;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_focused" } else { "xtest" };
            let result =
                spawn_blocking_bounded("desktop hotkey", foreground_budget(1), move || {
                    if wayland {
                        crate::wayland::hotkey_focused(&keys)
                    } else {
                        let refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                        crate::input::send_key_xtest(&key, &refs)
                    }
                })
                .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Pressed desktop hotkey {display}."))
                    .with_structured(
                        json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                    ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let mut pid = args.u64_or("pid", 0) as u32;
        let window_id_arg = args.opt_u64("window_id");
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(resolved) => resolved,
            Err(error) => return error,
        };
        let token = match prove_token_element(pid, &resolved, "hotkey").await {
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

        // Parse keys array (preferred) or fall back to legacy key+modifiers.
        let (key, mods) = if let Some(arr) = args.get("keys").and_then(|v| v.as_array()) {
            let keys: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            let modifiers: Vec<String> = keys.iter().filter(|k| is_modifier(k)).cloned().collect();
            let non_mods: Vec<String> = keys.iter().filter(|k| !is_modifier(k)).cloned().collect();
            if non_mods.is_empty() {
                return ToolResult::error("keys must include at least one non-modifier key.");
            }
            (non_mods.last().unwrap().clone(), modifiers)
        } else if let Some(k) = args.opt_str("key") {
            let mods: Vec<String> = args.str_array("modifiers");
            (k, mods)
        } else {
            return ToolResult::error(
                "Provide 'keys' array (e.g. [\"ctrl\",\"c\"]) or 'key'+'modifiers' parameters.",
            );
        };

        let key_display = format!("{}+{}", mods.join("+"), key);
        let key_for_wayland = key.clone();
        let mods_for_wayland = mods.clone();
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);

        if isolated_hyprland_background(delivery) {
            if xid_opt.is_none() {
                return isolated_hyprland_refusal(
                    "an exact window_id is required for isolated hotkeys",
                );
            }
            if resolved_element_index.is_some()
                || args.get("x").is_some()
                || args.get("y").is_some()
            {
                return isolated_hyprland_refusal("isolated hotkeys address the exact top-level; first click the child explicitly");
            }
            if let Some(keys) = args.get("keys").and_then(Value::as_array) {
                if keys
                    .iter()
                    .filter(|value| value.as_str().is_some_and(|key| !is_modifier(key)))
                    .count()
                    != 1
                    || keys.iter().any(|value| !value.is_string())
                {
                    return isolated_hyprland_refusal(
                        "isolated hotkeys require exactly one non-modifier key",
                    );
                }
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
                "hotkey",
                "x and y must be passed together (a pixel focus point), or neither",
            );
        }
        if px.is_some() && resolved_element_index.is_some() {
            return ToolResult::error(
                "Pass either element_token (ax) or x,y (px) to hotkey, not both.",
            );
        }

        if hyprland_foreground(delivery) {
            if xid_opt.is_none() {
                return foreground_hyprland_refusal("an exact window_id is required");
            }
            if let Some(keys) = args.get("keys").and_then(Value::as_array) {
                if keys.iter().any(|key| !key.is_string())
                    || keys
                        .iter()
                        .filter(|key| key.as_str().is_some_and(|key| !is_modifier(key)))
                        .count()
                        != 1
                {
                    return foreground_hyprland_refusal(
                        "hotkeys require exactly one non-modifier key",
                    );
                }
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

        if crate::wayland::is_inject_mode() {
            if let Err(error) =
                focus_nested_inject_target(pid, xid, token.clone(), px.zip(py)).await
            {
                return error;
            }
            let mut chord = mods.clone();
            chord.push(key.clone());
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::inject_hotkey(pid, xid, &chord)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Pressed hotkey '{key_display}' (focus-free via cua-compositor)."
                )),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }

        // Chromium renderer shortcuts need DOM focus established by a real
        // pointer event. Component.GrabFocus alone can report success while
        // the renderer still drops the virtual-keyboard chord.
        if delivery.is_foreground()
            && crate::wayland::wayland_input_enabled()
            && is_chromium_embedder(pid)
        {
            if let Some(element) = token.clone() {
                let coordinates = tokio::task::spawn_blocking(move || {
                    resolve_element_local_coords(pid, &element, Some(xid))
                })
                .await;
                let (_, x, y) = match coordinates {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => return ToolResult::error(error.to_string()),
                    Err(error) => return ToolResult::error(format!("Task error: {error}")),
                };
                if let Err(error) =
                    focus_by_pixel(&self.state, pid, Some(xid), (x, y), true, &args, false).await
                {
                    return error;
                }
            }
        }

        // Foreground: GrabFocus the addressed element up front. Background
        // does it inside `background_key_route`, under the focus guard.
        if let Some(element) = token.clone().filter(|_| delivery.is_foreground()) {
            let element_index = element.index;
            let focused =
                tokio::task::spawn_blocking(move || crate::atspi::focus_token(pid, &element)).await;
            match focused {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => {
                    return ToolResult::error(format!(
                        "AT-SPI Component.GrabFocus returned false for element {element_index}"
                    ))
                }
                Ok(Err(error)) => {
                    return stale_observed_element("hotkey", element_index, &error)
                        .unwrap_or_else(|| ToolResult::error(error.to_string()))
                }
                Err(error) => return ToolResult::error(format!("Task error: {error}")),
            }
        }

        // ── px form: pixel-click to focus, then the combo acts on the focused field ──
        // (e.g. Ctrl+V into a Chromium input). Reuses click's translation +
        // delivery_mode; after it, deliver the combo via the plain background path
        // (the focus-click already fronted when fg).
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
        let deliver_fg = delivery.is_foreground();
        let cursor_id = resolve_cursor_key(&args);

        let result = spawn_blocking_bounded(
            "hotkey",
            foreground_budget(1),
            move || -> anyhow::Result<KeyRoute> {
                if crate::wayland::wayland_input_enabled() {
                    // Native Wayland: route the modifier combo through wtype's
                    // -M/-k/-m sequence — the closest equivalent to the X11
                    // state-mask path. window_id is irrelevant once focused.
                    let mut combo: Vec<String> = mods_for_wayland.clone();
                    combo.push(key_for_wayland.clone());
                    return crate::wayland::hotkey(xid, &combo).map(|()| KeyRoute::Synthetic);
                }
                let m: Vec<&str> = mods.iter().map(String::as_str).collect();
                // foreground: activate the target first, then inject the accelerator
                // as REAL key events via XTest. Synthetic XSendEvent keys
                // (`send_key`) are dropped by GTK/Qt/Chromium/Firefox, so the
                // foreground rung must use XTest, which reaches the focused window.
                if deliver_fg {
                    return crate::interaction::with_x11_foreground(
                        xid,
                        crate::input::ForegroundOptions::keyboard(),
                        || crate::input::send_key_xtest(&key, &m),
                    )
                    .map(|((), report)| KeyRoute::Foreground(report));
                }
                background_key_route(&cursor_id, pid, xid, px_target, &key, &m, token.as_ref())
            },
        )
        .await;
        let mode_label = if deliver_fg {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok(route)) => key_route_result(
                &format!("Pressed {key_display} on pid {pid}"),
                route,
                mode_label,
            ),
            Ok(Err(e)) => input_error_result(e),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
