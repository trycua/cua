use async_trait::async_trait;
use cua_driver_core::tool::spawn_native;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{copy_string_attr, kAXErrorSuccess, perform_action, AXUIElementRef};

use super::ToolState;

pub struct RightClickTool {
    state: Arc<ToolState>,
}

impl RightClickTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "right_click".into(),
        description:
            "Right-click against a target pid. Two addressing modes:\n\n\
             - `element_token` + `window_id` (from the last `get_window_state` snapshot) — \
               performs `AXShowMenu` on the freshly resolved element. Pure AX RPC, works on backgrounded / \
               hidden windows, no cursor move or focus steal. Requires a prior \
               `get_window_state(pid, window_id)` in this turn.\n\n\
             - `x`, `y` — synthesizes `rightMouseDown` / `rightMouseUp` CGEvent pair posted \
               to the pid. Driver converts image-pixel → screen-point internally. \
               `modifier` forces the CGEvent path (AX actions don't propagate modifier keys).\n\n\
             Exactly one of `element_token` or (`x` AND `y`) must be provided. `pid` always \
             required. `window_id` required when `element_token` is used."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid": { "type": "integer", "description": "Target process ID." },
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                "window_id": {
                    "type": "integer",
                    "description": "Window ID. Optional with element_token; if supplied, it must match the token window."
                },
                "x": {
                    "type": "number",
                    "description": "X in window-local screenshot pixels. Must be provided together with y."
                },
                "y": {
                    "type": "number",
                    "description": "Y in window-local screenshot pixels. Must be provided together with x."
                },
                "modifier": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Modifier keys held during the right-click: cmd/shift/option/ctrl. Pixel path only."
                },
                "delivery_mode": cua_driver_core::tool_schema::delivery_mode_schema()
            },
            "additionalProperties": false
        }),
        read_only:   false,
        destructive: true,
        idempotent:  false,
        open_world:  true,
    })
}

#[async_trait]
impl Tool for RightClickTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // delivery_mode: foreground briefly fronts the window before the pixel
        // right-click (the explicit last resort for surfaces that drop
        // background CGEvents), via the same skylight assist click uses. The AX
        // (AXShowMenu) path is background-by-design and untouched.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);

        // Surface 6: element_token / element_index precedence resolution.
        let window_id_arg = args.opt_u64("window_id");
        let resolved = match crate::ax::element_resolver::resolve_element_args(
            pid,
            &args,
            "right_click",
        )
        .await
        {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id, element_guard) = resolved.into_parts(window_id_arg);
        let window_id = match super::native_window_id(window_id) {
            Ok(window_id) => window_id,
            Err(error) => return error,
        };
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
        let has_xy = x.is_some() && y.is_some();
        let partial_xy = x.is_some() != y.is_some();
        let modifiers: Vec<String> = args.str_array("modifier");

        if partial_xy {
            return ToolResult::error("Provide both x and y together, not just one.");
        }
        if element_index.is_some() && has_xy {
            return ToolResult::error("Provide either element_index or (x, y), not both.");
        }
        if element_index.is_none() && !has_xy {
            return ToolResult::error(
                "Provide element_index or (x, y) to address the right-click target.",
            );
        }
        if element_index.is_some() && window_id.is_none() {
            return ToolResult::error("window_id is required when element_index is used.");
        }

        // ── AX element path ──────────────────────────────────────────────────
        if let (Some(idx), Some(wid), Some(element_guard)) =
            (element_index, window_id, element_guard)
        {
            let probe = element_guard.clone();
            let semantic = match spawn_native(move || -> anyhow::Result<bool> {
                let pointer = probe.checked_ptr()?;
                Ok(unsafe {
                    crate::ax::bindings::copy_action_names_checked(pointer as AXUIElementRef)
                }
                .map_err(|code| anyhow::anyhow!("native action read failed: {code}"))?
                .iter()
                .any(|action| action == "AXShowMenu"))
            })
            .await
            {
                Ok(Ok(semantic)) => semantic,
                Ok(Err(error)) => return ToolResult::error(error.to_string()),
                Err(error) => return ToolResult::error(error.to_string()),
            };
            let element_ptr = element_guard.as_ptr();

            if !modifiers.is_empty() {
                return ToolResult::error(
                    "modified semantic right-click is unsupported; no action was sent",
                );
            }
            let foreground = delivery_mode.is_foreground();
            let _mutation_lease = if foreground {
                None
            } else {
                Some(
                    match super::gate_background_window_action(
                        pid,
                        wid,
                        Some(element_ptr),
                        if semantic {
                            cua_driver_core::background_input::BackgroundAction::AxSemantic
                        } else {
                            cua_driver_core::background_input::BackgroundAction::WindowPointer
                        },
                    )
                    .await
                    {
                        Ok(lease) => lease,
                        Err(refusal_result) => return refusal_result,
                    },
                )
            };

            let result = spawn_native(move || {
                let dispatch = || {
                    ax_show_menu(
                        element_guard.checked_ptr()?,
                        idx,
                        pid,
                        wid,
                        semantic,
                        foreground,
                    )
                };
                if foreground {
                    let mut result = None;
                    crate::input::skylight::with_foreground_hid_activation(pid, wid, || {
                        result = Some(dispatch());
                        Ok(())
                    })?;
                    result.ok_or_else(|| anyhow::anyhow!("native action did not start"))?
                } else {
                    dispatch()
                }
            })
            .await;

            return match result {
                Ok(Ok(msg)) if semantic => crate::input::ax_actions::acknowledged(
                    msg,
                    "AXShowMenu",
                    if foreground {
                        cua_driver_core::action_record::RequestedDelivery::Foreground
                    } else {
                        cua_driver_core::action_record::RequestedDelivery::Background
                    },
                    foreground,
                ),
                Ok(Ok(msg)) => ToolResult::text(msg).with_structured(serde_json::json!({
                    "path": if foreground { "cgevent_fg" } else { "cgevent" },
                    "verified": false,
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::from_native_error(
                    error,
                    if foreground {
                        cua_driver_core::action_record::RequestedDelivery::Foreground
                    } else {
                        cua_driver_core::action_record::RequestedDelivery::Background
                    },
                ),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // ── Pixel path ───────────────────────────────────────────────────────
        let (mut cx, mut cy) = (x.unwrap(), y.unwrap());
        // Scale back from downscaled-image space to native pixels when needed.
        if let Some(ratio) = self.state.resize_registry.ratio(pid, window_id) {
            cx *= ratio;
            cy *= ratio;
        }

        // Window-local → screen coordinate translation + win-local logical coords
        // for CGEventSetWindowLocation (shared with click.rs via px_frame, which
        // refuses a window with no live frame).
        let (screen_x, screen_y, win_local_x, win_local_y) = if let Some(wid) = window_id {
            match super::px_frame::resolve_or_refuse(wid).await {
                Ok(frame) => {
                    let translated = frame.to_screen(cx, cy);
                    if !delivery_mode.is_foreground()
                        && (translated.2 < 0.0
                            || translated.3 < 0.0
                            || translated.2 > frame.bounds.width
                            || translated.3 > frame.bounds.height)
                    {
                        return ToolResult::error(format!(
                            "right_click: window-local point ({:.1}, {:.1}) pt lies outside \
                             window {wid}'s {:.0}×{:.0} pt frame; background delivery refused",
                            translated.2, translated.3, frame.bounds.width, frame.bounds.height
                        ));
                    }
                    translated
                }
                Err(refusal) => return refusal,
            }
        } else {
            (cx, cy, cx, cy)
        };

        let _mutation_lease = if !delivery_mode.is_foreground() {
            if let Some(wid) = window_id {
                match super::gate_background_window_action(
                    pid,
                    wid,
                    None,
                    cua_driver_core::background_input::BackgroundAction::WindowPointer,
                )
                .await
                {
                    Ok(lease) => Some(lease),
                    Err(refusal_result) => return refusal_result,
                }
            } else {
                None
            }
        } else {
            None
        };

        // Pin overlay above the target window before animating.
        if let Some(wid) = window_id {
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::PinAbove(wid as u64),
            );
        }
        // Animate cursor to the click point; wait for arrival before firing.
        crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), screen_x, screen_y).await;
        crate::cursor::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::ClickPulse {
                x: screen_x,
                y: screen_y,
            },
        );

        let mod_suffix = if modifiers.is_empty() {
            String::new()
        } else {
            format!(" with {}", modifiers.join("+"))
        };

        let fg = delivery_mode.is_foreground() && window_id.is_some();
        let result = spawn_native(move || -> anyhow::Result<()> {
            let do_it = move || -> anyhow::Result<()> {
                let m: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                if let Some(wid) = window_id {
                    crate::input::mouse::right_click_at_xy_with_window_local(
                        pid,
                        screen_x,
                        screen_y,
                        win_local_x,
                        win_local_y,
                        wid,
                        &m,
                    )
                } else {
                    crate::input::mouse::right_click_at_xy(pid, screen_x, screen_y, &m)
                }
            };
            // Foreground rung: brief front → right-click → restore prior frontmost.
            match (fg, window_id) {
                (true, Some(wid)) => {
                    crate::input::skylight::with_foreground_assist(pid as libc::pid_t, wid, do_it)?;
                    Ok(())
                }
                _ => do_it(),
            }
        })
        .await;
        let mode_label = if fg {
            " (delivery_mode:foreground)"
        } else {
            ""
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Right-clicked{mod_suffix} at ({screen_x:.1}, {screen_y:.1}){mode_label}."))
                .with_structured(serde_json::json!({
                    "path": if fg { "cgevent_fg" } else { "cgevent" }, "verified": false, "effect": "unverifiable"
                })),
            Ok(Err(error)) => ToolResult::from_native_error(
                error,
                if fg {
                    cua_driver_core::action_record::RequestedDelivery::Foreground
                } else {
                    cua_driver_core::action_record::RequestedDelivery::Background
                },
            ),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking AX path ─────────────────────────────────────────────────────────

fn ax_show_menu(
    element_ptr: usize,
    idx: usize,
    pid: i32,
    wid: u32,
    semantic: bool,
    foreground: bool,
) -> anyhow::Result<String> {
    let element = element_ptr as AXUIElementRef;

    let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
    let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

    if semantic {
        let err = unsafe { perform_action(element, "AXShowMenu") };
        if err == kAXErrorSuccess {
            return Ok(format!(
                "Shown menu for [{idx}] {role} \"{title}\" (AXShowMenu)."
            ));
        }
        return Err(ToolResult::native_action_error(
            format!("AXShowMenu returned {err}; inspect fresh state and do not replay"),
            cua_driver_core::action_record::ActionTransport::MacosAxAction,
        ));
    }

    // Pixel right-click at the element's screen-space center.
    let (cx, cy) =
        unsafe { crate::ax::bindings::element_screen_center(element) }.ok_or_else(|| {
            anyhow::anyhow!(
                "[{idx}] {role} \"{title}\" advertises no AXShowMenu and has no resolvable \
             on-screen center for a pixel right-click. Pass x, y directly."
            )
        })?;
    let (wx, wy) = crate::windows::window_bounds_by_id(wid)
        .map(|b| (cx - b.x, cy - b.y))
        .ok_or_else(|| anyhow::anyhow!("target window geometry is unavailable"))?;
    if foreground {
        crate::input::mouse::click_at_xy_desktop_with_modifiers_preserving_cursor(
            cx,
            cy,
            1,
            "right",
            &[],
        )?;
    } else {
        crate::input::mouse::right_click_at_xy_with_window_local(pid, cx, cy, wx, wy, wid, &[])?;
    }
    Ok(format!(
        "Right-clicked [{idx}] {role} \"{title}\" at element center ({cx:.0}, {cy:.0}) \
         (pixel right-click; element advertises no AXShowMenu)."
    ))
}
