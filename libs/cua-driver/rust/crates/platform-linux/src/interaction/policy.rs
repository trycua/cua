//! What each primitive decides from the facts an adapter reports. Every
//! decision states what an unknown fact means for each scope.

use anyhow::{bail, Result};

use super::facts::{Fact, Facts};

/// What an operation addresses.
#[derive(Clone, Copy)]
pub enum Scope {
    WindowInput,
    /// Window input whose route activates or addresses the target itself,
    /// without the shared transaction's focus read-back.
    TargetBoundInput,
    DesktopInput,
    Capture,
}

/// Whether the session lock admits the operation. A known lock refuses
/// everything. An unknown lock admits window input only when the target's
/// exact focus was read back as `Known(true)`, and admits target-bound input, desktop input
/// and capture on the adapter's other checks, as before this rule existed.
pub fn lock(scope: Scope, facts: &Facts) -> Result<()> {
    match (&facts.locked, scope) {
        (Fact::Known(true), _) => {
            bail!("screen_locked: the session screensaver reports itself active, so nothing was sent or captured")
        }
        (Fact::Known(false), _)
        | (
            Fact::Unknown(_),
            Scope::TargetBoundInput | Scope::DesktopInput | Scope::Capture,
        ) => Ok(()),
        (Fact::Unknown(_), Scope::WindowInput) if matches!(facts.focused, Fact::Known(true)) => {
            Ok(())
        }
        (Fact::Unknown(why), Scope::WindowInput) => bail!(
            "foreground_unavailable: the lock state is unknown ({why}) and the target's focus was not read back"
        ),
    }
}

/// Require an exact observed identity. Missing, mismatched and unknown
/// identities refuse; focus or geometry cannot substitute for ownership.
pub fn identity<T: PartialEq>(expected: &T, observed: &Fact<Option<T>>) -> Result<()> {
    match observed {
        Fact::Known(Some(actual)) if actual == expected => Ok(()),
        Fact::Known(_) => {
            bail!("foreground_unavailable: the exact target identity was not observed")
        }
        Fact::Unknown(why) => bail!("foreground_unavailable: target identity is unknown ({why})"),
    }
}

/// Whether the adapter's input transport can bind delivery to the confirmed
/// target. A transport that only reaches whatever surface is focused when
/// the compositor processes the event refuses, as does an unknown one;
/// more focus checks before dispatch do not bind the input.
pub fn delivery(target_bound: &Fact<bool>) -> Result<()> {
    match target_bound {
        Fact::Known(true) => Ok(()),
        Fact::Known(false) => bail!(
            "foreground_unavailable: the target window is verified, but this compositor offers only focus-bound input; refusing raw input until a target-bound input path exists"
        ),
        Fact::Unknown(why) => {
            bail!("foreground_unavailable: the input binding is unknown ({why}); refusing raw input")
        }
    }
}

/// An exact focus read-back permits delivery. A known different or absent
/// focus needs activation and another read-back; unknown focus cannot
/// confirm delivery. Command acceptance alone is not a focus fact.
pub fn focused<T: PartialEq>(expected: &T, observed: &Fact<Option<T>>) -> bool {
    identity(expected, observed).is_ok()
}

/// Whether an admitted observation still holds. No adapter counts layout
/// events yet, so this is the weaker snapshot check: the current geometry
/// equals the admitted one, which cannot see a move away and back between
/// the two reads. A changed or unknown current geometry is stale.
pub fn fresh<G: PartialEq>(admitted: &G, current: &Fact<G>) -> bool {
    matches!(current, Fact::Known(current) if current == admitted)
}

/// What an adapter observed about one point of window-coordinate input
/// against the target window's current geometry.
pub struct Point {
    /// The window still has the geometry the point was computed on.
    pub fresh: bool,
    /// The point lies on the window.
    pub on_window: bool,
    /// The output shows the window at the point.
    pub shown: bool,
    /// No other window may be above the window at the point.
    pub unobscured: bool,
}

/// Why a point of window-coordinate input is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    Stale,
    OffWindow,
    Hidden,
    Obscured,
}

/// Whether a point reaches the target window: it must have been computed on
/// the window's current geometry, lie on the window, be shown, and not be
/// possibly covered by another window. The first fact that fails, in that
/// order, is the refusal.
pub fn reach(point: &Point) -> Result<(), Miss> {
    match point {
        Point { fresh: false, .. } => Err(Miss::Stale),
        Point {
            on_window: false, ..
        } => Err(Miss::OffWindow),
        Point { shown: false, .. } => Err(Miss::Hidden),
        Point {
            unobscured: false, ..
        } => Err(Miss::Obscured),
        _ => Ok(()),
    }
}

/// Whether observable geometry stopped moving: two consecutive reads agree.
/// A read that differs, or one that could not be taken, is still moving;
/// agreeing reads do not prove a rendered animation finished.
pub fn settled<K: PartialEq>(last: &K, next: &Fact<K>) -> bool {
    matches!(next, Fact::Known(next) if next == last)
}

/// The saved focus a `RestorePrevious` transaction gives back. Without a
/// valid saved recipe, or with an unknown one, the transaction does not
/// begin, so nothing is focused that could not be restored.
pub fn restore<R>(saved: Fact<Option<R>>) -> Result<R> {
    match saved {
        Fact::Known(Some(recipe)) => Ok(recipe),
        Fact::Known(None) => bail!(
            "foreground_unavailable: the compositor exposes no restorable prior focus, so nothing was focused"
        ),
        Fact::Unknown(why) => {
            bail!("foreground_unavailable: {why}")
        }
    }
}

/// The focus a restore leaves alone: focus that moved during the operation
/// to something that is neither the target nor the saved prior is the
/// user's newer choice. Returns that choice; `None` means restore.
pub fn newer_choice<T: PartialEq>(
    current: Option<T>,
    target: Option<T>,
    prior: Option<T>,
) -> Option<T> {
    current.filter(|current| Some(current) != target.as_ref() && Some(current) != prior.as_ref())
}

/// Where focus moved while an operation ran, relative to the target
/// application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusMove {
    /// The target app owned the focus before and still does: it opened one
    /// of its own windows (a dialog). Not an intrusion; leave it.
    SameAppDialog,
    /// The focus left the user's application (or arrived in the target from
    /// another app): restore it.
    OtherApp,
}

/// The same-app rule: a move is the app's own dialog only when the target
/// pid owned the focus before the action *and* owns it now. A move from a
/// decoy (another pid) into the target's dialog is a steal, as is a move
/// from the target to anything else. Unknown owners never qualify.
pub fn focus_move(
    target_pid: Option<u32>,
    previous_owner: Option<u32>,
    new_owner: Option<u32>,
) -> FocusMove {
    match (target_pid, previous_owner, new_owner) {
        (Some(target), Some(previous), Some(new)) if previous == target && new == target => {
            FocusMove::SameAppDialog
        }
        _ => FocusMove::OtherApp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restore_leaves_focus_only_on_a_third_choice() {
        assert_eq!(newer_choice(Some(3), Some(1), Some(2)), Some(3));
        assert_eq!(newer_choice(Some(1), Some(1), Some(2)), None);
        assert_eq!(newer_choice(Some(2), Some(1), Some(2)), None);
        assert_eq!(newer_choice(None, Some(1), Some(2)), None);
        assert_eq!(newer_choice(Some(3), None, None), Some(3));
    }

    #[test]
    fn same_app_rule_only_when_the_target_owned_the_focus_before_and_after() {
        assert_eq!(
            focus_move(Some(7), Some(7), Some(7)),
            FocusMove::SameAppDialog
        );
        for (target, previous, new) in [
            (Some(7), Some(3), Some(7)),
            (Some(7), Some(7), Some(3)),
            (Some(7), None, Some(7)),
            (Some(7), Some(7), None),
            (None, Some(7), Some(7)),
        ] {
            assert_eq!(focus_move(target, previous, new), FocusMove::OtherApp);
        }
    }

    fn unknown<T>() -> Fact<T> {
        Fact::Unknown("no source".into())
    }

    fn facts(locked: Fact<bool>, focused: Fact<bool>) -> Facts {
        Facts { locked, focused }
    }

    #[test]
    fn a_known_lock_refuses_every_scope_and_a_known_unlock_admits_every_scope() {
        for scope in [
            Scope::WindowInput,
            Scope::TargetBoundInput,
            Scope::DesktopInput,
            Scope::Capture,
        ] {
            let refused = lock(scope, &facts(Fact::Known(true), Fact::Known(true))).unwrap_err();
            assert!(refused.to_string().starts_with("screen_locked:"));
            lock(scope, &facts(Fact::Known(false), Fact::Known(false))).unwrap();
        }
    }

    #[test]
    fn an_unknown_lock_admits_window_input_only_after_focus_read_back() {
        lock(Scope::WindowInput, &facts(unknown(), Fact::Known(true))).unwrap();
        assert!(lock(Scope::WindowInput, &facts(unknown(), unknown())).is_err());
        assert_eq!(
            lock(Scope::WindowInput, &facts(unknown(), Fact::Known(false))).unwrap_err().to_string(),
            "foreground_unavailable: the lock state is unknown (no source) and the target's focus was not read back"
        );
    }

    #[test]
    fn an_unknown_lock_admits_target_bound_input_desktop_input_and_capture() {
        lock(Scope::TargetBoundInput, &facts(unknown(), unknown())).unwrap();
        lock(Scope::DesktopInput, &facts(unknown(), unknown())).unwrap();
        lock(Scope::Capture, &facts(unknown(), unknown())).unwrap();
    }

    #[test]
    fn delivery_requires_a_known_target_bound_transport() {
        delivery(&Fact::Known(true)).unwrap();
        for binding in [Fact::Known(false), unknown()] {
            assert!(delivery(&binding)
                .unwrap_err()
                .to_string()
                .starts_with("foreground_unavailable:"));
        }
    }

    #[test]
    fn only_an_unchanged_known_geometry_is_fresh() {
        assert!(fresh(&(100, 60), &Fact::Known((100, 60))));
        assert!(!fresh(&(100, 60), &Fact::Known((120, 60))));
        assert!(!fresh(&(100, 60), &unknown()));
    }

    #[test]
    fn a_point_reaches_the_window_only_when_every_fact_holds_and_the_first_failure_refuses() {
        let point = |fresh, on_window, shown, unobscured| Point {
            fresh,
            on_window,
            shown,
            unobscured,
        };
        assert_eq!(reach(&point(true, true, true, true)), Ok(()));
        assert_eq!(reach(&point(false, false, false, false)), Err(Miss::Stale));
        assert_eq!(
            reach(&point(true, false, false, false)),
            Err(Miss::OffWindow)
        );
        assert_eq!(reach(&point(true, true, false, false)), Err(Miss::Hidden));
        assert_eq!(reach(&point(true, true, true, false)), Err(Miss::Obscured));
    }

    #[test]
    fn only_two_agreeing_known_reads_are_settled() {
        assert!(settled(&3, &Fact::Known(3)));
        assert!(!settled(&3, &Fact::Known(4)));
        assert!(!settled(&3, &unknown()));
    }

    #[test]
    fn restore_needs_a_known_saved_focus_before_focusing() {
        assert_eq!(restore(Fact::Known(Some(5))).unwrap(), 5);
        assert_eq!(
            restore::<u64>(Fact::Known(None)).unwrap_err().to_string(),
            "foreground_unavailable: the compositor exposes no restorable prior focus, so nothing was focused"
        );
        assert_eq!(
            restore::<u64>(unknown()).unwrap_err().to_string(),
            "foreground_unavailable: no source"
        );
    }

    #[test]
    fn identity_requires_both_the_process_and_window() {
        let target = (107, 7);
        identity(&target, &Fact::Known(Some(target))).unwrap();
        for observed in [
            Fact::Known(Some((108, 7))),
            Fact::Known(Some((107, 8))),
            Fact::Known(None),
            unknown(),
        ] {
            assert!(identity(&target, &observed)
                .unwrap_err()
                .to_string()
                .starts_with("foreground_unavailable:"));
            assert!(!focused(&target, &observed));
        }
        assert!(focused(&target, &Fact::Known(Some(target))));
    }
}
