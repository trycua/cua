//! The decision layer every Linux compositor adapter shares. Adapters report
//! facts; this layer owns the deadline rule, the focus read-back loop and the
//! settle loop.

use std::time::{Duration, Instant};

use anyhow::Result;

/// Whether a foreground transaction gives focus back once its body ran.
#[derive(Clone, Copy)]
pub enum RestoreMode {
    RestorePrevious,
    /// The body activates the target itself and leaves it focused.
    LeaveFocused,
}

/// How a window-addressed pointer gesture (double click, scroll, drag)
/// treats focus: niri restores the prior focus, as its clicks do; other
/// compositors leave the target focused.
pub fn pointer_gesture_restore() -> RestoreMode {
    if crate::wayland::niri::is_session() {
        RestoreMode::RestorePrevious
    } else {
        RestoreMode::LeaveFocused
    }
}

/// Run `body` with the exact `pid`/`window_id` pair focused: through the
/// Wayland compositor adapter, which restores the prior focus, or the X11
/// foreground rung, which leaves the target active.
pub fn with_target_foreground<T>(
    pid: u32,
    window_id: u64,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        crate::wayland::with_target_foreground(pid, window_id, RestoreMode::RestorePrevious, body)
    } else {
        crate::input::with_x11_foreground(window_id, 80, body)
    }
}

const POLL: Duration = Duration::from_millis(10);
const SETTLE_READS: usize = 5;
const SETTLE_INTERVAL: Duration = Duration::from_millis(30);
const SETTLE_CAP: Duration = Duration::from_secs(1);

/// The deadline of an operation capped at `cap`, never past the deadline of
/// the action bound to this thread. Cleanup that must outlive the action
/// (releases, focus restore) takes `Instant::now() + cap` instead.
pub fn operation_deadline(cap: Duration) -> Instant {
    let capped = Instant::now() + cap;
    crate::input::action::current().map_or(capped, |action| capped.min(action.deadline()))
}

/// Read `confirmed` back until it holds or `deadline` passes, at least once.
/// A compositor that accepts a request has not applied it: only the
/// read-back says it did.
pub fn confirm<E>(
    deadline: Instant,
    mut confirmed: impl FnMut() -> Result<bool, E>,
) -> Result<bool, E> {
    loop {
        if confirmed()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(POLL);
    }
}

/// Read `observe` until two consecutive reads agree on `key`, a few reads a
/// fixed interval apart and never past the action's deadline, else refuse
/// with `unsettled`. Agreeing reads do not prove an animation finished.
pub fn settled<T, K: PartialEq, E>(
    mut observe: impl FnMut() -> Result<T, E>,
    key: impl Fn(&T) -> K,
    unsettled: impl FnOnce() -> E,
) -> Result<T, E> {
    let deadline = operation_deadline(SETTLE_CAP);
    let mut last = observe()?;
    for _ in 1..SETTLE_READS {
        if Instant::now() + SETTLE_INTERVAL > deadline {
            break;
        }
        std::thread::sleep(SETTLE_INTERVAL);
        let next = observe()?;
        if key(&next) == key(&last) {
            return Ok(next);
        }
        last = next;
    }
    Err(unsettled())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::action::{bind, Action};

    #[test]
    fn the_operation_deadline_never_passes_the_bound_action() {
        let unbound = operation_deadline(Duration::from_secs(5));
        assert!(unbound > Instant::now() + Duration::from_secs(4));
        let action = Action::new(Duration::from_millis(20));
        let _bound = bind(action.clone());
        assert!(operation_deadline(Duration::from_secs(5)) <= action.deadline());
    }

    #[test]
    fn leave_focused_runs_the_body_without_a_compositor_transaction() {
        let ran =
            crate::wayland::with_target_foreground(1, 2, RestoreMode::LeaveFocused, || Ok("sent"));
        assert_eq!(ran.unwrap(), "sent");
    }

    #[test]
    fn confirm_reads_back_until_it_holds_and_reads_once_past_the_deadline() {
        let mut reads = 0;
        let held = confirm(Instant::now() + Duration::from_secs(1), || {
            reads += 1;
            Ok::<_, ()>(reads == 3)
        });
        assert_eq!((held, reads), (Ok(true), 3));
        let mut reads = 0;
        let expired = confirm(Instant::now(), || {
            reads += 1;
            Ok::<_, ()>(false)
        });
        assert_eq!((expired, reads), (Ok(false), 1));
        assert_eq!(
            confirm(Instant::now(), || Err::<bool, _>("unknown")),
            Err("unknown")
        );
    }

    #[test]
    fn settle_stops_at_the_action_deadline() {
        let _bound = bind(Action::new(Duration::ZERO));
        let mut reads = 0;
        let moving = settled(
            || {
                reads += 1;
                Ok::<_, &str>(reads)
            },
            |read| *read,
            || "unsettled",
        );
        assert_eq!((moving, reads), (Err("unsettled"), 1));
    }

    #[test]
    fn settle_returns_the_first_of_two_agreeing_reads() {
        let mut reads = 0;
        let settling = settled(
            || {
                reads += 1;
                Ok::<_, &str>(reads.min(2))
            },
            |read| *read,
            || "unsettled",
        );
        assert_eq!(settling, Ok(2));
    }
}
