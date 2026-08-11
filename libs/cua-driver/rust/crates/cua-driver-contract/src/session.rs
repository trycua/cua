// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::{
    CursorAction, CursorSemantics, EndSessionInput, EndSessionOutput, EscalateSessionInput,
    GetSessionInput, GetSessionStateInput, ListSessionsInput, ListSessionsOutput, Platform,
    SchemaMode, SessionOutput, SessionStateOutput, StartSessionInput, StartSessionOutput,
    ToolAnnotations, ToolContract, ToolInput, ToolOutput,
};

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub fn contracts() -> Vec<ToolContract> {
    vec![start(), escalate(), get(), list(), get_state(), end()]
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
        cursor_semantics: Some(CursorSemantics::new(CursorAction::System)),
        input_schema: I::input_schema(),
        success_output_schema: Some(O::output_schema()),
        output_validator: crate::validate_typed_output::<O>,
    }
}

fn start() -> ToolContract {
    contract::<StartSessionInput, StartSessionOutput>(
        "start_session",
        "Optionally name and start a lifecycle session for this agent run. When `session` is omitted, create or return the authenticated transport lease's implicit session. Ordinary actions create that implicit session automatically, so this tool is optional. `capture_scope` is deprecated compatibility input; new callers select window or desktop modality per action. Idempotent.",
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
        "Deprecated compatibility tool for legacy capture-scope sessions. New callers select window or desktop modality on each action. No deescalate_session tool exists.",
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
        "Deprecated compatibility alias that reads a live legacy session's capture policy. Use get_session for lifecycle state.",
        &["session.capture_scope.read"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
    )
}

fn get() -> ToolContract {
    contract::<GetSessionInput, SessionOutput>(
        "get_session",
        "Read content-free lifecycle, cursor, recording, and idle status for one session visible to this authenticated transport. Omit `session` to inspect its implicit session.",
        &["session.lifecycle.read"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
    )
}

fn list() -> ToolContract {
    contract::<ListSessionsInput, ListSessionsOutput>(
        "list_sessions",
        "List content-free lifecycle summaries attached to this authenticated transport lease. It does not enumerate other callers' sessions.",
        &["session.lifecycle.list"],
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
        "End one visible lifecycle session and run its cursor, recording, configuration, and other cleanup hooks exactly once. Omit `session` to end the authenticated transport's implicit session. Idempotent.",
        &["session.lifecycle.end"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        },
    )
}
