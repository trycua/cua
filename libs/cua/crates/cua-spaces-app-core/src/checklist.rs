// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The getting-started checklist: the first five things worth doing in Cua
//! Spaces, ticked off as they happen, closable, and opened again from the
//! command palette or Help, Getting Started.
//!
//! The host app keeps the [`ChecklistState`] in one place
//! (`checklist.json` beside its settings; the web UI asks the host for it
//! with `checklist.get` and `checklist.send`), sends a [`ChecklistAction`]
//! for each thing that happens and records the usage events the
//! [`ChecklistTransition`] carries. [`observe_from`] turns what the app
//! knows (its machines, its Spaces, its agents) into the
//! [`ChecklistAction::Observe`] that ticks steps off, the same for every
//! UI; Teleport and Share tick themselves off where they finish.
//! [`ChecklistAction::Merge`] takes in progress kept elsewhere before (the
//! web UI's own storage) without recording its events again.

use serde::{Deserialize, Serialize};

use crate::machines::MachinePageRow;
use crate::model::{Space, SpaceStatus};
use crate::spaces::{creating, is_machine_desktop};
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
                "Add a computer",
                "Connect a Mac, Linux or Windows computer you own.",
                "Add computer",
            ),
            ChecklistStep::CreateSpace => (
                "Create a Space",
                "A computer for one task, on one of your computers.",
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
    /// Steps skipped ("Skip"), each once, in checklist order: they count as
    /// done for the progress and for `complete`, and a skipped step done
    /// later shows as done.
    #[serde(default)]
    pub skipped: Vec<ChecklistStep>,
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
    /// "Skip" on a step: not for this person; it counts as done.
    Skip {
        /// The step.
        step: ChecklistStep,
    },
    /// Closed with × ("Hide", or "Done" once complete).
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
    /// Progress kept elsewhere before (the web UI's own storage, another
    /// copy of the file): its steps are ticked, and it stays closed or
    /// shown if it was. Its events were recorded where it was ticked, so
    /// none are recorded again.
    Merge {
        /// The other copy.
        state: ChecklistState,
    },
}

/// What the app knows, for [`observe_from`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistSeen {
    /// The Machines page's rows ([`crate::machines::page_rows`]).
    #[serde(default)]
    pub machines: Vec<MachinePageRow>,
    /// The Spaces list.
    #[serde(default)]
    pub spaces: Vec<Space>,
    /// Persistent agents listed.
    #[serde(default)]
    pub agents: u32,
    /// Agent runs listed in any Space.
    #[serde(default)]
    pub runs: u32,
}

/// What the app shows as the checklist's [`ChecklistAction::Observe`]:
///
/// - another machine: any row of the Machines page but this one (a
///   machine on the relay, one that provides Spaces, a device of your own);
/// - a Space: one you made (not a machine's own desktop), there now:
///   running, waiting for approval, or stopped; not one being created,
///   deleted, or whose create failed;
/// - an agent: a persistent agent, or a run in any Space.
pub fn observe_from(seen: &ChecklistSeen) -> ChecklistAction {
    ChecklistAction::Observe {
        other_machine: seen.machines.iter().any(|m| !m.current),
        has_space: seen.spaces.iter().any(|s| {
            !is_machine_desktop(s)
                && !creating::is_pending(&s.id)
                && matches!(
                    s.status,
                    SpaceStatus::Running | SpaceStatus::Approval | SpaceStatus::Suspended
                )
                && s.progress.as_ref().is_none_or(|p| p.error.is_none())
        }),
        agent_ran: seen.agents + seen.runs > 0,
    }
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
    /// Skipped (struck through, not ticked); never with `done`.
    pub skipped: bool,
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
    /// "Skip", on each step not done or skipped.
    pub skip_label: String,
    /// "Hide", or "Done" once complete: closes the checklist.
    pub hide_label: String,
    /// The steps in order.
    pub rows: Vec<ChecklistRow>,
}

fn is_done(state: &ChecklistState, step: ChecklistStep) -> bool {
    state.done.contains(&step)
}

fn is_skipped(state: &ChecklistState, step: ChecklistStep) -> bool {
    state.skipped.contains(&step)
}

/// Done or skipped: what the progress counts.
fn is_settled(state: &ChecklistState, step: ChecklistStep) -> bool {
    is_done(state, step) || is_skipped(state, step)
}

/// Every step done or skipped.
pub fn complete(state: &ChecklistState) -> bool {
    STEPS.iter().all(|s| is_settled(state, *s))
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
        ChecklistAction::Merge { state: other } => ChecklistTransition {
            state: ChecklistState {
                done: STEPS
                    .iter()
                    .copied()
                    .filter(|s| is_done(state, *s) || is_done(other, *s))
                    .collect(),
                dismissed: state.dismissed || other.dismissed,
                shown: state.shown || other.shown,
                skipped: STEPS
                    .iter()
                    .copied()
                    .filter(|s| is_skipped(state, *s) || is_skipped(other, *s))
                    .collect(),
            },
            signals: vec![],
        },
        ChecklistAction::Skip { step } => {
            if is_settled(state, *step) {
                return same();
            }
            let mut next = state.clone();
            next.skipped = STEPS
                .iter()
                .copied()
                .filter(|s| is_skipped(state, *s) || s == step)
                .collect();
            // A skip records no step of its own; the last one to settle
            // still completes the checklist.
            let signals = if complete(&next) {
                signals(&["completed"])
            } else {
                vec![]
            };
            ChecklistTransition {
                state: next,
                signals,
            }
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
    let count = STEPS.iter().filter(|s| is_settled(state, **s)).count();
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
            "Shared computers for your team and its agents, on computers you own."
        }
        .into(),
        progress: format!("{count} of {}", STEPS.len()),
        close_label: "Close getting started".into(),
        skip_label: "Skip".into(),
        hide_label: if complete { "Done" } else { "Hide" }.into(),
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
                    skipped: is_skipped(state, step) && !is_done(state, step),
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
                ("Add a computer", "Add computer", false),
                ("Create a Space", "Create", false),
                ("Teleport an app", "Teleport", true),
                ("Share a Space", "Share", false),
                ("Run an agent", "Open Agents", false),
            ]
        );
    }

    #[test]
    fn merging_another_copy_ticks_its_steps_without_events() {
        let here = ChecklistState {
            done: vec![ChecklistStep::CreateSpace, ChecklistStep::AddMachine],
            ..Default::default()
        };
        let there = ChecklistState {
            done: vec![ChecklistStep::Teleport, ChecklistStep::AddMachine],
            shown: true,
            ..Default::default()
        };
        let t = reduce(&here, &ChecklistAction::Merge { state: there });
        assert!(t.signals.is_empty(), "recorded where they were ticked");
        assert_eq!(
            t.state.done,
            [
                ChecklistStep::AddMachine,
                ChecklistStep::CreateSpace,
                ChecklistStep::Teleport
            ]
        );
        assert!(t.state.shown && !t.state.dismissed);
        assert_eq!(view(&t.state).progress, "3 of 5");
        // Merging again changes nothing.
        let again = reduce(&t.state, &ChecklistAction::Merge { state: here });
        assert_eq!(again.state, t.state);
    }

    fn space(id: &str, status: &str, extra: serde_json::Value) -> Space {
        let mut v = serde_json::json!({
            "id": id, "name": id, "os": "linux", "status": status, "detail": "",
            "lastUsedAt": 0, "scene": "linux-terminal"
        });
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        serde_json::from_value(v).unwrap()
    }

    fn row(id: &str, current: bool, device_only: bool) -> MachinePageRow {
        MachinePageRow {
            id: id.into(),
            name: id.into(),
            online: true,
            os: "macos".into(),
            via: if current { "local" } else { "relay" }.into(),
            current,
            device: None,
            device_only,
            space_id: None,
            stopped_sharing: false,
            last_seen: None,
            status: "Online".into(),
            spaces: 0,
            spaces_label: "No Spaces".into(),
        }
    }

    fn seen(t: &ChecklistAction) -> (bool, bool, bool) {
        match t {
            ChecklistAction::Observe {
                other_machine,
                has_space,
                agent_ran,
            } => (*other_machine, *has_space, *agent_ran),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn observe_from_reads_machines_spaces_and_agents_alike_for_every_ui() {
        // Only this machine, its own desktop, a machine's desktop and
        // Spaces that are not there yet: nothing.
        let none = ChecklistSeen {
            machines: vec![row("this-mac", true, false)],
            spaces: vec![
                space("this-mac", "local", serde_json::json!({})),
                space(
                    "relay:b695",
                    "suspended",
                    serde_json::json!({"provider": "relay"}),
                ),
                space("pending:1", "provisioning", serde_json::json!({})),
                space("local:gone", "deleting", serde_json::json!({})),
                space(
                    "local:failed",
                    "suspended",
                    serde_json::json!({
                        "progress": {"phase": "pull", "permille": 0, "label": "", "error": "no disk",
                                     "cancellable": false}
                    }),
                ),
            ],
            agents: 0,
            runs: 0,
        };
        assert_eq!(seen(&observe_from(&none)), (false, false, false));
        // A device of your own counts as another machine; a stopped Space
        // counts; a run in a Space counts as an agent.
        let some = ChecklistSeen {
            machines: vec![row("this-mac", true, false), row("dev_lap", false, true)],
            spaces: vec![space("local:fbd1", "suspended", serde_json::json!({}))],
            agents: 0,
            runs: 1,
        };
        assert_eq!(seen(&observe_from(&some)), (true, true, true));
        // A Space a machine provides counts too.
        let hosted = ChecklistSeen {
            spaces: vec![space(
                "relay:m-mini/space-1",
                "running",
                serde_json::json!({"provider": "relay", "host": "m-mini"}),
            )],
            agents: 2,
            ..Default::default()
        };
        assert_eq!(seen(&observe_from(&hosted)), (false, true, true));
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
                shown: false,
                skipped: vec![],
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

    #[test]
    fn a_skipped_step_counts_as_done_and_is_kept() {
        let skip = |s: &ChecklistState, step| reduce(s, &ChecklistAction::Skip { step });
        let mut state = ChecklistState {
            done: vec![ChecklistStep::AddMachine, ChecklistStep::CreateSpace],
            ..Default::default()
        };
        let t = skip(&state, ChecklistStep::Teleport);
        assert!(t.signals.is_empty(), "a skip records no step");
        state = t.state;
        assert_eq!(state.skipped, [ChecklistStep::Teleport]);
        let v = view(&state);
        assert_eq!(v.progress, "3 of 5");
        let teleport = v
            .rows
            .iter()
            .find(|r| r.step == ChecklistStep::Teleport)
            .unwrap();
        assert!(teleport.skipped && !teleport.done);
        assert_eq!(
            (v.skip_label.as_str(), v.hide_label.as_str()),
            ("Skip", "Hide")
        );
        // Skipping again, or skipping a done step, changes nothing.
        assert_eq!(skip(&state, ChecklistStep::Teleport).state, state);
        assert_eq!(skip(&state, ChecklistStep::AddMachine).state, state);
        // Kept as the hosts store it, and read back.
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["skipped"], serde_json::json!(["teleport"]));
        assert_eq!(
            serde_json::from_value::<ChecklistState>(json).unwrap(),
            state
        );
        // The last one settled completes it, whether done or skipped.
        state = skip(&state, ChecklistStep::Share).state;
        let t = reduce(
            &state,
            &ChecklistAction::Done {
                step: ChecklistStep::RunAgent,
            },
        );
        assert_eq!(events(&t), ["checklist_run_agent", "checklist_completed"]);
        let v = view(&t.state);
        assert!(v.complete);
        assert_eq!(
            (v.progress.as_str(), v.hide_label.as_str()),
            ("5 of 5", "Done")
        );
        // A skipped step done later shows as done.
        let t = reduce(
            &t.state,
            &ChecklistAction::Done {
                step: ChecklistStep::Teleport,
            },
        );
        let row = view(&t.state)
            .rows
            .into_iter()
            .find(|r| r.step == ChecklistStep::Teleport)
            .unwrap();
        assert!(row.done && !row.skipped);
    }

    #[test]
    fn merging_keeps_both_copies_skips() {
        let here = ChecklistState {
            skipped: vec![ChecklistStep::Share],
            ..Default::default()
        };
        let there = ChecklistState {
            skipped: vec![ChecklistStep::Teleport],
            ..Default::default()
        };
        let t = reduce(&here, &ChecklistAction::Merge { state: there });
        assert_eq!(
            t.state.skipped,
            [ChecklistStep::Teleport, ChecklistStep::Share]
        );
        // An older copy without skips keeps this one's.
        let old: ChecklistState = serde_json::from_str(r#"{"done":["share"]}"#).unwrap();
        assert!(old.skipped.is_empty());
        assert_eq!(
            reduce(&here, &ChecklistAction::Merge { state: old })
                .state
                .skipped,
            [ChecklistStep::Share]
        );
    }
}
