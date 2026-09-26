//! Presentation accounting for the Wayland latency fixture.
//!
//! This module is deliberately platform-independent: it owns the causal row
//! shape, the outcome decision, and the derived deltas, so the rules that
//! decide "was this exact content update actually presented?" are unit tested
//! on every host instead of only inside a compositor lane.
//!
//! The one rule that matters: a content update is reported as presented only
//! when the compositor said `presented` for that update's own feedback object,
//! in a clock domain comparable to the fixture's own timestamps. Everything
//! else (discarded, superseded, timed out, foreign clock, no mutation) is
//! retained as a typed non-presented outcome and never counted as a success.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Schema tag written on every journal line.
pub const JOURNAL_SCHEMA: &str = "cua-wayland-presentation/v1";

/// `CLOCK_MONOTONIC`. The fixture stamps every one of its own timestamps with
/// this clock, so a compositor reporting any other `clock_id` is not
/// comparable and must not produce presentation deltas.
pub const CLOCK_MONOTONIC_ID: u32 = 1;

/// Bit values from `wp_presentation_feedback.kind`.
const KIND_VSYNC: u32 = 0x1;
const KIND_HW_CLOCK: u32 = 0x2;
const KIND_HW_COMPLETION: u32 = 0x4;
const KIND_ZERO_COPY: u32 = 0x8;

/// Which fixture region an input landed in. The inert region exists so a real
/// delivered input that intentionally changes nothing has a typed outcome and
/// can never be mistaken for a presented mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// Mutates fixture state and submits exactly one content update.
    Active,
    /// Receives the input, mutates nothing, submits no content update.
    Inert,
    /// Mutates twice back to back so the first update can be superseded.
    Supersede,
}

/// The typed fate of one submitted content update.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// State changed, this update's own feedback reported `presented`, and the
    /// compositor clock is comparable to the fixture clock.
    Verified,
    /// The compositor discarded or superseded this content update.
    Discarded,
    /// No feedback event arrived before the fixture's deadline.
    Timeout,
    /// `presented` arrived, but in a clock domain the fixture cannot compare.
    ClockMismatch,
    /// `presented` arrived in a comparable clock, but earlier than the commit
    /// it claims to present. Retained, never counted as a success.
    Implausible,
    /// The input was delivered but changed no state and committed nothing.
    NoMutation,
}

impl Outcome {
    /// True only for the single outcome that proves a presented mutation.
    pub fn is_presented_mutation(self) -> bool {
        matches!(self, Outcome::Verified)
    }
}

/// Compositor-supplied presentation metadata, retained verbatim.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Presentation {
    /// Compositor presentation clock id, as advertised by `wp_presentation`.
    pub clock_id: u32,
    /// Whether that clock is the fixture's own `CLOCK_MONOTONIC`.
    pub clock_comparable: bool,
    pub refresh_ns: u32,
    pub sequence: u64,
    pub flags: u32,
    pub vsync: bool,
    pub hw_clock: bool,
    pub hw_completion: bool,
    pub zero_copy: bool,
}

impl Presentation {
    pub fn new(clock_id: u32, refresh_ns: u32, sequence: u64, flags: u32) -> Self {
        Self {
            clock_id,
            clock_comparable: clock_id == CLOCK_MONOTONIC_ID,
            refresh_ns,
            sequence,
            flags,
            vsync: flags & KIND_VSYNC != 0,
            hw_clock: flags & KIND_HW_CLOCK != 0,
            hw_completion: flags & KIND_HW_COMPLETION != 0,
            zero_copy: flags & KIND_ZERO_COPY != 0,
        }
    }
}

/// What the compositor said about one content update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Feedback {
    Presented {
        /// Local callback receipt, sampled in the fixture's monotonic clock.
        feedback_received_ns: u64,
        presented_ns: u64,
        refresh_ns: u32,
        sequence: u64,
        flags: u32,
        /// `None` when `wp_presentation` never advertised a clock id.
        clock_id: Option<u32>,
    },
    Discarded {
        feedback_received_ns: u64,
    },
    Timeout,
}

/// One in-flight fixture action, from input to commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    pub action: String,
    pub sequence: u64,
    pub region: Region,
    pub input_received_ns: u64,
    pub state_changed_ns: Option<u64>,
    /// `None` when the action committed nothing (inert region).
    pub surface_commit_ns: Option<u64>,
    pub counter_before: u64,
    pub counter_after: u64,
    /// True for the first of the two `Supersede` commits.
    pub supersede_probe: bool,
}

impl Pending {
    pub fn mutated(&self) -> bool {
        self.counter_after != self.counter_before
    }
}

/// Deltas the fixture can derive on its own. Cross-boundary deltas that need
/// the Driver's own timestamps (`dispatch_to_app`, `return_minus_present`) are
/// left to the runner, which owns those stamps.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Derived {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_to_state_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_to_commit_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_to_present_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_to_present_ns: Option<u64>,
}

/// One retained raw observation. Small fixture runs keep every row; no
/// percentile machinery is introduced here on purpose (see issue #4012).
///
/// Write-only by design: the fixture emits rows and the runner reads them as
/// JSON, so the schema tags stay `&'static str` rather than allocating a
/// `String` per row for a `Deserialize` nobody needs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Sample {
    pub schema: &'static str,
    pub kind: &'static str,
    pub action: String,
    pub sequence: u64,
    pub region: Region,
    pub clock_id: u32,
    pub fixture_input_received_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture_state_changed_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_commit_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_received_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presented_ns: Option<u64>,
    pub counter_before: u64,
    pub counter_after: u64,
    pub mutated: bool,
    pub supersede_probe: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation: Option<Presentation>,
    pub derived: Derived,
    pub fixture_outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_miss: Option<bool>,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub note: String,
}

/// Decide the fate of one content update and derive only the deltas that the
/// evidence actually supports.
pub fn finalize(pending: &Pending, feedback: Feedback, deadline_ns: u64) -> Sample {
    let mut note = String::new();
    let mut presentation = None;
    let mut presented_ns = None;
    let mut feedback_received_ns = None;

    let outcome = if !pending.mutated() || pending.surface_commit_ns.is_none() {
        // A delivered-but-inert input, or any action that committed nothing.
        // There is no content update, so there is nothing to present.
        note.push_str("input delivered without a content update");
        Outcome::NoMutation
    } else {
        match feedback {
            Feedback::Discarded {
                feedback_received_ns: receipt,
            } => {
                feedback_received_ns = Some(receipt);
                note.push_str("compositor discarded or superseded this content update");
                Outcome::Discarded
            }
            Feedback::Timeout => {
                note.push_str("no presentation feedback before the fixture deadline");
                Outcome::Timeout
            }
            Feedback::Presented {
                feedback_received_ns: receipt,
                presented_ns: stamp,
                refresh_ns,
                sequence,
                flags,
                clock_id,
            } => {
                feedback_received_ns = Some(receipt);
                let clock = clock_id.unwrap_or(0);
                let record = Presentation::new(clock, refresh_ns, sequence, flags);
                let commit = pending.surface_commit_ns.unwrap_or(0);
                let decided = if !record.clock_comparable {
                    note.push_str(
                        "compositor presentation clock is not the fixture clock; \
                         presentation deltas are withheld",
                    );
                    Outcome::ClockMismatch
                } else if stamp < commit {
                    note.push_str("presented timestamp precedes its own commit");
                    Outcome::Implausible
                } else {
                    presented_ns = Some(stamp);
                    Outcome::Verified
                };
                presentation = Some(record);
                decided
            }
        }
    };

    let derived = Derived {
        input_to_state_ns: pending
            .state_changed_ns
            .and_then(|state| state.checked_sub(pending.input_received_ns)),
        app_to_commit_ns: pending
            .surface_commit_ns
            .and_then(|commit| commit.checked_sub(pending.input_received_ns)),
        commit_to_present_ns: presented_ns.and_then(|present| {
            pending
                .surface_commit_ns
                .and_then(|commit| present.checked_sub(commit))
        }),
        input_to_present_ns: presented_ns
            .and_then(|present| present.checked_sub(pending.input_received_ns)),
    };

    Sample {
        schema: JOURNAL_SCHEMA,
        kind: "sample",
        action: pending.action.clone(),
        sequence: pending.sequence,
        region: pending.region,
        clock_id: CLOCK_MONOTONIC_ID,
        fixture_input_received_ns: pending.input_received_ns,
        fixture_state_changed_ns: pending.state_changed_ns,
        surface_commit_ns: pending.surface_commit_ns,
        feedback_received_ns,
        presented_ns,
        counter_before: pending.counter_before,
        counter_after: pending.counter_after,
        mutated: pending.mutated(),
        supersede_probe: pending.supersede_probe,
        presentation,
        derived,
        fixture_outcome: outcome,
        deadline_miss: derived
            .input_to_present_ns
            .map(|elapsed| elapsed > deadline_ns),
        note,
    }
}

/// Consume only the update whose feedback object carried this ID. A callback
/// for an unknown or already-accounted update cannot acquire another row.
pub fn correlate(
    pending: &mut BTreeMap<u64, Pending>,
    update_id: u64,
    feedback: Feedback,
    deadline_ns: u64,
) -> Option<Sample> {
    pending
        .remove(&update_id)
        .map(|update| finalize(&update, feedback, deadline_ns))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEADLINE: u64 = 1_000_000_000;

    fn pending(region: Region, committed: bool) -> Pending {
        Pending {
            action: "click".to_owned(),
            sequence: 1,
            region,
            input_received_ns: 1_000,
            state_changed_ns: committed.then_some(1_200),
            surface_commit_ns: committed.then_some(2_000),
            counter_before: 0,
            counter_after: if committed { 1 } else { 0 },
            supersede_probe: false,
        }
    }

    fn presented(stamp: u64, clock: Option<u32>) -> Feedback {
        Feedback::Presented {
            feedback_received_ns: stamp + 100,
            presented_ns: stamp,
            refresh_ns: 16_666_666,
            sequence: 42,
            flags: KIND_VSYNC | KIND_HW_CLOCK,
            clock_id: clock,
        }
    }

    #[test]
    fn presented_update_in_the_fixture_clock_is_verified_with_full_deltas() {
        let sample = finalize(
            &pending(Region::Active, true),
            presented(10_000, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        );
        assert_eq!(sample.fixture_outcome, Outcome::Verified);
        assert!(sample.fixture_outcome.is_presented_mutation());
        assert_eq!(sample.presented_ns, Some(10_000));
        assert_eq!(sample.feedback_received_ns, Some(10_100));
        assert_eq!(sample.derived.input_to_state_ns, Some(200));
        assert_eq!(sample.derived.app_to_commit_ns, Some(1_000));
        assert_eq!(sample.derived.commit_to_present_ns, Some(8_000));
        assert_eq!(sample.derived.input_to_present_ns, Some(9_000));
        assert_eq!(sample.deadline_miss, Some(false));
        let presentation = sample.presentation.expect("presentation metadata");
        assert!(presentation.clock_comparable);
        assert!(presentation.vsync);
        assert!(presentation.hw_clock);
        assert!(!presentation.hw_completion);
        assert!(!presentation.zero_copy);
        assert_eq!(presentation.refresh_ns, 16_666_666);
        assert_eq!(presentation.sequence, 42);
    }

    #[test]
    fn discarded_update_is_never_counted_as_presented() {
        let sample = finalize(
            &pending(Region::Active, true),
            Feedback::Discarded {
                feedback_received_ns: 10_100,
            },
            DEADLINE,
        );
        assert_eq!(sample.fixture_outcome, Outcome::Discarded);
        assert!(!sample.fixture_outcome.is_presented_mutation());
        assert_eq!(sample.presented_ns, None);
        assert_eq!(sample.feedback_received_ns, Some(10_100));
        assert_eq!(sample.derived.commit_to_present_ns, None);
        assert_eq!(sample.derived.input_to_present_ns, None);
        assert_eq!(sample.deadline_miss, None);
        // The application-owned half of the evidence survives a discard.
        assert!(sample.mutated);
        assert_eq!(sample.derived.app_to_commit_ns, Some(1_000));
    }

    #[test]
    fn missing_feedback_times_out_instead_of_presenting() {
        let sample = finalize(&pending(Region::Active, true), Feedback::Timeout, DEADLINE);
        assert_eq!(sample.fixture_outcome, Outcome::Timeout);
        assert_eq!(sample.presented_ns, None);
        assert_eq!(sample.feedback_received_ns, None);
        assert_eq!(sample.presentation, None);
    }

    #[test]
    fn foreign_presentation_clock_withholds_deltas_but_retains_metadata() {
        let sample = finalize(
            &pending(Region::Active, true),
            presented(10_000, Some(7)),
            DEADLINE,
        );
        assert_eq!(sample.fixture_outcome, Outcome::ClockMismatch);
        assert_eq!(sample.presented_ns, None);
        assert_eq!(sample.feedback_received_ns, Some(10_100));
        assert_eq!(sample.derived.commit_to_present_ns, None);
        let presentation = sample.presentation.expect("presentation metadata");
        assert_eq!(presentation.clock_id, 7);
        assert!(!presentation.clock_comparable);
    }

    #[test]
    fn unadvertised_presentation_clock_is_not_assumed_to_be_monotonic() {
        let sample = finalize(
            &pending(Region::Active, true),
            presented(10_000, None),
            DEADLINE,
        );
        assert_eq!(sample.fixture_outcome, Outcome::ClockMismatch);
        assert_eq!(sample.presented_ns, None);
        assert_eq!(sample.presentation.expect("metadata").clock_id, 0);
    }

    #[test]
    fn presentation_before_its_own_commit_is_implausible_not_verified() {
        let sample = finalize(
            &pending(Region::Active, true),
            presented(1_500, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        );
        assert_eq!(sample.fixture_outcome, Outcome::Implausible);
        assert_eq!(sample.presented_ns, None);
        assert_eq!(sample.derived.commit_to_present_ns, None);
    }

    #[test]
    fn inert_input_is_recorded_as_delivered_without_a_presented_mutation() {
        let sample = finalize(
            &pending(Region::Inert, false),
            presented(10_000, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        );
        assert_eq!(sample.fixture_outcome, Outcome::NoMutation);
        assert!(!sample.mutated);
        assert_eq!(sample.presented_ns, None);
        assert_eq!(sample.surface_commit_ns, None);
        assert_eq!(sample.derived.app_to_commit_ns, None);
    }

    #[test]
    fn a_mutation_whose_commit_is_unknown_cannot_be_presented() {
        let mut pending = pending(Region::Active, true);
        pending.surface_commit_ns = None;
        let sample = finalize(
            &pending,
            presented(10_000, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        );
        assert_eq!(sample.fixture_outcome, Outcome::NoMutation);
        assert_eq!(sample.presented_ns, None);
    }

    #[test]
    fn a_slow_presentation_is_verified_and_flagged_as_a_deadline_miss() {
        let sample = finalize(
            &pending(Region::Active, true),
            presented(1_000 + DEADLINE + 1, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        );
        assert_eq!(sample.fixture_outcome, Outcome::Verified);
        assert_eq!(sample.deadline_miss, Some(true));
    }

    #[test]
    fn rows_serialize_with_the_schema_and_snake_case_outcomes() {
        let sample = finalize(
            &pending(Region::Active, true),
            presented(10_000, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        );
        let row = serde_json::to_value(&sample).expect("serialize row");
        assert_eq!(row["schema"], JOURNAL_SCHEMA);
        assert_eq!(row["kind"], "sample");
        assert_eq!(row["fixture_outcome"], "verified");
        assert_eq!(row["region"], "active");
        assert_eq!(row["clock_id"], CLOCK_MONOTONIC_ID);
        assert_eq!(row["presented_ns"], 10_000);
        assert_eq!(row["derived"]["commit_to_present_ns"], 8_000);
        // Withheld deltas are absent rather than zero-filled.
        let discarded = serde_json::to_value(finalize(
            &pending(Region::Active, true),
            Feedback::Discarded {
                feedback_received_ns: 10_100,
            },
            DEADLINE,
        ))
        .expect("serialize discarded row");
        assert!(discarded.get("presented_ns").is_none());
        assert!(discarded["derived"].get("commit_to_present_ns").is_none());
    }

    #[test]
    fn wrong_and_late_feedback_cannot_claim_another_update() {
        let mut pending_updates = BTreeMap::from([(1, pending(Region::Active, true))]);
        assert!(correlate(
            &mut pending_updates,
            2,
            presented(10_000, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        )
        .is_none());
        assert!(pending_updates.contains_key(&1));

        let matched = correlate(
            &mut pending_updates,
            1,
            presented(10_000, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        )
        .expect("the exact update is still pending");
        assert_eq!(matched.sequence, 1);
        assert_eq!(matched.fixture_outcome, Outcome::Verified);
        assert!(correlate(
            &mut pending_updates,
            1,
            presented(11_000, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        )
        .is_none());

        let mut timed_out = pending(Region::Active, true);
        timed_out.sequence = 3;
        pending_updates.insert(3, timed_out);
        let expired = correlate(&mut pending_updates, 3, Feedback::Timeout, DEADLINE)
            .expect("timeout consumes its own update");
        assert_eq!(expired.fixture_outcome, Outcome::Timeout);
        assert!(correlate(
            &mut pending_updates,
            3,
            presented(12_000, Some(CLOCK_MONOTONIC_ID)),
            DEADLINE,
        )
        .is_none());
    }
}
