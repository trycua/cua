//! A nudge toward `run_steps` for agents that act one call at a time.
//!
//! Every single action call costs the agent a model turn, and in recorded runs
//! a turn took 3 to 5 s while the action itself took a fraction of that.
//! `run_steps` chains the actions, stops at the first failure and returns what
//! changed. This module notices a streak of direct action calls in one session
//! and puts a one-line `hint` on the third result, and on every third after it.
//!
//! The nudge is a `hint` on a successful action result, the same field the
//! recovery hints use ([`crate::recovery_hint`]). Most clicks and keypresses
//! come back `unverifiable` with a recovery hint already, so the nudge is added
//! after it instead of replacing it. It never rides on an error, and is never
//! sent for a step that already runs inside `run_steps` or `run_script`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocol::ToolResult;

/// Direct actions in a row before the first nudge, and between nudges.
pub const STREAK_LEN: u32 = 3;

/// A pause longer than this ends a streak: the agent has stopped and started
/// again, so the next action is not one of an unbroken run.
pub const STREAK_GAP: Duration = Duration::from_secs(60);

/// Sessions tracked at once. The oldest is dropped past this, so a long-lived
/// daemon that sees many short sessions keeps a bounded table.
const MAX_SESSIONS: usize = 256;

/// The hint text. One sentence, so it costs few tokens.
pub const NUDGE: &str = "Three single actions in a row: send the next ones as one run_steps \
     call. It runs them in order, stops at the first failure and returns what changed.";

/// Action tools that count toward a streak: the ones `run_steps` can chain.
/// `move_cursor` only moves the agent cursor, so it is left out.
fn counts(tool: &str) -> bool {
    crate::batch_tools::BATCHABLE_TOOLS.contains(&tool) && tool != "move_cursor"
}

/// Tools that mean the agent already batches.
fn is_batch_tool(tool: &str) -> bool {
    #[cfg(feature = "script")]
    if tool == crate::script_tool::RUN_SCRIPT_TOOL {
        return true;
    }
    tool == crate::batch_tools::RUN_STEPS_TOOL
}

#[derive(Clone, Copy)]
struct Streak {
    count: u32,
    last: Instant,
}

/// Streaks per public session label (`""` for the implicit session).
#[derive(Default)]
struct Tracker {
    streaks: HashMap<String, Streak>,
}

impl Tracker {
    /// Record one successful direct call. Returns whether it should carry the
    /// nudge.
    fn note(&mut self, session: &str, tool: &str, now: Instant) -> bool {
        if is_batch_tool(tool) {
            self.streaks.remove(session);
            return false;
        }
        if !counts(tool) {
            return false;
        }
        let streak = self.streaks.get(session).copied();
        let count = match streak {
            Some(streak) if now.saturating_duration_since(streak.last) <= STREAK_GAP => {
                streak.count + 1
            }
            _ => 1,
        };
        if self.streaks.len() >= MAX_SESSIONS && !self.streaks.contains_key(session) {
            if let Some(oldest) = self
                .streaks
                .iter()
                .min_by_key(|(_, streak)| streak.last)
                .map(|(session, _)| session.clone())
            {
                self.streaks.remove(&oldest);
            }
        }
        self.streaks
            .insert(session.to_owned(), Streak { count, last: now });
        count % STREAK_LEN == 0
    }
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

fn note(session: &str, tool: &str, now: Instant) -> bool {
    let mut guard = TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(Tracker::default)
        .note(session, tool, now)
}

/// Add the nudge to `result` when `tool`, called directly in `session`, makes
/// a streak. Call once per tool call at the dispatch seam, with the call's
/// public `session` label and whether the call is a step of a batch.
pub fn annotate(tool: &str, session: Option<&str>, in_batch: bool, result: &mut ToolResult) {
    if in_batch || result.is_error == Some(true) {
        return;
    }
    if !note(session.unwrap_or(""), tool, Instant::now()) {
        return;
    }
    let Some(Value::Object(structured)) = result.structured_content.as_mut() else {
        return;
    };
    // Only an action result carries a hint field; a read does not.
    if !structured.contains_key("effect") {
        return;
    }
    match structured.get_mut("hint") {
        Some(Value::String(existing)) => {
            existing.push(' ');
            existing.push_str(NUDGE);
        }
        Some(_) => {}
        None => {
            structured.insert("hint".to_owned(), Value::String(NUDGE.to_owned()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn action_result() -> ToolResult {
        ToolResult::text("done")
            .with_structured(json!({ "effect": "confirmed", "route": "accessibility" }))
    }

    #[test]
    fn the_third_direct_action_in_a_row_is_nudged_and_so_is_every_third_after() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        let nudged: Vec<bool> = (0..7)
            .map(|i| tracker.note("s", "click", t0 + Duration::from_secs(i)))
            .collect();
        assert_eq!(nudged, [false, false, true, false, false, true, false]);
    }

    #[test]
    fn different_action_tools_share_a_streak() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        assert!(!tracker.note("s", "click", t0));
        assert!(!tracker.note("s", "type_text", t0));
        assert!(tracker.note("s", "press_key", t0));
    }

    #[test]
    fn reads_and_cursor_moves_neither_count_nor_break_a_streak() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        assert!(!tracker.note("s", "click", t0));
        for tool in ["get_window_state", "list_windows", "move_cursor", "zoom"] {
            assert!(!tracker.note("s", tool, t0));
        }
        assert!(!tracker.note("s", "click", t0));
        assert!(tracker.note("s", "click", t0));
    }

    #[test]
    fn using_run_steps_or_a_script_ends_the_streak() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        assert!(!tracker.note("s", "click", t0));
        assert!(!tracker.note("s", "click", t0));
        assert!(!tracker.note("s", "run_steps", t0));
        assert!(!tracker.note("s", "click", t0));
        assert!(!tracker.note("s", "click", t0));
        assert!(tracker.note("s", "click", t0));
        #[cfg(feature = "script")]
        {
            assert!(!tracker.note("s", "run_script", t0));
            assert!(!tracker.note("s", "click", t0));
        }
    }

    #[test]
    fn a_long_pause_starts_a_new_streak() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        assert!(!tracker.note("s", "click", t0));
        assert!(!tracker.note("s", "click", t0 + Duration::from_secs(1)));
        // The agent comes back after the gap: this is the first of a new run.
        let later = t0 + STREAK_GAP + Duration::from_secs(5);
        assert!(!tracker.note("s", "click", later));
        assert!(!tracker.note("s", "click", later));
        assert!(tracker.note("s", "click", later));
    }

    #[test]
    fn sessions_keep_separate_streaks() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        assert!(!tracker.note("a", "click", t0));
        assert!(!tracker.note("b", "click", t0));
        assert!(!tracker.note("a", "click", t0));
        assert!(!tracker.note("b", "click", t0));
        assert!(tracker.note("a", "click", t0));
        assert!(tracker.note("b", "click", t0));
    }

    #[test]
    fn the_session_table_stays_bounded() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        for i in 0..(MAX_SESSIONS + 50) {
            tracker.note(
                &format!("s{i}"),
                "click",
                t0 + Duration::from_millis(i as u64),
            );
        }
        assert_eq!(tracker.streaks.len(), MAX_SESSIONS);
        // The newest sessions survive; the oldest were dropped.
        assert!(tracker
            .streaks
            .contains_key(&format!("s{}", MAX_SESSIONS + 49)));
        assert!(!tracker.streaks.contains_key("s0"));
    }

    #[test]
    fn the_nudge_rides_on_a_successful_action_result() {
        let session = "nudge-annotate-success";
        let mut results: Vec<ToolResult> = (0..3).map(|_| action_result()).collect();
        for result in &mut results {
            annotate("click", Some(session), false, result);
        }
        let hint = |result: &ToolResult| {
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("hint"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        assert_eq!(hint(&results[0]), None);
        assert_eq!(hint(&results[1]), None);
        assert_eq!(hint(&results[2]).as_deref(), Some(NUDGE));
    }

    #[test]
    fn a_step_of_a_batch_is_never_nudged_or_counted() {
        let session = "nudge-annotate-batch";
        for _ in 0..6 {
            let mut result = action_result();
            annotate("click", Some(session), true, &mut result);
            assert!(result.structured_content.unwrap().get("hint").is_none());
        }
        // The six batched steps did not count: two direct calls are not yet a streak.
        for _ in 0..2 {
            let mut result = action_result();
            annotate("click", Some(session), false, &mut result);
            assert!(result.structured_content.unwrap().get("hint").is_none());
        }
    }

    #[test]
    fn an_error_is_never_nudged() {
        let session = "nudge-annotate-error";
        for _ in 0..2 {
            annotate("click", Some(session), false, &mut action_result());
        }
        let mut refused = ToolResult::error("refused")
            .with_structured(json!({ "code": "background_unavailable", "effect": "refused" }));
        annotate("click", Some(session), false, &mut refused);
        assert!(refused.structured_content.unwrap().get("hint").is_none());
    }

    #[test]
    fn the_nudge_follows_a_recovery_hint_instead_of_replacing_it() {
        let session = "nudge-annotate-append";
        let unverifiable = || {
            ToolResult::text("done").with_structured(json!({
                "effect": "unverifiable",
                "route": "synthetic_events",
                "hint": "Read what changed before repeating.",
            }))
        };
        for _ in 0..2 {
            annotate("click", Some(session), false, &mut unverifiable());
        }
        let mut third = unverifiable();
        annotate("click", Some(session), false, &mut third);
        assert_eq!(
            third.structured_content.unwrap()["hint"],
            format!("Read what changed before repeating. {NUDGE}")
        );
    }

    #[test]
    fn a_result_that_is_not_an_action_result_is_left_alone() {
        let session = "nudge-annotate-read";
        for _ in 0..2 {
            annotate("click", Some(session), false, &mut action_result());
        }
        // The third direct action has no `effect`: nothing to attach a hint to.
        let mut odd = ToolResult::text("done").with_structured(json!({ "ok": true }));
        annotate("click", Some(session), false, &mut odd);
        assert_eq!(odd.structured_content.unwrap(), json!({ "ok": true }));
    }
}
