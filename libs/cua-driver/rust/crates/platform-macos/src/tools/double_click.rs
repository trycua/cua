use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{
    copy_action_names, element_screen_center, kAXErrorSuccess, perform_action, AXUIElementRef,
};

use super::ToolState;

pub struct DoubleClickTool {
    state: Arc<ToolState>,
}

fn background_action_for_element(
    has_ax_open: bool,
) -> cua_driver_core::background_input::BackgroundAction {
    if has_ax_open {
        cua_driver_core::background_input::BackgroundAction::AxSemantic
    } else {
        cua_driver_core::background_input::BackgroundAction::WindowPointer
    }
}

impl DoubleClickTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "double_click".into(),
        description:
            "Double-click at (x, y) or on an AX element identified by element_index + window_id.\n\n\
             AX path (element_index provided): performs `AXOpen` when the element advertises it \
             (Finder items, openable list rows/cells); otherwise resolves the element's on-screen \
             center and falls back to a pixel double-click there.\n\n\
             Pixel path (x, y provided): two down/up pairs ~80 ms apart at the given coordinates."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid":           { "type": "integer" },
                "x":             { "type": "number",  "description": "Screen X coordinate (pixel path)." },
                "y":             { "type": "number",  "description": "Screen Y coordinate (pixel path)." },
                "window_id":     { "type": "integer", "description": "CGWindowID. Required when element_index is used. Optional when element_token is supplied (the token carries it)." },
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
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
impl Tool for DoubleClickTool {
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
        // double-click (the explicit last resort for surfaces that drop
        // background CGEvents), via the same skylight assist click uses.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
        // Surface 6: token / index precedence — see click.rs for the
        // canonical comment.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg,
            "double_click",
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

        // ── AX element path ──────────────────────────────────────────────────
        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            // Retain out of the cache so a concurrent get_window_state can't
            // free the element mid-action (use-after-free → daemon crash).
            let element_guard = match self.state.element_cache.get_element_retained(pid, wid, idx) {
                Some(e) => e,
                None => {
                    return ToolResult::error(format!(
                        "Element index {idx} not found. Call get_window_state first."
                    ))
                }
            };
            let element_ptr = element_guard.as_ptr();

            // Choose one background actuator before dispatch. An element that
            // advertises AXOpen uses the exact semantic route; all other
            // elements require the stricter routed-pointer proof. Do not let a
            // failed AXOpen silently cross into an ungated pointer fallback.
            let has_ax_open = tokio::task::spawn_blocking(move || unsafe {
                copy_action_names(element_ptr as AXUIElementRef)
                    .iter()
                    .any(|action| action == "AXOpen")
            })
            .await
            .unwrap_or(false);
            let _mutation_lease = if !delivery_mode.is_foreground() {
                let action = background_action_for_element(has_ax_open);
                match super::gate_background_window_action(pid, wid, Some(element_ptr), action)
                    .await
                {
                    Ok(lease) => Some(lease),
                    Err(refusal_result) => return refusal_result,
                }
            } else {
                None
            };

            // Thread the resolved session cursor key into the blocking AX path
            // so its ClickPulse lands on THIS session's cursor, not "default".
            let ck = cursor_key.clone();
            let result = tokio::task::spawn_blocking(move || {
                ax_double_click(
                    pid,
                    wid,
                    element_ptr,
                    idx,
                    &ck,
                    has_ax_open,
                    delivery_mode.is_foreground(),
                )
            })
            .await;

            return match result {
                Ok(Ok(msg)) => ToolResult::text(msg),
                Ok(Err(e)) => ToolResult::error(format!("double_click failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // ── Pixel path ───────────────────────────────────────────────────────
        let mut cx = match args.get("x").and_then(|v| v.as_f64()) {
            Some(v) => v,
            None => {
                return ToolResult::error(
                    "Either element_index + window_id or x + y must be provided.",
                )
            }
        };
        let mut cy = match args.get("y").and_then(|v| v.as_f64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: y"),
        };

        // Scale back from downscaled-image space to native pixels when needed.
        if let Some(ratio) = self.state.resize_registry.ratio(pid, window_id) {
            cx *= ratio;
            cy *= ratio;
        }

        // Window-local → screen coordinate translation + win-local logical coords
        // for CGEventSetWindowLocation (shared with click.rs via px_frame, which
        // refuses a window with no live frame instead of silently treating the
        // local point as screen-absolute).
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
                            "double_click: window-local point ({:.1}, {:.1}) pt lies outside \
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

        let fg = delivery_mode.is_foreground() && window_id.is_some();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let do_click = move || -> anyhow::Result<()> {
                if let Some(wid) = window_id {
                    crate::input::mouse::click_at_xy_with_window_local(
                        pid,
                        screen_x,
                        screen_y,
                        win_local_x,
                        win_local_y,
                        wid,
                        2,
                        &[],
                    )
                } else {
                    crate::input::mouse::click_at_xy(pid, screen_x, screen_y, 2, &[])
                }
            };
            // Foreground rung: brief front → double-click → restore prior frontmost.
            match (fg, window_id) {
                (true, Some(wid)) => {
                    crate::input::skylight::with_foreground_assist(
                        pid as libc::pid_t,
                        wid,
                        do_click,
                    )?;
                    Ok(())
                }
                _ => do_click(),
            }
        })
        .await;

        let mode_label = if fg {
            " (delivery_mode:foreground)"
        } else {
            ""
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!("✅ Double-clicked at ({screen_x:.1}, {screen_y:.1}){mode_label}."))
                .with_structured(serde_json::json!({
                    "path": if fg { "cgevent_fg" } else { "cgevent" }, "verified": false, "effect": "unverifiable"
                })),
            Ok(Err(e)) => ToolResult::error(format!("Double-click failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking AX path ─────────────────────────────────────────────────────────

fn ax_double_click(
    pid: i32,
    wid: u32,
    element_ptr: usize,
    idx: usize,
    cursor_key: &str,
    has_ax_open: bool,
    allow_pointer_fallback: bool,
) -> anyhow::Result<String> {
    let element = element_ptr as AXUIElementRef;

    // Try AXOpen first (Finder items, openable list rows, document cells).
    if has_ax_open {
        let err = unsafe { perform_action(element, "AXOpen") };
        if err == kAXErrorSuccess {
            return Ok(format!("AXOpen performed on element [{idx}]."));
        }
        if !allow_pointer_fallback {
            anyhow::bail!(
                "AXOpen returned {err} for element [{idx}]; background delivery will not \
                 improvise a pointer fallback after choosing the semantic route"
            );
        }
        tracing::debug!(
            "AXOpen returned {err} for element [{idx}], falling back to pixel double-click"
        );
    }

    // Resolve screen center and fall back to pixel double-click.
    let (cx, cy) = unsafe { element_screen_center(element) }
        .ok_or_else(|| anyhow::anyhow!("Cannot resolve screen center for element [{idx}]"))?;

    // Drive THIS session's cursor (threaded in via `cursor_key`), not "default".
    crate::cursor::overlay::send_command(
        cursor_key.to_owned(),
        cursor_overlay::OverlayCommand::ClickPulse { x: cx, y: cy },
    );
    // Use the window-local primitive (not bare click_at_xy): a plain
    // click_at_xy does NOT reliably reach a backgrounded / non-key window — it
    // no-ops on AppKit controls that hit-test the window-local stamp. Mirror the
    // delivering path the `click` pixel branch uses so the double-click actually
    // lands without foregrounding the app.
    // Window-routed target: we MUST have the window's bounds to translate the
    // screen center into a window-local stamp. If bounds are missing, refuse
    // rather than stamping screen coords as window-local — that no-ops or hits
    // the wrong location while still routing to `wid`.
    let (wx, wy) = crate::windows::window_bounds_by_id(wid)
        .map(|b| (cx - b.x, cy - b.y))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot resolve window bounds for window_id {wid}; refusing to stamp \
             screen coordinates as window-local for element [{idx}]."
            )
        })?;
    crate::input::mouse::click_at_xy_with_window_local(pid, cx, cy, wx, wy, wid, 2, &[])?;
    Ok(format!(
        "✅ Double-clicked element [{idx}] at ({cx:.1}, {cy:.1})."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_element_route_does_not_guess_across_actuator_classes() {
        assert_eq!(
            background_action_for_element(true),
            cua_driver_core::background_input::BackgroundAction::AxSemantic
        );
        assert_eq!(
            background_action_for_element(false),
            cua_driver_core::background_input::BackgroundAction::WindowPointer
        );
    }
}
