// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::{schema, Platform, ToolAnnotations, ToolContract};
use serde_json::json;

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub fn contracts() -> Vec<ToolContract> {
    vec![start(), escalate(), get_state(), end()]
}

fn contract(
    name: &str,
    description: &str,
    capabilities: &[&str],
    annotations: ToolAnnotations,
    input_schema: serde_json::Value,
    success_output_schema: serde_json::Value,
) -> ToolContract {
    ToolContract {
        name: name.into(),
        description: description.into(),
        platforms: ALL_PLATFORMS.to_vec(),
        aliases: Vec::new(),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        annotations,
        input_schema,
        success_output_schema: Some(success_output_schema),
    }
}

fn start() -> ToolContract {
    contract(
        "start_session",
        "Declare a session — a named, color-coded identity for THIS agent run. Pass a stable `session` id and choose capture_scope=auto|window|desktop; the agent cursor, capture policy, per-session config, and recording all key on it, and it follows the run across any apps/windows. The cursor's color is derived from the id, so distinct runs are visually distinct. A cursor is shown only for a declared session — call this (or pass `session` on your first action) to opt in. Idempotent: re-calling with the same id just refreshes its idle-TTL. End it with `end_session` (or let the idle-TTL reclaim it). Concurrent runs/subagents each pass their own `session` to get their own cursor.",
        &["session.lifecycle.start", "session.capture_scope"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema::object(
            &["session"],
            [
                (
                    "session",
                    schema::string(Some("Stable session id for this run (e.g. \"research-run-1\").")),
                ),
                (
                    "capture_scope",
                    {
                        let mut value = schema::string_enum(
                            &["auto", "window", "desktop"],
                            Some("Per-session perception/action modality. auto starts window-only and requires explicit escalation before desktop tools; window and desktop are strict. Immutable for the live session."),
                        );
                        value["default"] = json!("auto");
                        value
                    },
                ),
            ],
        ),
        schema::session_state_output([
            ("active", json!({ "type": "boolean" })),
            ("revived", json!({ "type": "boolean" })),
        ]),
    )
}

fn escalate() -> ToolContract {
    contract(
        "escalate_session",
        "Unlock the desktop phase of an auto capture-scope session after the window action ladder has been exhausted and verified. This is a one-way transition for the live session and records a bounded reason.",
        &["session.capture_scope.escalate"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        schema::object(
            &["session", "reason"],
            [
                ("session", schema::string(None)),
                (
                    "reason",
                    schema::string_enum(
                        &[
                            "ax_tree_pixel_mismatch",
                            "background_delivery_failed",
                            "foreground_ineffective",
                            "no_window_target",
                            "other",
                        ],
                        None,
                    ),
                ),
                (
                    "detail",
                    {
                        let mut value = schema::string(Some(
                            "Optional bounded diagnostic detail. Never use secrets or page content.",
                        ));
                        value["maxLength"] = json!(200);
                        value
                    },
                ),
            ],
        ),
        schema::session_state_output([]),
    )
}

fn get_state() -> ToolContract {
    contract(
        "get_session_state",
        "Read the live session's capture policy and effective scope.",
        &["session.capture_scope.read"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema::object(&["session"], [("session", schema::string(None))]),
        schema::session_state_output([]),
    )
}

fn end() -> ToolContract {
    contract(
        "end_session",
        "End a session declared with `start_session`: removes its agent cursor, stops any recording it owns, and clears its per-session config. Call this when a run finishes so its cursor doesn't linger (otherwise the idle-TTL reclaims it after a period of inactivity). Idempotent.",
        &["session.lifecycle.end"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        },
        schema::object(
            &["session"],
            [(
                "session",
                schema::string(Some("The session id to end.")),
            )],
        ),
        json!({
            "type": "object",
            "required": ["session", "active"],
            "properties": {
                "session": { "type": "string" },
                "active": { "const": false }
            },
            "additionalProperties": true
        }),
    )
}
