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
            "Double-click at (x, y) or on an AX element identified by element_token.\n\n\
             AX path (element_token provided): performs `AXOpen` when the element advertises it \
             (Finder items, openable list rows/cells); otherwise resolves the element's on-screen \
             center and falls back to a pixel double-click there.\n\n\
             Pixel path (x, y provided): two down/up pairs ~80 ms apart at the given coordinates."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": [],
            "properties": {
                "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                "pid":           { "type": "integer", "description": "Target process ID. Optional with element_token (the token carries the pid)." },
                "x":             { "type": "number",  "description": "Screen X coordinate (pixel path)." },
                "y":             { "type": "number",  "description": "Screen Y coordinate (pixel path)." },
                "window_id":     { "type": "integer", "description": "CGWindowID. Omit when element_token is supplied (the token carries it)." },
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
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
        let pid = match super::target_pid(&self.state, &args) {
            Ok(v) => v,
            Err(e) => return e,
        };
        // delivery_mode: foreground briefly fronts the window before the pixel
        // double-click (the explicit last resort for surfaces that drop
        // background CGEvents), via the same skylight assist click uses.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
        let window_id_arg = args.opt_u64("window_id");
        let resolved = match self.state.snapshots.resolve(pid, &args) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id, element_guard) = resolved.into_parts(window_id_arg);
        let window_id = match super::native_window_id(window_id) {
            Ok(window_id) => window_id,
            Err(error) => return error,
        };

        // ── AX element path ──────────────────────────────────────────────────
        if let (Some(idx), Some(wid), Some(element_guard)) =
            (element_index, window_id, element_guard)
        {
            let element_ptr = element_guard.as_ptr();

            // Choose one background actuator before dispatch. An element that
            // advertises AXOpen uses the exact semantic route; all other
            // elements require the stricter routed-pointer proof. Do not let a
            // failed AXOpen silently cross into an ungated pointer fallback.
            let probe_guard = element_guard.clone();
            let has_ax_open = tokio::task::spawn_blocking(move || unsafe {
                copy_action_names(probe_guard.as_ptr() as AXUIElementRef)
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
            let foreground = delivery_mode.is_foreground();
            // AXOpen or the routed double-click can make the app activate
            // itself (opening a document or a register). In the background,
            // revert that and keep the user's key window, as `click` does.
            let prior_front = crate::apps::frontmost_pid();
            let snapshot = if foreground {
                crate::window_change_detector::WindowChangeDetector::snapshot_without_suppression(
                    prior_front,
                )
            } else {
                crate::window_change_detector::WindowChangeDetector::snapshot(prior_front)
            };
            let result = crate::focus_guard::with_focus_suppressed(
                (!foreground).then_some(pid),
                prior_front,
                "double_click.ax",
                || async move {
                    tokio::task::spawn_blocking(move || {
                        ax_double_click(
                            pid,
                            wid,
                            element_guard.as_ptr(),
                            idx,
                            &ck,
                            has_ax_open,
                            foreground,
                        )
                    })
                    .await
                },
            )
            .await;
            let changes = super::finish_window_observation(snapshot).await;

            return match result {
                Ok(Ok(msg)) => ToolResult::text(format!("{msg}{}", changes.result_suffix())),
                Ok(Err(e)) => ToolResult::error(format!("double_click failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // ── Pixel path ───────────────────────────────────────────────────────
        // A pixel double-click is a click with count 2. Route it through
        // `click` so it gets the same coordinate mapping, exact-window gate,
        // window-change observation, key-focus guard and focus-steal restore.
        // Some apps activate themselves when a double-click opens something
        // (GnuCash opening an account register); before this, the pixel
        // double-click had none of those and the activation stayed (Bench v2
        // CDB-G03, CUA-1341).
        if args.get("x").and_then(|v| v.as_f64()).is_none() {
            return ToolResult::error("Either element_token or x + y must be provided.");
        }
        if args.get("y").and_then(|v| v.as_f64()).is_none() {
            return ToolResult::error("Missing required parameter: y");
        }
        let mut click_args = args;
        if let Some(map) = click_args.as_object_mut() {
            map.insert("count".into(), serde_json::json!(2));
        }
        super::click::ClickTool::new(self.state.clone())
            .invoke(click_args)
            .await
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
    foreground: bool,
) -> anyhow::Result<String> {
    let element = element_ptr as AXUIElementRef;

    // Try AXOpen first (Finder items, openable list rows, document cells).
    if has_ax_open {
        let err = unsafe { perform_action(element, "AXOpen") };
        if err == kAXErrorSuccess {
            return Ok(format!("AXOpen performed on element [{idx}]."));
        }
        if !foreground {
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
    crate::input::mouse::click_at_xy_with_window_local(
        pid,
        cx,
        cy,
        wx,
        wy,
        wid,
        2,
        &[],
        crate::input::mouse::WindowClickDelivery::from_foreground(foreground),
    )?;
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
