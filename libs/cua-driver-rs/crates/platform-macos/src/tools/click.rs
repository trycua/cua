//! click tool — matches the Swift reference ClickTool.swift.
//!
//! Two addressing modes:
//!
//! * **AX path** (`element_index` + `window_id`): performs AXAction on the cached
//!   element. Fires via AX RPC — the target app never needs to be frontmost.
//!   Extra behaviors vs. the naive dispatch:
//!   - AXTextField / AXTextArea: 800 ms post-click delay for WebKit DOM focus settle.
//!   - AXPopUpButton: appends the list of available options and redirects to set_value.
//!   - Advertised-action warning if the element didn't list the requested action.
//!
//! * **Pixel path** (`x`, `y`): synthesises CGEvent mouse clicks and posts them to
//!   the target pid.  `from_zoom=true` translates zoom-crop pixel coordinates back
//!   to full-window space using the most recent `zoom` context stored per-pid.

use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{
    copy_action_names, copy_children, copy_string_attr, element_screen_rect,
    AXUIElementRef,
};
use core_foundation::base::CFRelease;

use super::ToolState;

pub struct ClickTool {
    state: Arc<ToolState>,
}

impl ClickTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "click".into(),
        description:
            "Left-click against a target pid. Two addressing modes:\n\n\
             - element_index + window_id (from last get_window_state): AX action path. \
               Works on backgrounded/hidden windows. No cursor move, no focus steal. \
               Preferred path.\n\n\
             - x, y (window-local screenshot pixels): CGEvent path. Synthesizes mouse \
               events and posts to pid. Use modifier for cmd/shift/option/ctrl.\n\n\
             action: press (default), show_menu, pick, confirm, cancel, open.\n\
             from_zoom: set true after a zoom call to auto-translate zoom-image pixel \
             coordinates to full-window space."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "pid":           { "type": "integer", "description": "Target process ID." },
                "window_id":     { "type": "integer", "description": "Target window ID. Required for element_index." },
                "element_index": { "type": "integer", "description": "Element index from last get_window_state." },
                "x":             { "type": "number",  "description": "Window-local screenshot X coordinate." },
                "y":             { "type": "number",  "description": "Window-local screenshot Y coordinate." },
                "action":        { "type": "string",  "description": "AX action: press, show_menu, pick, confirm, cancel, open." },
                "count":         { "type": "integer", "description": "Click count (pixel path only). Default 1." },
                "modifier": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Modifier keys: cmd, shift, option/alt, ctrl."
                },
                "from_zoom": {
                    "type": "boolean",
                    "description": "When true, x and y are in the last zoom image for this pid; driver translates back to full-window coordinates."
                },
                "debug_image_out": {
                    "type": "string",
                    "description": "Optional file path. When set on a pixel-addressed click, captures a fresh screenshot, draws a red crosshair at (x, y), and writes the PNG. Use to verify coordinate spaces. Requires window_id; incompatible with from_zoom."
                }
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
impl Tool for ClickTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as i32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };

        let element_index = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let window_id     = args.get("window_id").and_then(|v| v.as_u64()).map(|v| v as u32);
        let x             = args.get("x").and_then(|v| v.as_f64())
            .or_else(|| args.get("x").and_then(|v| v.as_i64()).map(|i| i as f64));
        let y             = args.get("y").and_then(|v| v.as_f64())
            .or_else(|| args.get("y").and_then(|v| v.as_i64()).map(|i| i as f64));
        let action        = args.get("action").and_then(|v| v.as_str()).unwrap_or("press").to_owned();
        let count         = args.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let from_zoom     = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
        let debug_image_out = args.get("debug_image_out").and_then(|v| v.as_str()).map(str::to_owned);
        let modifiers: Vec<String> = args.get("modifier")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();

        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            // ── AX element path ────────────────────────────────────────────
            let element_ptr = match self.state.element_cache.get_element_ptr(pid, wid, idx) {
                Some(p) => p,
                None => return ToolResult::error(format!(
                    "Element index {idx} not found in cache for pid={pid} window_id={wid}. \
                     Call get_window_state first."
                )),
            };

            // Animate cursor to element center BEFORE firing AX action,
            // mirroring Swift's `performElementClick` → `animateAndWait(to:)`.
            let center_ptr = element_ptr;
            let center = tokio::task::spawn_blocking(move || unsafe {
                crate::ax::bindings::element_screen_center(center_ptr as AXUIElementRef)
            }).await.ok().flatten();

            if let Some((cx, cy)) = center {
                // Pin overlay above target window first.
                crate::cursor::overlay::send_command(
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64)
                );
                crate::cursor::overlay::animate_cursor_to(cx, cy).await;
            }

            // Run AX work on a blocking thread (can't block async executor).
            let action_clone = action.clone();
            let result = tokio::task::spawn_blocking(move || {
                perform_ax_click(element_ptr, idx, pid, wid, &action_clone)
            }).await;

            match result {
                Ok(Ok((msg, needs_webkit_delay))) => {
                    // For text inputs, wait 800ms for WebKit DOM focus to settle
                    // before returning — matches the Swift reference behaviour.
                    if needs_webkit_delay {
                        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    }
                    ToolResult::text(msg)
                }
                Ok(Err(e)) => ToolResult::error(format!("AX action failed: {e}")),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut cx), Some(mut cy)) = (x, y) {
            // ── Pixel path ─────────────────────────────────────────────────

            // debug_image_out: capture fresh screenshot, overlay crosshair BEFORE
            // any coordinate translation (so it shows received coords in the same
            // space the caller was reasoning in).
            if let Some(ref dbg_path) = debug_image_out {
                if from_zoom {
                    return ToolResult::error(
                        "debug_image_out is incompatible with from_zoom — \
                         received (x, y) would be in zoom-crop space, not window-local."
                    );
                }
                match window_id {
                    None => return ToolResult::error(
                        "debug_image_out requires window_id."
                    ),
                    Some(wid) => {
                        let max_dim = self.state.config.read().unwrap().max_image_dimension;
                        let dbg_path_c = dbg_path.clone();
                        let dbg_result = tokio::task::spawn_blocking(move || {
                            let png = crate::capture::screenshot_window_bytes(wid)?;
                            let png = crate::capture::resize_png_if_needed(&png, max_dim)?;
                            crate::capture::write_crosshair_png(&png, cx, cy, &dbg_path_c)
                        }).await;
                        match dbg_result {
                            Err(e) => return ToolResult::error(format!(
                                "debug_image_out task failed: {e}. Not dispatching click."
                            )),
                            Ok(Err(e)) => return ToolResult::error(format!(
                                "debug_image_out write failed: {e}. Not dispatching click."
                            )),
                            Ok(Ok(())) => {}
                        }
                    }
                }
            }

            if from_zoom {
                match self.state.zoom_registry.get(pid) {
                    Some(ctx) => {
                        let (wx, wy) = ctx.zoom_to_window(cx, cy);
                        cx = wx;
                        cy = wy;
                    }
                    None => return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                    )),
                }
            } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                // Coordinates are in the downscaled image space; scale back to native pixels.
                cx *= ratio;
                cy *= ratio;
            }

            // ── Window-local → screen coordinate translation ──────────────────
            // `click_at_xy` accepts screen-space coordinates (top-left origin).
            // Callers supply window-local screenshot pixels; we add the window's
            // screen-origin to produce the final screen position.
            //
            // Backing scale: screencapture captures at physical pixels, so on a
            // Retina display the screenshot is 2× the logical window size.
            // We detect the scale by comparing the live screenshot dimensions to
            // the window's logical bounds from WindowServer.
            //
            // win_local_x/y: window-local logical-pixel coords (= cx/scale, cy/scale)
            // needed for CGEventSetWindowLocation in the Chromium recipe.
            let (screen_x, screen_y, win_local_x, win_local_y) = if let Some(wid) = window_id {
                let result = tokio::task::spawn_blocking(move || {
                    let bounds = crate::windows::window_bounds_by_id(wid);
                    let scale: f64 = if let Some(ref b) = bounds {
                        // Detect Retina scale from the window screenshot.
                        // We take a tiny peek at the PNG dimensions to compare
                        // against the logical bounds.
                        if let Ok(png) = crate::capture::screenshot_window_bytes(wid) {
                            if png.len() >= 24 {
                                let pw = u32::from_be_bytes([png[16], png[17], png[18], png[19]]) as f64;
                                let lw = b.width;
                                if lw > 0.0 && pw > lw { pw / lw } else { 1.0 }
                            } else { 1.0 }
                        } else { 1.0 }
                    } else { 1.0 };
                    (bounds, scale)
                }).await.unwrap_or((None, 1.0));
                if let (Some(b), scale) = result {
                    let wx = cx / scale;
                    let wy = cy / scale;
                    (b.x + wx, b.y + wy, wx, wy)
                } else {
                    // window_id not found — fall back to treating x,y as screen coords.
                    (cx, cy, cx, cy)
                }
            } else {
                // No window_id → treat x,y as screen coordinates (legacy behaviour).
                (cx, cy, cx, cy)
            };

            // Pin the overlay above the target window BEFORE animating so
            // the cursor is already sandwiched correctly while it glides in.
            if let Some(wid) = window_id {
                crate::cursor::overlay::send_command(
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64)
                );
            }
            // Animate the visual cursor to the click point and wait for it to
            // arrive — mirrors Swift's `AgentCursor.shared.animateAndWait(to:)`.
            crate::cursor::overlay::animate_cursor_to(screen_x, screen_y).await;
            // Show click-pulse on the agent cursor overlay.
            crate::cursor::overlay::send_command(
                cursor_overlay::OverlayCommand::ClickPulse { x: screen_x, y: screen_y }
            );

            let mods_owned = modifiers.clone();
            let result = tokio::task::spawn_blocking(move || {
                let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                // When we know the window_id, pass the window-local coordinates so
                // `click_at_xy_with_window_local` can stamp `CGEventSetWindowLocation`
                // and Chromium-specific fields (f40, f51, f58, f91, f92) onto events
                // for better backgrounded-target delivery.
                if let Some(wid) = window_id {
                    return crate::input::mouse::click_at_xy_with_window_local(
                        pid, screen_x, screen_y,
                        win_local_x, win_local_y,
                        wid, count, &m,
                    );
                }
                crate::input::mouse::click_at_xy(pid, screen_x, screen_y, count, &m)
            }).await;

            match result {
                Ok(Ok(())) => ToolResult::text(format!("✅ Posted click to pid {pid}.")),
                Ok(Err(e)) => ToolResult::error(format!("Click failed: {e}")),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            ToolResult::error(
                "Provide either (element_index + window_id) or (x + y). pid is always required."
            )
        }
    }
}

// ── AX click implementation (blocking) ───────────────────────────────────────

/// Returns `(summary_text, needs_webkit_delay)`.
fn perform_ax_click(
    element_ptr: usize,
    idx: usize,
    pid: i32,
    window_id: u32,
    action_str: &str,
) -> anyhow::Result<(String, bool)> {
    let ax_action = map_action(action_str);
    let element = element_ptr as AXUIElementRef;

    // Capture advertised actions BEFORE dispatching so we can detect silent no-ops
    // (AX returns success even when the element doesn't advertise the action).
    let advertised = unsafe { copy_action_names(element) };

    let err = unsafe { crate::ax::bindings::perform_action(element, ax_action) };
    if err != crate::ax::bindings::kAXErrorSuccess {
        anyhow::bail!("AXUIElementPerformAction({ax_action}) returned {err}");
    }

    let role  = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
    let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

    let mut summary = format!("✅ Performed {ax_action} on [{idx}] {role} \"{title}\".");

    // AXPopUpButton: list available options, redirect to set_value.
    if role == "AXPopUpButton" {
        let children = unsafe { copy_children(element) };
        if !children.is_empty() {
            let options: Vec<String> = children.iter()
                .filter_map(|&child| {
                    let t = unsafe { copy_string_attr(child, "AXTitle") }.unwrap_or_default();
                    let v = unsafe { copy_string_attr(child, "AXValue") }.unwrap_or_default();
                    if t.is_empty() && v.is_empty() { return None; }
                    Some(if v.is_empty() || v == t {
                        format!("\"{t}\"")
                    } else {
                        format!("\"{t}\" (value: {v})")
                    })
                })
                .collect();
            for &child in &children { unsafe { CFRelease(child as _); } }

            if !options.is_empty() {
                let opt_list = options.join(", ");
                summary.push_str(
                    "\n\n⚠️ This is a popup/select button. The native macOS menu closes \
                     immediately when the window is in the background. Do NOT use click \
                     again — instead, use:\n  set_value(pid, window_id, element_index, value)\n\
                     Available options: ["
                );
                summary.push_str(&opt_list);
                summary.push(']');
            }
        }
    }

    // Advertised-action warning: non-fatal but surfaces likely no-ops.
    if !advertised.contains(&ax_action.to_string()) {
        let adv_list = if advertised.is_empty() { "none".into() } else { advertised.join(", ") };
        summary.push_str(&format!(
            "\n⚠️ Element does not advertise {ax_action} (actions: {adv_list}). \
             Action may have been a no-op."
        ));
    }

    // WebKit DOM focus settle: 800 ms for text inputs (returned to async caller).
    let needs_webkit_delay = ax_action == "AXPress"
        && (role == "AXTextField" || role == "AXTextArea");

    // Show focus-rect highlight around the element (matches Swift showFocusRect).
    // Also move the cursor to the element center so the glide animation plays.
    if let Some(rect) = unsafe { element_screen_rect(element) } {
        crate::cursor::overlay::send_command(
            cursor_overlay::OverlayCommand::ShowFocusRect(Some(rect))
        );
        // Animate cursor to element center.
        let cx = rect[0] + rect[2] / 2.0;
        let cy = rect[1] + rect[3] / 2.0;
        crate::cursor::overlay::send_command(
            cursor_overlay::OverlayCommand::ClickPulse { x: cx, y: cy }
        );
    }
    let _ = pid; let _ = window_id; // used by caller context

    Ok((summary, needs_webkit_delay))
}

fn map_action(action: &str) -> &'static str {
    match action.to_lowercase().as_str() {
        "press" | "click"          => "AXPress",
        "show_menu" | "right_click" => "AXShowMenu",
        "pick"                     => "AXPick",
        "confirm"                  => "AXConfirm",
        "cancel"                   => "AXCancel",
        "open"                     => "AXOpen",
        _                          => "AXPress",
    }
}
