// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The composer while an agent runs: the send button becomes **Queue** (the
//! message waits behind the step in progress) or **Steer** (it interrupts the
//! step and goes now). The daemon's `agent_message` already does both
//! (`force` interrupts first); this module owns the words and the rule for
//! which one is the default, so every shell offers the same two choices.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SendModeInput {
    /// An agent in the room is working.
    pub running: bool,
    /// The box has text.
    pub has_text: bool,
    /// Messages already waiting behind the running step.
    pub queued: u32,
    /// The Space's daemon can interrupt (`agent_message.force`).
    pub can_steer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    /// `send`, `queue`, `steer`.
    pub mode: String,
    pub label: String,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendModeView {
    /// What a tap on the button does.
    pub primary: Choice,
    /// What the menu on the button adds while an agent runs.
    pub alternate: Option<Choice>,
    /// "2 messages queued", while some wait.
    pub queued_line: Option<String>,
    /// The spoken label of the button.
    pub accessibility_label: String,
    /// The button is the Stop button (an agent runs and the box is empty).
    pub stop: bool,
    pub stop_label: String,
}

fn queue() -> Choice {
    Choice {
        mode: "queue".into(),
        label: "Queue".into(),
        hint: "Sends after the current step finishes.".into(),
    }
}

fn steer() -> Choice {
    Choice {
        mode: "steer".into(),
        label: "Steer".into(),
        hint: "Interrupts the current step and sends this now.".into(),
    }
}

pub fn send_mode(i: &SendModeInput) -> SendModeView {
    let queued_line = (i.queued > 0).then(|| {
        if i.queued == 1 {
            "1 message queued".to_string()
        } else {
            format!("{} messages queued", i.queued)
        }
    });
    if !i.running {
        return SendModeView {
            primary: Choice {
                mode: "send".into(),
                label: "Send".into(),
                hint: "Sends this message.".into(),
            },
            alternate: None,
            queued_line,
            accessibility_label: if i.has_text {
                "Send".into()
            } else {
                "Send, type a message first".into()
            },
            stop: false,
            stop_label: "Stop".into(),
        };
    }
    SendModeView {
        accessibility_label: if i.has_text {
            "Queue".into()
        } else {
            "Stop".into()
        },
        primary: queue(),
        alternate: i.can_steer.then(steer),
        queued_line,
        stop: !i.has_text,
        stop_label: "Stop".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_plain_send() {
        let v = send_mode(&SendModeInput {
            running: false,
            has_text: true,
            ..Default::default()
        });
        assert_eq!(v.primary.label, "Send");
        assert_eq!(v.primary.mode, "send");
        assert!(v.alternate.is_none() && !v.stop);
        assert_eq!(v.accessibility_label, "Send");
        let empty = send_mode(&SendModeInput::default());
        assert_eq!(empty.accessibility_label, "Send, type a message first");
    }

    #[test]
    fn while_running_the_default_is_queue_and_steer_is_the_other_choice() {
        let v = send_mode(&SendModeInput {
            running: true,
            has_text: true,
            queued: 0,
            can_steer: true,
        });
        assert_eq!(v.primary.mode, "queue");
        assert_eq!(v.primary.label, "Queue");
        assert_eq!(v.alternate.as_ref().unwrap().label, "Steer");
        assert!(!v.stop);
        let none = send_mode(&SendModeInput {
            running: true,
            has_text: true,
            queued: 0,
            can_steer: false,
        });
        assert!(none.alternate.is_none());
    }

    #[test]
    fn an_empty_box_while_running_is_stop_and_queued_messages_are_counted() {
        let v = send_mode(&SendModeInput {
            running: true,
            has_text: false,
            queued: 2,
            can_steer: true,
        });
        assert!(v.stop);
        assert_eq!(v.accessibility_label, "Stop");
        assert_eq!(v.queued_line.as_deref(), Some("2 messages queued"));
        let one = send_mode(&SendModeInput {
            running: true,
            has_text: true,
            queued: 1,
            can_steer: true,
        });
        assert_eq!(one.queued_line.as_deref(), Some("1 message queued"));
    }
}
