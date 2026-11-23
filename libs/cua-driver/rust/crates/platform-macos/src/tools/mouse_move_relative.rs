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
                " On macOS the motion is a HID mouse-moved event carrying the delta (a dragged \
                 event while a button is down). When the pointer is over a window of an app that \
                 is not in front it refuses unless delivery_mode is \"foreground\".",
                &[(
                    "delivery_mode",
                    cua_driver_core::tool_schema::delivery_mode_schema_with(
                        "Default \"background\": refuses, before sending anything, when the \
                         pointer is over a window of an app that is not in front, since the motion \
                         would go to that app with the user's pointer. \"foreground\": send it \
                         anyway; only when nobody is using this screen or the user agreed.",
                    ),
                )],
            )
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let request = match relative_pointer::parse(&args) {
            Ok(request) => request,
            Err(refusal) => return refusal,
        };
        let here = match tokio::task::spawn_blocking(crate::input::mouse::pointer_location).await {
            Ok(Ok(here)) => here,
            Ok(Err(error)) => return relative_pointer::not_sent(&error),
            Err(error) => return ToolResult::error(format!("Task error: {error}")),
        };
        let steps = request.steps();
        let count = steps.len();
        // Every step posts at the pointer's new location, so each one must be
        // over the frontmost app or over no window.
        let points = std::iter::once((here.x, here.y))
            .chain(steps.iter().scan((here.x, here.y), |at, step| {
                *at = (at.0 + f64::from(step.dx), at.1 + f64::from(step.dy));
                Some(*at)
            }))
            .collect();
        if let Some(refusal) =
            super::desktop_pointer_guard::check(relative_pointer::TOOL_NAME, &args, points).await
        {
            return refusal;
        }
        match tokio::task::spawn_blocking(move || {
            relative_pointer::send_steps(&steps, crate::input::mouse::post_relative_motion)
        })
        .await
        {
            Ok(sent) => relative_pointer::outcome(
                request,
                count,
                ActionTransport::MacosCgEventHid,
                &args,
                sent,
            ),
            Err(error) => ToolResult::error(format!("Task error: {error}")),
        }
    }
}
