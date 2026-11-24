// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::{
    GetAgentCursorStateInput, GetAgentCursorStateOutput, Platform, SchemaMode,
    SetAgentCursorBadgeInput, SetAgentCursorBadgeOutput, SetAgentCursorEnabledInput,
    SetAgentCursorEnabledOutput, SetAgentCursorMotionInput, SetAgentCursorMotionOutput,
    SetAgentCursorThemeInput, SetAgentCursorThemeOutput, ToolAnnotations, ToolContract, ToolInput,
    ToolOutput,
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
        contract::<SetAgentCursorBadgeInput, SetAgentCursorBadgeOutput>(
            "set_agent_cursor_badge",
            "Show or hide the session label text beneath a session's cursor.",
            &["agent_cursor.set_badge"],
            false,
        ),
        contract::<GetAgentCursorStateInput, GetAgentCursorStateOutput>(
            "get_agent_cursor_state",
            "Return the session cursor's theme, semantic playback, position, visibility, motion, and badge settings.",
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
    use crate::{
        CursorBadgeOutput, CursorBadgeSelection, CursorEffectSetting, CursorMotionStyle,
        GetAgentCursorStateOutput, SetAgentCursorBadgeInput, SetAgentCursorMotionInput,
        StartSessionInput, ToolInput,
    };
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
    fn session_label_visibility_round_trips_and_is_optional_on_older_daemons() {
        let started: StartSessionInput = serde_json::from_value(json!({
            "session": "s",
            "cursor_badge": {"show_session_label": false}
        }))
        .unwrap();
        assert_eq!(
            started.cursor_badge,
            Some(CursorBadgeSelection {
                show_session_label: Some(false)
            })
        );
        assert_eq!(
            serde_json::to_value(&started).unwrap()["cursor_badge"],
            json!({"show_session_label": false})
        );
        let omitted: StartSessionInput = serde_json::from_value(json!({"session": "s"})).unwrap();
        assert!(serde_json::to_value(&omitted)
            .unwrap()
            .get("cursor_badge")
            .is_none());

        let schema = SetAgentCursorBadgeInput::input_schema();
        assert_eq!(schema["required"], json!(["session", "show_session_label"]));
        assert_eq!(
            schema["properties"]["show_session_label"]["type"],
            "boolean"
        );
        assert!(
            serde_json::from_value::<SetAgentCursorBadgeInput>(json!({"session": "s"})).is_err()
        );

        let mut state = json!({
            "session": "s",
            "enabled": true,
            "position": null,
            "theme": {
                "id": "cua.default",
                "version": "1",
                "profile": "p",
                "reduced_motion": "auto",
                "fallback": null
            },
            "visual_state": {
                "requested_action": "idle",
                "resolved_action": "idle",
                "modifiers": [],
                "phase": "idle",
                "frame": 0,
                "preempted_count": 0
            },
            "motion": {
                "start_handle": 0.3,
                "end_handle": 0.3,
                "arc_size": 0.25,
                "arc_flow": 0.0,
                "spring": 0.72,
                "glide_duration_ms": 0.0,
                "dwell_after_click_ms": 80.0,
                "idle_hide_ms": 15000.0,
                "turn_radius": 80.0
            }
        });
        let older: GetAgentCursorStateOutput = serde_json::from_value(state.clone()).unwrap();
        assert_eq!(older.badge, None);
        state["badge"] = json!({"show_session_label": false});
        let current: GetAgentCursorStateOutput = serde_json::from_value(state).unwrap();
        assert_eq!(
            current.badge,
            Some(CursorBadgeOutput {
                show_session_label: false
            })
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
