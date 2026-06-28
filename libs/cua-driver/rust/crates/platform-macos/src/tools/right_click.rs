use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;
use std::sync::Arc;

use crate::ax::bindings::{
    copy_action_names, perform_action, kAXErrorSuccess, AXUIElementRef,
    copy_string_attr,
};

use super::ToolState;

pub struct RightClickTool {
    state: Arc<ToolState>,
}

impl RightClickTool {
    pub fn new(state: Arc<ToolState>) -> Self { Self { state } }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "right_click".into(),
        description:
            "Right-click against a target pid. Two addressing modes:\n\n\
             - `element_index` + `window_id` (from the last `get_window_state` snapshot) — \
               performs `AXShowMenu` on the cached element. Pure AX RPC, works on backgrounded / \
               hidden windows, no cursor move or focus steal. Requires a prior \
               `get_window_state(pid, window_id)` in this turn.\n\n\
             - `x`, `y` — synthesizes `rightMouseDown` / `rightMouseUp` CGEvent pair posted \
               to the pid. Driver converts image-pixel → screen-point internally. \
               `modifier` forces the CGEvent path (AX actions don't propagate modifier keys).\n\n\
             Exactly one of `element_index` or (`x` AND `y`) must be provided. `pid` always \
             required. `window_id` required when `element_index` is used."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "session": { "type": "string", "description": "Optional session id: declares/uses the agent cursor and per-session state for this run. The same id works over MCP, the CLI, or the raw socket, and follows the run across apps/windows. Omit to run cursor-less." },
                "pid": { "type": "integer", "description": "Target process ID." },
                "element_index": {
                    "type": "integer",
                    "description": "Element index from last get_window_state. Routes through AXShowMenu. REQUIRES `pid` and `window_id` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op."
                },
                "element_token": {
                    "type": "string",
                    "description": "Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."
                },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID. Required when element_index is used. Optional when element_token is supplied (the token carries it)."
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
impl Tool for RightClickTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") { Ok(v) => v, Err(e) => return e };
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);

        // Surface 6: element_token / element_index precedence resolution.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg     = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg,
            "right_click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id) = match resolved {
            cua_driver_core::element_token::ResolvedElement::None => (None, window_id_arg),
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: wid, element_index: idx, via_token: _,
            } => (Some(idx), wid),
        };
        let x             = args.opt_f64("x");
        let y             = args.opt_f64("y");
        let has_xy        = x.is_some() && y.is_some();
        let partial_xy    = x.is_some() != y.is_some();
        let modifiers: Vec<String> = args.str_array("modifier");

        if partial_xy {
            return ToolResult::error("Provide both x and y together, not just one.");
        }
        if element_index.is_some() && has_xy {
            return ToolResult::error("Provide either element_index or (x, y), not both.");
        }
        if element_index.is_none() && !has_xy {
            return ToolResult::error(
                "Provide element_index or (x, y) to address the right-click target."
            );
        }
        if element_index.is_some() && window_id.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used."
            );
        }

        // ── AX element path ──────────────────────────────────────────────────
        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            // Retain out of the cache so a concurrent get_window_state can't
            // free the element mid-action (use-after-free → daemon crash).
            let element_guard = match self.state.element_cache.get_element_retained(pid, wid, idx) {
                Some(e) => e,
                None    => return ToolResult::error(format!(
                    "Element index {idx} not found. Call get_window_state first."
                )),
            };
            let element_ptr = element_guard.as_ptr();

            let result = tokio::task::spawn_blocking(move || {
                ax_show_menu(element_ptr, idx, pid, wid)
            }).await;

            return match result {
                Ok(Ok(msg))  => ToolResult::text(msg),
                Ok(Err(e))   => ToolResult::error(format!("Right-click failed: {e}")),
                Err(e)       => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // ── Pixel path ───────────────────────────────────────────────────────
        let (mut cx, mut cy) = (x.unwrap(), y.unwrap());
        // Scale back from downscaled-image space to native pixels when needed.
        if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            cx *= ratio;
            cy *= ratio;
        }

        // Window-local → screen coordinate translation + win-local logical coords
        // for CGEventSetWindowLocation (matches click.rs enhancement).
        let (screen_x, screen_y, win_local_x, win_local_y) = if let Some(wid) = window_id {
            let result = tokio::task::spawn_blocking(move || {
                let bounds = crate::windows::window_bounds_by_id(wid);
                let scale: f64 = if let Some(ref b) = bounds {
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
                (cx, cy, cx, cy)
            }
        } else {
            (cx, cy, cx, cy)
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
            cursor_overlay::OverlayCommand::ClickPulse { x: screen_x, y: screen_y },
        );

        let mod_suffix = if modifiers.is_empty() {
            String::new()
        } else {
            format!(" with {}", modifiers.join("+"))
        };

        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            if let Some(wid) = window_id {
                crate::input::mouse::right_click_at_xy_with_window_local(
                    pid, screen_x, screen_y, win_local_x, win_local_y, wid, &m,
                )
            } else {
                crate::input::mouse::right_click_at_xy(pid, screen_x, screen_y, &m)
            }
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Right-clicked{mod_suffix} at ({screen_x:.1}, {screen_y:.1}).")),
            Ok(Err(e)) => ToolResult::error(format!("Right-click failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking AX path ─────────────────────────────────────────────────────────

fn ax_show_menu(element_ptr: usize, idx: usize, pid: i32, wid: u32) -> anyhow::Result<String> {
    let element = element_ptr as AXUIElementRef;

    let role  = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
    let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

    let advertised = unsafe { copy_action_names(element) };

    // Only attempt the pure-AX AXShowMenu when the element actually advertises
    // it. Plain controls (NSButton, custom NSView click targets, most web
    // nodes) DON'T — calling AXShowMenu on them returns kAXErrorActionUnsupported
    // (-25206), which used to surface as a hard "AXShowMenu failed" error and
    // forced the agent onto raw pixels. Instead, resolve the element's on-screen
    // center and synthesize a REAL pixel right-click there — the same actuation
    // a user performs, delivered to backgrounded windows via the window-local
    // primitive. This makes "right-click element N" land on any element, not
    // just ones with a native context-menu AX action.
    if advertised.iter().any(|a| a == "AXShowMenu") {
        let err = unsafe { perform_action(element, "AXShowMenu") };
        if err == kAXErrorSuccess {
            return Ok(format!("Shown menu for [{idx}] {role} \"{title}\" (AXShowMenu)."));
        }
        // Advertised but the action failed — fall through to the pixel path
        // rather than erroring out.
        tracing::debug!("AXShowMenu returned {err} for [{idx}]; falling back to pixel right-click");
    }

    // Pixel right-click at the element's screen-space center.
    let (cx, cy) = unsafe { crate::ax::bindings::element_screen_center(element) }
        .ok_or_else(|| anyhow::anyhow!(
            "[{idx}] {role} \"{title}\" advertises no AXShowMenu and has no resolvable \
             on-screen center for a pixel right-click. Pass x, y directly."
        ))?;
    let (wx, wy) = crate::windows::window_bounds_by_id(wid)
        .map(|b| (cx - b.x, cy - b.y))
        .unwrap_or((cx, cy));
    crate::input::mouse::right_click_at_xy_with_window_local(pid, cx, cy, wx, wy, wid, &[])?;
    Ok(format!(
        "Right-clicked [{idx}] {role} \"{title}\" at element center ({cx:.0}, {cy:.0}) \
         (pixel right-click; element advertises no AXShowMenu)."
    ))
}
