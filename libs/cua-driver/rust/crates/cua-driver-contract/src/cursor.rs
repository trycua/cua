// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Transport-neutral semantic contract for the agent cursor.
//!
//! Tool implementations emit these semantics as best-effort visual telemetry.
//! They never affect authorization, dispatch, input delivery, or tool results.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    uniffi::Enum,
)]
#[serde(rename_all = "snake_case")]
pub enum CursorAction {
    #[default]
    Idle,
    Observe,
    Click,
    Drag,
    Scroll,
    Text,
    Key,
    Navigate,
    App,
    Transfer,
    Record,
    System,
}

impl CursorAction {
    pub const ALL: [Self; 12] = [
        Self::Idle,
        Self::Observe,
        Self::Click,
        Self::Drag,
        Self::Scroll,
        Self::Text,
        Self::Key,
        Self::Navigate,
        Self::App,
        Self::Transfer,
        Self::Record,
        Self::System,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Observe => "observe",
            Self::Click => "click",
            Self::Drag => "drag",
            Self::Scroll => "scroll",
            Self::Text => "text",
            Self::Key => "key",
            Self::Navigate => "navigate",
            Self::App => "app",
            Self::Transfer => "transfer",
            Self::Record => "record",
            Self::System => "system",
        }
    }

    pub const fn playback(self) -> CursorPlayback {
        match self {
            Self::Idle => CursorPlayback::Resting,
            Self::Observe | Self::Scroll | Self::Transfer | Self::Record => CursorPlayback::Loop,
            Self::Drag | Self::Text => CursorPlayback::Held,
            Self::Click | Self::Key | Self::Navigate | Self::App | Self::System => {
                CursorPlayback::OneShot
            }
        }
    }

    pub const fn duration_secs(self) -> f64 {
        match self {
            Self::Idle => 4.0,
            Self::Click => 0.67,
            Self::Observe
            | Self::Drag
            | Self::Scroll
            | Self::Text
            | Self::Key
            | Self::Navigate
            | Self::App
            | Self::Transfer
            | Self::Record
            | Self::System => 1.6,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CursorPlayback {
    Resting,
    OneShot,
    Held,
    Loop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CursorDelivery {
    Background,
    Foreground,
}

impl CursorDelivery {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Foreground => "foreground",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CursorTarget {
    Ax,
    Pixel,
    Browser,
    Desktop,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    PartialEq,
    Eq,
    uniffi::Enum,
)]
#[serde(rename_all = "snake_case")]
pub enum CursorReducedMotion {
    #[default]
    Auto,
    On,
    Off,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, uniffi::Record,
)]
pub struct CursorThemeSelection {
    pub theme_id: String,
    #[serde(default)]
    pub reduced_motion: CursorReducedMotion,
}

impl CursorTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ax => "ax",
            Self::Pixel => "pixel",
            Self::Browser => "browser",
            Self::Desktop => "desktop",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Hash)]
pub struct CursorSemantics {
    pub action: CursorAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<CursorDelivery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<CursorTarget>,
}

impl CursorSemantics {
    pub const fn new(action: CursorAction) -> Self {
        Self {
            action,
            delivery: None,
            target: None,
        }
    }
}

/// Resolve a public tool call into the small visual vocabulary.
///
/// The mapping deliberately uses the resolved route in `args` where the
/// runtime exposes it. Unknown and cursor-management tools return `None`
/// rather than inventing a misleading cue.
pub fn classify_cursor_semantics(name: &str, args: &Value) -> Option<CursorSemantics> {
    let action = match name {
        "get_desktop_state"
        | "get_window_state"
        | "get_accessibility_tree"
        | "get_ax_tree"
        | "get_screen_size"
        | "get_cursor_position"
        | "screenshot"
        | "zoom"
        | "browser_get_state"
        | "browser_snapshot"
        | "browser_screenshot"
        | "browser_tabs"
        | "browser_get_tabs" => CursorAction::Observe,

        "click"
        | "double_click"
        | "right_click"
        | "browser_click"
        | "browser_double_click"
        | "browser_right_click" => CursorAction::Click,

        "drag" | "browser_drag" => CursorAction::Drag,
        "scroll" | "browser_scroll" => CursorAction::Scroll,
        "type_text" | "set_value" | "browser_type" | "browser_fill" => CursorAction::Text,
        "press_key" | "hotkey" | "browser_press_key" | "browser_hotkey" => CursorAction::Key,

        "move_cursor" | "browser_navigate" | "browser_go_back" | "browser_go_forward"
        | "browser_reload" | "browser_new_tab" | "browser_close_tab" | "browser_select_tab" => {
            CursorAction::Navigate
        }

        "launch_app" | "activate_app" | "bring_to_front" | "set_window_frame" | "invoke_menu"
        | "list_apps" | "list_windows" | "kill_app" => CursorAction::App,

        "upload_file" | "download_file" | "copy_file" | "move_file" => CursorAction::Transfer,

        "start_recording" | "stop_recording" | "get_recording_state" | "replay_trajectory" => {
            CursorAction::Record
        }

        "start_session" | "escalate_session" | "get_session_state" | "end_session"
        | "check_permissions" | "get_config" | "set_config" | "health_report"
        | "browser_prepare" | "browser_close" | "browser_release" | "browser_activate"
        | "install_ffmpeg" | "check_update" | "update" => CursorAction::System,

        "set_agent_cursor_enabled"
        | "set_agent_cursor_motion"
        | "set_agent_cursor_theme"
        | "get_agent_cursor_state" => return None,
        _ => return None,
    };

    let target = if name.starts_with("browser_") {
        Some(CursorTarget::Browser)
    } else if matches!(name, "set_window_frame" | "invoke_menu") {
        Some(CursorTarget::Desktop)
    } else if args
        .get("element_index")
        .or_else(|| args.get("element_token"))
        .is_some_and(|value| !value.is_null())
    {
        Some(CursorTarget::Ax)
    } else if args.get("x").is_some() || args.get("y").is_some() {
        Some(CursorTarget::Pixel)
    } else {
        Some(CursorTarget::Desktop)
    };

    let delivery = args
        .get("delivery_mode")
        .or_else(|| args.get("delivery"))
        .and_then(Value::as_str)
        .and_then(|value| match value {
            "background" | "background_ax" | "background_pixel" => Some(CursorDelivery::Background),
            "foreground" => Some(CursorDelivery::Foreground),
            _ => None,
        })
        .or_else(|| {
            args.get("background")
                .and_then(Value::as_bool)
                .map(|background| {
                    if background {
                        CursorDelivery::Background
                    } else {
                        CursorDelivery::Foreground
                    }
                })
        });

    Some(CursorSemantics {
        action,
        delivery,
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_vocabulary_is_complete_and_unique() {
        let mut names = std::collections::BTreeSet::new();
        for action in CursorAction::ALL {
            assert!(names.insert(action.as_str()));
        }
        assert_eq!(names.len(), 12);
    }

    #[test]
    fn classifier_uses_resolved_target_and_delivery() {
        assert_eq!(
            classify_cursor_semantics(
                "click",
                &json!({"element_index":"ax:1","delivery_mode":"background"})
            ),
            Some(CursorSemantics {
                action: CursorAction::Click,
                delivery: Some(CursorDelivery::Background),
                target: Some(CursorTarget::Ax),
            })
        );
        assert_eq!(
            classify_cursor_semantics("browser_click", &json!({"x":12,"y":14})),
            Some(CursorSemantics {
                action: CursorAction::Click,
                delivery: None,
                target: Some(CursorTarget::Browser),
            })
        );
        assert_eq!(
            classify_cursor_semantics(
                "type_text",
                &json!({"element_token":"opaque", "delivery_mode":"foreground"})
            ),
            Some(CursorSemantics {
                action: CursorAction::Text,
                delivery: Some(CursorDelivery::Foreground),
                target: Some(CursorTarget::Ax),
            })
        );
        assert_eq!(
            classify_cursor_semantics(
                "set_window_frame",
                &json!({"pid": 7, "window_id": 9, "x": 0, "y": 0})
            ),
            Some(CursorSemantics {
                action: CursorAction::App,
                delivery: None,
                target: Some(CursorTarget::Desktop),
            })
        );
    }

    #[test]
    fn cursor_configuration_does_not_animate_itself() {
        assert_eq!(
            classify_cursor_semantics("set_agent_cursor_theme", &json!({})),
            None
        );
    }
}
