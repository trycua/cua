use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use libc;
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;

use super::ToolState;

pub struct PressKeyTool {
    state: Arc<ToolState>,
}

impl PressKeyTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "press_key".into(),
        description: "Press and release a single key, delivered to the target pid via \
            CGEventPostToPid. Follows the same `delivery_mode` ladder as click/type_text \
            — it does NOT raise the window by default:\n\
            • `background` (default): post to the pid WITHOUT fronting/raising — the \
              auth-message path (Chromium-safe). With element_index it focuses that AX \
              element first. `window_id` only targets; it does not raise.\n\
            • `foreground`: briefly front the window (NSMenu path, < 1 ms) so native \
              menu key-equivalents dispatch, then restore prior frontmost — the explicit \
              escalation for menu shortcuts an app drops in the background. Requires \
              window_id (and no element_index).\n\n\
            A key press is never driver-verifiable → effect:\"unverifiable\"; confirm via \
            screenshot. Key names: return, tab, escape, up/down/left/right, space, delete, \
            home, end, pageup, pagedown, f1-f12, plus any letter or digit. \
            Modifiers array: cmd, shift, option/alt, ctrl, fn.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["key"],
            "properties": {
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
                "pid": { "type": "integer" },
                "key": { "type": "string", "description": "Key name: return, tab, escape, up, down, etc." },
                "modifiers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Modifier keys: cmd, shift, option/alt, ctrl, fn."
                },
                "window_id": { "type": "integer", "description": "Target window. Required for delivery_mode:\"foreground\". Does NOT itself raise the window — raising is gated on delivery_mode." },
                "element_index": { "type": "integer" },
                "element_token": { "type": "string", "description": "Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded." },
                "x": { "type": "number", "description": "Screenshot-pixel X — the element px action form: pixel-click there to focus, then send the key. Use when the key must go to a Chromium/Electron surface the AX path can't focus. Pass with y, no element_index." },
                "y": { "type": "number", "description": "Screenshot-pixel Y (see x)." },
                "scope": { "type": "string", "enum": ["window", "desktop"], "default": "window", "description": "Use desktop with no pid/window_id to send the key to the frontmost application." },
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

#[async_trait]
impl Tool for PressKeyTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let key = match args.require_str("key") {
                Ok(value) => value,
                Err(error) => return error,
            };
            let modifiers: Vec<String> = args.str_array("modifiers");
            let key_for_input = key.clone();
            let result = tokio::task::spawn_blocking(move || {
                let modifier_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                crate::input::keyboard::press_key_global(&key_for_input, &modifier_refs)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Pressed '{key}' on the desktop."))
                    .with_structured(serde_json::json!({
                        "scope": "desktop",
                        "path": "hid",
                        "effect": "unverifiable"
                    })),
                Ok(Err(error)) => ToolResult::error(format!("desktop press_key failed: {error}")),
                Err(error) => ToolResult::error(format!("desktop press_key task failed: {error}")),
            };
        }
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let key_raw = match args.require_str("key") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mut modifiers: Vec<String> = args.str_array("modifiers");
        // Surface 6: element_token / element_index precedence resolution.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg,
            "press_key",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id) = match resolved {
            cua_driver_core::element_token::ResolvedElement::None => (None, window_id_arg),
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: wid,
                element_index: idx,
                via_token: _,
            } => (Some(idx), wid),
        };

        // Remap "+" / "plus" → "=" + Shift (same physical key on US layout).
        let key = if key_raw == "+" || key_raw == "plus" {
            if !modifiers.iter().any(|m| m.eq_ignore_ascii_case("shift")) {
                modifiers.push("shift".to_string());
            }
            "=".to_string()
        } else {
            key_raw.clone()
        };
        let display_key = key_raw.clone();
        // delivery_mode gates the raise: background (default) never fronts the
        // window (auth-envelope post, even with window_id); foreground is the
        // explicit NSMenu-activation rung. Matches click/type_text/hotkey.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        let fg = delivery_mode.is_foreground();

        // px form: pixel-click to focus, then the key goes to the focused element.
        // Reuses click's translation + delivery_mode; after it, deliver via the
        // plain background path (the focus-click already handled fronting if fg).
        let px_focus = {
            let px = args.get("x").and_then(|v| v.as_f64());
            let py = args.get("y").and_then(|v| v.as_f64());
            if let (Some(cx), Some(cy)) = (px, py) {
                if element_index.is_some() {
                    return ToolResult::error(
                        "Pass either element_index (ax) or x,y (px) to press_key, not both.",
                    );
                }
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

        // Resolve the pre-focus element pointer (if requested) outside
        // the suppression closure — only the focus_element() write itself
        // needs to run under suppression, the cache lookup does not.
        // Retain out of the cache so a concurrent get_window_state can't free
        // the element before the suppressed focus below dereferences it
        // (use-after-free → daemon crash). Guard lives to method end.
        let pre_focus_guard = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            self.state.element_cache.get_element_retained(pid, wid, idx)
        } else {
            None
        };
        let pre_focus_ptr: Option<usize> = pre_focus_guard.as_ref().map(|g| g.as_ptr());

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Single-key presses can fire autocomplete (Return on a search
        // box opens a results popover) or trigger menu shortcuts that
        // open windows. Wrapping mirrors the hotkey path.
        //
        // The AX focus_element() pre-write also runs inside the closure
        // so any reflex activations it triggers are caught by both the
        // wildcard snapshot suppressor and the targeted FocusGuard lease.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "press_key.CGEvent",
            || async move {
                // Pre-focus the element under suppression so its
                // side-effects are captured by the snapshot + lease.
                if let Some(element_ptr) = pre_focus_ptr {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::input::ax_actions::focus_element(element_ptr)
                    })
                    .await;
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }

                tokio::task::spawn_blocking(move || {
                    let m: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                    // foreground rung: NSMenu activation (raise+restore), only when
                    // explicitly requested and addressing a window without an element.
                    // Skipped when px-focus already fronted/clicked the target.
                    if fg && !px_focus {
                        if let Some(wid) = window_id {
                            if element_index.is_none() {
                                crate::input::skylight::with_menu_shortcut_activation(
                                    pid as libc::pid_t,
                                    wid,
                                    || crate::input::keyboard::press_key_no_auth(pid, &key, &m),
                                )?;
                                return Ok(());
                            }
                        }
                    }
                    // background (default): auth-envelope post, no raise.
                    crate::input::keyboard::press_key(pid, &key, &m)
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
                let mut structured = serde_json::json!({
                    "path": if fg { "key_events_fg" } else { "key_events" },
                    "verified": false,
                    "effect": "unverifiable",
                });
                if !fg && window_id.is_some() && element_index.is_none() {
                    structured["escalation"] = serde_json::json!({
                        "recommended": "foreground",
                        "reason": "a background menu key didn't land? re-call with \
                                   delivery_mode:\"foreground\". (To type into a field, \
                                   pixel-click to focus then type_text.)"
                    });
                }
                ToolResult::text(format!(
                    "✅ Pressed {display_key} on pid {pid}{label}.{}",
                    changes.result_suffix()
                ))
                .with_structured(structured)
            }
            Ok(Err(e)) => ToolResult::error(format!("press_key failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
