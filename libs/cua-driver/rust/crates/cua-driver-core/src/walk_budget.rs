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
}

impl WalkStop {
    pub fn as_str(self) -> &'static str {
        match self {
            WalkStop::Timeout => "timeout",
            WalkStop::NodeBudget => "node_budget",
        }
    }
}

/// Budget for one walk. Call [`WalkBudget::admit`] before visiting each node;
/// a refused node is counted as discovered but not visited.
#[derive(Debug)]
pub struct WalkBudget {
    started: Instant,
    deadline: Instant,
    timeout_ms: u64,
    max_elements: usize,
    visited: usize,
    pending: usize,
    stop: Option<WalkStop>,
}

impl WalkBudget {
    /// A walk bounded by `timeout_ms` of wall-clock time and `max_elements`
    /// nodes.
    pub fn new(timeout_ms: u64, max_elements: usize) -> Self {
        let started = Instant::now();
        Self {
            started,
            deadline: started + Duration::from_millis(timeout_ms),
            timeout_ms,
            max_elements,
            visited: 0,
            pending: 0,
            stop: None,
        }
    }

    /// A walk bounded only by `max_elements` (internal callers with no
    /// caller-supplied time budget).
    pub fn nodes_only(max_elements: usize) -> Self {
        // A year: never fires, without overflowing Instant.
        Self::new(365 * 24 * 60 * 60 * 1000, max_elements)
    }

    /// Admit one node for visiting. `false` means the walk must not visit it:
    /// a budget has run out, and the node is counted as pending.
    pub fn admit(&mut self) -> bool {
        if self.stop.is_none() {
            if self.visited >= self.max_elements {
                self.stop = Some(WalkStop::NodeBudget);
            } else if Instant::now() >= self.deadline {
                self.stop = Some(WalkStop::Timeout);
            }
        }
        if self.stop.is_some() {
            self.pending += 1;
            return false;
        }
        self.visited += 1;
        true
    }

    /// Whether the wall-clock budget has run out (for phases that cannot be
    /// interrupted per node, e.g. deciding whether to retry a bulk fetch).
    pub fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// Time left before the deadline (zero once it has passed).
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Record that the walk ran out of time outside [`WalkBudget::admit`]
    /// (a bulk fetch that returned after the deadline).
    pub fn stop_for_timeout(&mut self) {
        self.stop.get_or_insert(WalkStop::Timeout);
    }

    pub fn outcome(&self) -> WalkOutcome {
        WalkOutcome {
            timeout_ms: self.timeout_ms,
            stop: self.stop,
            nodes_visited: self.visited,
            nodes_pending: self.pending,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
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

/// The note that heads a partial tree, naming why the walk stopped.
pub fn truncation_note(
    reason: Option<&str>,
    timeout_ms: u64,
    visited: usize,
    pending: usize,
) -> String {
    let why = match reason {
        Some("timeout") => format!("the {timeout_ms} ms timeout_ms budget ran out"),
        Some("node_budget") => "the max_elements node budget ran out".to_owned(),
        Some("app_unresponsive") => {
            "the application stopped answering its accessibility API".to_owned()
        }
        Some("app_lookup_timeout") => {
            format!("the application did not register with AT-SPI within {timeout_ms} ms")
        }
        Some("huge_container") => "a container with more children than can be enumerated \
            (e.g. a spreadsheet's cell grid) was not expanded"
            .to_owned(),
        Some(other) => other.to_owned(),
        None => "the walk stopped early".to_owned(),
    };
    format!(
        "⚠️ PARTIAL TREE: {why} after {visited} node(s) ({pending} discovered but not visited). \
         Every element listed is real; elements after the cut are missing. If the element you \
         need is absent, retry with a larger timeout_ms (e.g. 5000) or narrow with query / max_depth."
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
    fn expired_deadline_stops_with_timeout() {
        let mut budget = WalkBudget::new(0, 100);
        assert!(budget.expired());
        assert!(!budget.admit());
        let outcome = budget.outcome();
        assert_eq!(outcome.reason(), Some("timeout"));
        assert_eq!((outcome.nodes_visited, outcome.nodes_pending), (0, 1));
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

    #[test]
    fn truncation_note_names_the_budget_and_the_remedy() {
        let note = truncation_note(Some("timeout"), 1000, 240, 88);
        assert!(note.contains("PARTIAL TREE"));
        assert!(note.contains("1000 ms"));
        assert!(note.contains("240 node(s)"));
        assert!(note.contains("88 discovered"));
        assert!(note.contains("timeout_ms"));
        assert!(note.contains("query"));
        let note = truncation_note(Some("node_budget"), 1000, 5000, 3);
        assert!(note.contains("max_elements"));
        let note = truncation_note(Some("app_unresponsive"), 1000, 3, 0);
        assert!(note.contains("stopped answering"));
        let note = truncation_note(Some("app_lookup_timeout"), 250, 0, 0);
        assert!(note.contains("250 ms"));
        let note = truncation_note(Some("huge_container"), 1000, 1918, 0);
        assert!(note.contains("not expanded"));
    }
}
