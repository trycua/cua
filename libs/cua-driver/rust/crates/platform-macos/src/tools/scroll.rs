use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_contract::{ScrollBy, ScrollDirection, ScrollInput};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_children, copy_element_attr, copy_string_attr, element_screen_center, kAXErrorSuccess,
    perform_action, AXUIElementRef,
};
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
            `element_index`/`element_token` (preferred) or window-local `x, y` pixels: \
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
            let result = tokio::task::spawn_blocking(move || {
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
                Ok(Err(error)) => ToolResult::error(format!("desktop scroll failed: {error}")),
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
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg,
            "scroll",
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

        // An element target was explicitly requested (element_index/element_token)
        // but couldn't be retained from the cache — the snapshot is stale. Fail
        // loudly instead of silently falling through to the keystroke/page
        // scroller below, which would scroll the wrong thing.
        if let Some(idx) = element_index {
            if pre_focus_guard.is_none() {
                return ToolResult::error(format!(
                    "Element index {idx} not found. Call get_window_state first."
                ));
            }
        }
        let mut _mutation_lease: Option<super::BackgroundMutationLease> = None;

        // AppKit exposes vertical scroll-bar buttons beneath the text area's
        // AXScrollArea parent. Pressing those controls is a true
        // background-safe scroll: no activation, z-order change, or cursor move.
        if matches!(direction.as_str(), "up" | "down") {
            if let (Some(index), Some(wid)) = (element_index, window_id) {
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
                let native_element_guard = self
                    .state
                    .element_cache
                    .get_element_retained(pid, wid, index);
                let direction_for_ax = direction.clone();
                let by_for_ax = by.clone();
                let foreground = delivery_mode.is_foreground();
                let ax_result =
                    tokio::task::spawn_blocking(move || -> anyhow::Result<(bool, bool)> {
                        let Some(element_guard) = native_element_guard else {
                            return Ok((false, false));
                        };
                        if foreground {
                            let mut delivered = false;
                            let fronted = crate::input::skylight::with_foreground_assist(
                                pid as libc::pid_t,
                                wid,
                                || {
                                    delivered = unsafe {
                                        scroll_native_text_area(
                                            element_guard.as_ptr() as AXUIElementRef,
                                            &direction_for_ax,
                                            &by_for_ax,
                                            amount,
                                        )
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
                                        element_guard.as_ptr() as AXUIElementRef,
                                        &direction_for_ax,
                                        &by_for_ax,
                                        amount,
                                    )
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
                        return ToolResult::error(format!("Native AX scroll failed: {error}"));
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
            // cached element still belongs to the exact requested window before
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
            let target_task = tokio::task::spawn_blocking(move || {
                // Web content can be present in AX while its frame is below
                // the outer page viewport. Ask the accessibility hierarchy to
                // reveal the target before taking the screen-space center;
                // otherwise the wheel is posted outside the rendered window
                // and nested overflow regions never receive it.
                after_exact_target_gate(semantic_gate, || unsafe {
                    crate::ax::bindings::perform_action(
                        element_ptr as AXUIElementRef,
                        "AXScrollToVisible",
                    )
                })?;
                std::thread::sleep(std::time::Duration::from_millis(40));
                let center = unsafe { element_screen_center(element_ptr as AXUIElementRef) };
                Ok(center.map(|(cx, cy)| {
                    let win_local = wid
                        .and_then(crate::windows::window_bounds_by_id)
                        .map(|b| (cx - b.x, cy - b.y));
                    WheelTarget {
                        screen_x: cx,
                        screen_y: cy,
                        win_local,
                        wid,
                    }
                }))
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
                    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                        let do_it = move || -> anyhow::Result<()> {
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
                Ok(Err(e)) => ToolResult::error(format!("Wheel scroll failed: {e}")),
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
                if let Some(element_ptr) = pre_focus_ptr {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::input::ax_actions::focus_element(element_ptr)
                    })
                    .await;
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }

                tokio::task::spawn_blocking(move || {
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
            Ok(Err(e)) => ToolResult::error(format!("Scroll failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

unsafe fn scroll_native_text_area(
    element: AXUIElementRef,
    direction: &str,
    by: &str,
    amount: usize,
) -> bool {
    if copy_string_attr(element, "AXRole").as_deref() != Some("AXTextArea") {
        return false;
    }
    let Some(scroll_area) = copy_element_attr(element, "AXParent") else {
        return false;
    };
    if copy_string_attr(scroll_area, "AXRole").as_deref() != Some("AXScrollArea") {
        CFRelease(scroll_area as CFTypeRef);
        return false;
    }
    let mut buttons = Vec::new();
    collect_ax_buttons(scroll_area, 0, &mut buttons);
    CFRelease(scroll_area as CFTypeRef);
    if buttons.is_empty() {
        return false;
    }

    let reverse = direction == "up";
    let base = if by == "page" && buttons.len() >= 4 {
        2
    } else {
        0
    };
    let index = base + usize::from(reverse);
    let mut delivered = false;
    if let Some(target) = buttons.get(index).copied() {
        for _ in 0..amount.max(1) {
            if perform_action(target, "AXPress") != kAXErrorSuccess {
                break;
            }
            delivered = true;
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    for button in buttons {
        CFRelease(button as CFTypeRef);
    }
    delivered
}

unsafe fn collect_ax_buttons(
    element: AXUIElementRef,
    depth: usize,
    buttons: &mut Vec<AXUIElementRef>,
) {
    if depth >= 4 || buttons.len() >= 4 {
        return;
    }
    for child in copy_children(element) {
        if buttons.len() >= 4 {
            CFRelease(child as CFTypeRef);
        } else if copy_string_attr(child, "AXRole").as_deref() == Some("AXButton") {
            buttons.push(child);
        } else {
            collect_ax_buttons(child, depth + 1, buttons);
            CFRelease(child as CFTypeRef);
        }
    }
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
