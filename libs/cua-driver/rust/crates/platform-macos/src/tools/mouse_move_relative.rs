// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use cua_driver_core::{
    action_record::ActionTransport,
    protocol::{Content, ToolResult},
    relative_pointer,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

use super::desktop_pointer_guard::PointerGuard;

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
        let steps = request.steps();
        let count = steps.len();
        let guard_args = args.clone();
        let sent = tokio::task::spawn_blocking(move || {
            // Each event posts at the pointer's location plus its delta, so
            // that location is checked right before the event goes out.
            let guard = PointerGuard::read(relative_pointer::TOOL_NAME, &guard_args);
            let mut refusal = None;
            let sent = relative_pointer::send_steps(&steps, |dx, dy| {
                crate::input::mouse::post_relative_motion(dx, dy, |at| {
                    match guard
                        .as_ref()
                        .and_then(|guard| guard.refusal_at((at.x, at.y)))
                    {
                        Some(refused) => {
                            let reason = refused
                                .content
                                .iter()
                                .find_map(|content| match content {
                                    Content::Text { text, .. } => Some(text.clone()),
                                    _ => None,
                                })
                                .unwrap_or_default();
                            refusal = Some(refused);
                            anyhow::bail!("{reason}")
                        }
                        None => Ok(()),
                    }
                })
            });
            (sent, refusal)
        })
        .await;
        match sent {
            // Refused before the first event: the guard's own refusal.
            Ok((Err(incomplete), Some(refusal))) if incomplete.sent_steps == 0 => refusal,
            Ok((sent, _)) => relative_pointer::outcome(
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
