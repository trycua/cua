// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::{
    CursorAction, CursorSemantics, EndSessionInput, EndSessionOutput, GetSessionInput,
    ListSessionsInput, ListSessionsOutput, Platform, SchemaMode, SessionOutput, StartSessionInput,
    StartSessionOutput, ToolAnnotations, ToolContract, ToolInput, ToolOutput,
};

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub fn contracts() -> Vec<ToolContract> {
    vec![start(), get(), list(), end()]
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
        error_output_schema: None,
        output_validator: crate::validate_typed_output::<O>,
    }
}

fn start() -> ToolContract {
    contract::<StartSessionInput, StartSessionOutput>(
        "start_session",
        "Optionally create or return a lifecycle session before acting. For multi-call work, prefer a short public `session` label and repeat it on every call that accepts it; an omitted value uses the authenticated transport lease's implicit session instead. This tool is optional because an ordinary action can create or reuse a named run directly. Use it to set the initial cursor theme before acting or to revive a public name after it has ended; ordinary actions never revive ended names. `capture_scope` is deprecated compatibility input; new callers select window or desktop modality per action. Idempotent.",
        &["session.lifecycle.start", "session.capture_scope"],
        ToolAnnotations {
            read_only: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_description_explains_direct_naming_and_explicit_revival() {
        let description = start().description;
        assert!(description.contains("prefer a short public `session` label"));
        assert!(description.contains("repeat it on every call that accepts it"));
        assert!(description.contains("omitted value uses the authenticated transport"));
        assert!(description.contains("revive a public name after it has ended"));
        assert!(description.contains("ordinary actions never revive ended names"));
    }
}
