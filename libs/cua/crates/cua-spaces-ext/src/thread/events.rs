// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! System events in the thread: one way to say "this happened" to the person and the agent.
//!
//! A system-originated event (an approval was answered, a Space the agent made is ready or failed, a subagent
//! ended) is one thread entry with a [`Kind`], a structured payload (`event`), an [`Audience`] and its plain-text
//! rendering (`text`, which every text client reads). People see the rich card or row the apps draw from the
//! payload; agents read the text. An event that must reach an agent that has finished its turn [`Kind::wakes`]:
//! it is then an `@mention` task for that agent, which the supervisor delivers as a new turn, so no agent ever
//! polls or has to be told in prose.
//!
//! Every kind chooses its audience and whether it wakes in [`Kind::audience`] and [`Kind::wakes`]: both are
//! exhaustive matches, so a new kind does not compile until it chooses (and [`ALL`] must list it, which a
//! test checks).

use serde_json::{Value, json};

use super::{Author, Draft, EntryKind, TaskTo, ThreadEntry, ThreadStore};
use crate::Result;

/// Who an event is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    /// People see the card or row; agents read the text.
    Both,
    /// Only agents are told; the apps do not draw it (a card already shows the outcome).
    Agent,
    /// Only people (a notice that means nothing to an agent).
    Human,
}

impl Audience {
    /// The `audience` field an entry carries (`None`: both).
    pub fn field(self) -> Option<&'static str> {
        match self {
            Audience::Both => None,
            Audience::Agent => Some("agent"),
            Audience::Human => Some("human"),
        }
    }
}

/// The kinds of system event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The person allowed an agent's request.
    ApprovalAllowed,
    /// The person said no.
    ApprovalDenied,
    /// Nobody answered an agent's request in time.
    ApprovalExpired,
    /// A Space the agent asked the host to make is being made.
    ChildSpaceCreating,
    /// It is ready.
    ChildSpaceReady,
    /// It could not be made.
    ChildSpaceFailed,
    /// A job the agent started has finished or failed.
    JobFinished,
    /// A subagent started.
    SubagentStarted,
    /// A subagent ended (done or failed).
    SubagentEnded,
}

/// Every kind (a test keeps this complete).
pub const ALL: [Kind; 9] = [
    Kind::ApprovalAllowed,
    Kind::ApprovalDenied,
    Kind::ApprovalExpired,
    Kind::ChildSpaceCreating,
    Kind::ChildSpaceReady,
    Kind::ChildSpaceFailed,
    Kind::JobFinished,
    Kind::SubagentStarted,
    Kind::SubagentEnded,
];

impl Kind {
    /// The kind's name in the entry's `event.kind`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::ApprovalAllowed => "approval_allowed",
            Kind::ApprovalDenied => "approval_denied",
            Kind::ApprovalExpired => "approval_expired",
            Kind::ChildSpaceCreating | Kind::ChildSpaceReady | Kind::ChildSpaceFailed => {
                "child_space"
            }
            Kind::JobFinished => "job_finished",
            Kind::SubagentStarted | Kind::SubagentEnded => "subagent",
        }
    }

    /// Who the kind is for. Exhaustive: a new kind must choose.
    pub fn audience(self) -> Audience {
        match self {
            // The approval card shows the outcome to people.
            Kind::ApprovalAllowed | Kind::ApprovalDenied | Kind::ApprovalExpired => Audience::Agent,
            // The row shows people; the agent reads the text in the thread.
            Kind::ChildSpaceCreating
            | Kind::ChildSpaceReady
            | Kind::ChildSpaceFailed
            | Kind::JobFinished
            | Kind::SubagentStarted
            | Kind::SubagentEnded => Audience::Both,
        }
    }

    /// Whether the event starts a new turn for an agent that has ended its own (an `@mention` task for it).
    /// Exhaustive: a new kind must choose.
    pub fn wakes(self) -> bool {
        match self {
            // The agent waits for the answer and has nothing else to do until it comes.
            Kind::ApprovalAllowed
            | Kind::ApprovalDenied
            | Kind::ApprovalExpired
            | Kind::JobFinished => true,
            // The agent that asked for the Space is inside the call; the others inform what it reads next.
            Kind::ChildSpaceCreating
            | Kind::ChildSpaceReady
            | Kind::ChildSpaceFailed
            | Kind::SubagentStarted
            | Kind::SubagentEnded => false,
        }
    }
}

/// One system event, ready to post.
#[derive(Clone, Debug)]
pub struct AgentEvent {
    pub kind: Kind,
    /// The agent member it concerns (its handle).
    pub agent: String,
    /// The plain statement. A waking event starts with `@<agent>` (a real mention); people read this text too.
    pub text: String,
    /// What only the agent is told to do about it (never printed by the apps).
    pub guidance: Option<String>,
    /// What the apps draw the card or row from.
    pub payload: Value,
    /// The task it belongs to, for a note that folds under it.
    pub task_seq: Option<u64>,
    /// One entry per key, so a pass never repeats an event.
    pub client_id: Option<String>,
    /// Who posts it (`system:<what>`).
    pub from: &'static str,
}

/// The thread entry for `ev`.
pub fn draft(ev: &AgentEvent) -> Draft {
    let mut event = ev.payload.clone();
    if !event.is_object() {
        event = json!({});
    }
    event["kind"] = json!(ev.kind.name());
    let mut d = Draft::new(
        Author::system(ev.from),
        if ev.kind.wakes() {
            EntryKind::Message
        } else {
            EntryKind::System
        },
        ev.text.clone(),
    );
    d.audience = ev.kind.audience().field().map(str::to_string);
    d.task_seq = ev.task_seq;
    d.client_id = ev.client_id.clone();
    if let Some(g) = &ev.guidance {
        event["agent_note"] = json!(g);
    }
    d.event = Some(event);
    if ev.kind.wakes() {
        d.mentions = vec![ev.agent.clone()];
        d.task = Some(TaskTo {
            to: ev.agent.clone(),
            session: None,
        });
    }
    d
}

/// Posts `ev` to the thread: the one way system events reach people and agents.
pub fn post_agent_event(store: &ThreadStore, ev: &AgentEvent) -> Result<ThreadEntry> {
    store.append(draft(ev))
}

/// The guidance only the agent gets, from an entry's `event` (what delivery appends to the prompt).
pub fn guidance_of(entry: &ThreadEntry) -> Option<&str> {
    let ev = entry.event.as_ref()?;
    ev.get("agent_note").and_then(Value::as_str).or_else(|| {
        // Notices written before the helper: `{kind: agent_note, note}`.
        (ev["kind"] == "agent_note")
            .then(|| ev["note"].as_str())
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new kind cannot be added without choosing: the two matches are exhaustive, and this list is complete.
    #[test]
    fn every_kind_is_listed_and_has_an_audience_and_a_wake_rule() {
        fn index(k: Kind) -> usize {
            match k {
                Kind::ApprovalAllowed => 0,
                Kind::ApprovalDenied => 1,
                Kind::ApprovalExpired => 2,
                Kind::ChildSpaceCreating => 3,
                Kind::ChildSpaceReady => 4,
                Kind::ChildSpaceFailed => 5,
                Kind::JobFinished => 6,
                Kind::SubagentStarted => 7,
                Kind::SubagentEnded => 8,
            }
        }
        let mut seen = vec![false; ALL.len()];
        for k in ALL {
            seen[index(k)] = true;
            let _ = (k.audience(), k.wakes(), k.name());
        }
        assert!(seen.iter().all(|s| *s), "ALL lists every kind exactly once");
    }

    #[test]
    fn a_waking_event_mentions_the_agent_and_tasks_it_and_a_note_does_neither() {
        let ev = |kind| AgentEvent {
            kind,
            agent: "claude".into(),
            text: "@claude x".into(),
            guidance: Some("do y".into()),
            payload: json!({"a": 1}),
            task_seq: None,
            client_id: None,
            from: "approval",
        };
        let w = draft(&ev(Kind::ApprovalAllowed));
        assert_eq!(w.mentions, ["claude"]);
        assert!(w.task.is_some());
        assert_eq!(w.audience.as_deref(), Some("agent"));
        assert_eq!(w.event.as_ref().unwrap()["kind"], "approval_allowed");
        assert_eq!(w.event.as_ref().unwrap()["a"], 1);
        let n = draft(&ev(Kind::ChildSpaceReady));
        assert!(n.mentions.is_empty() && n.task.is_none() && n.audience.is_none());
    }
}
