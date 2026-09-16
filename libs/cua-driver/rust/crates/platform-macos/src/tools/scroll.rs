use async_trait::async_trait;
use cua_driver_contract::{ScrollBy, ScrollDirection, ScrollInput};
use cua_driver_core::tool::spawn_native;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{element_screen_center, kAXErrorSuccess, perform_action, AXUIElementRef};
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;

use super::ToolState;

/// Per-notch pixel step for the wheel path. `page` rolls a screenful-ish chunk,
/// `line` a few text lines — tuned to feel like a real wheel notch. Runtime
/// tuning deferred (host is screen-recording); centralized here for one-line edits.
const WHEEL_STEP_LINE_PX: i32 = 120;
const WHEEL_STEP_PAGE_PX: i32 = 600;

/// Resolved pixel-wheel target in screen space, plus optional window-local
/// stamp + window id for backgrounded delivery.
struct WheelTarget {
    screen_x: f64,
    screen_y: f64,
    win_local: Option<(f64, f64)>,
    wid: Option<u32>,
}

impl WheelTarget {
    fn from_retained(
        element: &crate::ax::element_resolver::RetainedElement,
        wid: Option<u32>,
    ) -> anyhow::Result<Self> {
        let wid = wid.ok_or_else(|| anyhow::anyhow!("retained target has no native window"))?;
        let (x, y) = unsafe { element_screen_center(element.checked_ptr()? as AXUIElementRef) }
            .ok_or_else(|| anyhow::anyhow!("retained target geometry is unavailable"))?;
        let bounds = crate::windows::window_bounds_by_id(wid)
            .ok_or_else(|| anyhow::anyhow!("target window geometry is unavailable"))?;
        let local = (x - bounds.x, y - bounds.y);
        if local.0 < 0.0 || local.1 < 0.0 || local.0 >= bounds.width || local.1 >= bounds.height {
            anyhow::bail!("retained scroll target is outside the native window");
        }
        Ok(Self {
            screen_x: x,
            screen_y: y,
            win_local: Some(local),
            wid: Some(wid),
        })
    }
}

fn after_exact_target_gate<T>(
    gate: Result<(), ToolResult>,
    action: impl FnOnce() -> T,
) -> Result<T, ToolResult> {
    gate?;
    Ok(action())
}

pub struct ScrollTool {
    state: Arc<ToolState>,
}

impl ScrollTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "scroll".into(),
        description: "Scroll the target pid. Two paths, picked by how you address the scroll:\n\n\
            • **Targeted wheel path** — when you pass a target, either \
            `element_token` (preferred) or window-local `x, y` pixels: \
            the driver synthesizes a real mouse-wheel event (CGEventCreateScrollWheelEvent, \
            at that screen point. The renderer hit-tests the wheel at the \
            cursor, so the scroll lands on whatever element is under the point — exactly \
            like physically rolling the wheel over it. This is the ONLY way to scroll a \
            nested `overflow:auto` region (e.g. a scrollable <div> with no tabindex): such \
            regions never take keyboard focus, so the keystroke path below no-ops on them. \
            Use this for inner/nested scrollers in web views.\n\n\
            • **Keystroke path (focused region)** — when you pass NO target (just pid + \
            direction): synthesizes PageDown/PageUp (by='page') or Down/Up arrows \
            (by='line'); horizontal uses Left/Right arrows. Drives the focused / page \
            scroller only.\n\n\
            Mapping: by='page' → larger step; by='line' → smaller step; amount = number of \
            wheel notches (targeted path) or keystroke repetitions (keystroke path).".into(),
        input_schema: serde_json::json!({
            "type": "object",
            // `pid` conditionally required (validated in code), not pinned in the
            // schema — keeps the contract consistent across platforms.
            "required": ["direction"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid": { "type": "integer" },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction."
                },
                "by": {
                    "type": "string",
                    "enum": ["line", "page"],
                    "description": "Scroll granularity. Default: line."
                },
                "amount": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "Pixel-wheel path: number of wheel notches. Keystroke path: number of keystroke repetitions. Default: 3."
                },
                "window_id": { "type": "integer" },
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                "x": { "type": "number", "description": "Window-local screenshot X (top-left origin of the PNG from get_window_state). With `y`, routes through the pixel-wheel path at this point — use for a scrollable surface that isn't in the AX tree. Requires window_id to anchor the window→screen conversion." },
                "y": { "type": "number", "description": "Window-local screenshot Y. See `x`." },
                "scope": { "type": "string", "enum": ["window", "desktop"], "default": "window", "description": "Use desktop with x,y and no pid/window_id for native get_desktop_state screenshot coordinates." },
                "delivery_mode": cua_driver_core::tool_schema::delivery_mode_schema()
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    })
}

#[async_trait]
impl Tool for ScrollTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<ScrollInput>("scroll", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let (x, y) = (input.x, input.y);
            let direction = input.direction.as_str();
            let by = input.by.unwrap_or(ScrollBy::Line).as_str();
            let amount = input.amount.unwrap_or(3).clamp(1, 50) as usize;
            let step = if input.by == Some(ScrollBy::Page) {
                WHEEL_STEP_PAGE_PX
            } else {
                WHEEL_STEP_LINE_PX
            };
            let (delta_y, delta_x) = match input.direction {
                ScrollDirection::Down => (-step, 0),
                ScrollDirection::Up => (step, 0),
                ScrollDirection::Right => (0, -step),
                ScrollDirection::Left => (0, step),
            };
            let (x, y) = super::desktop_screenshot_point(x, y).await;
            let result = spawn_native(move || {
                crate::input::mouse::scroll_wheel_desktop(x, y, delta_y, delta_x, amount)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Scrolled desktop {direction} by {by} × {amount} at ({x:.1}, {y:.1})."
                ))
                .with_structured(serde_json::json!({
                    "scope": "desktop",
                    "path": "hid",
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::from_native_error(
                    error,
                    cua_driver_core::action_record::RequestedDelivery::Foreground,
                ),
                Err(error) => ToolResult::error(format!("desktop scroll task failed: {error}")),
            };
        }
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // delivery_mode: foreground briefly fronts the window before the
        // pixel-wheel dispatch (the explicit last resort for surfaces that drop
        // background CGEvents). Only the pixel-wheel path honors it; the
        // keystroke path is background-by-design and untouched.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        if !delivery_mode.is_foreground() && crate::browser::ElectronJs::is_electron(pid) {
            return ToolResult::error(
                "Background scroll is unavailable for Electron/Chromium windows on macOS."
                    .to_owned(),
            )
            .with_structured(serde_json::json!({ "code": "background_unavailable" }));
        }
        let direction = match args.require_str("direction") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let by = args.str_or("by", "line");
        let amount = args.u64_or("amount", 3) as usize;
        // Surface 6: element_token / element_index precedence.
        let window_id_arg = args.opt_u64("window_id");
        let resolved =
            match crate::ax::element_resolver::resolve_element_args(pid, &args, "scroll").await {
                Ok(r) => r,
                Err(e) => return e,
            };
        let (_, window_id, pre_focus_guard) = resolved.into_parts(window_id_arg);
        let window_id = match super::native_window_id(window_id) {
            Ok(window_id) => window_id,
            Err(error) => return error,
        };
        let pre_focus_ptr: Option<usize> = pre_focus_guard.as_ref().map(|g| g.as_ptr());

        let mut _mutation_lease: Option<super::BackgroundMutationLease> = None;

        // AppKit exposes vertical scroll-bar buttons beneath the text area's
        // AXScrollArea parent. Pressing those controls is a true
        // background-safe scroll: no activation, z-order change, or cursor move.
        if matches!(direction.as_str(), "up" | "down") {
            if let (Some(element_guard), Some(wid)) = (pre_focus_guard.clone(), window_id) {
                if !delivery_mode.is_foreground() {
                    if let Some(lease) = _mutation_lease.as_ref() {
                        if let Err(refusal_result) = lease
                            .gate_again(
                                wid,
                                pre_focus_ptr,
                                cua_driver_core::background_input::BackgroundAction::AxSemantic,
                            )
                            .await
                        {
                            return refusal_result;
                        }
                    } else {
                        match super::gate_background_window_action(
                            pid,
                            wid,
                            pre_focus_ptr,
                            cua_driver_core::background_input::BackgroundAction::AxSemantic,
                        )
                        .await
                        {
                            Ok(lease) => _mutation_lease = Some(lease),
                            Err(refusal_result) => return refusal_result,
                        }
                    }
                }
                let direction_for_ax = direction.clone();
                let by_for_ax = by.clone();
                let foreground = delivery_mode.is_foreground();
                let ax_result = spawn_native(move || -> anyhow::Result<(bool, bool)> {
                    if foreground {
                        let mut delivered = false;
                        let fronted = crate::input::skylight::with_foreground_assist(
                            pid as libc::pid_t,
                            wid,
                            || {
                                delivered = unsafe {
                                    scroll_native_text_area(
                                        element_guard.checked_ptr()? as AXUIElementRef,
                                        &direction_for_ax,
                                        &by_for_ax,
                                        amount,
                                    )?
                                };
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                Ok(())
                            },
                        )?;
                        Ok((delivered, fronted))
                    } else {
                        Ok((
                            unsafe {
                                scroll_native_text_area(
                                    element_guard.checked_ptr()? as AXUIElementRef,
                                    &direction_for_ax,
                                    &by_for_ax,
                                    amount,
                                )?
                            },
                            false,
                        ))
                    }
                })
                .await;
                match ax_result {
                    Ok(Ok((true, fronted))) => {
                        return ToolResult::text(format!(
                        "✅ Scrolled native macOS control {direction} by {by} × {amount} through AX."
                    ))
                    .with_structured(serde_json::json!({
                        "path": if fronted { "ax_fg" } else { "ax" },
                        "verified": false,
                        "effect": "unverifiable"
                    }));
                    }
                    Ok(Ok((false, _))) => {}
                    Ok(Err(error)) => {
                        return ToolResult::from_native_error(
                            error,
                            if delivery_mode.is_foreground() {
                                cua_driver_core::action_record::RequestedDelivery::Foreground
                            } else {
                                cua_driver_core::action_record::RequestedDelivery::Background
                            },
                        );
                    }
                    Err(error) => {
                        return ToolResult::error(format!("Native AX scroll task failed: {error}"));
                    }
                }
            }
        }

        // ── Targeted wheel path ─────────────────────────────────────────────
        // A target — element (preferred) OR window-local x,y — routes the scroll
        // through a synthesized mouse-wheel event at that screen point, so the
        // renderer's hit-test delivers it to whatever element is under the
        // cursor. This is the ONLY way to scroll a nested overflow:auto region
        // that never takes keyboard focus (the keystroke path below no-ops on
        // it). No user-facing flag: presence of a target IS the switch.
        let x_arg = args
            .opt_f64("x")
            .or_else(|| args.opt_i64("x").map(|v| v as f64));
        let y_arg = args
            .opt_f64("y")
            .or_else(|| args.opt_i64("y").map(|v| v as f64));

        // Per-notch step + direction→delta mapping (sign convention lives
        // here; the mouse primitive stays sign-agnostic). macOS: +y reveals
        // content ABOVE, -y reveals BELOW; +x reveals LEFT, -x reveals RIGHT.
        let step = if by == "page" {
            WHEEL_STEP_PAGE_PX
        } else {
            WHEEL_STEP_LINE_PX
        };
        let (delta_y, delta_x): (i32, i32) = match direction.as_str() {
            "down" => (-step, 0),
            "up" => (step, 0),
            "right" => (0, -step),
            "left" => (0, step),
            _ => (-step, 0),
        };

        // Resolve a screen-space wheel target, if a target was supplied.
        let wheel_target: Option<WheelTarget> = if let Some(element_ptr) = pre_focus_ptr {
            // Revealing an element is itself an AX mutation. Prove that the
            // freshly resolved element still belongs to the exact requested window before
            // AXScrollToVisible for every direction, then keep the lease for the
            // stricter pointer revalidation below.
            let semantic_gate = if !delivery_mode.is_foreground() {
                if let Some(wid) = window_id {
                    if let Some(lease) = _mutation_lease.as_ref() {
                        lease
                            .gate_again(
                                wid,
                                Some(element_ptr),
                                cua_driver_core::background_input::BackgroundAction::AxSemantic,
                            )
                            .await
                    } else {
                        match super::gate_background_window_action(
                            pid,
                            wid,
                            Some(element_ptr),
                            cua_driver_core::background_input::BackgroundAction::AxSemantic,
                        )
                        .await
                        {
                            Ok(lease) => {
                                _mutation_lease = Some(lease);
                                Ok(())
                            }
                            Err(refusal) => Err(refusal),
                        }
                    }
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            };
            // Element path: wheel at the element's screen-space center. Both AX
            // coordinates and window bounds are logical top-left points, so no
            // Retina scaling is needed here.
            let wid = window_id;
            let target_guard = pre_focus_guard.clone();
            let target_task = spawn_native(move || {
                let element = target_guard
                    .as_ref()
                    .ok_or_else(|| ToolResult::error("retained scroll target is unavailable"))?;
                let pointer = element
                    .checked_ptr()
                    .map_err(|error| ToolResult::error(error.to_string()))?
                    as AXUIElementRef;
                let actions = unsafe { crate::ax::bindings::copy_action_names_checked(pointer) }
                    .map_err(|code| {
                        ToolResult::error(format!("native action read failed: {code}"))
                    })?;
                if actions.iter().any(|action| action == "AXScrollToVisible") {
                    let status = after_exact_target_gate(semantic_gate, || unsafe {
                        perform_action(pointer, "AXScrollToVisible")
                    })?;
                    if status != kAXErrorSuccess {
                        return Err(ToolResult::native_outcome_unknown(
                            format!("AXScrollToVisible returned {status}; inspect fresh state and do not replay"),
                            cua_driver_core::action_record::ActionTransport::MacosAxAction,
                            if delivery_mode.is_foreground() { cua_driver_core::action_record::RequestedDelivery::Foreground } else { cua_driver_core::action_record::RequestedDelivery::Background },
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(40));
                } else {
                    semantic_gate?;
                }
                WheelTarget::from_retained(element, wid)
                    .map(Some)
                    .map_err(|error| ToolResult::error(error.to_string()))
            });
            match target_task.await {
                Ok(Ok(target)) => target,
                Ok(Err(refusal)) => return refusal,
                Err(_) => None,
            }
        } else if let (Some(mut cx), Some(mut cy)) = (x_arg, y_arg) {
            // Targeted x,y are window-local screenshot pixels and REQUIRE a
            // window_id to anchor the window→screen conversion (schema contract).
            // Without one, refuse rather than scrolling at screen-absolute coords.
            if window_id.is_none() {
                return ToolResult::error(
                    "window_id is required when scrolling by window-local x,y pixels.".to_string(),
                );
            }
            // Pixel path: x,y are window-local screenshot pixels. Mirror the
            // click pixel path — undo any session downscale, then translate
            // through the shared window frame (which refuses a window with no
            // live frame rather than scrolling at screen-absolute coords).
            if let Some(ratio) = self.state.resize_registry.ratio(pid, window_id) {
                cx *= ratio;
                cy *= ratio;
            }
            let Some(wid) = window_id else {
                // Unreachable: the None case refused above. Kept explicit so a
                // future edit cannot reintroduce the screen-absolute fallback.
                return ToolResult::error(
                    "window_id is required when scrolling by window-local x,y pixels.".to_string(),
                );
            };
            match super::px_frame::resolve_or_refuse(wid).await {
                Ok(frame) => {
                    let (sx, sy, lx, ly) = frame.to_screen(cx, cy);
                    Some(WheelTarget {
                        screen_x: sx,
                        screen_y: sy,
                        win_local: Some((lx, ly)),
                        wid: Some(wid),
                    })
                }
                Err(refusal) => return refusal,
            }
        } else {
            None
        };

        if let Some(target) = wheel_target {
            if !delivery_mode.is_foreground() {
                if let Some(wid) = target.wid {
                    if let (Some((lx, ly)), Some(bounds)) =
                        (target.win_local, crate::windows::window_bounds_by_id(wid))
                    {
                        if lx < 0.0 || ly < 0.0 || lx > bounds.width || ly > bounds.height {
                            return ToolResult::error(format!(
                                "scroll: window-local point ({lx:.1}, {ly:.1}) pt lies outside \
                                 window {wid}'s {:.0}×{:.0} pt frame; background delivery \
                                 refused",
                                bounds.width, bounds.height
                            ));
                        }
                    }
                    // The semantic reveal gate does not authorize pointer
                    // delivery. Revalidate the stricter route immediately
                    // before proceeding to wheel dispatch.
                    if let Some(lease) = _mutation_lease.as_ref() {
                        if let Err(refusal_result) = lease
                            .gate_again(
                                wid,
                                pre_focus_ptr,
                                cua_driver_core::background_input::BackgroundAction::WindowPointer,
                            )
                            .await
                        {
                            return refusal_result;
                        }
                    } else {
                        match super::gate_background_window_action(
                            pid,
                            wid,
                            pre_focus_ptr,
                            cua_driver_core::background_input::BackgroundAction::WindowPointer,
                        )
                        .await
                        {
                            Ok(lease) => _mutation_lease = Some(lease),
                            Err(refusal_result) => return refusal_result,
                        }
                    }
                }
            }
            let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
            // Pin + glide the agent-cursor overlay to the target for visibility
            // (overlay only — does NOT move the hardware cursor). Mirrors click.
            if let Some(wid) = target.wid {
                crate::cursor::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
            }
            crate::cursor::overlay::animate_cursor_to(
                cursor_key.clone(),
                target.screen_x,
                target.screen_y,
            )
            .await;
            self.state.cursor_registry.update_position(
                &cursor_key,
                target.screen_x,
                target.screen_y,
            );

            let prior_front = apps::frontmost_pid();
            let snapshot = WindowChangeDetector::snapshot(prior_front);

            let WheelTarget {
                screen_x,
                screen_y,
                win_local,
                wid,
            } = target;
            let amount_ticks = amount;
            let fg = delivery_mode.is_foreground() && wid.is_some();
            let result = focus_guard::with_focus_suppressed(
                Some(pid),
                prior_front,
                "scroll.CGScrollWheel",
                || async move {
                    spawn_native(move || -> anyhow::Result<()> {
                        let do_it = move || -> anyhow::Result<()> {
                            let (screen_x, screen_y, win_local) =
                                if let Some(element) = pre_focus_guard.as_ref() {
                                    let target = WheelTarget::from_retained(element, wid)?;
                                    (target.screen_x, target.screen_y, target.win_local)
                                } else {
                                    (screen_x, screen_y, win_local)
                                };
                            crate::input::mouse::scroll_wheel_at_xy(
                                pid,
                                screen_x,
                                screen_y,
                                win_local,
                                wid,
                                delta_y,
                                delta_x,
                                amount_ticks,
                            )
                        };
                        // Foreground rung: brief front → wheel → restore prior frontmost.
                        match (fg, wid) {
                            (true, Some(w)) => {
                                crate::input::skylight::with_foreground_assist(
                                    pid as libc::pid_t,
                                    w,
                                    do_it,
                                )?;
                                Ok(())
                            }
                            _ => do_it(),
                        }
                    })
                    .await
                },
            )
            .await;

            let changes = super::finish_window_observation(snapshot, &args).await;
            let mode_label = if fg {
                " (delivery_mode:foreground)"
            } else {
                ""
            };
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent {direction} scroll by {by} × {amount} via pixel wheel at \
                     ({screen_x:.0}, {screen_y:.0}){mode_label} (background CGEvent; not \
                     driver-verified — confirm via screenshot).{}",
                    changes.result_suffix()
                ))
                .with_structured(serde_json::json!({
                    "path": if fg { "cgevent_fg" } else { "cgevent" }, "verified": false, "effect": "unverifiable"
                })),
                Ok(Err(e)) => ToolResult::from_native_error(e, if delivery_mode.is_foreground() { cua_driver_core::action_record::RequestedDelivery::Foreground } else { cua_driver_core::action_record::RequestedDelivery::Background }),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let key = match (by.as_str(), direction.as_str()) {
            ("page", "down") | (_, "down") if by == "page" => "pagedown",
            ("page", "up") | (_, "up") if by == "page" => "pageup",
            ("line", "down") | (_, "down") => "down",
            ("line", "up") | (_, "up") => "up",
            (_, "left") => "left",
            (_, "right") => "right",
            _ => "down",
        };
        let key = key.to_owned();

        if !delivery_mode.is_foreground() {
            if let Some(wid) = window_id {
                if let Some(lease) = _mutation_lease.as_ref() {
                    if let Err(refusal_result) = lease
                        .gate_again(
                            wid,
                            pre_focus_ptr,
                            cua_driver_core::background_input::BackgroundAction::GenericKey,
                        )
                        .await
                    {
                        return refusal_result;
                    }
                } else {
                    match super::gate_background_window_action(
                        pid,
                        wid,
                        pre_focus_ptr,
                        cua_driver_core::background_input::BackgroundAction::GenericKey,
                    )
                    .await
                    {
                        Ok(lease) => _mutation_lease = Some(lease),
                        Err(refusal_result) => return refusal_result,
                    }
                }
            }
        }

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Scroll keystrokes (PageDown / arrow) into search-box autocomplete
        // can spawn floating helper windows; rare but real. Wrap for parity
        // with the other action tools.
        //
        // The AX focus_element() pre-write also runs inside the closure so
        // any reflex activations it triggers are caught by both the wildcard
        // snapshot suppressor and the targeted FocusGuard lease.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot(prior_front);

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "scroll.CGEvent",
            || async move {
                // Pre-focus the element under suppression so its
                // side-effects are captured by the snapshot + lease.
                if let Some(guard) = pre_focus_guard {
                    let _ = spawn_native(move || {
                        crate::input::ax_actions::focus_element(guard.as_ptr())
                    })
                    .await;
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }

                spawn_native(move || {
                    for _ in 0..amount {
                        crate::input::keyboard::press_key(pid, &key, &[])?;
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await
            },
        )
        .await;

        let changes = super::finish_window_observation(snapshot, &args).await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Sent {direction} scroll by {by} × {amount} via keystroke \
                 (background; not driver-verified — confirm via screenshot).{}",
                changes.result_suffix()
            ))
            .with_structured(serde_json::json!({ "path": "key_events", "verified": false })),
            Ok(Err(e)) => ToolResult::from_native_error(
                e,
                if delivery_mode.is_foreground() {
                    cua_driver_core::action_record::RequestedDelivery::Foreground
                } else {
                    cua_driver_core::action_record::RequestedDelivery::Background
                },
            ),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

unsafe fn scroll_native_text_area(
    element: AXUIElementRef,
    direction: &str,
    by: &str,
    amount: usize,
) -> anyhow::Result<bool> {
    use crate::ax::bindings::*;
    use crate::ax::element_resolver::FreshAxElements;
    let error = |code| anyhow::anyhow!("native scroll metadata read failed: {code}");
    if copy_string_attr_checked(element, "AXRole")
        .map_err(error)?
        .as_deref()
        != Some("AXTextArea")
    {
        return Ok(false);
    }
    let Some(area) = copy_element_attr_checked(element, "AXParent").map_err(error)? else {
        return Ok(false);
    };
    let mut owned = FreshAxElements {
        elements: vec![area as usize],
    };
    if copy_string_attr_checked(area, "AXRole")
        .map_err(error)?
        .as_deref()
        != Some("AXScrollArea")
    {
        return Ok(false);
    }
    let Some(bar) = copy_element_attr_checked(area, "AXVerticalScrollBar").map_err(error)? else {
        return Ok(false);
    };
    owned.elements.push(bar as usize);
    let children = copy_element_array(bar, "AXChildren").map_err(error)?;
    owned
        .elements
        .extend(children.iter().map(|child| *child as usize));
    if children.len() > 64 {
        anyhow::bail!("native scrollbar traversal is incomplete");
    }
    let subrole = match (direction, by) {
        ("up", "page") => "AXDecrementPage",
        ("down", "page") => "AXIncrementPage",
        ("up", _) => "AXDecrementArrow",
        ("down", _) => "AXIncrementArrow",
        _ => return Ok(false),
    };
    let mut selected = None;
    for child in children {
        if copy_string_attr_checked(child, "AXSubrole")
            .map_err(error)?
            .as_deref()
            == Some(subrole)
        {
            if selected.replace(child).is_some() {
                anyhow::bail!("native scroll control is ambiguous");
            }
        }
    }
    let Some(target) = selected else {
        return Ok(false);
    };
    if copy_bool_attr_checked(target, "AXEnabled").map_err(error)? == Some(false) {
        anyhow::bail!("native scroll control is disabled");
    }
    if !copy_action_names_checked(target)
        .map_err(error)?
        .iter()
        .any(|action| action == "AXPress")
    {
        return Ok(false);
    }
    for _ in 0..amount.max(1) {
        cua_driver_core::tool::check_native_dispatch()?;
        let status = perform_action(target, "AXPress");
        if status != kAXErrorSuccess {
            return Err(cua_driver_core::protocol::ToolResult::native_action_error(format!("native scroll outcome is unknown ({status}); inspect fresh state and do not replay"), cua_driver_core::action_record::ActionTransport::MacosAxAction));
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::background_input::{
        decide_background_input, BackgroundAction, BackgroundInputDecision, BackgroundTargetFacts,
        ElementAncestry, ExactWindowTarget, WindowServerOwnership,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn exact_target_refusal_prevents_ax_reveal() {
        let action_ran = AtomicBool::new(false);
        let target = ExactWindowTarget {
            pid: 42,
            window_id: 7,
        };
        let facts = BackgroundTargetFacts {
            window_server: WindowServerOwnership::SamePid,
            ax_window_present: true,
            target_minimized: Some(false),
            app_hidden: Some(false),
            competing_keyboard_destinations: 0,
            element: ElementAncestry::OutsideTargetWindow,
        };
        let refusal = match decide_background_input(target, &facts, BackgroundAction::AxSemantic) {
            BackgroundInputDecision::Refuse(refusal) => Err(
                super::super::background_refusal_result(target.pid, target.window_id, &refusal),
            ),
            BackgroundInputDecision::Execute { .. } => panic!("exact-target facts must refuse"),
        };

        let result = after_exact_target_gate(refusal, || {
            action_ran.store(true, Ordering::SeqCst);
        });

        assert!(result.is_err());
        assert!(!action_ran.load(Ordering::SeqCst));
    }
}
