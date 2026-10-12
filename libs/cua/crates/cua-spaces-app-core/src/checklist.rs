// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The getting-started checklist: the first five things worth doing in Cua
//! Spaces, ticked off as they happen, closable, and opened again from the
//! command palette or Help, Getting Started.
//!
//! The shell keeps the [`ChecklistState`] (the React UI in the page's
//! storage, the SwiftUI app in its support folder), sends a
//! [`ChecklistAction`] for each thing that happens and records the usage
//! events the [`ChecklistTransition`] carries. [`ChecklistAction::Observe`]
//! ticks steps off from what the app already shows (another machine, a
//! Space, an agent); Teleport and Share tick themselves off where they
//! finish.

use serde::{Deserialize, Serialize};

use crate::telemetry::{self, TelemetrySignal};

/// One step, in the order the checklist lists them
/// ([`telemetry::CHECKLIST_STEPS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistStep {
    /// Connect another computer you own.
    AddMachine,
    /// Create a Space.
    CreateSpace,
    /// Teleport an app into a Space.
    Teleport,
    /// Share a Space with someone.
    Share,
    /// Run an agent.
    RunAgent,
}

/// Every step, in order.
pub const STEPS: [ChecklistStep; 5] = [
    ChecklistStep::AddMachine,
    ChecklistStep::CreateSpace,
    ChecklistStep::Teleport,
    ChecklistStep::Share,
    ChecklistStep::RunAgent,
];

impl ChecklistStep {
    /// The step's word (`add_machine`), as stored and as its usage event says.
    pub fn word(self) -> &'static str {
        match self {
            ChecklistStep::AddMachine => "add_machine",
            ChecklistStep::CreateSpace => "create_space",
            ChecklistStep::Teleport => "teleport",
            ChecklistStep::Share => "share",
            ChecklistStep::RunAgent => "run_agent",
        }
    }

    fn copy(self) -> (&'static str, &'static str, &'static str) {
        match self {
            ChecklistStep::AddMachine => (
                "Add a machine",
                "Connect a Mac, Linux or Windows computer you own.",
                "Add machine",
            ),
            ChecklistStep::CreateSpace => (
                "Create a Space",
                "A computer for one task, on one of your machines.",
                "Create",
            ),
            ChecklistStep::Teleport => (
                "Teleport an app",
                "Move a signed-in app into a Space.",
                "Teleport",
            ),
            ChecklistStep::Share => (
                "Share a Space",
                "Let a teammate watch or work in it.",
                "Share",
            ),
            ChecklistStep::RunAgent => (
                "Run an agent",
                "Give an agent a Space to work in.",
                "Open Agents",
            ),
        }
    }
}

/// What the checklist remembers between launches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistState {
    /// Steps done, each once, in checklist order.
    #[serde(default)]
    pub done: Vec<ChecklistStep>,
    /// Closed with ×; Getting Started opens it again.
    #[serde(default)]
    pub dismissed: bool,
    /// It was on screen once (`checklist_shown` was recorded).
    #[serde(default)]
    pub shown: bool,
}

/// Something that happened to the checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ChecklistAction {
    /// A step finished (a teleport ran, a Space was shared).
    Done {
        /// The step.
        step: ChecklistStep,
    },
    /// The checklist is on screen.
    Shown,
    /// Closed with ×.
    Dismiss,
    /// Getting Started (the palette, the Help menu): open it again.
    Reopen,
    /// What the app shows now: each `true` ticks its step off.
    Observe {
        /// One of your machines other than this one is listed.
        other_machine: bool,
        /// A Space of yours exists (running or stopped).
        has_space: bool,
        /// An agent exists or has run.
        agent_ran: bool,
    },
}

/// The state after an action and the usage events it means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistTransition {
    /// The new state (save it when it changed).
    pub state: ChecklistState,
    /// `cua_onboarding_step` events to record (`checklist_<event>`).
    pub signals: Vec<TelemetrySignal>,
}

/// One row of the checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistRow {
    /// The step.
    pub step: ChecklistStep,
    /// "Add a machine".
    pub title: String,
    /// One line under the title.
    pub detail: String,
    /// The button that does it; hidden once done.
    pub action: String,
    /// Done (ticked, struck through).
    pub done: bool,
}

/// The checklist as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistView {
    /// On screen (not closed).
    pub visible: bool,
    /// Every step done.
    pub complete: bool,
    /// "Get started", or "You're set up" when complete.
    pub title: String,
    /// The line under the title.
    pub detail: String,
    /// "2 of 5".
    pub progress: String,
    /// The close button's accessibility label.
    pub close_label: String,
    /// The steps in order.
    pub rows: Vec<ChecklistRow>,
}

fn is_done(state: &ChecklistState, step: ChecklistStep) -> bool {
    state.done.contains(&step)
}

/// Every step done.
pub fn complete(state: &ChecklistState) -> bool {
    STEPS.iter().all(|s| is_done(state, *s))
}

fn signals(events: &[&str]) -> Vec<TelemetrySignal> {
    events
        .iter()
        .flat_map(|e| telemetry::checklist(e))
        .collect()
}

/// Ticks `steps` off (each once, kept in checklist order) and answers the
/// events: each new step, then `completed` with the last one.
fn tick(state: &ChecklistState, steps: &[ChecklistStep]) -> ChecklistTransition {
    let new: Vec<ChecklistStep> = STEPS
        .iter()
        .copied()
        .filter(|s| steps.contains(s) && !is_done(state, *s))
        .collect();
    if new.is_empty() {
        return ChecklistTransition {
            state: state.clone(),
            signals: vec![],
        };
    }
    let mut next = state.clone();
    next.done = STEPS
        .iter()
        .copied()
        .filter(|s| is_done(state, *s) || new.contains(s))
        .collect();
    let mut events: Vec<&str> = new.iter().map(|s| s.word()).collect();
    if complete(&next) {
        events.push("completed");
    }
    ChecklistTransition {
        signals: signals(&events),
        state: next,
    }
}

/// The checklist after `action`, and the usage events it means.
pub fn reduce(state: &ChecklistState, action: &ChecklistAction) -> ChecklistTransition {
    let same = || ChecklistTransition {
        state: state.clone(),
        signals: vec![],
    };
    match action {
        ChecklistAction::Done { step } => tick(state, &[*step]),
        ChecklistAction::Observe {
            other_machine,
            has_space,
            agent_ran,
        } => {
            let seen: Vec<ChecklistStep> = [
                (*other_machine, ChecklistStep::AddMachine),
                (*has_space, ChecklistStep::CreateSpace),
                (*agent_ran, ChecklistStep::RunAgent),
            ]
            .into_iter()
            .filter_map(|(on, s)| on.then_some(s))
            .collect();
            tick(state, &seen)
        }
        ChecklistAction::Shown if !state.shown => ChecklistTransition {
            state: ChecklistState {
                shown: true,
                ..state.clone()
            },
            signals: signals(&["shown"]),
        },
        ChecklistAction::Dismiss | ChecklistAction::Reopen => {
            let dismissed = matches!(action, ChecklistAction::Dismiss);
            if state.dismissed == dismissed {
                return same();
            }
            ChecklistTransition {
                state: ChecklistState {
                    dismissed,
                    ..state.clone()
                },
                signals: signals(&[if dismissed { "dismissed" } else { "reopened" }]),
            }
        }
        ChecklistAction::Shown => same(),
    }
}

/// The checklist as drawn.
pub fn view(state: &ChecklistState) -> ChecklistView {
    let complete = complete(state);
    let count = STEPS.iter().filter(|s| is_done(state, **s)).count();
    ChecklistView {
        visible: !state.dismissed,
        complete,
        title: if complete {
            "You're set up"
        } else {
            "Get started"
        }
        .into(),
        detail: if complete {
            "Your team and its agents can now share computers you own."
        } else {
            "Shared computers for your team and its agents, on machines you own."
        }
        .into(),
        progress: format!("{count} of {}", STEPS.len()),
        close_label: "Close getting started".into(),
        rows: STEPS
            .iter()
            .map(|&step| {
                let (title, detail, action) = step.copy();
                ChecklistRow {
                    step,
                    title: title.into(),
                    detail: detail.into(),
                    action: action.into(),
                    done: is_done(state, step),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(t: &ChecklistTransition) -> Vec<String> {
        t.signals
            .iter()
            .map(|s| match s {
                TelemetrySignal::Step { step, ok: true } => step.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    #[test]
    fn steps_match_the_usage_vocabulary() {
        let words: Vec<&str> = STEPS.iter().map(|s| s.word()).collect();
        assert_eq!(words, telemetry::CHECKLIST_STEPS);
        for s in STEPS {
            assert_eq!(serde_json::to_value(s).unwrap(), s.word());
        }
    }

    #[test]
    fn each_step_counts_once_and_the_last_completes_it() {
        let mut state = ChecklistState::default();
        let mut seen = vec![];
        for step in STEPS.iter().rev() {
            for _ in 0..2 {
                let t = reduce(&state, &ChecklistAction::Done { step: *step });
                seen.extend(events(&t));
                state = t.state;
            }
        }
        assert_eq!(
            seen,
            [
                "checklist_run_agent",
                "checklist_share",
                "checklist_teleport",
                "checklist_create_space",
                "checklist_add_machine",
                "checklist_completed",
            ]
        );
        assert_eq!(state.done, STEPS, "kept in checklist order");
        let v = view(&state);
        assert!(v.complete);
        assert_eq!(v.title, "You're set up");
        assert_eq!(v.progress, "5 of 5");
    }

    #[test]
    fn observing_ticks_off_what_the_app_shows() {
        let t = reduce(
            &ChecklistState::default(),
            &ChecklistAction::Observe {
                other_machine: true,
                has_space: true,
                agent_ran: false,
            },
        );
        assert_eq!(
            events(&t),
            ["checklist_add_machine", "checklist_create_space"]
        );
        let again = reduce(
            &t.state,
            &ChecklistAction::Observe {
                other_machine: true,
                has_space: false,
                agent_ran: false,
            },
        );
        assert!(again.signals.is_empty());
        assert_eq!(again.state, t.state, "nothing is unticked");
        assert_eq!(view(&again.state).progress, "2 of 5");
    }

    #[test]
    fn shown_once_and_closing_and_reopening() {
        let s = ChecklistState::default();
        let shown = reduce(&s, &ChecklistAction::Shown);
        assert_eq!(events(&shown), ["checklist_shown"]);
        assert!(
            reduce(&shown.state, &ChecklistAction::Shown)
                .signals
                .is_empty()
        );

        let closed = reduce(&shown.state, &ChecklistAction::Dismiss);
        assert_eq!(events(&closed), ["checklist_dismissed"]);
        assert!(!view(&closed.state).visible);
        assert!(
            reduce(&closed.state, &ChecklistAction::Dismiss)
                .signals
                .is_empty()
        );

        let open = reduce(&closed.state, &ChecklistAction::Reopen);
        assert_eq!(events(&open), ["checklist_reopened"]);
        assert!(view(&open.state).visible);
        assert!(
            reduce(&open.state, &ChecklistAction::Reopen)
                .signals
                .is_empty()
        );
    }

    #[test]
    fn the_view_lists_every_step_with_its_button() {
        let s = ChecklistState {
            done: vec![ChecklistStep::Teleport],
            ..Default::default()
        };
        let v = view(&s);
        assert_eq!(v.title, "Get started");
        assert_eq!(v.progress, "1 of 5");
        let rows: Vec<(&str, &str, bool)> = v
            .rows
            .iter()
            .map(|r| (r.title.as_str(), r.action.as_str(), r.done))
            .collect();
        assert_eq!(
            rows,
            [
                ("Add a machine", "Add machine", false),
                ("Create a Space", "Create", false),
                ("Teleport an app", "Teleport", true),
                ("Share a Space", "Share", false),
                ("Run an agent", "Open Agents", false),
            ]
        );
    }

    #[test]
    fn reads_the_stored_shape() {
        let s: ChecklistState =
            serde_json::from_str(r#"{"done":["share","add_machine"],"dismissed":true}"#).unwrap();
        assert_eq!(
            s,
            ChecklistState {
                done: vec![ChecklistStep::Share, ChecklistStep::AddMachine],
                dismissed: true,
                shown: false
            }
        );
        let a: ChecklistAction = serde_json::from_str(
            r#"{"type":"observe","otherMachine":false,"hasSpace":true,"agentRan":false}"#,
        )
        .unwrap();
        assert_eq!(
            a,
            ChecklistAction::Observe {
                other_machine: false,
                has_space: true,
                agent_ran: false
            }
        );
    }
}
