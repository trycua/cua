//! Waiting for a launching application before its first AX walk.
//!
//! AppKit creates an `NSWindow` in WindowServer as soon as the app constructs
//! it, so `list_windows` (and `launch_app`) can report the window while the
//! application has not yet entered its run loop. Until it does, every AX
//! request to the application element fails at once with
//! `kAXErrorCannotComplete`, and a window-scoped walk finds zero `AXWindow`
//! elements. Without a wait, the first `get_window_state` of a slow-launching
//! app returned an empty tree that blamed the window scope.
//!
//! The walker retries window resolution while AppKit reports the process as
//! still launching, bounded by the caller's `timeout_ms`. A process that has
//! finished launching, or that LaunchServices does not know, is never waited
//! on, so an ordinary unresolved window still fails fast.

use std::time::{Duration, Instant};

/// Interval between window-resolution attempts while an app launches.
const LAUNCH_POLL: Duration = Duration::from_millis(50);

/// Whether AppKit reports `pid` as an application that has not finished
/// launching. `false` for finished apps and for processes LaunchServices
/// does not know.
pub fn is_still_launching(pid: i32) -> bool {
    use objc2_app_kit::NSRunningApplication;
    // SAFETY: both calls are plain Objective-C messages on a retained
    // NSRunningApplication; neither requires the main thread.
    unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .is_some_and(|app| !app.isFinishedLaunching())
    }
}

/// What the walker does after a window-scoped resolution found nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchStep {
    /// Report the resolution as it is.
    Proceed,
    /// The app is still launching and the budget allows another attempt.
    Retry(Duration),
    /// The app is still launching and the budget ran out while waiting.
    TimedOut,
}

/// The bounded wait for one walk.
///
/// The deadline is anchored at the start of the first unresolved attempt, so
/// an app that resolves immediately pays nothing and the time an attempt
/// itself takes counts against the budget. An app whose AX server is
/// registered but busy answers each request only after the messaging timeout;
/// a retry is refused once another attempt of the same length would overrun
/// the deadline, so the wait never extends a slow walk by a whole attempt.
#[derive(Debug)]
pub struct LaunchWait {
    limit: Option<Duration>,
    deadline: Option<Instant>,
}

impl LaunchWait {
    /// `limit` is the caller's wall-clock budget; `None` (an internal,
    /// node-only walk) never waits.
    pub fn new(limit: Option<Duration>) -> Self {
        Self {
            limit,
            deadline: None,
        }
    }

    /// Decide the next step after an unresolved attempt that started at
    /// `attempt_started` and ended at `now`. `still_launching` is only
    /// consulted when the budget allows a wait.
    pub fn step(
        &mut self,
        attempt_started: Instant,
        now: Instant,
        still_launching: impl FnOnce() -> bool,
    ) -> LaunchStep {
        let Some(limit) = self.limit else {
            return LaunchStep::Proceed;
        };
        if !still_launching() {
            return LaunchStep::Proceed;
        }
        let deadline = *self.deadline.get_or_insert(attempt_started + limit);
        let attempt = now.saturating_duration_since(attempt_started);
        match deadline
            .checked_duration_since(now)
            .and_then(|remaining| remaining.checked_sub(attempt))
        {
            Some(slack) if !slack.is_zero() => LaunchStep::Retry(slack.min(LAUNCH_POLL)),
            _ => LaunchStep::TimedOut,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: Duration = Duration::from_millis(5);

    #[test]
    fn a_node_only_walk_never_waits_or_asks_about_launch_state() {
        let mut wait = LaunchWait::new(None);
        let now = Instant::now();
        let step = wait.step(now, now, || {
            panic!("a node-only walk must not query launch state")
        });
        assert_eq!(step, LaunchStep::Proceed);
    }

    #[test]
    fn a_finished_app_fails_fast() {
        let mut wait = LaunchWait::new(Some(Duration::from_secs(1)));
        let start = Instant::now();
        assert_eq!(
            wait.step(start, start + FAST, || false),
            LaunchStep::Proceed
        );
        // No deadline was armed: a later launching attempt gets the whole budget.
        let later = start + Duration::from_secs(5);
        assert_eq!(
            wait.step(later, later + FAST, || true),
            LaunchStep::Retry(LAUNCH_POLL)
        );
    }

    #[test]
    fn a_launching_app_is_retried_until_the_budget_runs_out() {
        let start = Instant::now();
        let mut wait = LaunchWait::new(Some(Duration::from_millis(120)));
        assert_eq!(
            wait.step(start, start + FAST, || true),
            LaunchStep::Retry(LAUNCH_POLL)
        );
        let late = start + Duration::from_millis(95);
        assert_eq!(
            wait.step(late, late + FAST, || true),
            LaunchStep::Retry(Duration::from_millis(15)),
            "the last pause leaves room for one more attempt of the same length"
        );
        let last = start + Duration::from_millis(115);
        assert_eq!(wait.step(last, last + FAST, || true), LaunchStep::TimedOut);
        let past = start + Duration::from_millis(500);
        assert_eq!(wait.step(past, past + FAST, || true), LaunchStep::TimedOut);
    }

    #[test]
    fn a_busy_launch_is_not_retried_past_its_budget() {
        // AX is registered but the app is blocked, so each attempt waits out
        // the messaging timeout. The first attempt already spent the budget.
        let start = Instant::now();
        let mut wait = LaunchWait::new(Some(Duration::from_secs(1)));
        assert_eq!(
            wait.step(start, start + Duration::from_secs(4), || true),
            LaunchStep::TimedOut
        );
        // A slow attempt that fits once but not twice is not repeated.
        let mut wait = LaunchWait::new(Some(Duration::from_secs(5)));
        assert_eq!(
            wait.step(start, start + Duration::from_secs(3), || true),
            LaunchStep::TimedOut
        );
    }

    #[test]
    fn an_app_that_finishes_launching_mid_wait_proceeds() {
        let start = Instant::now();
        let mut wait = LaunchWait::new(Some(Duration::from_secs(1)));
        assert_eq!(
            wait.step(start, start + FAST, || true),
            LaunchStep::Retry(LAUNCH_POLL)
        );
        let later = start + Duration::from_millis(300);
        assert_eq!(
            wait.step(later, later + FAST, || false),
            LaunchStep::Proceed
        );
    }

    #[test]
    fn the_current_process_is_not_a_launching_app() {
        // A test binary is not a LaunchServices application, so it must never
        // be waited on.
        assert!(!is_still_launching(std::process::id() as i32));
        assert!(!is_still_launching(0));
    }
}
