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
            "Configure the movement style, timing, effects and visibility timing for a session cursor.",
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
        error_output_schema: None,
        output_validator: crate::validate_typed_output::<O>,
    }
}

#[cfg(test)]
mod tests {
    use crate::{CursorEffectSetting, CursorMotionStyle, SetAgentCursorMotionInput, StartSessionInput, ToolInput};
    use serde_json::json;

    #[test]
    fn motion_style_schema_lists_public_names_and_accepts_lab_ids() {
        let schema = SetAgentCursorMotionInput::input_schema();
        let names: Vec<&str> = schema["properties"]["style"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect();
        assert_eq!(names, CursorMotionStyle::ALL.map(CursorMotionStyle::as_str));
        for (alias, style) in [
            ("dc-signature-arc", CursorMotionStyle::SignatureArc),
            ("dc-spring-settle", CursorMotionStyle::SpringSettle),
            ("dc-magnetic", CursorMotionStyle::Magnetic),
            ("dc-comet-swoop", CursorMotionStyle::CometSwoop),
            ("adaptive-auto", CursorMotionStyle::Adaptive),
            ("dubins-glide", CursorMotionStyle::Classic),
        ] {
            let input: SetAgentCursorMotionInput =
                serde_json::from_value(json!({"session": "s", "style": alias})).unwrap();
            assert_eq!(input.style, Some(style));
        }
        assert!(serde_json::from_value::<SetAgentCursorMotionInput>(
            json!({"session": "s", "effects": {"sparkle": true}})
        )
        .is_err());
    }

    #[test]
    fn explicit_null_effect_reset_survives_typed_tool_inputs() {
        let set: SetAgentCursorMotionInput = serde_json::from_value(json!({
            "session": "s",
            "effects": {"trail": null, "glow": true}
        }))
        .unwrap();
        let effects = set.effects.as_ref().expect("effects object");
        assert_eq!(effects.trail, Some(CursorEffectSetting::Default));
        assert_eq!(effects.glow, Some(CursorEffectSetting::On));
        assert_eq!(effects.magnet, None);
        assert_eq!(
            serde_json::to_value(&set).unwrap()["effects"],
            json!({"trail": "default", "glow": "on"})
        );

        let started: StartSessionInput = serde_json::from_value(json!({
            "session": "s",
            "cursor_motion": {"effects": {"trail": null}}
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&started).unwrap()["cursor_motion"]["effects"],
            json!({"trail": "default"})
        );
    }

    #[test]
    fn unset_effects_are_omitted_on_the_wire() {
        let input: SetAgentCursorMotionInput =
            serde_json::from_value(json!({"session": "s", "effects": {"trail": false}})).unwrap();
        assert_eq!(
            serde_json::to_value(&input).unwrap()["effects"],
            json!({"trail": "off"})
        );
    }
}
