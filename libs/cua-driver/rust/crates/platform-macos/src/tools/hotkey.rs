use async_trait::async_trait;
use cua_driver_contract::HotkeyInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use libc;
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;

use super::ToolState;

pub struct HotkeyTool {
    state: Arc<ToolState>,
}

impl HotkeyTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "hotkey".into(),
        description:
            "Press a key combination — e.g. `[\"cmd\", \"c\"]` for Copy, \
             `[\"cmd\", \"shift\", \"4\"]` for screenshot selection. Follows the same \
             `delivery_mode` ladder as click/type_text — it does NOT raise the \
             window by default:\n\
             • `background` (default): post the combo to the target pid WITHOUT \
               fronting or raising it — uses the macOS 14+ auth-message envelope so \
               Chromium/Electron accept it as trusted live input. With an AX target, \
               focus that exact element first. No top-level focus steal. \
               `window_id` here only targets the combo; it does not raise.\n\
             • `foreground`: briefly front the window (NSMenu path, < 1 ms via \
               SLPSSetFrontProcessWithOptions) so native menu key-equivalents \
               (Cmd+Z, Cmd+W) dispatch, then restore the prior frontmost — the \
               explicit escalation for menu-bar shortcuts on non-Chromium apps that \
               ignore a background combo. With an AX target or x,y, the focused field \
               receives the chord through the foreground HID queue (needed by native \
               Chromium fields such as the omnibox). Requires window_id.\n\n\
             A combo is never driver-verifiable (no read-back) → effect:\"unverifiable\"; \
             confirm via screenshot. NOTE: a keyboard combo does NOT focus a text \
             field — to type into a backgrounded Electron input, establish real \
             renderer focus with a PIXEL click first, then `type_text`. If an app only \
             accepts paste, call `clipboard_write`, then `clipboard_read` and verify its \
             types (and text when applicable) before selecting or replacing editor content; \
             only then send Cmd+V.\n\n\
             Recognized modifiers: cmd/command, shift, option/alt, ctrl/control, fn. \
             Non-modifier keys use the same vocabulary as `press_key`. Order: \
             modifiers first, one non-modifier last."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["keys"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid": { "type": "integer", "description": "Target process ID." },
                "keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 2,
                    "description": "Modifier(s) and one non-modifier key, e.g. [\"cmd\", \"c\"]."
                },
                "x": { "type": "number", "description": "Screenshot-pixel X — the element px action form: pixel-click there to focus, then send the combo (so e.g. Cmd+V pastes into that field). Pass with y. Use for Chromium/Electron surfaces the background combo can't reach." },
                "y": { "type": "number", "description": "Screenshot-pixel Y (see x)." },
                "window_id": {
                    "type": "integer",
                    "description": "Target window. Required for delivery_mode:\"foreground\" (the NSMenu activation needs a window). Does NOT itself raise the window — raising is gated on delivery_mode."
                },
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                "scope": { "type": "string", "enum": ["window", "desktop"], "default": "window", "description": "Use desktop with no pid/window_id to send the chord to the frontmost application." },
                "delivery_mode": cua_driver_core::tool_schema::delivery_mode_schema()
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true,
    })
}

/// Modifier key names — split the keys array into modifiers + base key.
fn is_modifier(k: &str) -> bool {
    matches!(
        k.to_lowercase().as_str(),
        "cmd" | "command" | "shift" | "option" | "alt" | "ctrl" | "control" | "fn"
    )
}

const HOTKEY_FOCUS_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(200);
const HOTKEY_FOCUS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

fn focus_hotkey_element(pid: i32, element_ptr: usize) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + HOTKEY_FOCUS_TIMEOUT;
    loop {
        crate::input::ax_actions::focus_element(element_ptr)?;
        if crate::input::ax_actions::is_element_focused(pid, element_ptr) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("requested hotkey element did not become focused");
        }
        std::thread::sleep(HOTKEY_FOCUS_POLL_INTERVAL);
    }
}

fn screen_sharing_modifier_delivery_error(
    is_screen_sharing: bool,
    has_modifiers: bool,
    foreground: bool,
    window_id: Option<u32>,
) -> Option<ToolResult> {
    if !is_screen_sharing || !has_modifiers || (foreground && window_id.is_some()) {
        return None;
    }
    Some(
        ToolResult::error(
            "Screen Sharing modifier hotkeys require delivery_mode:\"foreground\" and window_id \
             so Cua Driver can deliver physical modifier transitions safely.",
        )
        .with_structured(serde_json::json!({
            "code": "SCREEN_SHARING_REQUIRES_FOREGROUND_HID",
            "effect": "refused",
            "escalation": {
                "recommended": "foreground",
                "reason": "Screen Sharing does not forward modifier state from background \
                           PID-routed base-key events.",
                "requires": ["window_id"]
            }
        })),
    )
}

#[async_trait]
impl Tool for HotkeyTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<HotkeyInput>("hotkey", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let raw_keys = input.keys;
            if raw_keys.len() < 2 {
                return ToolResult::error("hotkey.keys must contain at least two keys.")
                    .with_structured(serde_json::json!({ "code": "invalid_arguments" }));
            }
            let modifiers: Vec<String> = raw_keys
                .iter()
                .filter(|key| is_modifier(key))
                .cloned()
                .collect();
            let Some(key) = raw_keys.iter().rev().find(|key| !is_modifier(key)).cloned() else {
                return ToolResult::error(
                    "keys must include at least one non-modifier key for desktop hotkey",
                );
            };
            let display = raw_keys.join("+");
            let result = tokio::task::spawn_blocking(move || {
                let modifier_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                crate::input::keyboard::press_key_global(&key, &modifier_refs)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Pressed desktop hotkey {display}."))
                    .with_structured(serde_json::json!({
                        "scope": "desktop",
                        "path": "hid",
                        "effect": "unverifiable"
                    })),
                Ok(Err(error)) => ToolResult::error(format!("desktop hotkey failed: {error}")),
                Err(error) => ToolResult::error(format!("desktop hotkey task failed: {error}")),
            };
        }

        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };

        if args.get("keys").and_then(|v| v.as_array()).is_none() {
            return ToolResult::error("Missing required parameter: keys");
        }
        let raw_keys = args.str_array("keys");

        if raw_keys.is_empty() {
            return ToolResult::error("keys must be a non-empty array of strings.");
        }

        // Split: modifiers are all entries that are modifier names; base key is everything else.
        // Typically: last non-modifier is the key; all others are modifiers.
        let modifiers: Vec<String> = raw_keys
            .iter()
            .filter(|k| is_modifier(k))
            .cloned()
            .collect();
        let non_modifiers: Vec<String> = raw_keys
            .iter()
            .filter(|k| !is_modifier(k))
            .cloned()
            .collect();

        if non_modifiers.is_empty() {
            return ToolResult::error(
                "keys must include at least one non-modifier key (e.g. \"c\" in [\"cmd\", \"c\"]).",
            );
        }

        // Use the last non-modifier key; if there are multiple, treat earlier ones as extra keys.
        let key = non_modifiers.last().unwrap().clone();
        let key_display = raw_keys.join("+");
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg,
            "hotkey",
        ) {
            Ok(resolved) => resolved,
            Err(error) => return error,
        };
        let (element_index, window_id) = match resolved {
            cua_driver_core::element_token::ResolvedElement::None => (None, window_id_arg),
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id,
                element_index,
                via_token: _,
            } => (Some(element_index), window_id),
        };
        // delivery_mode gates whether we raise: background (default) never fronts
        // the window — passing window_id only targets the combo. foreground is the
        // explicit NSMenu-activation rung for menu shortcuts that ignore a
        // background combo (matches click/type_text).
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        let fg = delivery_mode.is_foreground();
        let px = args.get("x").and_then(|value| value.as_f64());
        let py = args.get("y").and_then(|value| value.as_f64());
        if px.is_some() && py.is_some() && element_index.is_some() {
            return ToolResult::error(
                "Pass either element_index (ax) or x,y (px) to hotkey, not both.",
            );
        }

        let element_guard = if let (Some(index), Some(window_id)) = (element_index, window_id) {
            match self
                .state
                .element_cache
                .get_element_retained(pid, window_id, index)
            {
                Some(guard) => Some(guard),
                None => {
                    return ToolResult::error(format!(
                        "Element index {index} not found. Call get_window_state first."
                    ));
                }
            }
        } else {
            None
        };
        let element_ptr = element_guard.as_ref().map(|guard| guard.as_ptr());

        let screen_sharing_target = crate::input::keyboard::is_screen_sharing_pid(pid);
        if let Some(error) = screen_sharing_modifier_delivery_error(
            screen_sharing_target,
            !modifiers.is_empty(),
            fg,
            window_id,
        ) {
            return error;
        }

        // ── Exact-target background gate (macOS background input v1) ──
        // A window-addressed background combo is process-scoped transport: it
        // must prove exact delivery to the requested window (fresh AXWindows
        // membership, not minimized/hidden, no competing same-pid keyboard
        // destination) BEFORE anything is sent — including the px focus
        // click. delivery_mode:"foreground" stays the caller's explicit last
        // resort and is not gated here.
        let _mutation_lease = if !fg {
            if let Some(wid) = window_id {
                match super::gate_background_window_action(
                    pid,
                    wid,
                    element_ptr,
                    cua_driver_core::background_input::BackgroundAction::GenericKey,
                )
                .await
                {
                    Ok(lease) => Some(lease),
                    Err(refusal_result) => return refusal_result,
                }
            } else {
                None
            }
        } else {
            None
        };

        // Web-content AX nodes can acknowledge AXFocused without moving the
        // renderer's real first responder. That makes a direct focus write an
        // unsafe oracle for Chromium/WebKit hotkeys: the following PID/HID
        // chord can still land on the renderer's remembered control. Resolve
        // the requested AX node's exact center and reuse the proven PX focus
        // ladder for web areas. The request remains snapshot-bound AX
        // targeting; only the focus transport falls back through a hit-test
        // and, on the explicit foreground rung, a real click when required.
        let web_ax_focus_xy =
            if let (Some(ptr), Some(wid), Some(index)) = (element_ptr, window_id, element_index) {
                let is_web = tokio::task::spawn_blocking(move || {
                    super::type_text::target_in_web_area(pid, Some((ptr, Some(index))), Some(wid))
                })
                .await
                .unwrap_or(true);
                if is_web {
                    tokio::task::spawn_blocking(move || unsafe {
                        let (screen_x, screen_y) = crate::ax::bindings::element_screen_center(
                            ptr as crate::ax::bindings::AXUIElementRef,
                        )?;
                        let frame = super::px_frame::resolve_window_px_frame(wid).ok()?;
                        Some((
                            (screen_x - frame.bounds.x) * frame.scale,
                            (screen_y - frame.bounds.y) * frame.scale,
                        ))
                    })
                    .await
                    .unwrap_or(None)
                } else {
                    None
                }
            } else {
                None
            };

        // PX form, plus the web-content AX fallback above: focus the field
        // before sending the combo. Foreground delivery still needs to front
        // the target for the chord itself because the focus helper restores
        // the previous app before returning.
        let coordinate_focus = {
            let focus_xy = match ((px, py), web_ax_focus_xy) {
                ((Some(cx), Some(cy)), _) => Some((cx, cy)),
                ((None, None), Some(center)) => Some(center),
                _ => None,
            };
            if let Some((cx, cy)) = focus_xy {
                let from_zoom = args
                    .get("from_zoom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Err(e) = super::focus_by_pixel(
                    &self.state,
                    pid,
                    window_id,
                    cx,
                    cy,
                    fg,
                    args.opt_str("session"),
                    args.opt_str("_session_id"),
                    from_zoom,
                    _mutation_lease.as_ref(),
                )
                .await
                {
                    return e;
                }
                true
            } else {
                false
            }
        };

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Hotkeys like Cmd+N, Cmd+W, Cmd+T explicitly open/close
        // windows. The NSMenu path also briefly activates the target via
        // SLPSSetFrontProcessWithOptions which can race the wildcard
        // suppressor — wrapping ensures both side-effects are observed
        // and the prior frontmost is restored if the activation lingers.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "hotkey.CGEvent",
            || async move {
                tokio::task::spawn_blocking(move || {
                    let m: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                    match (fg, coordinate_focus, window_id, element_ptr) {
                        // Chrome's native omnibox and Chromium/Electron inputs
                        // require a genuine foreground HID chord. Keep the exact
                        // target frontmost until both key events are consumed;
                        // otherwise Cmd+A/Cmd+V can be silently ignored.
                        (true, true, Some(wid), _) => {
                            crate::input::skylight::with_foreground_hid_activation(
                                pid as libc::pid_t,
                                wid,
                                || {
                                    if screen_sharing_target {
                                        crate::input::keyboard::press_key_bare_global(&key, &m)
                                    } else {
                                        crate::input::keyboard::press_key_global(&key, &m)
                                    }
                                },
                            )?;
                            Ok(())
                        }
                        // An AX-addressed chord has the same renderer-focus
                        // requirement as the px form. Activate the exact window,
                        // establish and confirm the requested child focus after
                        // activation, then use the guarded global HID queue.
                        (true, false, Some(wid), Some(ptr)) => {
                            crate::input::skylight::with_foreground_hid_activation(
                                pid as libc::pid_t,
                                wid,
                                || {
                                    focus_hotkey_element(pid, ptr)?;
                                    crate::input::keyboard::press_key_bare_global(&key, &m)
                                },
                            )?;
                            Ok(())
                        }
                        // Screen Sharing is an input forwarder: modifier flags
                        // on a PID-routed base-key event are not relayed to the
                        // guest. Emit the physical modifier down/base/up
                        // sequence through the guarded foreground HID path.
                        (true, false, Some(wid), None)
                            if crate::input::keyboard::is_screen_sharing_pid(pid) =>
                        {
                            crate::input::skylight::with_foreground_hid_activation(
                                pid as libc::pid_t,
                                wid,
                                || crate::input::keyboard::press_key_bare_global(&key, &m),
                            )?;
                            Ok(())
                        }
                        // foreground rung: briefly front the window so NSMenu key
                        // equivalents dispatch, then restore prior frontmost.
                        (true, false, Some(wid), None) => {
                            crate::input::skylight::with_menu_shortcut_activation(
                                pid as libc::pid_t,
                                wid,
                                || crate::input::keyboard::hotkey_no_auth(pid, &key, &m),
                            )?;
                            Ok(())
                        }
                        // background (default): auth-envelope post to the pid, no
                        // raise — even when window_id was supplied for targeting.
                        (false, false, _, Some(ptr)) => {
                            focus_hotkey_element(pid, ptr)?;
                            crate::input::keyboard::hotkey(pid, &key, &m)
                        }
                        _ => crate::input::keyboard::hotkey(pid, &key, &m),
                    }
                })
                .await
            },
        )
        .await;

        let changes = super::finish_window_observation(snapshot, &args).await;

        match result {
            Ok(Ok(())) => {
                let label = if fg {
                    " (delivery_mode:foreground)"
                } else {
                    ""
                };
                // A combo is never read-back-verifiable. On the background rung,
                // point the agent at the foreground escalation for menu shortcuts
                // an app drops in the background — same contract as type_text.
                let mut structured = serde_json::json!({
                    "path": if fg { "key_events_fg" } else { "key_events" },
                    "verified": false,
                    "effect": "unverifiable",
                });
                if !fg && window_id.is_some() {
                    structured["escalation"] = serde_json::json!({
                        "recommended": "foreground",
                        "reason": "a background combo didn't land? menu key-equivalents \
                                   often need the window fronted — re-call with \
                                   delivery_mode:\"foreground\". (To type into a field, \
                                   pixel-click to focus then type_text instead.)"
                    });
                }
                ToolResult::text(format!(
                    "Pressed {key_display} on pid {pid}{label}.{}",
                    changes.result_suffix()
                ))
                .with_structured(structured)
            }
            Ok(Err(e)) => ToolResult::error(format!("hotkey failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_contract_accepts_snapshot_bound_ax_targets() {
        let properties = def().input_schema["properties"]
            .as_object()
            .expect("hotkey properties");
        for field in ["element_index", "element_token", "snapshot_id"] {
            assert!(properties.contains_key(field), "missing {field} schema");
        }
    }

    #[test]
    fn screen_sharing_modifier_hotkeys_fail_closed_without_foreground_window() {
        for (foreground, window_id) in [(false, None), (false, Some(7)), (true, None)] {
            let result = screen_sharing_modifier_delivery_error(true, true, foreground, window_id)
                .expect("unsafe Screen Sharing modifier route must be refused");
            assert_eq!(result.is_error, Some(true));
            let structured = result.structured_content.unwrap();
            assert_eq!(structured["code"], "SCREEN_SHARING_REQUIRES_FOREGROUND_HID");
            assert_eq!(structured["effect"], "refused");
            assert_eq!(structured["escalation"]["recommended"], "foreground");
            assert_eq!(structured["escalation"]["requires"][0], "window_id");
        }
        assert!(screen_sharing_modifier_delivery_error(true, true, true, Some(7)).is_none());
        assert!(screen_sharing_modifier_delivery_error(true, false, false, None).is_none());
        assert!(screen_sharing_modifier_delivery_error(false, true, false, None).is_none());
    }
}
