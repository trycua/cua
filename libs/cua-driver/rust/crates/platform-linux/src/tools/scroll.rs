use async_trait::async_trait;
use cua_driver_contract::ScrollInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::{parse_typed_projection, ArgsExt as _},
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    coordinate_frame_schema, desktop_frame_requested, desktop_input_space, desktop_to_window_local,
    explicit_keyboard_cursor_target, foreground_hyprland_refusal, hyprland_foreground,
    hyprland_input_result, input_error_result, invalid_pointer_arguments, is_webkitgtk_embedder,
    isolated_hyprland_background, isolated_hyprland_refusal, isolated_hyprland_result,
    isolated_hyprland_task_error, keyboard_window_center, named_session_cursor_key,
    prove_token_element, resolve_cursor_key, resolve_element_local_coords,
    reveal_pointer_action_for, screenshot_scale, spawn_isolated_hyprland, stale_observed_element,
    unavailable_chromium_background, unavailable_gtk_pointer_background,
    unavailable_wayland_focused_input_background, unavailable_webkit_background,
    window_local_to_screen, window_screen_center, ToolState,
};

/// Pixel scroll shares click and drag's refusal order: an unsupported native
/// background delivery must report `background_unavailable` before the
/// screenshot frame is consulted, because native Wayland may legitimately
/// withhold the screenshot (`surface_identity_unproven`) that the frame needs.
fn coordinate_scroll_scale(
    native_refusal: Option<ToolResult>,
    resolve_scale: impl FnOnce() -> Result<f64, ToolResult>,
) -> Result<f64, ToolResult> {
    if let Some(refusal) = native_refusal {
        return Err(refusal);
    }
    resolve_scale()
}

async fn foreground_hyprland_scroll(
    args: &Value,
    pid: u32,
    xid: u64,
    actions: Vec<crate::wayland::hyprland_input::Action>,
) -> ToolResult {
    if !crate::wayland::hyprland_input::enabled() {
        return foreground_hyprland_refusal("production Hyprland input plugin is unavailable");
    }
    let owner = named_session_cursor_key(args);
    let (_cancellation, dispatch) = match spawn_isolated_hyprland(args, move |cancellation| {
        crate::wayland::hyprland_input::execute_foreground_scroll(
            owner,
            pid,
            xid,
            actions,
            cancellation,
        )
    }) {
        Ok(dispatch) => dispatch,
        Err(_) => return foreground_hyprland_refusal("authenticated admitted lifecycle required"),
    };
    hyprland_input_result(
        match dispatch.await {
            Ok(result) => result,
            Err(error) => Err(crate::wayland::hyprland_input::unknown_dispatch(
                error.into(),
                0,
            )),
        },
        true,
    )
}

#[cfg(test)]
#[test]
fn coordinate_scroll_keeps_wayland_background_refusal_ahead_of_screenshot_context() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let resolved = AtomicBool::new(false);
    let native_refusal = crate::input::delivery::background_unavailable_error(
        crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
    );
    let result = coordinate_scroll_scale(Some(native_refusal), || {
        resolved.store(true, Ordering::SeqCst);
        Err(ToolResult::error(
            "screenshot context must not be consulted",
        ))
    })
    .expect_err("unsupported Wayland background delivery must refuse pixel scroll");

    assert_eq!(
        result.structured_content.as_ref().unwrap()["code"],
        "background_unavailable"
    );
    assert!(
        !resolved.load(Ordering::SeqCst),
        "scroll snapshot context must not be consulted before the native refusal"
    );
    assert_eq!(coordinate_scroll_scale(None, || Ok(1.5)).unwrap(), 1.5);

    let source = include_str!("scroll.rs");
    let invoke = source
        .rsplit_once("impl Tool for ScrollTool {")
        .unwrap()
        .1
        .split_once("async fn invoke")
        .unwrap()
        .1;
    let (before_scale, _) = invoke
        .split_once("coordinate_scroll_scale(native_refusal")
        .expect("pixel scroll must resolve its frame through the refusal-first helper");
    assert!(
        before_scale.contains("unavailable_wayland_focused_input_background(delivery, true)"),
        "pixel scroll must compute the native Wayland refusal before its frame"
    );
    assert!(
        !before_scale.contains("screenshot_scale("),
        "pixel scroll must not consult the screenshot frame before the native refusal"
    );
}

// ── scroll ────────────────────────────────────────────────────────────────────

pub struct ScrollTool {
    pub(crate) state: Arc<ToolState>,
}

fn atspi_scroll_result(progress: crate::atspi::ScrollProgress, foreground: bool) -> ToolResult {
    use cua_driver_core::action_record::{
        ActionEffect, ActionExecutionRecord, ActionTransport, ActualDelivery, RequestedDelivery,
    };
    let effect = if !progress.complete && progress.acknowledged > 0 {
        ActionEffect::Partial
    } else {
        ActionEffect::Unverifiable
    };
    let mut record = ActionExecutionRecord::builder(
        effect,
        ActionTransport::LinuxAtSpiAction,
        if foreground {
            RequestedDelivery::Foreground
        } else {
            RequestedDelivery::Background
        },
    )
    .actual_delivery(if !progress.complete {
        ActualDelivery::Unknown
    } else if foreground {
        ActualDelivery::Foreground
    } else {
        ActualDelivery::Background
    });
    if progress.acknowledged > 0 {
        record = record.delivered_count(progress.acknowledged);
    }
    let text = if progress.complete {
        format!(
            "AT-SPI acknowledged {} scroll action(s); effect remains unverified.",
            progress.acknowledged
        )
    } else {
        format!("AT-SPI scroll stopped after {} acknowledged action(s): {}. Refresh state before another action.", progress.acknowledged, progress.detail.as_deref().unwrap_or("unknown outcome"))
    };
    let record = record
        .detail(text.clone())
        .build()
        .expect("AT-SPI scroll preserves acknowledged progress");
    let public = serde_json::to_value(record.public_result().unwrap()).unwrap();
    let result = if progress.complete {
        ToolResult::text(text)
    } else {
        ToolResult::error(text)
    };
    result.with_structured(public).with_action_record(record)
}

#[test]
fn atspi_scroll_outcomes_preserve_uncertainty_and_acknowledged_count() {
    use cua_driver_core::action_record::ActionEffect;
    for (complete, acknowledged, effect) in [
        (false, 0, ActionEffect::Unverifiable),
        (false, 1, ActionEffect::Partial),
        (true, 2, ActionEffect::Unverifiable),
    ] {
        let result = atspi_scroll_result(
            crate::atspi::ScrollProgress {
                acknowledged,
                complete,
                detail: None,
            },
            false,
        );
        let record = result.action_record.unwrap();
        assert_eq!(record.effect, effect);
        let public = serde_json::to_value(record.public_result().unwrap()).unwrap();
        assert_eq!(
            public["delivery"]["mode"],
            if complete { "background" } else { "unknown" }
        );
        assert_eq!(
            public["delivery"]["delivered_count"],
            if acknowledged == 0 {
                Value::Null
            } else {
                json!(acknowledged)
            }
        );
    }
}
static SCROLL_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ScrollTool {
    fn def(&self) -> &ToolDef {
        SCROLL_DEF.get_or_init(|| ToolDef {
            name: "scroll".into(),
            description: "Scroll the target pid's focused region via XSendEvent Button4/5. \
                direction required; by defaults to line, amount defaults to 3.".into(),
            input_schema: json!({
                // `pid` is conditionally required (validated in code), so only
                // `direction` is pinned — matches the scroll→["direction"] canon
                // in cua_driver_core::tool_schema.
                "type":"object","required":["direction"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                    "pid":{"type":"integer","description":"Target process ID. Required unless scope is \"desktop\" or element_token is supplied (the token carries the pid)."},
                    "direction":{"type":"string","enum":["up","down","left","right"],"description":"Scroll direction."},
                    "by":{"type":"string","enum":["line","page"],"description":"Scroll granularity. Default: line."},
                    "amount":{"type":"integer","minimum":1,"maximum":50,"description":"Number of scroll steps. Default: 3."},
                    "window_id":{"type":"integer","description":"Window id from list_windows. Required with x/y; optional with element_token (the token carries it)."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "x":{"type":"number","description":"Window-local screenshot-pixel X of the scroll target. Pass with y and without element_token."},
                    "y":{"type":"number","description":"Window-local screenshot-pixel Y of the scroll target. Pass with x and without element_token."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"Use \"desktop\" with x,y and no pid/window_id for get_desktop_state screen coordinates. Default \"window\"."},
                    "coordinate_frame": coordinate_frame_schema(),
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<ScrollInput>("scroll", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let direction = input.direction.as_str().to_owned();
            let (x, y) = (input.x, input.y);
            let x = x.round() as i32;
            let y = y.round() as i32;
            let amount = input.amount.unwrap_or(3).clamp(1, 50) as usize;
            let display = direction.clone();
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_desktop" } else { "xtest" };
            let space = match desktop_input_space().await {
                Ok(space) => space,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            if named_session_cursor_key(&args).is_some() {
                let (overlay_x, overlay_y) = space.to_layout(x, y);
                reveal_pointer_action_for(
                    &self.state,
                    &cursor_id,
                    f64::from(overlay_x),
                    f64::from(overlay_y),
                    false,
                )
                .await;
            }
            let result = tokio::task::spawn_blocking(move || {
                if wayland {
                    crate::wayland::scroll_desktop(&space, x, y, &direction, amount as u32)
                } else {
                    crate::input::send_scroll_xtest_desktop(x, y, &direction, amount)
                }
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Scrolled desktop {display} × {amount}."))
                    .with_structured(
                        json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                    ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let mut pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let direction = match args.require_str("direction") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let amount = args.u64_or("amount", 3).clamp(1, 50) as usize;
        let by = match serde_json::from_value::<cua_driver_contract::ScrollBy>(
            args.get("by").cloned().unwrap_or(json!("line")),
        ) {
            Ok(by) => by,
            Err(error) => return ToolResult::error(format!("Invalid scroll by: {error}")),
        };
        // The Linux scroll implementation doesn't pre-focus the element (X11
        // scroll buttons go to the window root), but the token still needs to
        // be validated so a stale token surfaces an error instead of silently
        // no-op'ing.
        let resolved = match self.state.snapshots.resolve(pid as i32, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let token = match prove_token_element(pid, &resolved, "scroll").await {
            Ok(token) => token,
            Err(refusal) => return refusal,
        };
        let xid_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => {
                Some(*window_id)
            }
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };

        // Resolve XID: use window_id if given, else first window for pid.
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows = tokio::task::spawn_blocking(move || {
                    crate::x11::list_windows(if pid == 0 { None } else { Some(pid) })
                })
                .await
                .unwrap_or_default();
                // pid omitted: the keys go to the active window, like a
                // physical keyboard would, and the action adopts its pid.
                let chosen = if pid == 0 {
                    let active = crate::x11::active_window();
                    windows
                        .iter()
                        .find(|w| Some(w.xid) == active && w.pid.is_some())
                        .or_else(|| windows.iter().find(|w| w.is_on_screen && w.pid.is_some()))
                } else {
                    // The pid's active window, else its largest on-screen
                    // toplevel: LibreOffice and GIMP own hidden/utility
                    // toplevels that a plain `first()` could pick, and an
                    // unmapped window cannot take the virtual keyboard focus.
                    let active = crate::x11::active_window();
                    windows
                        .iter()
                        .find(|w| Some(w.xid) == active)
                        .or_else(|| {
                            windows
                                .iter()
                                .filter(|w| w.is_on_screen)
                                .max_by_key(|w| u64::from(w.width) * u64::from(w.height))
                        })
                        .or_else(|| windows.first())
                };
                match chosen {
                    Some(w) => {
                        if pid == 0 {
                            pid = w.pid.unwrap_or(0);
                        }
                        w.xid
                    }
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let isolated_background = isolated_hyprland_background(delivery);
        let pixel_target = match (
            args.get("x").and_then(|value| value.as_f64()),
            args.get("y").and_then(|value| value.as_f64()),
        ) {
            (Some(x), Some(y)) if desktop_frame_requested(&args) => {
                // Desktop-frame pixels (from get_desktop_state) against a named
                // window: map into the window-local frame the pipeline expects.
                match tokio::task::spawn_blocking(move || desktop_to_window_local(xid, x, y)).await
                {
                    Ok(Ok(local)) => Some(local),
                    Ok(Err(e)) => {
                        return ToolResult::error(format!(
                            "desktop-frame coordinates could not be mapped into window {xid}: {e}"
                        ))
                    }
                    Err(e) => return ToolResult::error(format!("Task error: {e}")),
                }
            }
            (Some(x), Some(y)) => {
                // Pixel targets use the latest screenshot's coordinate frame.
                // Apply the same buffer-to-window ratio as click/drag before
                // positioning either the agent cursor or the input device.
                // An unsupported native background delivery refuses first.
                let native_refusal = (!isolated_background)
                    .then(|| unavailable_wayland_focused_input_background(delivery, true))
                    .flatten();
                let ratio = match coordinate_scroll_scale(native_refusal, || {
                    screenshot_scale(&self.state, &args, pid, Some(xid))
                }) {
                    Ok(ratio) => ratio,
                    Err(refusal) => return refusal,
                };
                Some((x * ratio, y * ratio))
            }
            (None, None) => None,
            (Some(_), None) => return invalid_pointer_arguments("scroll", "x was given without y"),
            (None, Some(_)) => return invalid_pointer_arguments("scroll", "y was given without x"),
        };
        let resolved_element_index = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        if pixel_target.is_some() && resolved_element_index.is_some() {
            return ToolResult::error(
                "Pass either element_token (ax) or x,y (px) to scroll, not both.",
            );
        }

        if named_session_cursor_key(&args).is_some() {
            let element = token.clone();
            let visual_target = tokio::task::spawn_blocking(move || {
                explicit_keyboard_cursor_target(pid, xid, element.as_ref(), pixel_target)
                    .or_else(|| keyboard_window_center(xid))
            })
            .await
            .ok()
            .flatten();
            if let Some((sx, sy)) = visual_target {
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(xid),
                );
                reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, false).await;
            }
        }

        if !isolated_background {
            if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
                return refusal;
            }
        }
        if let Some(element) = token.clone() {
            let idx = element.index;
            // WebKitGTK acknowledges the AT-SPI scroll action without moving
            // the DOM scroller. On native Wayland, use a real compositor wheel
            // event at the resolved element instead of reporting a silent
            // success. Other AX targets keep their semantic route before any
            // coordinate-based compositor fallback.
            if !(crate::wayland::wayland_input_enabled() && is_webkitgtk_embedder(pid)) {
                let direction_for_ax = direction.clone();
                let ax_result = tokio::task::spawn_blocking(move || {
                    crate::atspi::scroll_token(pid, &element, &direction_for_ax, amount, by)
                })
                .await;
                if let Ok(Err(error)) = &ax_result {
                    // The observed element is gone: nothing was scrolled.
                    if let Some(stale) = stale_observed_element("scroll", idx, error) {
                        return stale;
                    }
                }
                match ax_result {
                    Ok(Ok(progress)) => {
                        return atspi_scroll_result(progress, delivery.is_foreground())
                    }
                    Err(error) => {
                        return atspi_scroll_result(
                            crate::atspi::ScrollProgress {
                                acknowledged: 0,
                                complete: false,
                                detail: Some(error.to_string()),
                            },
                            delivery.is_foreground(),
                        )
                    }
                    Ok(Err(_)) => {}
                }
            }
        }
        if hyprland_foreground(delivery) {
            if xid_opt.is_none() {
                return foreground_hyprland_refusal(
                    "an exact window_id or window-bound element token is required",
                );
            }
            let point = if let Some(element) = token.clone() {
                match tokio::task::spawn_blocking(move || {
                    resolve_element_local_coords(pid, &element, Some(xid))
                })
                .await
                {
                    Ok(Ok((_, x, y))) => Some((x, y)),
                    _ => {
                        return foreground_hyprland_refusal(
                            "could not resolve exact element coordinates",
                        )
                    }
                }
            } else {
                pixel_target
            };
            // `by: "page"` is the documented 3-detent approximation, not an
            // exact viewport; large requests become several wheel packets.
            let actions =
                match crate::wayland::hyprland_input::scroll_actions(point, &direction, amount, by)
                {
                    Ok(actions) => actions,
                    Err(error) => return foreground_hyprland_refusal(error.to_string()),
                };
            return foreground_hyprland_scroll(&args, pid, xid, actions).await;
        }
        if isolated_background {
            if xid_opt.is_none() {
                return isolated_hyprland_refusal(
                    "an exact window_id or window-bound element token is required",
                );
            }
            let owner = named_session_cursor_key(&args);
            let element = token.clone();
            let (_cancellation, dispatch) =
                match spawn_isolated_hyprland(&args, move |cancellation| {
                    let point = if let Some(element) = &element {
                        let (_, x, y) = resolve_element_local_coords(pid, element, Some(xid))?;
                        Some((x, y))
                    } else {
                        pixel_target
                    };
                    let actions = crate::wayland::hyprland_input::scroll_actions(
                        point, &direction, amount, by,
                    )?;
                    crate::wayland::hyprland_input::execute_scroll(
                        owner,
                        pid,
                        xid,
                        actions,
                        cancellation,
                    )
                }) {
                    Ok(dispatch) => dispatch,
                    Err(refusal) => return refusal,
                };
            return match dispatch.await {
                Ok(result) => isolated_hyprland_result(result),
                Err(error) => isolated_hyprland_task_error(error, false),
            };
        }

        if crate::wayland::is_inject_mode() {
            let Some((x, y)) = pixel_target else {
                return crate::input::delivery::background_unavailable_error(
                    crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                );
            };
            let direction_for_inject = direction.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::inject_scroll(pid, xid, x, y, &direction_for_inject, amount as u32)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Scrolled {direction} {amount} ticks (focus-free via cua-compositor)."
                ))
                .with_structured(json!({
                    "verified": false,
                    "delivery_mode": if delivery.is_foreground() { "foreground" } else { "background" },
                    "route": "cua_compositor_inject"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }

        if crate::wayland::wayland_input_enabled() {
            if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, false) {
                return refusal;
            }
            let direction_for_wayland = direction.clone();
            let local_point = match (pixel_target, token.clone()) {
                (Some(point), _) => Some(point),
                (None, Some(element)) => {
                    let idx = element.index;
                    match tokio::task::spawn_blocking(move || {
                        resolve_element_local_coords(pid, &element, Some(xid))
                    })
                    .await
                    {
                        Ok(Ok((_resolved_xid, x, y))) => Some((x, y)),
                        Ok(Err(error)) => {
                            return ToolResult::error(format!(
                                "Could not resolve scroll element [{idx}] coordinates: {error}"
                            ))
                        }
                        Err(error) => {
                            return ToolResult::error(format!(
                                "Scroll element coordinate task failed: {error}"
                            ))
                        }
                    }
                }
                (None, None) => None,
            };
            let output_point = local_point.map(|(x, y)| {
                crate::wayland::window_local_to_output(xid, x.round() as i32, y.round() as i32)
            });
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::scroll_at(xid, output_point, &direction_for_wayland, amount as u32)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Scrolled {direction} {amount} ticks (delivery_mode=foreground)."
                ))
                .with_structured(json!({ "verified": false, "delivery_mode": "foreground" })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }

        if let Some(refusal) = unavailable_webkit_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_pointer_background(pid, delivery) {
            return refusal;
        }

        // An element-addressed scroll must land over the element. The old
        // fallback used (0, 0) in the window, which can report success while
        // Chromium/GTK routes the wheel to the toplevel instead of the
        // requested scroll region.
        let element_point = match token {
            Some(element) => {
                let idx = element.index;
                match tokio::task::spawn_blocking(move || {
                    resolve_element_local_coords(pid, &element, Some(xid))
                })
                .await
                {
                    Ok(Ok((_resolved_xid, local_x, local_y))) => {
                        let screen = window_local_to_screen(xid, local_x, local_y).ok();
                        Some(((local_x, local_y), screen))
                    }
                    Ok(Err(e)) => {
                        return ToolResult::error(format!(
                            "Could not resolve scroll element [{idx}] coordinates: {e}"
                        ))
                    }
                    Err(e) => {
                        return ToolResult::error(format!(
                            "Scroll element coordinate task failed: {e}"
                        ))
                    }
                }
            }
            None => pixel_target.map(|local| {
                let screen = window_local_to_screen(xid, local.0, local.1).ok();
                (local, screen)
            }),
        };

        // X11 scroll buttons: 4=up, 5=down, 6=left, 7=right
        // Note: "page" scroll is still per-click on X11; send more ticks for page.
        let button: u8 = match direction.as_str() {
            "up" => 4,
            "left" => 6,
            "right" => 7,
            _ => 5,
        };
        let cursor_id_for_task = cursor_id.clone();
        let direction_for_wayland = direction.clone();
        let amount_u32 = amount as u32;
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::wayland_input_enabled() {
                return crate::wayland::scroll(xid, &direction_for_wayland, amount_u32);
            }
            // foreground: activate the window, then scroll, then restore — for
            // surfaces that only route wheel events to the active window.
            let x11_scroll = || -> anyhow::Result<()> {
                // X11: synthetic Button4-7 XSendEvents are dropped by XInput2
                // toolkits (GTK never scrolls). On a real Xorg host, drive a real
                // wheel detent through the MPX uinput pointer over the window's
                // center — libinput turns it into the XI2 smooth-scroll GTK reads —
                // without stealing focus. Falls back to the legacy XSendEvent
                // Button4-7 path on Xvfb / unsupported servers.
                // Button → axis/sign: 4=up(+v) 5=down(-v) 6=left(-h) 7=right(+h).
                if crate::input::real_pointer_input_available() {
                    let pointer_point = element_point
                        .and_then(|(_, screen)| screen)
                        .map(|(x, y)| (x as i32, y as i32))
                        .or_else(|| window_screen_center(xid).ok());
                    if let Some((cx, cy)) = pointer_point {
                        let horizontal = matches!(button, 6 | 7);
                        let ticks = match button {
                            4 | 7 => amount as i32,
                            _ => -(amount as i32), // 5 (down) and 6 (left)
                        };
                        match crate::input::send_virtual_pointer_scroll(
                            &cursor_id_for_task,
                            &crate::input::VirtualPointerScroll {
                                target_window: xid,
                                x: cx,
                                y: cy,
                                horizontal,
                                ticks,
                            },
                        ) {
                            Ok(()) => return Ok(()),
                            Err(error) if crate::input::is_uinput_unavailable(&error) => {
                                return Err(error)
                            }
                            Err(e) => tracing::warn!("MPX scroll fell back to XSendEvent: {e}"),
                        }
                    }
                }
                let (local_x, local_y) =
                    element_point.map(|(local, _)| local).unwrap_or((0.0, 0.0));
                crate::input::send_click(xid, local_x as i32, local_y as i32, amount, button)
            };
            if delivery.is_foreground() {
                let point = element_point
                    .and_then(|(_, screen)| screen)
                    .or_else(|| {
                        window_screen_center(xid)
                            .ok()
                            .map(|(x, y)| (x as f64, y as f64))
                    })
                    .ok_or_else(|| anyhow::anyhow!("could not resolve foreground scroll point"))?;
                crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_click_xtest_desktop(
                        point.0 as i32,
                        point.1 as i32,
                        button,
                        amount,
                    )
                })
            } else {
                x11_scroll()
            }
        })
        .await;
        let mode_label = if delivery.is_foreground() {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Scrolled {direction} {amount} ticks (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
            Ok(Err(e)) => input_error_result(e),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}
