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
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{
    copy_action_names, copy_children, copy_string_attr, element_screen_rect,
    AXUIElementRef,
};
use crate::apps;
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;
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
            "Click against a target pid. **Prefer `element_index` over pixel \
             coordinates** — element_index works on backgrounded / minimized / hidden / \
             off-Space windows, surfaces a stable handle that survives rebuilds, and tells \
             you what you're clicking via the cached element's role + label. Reach for \
             `x, y` only when the target is a canvas / video / WebGL / custom-drawn surface \
             that doesn't appear in the AX tree.\n\n\
             Two addressing modes:\n\n\
             - element_index + window_id (from last get_window_state): AX action path. \
               Works on backgrounded/hidden windows. No cursor move, no focus steal. \
               element_index cache is scoped per (pid, window_id) and is replaced by the \
               next snapshot of the same window — re-snapshot every turn before clicking.\n\n\
             - x, y (window-local screenshot pixels, top-left origin of the PNG returned \
               by get_window_state): CGEvent path. Synthesizes mouse events and posts to \
               pid. Use modifier for cmd/shift/option/ctrl. Needs a visible on-screen \
               window to anchor the conversion.\n\n\
             button: \"left\" (default), \"right\", or \"middle\". Defaults to left so the \
             field is fully back-compat — omit it and you get the legacy left-click behaviour. \
             Pixel path: routes through the CGEvent left/right/middle mouse-button primitives. \
             AX path: \"right\" maps to AXShowMenu (same surface as the dedicated `right_click` \
             tool); \"middle\" has no AX equivalent and falls back to a pixel middle-click at the \
             element's center.\n\
             action: press (default), show_menu, pick, confirm, cancel, open.\n\
             from_zoom: set true after a zoom call to auto-translate zoom-image pixel \
             coordinates to full-window space."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
                "pid":           { "type": "integer", "description": "Target process ID." },
                "window_id":     { "type": "integer", "description": "Target window ID. Required for element_index. Optional when element_token is supplied (the token carries it)." },
                "element_index": { "type": "integer", "description": "Element index from last get_window_state. REQUIRES `pid` and `window_id` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op." },
                "element_token": { "type": "string",  "description": "Opaque per-snapshot element handle from `structuredContent.elements[].element_token` of the last get_window_state. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded — re-snapshot in that case." },
                "x":             { "type": "number",  "description": "Window-local screenshot X coordinate." },
                "y":             { "type": "number",  "description": "Window-local screenshot Y coordinate." },
                "action":        { "type": "string",  "description": "AX action: press, show_menu, pick, confirm, cancel, open." },
                "button":        {
                    "type": "string",
                    "enum": ["left", "right", "middle"],
                    "description": "Mouse button. Default: \"left\" — omit for legacy left-click behaviour. Pixel path uses the matching CGEvent primitive; AX path maps \"right\" to AXShowMenu and falls back to a pixel middle-click at the element's center for \"middle\"."
                },
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
        use cua_driver_core::tool_args::ArgsExt;

        // ── Window-less screen-absolute branch (capture_scope="desktop") ──────
        // x,y given with NO pid and NO window_id → the coordinates are TRUE
        // SCREEN pixels. This is the foreground, vision-driven desktop-scope
        // path, the macOS peer of the Windows WindowFromPoint click. Gate on the
        // effective scope: under "window" return a structured
        // `desktop_scope_disabled` error (same contract as Windows) rather than
        // silently treating window-local pixels as screen pixels.
        let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
        let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
        let has_xy = args.get("x").map(|v| v.is_number()).unwrap_or(false)
            && args.get("y").map(|v| v.is_number()).unwrap_or(false);
        if has_xy && !has_pid && !has_window_id {
            let session_id = args.opt_str("_session_id");
            let scope = self
                .state
                .session_config
                .effective_scope(session_id.as_deref(), &self.state.config.read().unwrap());
            if scope != "desktop" {
                return ToolResult::error(
                    "click: x,y given with no pid/window_id, but capture_scope is \
                     \"window\". Screen-absolute clicks require desktop scope. Call \
                     set_config with capture_scope=desktop (and use get_desktop_state \
                     to read true screen pixels) first."
                        .to_string(),
                )
                .with_structured(serde_json::json!({
                    "code": "desktop_scope_disabled",
                    "capture_scope": scope,
                    "suggestion": "set_config capture_scope=desktop",
                }));
            }
            let sx = args.opt_f64("x").or_else(|| args.opt_i64("x").map(|i| i as f64)).unwrap_or(0.0);
            let sy = args.opt_f64("y").or_else(|| args.opt_i64("y").map(|i| i as f64)).unwrap_or(0.0);
            let button = match args.str_or("button", "left").to_lowercase().as_str() {
                "right" => "right",
                "middle" => "middle",
                "" | "left" => "left",
                other => {
                    return ToolResult::error(format!(
                        "click: unknown button \"{other}\" — expected one of left, right, middle."
                    ))
                }
            }
            .to_string();
            let count = args.u64_or("count", 1) as usize;
            // Glide the session's agent cursor to the screen point for visibility.
            let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
            crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), sx, sy).await;
            self.state.cursor_registry.update_position(&cursor_key, sx, sy);

            // Resolve the frontmost on-screen window under the point (the macOS
            // peer of Windows' WindowFromPoint). When found, click THAT pid via
            // the proven SkyLight path (`click_at_xy`, screen coords) — reliable
            // on AppKit/Chromium where a bare HID post can miss. Only when no
            // app window owns the pixel (desktop background, etc.) fall back to
            // the cursor-warp + HID post.
            // Resolve as (pid, window_id, win_origin_x, win_origin_y) so the
            // click can stamp the window-LOCAL point — AppKit hit-tests the
            // stamped window-local coordinate, not the bare screen point, so a
            // plain screen-coord post misses.
            // Exclude our OWN windows (the agent-cursor overlay we just glided to
            // the point sits on top of the target — never resolve the click to it).
            let own_pid = std::process::id() as i32;
            let target = {
                let mut wins = crate::windows::visible_windows();
                wins.sort_by_key(|w| w.z_index); // front-to-back
                wins.into_iter().find(|w| {
                    w.layer == 0
                        && w.pid != own_pid
                        && sx >= w.bounds.x
                        && sx < w.bounds.x + w.bounds.width
                        && sy >= w.bounds.y
                        && sy < w.bounds.y + w.bounds.height
                }).map(|w| (w.pid, w.window_id, w.bounds.x, w.bounds.y))
            };
            let btn = button.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<i32>> {
                match target {
                    Some((pid, wid, ox, oy)) => {
                        let (wx, wy) = (sx - ox, sy - oy);
                        crate::input::mouse::click_at_xy_with_window_local(
                            pid, sx, sy, wx, wy, wid, count, &[],
                        )?;
                        Ok(Some(pid))
                    }
                    None => {
                        crate::input::mouse::click_at_xy_desktop(sx, sy, count, &btn)?;
                        Ok(None)
                    }
                }
            })
            .await;
            return match result {
                Ok(Ok(Some(pid))) => ToolResult::text(format!(
                    "✅ Sent click at screen ({sx},{sy}) on pid {pid} (desktop scope)."
                )),
                Ok(Ok(None)) => ToolResult::text(format!(
                    "✅ Sent screen-absolute click at ({sx},{sy}) (desktop scope, no window under point)."
                )),
                Ok(Err(e)) => ToolResult::error(format!("desktop-scope click failed: {e}")),
                Err(e) => ToolResult::error(format!("task error: {e}")),
            };
        }

        let pid = match args.require_i32("pid") { Ok(v) => v, Err(e) => return e };
        // Resolve this action's cursor key so its click-pulse / glide land on
        // the calling session's cursor, not the shared "default" one.
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);

        // Surface 6: resolve element_token / element_index precedence
        // BEFORE the pixel-path fallback. Token wins on disagreement; a
        // stale token returns an explicit error instead of silently
        // falling back to the integer (Surface 6 hard constraint).
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg     = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg,
            "click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id, _via_token) = match resolved {
            cua_driver_core::element_token::ResolvedElement::None => (None, window_id_arg, false),
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: wid, element_index: idx, via_token,
            } => (Some(idx), wid, via_token),
        };
        let x             = args.opt_f64("x").or_else(|| args.opt_i64("x").map(|i| i as f64));
        let y             = args.opt_f64("y").or_else(|| args.opt_i64("y").map(|i| i as f64));
        let action        = args.str_or("action", "press");
        // Surface 5: optional `button` arg, default "left" preserves legacy behaviour.
        // Pixel path: routes to left/right/middle CGEvent primitives.
        // AX path: "right" delegates to AXShowMenu (same surface as right_click);
        // "middle" has no AX equivalent and falls back to a pixel middle-click
        // at the element's screen-space center.
        let button_str    = args.str_or("button", "left").to_lowercase();
        // Reject unknown buttons explicitly so silent left-click fall-through can't
        // mask a typo. Keep "" → default left for old clients that never sent the field.
        if !matches!(button_str.as_str(), "" | "left" | "right" | "middle") {
            return ToolResult::error(format!(
                "click: unknown button \"{button_str}\" — expected one of left, right, middle."
            ));
        }
        let button_str = if button_str.is_empty() { "left".to_string() } else { button_str };
        let count         = args.u64_or("count", 1) as usize;
        let from_zoom     = args.bool_or("from_zoom", false);
        let debug_image_out = args.opt_str("debug_image_out");
        let modifiers: Vec<String> = args.str_array("modifier");

        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            // ── AX element path ────────────────────────────────────────────
            // Retain the element out of the cache so it can't be freed by a
            // concurrent get_window_state on the same (pid, window_id) while
            // this click is mid-flight (use-after-free → daemon crash). The
            // guard lives to the end of this method, past the AX action below.
            let element_guard = match self.state.element_cache.get_element_retained(pid, wid, idx) {
                Some(e) => e,
                None => return ToolResult::error(format!(
                    "Element index {idx} not found in cache for pid={pid} window_id={wid}. \
                     Call get_window_state first."
                )),
            };
            let element_ptr = element_guard.as_ptr();

            // Surface 5: button=right on the AX path → AXShowMenu (the same surface
            // the dedicated `right_click` tool dispatches). Threads through the
            // identical perform_ax_click code path with the action remapped.
            let effective_action = if button_str == "right" && action == "press" {
                "show_menu".to_string()
            } else {
                action.clone()
            };

            // Animate cursor to element center BEFORE firing AX action,
            // mirroring Swift's `performElementClick` → `animateAndWait(to:)`.
            let center_ptr = element_ptr;
            let center = tokio::task::spawn_blocking(move || unsafe {
                crate::ax::bindings::element_screen_center(center_ptr as AXUIElementRef)
            }).await.ok().flatten();

            // Surface 5: button=middle on the AX path has no AX equivalent.
            // Fall back to a pixel middle-click at the element's screen-space center
            // so the request still produces a real middle-button event (browser tab
            // close, autoscroll, etc.). If we can't resolve a center, error rather
            // than silently degrade to AXPress.
            if button_str == "middle" {
                let (cx, cy) = match center {
                    Some(c) => c,
                    None => return ToolResult::error(
                        "click(button=middle) on element_index: could not resolve element \
                         center for the pixel-middle-click fallback. Pass x, y directly."
                    ),
                };
                crate::cursor::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
                crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), cx, cy).await;
                self.state.cursor_registry.update_position(&cursor_key, cx, cy);

                let mods_owned = modifiers.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                    crate::input::mouse::middle_click_at_xy(pid, cx, cy, &m)
                }).await;
                return match result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Posted middle-click to pid {pid} at element [{idx}] center."
                    )),
                    Ok(Err(e)) => ToolResult::error(format!("Middle-click failed: {e}")),
                    Err(e)     => ToolResult::error(format!("Task error: {e}")),
                };
            }

            if let Some((cx, cy)) = center {
                // Pin overlay above target window first.
                crate::cursor::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
                crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), cx, cy).await;
                // Keep the registry in sync with the overlay so
                // get_agent_cursor_state reports a truthful position even when
                // the click was dispatched via the AX path (no pixel coords).
                self.state.cursor_registry.update_position(&cursor_key, cx, cy);
            }

            // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
            // Capture prior frontmost, arm the wildcard suppressor in the
            // snapshot, then arm a targeted suppressor across the AX action
            // itself via FocusGuard. After the action returns, detect any
            // new-window / foreground side-effects and append a one-liner
            // suffix matching Swift's wording.
            let prior_front = apps::frontmost_pid();
            let snapshot = WindowChangeDetector::snapshot(prior_front);

            // Run AX work on a blocking thread (can't block async executor).
            // Use `effective_action` so button=right rewrites press → show_menu.
            let action_clone = effective_action.clone();
            // Thread the resolved session cursor key into the blocking AX path
            // so its ShowFocusRect + ClickPulse land on THIS session's cursor,
            // not the shared "default" one (which would light the wrong cursor
            // and stomp default for a non-default session).
            let ck = cursor_key.clone();
            let result = focus_guard::with_focus_suppressed(
                Some(pid),
                prior_front,
                "click.AXPress",
                || async move {
                    tokio::task::spawn_blocking(move || {
                        perform_ax_click(element_ptr, idx, pid, wid, &action_clone, &ck)
                    })
                    .await
                },
            )
            .await;

            // Drop the wildcard lease + detect window/foreground side-effects.
            let changes = snapshot.detect_async().await;

            match result {
                Ok(Ok((mut msg, needs_webkit_delay))) => {
                    // For text inputs, wait 800ms for WebKit DOM focus to settle
                    // before returning — matches the Swift reference behaviour.
                    if needs_webkit_delay {
                        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    }
                    msg.push_str(&changes.result_suffix());
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
                        // Session-effective max dimension so debug_image_out
                        // matches the resize the calling session sees in
                        // get_window_state (precedence: session override > global).
                        let max_dim = self.state.session_config
                            .effective(
                                args.opt_str("_session_id").as_deref(),
                                &self.state.config.read().unwrap(),
                            )
                            .1;
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
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
            }
            // Animate the visual cursor to the click point and wait for it to
            // arrive — mirrors Swift's `AgentCursor.shared.animateAndWait(to:)`.
            crate::cursor::overlay::animate_cursor_to(cursor_key.clone(), screen_x, screen_y).await;
            // Keep the registry in sync with the overlay (see AX path above).
            self.state.cursor_registry.update_position(&cursor_key, screen_x, screen_y);
            // Show click-pulse on the agent cursor overlay.
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: screen_x, y: screen_y },
            );

            // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
            // A pixel click can land on a "Sign In" button that opens a sheet
            // or a Safari link that activates a new tab — same side-effect
            // shape as the AX path, so we wrap identically.
            let prior_front = apps::frontmost_pid();
            let snapshot = WindowChangeDetector::snapshot(prior_front);

            let mods_owned = modifiers.clone();
            // Surface 5: route to the right/middle CGEvent primitives when
            // button != left. Left-button path stays on the existing Chromium-
            // routed `click_at_xy_with_window_local` for back-compat.
            let button_kind = button_str.clone();
            let result = focus_guard::with_focus_suppressed(
                Some(pid),
                prior_front,
                "click.pixel",
                || async move {
                    tokio::task::spawn_blocking(move || {
                        let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                        match button_kind.as_str() {
                            "right" => {
                                if let Some(_wid) = window_id {
                                    return crate::input::mouse::right_click_at_xy_with_window_local(
                                        pid, screen_x, screen_y, win_local_x, win_local_y, &m,
                                    );
                                }
                                crate::input::mouse::right_click_at_xy(pid, screen_x, screen_y, &m)
                            }
                            "middle" => {
                                if let Some(_wid) = window_id {
                                    return crate::input::mouse::middle_click_at_xy_with_window_local(
                                        pid, screen_x, screen_y, win_local_x, win_local_y, &m,
                                    );
                                }
                                crate::input::mouse::middle_click_at_xy(pid, screen_x, screen_y, &m)
                            }
                            // "left" (default) or anything else — preserve legacy left-click path.
                            _ => {
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
                            }
                        }
                    })
                    .await
                },
            )
            .await;

            let changes = snapshot.detect_async().await;

            let button_label = match button_str.as_str() {
                "right"  => "right-click",
                "middle" => "middle-click",
                _        => "click",
            };
            match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Posted {button_label} to pid {pid}.{}",
                    changes.result_suffix()
                )),
                Ok(Err(e)) => ToolResult::error(format!("{button_label} failed: {e}")),
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
    cursor_key: &str,
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
        // Drive THIS session's cursor (threaded in via `cursor_key`), matching
        // the keyed glide already played in the invoke body above. The keyed
        // glide already played on the session's cursor in the invoke body.
        crate::cursor::overlay::send_command(
            cursor_key.to_owned(),
            cursor_overlay::OverlayCommand::ShowFocusRect(Some(rect)),
        );
        // Animate cursor to element center.
        let cx = rect[0] + rect[2] / 2.0;
        let cy = rect[1] + rect[3] / 2.0;
        crate::cursor::overlay::send_command(
            cursor_key.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse { x: cx, y: cy },
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Surface 5: schema must advertise the new `button` field with the three
    /// canonical values and default to "left". Hermes / Codex / Claude Code
    /// consumers branch on this enum being present.
    #[test]
    fn schema_advertises_button_enum() {
        let d = def();
        let props = d.input_schema.get("properties").expect("properties");
        let button = props.get("button").expect("button field present");
        let kind = button.get("type").and_then(|v| v.as_str());
        assert_eq!(kind, Some("string"));
        let enum_vals: Vec<&str> = button
            .get("enum")
            .and_then(|v| v.as_array())
            .expect("button.enum present")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(enum_vals.contains(&"left"));
        assert!(enum_vals.contains(&"right"));
        assert!(enum_vals.contains(&"middle"));
    }

    /// Surface 5 hard constraint: the tool description must mention the
    /// `button` argument and the "left" default so MCP introspection (which
    /// pipes description into LLM prompts) carries the back-compat note.
    #[test]
    fn description_mentions_button_default() {
        let d = def();
        let desc = d.description.to_ascii_lowercase();
        assert!(desc.contains("button"), "description should mention button arg");
        assert!(desc.contains("left"), "description should mention left default");
        assert!(desc.contains("middle"), "description should mention middle button");
    }

    /// Existing default behaviour preserved: no `button` field on the call →
    /// resolves to "left" inside invoke. We can't drive the AX path without a
    /// live macOS Window Server, but we CAN check the same arg-parsing logic
    /// the invoke uses produces "left" for empty / absent input.
    #[test]
    fn button_defaults_to_left_when_absent() {
        use cua_driver_core::tool_args::ArgsExt;
        let args = serde_json::json!({ "pid": 1234 });
        let button_str_raw = args.str_or("button", "left").to_lowercase();
        let resolved = if button_str_raw.is_empty() { "left".to_string() } else { button_str_raw };
        assert_eq!(resolved, "left");
    }

    /// Round-trip the three canonical values through the same parse the invoke
    /// uses, so any future refactor that changes str_or semantics breaks here
    /// before it breaks consumers.
    #[test]
    fn button_round_trips_right_and_middle() {
        use cua_driver_core::tool_args::ArgsExt;
        for v in ["left", "right", "middle"] {
            let args = serde_json::json!({ "pid": 1234, "button": v });
            let s = args.str_or("button", "left").to_lowercase();
            assert_eq!(s, v);
        }
    }
}
