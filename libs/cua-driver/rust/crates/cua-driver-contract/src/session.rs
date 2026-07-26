// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::{
    EndSessionInput, EndSessionOutput, EscalateSessionInput, GetSessionStateInput, Platform,
    SchemaMode, SessionStateOutput, StartSessionInput, StartSessionOutput, ToolAnnotations,
    ToolContract, ToolInput, ToolOutput,
};

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub fn contracts() -> Vec<ToolContract> {
    vec![start(), escalate(), get_state(), end()]
}

fn contract<I: ToolInput, O: ToolOutput>(
    name: &str,
    description: &str,
    capabilities: &[&str],
    annotations: ToolAnnotations,
) -> ToolContract {
    assert_eq!(name, I::TOOL_NAME, "typed input is bound to the wrong tool");
    ToolContract {
        name: name.into(),
        description: description.into(),
        platforms: ALL_PLATFORMS.to_vec(),
        aliases: Vec::new(),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        annotations,
        schema_mode: SchemaMode::CanonicalRuntime,
        input_schema: I::input_schema(),
        success_output_schema: Some(O::output_schema()),
        output_validator: crate::validate_typed_output::<O>,
    }
}

fn start() -> ToolContract {
    contract::<StartSessionInput, StartSessionOutput>(
        "start_session",
        "Declare a session — a named, color-coded identity for THIS agent run. Pass a stable `session` id and choose capture_scope=auto|window|desktop; the agent cursor, capture policy, per-session config, and recording all key on it, and it follows the run across any apps/windows. The cursor's color is derived from the id, so distinct runs are visually distinct. A cursor is shown only for a declared session — call this (or pass `session` on your first action) to opt in. Idempotent: re-calling with the same id just refreshes its idle-TTL. End it with `end_session` (or let the idle-TTL reclaim it). Concurrent runs/subagents each pass their own `session` to get their own cursor.",
        &["session.lifecycle.start", "session.capture_scope"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
    )
}

fn escalate() -> ToolContract {
    contract::<EscalateSessionInput, SessionStateOutput>(
        "escalate_session",
        "Unlock the desktop phase of an auto capture-scope session after the window action ladder has been exhausted and verified. This is a one-way transition for the live session and records a bounded reason.",
        &["session.capture_scope.escalate"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
    )
}

fn get_state() -> ToolContract {
    contract::<GetSessionStateInput, SessionStateOutput>(
        "get_session_state",
        "Read the live session's capture policy and effective scope.",
        &["session.capture_scope.read"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
    )
}

fn end() -> ToolContract {
    contract::<EndSessionInput, EndSessionOutput>(
        "end_session",
        "End a session declared with `start_session`: removes its agent cursor, stops any recording it owns, and clears its per-session config. Call this when a run finishes so its cursor doesn't linger (otherwise the idle-TTL reclaims it after a period of inactivity). Idempotent.",
        &["session.lifecycle.end"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        },
    )
}
