//! macOS `bring_to_front`.
//!
//! `bring_to_front` exists to let an agent pay a one-shot, persistent foreground
//! swap before driving a focus-proxy target — a window that only accepts input
//! while its host app genuinely holds activation. The macOS input rungs never
//! need this internally: every `CGEvent.postToPid` dispatch reaches a
//! backgrounded window, and the `dispatch:"foreground"` rung does its own
//! sub-millisecond front→act→restore flash. The one surface that flash can't
//! satisfy is a remote-desktop client (e.g. Microsoft's Windows App / RDP),
//! which re-establishes its keyboard channel with the remote host *on
//! activation* and needs the app to stay frontmost across the whole interaction
//! — not flashed and restored. `bring_to_front` is that explicit, persistent
//! activation.
//!
//! It fronts the exact window through WindowServer's
//! `SLPSSetFrontProcessWithOptions` and falls back to
//! `-[NSRunningApplication activateWithOptions:]` when that SPI is unavailable.
//! Unlike the rest of the macOS driver this DOES steal foreground — it is an
//! explicit opt-in, never called by the input ladder.

use async_trait::async_trait;
use cua_driver_core::{protocol::ToolResult, tool::{Tool, ToolDef}};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use serde_json::Value;

pub struct BringToFrontTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "bring_to_front".into(),
        description:
            "Persistently activate an app so it genuinely holds macOS foreground, \
             then leave it there. Most input does NOT need this — every macOS \
             dispatch reaches backgrounded windows, and `dispatch:\"foreground\"` \
             does its own brief front→act→restore. Reach for `bring_to_front` only \
             for a focus-proxy surface that re-arms its own input channel on \
             activation and must stay frontmost across the interaction — chiefly a \
             remote-desktop client (Microsoft Windows App / RDP), where the brief \
             flash drops keystrokes. Fronts the exact macOS window through \
             WindowServer when `window_id` is present, with app-level Cocoa \
             activation as a fallback. This DOES steal foreground — explicit \
             opt-in, never used by the input ladder."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "pid": { "type": "integer" },
                "window_id": { "type": "integer" }
            },
            "additionalProperties": false,
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for BringToFrontTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(Value::as_i64) {
            Some(p) => match libc::pid_t::try_from(p) {
                Ok(pid) => pid,
                Err(_) => {
                    return ToolResult::error(format!(
                        "bring_to_front: `pid` {p} is out of range for a process identifier."
                    ))
                    .with_structured(serde_json::json!({
                        "code": "bring_to_front_pid_out_of_range",
                        "pid": p,
                    }));
                }
            },
            None => return ToolResult::error("Missing required integer field: pid".to_string()),
        };
        let window_id = args.get("window_id").and_then(Value::as_i64);

        let skylight_activated = window_id
            .and_then(|value| u32::try_from(value).ok())
            .is_some_and(|window_id| {
                crate::input::skylight::set_front_process_persistently(pid, window_id)
            });

        // Cocoa activation is the public fallback. On recent macOS it may
        // accept a request from a background process without actually changing
        // WindowServer ownership, which is why the exact-window SkyLight path
        // runs first.
        let activation = skylight_activated.then_some(true).or_else(|| unsafe {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid).map(|app| {
                app.activateWithOptions(
                    NSApplicationActivationOptions::NSApplicationActivateAllWindows,
                )
            })
        });

        let activated = match activation {
            None => {
                return ToolResult::error(format!(
                    "bring_to_front: no running application for pid {pid} \
                     (process not found or already exited)."
                ))
                .with_structured(serde_json::json!({
                    "code": "bring_to_front_pid_not_found",
                    "pid": pid,
                }));
            }
            Some(activated) => activated,
        };

        if !activated {
            return ToolResult::error(format!(
                "bring_to_front: macOS rejected activation for pid {pid} \
                 (activateWithOptions returned NO — the app may be hidden or \
                 terminating, or the system denied the foreground swap)."
            ))
            .with_structured(serde_json::json!({
                "code": "bring_to_front_activation_rejected",
                "pid": pid,
                "activated": false,
            }));
        }

        ToolResult::text(format!("Brought pid {pid} to the foreground."))
            .with_structured(serde_json::json!({
                "pid": pid,
                "window_id": window_id,
                "activated": true,
                "path": if skylight_activated { "skylight" } else { "cocoa" },
            }))
    }
}
