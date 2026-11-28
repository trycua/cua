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

// ── mouse_move_relative ──────────────────────────────────────────────────────

pub struct MouseMoveRelativeTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MouseMoveRelativeTool {
    fn def(&self) -> &ToolDef {
        DEF.get_or_init(|| {
            relative_pointer::tool_def(
                " On Windows the motion is SendInput relative motion: an app reading raw input \
                 gets dx/dy as sent, while the visible cursor moves by them after the user's \
                 pointer speed and acceleration settings.",
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
        match tokio::task::spawn_blocking(move || {
            relative_pointer::send_steps(&steps, crate::input::mouse::send_relative_motion)
        })
        .await
        {
            Ok(sent) => relative_pointer::outcome(
                request,
                count,
                ActionTransport::WindowsSendInput,
                &args,
                sent,
            ),
            Err(error) => ToolResult::error(format!("Task error: {error}")),
        }
    }
}
