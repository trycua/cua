// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Wall-clock and node budget for the accessibility walk behind
//! `get_window_state` (`timeout_ms` / `max_elements`).
//!
//! Every backend walks its own accessibility API (AT-SPI, AX, UIA/MSAA), but
//! the contract is shared: the walk stops at whichever budget runs out first,
//! the tool returns the PARTIAL tree it has, and the response says so with the
//! same structured fields and the same note on every platform.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Why a walk stopped before it had visited every node it discovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkStop {
    /// The `timeout_ms` wall-clock budget ran out.
    Timeout,
    /// The `max_elements` node budget ran out.
    NodeBudget,
    /// The application could not be reached through the platform
    /// accessibility API before `timeout_ms` ran out (still launching, or not
    /// yet registered), so nothing was walked.
    AppLookupTimeout,
}

impl WalkStop {
    pub fn as_str(self) -> &'static str {
        match self {
            WalkStop::Timeout => "timeout",
            WalkStop::NodeBudget => "node_budget",
            WalkStop::AppLookupTimeout => "app_lookup_timeout",
        }
    }
}

/// Budget for one walk. Call [`WalkBudget::admit`] before visiting each node;
/// a refused node is counted as discovered but not visited.
///
/// The clock starts at the first admitted node, so `timeout_ms` bounds the
/// walk itself rather than the backend's setup (resolving the application and
/// the target window, which on a freshly launched app can take most of a
/// second on a slow host). Each backend bounds that setup separately with its
/// own backstop past `timeout_ms`.
#[derive(Debug)]
pub struct WalkBudget {
    /// Set by the first [`WalkBudget::admit`].
    started: Option<Instant>,
    timeout: Duration,
    timeout_ms: u64,
    /// False for [`WalkBudget::nodes_only`], whose clock never fires.
    time_bounded: bool,
    max_elements: usize,
    visited: usize,
    /// Nodes held back by [`WalkBudget::reserve`] for siblings not yet walked.
    reserved: usize,
    pending: usize,
    stop: Option<WalkStop>,
}

impl WalkBudget {
    /// A walk bounded by `timeout_ms` of wall-clock time and `max_elements`
    /// nodes.
    pub fn new(timeout_ms: u64, max_elements: usize) -> Self {
        Self {
            started: None,
            timeout: Duration::from_millis(timeout_ms),
            timeout_ms,
            time_bounded: true,
            max_elements,
            visited: 0,
            reserved: 0,
            pending: 0,
            stop: None,
        }
    }

    /// A walk bounded only by `max_elements` (internal callers with no
    /// caller-supplied time budget).
    pub fn nodes_only(max_elements: usize) -> Self {
        // A year: never fires, without overflowing Instant.
        Self {
            time_bounded: false,
            ..Self::new(365 * 24 * 60 * 60 * 1000, max_elements)
        }
    }

    /// The caller's wall-clock budget, or `None` for a node-only budget.
    /// Backends use it to bound setup that happens before the first node,
    /// such as waiting for a launching application to become reachable.
    pub fn time_limit(&self) -> Option<Duration> {
        self.time_bounded.then_some(self.timeout)
    }

    /// Admit one node for visiting. `false` means the walk must not visit it:
    /// a budget has run out, and the node is counted as pending.
    pub fn admit(&mut self) -> bool {
        let started = *self.started.get_or_insert_with(Instant::now);
        // Running out of time ends the walk, and the reason says so even
        // after a node-budget refusal. Running out of nodes refuses only
        // this node: a sibling may still claim nodes reserved for it.
        let refused = if matches!(
            self.stop,
            Some(WalkStop::Timeout | WalkStop::AppLookupTimeout)
        ) {
            true
        } else if started.elapsed() >= self.timeout {
            self.stop = Some(WalkStop::Timeout);
            true
        } else if self.visited + self.reserved >= self.max_elements {
            self.stop.get_or_insert(WalkStop::NodeBudget);
            true
        } else {
            false
        };
        if refused {
            self.pending += 1;
            return false;
        }
        self.visited += 1;
        true
    }

    /// Hold back up to `count` nodes of the unreserved node budget and return
    /// how many were held. A walker holds nodes for the siblings it has not
    /// walked yet and [`WalkBudget::release`]s each sibling's share before
    /// walking it, so a deep early sibling cannot use up the nodes the later
    /// siblings need to be listed at all.
    pub fn reserve(&mut self, count: usize) -> usize {
        let free = self
            .max_elements
            .saturating_sub(self.visited + self.reserved);
        let held = count.min(free);
        self.reserved += held;
        held
    }

    /// Return `count` nodes held by [`WalkBudget::reserve`] to the budget.
    pub fn release(&mut self, count: usize) {
        self.reserved = self.reserved.saturating_sub(count);
    }

    /// Whether the wall-clock budget has run out (for phases that cannot be
    /// interrupted per node). Never true before the walk has started.
    pub fn expired(&self) -> bool {
        self.started
            .is_some_and(|started| started.elapsed() >= self.timeout)
    }

    /// Record that the walk ran out of time outside [`WalkBudget::admit`]
    /// (a bulk fetch that returned after the deadline).
    pub fn stop_for_timeout(&mut self) {
        self.stop.get_or_insert(WalkStop::Timeout);
    }

    /// Record that the application never became reachable within the budget,
    /// so the walk visited nothing.
    pub fn stop_for_app_lookup_timeout(&mut self) {
        self.stop.get_or_insert(WalkStop::AppLookupTimeout);
    }

    pub fn outcome(&self) -> WalkOutcome {
        WalkOutcome {
            timeout_ms: self.timeout_ms,
            stop: self.stop,
            nodes_visited: self.visited,
            nodes_pending: self.pending,
            elapsed_ms: self
                .started
                .map_or(0, |started| started.elapsed().as_millis() as u64),
        }
    }
}

/// What a finished walk reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkOutcome {
    pub timeout_ms: u64,
    pub stop: Option<WalkStop>,
    pub nodes_visited: usize,
    pub nodes_pending: usize,
    pub elapsed_ms: u64,
}

impl WalkOutcome {
    /// A walk that produced nothing because the whole budget ran out before
    /// the backend returned (an uninterruptible provider call).
    pub fn timed_out(timeout_ms: u64, elapsed: Duration) -> Self {
        Self {
            timeout_ms,
            stop: Some(WalkStop::Timeout),
            nodes_visited: 0,
            nodes_pending: 0,
            elapsed_ms: elapsed.as_millis() as u64,
        }
    }

    pub fn truncated(&self) -> bool {
        self.stop.is_some()
    }

    pub fn reason(&self) -> Option<&'static str> {
        self.stop.map(WalkStop::as_str)
    }

    /// Write the shared walk fields into a `get_window_state` payload.
    pub fn apply(&self, structured: &mut Value) {
        structured["truncated"] = json!(self.truncated());
        if let Some(reason) = self.reason() {
            structured["truncation_reason"] = json!(reason);
        }
        structured["nodes_visited"] = json!(self.nodes_visited);
        structured["nodes_pending"] = json!(self.nodes_pending);
        structured["walk_elapsed_ms"] = json!(self.elapsed_ms);
        structured["timeout_ms"] = json!(self.timeout_ms);
    }

    /// The PARTIAL TREE note for the tree text, when the walk was cut short.
    pub fn note(&self) -> Option<String> {
        self.truncated().then(|| {
            truncation_note(
                self.reason(),
                self.timeout_ms,
                self.nodes_visited,
                self.nodes_pending,
            )
        })
    }
}

/// The note that heads a partial tree, naming why the walk stopped and the
/// retry that can get past that stop. Callers follow the remedy literally, so
/// it must name the knob that lifts this cut and no other.
pub fn truncation_note(
    reason: Option<&str>,
    timeout_ms: u64,
    visited: usize,
    pending: usize,
) -> String {
    const NARROW: &str = "or narrow with query / max_depth";
    let (why, remedy) = match reason {
        Some("timeout") => (
            format!("the {timeout_ms} ms timeout_ms budget ran out"),
            format!("retry with a larger timeout_ms (e.g. 5000) {NARROW}"),
        ),
        Some("node_budget") => (
            "the max_elements node budget ran out".to_owned(),
            format!("retry with a larger max_elements {NARROW}"),
        ),
        Some("app_unresponsive") => (
            "the application stopped answering its accessibility API".to_owned(),
            "retry in a moment or with a larger timeout_ms (e.g. 5000)".to_owned(),
        ),
        Some("app_lookup_timeout") => (
            format!(
                "the application was not reachable through its accessibility API (AT-SPI, AX) \
                 within {timeout_ms} ms; it may still be launching"
            ),
            "retry in a moment or with a larger timeout_ms (e.g. 5000)".to_owned(),
        ),
        Some("children_unread") => (
            "a container's children could not be read from its accessibility API".to_owned(),
            "retry in a moment or with a larger timeout_ms (e.g. 5000)".to_owned(),
        ),
        Some("huge_container") => (
            "a container with more children than can be enumerated \
             (e.g. a spreadsheet's cell grid) was not expanded"
                .to_owned(),
            "retry with a max_elements above that container's child count, or act inside it \
             by pixel (x,y) off the screenshot"
                .to_owned(),
        ),
        other => (
            other.unwrap_or("the walk stopped early").to_owned(),
            format!("retry with a larger timeout_ms (e.g. 5000) {NARROW}"),
        ),
    };
    format!(
        "⚠️ PARTIAL TREE: {why} after {visited} node(s) ({pending} discovered but not visited). \
         Every element listed is real; elements after the cut are missing. If the element you \
         need is absent, {remedy}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_budget_stops_and_counts_pending() {
        let mut budget = WalkBudget::nodes_only(2);
        assert!(budget.admit());
        assert!(budget.admit());
        assert!(!budget.admit());
        assert!(!budget.admit());
        let outcome = budget.outcome();
        assert_eq!(outcome.reason(), Some("node_budget"));
        assert_eq!((outcome.nodes_visited, outcome.nodes_pending), (2, 2));
    }

    #[test]
    fn a_reserved_node_survives_a_deep_earlier_sibling() {
        // A parent and three siblings fill a budget of 4 exactly; the first
        // sibling has a deep subtree.
        let mut budget = WalkBudget::nodes_only(4);
        assert!(budget.admit(), "parent");
        assert_eq!(budget.reserve(3), 3);
        budget.release(1);
        assert!(budget.admit(), "first sibling");
        assert!(!budget.admit(), "its subtree stops at the held nodes");
        budget.release(1);
        assert!(budget.admit(), "second sibling");
        budget.release(1);
        assert!(budget.admit(), "third sibling");
        assert!(!budget.admit());
        let outcome = budget.outcome();
        assert_eq!(outcome.reason(), Some("node_budget"));
        assert_eq!((outcome.nodes_visited, outcome.nodes_pending), (4, 2));
    }

    #[test]
    fn a_timeout_after_a_node_budget_refusal_is_reported_as_timeout() {
        // Refused for nodes, then past the deadline: with the held node
        // given back, and with the budget still full.
        for give_back in [true, false] {
            let mut budget = WalkBudget::new(30, 2);
            assert!(budget.admit());
            assert_eq!(budget.reserve(1), 1);
            assert!(!budget.admit(), "held node refuses the deep child");
            std::thread::sleep(Duration::from_millis(40));
            if give_back {
                budget.release(1);
            }
            assert!(!budget.admit(), "the deadline has passed");
            assert_eq!(budget.outcome().reason(), Some("timeout"));
        }
    }

    #[test]
    fn a_reservation_never_exceeds_the_free_budget() {
        let mut budget = WalkBudget::nodes_only(3);
        assert!(budget.admit());
        assert_eq!(budget.reserve(10), 2);
        assert_eq!(budget.reserve(1), 0);
        budget.release(1);
        budget.release(1);
        budget.release(1);
        assert!(budget.admit());
        assert!(budget.admit());
        assert!(!budget.admit());
    }

    #[test]
    fn only_a_caller_budget_bounds_setup_waits() {
        assert_eq!(
            WalkBudget::new(1500, 10).time_limit(),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(WalkBudget::nodes_only(10).time_limit(), None);
    }

    #[test]
    fn an_unreachable_application_reports_app_lookup_timeout() {
        let mut budget = WalkBudget::new(1000, 10);
        budget.stop_for_app_lookup_timeout();
        let outcome = budget.outcome();
        assert!(outcome.truncated());
        assert_eq!(outcome.reason(), Some("app_lookup_timeout"));
        assert_eq!((outcome.nodes_visited, outcome.nodes_pending), (0, 0));
        let mut structured = json!({});
        outcome.apply(&mut structured);
        assert_eq!(structured["truncated"], true);
        assert_eq!(structured["truncation_reason"], "app_lookup_timeout");
        let note = outcome.note().unwrap();
        assert!(note.contains("not reachable through its accessibility API"));
        assert!(note.contains("1000 ms"));
    }

    #[test]
    fn expired_deadline_stops_with_timeout() {
        let mut budget = WalkBudget::new(0, 100);
        // Setup before the first node does not count against the budget.
        assert!(!budget.expired());
        assert!(!budget.admit());
        assert!(budget.expired());
        let outcome = budget.outcome();
        assert_eq!(outcome.reason(), Some("timeout"));
        assert_eq!((outcome.nodes_visited, outcome.nodes_pending), (0, 1));
    }

    #[test]
    fn the_clock_starts_at_the_first_node() {
        let mut budget = WalkBudget::new(50, 100);
        std::thread::sleep(Duration::from_millis(80));
        assert!(budget.admit(), "setup time must not exhaust the budget");
    }

    #[test]
    fn a_complete_walk_is_not_truncated() {
        let mut budget = WalkBudget::new(60_000, 10);
        for _ in 0..10 {
            assert!(budget.admit());
        }
        let outcome = budget.outcome();
        assert!(!outcome.truncated());
        assert_eq!(outcome.note(), None);
        let mut structured = json!({});
        outcome.apply(&mut structured);
        assert_eq!(structured["truncated"], false);
        assert!(structured.get("truncation_reason").is_none());
        assert_eq!(structured["nodes_visited"], 10);
        assert_eq!(structured["timeout_ms"], 60_000);
    }

    #[test]
    fn a_timed_out_walk_reports_the_budget() {
        let outcome = WalkOutcome::timed_out(1000, Duration::from_millis(1200));
        let mut structured = json!({});
        outcome.apply(&mut structured);
        assert_eq!(structured["truncated"], true);
        assert_eq!(structured["truncation_reason"], "timeout");
        let note = outcome.note().unwrap();
        assert!(note.starts_with("⚠️ PARTIAL TREE: the 1000 ms timeout_ms budget ran out"));
    }

    /// The advice after "If the element you need is absent,".
    fn remedy(note: &str) -> &str {
        note.split_once("If the element you need is absent, ")
            .expect("note carries a remedy")
            .1
    }

    #[test]
    fn truncation_note_names_the_budget_and_the_counts() {
        let note = truncation_note(Some("timeout"), 1000, 240, 88);
        assert!(note.starts_with("⚠️ PARTIAL TREE: the 1000 ms timeout_ms budget ran out"));
        assert!(note.contains("240 node(s)"));
        assert!(note.contains("88 discovered"));
        let note = truncation_note(Some("app_unresponsive"), 1000, 3, 0);
        assert!(note.contains("stopped answering"));
        let note = truncation_note(Some("app_lookup_timeout"), 250, 0, 0);
        assert!(note.contains("250 ms"));
        assert!(note.contains("still be launching"));
        let note = truncation_note(Some("huge_container"), 1000, 1918, 0);
        assert!(note.contains("not expanded"));
        let note = truncation_note(Some("children_unread"), 1000, 12, 0);
        assert!(note.contains("children could not be read"));
    }

    /// Each cut names the knob that lifts it, and never one that cannot: a
    /// caller told to raise timeout_ms after a max_elements cut gets the same
    /// partial tree back (#4720).
    #[test]
    fn truncation_note_remedy_matches_the_cut() {
        let cases: [(Option<&str>, &str, &[&str]); 7] = [
            (
                Some("timeout"),
                "retry with a larger timeout_ms (e.g. 5000) or narrow with query / max_depth.",
                &["max_elements"],
            ),
            (
                Some("node_budget"),
                "retry with a larger max_elements or narrow with query / max_depth.",
                &["timeout_ms"],
            ),
            (
                Some("app_unresponsive"),
                "retry in a moment or with a larger timeout_ms (e.g. 5000).",
                &["max_elements", "query"],
            ),
            (
                Some("app_lookup_timeout"),
                "retry in a moment or with a larger timeout_ms (e.g. 5000).",
                &["max_elements", "query"],
            ),
            (
                Some("children_unread"),
                "retry in a moment or with a larger timeout_ms (e.g. 5000).",
                &["max_elements", "query"],
            ),
            (
                Some("huge_container"),
                "retry with a max_elements above that container's child count, or act inside it \
                 by pixel (x,y) off the screenshot.",
                &["timeout_ms", "query"],
            ),
            (
                None,
                "retry with a larger timeout_ms (e.g. 5000) or narrow with query / max_depth.",
                &["max_elements"],
            ),
        ];
        for (reason, expected, wrong_knobs) in cases {
            let note = truncation_note(reason, 1000, 20, 15);
            let advice = remedy(&note);
            assert_eq!(advice, expected, "{reason:?}");
            for knob in wrong_knobs {
                assert!(
                    !advice.contains(knob),
                    "{reason:?} advises {knob}: {advice}"
                );
            }
        }
    }

    #[test]
    fn a_max_elements_cut_advises_max_elements_not_timeout_ms() {
        let mut budget = WalkBudget::new(1000, 20);
        for _ in 0..20 {
            assert!(budget.admit());
        }
        for _ in 0..15 {
            assert!(!budget.admit());
        }
        let note = budget.outcome().note().unwrap();
        assert_eq!(
            note,
            "⚠️ PARTIAL TREE: the max_elements node budget ran out after 20 node(s) \
             (15 discovered but not visited). Every element listed is real; elements after \
             the cut are missing. If the element you need is absent, retry with a larger \
             max_elements or narrow with query / max_depth."
        );
    }
}
