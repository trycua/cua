// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::{
    GetAgentCursorStateInput, GetAgentCursorStateOutput, Platform, SchemaMode,
    SetAgentCursorEnabledInput, SetAgentCursorEnabledOutput, SetAgentCursorMotionInput,
    SetAgentCursorMotionOutput, SetAgentCursorThemeInput, SetAgentCursorThemeOutput,
    ToolAnnotations, ToolContract, ToolInput, ToolOutput,
};

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub fn contracts() -> Vec<ToolContract> {
    vec![
        contract::<SetAgentCursorEnabledInput, SetAgentCursorEnabledOutput>(
            "set_agent_cursor_enabled",
            "Show or hide the agent cursor owned by a session.",
            &["agent_cursor.set_enabled"],
            false,
        ),
        contract::<SetAgentCursorMotionInput, SetAgentCursorMotionOutput>(
            "set_agent_cursor_motion",
            "Configure only movement physics and visibility timing for a session cursor.",
            &["agent_cursor.set_motion"],
            false,
        ),
        contract::<SetAgentCursorThemeInput, SetAgentCursorThemeOutput>(
            "set_agent_cursor_theme",
            "Select an already-installed cursor theme for a session.",
            &["agent_cursor.set_theme"],
            false,
        ),
        contract::<GetAgentCursorStateInput, GetAgentCursorStateOutput>(
            "get_agent_cursor_state",
            "Return the session cursor's theme, semantic playback, position, visibility, and motion.",
            &["agent_cursor.state"],
            true,
        ),
    ]
}

fn contract<I: ToolInput, O: ToolOutput>(
    name: &str,
    description: &str,
    capabilities: &[&str],
    read_only: bool,
) -> ToolContract {
    assert_eq!(name, I::TOOL_NAME, "typed input is bound to the wrong tool");
    ToolContract {
        name: name.into(),
        description: description.into(),
        platforms: ALL_PLATFORMS.to_vec(),
        aliases: Vec::new(),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        annotations: ToolAnnotations {
            read_only,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema_mode: SchemaMode::CanonicalRuntime,
        cursor_semantics: None,
        input_schema: I::input_schema(),
        success_output_schema: Some(O::output_schema()),
        output_validator: crate::validate_typed_output::<O>,
    }
}
