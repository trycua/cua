//! The layer every Linux compositor adapter shares: the lock, identity,
//! delivery, freshness, motion and restore rules in [`policy`], the one foreground transaction entry point, the deadline
//! rule, the focus read-back loop and the settle loop. Adapters still own
//! their focus, restore and geometry checks.

use std::time::{Duration, Instant};

use anyhow::Result;

pub mod facts;
pub mod policy;

/// Refuse an operation of `scope` the session lock does not admit.
/// `focused` is the target's exact focus, read back right before dispatch.
pub fn admit_lock(scope: policy::Scope, focused: facts::Fact<bool>) -> Result<()> {
    let facts = facts::Facts {
        locked: facts::locked(),
        focused,
    };
    policy::lock(scope, &facts)
}

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

/// Run `body` with the exact `pid`/`window_id` pair focused. On Wayland a
/// compositor adapter confirms the pair and restores the prior focus
/// afterwards; global input is refused when no adapter can confirm it. On
/// X11 the foreground rung activates the window and leaves it active.
/// [`RestoreMode::LeaveFocused`] runs `body` as it is: it activates the
/// target itself and leaves it focused.
pub fn with_target_foreground<T>(
    pid: u32,
    window_id: u64,
    mode: RestoreMode,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    use crate::wayland::{bind_foreground_target, kwin_helper, niri, shell_helper, sway_ipc};
    if let RestoreMode::LeaveFocused = mode {
        admit_lock(
            policy::Scope::TargetBoundInput,
            facts::Fact::Unknown("the body activates the target itself".into()),
        )?;
        return body();
    }
    let body = || {
        let focused = if niri::is_session() {
            match niri::keyboard_focus() {
                Ok(focused) => facts::Fact::Known(focused == Some(window_id)),
                Err(error) => facts::Fact::Unknown(format!("{error:#}")),
            }
        } else {
            facts::Fact::Known(true)
        };
        admit_lock(policy::Scope::WindowInput, focused)?;
        body()
    };
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return crate::input::with_x11_foreground(window_id, 80, body);
    }
    let _target_guard = bind_foreground_target(pid, window_id);
    if niri::is_session() {
        return niri::with_focused_window(pid, window_id, body);
    }
    if let Some(window) = sway_ipc::window_for_id(window_id) {
        policy::identity(
            &(pid, window_id),
            &facts::Fact::Known(Some((window.pid, window.id))),
        )?;
        return sway_ipc::with_focused_container(pid, window_id, body);
    }
    if kwin_helper::trusted_window_for_id(pid, window_id).is_some() {
        policy::delivery(&facts::Fact::Known(kwin_helper::available()))?;
    }
    if shell_helper::trusted_window_for_id(pid, window_id).is_some() {
        return shell_helper::with_focused_window(pid, window_id, body);
    }
    anyhow::bail!(
        "foreground_unavailable: no trusted Wayland compositor adapter can confirm exact target window {window_id} for pid {pid}; no global keyboard input was sent"
    )
}

/// The one budget every cleanup gets once its action is over, even past
/// the action's deadline: held inputs are released first, then focus is
/// restored, all within this cap from the start of cleanup.
pub const CLEANUP_CAP: Duration = Duration::from_millis(500);

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

/// Read `confirmed` back until it holds or `deadline` passes. The callback
/// receives the same absolute deadline for its own round trips.
/// A compositor that accepts a request has not applied it: only the
/// read-back says it did.
pub fn confirm<E>(
    deadline: Instant,
    mut confirmed: impl FnMut(Instant) -> Result<bool, E>,
) -> Result<bool, E> {
    while Instant::now() < deadline {
        if confirmed(deadline)? {
            return Ok(Instant::now() < deadline);
        }
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
    Ok(false)
}

/// Read `observe` until two consecutive reads agree on `key`, a few reads a
/// fixed interval apart. Callbacks receive the action-capped deadline and
/// must bound their round trips by it; late results refuse with `unsettled`.
/// Agreeing reads do not prove an animation finished.
pub fn settled<T, K: PartialEq, E>(
    mut observe: impl FnMut(Instant) -> Result<T, E>,
    key: impl Fn(&T) -> K,
    unsettled: impl FnOnce() -> E,
) -> Result<T, E> {
    let deadline = operation_deadline(SETTLE_CAP);
    if Instant::now() >= deadline {
        return Err(unsettled());
    }
    let mut last = observe(deadline)?;
    for _ in 1..SETTLE_READS {
        if Instant::now() + SETTLE_INTERVAL > deadline {
            break;
        }
        std::thread::sleep(SETTLE_INTERVAL);
        if Instant::now() >= deadline {
            break;
        }
        let next = observe(deadline)?;
        if Instant::now() < deadline
            && policy::settled(&key(&last), &facts::Fact::Known(key(&next)))
        {
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
        let ran = with_target_foreground(1, 2, RestoreMode::LeaveFocused, || Ok("sent"));
        assert_eq!(ran.unwrap(), "sent");
    }

    #[test]
    fn confirm_reads_back_until_it_holds_and_never_starts_past_the_deadline() {
        let mut reads = 0;
        let deadline = Instant::now() + Duration::from_secs(1);
        let held = confirm(deadline, |received| {
            assert_eq!(received, deadline);
            reads += 1;
            Ok::<_, ()>(reads == 3)
        });
        assert_eq!((held, reads), (Ok(true), 3));
        let mut reads = 0;
        let expired = confirm(Instant::now(), |_| {
            reads += 1;
            Ok::<_, ()>(false)
        });
        assert_eq!((expired, reads), (Ok(false), 0));
        assert_eq!(
            confirm(Instant::now() + Duration::from_secs(1), |_| Err::<bool, _>(
                "unknown"
            )),
            Err("unknown")
        );
    }

    #[test]
    fn a_late_focus_read_back_does_not_confirm_delivery() {
        let deadline = Instant::now() + Duration::from_millis(10);
        assert_eq!(
            confirm(deadline, |received| {
                assert_eq!(received, deadline);
                std::thread::sleep(Duration::from_millis(20));
                Ok::<_, ()>(true)
            }),
            Ok(false)
        );
    }

    #[test]
    fn settle_stops_at_the_action_deadline() {
        let _bound = bind(Action::new(Duration::ZERO));
        let mut reads = 0;
        let moving = settled(
            |_| {
                reads += 1;
                Ok::<_, &str>(reads)
            },
            |read| *read,
            || "unsettled",
        );
        assert_eq!((moving, reads), (Err("unsettled"), 0));
    }

    #[test]
    fn settle_returns_the_first_of_two_agreeing_reads() {
        let mut reads = 0;
        let settling = settled(
            |_| {
                reads += 1;
                Ok::<_, &str>(reads.min(2))
            },
            |read| *read,
            || "unsettled",
        );
        assert_eq!(settling, Ok(2));
    }
}
