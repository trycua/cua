use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    bitmap_to_screen, chromium_click_short_circuit, element_target_rect, overlay_glide_to,
    overlay_glide_to_target, pin_overlay_above, resolve_cursor_key,
    resolve_onscreen_point_with_scroll, restore_foreground_polling_best_effort, screenshot_scale,
    winui3_background_gesture, ToolState,
};

// ── right_click ───────────────────────────────────────────────────────────────

pub struct RightClickTool {
    pub(crate) state: Arc<ToolState>,
}

static RCLICK_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for RightClickTool {
    fn def(&self) -> &ToolDef {
        RCLICK_DEF.get_or_init(|| ToolDef {
            name: "right_click".into(),
            // Description ported from Swift `RightClickTool.swift` semantics.
            // Windows uses PostMessage(WM_RBUTTONDOWN/UP); the AXShowMenu
            // analogue (UIA ShowContextMenu) is not yet wired up, so element
            // path falls through to the pixel recipe at the cached center.
            description: "Right-click against a target pid. Two addressing modes:\n\n\
                - `element_token` (from the last `get_window_state` snapshot \
                of that window) — posts WM_RBUTTONDOWN/UP at the element's cached on-screen \
                center via PostMessage. (Windows has no AXShowMenu analogue wired up yet; \
                Swift's AX-action path falls through to the same pixel recipe on non-\
                advertising elements, so user-visible behavior matches.)\n\n\
                - `x`, `y` (window-local screenshot pixels, top-left origin of the PNG \
                returned by `get_window_state`) — posts WM_RBUTTONDOWN/UP to the deepest \
                child window at that point.\n\n\
                Exactly one of `element_token` or (`x` AND `y`) must be provided. `pid` is \
                required with `x`, `y`; an `element_token` carries its own pid. \
                `modifier` is accepted for parity (no-op on Windows — PostMessage doesn't \
                propagate modifier-key state).".into(),
            input_schema: json!({"type":"object","required":[],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "pid":{"type":"integer","description":"Target process ID. Optional with element_token (the token carries the pid)."},
                "window_id":{"type":"integer","description":"HWND for the target window. Omit when element_token is supplied (the token carries it)."},
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "x":{"type":"number","description":"X in window-local screenshot pixels. Must be provided together with y."},
                "y":{"type":"number","description":"Y in window-local screenshot pixels. Must be provided together with x."},
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "from_zoom":{"type":"boolean","description":"After a zoom call, pass true to translate zoom-image coords back to window space."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        use cua_driver_core::tool_args::ArgsExt;
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (elem_idx, hwnd_opt, admitted) = resolved.into_parts(args.opt_u64("window_id"));
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
        let delivery = DeliveryMode::from_args(&args);
        let cursor_key = resolve_cursor_key(&args);
        // Port Swift's full validation set.
        let has_xy = x.is_some() && y.is_some();
        let partial_xy = x.is_some() != y.is_some();
        if partial_xy {
            return ToolResult::error("Provide both x and y together, not just one.");
        }
        if elem_idx.is_some() && has_xy {
            return ToolResult::error("Provide either element_token or (x, y), not both.");
        }
        if elem_idx.is_none() && !has_xy {
            return ToolResult::error(
                "Provide element_token or (x, y) to address the right-click target.",
            );
        }

        // Resolve HWND: explicit, or auto from pid.
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

        if let Some(idx) = elem_idx {
            let (cx, cy) = match admitted.as_ref().map(|element| element.center) {
                Some(v) => v,
                None => {
                    return ToolResult::error(format!(
                        "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                    ))
                }
            };
            let recorded_center = (cx, cy);
            let (cx, cy) = match resolve_onscreen_point_with_scroll(
                &admitted,
                hwnd,
                idx,
                cx,
                cy,
                "right-clicking",
            ) {
                Ok(p) => p,
                Err(result) => return result,
            };
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to_target(
                &cursor_key,
                cx as f64,
                cy as f64,
                element_target_rect(
                    admitted.as_ref(),
                    (cx - recorded_center.0, cy - recorded_center.1),
                ),
            )
            .await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: cx as f64,
                    y: cy as f64,
                },
            );
            // delivery_mode:"background" on WinUI3: a right-click can't both land and
            // hold the contract — UIA has no right-click pattern, the content
            // island ignores posted WM_RBUTTON (measured: RightTapped never
            // fired), and the pen-barrel injector click-activates the frame.
            // Return the structured error (retry with delivery_mode:foreground).
            if delivery == DeliveryMode::Background {
                if let Some(r) =
                    winui3_background_gesture(&admitted, pid, hwnd, Some(idx), 1, "right").await
                {
                    return r;
                }
            }
            // delivery_mode:"background" (default): try coordinate injection (no
            // foreground swap) for drop-prone targets (#1984); a right-click
            // injection that the actuator can't express falls back to the error.
            if (cx, cy) != recorded_center {
                crate::recording_hooks::capture_dispatch_click_target(hwnd, pid, cx, cy);
            }
            if delivery == DeliveryMode::Background
                && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseClick)
            {
                let inj = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::input::inject_click_screen(hwnd, cx, cy, 1, "right")
                    }
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected right-click to [{idx}] at screen ({cx},{cy}) (background, no foreground swap)."
                    )),
                    Ok(Err(e)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        e.to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            if delivery == DeliveryMode::Foreground {
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::input::send_click_synthesized_active_mods(
                            hwnd,
                            cx,
                            cy,
                            1,
                            "right",
                            &[],
                        )
                    }
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Sent right-click via SendInput on [{idx}] at screen ({cx},{cy}) (delivery_mode:foreground)."
                    )),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            // Chromium/Electron silently drops PostMessage clicks (#1984) — route via SendInput.
            if let Some(r) =
                chromium_click_short_circuit(hwnd, cx, cy, 1, "right", pid, "right-click").await
            {
                return r;
            }
            let result = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || -> anyhow::Result<String> {
                    let _admission = &admitted;
                    crate::input::post_click_screen(hwnd, cx, cy, 1, "right")?;
                    // Match Swift's element-path text 1:1
                    // (`"✅ Shown menu for [N] role \"title\"."`).  UIA role/title
                    // placeholder pending element-cache enrichment.
                    Ok(format!("✅ Shown menu for [{idx}] (screen ({cx}, {cy}))."))
                }
            })
            .await;
            match result {
                Ok(Ok(msg)) => ToolResult::text(msg),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut px), Some(mut py)) = (x, y) {
            let from_zoom = args
                .get("from_zoom")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if from_zoom {
                match self.state.zoom_context(&args, pid, Some(hwnd)) {
                    Ok(ctx) => {
                        let (wx, wy) = ctx.zoom_to_window(px, py);
                        px = wx;
                        py = wy;
                    }
                    Err(refusal) => return refusal,
                }
            } else {
                let ratio = match screenshot_scale(&self.state, &args, pid, Some(hwnd)) {
                    Ok(ratio) => ratio,
                    Err(refusal) => return refusal,
                };
                px *= ratio;
                py *= ratio;
            }
            // bitmap pixels -> screen via DWM-frame origin (see
            // `bitmap_to_screen` doc).
            let (sx_i, sy_i) = bitmap_to_screen(hwnd, px as i32, py as i32);
            let (sx, sy) = (sx_i as f64, sy_i as f64);
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, sx, sy).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
            );
            // delivery_mode:"background" on WinUI3: right-click can't land while
            // holding the contract (no UIA right-click, posted WM_RBUTTON
            // ignored, pen-barrel injector steals foreground) → structured error.
            if delivery == DeliveryMode::Background {
                if let Some(r) =
                    winui3_background_gesture(&admitted, pid, hwnd, None, 1, "right").await
                {
                    return r;
                }
            }
            // delivery_mode:"background" (default): try coordinate injection (no
            // foreground swap) for drop-prone targets (#1984).
            if delivery == DeliveryMode::Background
                && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseClick)
            {
                let inj = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::input::inject_click_screen(hwnd, sx_i, sy_i, 1, "right")
                    }
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected right-click to pid {pid} at screen ({sx_i},{sy_i}) (background, no foreground swap)."
                    )),
                    Ok(Err(e)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        e.to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            if delivery == DeliveryMode::Foreground {
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::input::send_click_synthesized_active_mods(
                            hwnd,
                            sx_i,
                            sy_i,
                            1,
                            "right",
                            &[],
                        )
                    }
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Sent right-click via SendInput to pid {pid} at screen ({sx_i},{sy_i}) (delivery_mode:foreground)."
                    )),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            let (xi, yi) = (px as i32, py as i32);
            // Chromium/Electron silently drops PostMessage clicks (#1984) — route via SendInput.
            if let Some(r) =
                chromium_click_short_circuit(hwnd, sx_i, sy_i, 1, "right", pid, "right-click").await
            {
                return r;
            }
            let result = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    crate::input::post_click_screen(hwnd, sx_i, sy_i, 1, "right")
                }
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    // Swift pixel-path text 1:1.
                    ToolResult::text(format!(
                        "✅ Posted right-click to pid {pid} at window-pixel ({xi}, {yi}) → screen-point ({sx_i}, {sy_i})."))
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            // Guarded above by the early validation block — unreachable.
            unreachable!()
        }
    }
}
