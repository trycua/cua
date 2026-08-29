//! Platform-neutral human input observations.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    pub(crate) fn is_command(self) -> bool {
        self.ctrl || self.alt || self.meta
    }
}

/// An observed human action. These are observations, not executable driver
/// commands. Text content is never retained.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HumanEvent {
    Click {
        x: f64,
        y: f64,
        button: Button,
        t_ms: u64,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        t_ms: u64,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
        button: Button,
        t_ms: u64,
    },
    Key {
        key: String,
        modifiers: Modifiers,
        t_ms: u64,
    },
    Text {
        t_ms: u64,
    },
}

impl HumanEvent {
    pub fn t_ms(&self) -> u64 {
        match self {
            Self::Click { t_ms, .. }
            | Self::Scroll { t_ms, .. }
            | Self::Drag { t_ms, .. }
            | Self::Key { t_ms, .. }
            | Self::Text { t_ms } => *t_ms,
        }
    }

    /// Convert an observation to the demonstration artifact schema. The action
    /// names intentionally do not overlap executable cua-driver tools.
    pub fn to_record(&self) -> (&'static str, serde_json::Value) {
        use serde_json::json;
        match self {
            Self::Click { x, y, button, .. } => {
                ("human_click", json!({ "x": x, "y": y, "button": button }))
            }
            Self::Scroll { x, y, dx, dy, .. } => (
                "human_scroll",
                json!({ "x": x, "y": y, "dx": dx, "dy": dy }),
            ),
            Self::Drag {
                from, to, button, ..
            } => (
                "human_drag",
                json!({
                    "start_x": from.0,
                    "start_y": from.1,
                    "end_x": to.0,
                    "end_y": to.1,
                    "button": button,
                }),
            ),
            Self::Key { key, modifiers, .. } => {
                ("human_key", json!({ "key": key, "modifiers": modifiers }))
            }
            Self::Text { .. } => (
                "human_text",
                json!({ "redacted": "text entered (redacted)" }),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_modifiers_exclude_shift_only() {
        assert!(!Modifiers {
            shift: true,
            ..Default::default()
        }
        .is_command());
        assert!(Modifiers {
            ctrl: true,
            ..Default::default()
        }
        .is_command());
    }

    #[test]
    fn observations_do_not_use_driver_tool_names() {
        let event = HumanEvent::Click {
            x: 1.0,
            y: 2.0,
            button: Button::Right,
            t_ms: 0,
        };
        let (action, arguments) = event.to_record();
        assert_eq!(action, "human_click");
        assert_eq!(arguments["button"], "right");
    }

    #[test]
    fn text_records_no_content_or_length() {
        let (_, arguments) = HumanEvent::Text { t_ms: 0 }.to_record();
        assert_eq!(arguments["redacted"], "text entered (redacted)");
        assert!(arguments.get("text").is_none());
        assert!(arguments.get("char_count").is_none());
    }
}
