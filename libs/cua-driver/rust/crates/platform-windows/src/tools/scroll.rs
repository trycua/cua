use async_trait::async_trait;
use cua_driver_contract::{ScrollDirection, ScrollInput};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{bitmap_to_screen, is_windowless_desktop_action, ToolState};

// ── scroll ────────────────────────────────────────────────────────────────────

pub struct ScrollTool {
    pub(crate) state: Arc<ToolState>,
}
static SCROLL_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ScrollTool {
    fn def(&self) -> &ToolDef {
        SCROLL_DEF.get_or_init(|| ToolDef {
            name: "scroll".into(),
            // Description ported from Swift `ScrollTool.swift` with the
            // Windows-specific transport note (WM_VSCROLL/WM_HSCROLL).
            description: "Scroll the target pid's focused region.\n\n\
                Windows transport: WM_VSCROLL / WM_HSCROLL posted to the window — the same \
                events the OS sends when scrollbars or trackpads scroll, so any window that \
                handles scrollbars correctly responds to this. Swift uses synthesized \
                keystrokes (PageDown / arrow keys) via auth-signed SLEventPostToPid; both \
                approaches reach backgrounded windows.\n\n\
                Mapping: `by: \"page\"` → SB_PAGEDOWN/UP/LEFT/RIGHT × amount; \
                `by: \"line\"` → SB_LINEDOWN/UP/LEFT/RIGHT × amount.\n\n\
                Note: `element_token` is accepted for cross-platform parity but currently \
                no-op on Windows (UIA SetFocus not wired up yet — same caveat as `press_key`).".into(),
            input_schema: json!({
                "type":"object","required":["direction"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID for window scope. Optional with element_token (the token carries the pid). Omit with scope=desktop for screen-absolute coordinates from get_desktop_state."},
                    "direction":{"type":"string","enum":["up","down","left","right"],"description":"Scroll direction."},
                    "by":{"type":"string","enum":["line","page"],"description":"Scroll granularity. Default: line."},
                    "amount":{"type":"integer","minimum":1,"maximum":50,
                        "description":"Number of scroll ticks. Default 3."},
                    "x":{"type":"number","description":"With pid/window_id: window-local screenshot X used to target a nested scroll surface in foreground mode. Without pid/window_id: screen-absolute X for desktop scope. Must be paired with y."},
                    "y":{"type":"number","description":"With pid/window_id: window-local screenshot Y used to target a nested scroll surface in foreground mode. Without pid/window_id: screen-absolute Y for desktop scope. Must be paired with x."},
                    "window_id":{"type":"integer","description":"HWND of the target window. Omit when element_token is supplied; otherwise auto-resolves the pid's first visible window."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"Use \"desktop\" with x,y and no pid/window_id for screen-absolute coordinates from get_desktop_state. Default \"window\"."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        // ── Window-less screen-absolute branch (desktop target) ───────────────
        // No pid/window_id + numeric x,y + desktop scope → synthesize a wheel
        // event at the screen point via SendInput. The wheel routes to whatever
        // window is under (x,y). up/down map to a vertical wheel (sign), and
        // left/right to a horizontal wheel; `amount` is the tick count.
        let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
        let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
        let has_xy = args.get("x").map(|v| v.is_number()).unwrap_or(false)
            && args.get("y").map(|v| v.is_number()).unwrap_or(false);
        if !has_pid && !has_window_id && has_xy {
            if !is_windowless_desktop_action(&args) {
                return ToolResult::error(
                    "scroll: x,y given with no pid/window_id requires scope=\"desktop\"; \
                     use get_desktop_state to pick coordinates, or pass a pid/window_id.",
                )
                .with_structured(serde_json::json!({
                    "code": "desktop_coordinate_scope_required",
                    "suggestion": "pass scope=desktop",
                }));
            }
            let input = match parse_typed_projection::<ScrollInput>("scroll", &args) {
                Ok(input) => input,
                Err(error) => return error,
            };
            let sx = input.x as i32;
            let sy = input.y as i32;
            let direction = input.direction;
            let amount = input.amount.unwrap_or(3) as u32;
            // Direction → (horizontal?, sign). Positive ticks = up / right.
            let (horizontal, sign) = match direction {
                ScrollDirection::Up => (false, 1),
                ScrollDirection::Down => (false, -1),
                ScrollDirection::Right => (true, 1),
                ScrollDirection::Left => (true, -1),
            };
            let ticks = sign * amount as i32;
            let dir_disp = direction.as_str();
            let result = tokio::task::spawn_blocking(move || {
                crate::input::send_wheel_synthesized(sx, sy, ticks, horizontal)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Scrolled {dir_disp} via SendInput wheel ({amount} tick(s)) at screen ({sx},{sy}) (desktop scope)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Window-scoped arguments retain the richer platform contract.
        let direction = match args.get("direction").and_then(|v| v.as_str()) {
            Some(d) => d.to_owned(),
            None => return ToolResult::error("Missing required string field direction."),
        };
        let amount = args.u64_or("amount", 3).clamp(1, 50) as u32;

        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let delivery = DeliveryMode::from_args(&args);
        let by = args.str_or("by", "line");
        let direction_display = direction.clone();
        let by_display = by.clone();
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (elem_idx, hwnd_opt, admitted) = resolved.into_parts(args.opt_u64("window_id"));

        // Resolve HWND: use window_id if given, else first window for pid.
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

        // WebView2/Tauri hosts can expose the indexed scroll container through
        // UIA while their top-level HWND ignores WM_VSCROLL. Prefer the
        // accessibility channel for an indexed target; the message path below
        // remains the fallback for native Win32 scrollbars.
        if delivery == DeliveryMode::Background && crate::input::is_chromium_target_window(hwnd) {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::MouseScroll,
            );
        }
        if let Some(idx) = elem_idx {
            let prev_fg_addr = if delivery == DeliveryMode::Background {
                Some(unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize
                })
            } else {
                None
            };
            let _noact = if delivery == DeliveryMode::Background {
                Some(crate::input::NoActivateGuard::arm(
                    windows::Win32::Foundation::HWND(hwnd as *mut _),
                ))
            } else {
                None
            };
            let direction_for_uia = direction.clone();
            let uia_result = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    let retained = admitted.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("element [{idx}] is not in the UIA cache")
                    })?;
                    if !retained.is_uia() {
                        anyhow::bail!("element [{idx}] is not a UIA scroll element");
                    }
                    unsafe {
                        crate::uia::scroll::scroll_element(
                            retained.as_ptr(),
                            &direction_for_uia,
                            amount,
                        )
                    }
                }
            })
            .await;
            if matches!(uia_result, Ok(Ok(()))) {
                if delivery == DeliveryMode::Background {
                    // Keep WS_EX_NOACTIVATE armed through any WebView handler
                    // queued by the UIA operation.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    if let Some(previous_addr) = prev_fg_addr {
                        use windows::Win32::Foundation::HWND;
                        use windows::Win32::UI::WindowsAndMessaging::{
                            GetAncestor, GetForegroundWindow, GA_ROOT,
                        };
                        let previous = HWND(previous_addr as *mut _);
                        let current = unsafe { GetForegroundWindow() };
                        let current_root = unsafe { GetAncestor(current, GA_ROOT) };
                        let target_root = unsafe { GetAncestor(HWND(hwnd as *mut _), GA_ROOT) };
                        if !previous.0.is_null()
                            && current != previous
                            && !target_root.0.is_null()
                            && current_root == target_root
                        {
                            unsafe {
                                crate::input::force_foreground_attached(previous);
                                std::thread::sleep(std::time::Duration::from_millis(12));
                                crate::input::force_foreground_attached(previous);
                            }
                        }
                    }
                }
                return ToolResult::text(format!(
                    "Scrolled {direction} {amount} ticks via UIA (delivery_mode:background)."
                ))
                .with_structured(serde_json::json!({
                    "path": "uia",
                    "verified": false,
                    "delivery_mode": "background"
                }));
            }
        }

        if delivery == DeliveryMode::Background
            && args.get("x").is_some_and(serde_json::Value::is_number)
            && args.get("y").is_some_and(serde_json::Value::is_number)
        {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::MouseScroll,
            );
        }

        // delivery_mode:"background" — WM_VSCROLL/HSCROLL is silently dropped by
        // Chromium and may be by GTK. Surface the standard structured
        // background_unavailable error: its remediation (bring_to_front +
        // delivery_mode:"foreground") is now a real recovery because scroll's
        // foreground path is implemented below (SendInput wheel reaches the
        // Chromium/GTK targets PostMessage can't).
        if delivery == DeliveryMode::Background
            && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseScroll)
        {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::MouseScroll,
            );
        }
        // delivery_mode:"foreground" — SendInput MOUSEEVENTF_WHEEL at the target
        // window's screen center (send_wheel_synthesized). The wheel routes to
        // the window under the cursor — queue-origin input Chromium/GTK accept —
        // so it lands where PostMessage WM_*SCROLL is dropped. Mirrors the
        // desktop-scope wheel branch above; `by` (line/page) folds into the tick
        // count (page ≈ 3 detents).
        if delivery == DeliveryMode::Foreground {
            let (horizontal, sign) = match direction.as_str() {
                "up" => (false, 1),
                "down" => (false, -1),
                "right" => (true, 1),
                "left" => (true, -1),
                other => {
                    return ToolResult::error(format!(
                        "scroll: unknown direction \"{other}\" — expected up, down, left, right."
                    ))
                }
            };
            let per: i32 = if by == "page" { 3 } else { 1 };
            let ticks = sign * (amount as i32) * per;
            // A supplied PX target is window-local in the get_window_state
            // bitmap. Route the wheel there so nested web scrollers receive it;
            // otherwise retain the whole-window center fallback.
            let px = args.get("x").and_then(|value| value.as_f64());
            let py = args.get("y").and_then(|value| value.as_f64());
            if px.is_some() != py.is_some() {
                return ToolResult::error("scroll requires x and y together.");
            }
            let center = if let (Some(x), Some(y)) = (px, py) {
                let (x, y) = match super::scroll_coordinates::window_bitmap_point(
                    &self.state.snapshots,
                    &args,
                    pid,
                    hwnd,
                    x,
                    y,
                ) {
                    Ok(point) => point,
                    Err(refusal) => return refusal,
                };
                Some(bitmap_to_screen(hwnd, x, y))
            } else {
                tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::win32::list_windows(Some(pid))
                            .into_iter()
                            .find(|w| w.hwnd == hwnd)
                            .map(|w| (w.x + w.width / 2, w.y + w.height / 2))
                    }
                })
                .await
                .ok()
                .flatten()
            };
            let (cx, cy) = match center {
                Some(c) => c,
                None => {
                    return ToolResult::error(format!(
                        "scroll: could not resolve on-screen bounds for window {hwnd:#x} \
                     (pid {pid}) to target the wheel. The window may be minimized or \
                     off-screen — call bring_to_front first."
                    ))
                }
            };
            let dir_disp = direction.clone();
            let tick_disp = ticks.abs();
            let result = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    crate::input::send_wheel_synthesized(cx, cy, ticks, horizontal)
                }
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Scrolled {dir_disp} via SendInput wheel ({tick_disp} tick(s)) at \
                     screen ({cx},{cy}) (delivery_mode:foreground)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let result = tokio::task::spawn_blocking({
            let admitted = admitted.clone();
            move || -> anyhow::Result<()> {
                let _admission = &admitted;
                use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
                use windows::Win32::UI::WindowsAndMessaging::{
                    PostMessageW, SB_LINEDOWN, SB_LINELEFT, SB_LINERIGHT, SB_LINEUP, SB_PAGEDOWN,
                    SB_PAGELEFT, SB_PAGERIGHT, SB_PAGEUP, WM_HSCROLL, WM_VSCROLL,
                };

                let hwnd_win = HWND(hwnd as *mut _);
                let use_page = by == "page";
                let (msg, code) = match (direction.as_str(), use_page) {
                    ("up", false) => (WM_VSCROLL, SB_LINEUP),
                    ("up", true) => (WM_VSCROLL, SB_PAGEUP),
                    ("down", false) => (WM_VSCROLL, SB_LINEDOWN),
                    ("down", true) => (WM_VSCROLL, SB_PAGEDOWN),
                    ("left", false) => (WM_HSCROLL, SB_LINELEFT),
                    ("left", true) => (WM_HSCROLL, SB_PAGELEFT),
                    ("right", false) => (WM_HSCROLL, SB_LINERIGHT),
                    ("right", true) => (WM_HSCROLL, SB_PAGERIGHT),
                    _ => (WM_VSCROLL, SB_LINEDOWN),
                };
                for _ in 0..amount {
                    unsafe {
                        PostMessageW(hwnd_win, msg, WPARAM(code.0 as usize), LPARAM(0))?;
                    }
                }
                Ok(())
            }
        })
        .await;

        match result {
            Ok(Ok(())) => {
                // Match Swift's text format shape 1:1 (Swift uses key names
                // like "pagedown"; Windows uses Win32 SB_* constants — show
                // the actual mechanism for traceability).
                let mech_name = match (direction_display.as_str(), by_display.as_str()) {
                    ("up", "page") => "SB_PAGEUP",
                    ("down", "page") => "SB_PAGEDOWN",
                    ("left", "page") => "SB_PAGELEFT",
                    ("right", "page") => "SB_PAGERIGHT",
                    ("up", _) => "SB_LINEUP",
                    ("down", _) => "SB_LINEDOWN",
                    ("left", _) => "SB_LINELEFT",
                    ("right", _) => "SB_LINERIGHT",
                    _ => "SB_LINEDOWN",
                };
                ToolResult::text(format!(
                    "✅ Scrolled pid {raw_pid} {direction_display} via {amount}× {mech_name} message(s)."
                ))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
