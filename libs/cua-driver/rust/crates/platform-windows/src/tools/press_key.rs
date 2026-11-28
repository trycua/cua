// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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

/// Resolve only the admitted control's native message target. UIA SetFocus
/// is not a background preparation step: providers can activate their frame.
fn native_background_key_target(
    element: &crate::uia::snapshot::RetainedElement,
    owner: u64,
    pid: u32,
) -> anyhow::Result<u64> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsChild};

    let target = element.native_window_handle().ok_or_else(|| {
        anyhow::anyhow!("background_unavailable: exact element has no native keyboard target")
    })?;
    let native = HWND(target as *mut _);
    let mut target_pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(native, Some(&mut target_pid)) };
    if thread == 0
        || target_pid != pid
        || (target != owner && !unsafe { IsChild(HWND(owner as *mut _), native).as_bool() })
    {
        anyhow::bail!(
            "background_unavailable: exact element's native keyboard target is outside the admitted window"
        );
    }
    Ok(target)
}

/// Whether a background element key belongs to an embedded WebView rather
/// than a native control beside it. `native` is the element's nearest HWND:
/// its own, or a windowless element's nearest windowed ancestor. Only an HWND
/// inside a Chromium window proves WebView content; anything else, including
/// no HWND at all, keeps the exact-HWND route, which refuses what it cannot
/// address instead of focus-clicking a native control.
fn element_in_embedded_webview(
    native: Option<u64>,
    owner: u64,
    is_chromium: impl Fn(u64) -> bool,
    parent: impl Fn(u64) -> u64,
) -> bool {
    let Some(mut current) = native else {
        return false;
    };
    for _ in 0..64 {
        if current == 0 {
            return false;
        }
        if is_chromium(current) {
            return true;
        }
        if current == owner {
            return false;
        }
        current = parent(current);
    }
    false
}

fn native_parent(hwnd: u64) -> u64 {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_PARENT};
    unsafe { GetAncestor(HWND(hwnd as *mut _), GA_PARENT) }.0 as usize as u64
}

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
            description: "Press and release a single key. Background delivery uses \
                PostMessage(WM_KEYDOWN/WM_KEYUP) without fronting the target; explicit \
                foreground delivery uses SendInput.\n\n\
                Optional `window_id` selects a specific HWND when the pid owns more than one; \
                without it the first visible top-level window for the pid is used.\n\n\
                Key vocabulary: return, tab, escape, up/down/left/right, space, delete, home, \
                end, pageup, pagedown, f1-f12, plus any letter or digit. Optional `modifiers` \
                array takes ctrl/shift/alt/win. For true combinations (ctrl+c), `hotkey` is a \
                cleaner surface.\n\n\
                `element_token` addresses the cached UIA control. Foreground delivery focuses \
                it before sending the key. Background native-control delivery posts to its \
                exact HWND without focus preparation, or returns background_unavailable when \
                no safe exact target exists. Embedded WebView elements retain their background \
                focus-click route; do not rely on a native background key establishing focus \
                for later keys without an element_token.".into(),
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
            position_keyboard_cursor(&self.state, &args, hwnd, None).await;
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
                Ok(Err(error)) => crate::typing_turn::foreground_input_error(error),
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
        if delivery.is_background()
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

        // Embedded WebView keys need the renderer's actual focus target,
        // not merely the owning top-level HWND. These hosts
        // can activate their frame from UIA SetFocus even under
        // WS_EX_NOACTIVATE. Their proven-safe pixel route establishes renderer
        // focus with a posted click, so reuse that route at the AX element's
        // cached center.
        let background_webview_focus = elem_idx.is_some()
            && delivery.is_background()
            && crate::input::has_chromium_descendant(hwnd)
            && tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let native = admitted
                        .as_ref()
                        .and_then(|element| element.nearest_native_window_handle());
                    element_in_embedded_webview(
                        native,
                        hwnd,
                        crate::input::is_chromium_target_window,
                        native_parent,
                    )
                }
            })
            .await
            .unwrap_or(false);
        // Native controls are addressed directly below; only the existing
        // WebView focus-click route needs an activation guard.
        let mut noact = if background_webview_focus {
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
                idx,
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
        }
        let key_display = key.clone();
        // Foreground: send_key_synthesized takes the SetForegroundWindow path.
        // Skipped when px-focus already fronted/clicked the target — the key then
        // goes via the plain background post path below.
        if !px_focus && delivery == DeliveryMode::Foreground {
            let focus_target = elem_idx.map(|idx| {
                let point = admitted.as_ref().map(|element| element.center);
                (idx, point)
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
                Ok(Err(e)) => crate::typing_turn::foreground_input_error(e),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let result = tokio::task::spawn_blocking({
            let admitted = admitted.clone();
            move || {
                let _admission = &admitted;
                let m: Vec<&str> = mods.iter().map(String::as_str).collect();
                if elem_idx.is_some() && delivery.is_background() && !background_webview_focus {
                    let element = admitted.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("background_unavailable: missing admitted keyboard element")
                    })?;
                    let target = native_background_key_target(element, hwnd, pid)?;
                    crate::input::keyboard::post_key_to_hwnd(target, &key, &m)
                } else {
                    crate::input::post_key(hwnd, &key, &m)
                }
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
            Ok(Err(e)) if e.to_string().starts_with("background_unavailable:") => {
                cua_driver_core::delivery::background_unavailable_result(
                    format!("{e}; no keyboard input was sent"),
                    "background_unavailable",
                    "exact-control background keyboard delivery is unavailable; retry with delivery_mode:\"foreground\"",
                    json!({"window_id": hwnd, "event_kind": event_kind.name()}),
                )
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::element_in_embedded_webview;

    const OWNER: u64 = 0x100;
    const EDIT: u64 = 0x200;
    const WEBVIEW_HOST: u64 = 0x300;
    const RENDERER: u64 = 0x400;

    fn is_chromium(hwnd: u64) -> bool {
        hwnd == WEBVIEW_HOST
    }

    fn parent(hwnd: u64) -> u64 {
        match hwnd {
            EDIT | WEBVIEW_HOST => OWNER,
            RENDERER => WEBVIEW_HOST,
            _ => 0,
        }
    }

    #[test]
    fn native_control_beside_a_webview_keeps_its_exact_hwnd() {
        assert!(!element_in_embedded_webview(
            Some(EDIT),
            OWNER,
            is_chromium,
            parent
        ));
        // A windowless native control's nearest HWND is the owner itself.
        assert!(!element_in_embedded_webview(
            Some(OWNER),
            OWNER,
            is_chromium,
            parent
        ));
    }

    #[test]
    fn element_without_a_windowed_ancestor_is_not_webview_content() {
        assert!(!element_in_embedded_webview(
            None,
            OWNER,
            is_chromium,
            parent
        ));
    }

    #[test]
    fn webview_elements_take_the_focus_click_route() {
        assert!(element_in_embedded_webview(
            Some(OWNER),
            OWNER,
            |hwnd| hwnd == OWNER,
            parent
        ));
        assert!(element_in_embedded_webview(
            Some(WEBVIEW_HOST),
            OWNER,
            is_chromium,
            parent
        ));
        assert!(element_in_embedded_webview(
            Some(RENDERER),
            OWNER,
            is_chromium,
            parent
        ));
    }
}
