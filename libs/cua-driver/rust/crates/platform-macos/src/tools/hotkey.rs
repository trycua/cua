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
               Chromium/Electron accept it as trusted live input. No focus steal. \
               `window_id` here only targets the combo; it does not raise.\n\
             • `foreground`: briefly front the window (NSMenu path, < 1 ms via \
               SLPSSetFrontProcessWithOptions) so native menu key-equivalents \
               (Cmd+Z, Cmd+W) dispatch, then restore the prior frontmost — the \
               explicit escalation for menu-bar shortcuts on non-Chromium apps that \
               ignore a background combo. Requires window_id.\n\n\
             A combo is never driver-verifiable (no read-back) → effect:\"unverifiable\"; \
             confirm via screenshot. NOTE: a keyboard combo does NOT focus a text \
             field — to type into a backgrounded Electron input, establish real \
             renderer focus with a PIXEL click first, then `type_text` (do not reach \
             for a clipboard + Cmd+V dance).\n\n\
             Recognized modifiers: cmd/command, shift, option/alt, ctrl/control, fn. \
             Non-modifier keys use the same vocabulary as `press_key`. Order: \
             modifiers first, one non-modifier last."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["keys"],
            "properties": {
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
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

#[async_trait]
impl Tool for HotkeyTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let _ = &self.state;

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
        let window_id = args.opt_u64("window_id").map(|v| v as u32);
        // delivery_mode gates whether we raise: background (default) never fronts
        // the window — passing window_id only targets the combo. foreground is the
        // explicit NSMenu-activation rung for menu shortcuts that ignore a
        // background combo (matches click/type_text).
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        let fg = delivery_mode.is_foreground();

        // px form: pixel-click to focus, then the combo acts on the focused field
        // (e.g. Cmd+V into a Chromium input). After it, deliver the combo via the
        // plain background path (the focus-click already fronted if fg).
        let px_focus = {
            let px = args.get("x").and_then(|v| v.as_f64());
            let py = args.get("y").and_then(|v| v.as_f64());
            if let (Some(cx), Some(cy)) = (px, py) {
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
                    // px-focus already clicked/fronted the target → deliver background.
                    match (fg && !px_focus, window_id) {
                        // foreground rung: briefly front the window so NSMenu key
                        // equivalents dispatch, then restore prior frontmost.
                        (true, Some(wid)) => {
                            crate::input::skylight::with_menu_shortcut_activation(
                                pid as libc::pid_t,
                                wid,
                                || crate::input::keyboard::hotkey_no_auth(pid, &key, &m),
                            )?;
                            Ok(())
                        }
                        // background (default): auth-envelope post to the pid, no
                        // raise — even when window_id was supplied for targeting.
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
