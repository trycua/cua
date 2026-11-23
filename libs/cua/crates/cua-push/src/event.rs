// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What happened, and the alert it becomes.

use serde::{Deserialize, Serialize};

/// The longest agent name an alert carries, in characters.
pub const AGENT_MAX: usize = 32;
/// The longest id (Space, event) accepted.
pub const ID_MAX: usize = 200;

/// The notification category with Allow and Deny buttons (the phone app
/// registers it), for an approval.
pub const CATEGORY_APPROVAL: &str = "CUA_APPROVAL";
/// The category of every other alert: it opens the Space.
pub const CATEGORY_DONE: &str = "CUA_DONE";

/// Why the person is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Something waits for an answer (an approval).
    ApprovalNeeded,
    /// A task ended well.
    TaskFinished,
    /// A task or a run failed.
    TaskFailed,
    /// An agent cannot go on until the person acts (connect an account).
    AgentBlocked,
}

impl EventKind {
    pub const ALL: [EventKind; 4] = [
        EventKind::ApprovalNeeded,
        EventKind::TaskFinished,
        EventKind::TaskFailed,
        EventKind::AgentBlocked,
    ];

    /// The word the phone's `kind` carries: `approval`, `done`, `error` or
    /// `blocked`.
    pub fn wire_kind(self) -> &'static str {
        match self {
            EventKind::ApprovalNeeded => "approval",
            EventKind::TaskFinished => "done",
            EventKind::TaskFailed => "error",
            EventKind::AgentBlocked => "blocked",
        }
    }

    /// The word the event and the preferences use.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::ApprovalNeeded => "approval_needed",
            EventKind::TaskFinished => "task_finished",
            EventKind::TaskFailed => "task_failed",
            EventKind::AgentBlocked => "agent_blocked",
        }
    }

    /// An approval is time sensitive; the rest can wait a moment.
    pub fn urgent(self) -> bool {
        matches!(self, EventKind::ApprovalNeeded | EventKind::AgentBlocked)
    }
}

/// What the daemon reports. Text fields beyond `agent` are ignored on
/// purpose: the alert is built here from fixed phrases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushEvent {
    /// A unique id for this event (the daemon's notification id): the same
    /// id is sent once however often it is reported.
    pub id: String,
    pub kind: EventKind,
    /// The Space it is about, as the app opens it (`relay:<machine>`).
    #[serde(default)]
    pub space: Option<String>,
    /// The thread entry it is about (an approval's number).
    #[serde(default)]
    pub entry: Option<u64>,
    /// The agent's name, when one is involved.
    #[serde(default)]
    pub agent: Option<String>,
    /// For an approval: what an answer may choose (`once`, `space`), as
    /// the daemon decided. A phone shows only these.
    #[serde(default)]
    pub scope_choices: Vec<String>,
    /// Unix milliseconds.
    #[serde(default)]
    pub at_ms: u64,
}

/// Why an event is refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct Invalid(pub String);

impl PushEvent {
    /// Checks the fields a client controls.
    pub fn check(&self) -> Result<(), Invalid> {
        if self.id.trim().is_empty() || self.id.chars().count() > ID_MAX {
            return Err(Invalid("event id: 1 to 200 characters".into()));
        }
        if let Some(s) = &self.space
            && (s.trim().is_empty()
                || s.chars().count() > ID_MAX
                || s.chars().any(char::is_control))
        {
            return Err(Invalid(
                "space: 1 to 200 characters, no control characters".into(),
            ));
        }
        if self.scope_choices.len() > 2
            || self
                .scope_choices
                .iter()
                .any(|c| c != "once" && c != "space")
        {
            return Err(Invalid("scope_choices: once, space".into()));
        }
        Ok(())
    }

    /// The alert for this event.
    pub fn payload(&self) -> Payload {
        let agent = self
            .agent
            .as_deref()
            .map(clean_name)
            .filter(|a| !a.is_empty());
        let who = agent.clone().unwrap_or_else(|| "An agent".to_string());
        let (title, body) = match self.kind {
            EventKind::ApprovalNeeded => (format!("{who} needs you"), "Open to allow or decline."),
            EventKind::TaskFinished => (format!("{who} finished"), "Open to see the result."),
            EventKind::TaskFailed => (format!("{who} stopped"), "Open to see what happened."),
            EventKind::AgentBlocked => (format!("{who} is blocked"), "Open to connect it."),
        };
        Payload {
            title,
            body: body.to_string(),
            category: if self.kind == EventKind::ApprovalNeeded {
                CATEGORY_APPROVAL
            } else {
                CATEGORY_DONE
            }
            .to_string(),
            thread: self.space.clone(),
            kind: self.kind,
            space: self.space.clone(),
            entry: self.entry,
            scope_choices: if self.kind == EventKind::ApprovalNeeded {
                self.scope_choices.clone()
            } else {
                vec![]
            },
            urgent: self.kind.urgent(),
            collapse: collapse_id(&self.id),
        }
    }
}

/// An agent name as an alert shows it: one line, printable, short.
pub fn clean_name(name: &str) -> String {
    let flat: String = name
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= AGENT_MAX {
        return flat;
    }
    let mut cut: String = flat.chars().take(AGENT_MAX - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('\u{2026}');
    cut
}

/// APNs collapse ids are at most 64 bytes: a hash of the event id.
fn collapse_id(id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    format!("cua-{:016x}", h.finish())
}

/// What a provider sends: the visible alert and the ids that open the entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Payload {
    pub title: String,
    pub body: String,
    /// The notification category: [`CATEGORY_APPROVAL`] (Allow and Deny
    /// buttons) or [`CATEGORY_DONE`].
    pub category: String,
    /// Groups the Space's alerts together.
    pub thread: Option<String>,
    pub kind: EventKind,
    pub space: Option<String>,
    pub entry: Option<u64>,
    pub scope_choices: Vec<String>,
    /// Deliver at once (time sensitive).
    pub urgent: bool,
    /// Replaces an earlier alert for the same event.
    pub collapse: String,
}

impl Payload {
    /// The APNs body: `aps` for the system, and what the app routes the
    /// tap from, as the top-level keys the app reads from the
    /// notification's data: `v`, `kind` (`approval`, `done`, `error` or
    /// `blocked`), `space` (`relay:<machine>`), `entry` (the thread entry),
    /// and for an approval `approval` (the same entry) and `scope_choices`.
    pub fn apns_json(&self) -> serde_json::Value {
        let mut aps = serde_json::json!({
            "alert": {"title": self.title, "body": self.body},
            "sound": "default",
            "category": self.category,
            "interruption-level": if self.urgent { "time-sensitive" } else { "active" },
        });
        if let Some(t) = &self.thread {
            aps["thread-id"] = t.clone().into();
        }
        let mut out = serde_json::json!({"aps": aps, "v": 1, "kind": self.kind.wire_kind()});
        if let Some(s) = &self.space {
            out["space"] = s.clone().into();
        }
        if let Some(e) = self.entry {
            out["entry"] = e.into();
            if self.kind == EventKind::ApprovalNeeded {
                out["approval"] = e.into();
            }
        }
        if !self.scope_choices.is_empty() {
            out["scope_choices"] = serde_json::json!(self.scope_choices);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: EventKind) -> PushEvent {
        PushEvent {
            id: "n1".into(),
            kind,
            space: Some("relay:abc".into()),
            entry: Some(7),
            agent: Some("Claude Code".into()),
            scope_choices: vec!["once".into(), "space".into()],
            at_ms: 1,
        }
    }

    #[test]
    fn each_kind_has_its_own_short_fixed_alert() {
        let t: Vec<String> = EventKind::ALL
            .iter()
            .map(|k| event(*k).payload().title)
            .collect();
        assert_eq!(
            t,
            [
                "Claude Code needs you",
                "Claude Code finished",
                "Claude Code stopped",
                "Claude Code is blocked"
            ]
        );
        assert_eq!(
            event(EventKind::ApprovalNeeded).payload().category,
            "CUA_APPROVAL"
        );
        assert_eq!(
            event(EventKind::TaskFinished).payload().category,
            "CUA_DONE"
        );
        assert_eq!(
            event(EventKind::AgentBlocked).payload().category,
            "CUA_DONE"
        );
    }

    #[test]
    fn a_payload_never_carries_text_the_event_could_hold() {
        // The event type has no field for model text; an agent name that is
        // long or full of control characters is cleaned.
        let mut e = event(EventKind::TaskFailed);
        e.agent = Some(format!("a\nb\u{7}{}", "x".repeat(300)));
        let p = e.payload();
        assert!(!p.title.contains('\n') && !p.title.contains('\u{7}'));
        assert!(p.title.chars().count() < 60, "{}", p.title);
        let json = p.apns_json().to_string();
        assert!(json.len() < 600, "{json}");
    }

    #[test]
    fn the_apns_body_carries_the_ids_the_app_opens() {
        let j = event(EventKind::ApprovalNeeded).payload().apns_json();
        assert_eq!(j["aps"]["category"], "CUA_APPROVAL");
        assert_eq!(j["aps"]["thread-id"], "relay:abc");
        assert_eq!(j["space"], "relay:abc");
        assert_eq!(j["entry"], 7);
        assert_eq!(j["approval"], 7);
        assert_eq!(j["kind"], "approval");
        assert_eq!(j["v"], 1);
        assert_eq!(j["scope_choices"], serde_json::json!(["once", "space"]));
        assert_eq!(j["aps"]["interruption-level"], "time-sensitive");
        let j = event(EventKind::TaskFinished).payload().apns_json();
        assert_eq!(j["aps"]["category"], "CUA_DONE");
        assert_eq!(j["kind"], "done");
        assert!(j.get("approval").is_none() && j.get("scope_choices").is_none());
        assert_eq!(j["aps"]["interruption-level"], "active");
        assert_eq!(
            event(EventKind::TaskFailed).payload().apns_json()["kind"],
            "error"
        );
        assert_eq!(
            event(EventKind::AgentBlocked).payload().apns_json()["kind"],
            "blocked"
        );
    }

    #[test]
    fn events_are_checked() {
        let mut e = event(EventKind::TaskFinished);
        assert!(e.check().is_ok());
        e.id = " ".into();
        assert!(e.check().is_err());
        e.id = "x".into();
        e.space = Some("a\u{0}b".into());
        assert!(e.check().is_err());
        e.space = None;
        e.scope_choices = vec!["always".into()];
        assert!(e.check().is_err());
    }

    #[test]
    fn no_agent_reads_as_an_agent() {
        let mut e = event(EventKind::TaskFinished);
        e.agent = None;
        assert_eq!(e.payload().title, "An agent finished");
    }
}
