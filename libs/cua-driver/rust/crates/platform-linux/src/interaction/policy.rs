//! What each primitive decides from the facts an adapter reports. Every
//! decision states what an unknown fact means for each scope.

use anyhow::{bail, Result};

use super::facts::Fact;

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
/// exact focus was read back, and admits target-bound input, desktop input
/// and capture on the adapter's other checks, as before this rule existed.
pub fn lock(scope: Scope, locked: &Fact<bool>, focus_read_back: bool) -> Result<()> {
    match (locked, scope) {
        (Fact::Known(true), _) => {
            bail!("screen_locked: the session screensaver reports itself active, so nothing was sent or captured")
        }
        (Fact::Known(false), _)
        | (
            Fact::Unknown(_),
            Scope::TargetBoundInput | Scope::DesktopInput | Scope::Capture,
        ) => Ok(()),
        (Fact::Unknown(_), Scope::WindowInput) if focus_read_back => Ok(()),
        (Fact::Unknown(why), Scope::WindowInput) => bail!(
            "foreground_unavailable: the lock state is unknown ({why}) and the target's focus was not read back"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown<T>() -> Fact<T> {
        Fact::Unknown("no source".into())
    }

    #[test]
    fn a_known_lock_refuses_every_scope_and_a_known_unlock_admits_every_scope() {
        for scope in [
            Scope::WindowInput,
            Scope::TargetBoundInput,
            Scope::DesktopInput,
            Scope::Capture,
        ] {
            let refused = lock(scope, &Fact::Known(true), true).unwrap_err();
            assert!(refused.to_string().starts_with("screen_locked:"));
            lock(scope, &Fact::Known(false), false).unwrap();
        }
    }

    #[test]
    fn an_unknown_lock_admits_window_input_only_after_focus_read_back() {
        lock(Scope::WindowInput, &unknown(), true).unwrap();
        assert_eq!(
            lock(Scope::WindowInput, &unknown(), false).unwrap_err().to_string(),
            "foreground_unavailable: the lock state is unknown (no source) and the target's focus was not read back"
        );
    }

    #[test]
    fn an_unknown_lock_admits_target_bound_input_desktop_input_and_capture() {
        lock(Scope::TargetBoundInput, &unknown(), false).unwrap();
        lock(Scope::DesktopInput, &unknown(), false).unwrap();
        lock(Scope::Capture, &unknown(), false).unwrap();
    }
}
