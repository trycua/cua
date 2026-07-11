//! Opt-in mutation gate for the Codex Computer Use compatibility profile.
//!
//! Native cua-driver tools are shared by the regular and compatibility
//! registries. Compatibility installs one gate for its session while an action
//! is in flight; native tools resolve that gate from their private
//! `_session_id` argument and revalidate it at each real mutation sink. Calls
//! from the normal profile resolve an inert gate and pay no Window Server probe.

use crate::session::{SessionLockEpoch, SessionLockError, SessionLockGuardian};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

type LockProbe = Arc<dyn Fn() -> Option<bool> + Send + Sync>;

#[derive(Clone)]
struct ActiveGate {
    registration: u64,
    guardian: Arc<SessionLockGuardian>,
    epoch: SessionLockEpoch,
    probe: LockProbe,
}

fn active_gates() -> &'static Mutex<HashMap<String, ActiveGate>> {
    static GATES: OnceLock<Mutex<HashMap<String, ActiveGate>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_REGISTRATION: AtomicU64 = AtomicU64::new(1);

/// RAII owner of one compatibility action's session gate.
pub(crate) struct DispatchGateRegistration {
    session: String,
    registration: u64,
}

impl Drop for DispatchGateRegistration {
    fn drop(&mut self) {
        let mut gates = active_gates().lock().unwrap();
        if gates
            .get(&self.session)
            .is_some_and(|gate| gate.registration == self.registration)
        {
            gates.remove(&self.session);
        }
    }
}

/// A cloneable handle captured by native blocking workers.
///
/// `active=None` is the normal cua-driver profile and deliberately performs no
/// lock probe. Compatibility handles retain their epoch even if the registry
/// owner is later dropped, so cancellation cannot unguard an already-dispatched
/// blocking worker.
#[derive(Clone, Default)]
pub(crate) struct NativeDispatchGate {
    active: Option<ActiveGate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DispatchGateError {
    ScreenLocked,
    EpochChanged,
    SessionStateUnavailable,
}

impl std::fmt::Display for DispatchGateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ScreenLocked => "Computer Use is unavailable while the macOS screen is locked",
            Self::EpochChanged => {
                "the macOS lock state changed during Computer Use; refresh app state before acting"
            }
            Self::SessionStateUnavailable => {
                "Computer Use could not verify the current macOS login session"
            }
        })
    }
}

impl std::error::Error for DispatchGateError {}

impl NativeDispatchGate {
    pub(crate) fn for_session(session: &str) -> Self {
        let active = active_gates().lock().unwrap().get(session).cloned();
        Self { active }
    }

    pub(crate) fn for_args(args: &Value) -> Self {
        let session = args
            .get("_session_id")
            .or_else(|| args.get("session"))
            .and_then(Value::as_str)
            .filter(|session| !session.is_empty());
        session.map(Self::for_session).unwrap_or_default()
    }

    /// Reconcile the authoritative Window Server flag and validate the exact
    /// action epoch immediately before a mutation sink.
    pub(crate) fn check(&self) -> Result<(), DispatchGateError> {
        let Some(active) = &self.active else {
            return Ok(());
        };
        let Some(locked) = (active.probe)() else {
            // Unknown is unsafe. Marking the guardian locked also runs the same
            // snapshot/cursor cleanup as an observed lock notification.
            active.guardian.observe(true);
            return Err(DispatchGateError::SessionStateUnavailable);
        };
        active.guardian.observe(locked);
        if locked {
            return Err(DispatchGateError::ScreenLocked);
        }
        active.guardian.validate(active.epoch).map_err(|error| match error {
            SessionLockError::Locked => DispatchGateError::ScreenLocked,
            SessionLockError::EpochChanged => DispatchGateError::EpochChanged,
        })
    }

    pub(crate) fn dispatch<T>(
        &self,
        mutation: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.check()?;
        mutation()
    }
}

pub(crate) fn install(
    session: &str,
    guardian: Arc<SessionLockGuardian>,
    epoch: SessionLockEpoch,
) -> DispatchGateRegistration {
    install_with_probe(session, guardian, epoch, Arc::new(crate::session::current_screen_locked))
}

fn install_with_probe(
    session: &str,
    guardian: Arc<SessionLockGuardian>,
    epoch: SessionLockEpoch,
    probe: LockProbe,
) -> DispatchGateRegistration {
    let registration = NEXT_REGISTRATION.fetch_add(1, Ordering::Relaxed);
    active_gates().lock().unwrap().insert(
        session.to_owned(),
        ActiveGate {
            registration,
            guardian,
            epoch,
            probe,
        },
    );
    DispatchGateRegistration {
        session: session.to_owned(),
        registration,
    }
}

#[cfg(test)]
pub(crate) fn install_for_test(
    session: &str,
    guardian: Arc<SessionLockGuardian>,
    epoch: SessionLockEpoch,
    probe: impl Fn() -> Option<bool> + Send + Sync + 'static,
) -> DispatchGateRegistration {
    install_with_probe(session, guardian, epoch, Arc::new(probe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    #[test]
    fn normal_profile_gate_is_inert() {
        let calls = AtomicUsize::new(0);
        NativeDispatchGate::default()
            .dispatch(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lock_after_pre_dispatch_wait_prevents_the_sink() {
        let guardian = SessionLockGuardian::for_test(false);
        let epoch = guardian.begin().unwrap();
        let locked = Arc::new(AtomicBool::new(false));
        let probe_locked = locked.clone();
        let _registration = install_for_test("wait-lock", guardian, epoch, move || {
            Some(probe_locked.load(Ordering::SeqCst))
        });
        let gate = NativeDispatchGate::for_session("wait-lock");

        // Represents cursor glide/focus settle completing after the lock edge.
        locked.store(true, Ordering::SeqCst);
        let mutations = AtomicUsize::new(0);
        let error = gate
            .dispatch(|| {
                mutations.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err();
        assert!(error.to_string().contains("screen is locked"));
        assert_eq!(mutations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unavailable_direct_probe_fails_closed() {
        let guardian = SessionLockGuardian::for_test(false);
        let epoch = guardian.begin().unwrap();
        let _registration = install_for_test("probe-none", guardian, epoch, || None);
        assert_eq!(
            NativeDispatchGate::for_session("probe-none").check(),
            Err(DispatchGateError::SessionStateUnavailable)
        );
    }
}
