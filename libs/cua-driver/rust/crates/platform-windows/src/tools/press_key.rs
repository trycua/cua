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
    bitmap_to_screen, focus_by_pixel, focus_cached_element_for_foreground,
    position_keyboard_cursor, resolve_onscreen_point_with_scroll, ToolState,
};

fn screen_to_bitmap(hwnd: u64, sx: i32, sy: i32) -> (i32, i32) {
    let (origin_x, origin_y) = bitmap_to_screen(hwnd, 0, 0);
    (sx - origin_x, sy - origin_y)
}

// ── press_key ─────────────────────────────────────────────────────────────────

pub struct PressKeyTool {
    pub(crate) state: Arc<ToolState>,
}
static PRESS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for PressKeyTool {
    fn def(&self) -> &ToolDef {
        PRESS_DEF.get_or_init(|| ToolDef {
            name: "press_key".into(),
            // Description ported from Swift `PressKeyTool.swift` with
            // Windows-specific transport note (PostMessage WM_KEYDOWN/UP).
            description: "Press and release a single key, delivered directly to the target pid's \
                top-level window via PostMessage(WM_KEYDOWN/WM_KEYUP). The target does NOT need \
                to be frontmost — no focus steal.\n\n\
                Optional `window_id` selects a specific HWND when the pid owns more than one; \
                without it the first visible top-level window for the pid is used.\n\n\
                Key vocabulary: return, tab, escape, up/down/left/right, space, delete, home, \
                end, pageup, pagedown, f1-f12, plus any letter or digit. Optional `modifiers` \
                array takes ctrl/shift/alt/win. For true combinations (ctrl+c), `hotkey` is a \
                cleaner surface.\n\n\
                `element_token` focuses the cached UIA element before sending the key; the \
                top-level window remains backgrounded when delivery_mode is background.".into(),
            input_schema: json!({
                "type":"object","required":["key"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "scope":{"type":"string","enum":["window","desktop"],"description":"Use desktop with no pid/window_id to send the key to the current foreground application."},
                    "pid":{"type":"integer","description":"Target process ID."},
                    "key":{"type":"string","description":"Key name (return, tab, escape, up, down, left, right, space, delete, home, end, pageup, pagedown, f1-f12, letter, digit)."},
                    "modifiers":{"type":"array","items":{"type":"string"},"description":"Optional modifier names held while the key is pressed (ctrl/shift/alt/win)."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "x":{"type":"number","description":"Window-local screenshot-pixel X — the element px action form: pixel-click there to focus, then send the key. Use when the key must go to a Chromium/Electron surface the UIA path can't focus. Pass with y, no element_token."},
                    "y":{"type":"number","description":"Window-local screenshot-pixel Y (see x)."},
                    "window_id":{"type":"integer","description":"HWND for the target window. Omit when element_token is supplied; otherwise auto-resolves the pid's first visible window."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<PressKeyInput>("press_key", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let key = input.key;
            let mods = input.modifiers.unwrap_or_default();
            let hwnd = match crate::input::mouse::foreground_window() {
                Ok(hwnd) => hwnd,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            let key_display = key.clone();
            return match tokio::task::spawn_blocking(move || {
                let modifiers: Vec<&str> = mods.iter().map(String::as_str).collect();
                crate::input::send_key_synthesized(hwnd, &key, &modifiers)
            })
            .await
            {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Sent {key_display} to the foreground application (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SendInput", "key": key_display,
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("desktop key task failed: {error}")),
            };
        }
        let key = match args.require_str("key") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mods: Vec<String> = args.str_array("modifiers");
        let raw_pid = match args.require_i64("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pid = raw_pid as u32;
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (elem_idx, hwnd_opt, admitted) = resolved.into_parts(args.opt_u64("window_id"));
        let delivery = DeliveryMode::from_args(&args);
        // Swift requires window_id when element_index is supplied — ports the
        // same validation.

        // px form: pixel-click to focus, then the key goes to the focused element.
        // Reuses click's translation + delivery_mode; after it, deliver via the
        // plain background post path (the focus-click already handled fronting if
        // foreground), so skip the background_unavailable check + foreground swap
        // below. Mirrors macOS press_key.
        let px_focus = {
            let px = args.get("x").and_then(|v| v.as_f64());
            let py = args.get("y").and_then(|v| v.as_f64());
            if let (Some(cx), Some(cy)) = (px, py) {
                if elem_idx.is_some() {
                    return ToolResult::error(
                        "Pass either element_token (ax) or x,y (px) to press_key, not both.",
                    );
                }
                let from_zoom = args
                    .get("from_zoom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Err(e) = focus_by_pixel(
                    &self.state,
                    pid,
                    hwnd_opt,
                    cx,
                    cy,
                    delivery.is_foreground(),
                    args.opt_str("session"),
                    args.opt_str("_session_id"),
                    from_zoom,
                    false,
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

        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::win32::list_windows(Some(pid))
                    }
                })
                .await
                .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // Classify known background drops before touching UIA focus. Focusing
        // first made an honest Chromium refusal transiently activate the target.
        let event_kind = if mods.is_empty() {
            EventKind::Keystroke
        } else {
            EventKind::KeyCombo
        };
        if delivery == DeliveryMode::Background
            && crate::input::delivery::would_be_silently_dropped(hwnd, event_kind)
        {
            return crate::input::delivery::background_unavailable_error(hwnd, event_kind);
        }

        // The px form's focus click already moved the cursor. Otherwise place
        // a named session's cursor so a keyboard-first session stays visible.
        if !px_focus {
            let element_center = admitted
                .as_ref()
                .map(|element| (element.center.0 as f64, element.center.1 as f64));
            position_keyboard_cursor(&self.state, &args, hwnd, element_center).await;
        }

        // W1: an element-addressed key needs the control's actual focus
        // target, not merely its owning top-level HWND. Embedded WebView hosts
        // can activate their frame from UIA SetFocus even under
        // WS_EX_NOACTIVATE. Their proven-safe pixel route establishes renderer
        // focus with a posted click, so reuse that route at the AX element's
        // cached center.
        let background_webview_focus = elem_idx.is_some()
            && delivery == DeliveryMode::Background
            && crate::input::has_chromium_descendant(hwnd);
        let mut noact = if elem_idx.is_some() && delivery == DeliveryMode::Background {
            Some(crate::input::NoActivateGuard::arm(
                windows::Win32::Foundation::HWND(hwnd as *mut _),
            ))
        } else {
            None
        };
        if let Some(idx) = elem_idx.filter(|_| background_webview_focus) {
            let Some((cx, cy)) = admitted.as_ref().map(|element| element.center) else {
                return ToolResult::error(format!(
                    "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                ));
            };
            let (cx, cy) = match resolve_onscreen_point_with_scroll(
                &admitted,
                hwnd,
                idx as usize,
                cx,
                cy,
                "focusing for key delivery",
            ) {
                Ok(point) => point,
                Err(result) => return result,
            };
            let (px, py) = screen_to_bitmap(hwnd, cx, cy);
            // Release the outer guard before the shared pixel helper. ClickTool
            // owns a guard around the click itself, then releases it before its
            // renderer settle period. This is the exact route already proven by
            // the PX background cell, including targeted injection fallback.
            drop(noact.take());
            if let Err(error) = focus_by_pixel(
                &self.state,
                pid,
                Some(hwnd),
                px as f64,
                py as f64,
                false,
                args.opt_str("session"),
                args.opt_str("_session_id"),
                false,
                true,
            )
            .await
            {
                return error;
            }
        } else if elem_idx.is_some() && delivery != DeliveryMode::Foreground {
            let focused = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || {
                        admitted
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("missing admitted element"))
                            .and_then(|element| element.focus_element())
                    })
                }
            })
            .await;
            match focused {
                Ok(Ok(())) => {
                    if delivery == DeliveryMode::Background {
                        let _ = crate::input::wait_for_focused_descendant(
                            hwnd,
                            std::time::Duration::from_millis(500),
                        );
                    }
                }
                Ok(Err(e)) => return ToolResult::error(e.to_string()),
                Err(e) => return ToolResult::error(format!("UIA focus task failed: {e}")),
            }
        }
        let key_display = key.clone();
        // Foreground: send_key_synthesized takes the SetForegroundWindow path.
        // Skipped when px-focus already fronted/clicked the target — the key then
        // goes via the plain background post path below.
        if !px_focus && delivery == DeliveryMode::Foreground {
            let focus_target = elem_idx.map(|idx| {
                let point = admitted.as_ref().map(|element| element.center);
                (idx as usize, point)
            });
            let send_result = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    let m: Vec<&str> = mods.iter().map(String::as_str).collect();
                    crate::input::send_key_synthesized_after_focus(hwnd, &key, &m, || {
                        if let Some((idx, point)) = focus_target {
                            focus_cached_element_for_foreground(&admitted, hwnd, idx, point)?;
                        }
                        Ok(())
                    })
                }
            })
            .await;
            return match send_result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent {key_display} via SendInput on pid {raw_pid} (delivery_mode:foreground)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let result = tokio::task::spawn_blocking({
            let admitted = admitted.clone();
            move || {
                let _admission = &admitted;
                let m: Vec<&str> = mods.iter().map(String::as_str).collect();
                crate::input::post_key(hwnd, &key, &m)
            }
        })
        .await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "📨 Sent {key_display} to pid {raw_pid} via PostMessage (not verified). \
                 If it didn't land, retry with delivery_mode:\"foreground\"."
            ))
            .with_structured(serde_json::json!({
                "path": "key_events",
                "key": key_display,
                "verified": false,
                "verify": "unreadable",
                "effect": "unverifiable",
                "escalation": {
                    "recommended": "foreground",
                    "reason": "background key delivery could not be confirmed — re-call \
                               with delivery_mode:\"foreground\" if a screenshot shows the key didn't land."
                },
            })),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
