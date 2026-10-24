use async_trait::async_trait;
use cua_driver_contract::ClickButton;
use cua_driver_core::{
    action_record::ActionTransport,
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_legacy_click_input,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    bitmap_to_screen, element_target_rect, is_windowless_desktop_action, minimized_element_refusal,
    overlay_glide_to, overlay_glide_to_target, pin_overlay_above, resolve_cursor_key,
    resolve_onscreen_point_with_scroll, restore_foreground_polling_best_effort, screenshot_scale,
    winui3_background_gesture, ToolState,
};

fn tool_result_text(result: ToolResult) -> String {
    result
        .content
        .into_iter()
        .find_map(|content| match content {
            cua_driver_core::protocol::Content::Text { text, .. } => Some(text),
            _ => None,
        })
        .unwrap_or_else(|| "element action was refused".to_owned())
}

fn capture_admission_refusal(error: anyhow::Error) -> ToolResult {
    let code = cua_driver_core::capture_runtime::admission_error_code(&error);
    ToolResult::error(format!("Capture-bound click refused: {error}")).with_structured(json!({
        "code": code,
        "effect": "refused",
        "detail": error.to_string(),
    }))
}

// ── click ─────────────────────────────────────────────────────────────────────

fn posted_pixel_click_result(pid: u32, click_word: &str) -> ToolResult {
    ToolResult::text(format!("✅ Posted {click_word} to pid {pid}.")).with_structured(json!({
        "path": "post_message",
        "verified": false,
        "effect": "unverifiable"
    }))
}

/// Only a completed miss permits another transport. A provider may finish an
/// activation after the deadline, so errors must not replay the click.
fn finish_pixel_uia_attempt(
    outcome: crate::uia::windows_enum::PointInvokeOutcome,
    pid: u32,
    sx: i32,
    sy: i32,
) -> Option<ToolResult> {
    use crate::uia::windows_enum::PointInvokeOutcome;
    let status = match outcome {
        PointInvokeOutcome::Miss => return None,
        PointInvokeOutcome::Invoked => {
            return Some(
                ToolResult::text(format!(
                    "✅ Performed UIA Invoke at ({sx},{sy}) for pid {pid}."
                ))
                .with_structured(json!({
                    "path": "ax", "verified": false, "effect": "unverifiable"
                })),
            );
        }
        PointInvokeOutcome::Busy => "busy",
        PointInvokeOutcome::Timeout => "timeout",
        PointInvokeOutcome::Unavailable => "unavailable",
    };
    Some(cua_driver_core::delivery::background_unavailable_result(
        format!(
            "UIA pixel click {status} for pid {pid}. No fallback input was sent. \
         The click effect is unknown; inspect the target state before another action. \
         If the provider does not recover, retry this action with delivery_mode:\"foreground\"."
        ),
        "background_unavailable",
        format!(
            "the UIA provider is {status}; retry this action with delivery_mode:\"foreground\"."
        ),
        json!({
            "uia_status": status,
            "path": "ax",
            "verified": false,
            "effect": "unverifiable",
        }),
    ))
}

enum BackgroundElementClick {
    Semantic {
        message: String,
        transport: ActionTransport,
        failed_calls: Vec<ActionTransport>,
    },
    Inject {
        x: i32,
        y: i32,
        failed_calls: Vec<ActionTransport>,
    },
    Post {
        x: i32,
        y: i32,
        failed_calls: Vec<ActionTransport>,
    },
}

fn background_element_click_result(
    message: String,
    transport: ActionTransport,
    failed_calls: Vec<ActionTransport>,
) -> ToolResult {
    use cua_driver_core::action_record::{
        ActionAttempt, ActionEffect, ActionExecutionRecord, ActionFallback, ActualDelivery,
        RequestedDelivery,
    };
    let path = match transport {
        ActionTransport::WindowsTargetedInjection => "pixel",
        ActionTransport::WindowsPostMessage => "post_message",
        _ => "ax",
    };
    let mut record = ActionExecutionRecord::new(
        ActionEffect::Unverifiable,
        transport,
        RequestedDelivery::Background,
    );
    record.actual_delivery = Some(ActualDelivery::Background);
    for (index, attempted) in failed_calls.iter().copied().enumerate() {
        record.attempts.push(ActionAttempt {
            transport: attempted,
            delivery: ActualDelivery::Background,
            detail: Some("UIA provider action returned an error".into()),
        });
        record.fallbacks.push(ActionFallback {
            from: attempted,
            to: failed_calls.get(index + 1).copied().unwrap_or(transport),
            reason: "Previous UIA provider action failed".into(),
        });
    }
    ToolResult::text(message)
        .with_structured(json!({ "path": path, "verified": false, "effect": "unverifiable" }))
        .with_action_record(record)
}

pub struct ClickTool {
    pub(crate) state: Arc<ToolState>,
}

static CLICK_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ClickTool {
    fn def(&self) -> &ToolDef {
        CLICK_DEF.get_or_init(|| ToolDef {
            name: "click".into(),
            // Description matches Swift `ClickTool.swift` semantics (left-click
            // primitive with two addressing modes — element_token via UIA, or
            // window-local pixel coords via PostMessage).  Windows-only schema
            // extras (`button`; `action` limited to "expand"; no
            // `debug_image_out`) apply here.
            description: "Left-click against a target pid. **Prefer `element_token` over \
                pixel coordinates** — element_token works on backgrounded / minimized / \
                hidden / off-desktop windows, surfaces a stable handle that survives \
                rebuilds, and tells you what you're clicking via the cached element's \
                role + label. Reach for `x, y` only when the target is a canvas / video / \
                WebGL / custom-drawn surface that doesn't appear in the UIA tree.\n\n\
                Two addressing modes:\n\n\
                - `element_token` (from the last `get_window_state` snapshot \
                of that window) — performs the UIA Invoke pattern on the cached element via \
                PostMessage. No cursor move, no focus steal. Requires a prior \
                `get_window_state(pid, window_id)` in this turn; the token is stale once \
                the next snapshot of the same window replaces it.\n\n\
                - `x`, `y` (window-local screenshot pixels, top-left origin of the PNG \
                returned by `get_window_state`). On `delivery_mode:\"background\"` (the \
                default), cua-driver FIRST does a UIA hit-test at the resolved \
                screen position: if the deepest invokable element under that point exposes \
                `InvokePattern`, it's invoked through the accessibility channel — same \
                background-safe path as the `element_token` mode, no foreground swap, no \
                visible flash. **This makes pixel clicks on UWP / WinUI3 / Win11 packaged \
                apps work flash-free out of the box** — agents should default to \
                `delivery_mode:\"background\"` even on XAML hosts whose CoreInput dispatcher \
                drops raw PostMessage. The PostMessage(WM_LBUTTONDOWN/UP) fallback only \
                runs when the UIA hit-test misses (canvas / video / WebGL / custom-drawn \
                surfaces with no UIA peer), and on those targets `delivery_mode:\"background\"` \
                returns a structured `background_unavailable` error if PostMessage is \
                known to drop too — at which point you switch to `delivery_mode:\"foreground\"`. \
                **Always try the default `background` click first and let that error be \
                your signal — do NOT pass `foreground` preemptively because the target \
                'looks like' GTK/Chromium/etc. The driver decides when background is \
                impossible; a guessed foreground click needlessly steals the user's focus.** \
                `count: 2` posts two down/up pairs for a double-click. Pixel clicks need \
                a visible on-screen window to anchor the coordinate conversion (errors \
                with `pid X has no on-screen window` otherwise).\n\n\
                Exactly one of `element_token` or (`x` AND `y`) must be provided. \
                `pid` is required for window scope and omitted for desktop scope. After a `zoom` call, \
                pass `from_zoom=true` to auto-translate zoom-image coords back to \
                full-window space.\n\n\
                Windows-only convenience: `button: \"left\"|\"right\"|\"middle\"` switches \
                the mouse button (Swift exposes right-click as a separate `right_click` \
                tool). `action: \"expand\"` on an `element_token` opens the element \
                through UIA ExpandCollapsePattern (the dropdown half of an MSAA split \
                button); other actions use the default invoke. `debug_image_out` isn't \
                supported yet.".into(),
            input_schema: json!({
                "type":"object","properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID for window scope. Optional with element_token (the token carries the pid). Omit with scope=desktop for screen-absolute coordinates from get_desktop_state."},
                    "window_id":{"type":"integer","description":"HWND of the target window. Omit when element_token is supplied (the token carries it)."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "action":{"type":"string","description":"Accessibility action for an element_token target. \"expand\" opens the element through UIA ExpandCollapsePattern, or clicks the dropdown half of an MSAA split button. Omit for the default invoke."},
                    "capture_id":{"type":"string","minLength":1,"description":"Optional ID from get_window_state or get_desktop_state. When present, x and y are admitted against that exact capture and the ID is consumed before native dispatch."},
                    "x":{"type":"number","description":"X in window-local screenshot pixels — same space as the PNG get_window_state returns. Must be provided together with y."},
                    "y":{"type":"number","description":"Y in window-local screenshot pixels. Must be provided together with x."},
                    "button": cua_driver_core::tool_schema::button_schema(),
                    "count":{"type":"integer","minimum":1,"maximum":3,"description":"Click count — 1 (single), 2 (double), 3 (triple). Default 1."},
                    "modifier": cua_driver_core::tool_schema::modifier_schema(),
                    "from_zoom":{"type":"boolean","description":"When true, x and y are pixel coordinates in the last `zoom` image for this pid. The driver maps them back to window coords."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"Coordinate frame (default \"window\"). Pass \"desktop\" with x,y and no pid/window_id for a screen-absolute click in get_desktop_state coordinates."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use crate::uia::snapshot::SnapshotKind;
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_key = resolve_cursor_key(&args);

        // ── Window-less screen-absolute branch (desktop target) ───────────────
        // When the caller gives x,y with NO pid/window_id, treat x,y as TRUE
        // SCREEN pixels. The core registry has already enforced the session's
        // effective capture scope; this local check validates the action form.
        let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
        let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
        let has_xy = args.get("x").map(|v| v.is_number()).unwrap_or(false)
            && args.get("y").map(|v| v.is_number()).unwrap_or(false);
        if !has_pid && !has_window_id && has_xy {
            if !is_windowless_desktop_action(&args) {
                return ToolResult::error(
                    "click: x,y given with no pid/window_id requires scope=\"desktop\"; \
                     use get_desktop_state to pick coordinates, or pass a pid/window_id.",
                )
                .with_structured(json!({
                    "code": "desktop_coordinate_scope_required",
                    "suggestion": "pass scope=desktop",
                }));
            }
            let input = match parse_legacy_click_input(&args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let button = input
                .button
                .unwrap_or(ClickButton::Left)
                .as_str()
                .to_owned();
            let count = input.count.unwrap_or(1) as usize;
            if count == 0 {
                return ToolResult::error("click.count must be at least 1.")
                    .with_structured(json!({ "code": "invalid_arguments" }));
            }
            let modifiers: Vec<String> = args.str_array("modifier");
            let capture_point = match self.state.capture_bridge.as_ref() {
                Some(bridge) => match bridge.admit_click(
                    &args,
                    crate::capture_admission::WindowsCaptureTarget::PrimaryDesktop,
                    input.x,
                    input.y,
                ) {
                    Ok(point) => point,
                    Err(error) => return capture_admission_refusal(error),
                },
                None if args.get("capture_id").is_some() => {
                    return capture_admission_refusal(anyhow::anyhow!(
                        "capture service is unavailable"
                    ))
                }
                None => None,
            };
            let (sx, sy) = match capture_point {
                Some((action_x, action_y)) => {
                    match crate::capture_admission::round_action_point(action_x, action_y) {
                        Ok(point) => point,
                        Err(error) => return capture_admission_refusal(error),
                    }
                }
                None => (input.x as i32, input.y as i32),
            };

            // Resolve the application window before moving the agent cursor.
            // WindowFromPoint can return a transparent layered overlay, and the
            // cursor overlay is about to occupy this exact screen point.
            let root = unsafe {
                use windows::Win32::Foundation::POINT;
                use windows::Win32::UI::WindowsAndMessaging::{
                    GetAncestor, WindowFromPoint, GA_ROOT,
                };
                let target = WindowFromPoint(POINT { x: sx, y: sy });
                if target.0.is_null() {
                    return ToolResult::error(format!("No window under screen point ({sx},{sy})."));
                }
                let root = GetAncestor(target, GA_ROOT);
                if root.0.is_null() {
                    target
                } else {
                    root
                }
            };
            let hwnd_u = root.0 as u64;

            // Animate the agent cursor to the screen point, then click.
            overlay_glide_to(&cursor_key, sx as f64, sy as f64).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: sx as f64,
                    y: sy as f64,
                },
            );

            // Click the HWND that owned the pixel before the driver overlay
            // moved there. The active SendInput path performs the foreground
            // swap and UIPI checks needed for Chromium and retained-mode apps.
            let send_result = tokio::task::spawn_blocking(move || -> anyhow::Result<u64> {
                let mod_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                crate::input::send_click_synthesized_active_mods(
                    hwnd_u, sx, sy, count, &button, &mod_refs,
                )?;
                Ok(hwnd_u)
            })
            .await;
            return match send_result {
                Ok(Ok(hwnd_u)) => {
                    let click_word = match count {
                        2 => "double-click",
                        3 => "triple-click",
                        _ => "click",
                    };
                    ToolResult::text(format!(
                        "✅ Sent {click_word} via SendInput at screen ({sx},{sy}) on HWND 0x{hwnd_u:x} (desktop scope)."
                    ))
                    // SendInput dispatched but is not driver-verifiable (no read-back).
                    .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" }))
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (elem_idx, hwnd_opt, admitted) = resolved.into_parts(args.opt_u64("window_id"));
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
        // Surface 5: explicit rejection of unknown buttons so a typo doesn't fall
        // through to the SendInput `_ => MOUSEEVENTF_LEFTDOWN/UP` default and
        // silently emit a left-click. Empty / missing keeps back-compat.
        let button_raw = args.str_or("button", "left").to_lowercase();
        if !matches!(button_raw.as_str(), "" | "left" | "right" | "middle") {
            return ToolResult::error(format!(
                "click: unknown button \"{button_raw}\" — expected one of left, right, middle."
            ));
        }
        let button = if button_raw.is_empty() {
            "left".to_string()
        } else {
            button_raw
        };
        let count = args.u64_or("count", 1) as usize;
        // macOS click `modifier` surface: held keys (cmd/shift/option/ctrl). Only
        // the SendInput rung (delivery_mode:"foreground" pixel tap, and the
        // window-less desktop-scope click above) can hold them; the background
        // UIA-Invoke / PostMessage rungs have no keyboard state to carry them, so
        // a modifier passed to a background click is necessarily ignored there.
        let modifiers: Vec<String> = args.str_array("modifier");
        let delivery = DeliveryMode::from_args(&args);
        if !modifiers.is_empty() && delivery != DeliveryMode::Foreground {
            return ToolResult::error(
                "click modifiers require delivery_mode:\"foreground\" on Windows; UIA and \
                 PostMessage cannot carry live modifier-key state",
            )
            .with_structured(json!({
                "code": "background_unavailable",
                "effect": "refused",
                "escalation": {
                    "recommended": "foreground",
                    "reason": "Windows modifier clicks require SendInput so the target observes live modifier state"
                }
            }));
        }
        // Optional `action` arg picks among the actions exposed in the
        // accessibility tree. Today this only changes behavior for MSAA
        // BUTTONDROPDOWN: `"expand"` clicks the right-edge (dropdown arrow
        // half) instead of the center (press half). Defaults to first
        // action in the element's `actions=[...]` list, which preserves
        // existing semantics for UIA elements.
        let action_req = args
            .get("action")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Resolve HWND: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::win32::list_windows_via_win32(Some(pid))
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
        let capture_point = if args.get("capture_id").is_some() {
            if elem_idx.is_some()
                || x.is_none()
                || y.is_none()
                || hwnd_opt.is_none()
                || args.get("from_zoom").and_then(Value::as_bool) == Some(true)
            {
                return capture_admission_refusal(anyhow::anyhow!(
                    "capture_id requires x, y, and an exact window_id; element and zoom coordinates are not capture-bound"
                ));
            }
            let Some(bridge) = &self.state.capture_bridge else {
                return capture_admission_refusal(anyhow::anyhow!(
                    "capture service is unavailable"
                ));
            };
            match bridge.admit_click(
                &args,
                crate::capture_admission::WindowsCaptureTarget::Window {
                    pid,
                    window_id: hwnd,
                },
                x.unwrap(),
                y.unwrap(),
            ) {
                Ok(Some(point)) => Some(point),
                Ok(None) => unreachable!("capture_id was present"),
                Err(error) => return capture_admission_refusal(error),
            }
        } else {
            None
        };

        // Arm this only after capture admission so stale or mismatched IDs have
        // no native side effect before the click is refused.
        let _noact = match (delivery != DeliveryMode::Foreground, hwnd_opt) {
            (true, Some(h)) => Some(crate::input::NoActivateGuard::arm(
                windows::Win32::Foundation::HWND(h as *mut _),
            )),
            _ => None,
        };

        if let Some(idx) = elem_idx {
            // ── MSAA dispatch (SAL/VCL targets) ────────────────────────────
            // For MSAA elements, route by role:
            //   - `action:"expand"` on BUTTONDROPDOWN → SendInput at the
            //     cached rect's right-edge (the dropdown arrow half).
            //     Opens the picker (e.g. LO Writer Font Color → SALTMPSUBFRAME).
            //   - default action / `"invoke"` → SendInput at center
            //     (matches `accDoDefaultAction` semantics for VCL controls
            //     and works through delivery_mode:"foreground"). We DO NOT call
            //     `accDoDefaultAction` here because LO's MSAA impl applies
            //     the change asynchronously and returns S_OK either way,
            //     so the visible behavior is the same as a center click.
            if let Some((SnapshotKind::Msaa, role)) = admitted
                .as_ref()
                .map(|element| (element.kind, element.msaa_role))
            {
                const ROLE_BUTTONDROPDOWN: i32 = 0x38;
                const ROLE_BUTTONMENU: i32 = 0x39;
                const ROLE_BUTTONDROPDOWNGRID: i32 = 0x3A;
                const ROLE_SPLITBUTTON: i32 = 0x3E;
                let want_expand = action_req.as_deref() == Some("expand");
                let is_dropdown_role = matches!(
                    role,
                    Some(
                        ROLE_BUTTONDROPDOWN
                            | ROLE_BUTTONMENU
                            | ROLE_BUTTONDROPDOWNGRID
                            | ROLE_SPLITBUTTON
                    )
                );
                let (tx, ty) = if want_expand && is_dropdown_role {
                    match admitted.as_ref().and_then(|element| element.rect) {
                        Some((_l, t, r, b)) => {
                            // Right-edge of the SplitButton — the dropdown
                            // arrow half. -4 puts the click safely inside
                            // the arrow region for the typical ~12-16 px
                            // arrow width VCL uses.
                            (r - 4, (t + b) / 2)
                        }
                        None => {
                            return ToolResult::error(format!(
                                "MSAA element [{idx}] has no cached rect — \
                                 cannot dispatch action:\"expand\". Re-run \
                                 get_window_state to refresh the cache."
                            ));
                        }
                    }
                } else if want_expand && !is_dropdown_role {
                    return ToolResult::error(format!(
                        "action:\"expand\" requested for MSAA element [{idx}] \
                         but its role ({role:?}) is not a dropdown button. \
                         Use action:\"invoke\" or omit the action arg."
                    ));
                } else {
                    match admitted.as_ref().map(|element| element.center) {
                        Some(v) => v,
                        None => {
                            return ToolResult::error(format!(
                                "MSAA element [{idx}] not in cache for hwnd={hwnd}. \
                             Call get_window_state first."
                            ))
                        }
                    }
                };
                pin_overlay_above(&cursor_key, hwnd);
                overlay_glide_to_target(
                    &cursor_key,
                    tx as f64,
                    ty as f64,
                    element_target_rect(admitted.as_ref(), (0, 0)),
                )
                .await;
                crate::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::ClickPulse {
                        x: tx as f64,
                        y: ty as f64,
                    },
                );
                let btn_fg = button.clone();
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let mods_owned = modifiers.clone();
                let activate = delivery == DeliveryMode::Foreground;
                let send_result = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        let mod_refs: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                        if activate {
                            crate::input::send_click_synthesized_active_mods(
                                hwnd, tx, ty, count, &btn_fg, &mod_refs,
                            )
                        } else {
                            crate::input::send_click_synthesized_mods(
                                hwnd, tx, ty, count, &btn_fg, &mod_refs,
                            )
                        }
                    }
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                let half = if want_expand { "dropdown" } else { "press" };
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Performed SendInput click on MSAA [{idx}] {half} half at ({tx},{ty})."
                    ))
                    .with_structured(
                        json!({ "path": "msaa", "verified": false, "effect": "unverifiable" }),
                    ),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }

            // UIA path: get cached center (no COM call needed — captured at walk time).
            let (cx, cy) = match admitted.as_ref().map(|element| element.center) {
                Some(v) => v,
                None => {
                    return ToolResult::error(format!(
                        "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                    ))
                }
            };
            // NB: the off-screen guard is applied per-delivery-path below (the
            // foreground SendInput tap and the background coordinate injection),
            // NOT here — a background UIA Invoke fires the element's handler
            // regardless of whether the element is scrolled into view, so a broad
            // guard would wrongly block legitimate actions like opening a modal
            // from an off-screen button.
            // Step 2: pin overlay to target window, then animate to screen coords.
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to_target(
                &cursor_key,
                cx as f64,
                cy as f64,
                element_target_rect(admitted.as_ref(), (0, 0)),
            )
            .await;
            // Step 3: click pulse + actual click.
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: cx as f64,
                    y: cy as f64,
                },
            );
            let btn = button.clone();

            // An explicit accessibility action is a semantic request, not a
            // pixel gesture hint. Route expand through ExpandCollapsePattern
            // even when foreground delivery was allowed; this reliably opens
            // WPF/WinUI menus and tree nodes whose visual click target is
            // transient or scroll-adjusted.
            if action_req.as_deref() == Some("expand") {
                let expand = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || -> anyhow::Result<()> {
                        let _admission = &admitted;
                        use windows::core::Interface;
                        use windows::Win32::UI::Accessibility::{
                            IUIAutomationElement, IUIAutomationExpandCollapsePattern,
                            UIA_ExpandCollapsePatternId,
                        };

                        let retained = admitted.as_ref().ok_or_else(|| {
                            anyhow::anyhow!("element [{idx}] is not in the UIA cache")
                        })?;
                        if !retained.is_uia() {
                            anyhow::bail!("element [{idx}] is not a UIA element");
                        }
                        let element = std::mem::ManuallyDrop::new(unsafe {
                            IUIAutomationElement::from_raw(retained.as_ptr() as *mut _)
                        });
                        let pattern = unsafe {
                            element
                                .GetCurrentPattern(UIA_ExpandCollapsePatternId)
                                .and_then(|value| {
                                    value.cast::<IUIAutomationExpandCollapsePattern>()
                                })
                        }
                        .map_err(|error| {
                            anyhow::anyhow!("ExpandCollapsePattern unavailable: {error}")
                        })?;
                        let result =
                            crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                                pattern.Expand()
                            });
                        std::mem::forget(element);
                        result.map_err(|error| {
                            anyhow::anyhow!("ExpandCollapse.Expand failed: {error}")
                        })
                    }
                })
                .await;
                return match expand {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Expanded UIA element [{idx}] via ExpandCollapsePattern."
                    ))
                    .with_structured(json!({ "path": "uia_expand_collapse", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(error)) => ToolResult::error(error.to_string()),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }

            // delivery_mode:"foreground" — skip UIA Invoke and use SendInput at the
            // cached element center. The caller explicitly chose foreground
            // delivery; UIA Invoke would be background-safe (which they
            // rejected) and might fail on elements that don't support
            // InvokePattern anyway.
            if delivery == DeliveryMode::Foreground {
                // Foreground delivery is a real SendInput tap at (cx,cy); if the
                // element is off-screen, scroll it into view and re-resolve so the
                // tap doesn't land on the taskbar, else keep the clean failure.
                // Modified selection needs one stronger preflight: some WPF
                // ScrollViewers report a clipped item as on-screen because its
                // stale rectangle is still inside the outer HWND. Ask the item
                // to scroll itself into view before trusting that rectangle.
                let recorded_center = (cx, cy);
                let (cx, cy) = if !modifiers.is_empty() {
                    admitted
                        .clone()
                        .and_then(|retained| {
                            retained.is_uia().then(|| unsafe {
                                crate::uia::scroll::scroll_into_view_and_recenter(
                                    hwnd,
                                    retained.as_ptr(),
                                )
                            })
                        })
                        .flatten()
                        .unwrap_or((cx, cy))
                } else {
                    (cx, cy)
                };
                let (cx, cy) = match resolve_onscreen_point_with_scroll(
                    &admitted,
                    hwnd,
                    idx,
                    cx,
                    cy,
                    "a foreground click",
                ) {
                    Ok(p) => p,
                    Err(result) => return result,
                };
                if (cx, cy) != recorded_center {
                    crate::recording_hooks::capture_dispatch_click_target(hwnd, pid, cx, cy);
                }
                let btn_fg = btn.clone();
                let mods_owned = modifiers.clone();
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        let mod_refs: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                        crate::input::send_click_synthesized_active_mods(
                            hwnd, cx, cy, count, &btn_fg, &mod_refs,
                        )
                    }
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Performed SendInput click on [{idx}] at screen ({cx},{cy}) (delivery_mode:foreground)."
                    ))
                    .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            // UIA and posted-message routes can report success without a
            // user-visible effect on an iconic target. Refuse before any
            // background route selects or dispatches its transport.
            if let Some(result) = minimized_element_refusal(hwnd, idx, cx, cy, "clicking") {
                return result;
            }
            // delivery_mode:"background" on WinUI3 for double / right / middle: single
            // left falls through to the UIA Invoke path below (already drives
            // WinUI3). Double-left lands via a double UIA Invoke; right/middle
            // can't both land and hold the contract → structured error.
            if delivery == DeliveryMode::Background
                && (count > 1 || btn == "right" || btn == "middle")
            {
                if let Some(r) =
                    winui3_background_gesture(&admitted, pid, hwnd, Some(idx), count, &btn).await
                {
                    return r;
                }
            }
            // Chromium's UIA Invoke raises a fully occluded renderer, while a
            // targeted PostMessage left click reaches the renderer without
            // changing foreground, z-order, or the real cursor. Keep AX for
            // target resolution and use the posted-message transport only for
            // the empirically verified single-left-click shape.
            if delivery == DeliveryMode::Background
                && btn == "left"
                && count == 1
                && crate::input::is_chromium_target_window(hwnd)
            {
                let posted = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::input::post_click_screen(hwnd, cx, cy, count, &btn)
                    }
                })
                .await;
                return match posted {
                    Ok(Ok(())) => background_element_click_result(
                        format!(
                            "✅ Posted click on Chromium element [{idx}] at screen ({cx},{cy}) \
                         (background, no foreground swap)."
                        ),
                        ActionTransport::WindowsPostMessage,
                        vec![],
                    ),
                    Ok(Err(error)) => ToolResult::error(error.to_string()),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }
            // Try UIA Invoke first (it works for UWP / modern XAML / web
            // content where PostMessage(WM_LBUTTONDOWN) hits the outer
            // HWND but never reaches the inner XAML/composition element).
            // Fall through to PostMessage when:
            //   - element doesn't support InvokePattern (most text inputs,
            //     non-interactive elements)
            //   - right-click (UIA's ExpandCollapse / ShowContextMenu are
            //     different patterns; keep PostMessage for now)
            //   - count > 1 (double-click semantics aren't an Invoke
            //     concept — PostMessage produces the actual WM_LBUTTONDBLCLK)
            let use_uia_invoke = (btn == "left" || btn == "middle") && count == 1;
            let result = tokio::task::spawn_blocking({ let admitted = admitted.clone(); move || -> anyhow::Result<BackgroundElementClick> {
                let _admission = &admitted;
                let mut failed_calls = Vec::new();
                // Direct Chromium UIA Invoke can return S_OK without firing a
                // DOM event while occluded. Try the honest coordinate actuator
                // first: it lands while visible and reports occlusion without
                // raising the window when hidden.
                if delivery == DeliveryMode::Background
                    && crate::input::is_chromium_target_window(hwnd)
                {
                    let (cx, cy) = resolve_onscreen_point_with_scroll(
                        &admitted, hwnd, idx, cx, cy, "clicking",
                    )
                    .map_err(|result| anyhow::anyhow!(tool_result_text(result)))?;
                    return Ok(BackgroundElementClick::Inject { x: cx, y: cy, failed_calls });
                }
                if use_uia_invoke {
                    // Retain the element out of the cache (AddRef under the
                    // cache lock) so it can't be freed by a concurrent
                    // get_window_state on the same (pid, hwnd) while this click
                    // is mid-flight (use-after-free → daemon crash). The guard
                    // lives to the end of this `if let` block, past every UIA
                    // pattern dispatch below; its Release fires when it drops.
                    if let Some(element_guard) = admitted.as_ref() {
                        let ptr = element_guard.as_ptr();
                        use windows::Win32::UI::Accessibility::{
                            IUIAutomationElement, IUIAutomationInvokePattern,
                            IUIAutomationTogglePattern, IUIAutomationSelectionItemPattern,
                            IUIAutomationExpandCollapsePattern,
                            UIA_InvokePatternId, UIA_TogglePatternId,
                            UIA_SelectionItemPatternId, UIA_ExpandCollapsePatternId,
                        };
                        use windows::core::Interface;
                        let elem: IUIAutomationElement =
                            unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };

                        // Try patterns in order of click-semantics specificity:
                        //   1. Invoke      - buttons / hyperlinks (canonical)
                        //   2. Toggle      - checkboxes (no Invoke)
                        //   3. SelectionItem - radio buttons, listbox items
                        //   4. ExpandCollapse - combo boxes, tree nodes
                        // Each call is wrapped in the UWP foreground-steal bypass.
                        //
                        // Why this order: PR #1699's WinUI3 control parity tests
                        // surfaced these pattern-dispatch gaps. Without these
                        // fallthroughs, cua-driver couldn't drive WinUI3
                        // CheckBox / RadioButton / ComboBox because PostMessage
                        // doesn't reach their handlers (CoreInput dispatcher).
                        let invoke_result = unsafe { elem.GetCurrentPattern(UIA_InvokePatternId) };
                        if let Ok(pattern) = invoke_result {
                            if let Ok(inv) = pattern.cast::<IUIAutomationInvokePattern>() {
                                let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize, || unsafe { inv.Invoke() });
                                match outcome {
                                    Ok(()) => {
                                        std::mem::forget(elem);
                                        return Ok(BackgroundElementClick::Semantic { message: format!(
                                            "✅ Performed UIA Invoke on [{idx}] (screen ({cx},{cy}))."
                                        ), transport: ActionTransport::WindowsUiaInvoke, failed_calls });
                                    }
                                    Err(e) => tracing::debug!(target: "click",
                                        "UIA Invoke on [{idx}]: {e}, trying Toggle"),
                                }
                                failed_calls.push(ActionTransport::WindowsUiaInvoke);
                            }
                        }
                        let toggle_result = unsafe { elem.GetCurrentPattern(UIA_TogglePatternId) };
                        if let Ok(pattern) = toggle_result {
                            if let Ok(tg) = pattern.cast::<IUIAutomationTogglePattern>() {
                                let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize, || unsafe { tg.Toggle() });
                                if outcome.is_ok() {
                                    std::mem::forget(elem);
                                    return Ok(BackgroundElementClick::Semantic { message: format!(
                                        "✅ Performed UIA Toggle on [{idx}] (screen ({cx},{cy}))."
                                    ), transport: ActionTransport::WindowsUiaToggle, failed_calls });
                                }
                                failed_calls.push(ActionTransport::WindowsUiaToggle);
                            }
                        }
                        let sel_result = unsafe { elem.GetCurrentPattern(UIA_SelectionItemPatternId) };
                        if let Ok(pattern) = sel_result {
                            if let Ok(si) = pattern.cast::<IUIAutomationSelectionItemPattern>() {
                                let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize, || unsafe { si.Select() });
                                if outcome.is_ok() {
                                    std::mem::forget(elem);
                                    return Ok(BackgroundElementClick::Semantic { message: format!(
                                        "✅ Performed UIA SelectionItem.Select on [{idx}] (screen ({cx},{cy}))."
                                    ), transport: ActionTransport::WindowsUiaSelection, failed_calls });
                                }
                                failed_calls.push(ActionTransport::WindowsUiaSelection);
                            }
                        }
                        let exp_result = unsafe { elem.GetCurrentPattern(UIA_ExpandCollapsePatternId) };
                        if let Ok(pattern) = exp_result {
                            if let Ok(ec) = pattern.cast::<IUIAutomationExpandCollapsePattern>() {
                                let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize, || unsafe { ec.Expand() });
                                if outcome.is_ok() {
                                    std::mem::forget(elem);
                                    return Ok(BackgroundElementClick::Semantic { message: format!(
                                        "✅ Performed UIA ExpandCollapse.Expand on [{idx}] (screen ({cx},{cy}))."
                                    ), transport: ActionTransport::WindowsUiaExpandCollapse, failed_calls });
                                }
                                failed_calls.push(ActionTransport::WindowsUiaExpandCollapse);
                            }
                        }
                        std::mem::forget(elem);
                    }
                }
                if delivery == DeliveryMode::Background
                    && crate::input::delivery::would_be_silently_dropped(
                        hwnd,
                        EventKind::MouseClick,
                    )
                {
                    let (cx, cy) = resolve_onscreen_point_with_scroll(
                        &admitted,
                        hwnd,
                        idx,
                        cx,
                        cy,
                        "clicking",
                    )
                    .map_err(|result| anyhow::anyhow!(tool_result_text(result)))?;
                    return Ok(BackgroundElementClick::Inject { x: cx, y: cy, failed_calls });
                }
                Ok(BackgroundElementClick::Post { x: cx, y: cy, failed_calls })
            } }).await;
            let plan = match result {
                Ok(Ok(plan)) => plan,
                Ok(Err(e)) => return ToolResult::error(e.to_string()),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            };
            let (x, y, failed_calls, inject) = match plan {
                BackgroundElementClick::Semantic {
                    message,
                    transport,
                    failed_calls,
                } => {
                    return background_element_click_result(message, transport, failed_calls);
                }
                BackgroundElementClick::Inject { x, y, failed_calls } => (x, y, failed_calls, true),
                BackgroundElementClick::Post { x, y, failed_calls } => (x, y, failed_calls, false),
            };
            // Return to the dispatch task before capturing: task-local recording
            // context does not propagate into the blocking scroll resolver.
            if (x, y) != (cx, cy) {
                crate::recording_hooks::capture_dispatch_click_target(hwnd, pid, x, y);
            }
            let btn = button.clone();
            let action_name = if btn == "right" {
                "ShowMenu"
            } else {
                "PostMessage click"
            };
            let sent = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    if inject {
                        crate::input::inject_click_screen(hwnd, x, y, count, &btn)
                    } else {
                        crate::input::post_click_screen(hwnd, x, y, count, &btn)
                    }
                }
            })
            .await;
            match sent {
                Ok(Ok(())) => {
                    let (message, transport) = if inject {
                        (format!("✅ Injected click on [{idx}] (screen ({x},{y}), background, no foreground swap)."),
                            ActionTransport::WindowsTargetedInjection)
                    } else {
                        (
                            format!("✅ Performed {action_name} on [{idx}] (screen ({x},{y}))."),
                            ActionTransport::WindowsPostMessage,
                        )
                    };
                    background_element_click_result(message, transport, failed_calls)
                }
                Ok(Err(error)) if inject => {
                    crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        error.to_string(),
                    )
                }
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            }
        } else if let (Some(mut px), Some(mut py)) = (x, y) {
            let from_zoom = args
                .get("from_zoom")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Some((mapped_x, mapped_y)) = capture_point {
                let (mapped_x, mapped_y) =
                    match crate::capture_admission::round_action_point(mapped_x, mapped_y) {
                        Ok(point) => point,
                        Err(error) => return capture_admission_refusal(error),
                    };
                px = f64::from(mapped_x);
                py = f64::from(mapped_y);
            } else if from_zoom {
                match self.state.zoom_context(&args, pid, Some(hwnd)) {
                    Ok(ctx) => {
                        let (wx, wy) = ctx.zoom_to_window(px, py);
                        px = wx;
                        py = wy;
                    }
                    Err(refusal) => return refusal,
                }
            } else if !args.bool_or("_native_coordinates", false) {
                let ratio = match screenshot_scale(&self.state, &args, pid, Some(hwnd)) {
                    Ok(ratio) => ratio,
                    Err(refusal) => return refusal,
                };
                px *= ratio;
                py *= ratio;
            }
            // px/py are bitmap pixels — see `bitmap_to_screen` for the
            // mapping (DWM-frame top-left + 1-px inset, NOT ClientToScreen).
            let (sx_i, sy_i) = bitmap_to_screen(hwnd, px as i32, py as i32);
            let (sx, sy) = (sx_i as f64, sy_i as f64);
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, sx, sy).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
            );
            let btn = button.clone();
            // Vision-mode (x, y) dispatch is **layered**:
            //
            //   1. UIA hit-test in the target HWND's subtree. If the
            //      deepest InvokePattern-bearing element at (sx, sy) is
            //      found, fire `Invoke()`. Works occluded, no focus
            //      steal, and is the **only** path that lands on UWP /
            //      WebView2 / DirectComposition surfaces — those route
            //      input through `Windows.UI.Input`, not WM_LBUTTONDOWN.
            //
            //   2. If UIA completes with a miss (no Invokable element under the
            //      pixel inside `hwnd`, or `hwnd` has no useful UIA
            //      tree at all), fall through to
            //      `PostMessage(WM_LBUTTONDOWN/UP)` against the deepest
            //      child HWND at the screen point. Covers plain Win32
            //      controls without UIA InvokePattern (most edit
            //      fields, custom-drawn widgets, native containers) and
            //      apps that don't expose UIA at all.
            //
            // The combination closes the gap macOS gets for free from
            // `CGEventPostToPSN` (per-pid event routing): UIA covers
            // UWP-style targets without z-order tricks, PostMessage
            // covers plain Win32 without z-order tricks. Neither path
            // moves the OS cursor or activates the receiver.
            //
            // UIA path runs only for single left/middle clicks —
            // right-click (UIA has no by-coord `ShowContextMenu`) and
            // multi-click (`Invoke()` is single-fire by definition)
            // skip directly to PostMessage.
            // delivery_mode:"foreground" short-circuit — caller explicitly chose
            // foreground delivery. Skip the UIA hit-test (UIA Invoke is
            // background-safe and would deliver the click without the
            // foreground swap the caller asked for).
            if delivery == DeliveryMode::Foreground {
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let mods_owned = modifiers.clone();
                let send_result = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        let mod_refs: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                        crate::input::send_click_synthesized_active_mods(
                            hwnd, sx as i32, sy as i32, count, &btn, &mod_refs,
                        )
                    }
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => {
                        let click_word = match count {
                            2 => "double-click",
                            3 => "triple-click",
                            _ => "click",
                        };
                        ToolResult::text(format!(
                            "✅ Sent {click_word} via SendInput to pid {pid} at ({sx},{sy}) (delivery_mode:foreground)."
                        ))
                        .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" }))
                    }
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }

            // delivery_mode:"background" on WinUI3 for double / right / middle (pixel
            // path, no element_token → no cached element to UIA-Invoke): these
            // can't both land and hold the contract on WinUI3 (posted input is
            // ignored, the pen injector steals foreground) → structured error.
            if delivery == DeliveryMode::Background
                && (count > 1 || btn == "right" || btn == "middle")
            {
                if let Some(r) =
                    winui3_background_gesture(&admitted, pid, hwnd, None, count, &btn).await
                {
                    return r;
                }
            }
            // Match the AX-addressed Chromium route above: the point remains
            // PX-resolved, but transport uses the background-safe posted
            // message path proven against the fully occluded fixture.
            if delivery == DeliveryMode::Background
                && btn == "left"
                && count == 1
                && crate::input::is_chromium_target_window(hwnd)
            {
                let posted = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::input::post_click_screen(hwnd, sx_i, sy_i, count, &btn)
                    }
                })
                .await;
                return match posted {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Posted click to Chromium pid {pid} at ({sx},{sy}) \
                         (background, no foreground swap)."
                    ))
                    .with_structured(json!({
                        "path": "post_message",
                        "verified": false,
                        "effect": "unverifiable"
                    })),
                    Ok(Err(error)) => ToolResult::error(error.to_string()),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }
            // As above, bypass Chromium's false-positive UIA Invoke and use
            // the coordinate actuator before attempting any accessibility hit
            // test. The actuator itself distinguishes visible delivery from a
            // fully occluded structured refusal.
            if delivery == DeliveryMode::Background && crate::input::is_chromium_target_window(hwnd)
            {
                let btn2 = btn.clone();
                let inj = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::input::inject_click_screen(hwnd, sx as i32, sy as i32, count, &btn2)
                    }
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected click to pid {pid} at ({sx},{sy}) (background, no foreground swap)."
                    ))
                    .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(error)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        error.to_string(),
                    ),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }
            let use_uia = (btn == "left" || btn == "middle") && count == 1;
            if use_uia {
                let outcome = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::uia::windows_enum::try_invoke_in_window_at_point(
                            hwnd as isize,
                            sx as i32,
                            sy as i32,
                        )
                    }
                })
                .await
                .unwrap_or(crate::uia::windows_enum::PointInvokeOutcome::Unavailable);
                if let Some(result) = finish_pixel_uia_attempt(outcome, pid, sx_i, sy_i) {
                    return result;
                }
            }

            // UIA completed with a miss or was not applicable. Known dropped surfaces other than direct
            // Chromium (handled above) get one targeted injection attempt.
            if delivery == DeliveryMode::Background
                && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseClick)
            {
                let btn2 = btn.clone();
                let inj = tokio::task::spawn_blocking({
                    let admitted = admitted.clone();
                    move || {
                        let _admission = &admitted;
                        crate::input::inject_click_screen(hwnd, sx as i32, sy as i32, count, &btn2)
                    }
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected click to pid {pid} at ({sx},{sy}) (background, no foreground swap)."
                    ))
                    .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(error)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        error.to_string(),
                    ),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }

            // delivery_mode:"background", non-dropped + non-UIA click. This
            // includes plain Win32 targets with no invokable UIA element.
            // Attempt PostMessage without a
            // foreground swap; the caller must verify the resulting fixture state.
            // Foreground was handled at the top, and known dropped surfaces use
            // the targeted-injection path above.

            // bitmap pixels -> screen (DWM-frame origin + inset). Use
            // post_click_screen so we don't double-ClientToScreen.
            let result = tokio::task::spawn_blocking({
                let admitted = admitted.clone();
                move || {
                    let _admission = &admitted;
                    crate::input::post_click_screen(hwnd, sx_i, sy_i, count, &btn)
                }
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    // Match Swift text format: "✅ Posted click/double-click/triple-click to pid X."
                    let click_word = match count {
                        2 => "double-click",
                        3 => "triple-click",
                        _ => "click",
                    };
                    posted_pixel_click_result(pid, click_word)
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            // Match Swift's error wording for the missing-target case.
            ToolResult::error("Provide element_token or (x, y) to address the click target.")
        }
    }
}

#[cfg(test)]
mod pixel_click_transport_tests;

#[cfg(test)]
mod background_element_click_record_tests;

#[cfg(test)]
mod click_capture_id_schema_tests;
