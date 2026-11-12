// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use cua_driver_contract::MoveCursorInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_projection,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    foreground_budget, input_error_result, isolated_hyprland_background, named_session_cursor_key,
    resolve_cursor_key, reveal_pointer_action_for, screenshot_scale, spawn_blocking_bounded,
    spawn_isolated_hyprland, ToolState,
};

// ── move_cursor ───────────────────────────────────────────────────────────────

pub struct MoveCursorTool {
    pub(crate) state: Arc<ToolState>,
}

static MCURSOR_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorControlScope {
    Agent,
    Desktop,
}

fn cursor_control_scope(args: &Value) -> CursorControlScope {
    match args.get("scope").and_then(Value::as_str) {
        Some("desktop") => CursorControlScope::Desktop,
        _ => CursorControlScope::Agent,
    }
}

#[async_trait]
impl Tool for MoveCursorTool {
    fn def(&self) -> &ToolDef {
        MCURSOR_DEF.get_or_init(|| ToolDef {
            name: "move_cursor".into(),
            description: "Move the synthetic agent cursor without changing the user's pointer. With target {kind: \"window\", pid, window_id}, x/y are window screenshot pixels (as for click); on Hyprland the background app is also hovered through Cua's own input seat, so hover styles, hover-only controls and tooltips appear while the user's pointer stays put, and the result says whether the app saw the hover (hover_delivered). Without a target, x/y are screen coordinates for the overlay only. Only an explicit scope=desktop request moves the real OS pointer in get_desktop_state coordinates.".into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "x":{"type":"number","description":"Destination X. Window target: window screenshot pixels. No target: screen coordinates of the agent cursor overlay. Desktop scope: get_desktop_state screenshot pixels."},
                "y":{"type":"number","description":"Destination Y, in the same space as x."},
                "session": cua_driver_core::tool_schema::session_schema(),
                "cursor_id":{"type":"string","description":"Cursor instance to move. Default: 'default'."},
                "scope":{"type":"string","enum":["window","desktop"],"default":"window","description":"\"window\" (default) moves only the agent cursor overlay; \"desktop\" moves the real OS pointer."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if cursor_control_scope(&args) == CursorControlScope::Desktop {
            let input = match parse_typed_projection::<MoveCursorInput>("move_cursor", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let (x, y) = (input.x, input.y);
            let xi = x.round() as i32;
            let yi = y.round() as i32;
            let wayland = crate::wayland::is_wayland();
            let path = if wayland {
                "wayland_desktop"
            } else {
                "xtest_desktop"
            };
            let result = if wayland {
                spawn_blocking_bounded("move_cursor", foreground_budget(0), move || {
                    let space = crate::wayland::DesktopInputSpace::current()?;
                    crate::wayland::move_cursor_desktop(&space, xi, yi)
                })
                .await
            } else {
                spawn_blocking_bounded("move_cursor", foreground_budget(0), move || {
                    crate::input::send_move_xtest_desktop(xi, yi)
                })
                .await
            };
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Moved the real desktop pointer to ({xi}, {yi})."
                ))
                .with_structured(
                    json!({"scope":"desktop","path":path,"x":xi,"y":yi,"effect":"unverifiable"}),
                ),
                Ok(Err(error)) if crate::input::foreground::error_code(&error).is_some() => {
                    input_error_result(error)
                }
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        if let (Some(pid), Some(xid)) = (args.opt_u64("pid"), args.opt_u64("window_id")) {
            let Ok(pid) = u32::try_from(pid) else {
                return ToolResult::error("move_cursor: pid out of range");
            };
            return self.move_over_window(&args, pid, xid).await;
        }
        let x = args.f64_or("x", 0.0);
        let y = args.f64_or("y", 0.0);
        let cursor_id = resolve_cursor_key(&args);
        // End pointing upper-left (45°) — matches Swift's
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention so the
        // overlay arrow settles to the natural macOS-style pose.
        // Use the acknowledged animation path so a first-ever move seeds and
        // displays the session cursor just as reliably as a coordinate click.
        reveal_pointer_action_for(&self.state, &cursor_id, x, y, false).await;
        ToolResult::text(format!(
            "Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1}); the user pointer was unchanged."
        ))
    }
}

impl MoveCursorTool {
    /// `target: {kind: "window", pid, window_id}`: x/y are window screenshot
    /// pixels, as for `click`. Moves the agent cursor over that point and, on
    /// Hyprland, hovers the app there through the lane's agent seat (`HOVER`),
    /// which never touches the user's pointer, focus or stacking.
    async fn move_over_window(&self, args: &Value, pid: u32, xid: u64) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let (mut x, mut y) = match (args.require_f64("x"), args.require_f64("y")) {
            (Ok(x), Ok(y)) => (x, y),
            (Err(error), _) | (_, Err(error)) => return error,
        };
        let (shot_x, shot_y) = (x, y);
        match screenshot_scale(&self.state, args, pid, Some(xid)) {
            Ok(scale) => {
                x *= scale;
                y *= scale;
            }
            Err(refusal) => return refusal,
        }
        let screen = if crate::wayland::is_wayland() {
            let (lx, ly) = (x.round() as i32, y.round() as i32);
            match tokio::task::spawn_blocking(move || {
                crate::wayland::window_local_to_output(xid, lx, ly)
            })
            .await
            {
                Ok(Ok(aimed)) => Some((aimed.point.0 as f64, aimed.point.1 as f64)),
                Ok(Err(error)) => return input_error_result(error),
                Err(error) => return ToolResult::error(format!("Task error: {error}")),
            }
        } else {
            tokio::task::spawn_blocking(move || super::impl_::window_local_to_screen(xid, x, y))
                .await
                .ok()
                .and_then(Result::ok)
        };
        let cursor_id = resolve_cursor_key(args);
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Some((sx, sy)) = screen {
            reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, false).await;
        }
        let moved = format!(
            "Agent cursor '{cursor_id}' moved over window {xid} at ({shot_x:.0}, {shot_y:.0})"
        );
        let delivery = crate::input::delivery::DeliveryMode::from_args(args);
        if !isolated_hyprland_background(delivery) {
            return window_hover_result(&moved, xid, screen, HoverOutcome::Unavailable);
        }
        let owner = named_session_cursor_key(args);
        let action = crate::wayland::hyprland_input::Action::Hover { x, y };
        let (_cancellation, dispatch) = match spawn_isolated_hyprland(args, move |cancellation| {
            crate::wayland::hyprland_input::execute(owner, pid, xid, action, cancellation)
        }) {
            Ok(dispatch) => dispatch,
            Err(refusal) => {
                let reason = refusal_reason(refusal.structured_content.as_ref());
                return window_hover_result(&moved, xid, screen, HoverOutcome::Refused(reason));
            }
        };
        let outcome = match dispatch.await {
            Ok(Ok(reply)) if reply["ok"] == true => HoverOutcome::Delivered,
            Ok(Ok(reply)) => HoverOutcome::Refused(
                reply["code"]
                    .as_str()
                    .unwrap_or("background_unavailable")
                    .to_owned(),
            ),
            Ok(Err(error)) => HoverOutcome::Unknown(error.to_string()),
            Err(error) => HoverOutcome::Unknown(error.to_string()),
        };
        window_hover_result(&moved, xid, screen, outcome)
    }
}

#[derive(Debug, PartialEq)]
enum HoverOutcome {
    /// The app received pointer enter and motion on Cua's agent seat.
    Delivered,
    /// No background hover route on this desktop (not Hyprland with the
    /// Cua plugin, or a foreground request).
    Unavailable,
    /// The route refused before sending anything; carries the reason code.
    Refused(String),
    /// The route failed after it may have sent input.
    Unknown(String),
}

fn refusal_reason(structured: Option<&Value>) -> String {
    structured
        .and_then(|value| value.get("reason").or_else(|| value.get("code")))
        .and_then(Value::as_str)
        .unwrap_or("background_unavailable")
        .to_owned()
}

/// What to tell the agent after a window-target move. An app that saw no
/// hover must be named as such: the 6 Nov Omarchy run showed agents trusting
/// an overlay-only move and clicking without the hover the task needed.
fn window_hover_result(
    moved: &str,
    xid: u64,
    screen: Option<(f64, f64)>,
    outcome: HoverOutcome,
) -> ToolResult {
    let screen_point = screen.map(|(x, y)| json!([x, y]));
    let mut structured = json!({
        "scope": "window",
        "window_id": xid,
        "screen_point": screen_point,
        "effect": "unverifiable",
    });
    let text = match &outcome {
        HoverOutcome::Delivered => {
            structured["hover_delivered"] = json!(true);
            structured["hover_route"] = json!("linux_hyprland_agent_seat");
            format!(
                "{moved}, and hovered the app there through Cua's own input seat; the user's \
                 pointer, focus and windows did not change. Hover styles, hover-only controls and \
                 tooltips react to it. Call get_window_state to see them."
            )
        }
        HoverOutcome::Unknown(error) => {
            structured["hover_delivered"] = json!("unknown");
            structured["effect"] = json!("unknown");
            format!(
                "{moved}. The background hover may or may not have reached the app ({error}); \
                 read the window again before relying on it."
            )
        }
        HoverOutcome::Unavailable | HoverOutcome::Refused(_) => {
            structured["hover_delivered"] = json!(false);
            if let HoverOutcome::Refused(reason) = &outcome {
                structured["reason"] = json!(reason);
            }
            format!("{moved}; the user's pointer did not move.{NO_HOVER_NOTE}")
        }
    };
    ToolResult::text(text).with_structured(structured)
}

/// What a window-target move that delivered no hover means for the agent.
const NO_HOVER_NOTE: &str = " The app saw no hover: only the drawn agent cursor moved. To \
     reveal hover-only controls, read them with get_window_state and act on their element \
     tokens; for a web page use the browser tools. A desktop-scope move_cursor moves the user's \
     real pointer, so use it only when nobody is using this screen.";

#[cfg(test)]
mod window_hover_result_tests {
    use super::{refusal_reason, window_hover_result, HoverOutcome};
    use serde_json::json;

    fn structured(outcome: HoverOutcome) -> (String, serde_json::Value) {
        let result = window_hover_result("moved", 9, Some((10.0, 20.0)), outcome);
        let text = serde_json::to_string(&result.content).unwrap();
        (text, result.structured_content.unwrap())
    }

    #[test]
    fn delivered_hover_names_its_route() {
        let (_, value) = structured(HoverOutcome::Delivered);
        assert_eq!(value["hover_delivered"], true);
        assert_eq!(value["hover_route"], "linux_hyprland_agent_seat");
        assert_eq!(value["screen_point"], json!([10.0, 20.0]));
        assert_eq!(value["window_id"], 9);
    }

    #[test]
    fn an_overlay_only_move_says_the_app_saw_no_hover() {
        for outcome in [
            HoverOutcome::Unavailable,
            HoverOutcome::Refused("hover_unsupported".into()),
        ] {
            let refused = matches!(outcome, HoverOutcome::Refused(_));
            let (text, value) = structured(outcome);
            assert_eq!(value["hover_delivered"], false);
            assert!(text.contains("saw no hover"), "{text}");
            assert_eq!(value.get("reason").is_some(), refused);
        }
    }

    #[test]
    fn an_uncertain_dispatch_is_not_reported_as_no_hover() {
        let (_, value) = structured(HoverOutcome::Unknown("timeout".into()));
        assert_eq!(value["hover_delivered"], "unknown");
        assert_eq!(value["effect"], "unknown");
    }

    #[test]
    fn refusal_reason_prefers_the_specific_reason() {
        assert_eq!(
            refusal_reason(Some(
                &json!({"code":"background_unavailable","reason":"client_xwayland"})
            )),
            "client_xwayland"
        );
        assert_eq!(refusal_reason(Some(&json!({"code":"x"}))), "x");
        assert_eq!(refusal_reason(None), "background_unavailable");
    }
}

#[cfg(test)]
mod session_cursor_target_tests {
    use super::{cursor_control_scope, reveal_pointer_action_for, CursorControlScope, ToolState};
    use serde_json::json;

    #[test]
    fn real_pointer_control_requires_explicit_desktop_scope() {
        assert_eq!(cursor_control_scope(&json!({})), CursorControlScope::Agent);
        assert_eq!(
            cursor_control_scope(&json!({"scope": "window"})),
            CursorControlScope::Agent
        );
        assert_eq!(
            cursor_control_scope(&json!({"scope": "desktop"})),
            CursorControlScope::Desktop
        );
    }

    #[tokio::test]
    async fn pointer_position_survives_an_unavailable_overlay() {
        let state = ToolState::new();
        reveal_pointer_action_for(&state, "no-renderer", 123.0, 456.0, true).await;

        let cursor = state
            .cursor_registry
            .get("no-renderer")
            .expect("pointer action records its position independently of rendering");
        assert!(cursor.config.enabled, "input must revive its agent cursor");
        assert_eq!(cursor.x.zip(cursor.y), Some((123.0, 456.0)));
    }
}
