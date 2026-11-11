// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! When one foreground input turn may give the user's window back.
//!
//! Input the target thread has not read yet is read by whichever window is
//! foreground at that moment (#4477), so restoring the previous window early
//! hands it the unread tail. A late keystroke in the user's window cannot be
//! undone; a target left in front costs one click. The rule is modelled in
//! `.cursor/skills/formally-verify-cua-driver/models/windows_typing.tla`.
//! Pure, so the table test runs on every host.

/// How a foreground input turn ended, by what is known about inserted input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypingTurn {
    /// Nothing was inserted.
    NotSent,
    /// The target thread read every inserted event.
    Drained,
    /// Inserted without a queue attachment, so the read cannot be observed;
    /// a short fixed settle stands in for the drain.
    Settled,
    /// Inserted events may still be unread: a stalled unit, a short
    /// `SendInput`, or a final drain that timed out.
    Undrained,
    /// Another window took the foreground between text units.
    Lost,
}

/// Whether to give the previous foreground back. Never while inserted input
/// may be unread, and never from a window that took the foreground itself.
pub(crate) fn restores_previous(turn: TypingTurn) -> bool {
    match turn {
        TypingTurn::NotSent | TypingTurn::Drained | TypingTurn::Settled => true,
        TypingTurn::Undrained | TypingTurn::Lost => false,
    }
}

/// A failed turn that did not give the previous foreground back. Callers
/// report `foreground_restored: false`.
#[derive(Debug, thiserror::Error)]
#[error("{error} Focus was not restored: {}", reason(*.turn))]
pub(crate) struct ForegroundNotRestored {
    pub(crate) error: anyhow::Error,
    pub(crate) turn: TypingTurn,
}

fn reason(turn: TypingTurn) -> &'static str {
    if turn == TypingTurn::Lost {
        "another window took the foreground during the call, and the driver does not take \
         it back."
    } else {
        "the target window stays in front so input it has not read yet cannot reach the \
         previously foreground window. Bring that window back once the target settles."
    }
}

/// The tool result for a failed foreground input call.
pub(crate) fn foreground_input_error(
    error: anyhow::Error,
) -> cua_driver_core::protocol::ToolResult {
    let kept = error.downcast_ref::<ForegroundNotRestored>().is_some();
    let result = cua_driver_core::protocol::ToolResult::error(error.to_string());
    if kept {
        result.with_structured(serde_json::json!({ "foreground_restored": false }))
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TypingTurn::*;

    const RESTORE: bool = true;
    const LEAVE: bool = false;

    /// One row per exit of `with_confirmed_foreground` once the target is
    /// confirmed in front: the turn the exit produces and the expected outcome.
    #[test]
    fn each_exit_restores_or_leaves_the_target_in_front() {
        #[rustfmt::skip]
        let rows: &[(&str, TypingTurn, bool)] = &[
            ("focus step or owned-window check fails", NotSent,   RESTORE),
            ("SendInput inserted nothing",             NotSent,   RESTORE),
            ("foreground_lost before a text unit",     Lost,      LEAVE),
            ("text unit SendInput short",              Undrained, LEAVE),
            ("text_delivery_stalled",                  Undrained, LEAVE),
            ("key chord SendInput short",              Undrained, LEAVE),
            ("unpaced text SendInput short",           Undrained, LEAVE),
            ("final drain timed out",                  Undrained, LEAVE),
            ("every text unit read",                   Drained,   RESTORE),
            ("final drain read the sentinel",          Drained,   RESTORE),
            ("no attachment, settle elapsed",          Settled,   RESTORE),
        ];
        for &(exit, turn, expected) in rows {
            assert_eq!(restores_previous(turn), expected, "{exit}");
        }
    }

    #[test]
    fn an_unrestored_turn_says_so_in_text_and_structure() {
        let stalled = anyhow::anyhow!("text_delivery_stalled: unit 3 of 5 was not read.");
        let result = foreground_input_error(
            ForegroundNotRestored {
                error: stalled,
                turn: Undrained,
            }
            .into(),
        );
        let text = serde_json::to_value(&result.content).unwrap().to_string();
        assert!(
            text.contains("text_delivery_stalled: unit 3 of 5"),
            "{text}"
        );
        assert!(
            text.contains("Focus was not restored: the target window stays in front"),
            "{text}"
        );
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({ "foreground_restored": false }))
        );

        let refused = foreground_input_error(anyhow::anyhow!("foreground_unavailable: no input"));
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(refused.structured_content, None);
    }
}
