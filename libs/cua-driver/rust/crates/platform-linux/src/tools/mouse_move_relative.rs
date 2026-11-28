// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use cua_driver_core::{
    action_record::ActionTransport,
    protocol::ToolResult,
    relative_pointer,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::time::Duration;

use super::impl_::{foreground_budget, input_error_result, spawn_blocking_bounded};

// ── mouse_move_relative ──────────────────────────────────────────────────────

pub struct MouseMoveRelativeTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MouseMoveRelativeTool {
    fn def(&self) -> &ToolDef {
        DEF.get_or_init(|| {
            relative_pointer::tool_def(
                " On X11 the motion is XTest relative motion. On Wayland it is the wlroots \
                 virtual pointer's relative motion (sway, Hyprland, niri) or libei relative \
                 motion (GNOME, KDE).",
                &[],
            )
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let request = match relative_pointer::parse(&args) {
            Ok(request) => request,
            Err(refusal) => return refusal,
        };
        let steps = request.steps();
        let count = steps.len();
        let budget = foreground_budget(0) + Duration::from_millis(request.duration_ms);
        let result = if crate::wayland::is_wayland() {
            spawn_blocking_bounded("mouse_move_relative", budget, move || {
                crate::wayland::move_pointer_relative(&steps)
            })
            .await
        } else {
            spawn_blocking_bounded("mouse_move_relative", budget, move || {
                crate::input::send_relative_motion_xtest(&steps)
                    .map(|sent| (ActionTransport::LinuxXTest, sent))
            })
            .await
        };
        match result {
            Ok(Ok((transport, sent))) => {
                relative_pointer::outcome(request, count, transport, &args, sent)
            }
            Ok(Err(error)) if crate::wayland::relative_pointer_unavailable(&error) => {
                relative_pointer::unsupported(format!(
                    "this Wayland compositor offers no relative pointer input (no wlroots \
                     virtual pointer, and no libei relative pointer): {error:#}"
                ))
            }
            Ok(Err(error)) if crate::input::foreground::error_code(&error).is_some() => {
                input_error_result(error)
            }
            Ok(Err(error)) => relative_pointer::not_sent(&error),
            Err(error) => ToolResult::error(format!("Task error: {error}")),
        }
    }
}
