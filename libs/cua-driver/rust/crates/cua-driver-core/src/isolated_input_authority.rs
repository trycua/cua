//! Disabled host-lifetime primitive for the isolated-input contract under review.
//!
//! This module does not approve a tool, authenticate a transport, issue a
//! compositor lease, or enable input. A future trusted host bridge must create
//! authority only after operator approval, and reserve a dispatch only after
//! the shared registry's final policy/resource checks and
//! `commit_authorized_dispatch`. Reservations do not refresh session idle TTL.
//! Policy digests remain bound to the immutable session context: a changed
//! policy requires a new session, not a grant that overrides its ceiling.
//! The compositor must independently validate its connection, target, and revocation generation
//! immediately before each event. A successful local check cannot eliminate a
//! race between that check and remote delivery.
//!
//! Only the host holds `InputControl`; action-side handles cannot extend its
//! lifetime. Public labels, PIDs, addresses, environment variables, and JSON
//! arguments cannot reconstruct these non-serializable handles. This is a
//! trusted-local contract, not containment of hostile same-user native code.

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::session_authorization::{AuthorizationContextSource, EffectiveAuthorizationContext};

/// Candidate limits, not an accepted wire protocol or support claim.
pub const MAX_AUTHORITY_TTL: Duration = Duration::from_secs(60);
pub const MAX_AUTHORITY_ACTIONS: u16 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOperation {
    Click,
    Key,
    Scroll,
    Drag,
}

impl InputOperation {
    fn bit(self) -> u8 {
        match self {
            Self::Click => 1,
            Self::Key => 2,
            Self::Scroll => 4,
            Self::Drag => 8,
        }
    }
}

/// Opaque identity attested by a future compositor bridge, not a window address.
/// This type intentionally has no serde implementation or public tool schema.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InputTargetBinding {
    pub compositor_epoch: Uuid,
    pub target_generation: Uuid,
    pub geometry_revision: u64,
}

/// Fresh trusted observations. Missing observations refuse; they are never
/// filled from the original grant to make a stale binding appear current.
pub struct InputObservation<'a> {
    pub target: Option<InputTargetBinding>,
    pub user_policy_sha256: PolicyObservation<'a>,
    pub managed_policy_sha256: PolicyObservation<'a>,
}

/// An absent policy is different from a failed policy observation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PolicyObservation<'a> {
    Known(Option<&'a str>),
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevocationReason {
    OperatorStop,
    HostLost,
    SessionEnded,
    Expired,
    TargetChanged,
    PolicyChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthorityError {
    #[error("isolated input requires a live trusted-host authorization context")]
    TrustedContextRequired,
    #[error("invalid isolated-input scope or limits")]
    InvalidScope,
    #[error("isolated-input authority belongs to another authorization context")]
    ContextMismatch,
    #[error("isolated-input authority is revoked: {0:?}")]
    Revoked(RevocationReason),
    #[error("operation is outside the approved isolated-input scope")]
    OperationDenied,
    #[error("an isolated-input action is already reserved")]
    Busy,
    #[error("isolated-input action quota exhausted")]
    QuotaExhausted,
    #[error("isolated-input reservation is no longer current")]
    StaleReservation,
    #[error("isolated-input authority state is unavailable")]
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityPhase {
    Idle,
    Pending,
    Active,
    Revoked(RevocationReason),
}

/// Local control state only, not proof that a compositor emitted input or that
/// an app accepted it. No target tokens, session labels, or content are exposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputActivity {
    pub phase: ActivityPhase,
    pub actions_reserved: u16,
}

struct State {
    context: Arc<EffectiveAuthorizationContext>,
    target: InputTargetBinding,
    operations: u8,
    deadline: Instant,
    max_actions: u16,
    reserved: u16,
    current: Option<(u16, ActivityPhase)>,
    revoked: Option<RevocationReason>,
}

impl State {
    fn revoke(&mut self, reason: RevocationReason) {
        // The first terminal cause is retained; a later Drop must not replace
        // OperatorStop with HostLost. No new admission can revive this object.
        self.revoked.get_or_insert(reason);
        self.current = None;
    }

    fn check_lifetime(&mut self, now: Instant) -> Result<(), AuthorityError> {
        if let Some(reason) = self.revoked {
            return Err(AuthorityError::Revoked(reason));
        }
        if self.context.is_expired() {
            self.revoke(RevocationReason::SessionEnded);
        } else if now >= self.deadline {
            self.revoke(RevocationReason::Expired);
        }
        self.revoked
            .map_or(Ok(()), |reason| Err(AuthorityError::Revoked(reason)))
    }

    fn check(
        &mut self,
        context: &Arc<EffectiveAuthorizationContext>,
        observation: &InputObservation<'_>,
        now: Instant,
    ) -> Result<(), AuthorityError> {
        self.check_lifetime(now)?;
        // Equal public labels, modes, policies, or runtime keys are not enough.
        if !Arc::ptr_eq(context, &self.context) {
            return Err(AuthorityError::ContextMismatch);
        }
        if observation.target != Some(self.target) {
            self.revoke(RevocationReason::TargetChanged);
        } else if observation.user_policy_sha256
            != PolicyObservation::Known(self.context.user_policy_sha256())
            || observation.managed_policy_sha256
                != PolicyObservation::Known(self.context.managed_policy_sha256())
        {
            self.revoke(RevocationReason::PolicyChanged);
        }
        self.revoked
            .map_or(Ok(()), |reason| Err(AuthorityError::Revoked(reason)))
    }
}

/// Trusted control owner. It is deliberately non-Clone; dropping it revokes
/// pending and active authority even when the agent retains every handle.
pub struct InputControl {
    state: Arc<Mutex<State>>,
}

/// Action-side authority reference. It cannot renew, change scope, or retain
/// the host controller. It has no public constructor or serialization.
#[derive(Clone)]
pub struct InputDelegation {
    state: Weak<Mutex<State>>,
}

/// One pending/active action. Not Clone: starting twice or checking after
/// completion refuses. Long gestures must recheck before each bounded step.
pub struct InputReservation {
    state: Weak<Mutex<State>>,
    sequence: u16,
    operation: InputOperation,
}

impl InputControl {
    /// Called only by trusted host integration after exact-scope approval.
    /// This constructor is not reachable from any Driver tool or transport.
    pub fn new(
        context: Arc<EffectiveAuthorizationContext>,
        target: InputTargetBinding,
        operations: &[InputOperation],
        ttl: Duration,
        max_actions: u16,
    ) -> Result<(Self, InputDelegation), AuthorityError> {
        Self::new_at(
            context,
            target,
            operations,
            ttl,
            max_actions,
            Instant::now(),
        )
    }

    fn new_at(
        context: Arc<EffectiveAuthorizationContext>,
        target: InputTargetBinding,
        operations: &[InputOperation],
        ttl: Duration,
        max_actions: u16,
        now: Instant,
    ) -> Result<(Self, InputDelegation), AuthorityError> {
        if context.source() != AuthorizationContextSource::TrustedHost || context.is_expired() {
            return Err(AuthorityError::TrustedContextRequired);
        }
        if target.compositor_epoch.is_nil()
            || target.target_generation.is_nil()
            || target.geometry_revision == 0
            || operations.is_empty()
            || operations.len() > 4
            || ttl.is_zero()
            || ttl > MAX_AUTHORITY_TTL
            || max_actions == 0
            || max_actions > MAX_AUTHORITY_ACTIONS
        {
            return Err(AuthorityError::InvalidScope);
        }
        let deadline = now.checked_add(ttl).ok_or(AuthorityError::InvalidScope)?;
        let operation_bits = operations
            .iter()
            .fold(0_u8, |bits, operation| bits | operation.bit());
        if operation_bits.count_ones() as usize != operations.len() {
            return Err(AuthorityError::InvalidScope);
        }
        let state = Arc::new(Mutex::new(State {
            context,
            target,
            operations: operation_bits,
            deadline,
            max_actions,
            reserved: 0,
            current: None,
            revoked: None,
        }));
        let delegation = InputDelegation {
            state: Arc::downgrade(&state),
        };
        Ok((Self { state }, delegation))
    }

    pub fn revoke(&self, reason: RevocationReason) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .revoke(reason);
    }

    pub fn activity(&self) -> Result<InputActivity, AuthorityError> {
        let mut state = self.state.lock().map_err(|_| AuthorityError::Unavailable)?;
        let _ = state.check_lifetime(Instant::now());
        Ok(InputActivity {
            phase: state.revoked.map_or_else(
                || {
                    state
                        .current
                        .map_or(ActivityPhase::Idle, |(_, phase)| phase)
                },
                ActivityPhase::Revoked,
            ),
            actions_reserved: state.reserved,
        })
    }
}

impl Drop for InputControl {
    fn drop(&mut self) {
        self.revoke(RevocationReason::HostLost);
    }
}

impl InputDelegation {
    /// Reserve after shared policy admission; this does not perform or replace
    /// that admission. Compositor lane ownership remains compositor-owned.
    pub fn reserve(
        &self,
        context: &Arc<EffectiveAuthorizationContext>,
        observation: &InputObservation<'_>,
        operation: InputOperation,
    ) -> Result<InputReservation, AuthorityError> {
        self.reserve_at(context, observation, operation, Instant::now())
    }

    fn reserve_at(
        &self,
        context: &Arc<EffectiveAuthorizationContext>,
        observation: &InputObservation<'_>,
        operation: InputOperation,
        now: Instant,
    ) -> Result<InputReservation, AuthorityError> {
        let shared = self
            .state
            .upgrade()
            .ok_or(AuthorityError::Revoked(RevocationReason::HostLost))?;
        let mut state = shared.lock().map_err(|_| AuthorityError::Unavailable)?;
        state.check(context, observation, now)?;
        if state.operations & operation.bit() == 0 {
            return Err(AuthorityError::OperationDenied);
        }
        if state.current.is_some() {
            return Err(AuthorityError::Busy);
        }
        if state.reserved >= state.max_actions {
            return Err(AuthorityError::QuotaExhausted);
        }
        // Reservation consumes quota, even if dropped before delivery. Never
        // reuse an action identity or refund it in a cancellation loop.
        state.reserved += 1;
        let sequence = state.reserved;
        state.current = Some((sequence, ActivityPhase::Pending));
        Ok(InputReservation {
            state: self.state.clone(),
            sequence,
            operation,
        })
    }
}

impl InputReservation {
    /// The bridge must derive its operation descriptor from this bound value,
    /// not from a later caller-supplied operation. Payload binding is a separate
    /// requirement of the reviewed compositor wire contract.
    pub fn operation(&self) -> InputOperation {
        self.operation
    }

    pub fn start(
        &self,
        context: &Arc<EffectiveAuthorizationContext>,
        observation: &InputObservation<'_>,
    ) -> Result<(), AuthorityError> {
        self.check_phase(context, observation, ActivityPhase::Pending, Instant::now())
    }

    pub fn check_step(
        &self,
        context: &Arc<EffectiveAuthorizationContext>,
        observation: &InputObservation<'_>,
    ) -> Result<(), AuthorityError> {
        self.check_phase(context, observation, ActivityPhase::Active, Instant::now())
    }

    fn check_phase(
        &self,
        context: &Arc<EffectiveAuthorizationContext>,
        observation: &InputObservation<'_>,
        expected: ActivityPhase,
        now: Instant,
    ) -> Result<(), AuthorityError> {
        let shared = self
            .state
            .upgrade()
            .ok_or(AuthorityError::Revoked(RevocationReason::HostLost))?;
        let mut state = shared.lock().map_err(|_| AuthorityError::Unavailable)?;
        state.check(context, observation, now)?;
        if state.current != Some((self.sequence, expected)) {
            return Err(AuthorityError::StaleReservation);
        }
        state.current = Some((self.sequence, ActivityPhase::Active));
        Ok(())
    }
}

impl Drop for InputReservation {
    fn drop(&mut self) {
        if let Some(shared) = self.state.upgrade() {
            let mut state = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state
                .current
                .is_some_and(|(sequence, _)| sequence == self.sequence)
            {
                state.current = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::PermissionMode;
    use crate::session_authorization::{
        DelegatedSessionRequest, SessionAuthorizationRegistry, SessionModeCeiling, TrustedHostLease,
    };

    struct Fixture {
        registry: SessionAuthorizationRegistry,
        host: TrustedHostLease,
        context: Arc<EffectiveAuthorizationContext>,
        target: InputTargetBinding,
    }

    impl Fixture {
        fn new(mode: PermissionMode) -> Self {
            let registry = SessionAuthorizationRegistry::with_ceiling(
                SessionModeCeiling::for_trusted_sessions(
                    [mode],
                    true,
                    Duration::from_secs(120),
                    Duration::from_secs(90),
                )
                .unwrap(),
            );
            let (host, connection) = registry.trusted_in_process_binding();
            registry
                .bind_delegated_session(
                    &host,
                    &connection,
                    DelegatedSessionRequest {
                        public_session: "synthetic-session".to_owned(),
                        transport_session: "synthetic-transport".to_owned(),
                        mode,
                        ttl: Duration::from_secs(120),
                        idle_ttl: Duration::from_secs(90),
                        capability_manifest: None,
                    },
                )
                .unwrap();
            let context = registry
                .resolve_delegated(&connection, "synthetic-session", "synthetic-transport")
                .unwrap();
            Self {
                registry,
                host,
                context,
                target: InputTargetBinding {
                    compositor_epoch: Uuid::new_v4(),
                    target_generation: Uuid::new_v4(),
                    geometry_revision: 1,
                },
            }
        }

        fn observation(&self) -> InputObservation<'_> {
            InputObservation {
                target: Some(self.target),
                user_policy_sha256: PolicyObservation::Known(self.context.user_policy_sha256()),
                managed_policy_sha256: PolicyObservation::Known(
                    self.context.managed_policy_sha256(),
                ),
            }
        }

        fn authority(&self) -> (InputControl, InputDelegation) {
            InputControl::new(
                self.context.clone(),
                self.target,
                &[InputOperation::Click, InputOperation::Drag],
                Duration::from_secs(60),
                8,
            )
            .unwrap()
        }

        fn reserve(&self, delegation: &InputDelegation) -> InputReservation {
            delegation
                .reserve(&self.context, &self.observation(), InputOperation::Drag)
                .unwrap()
        }
    }

    fn refusal<T>(result: Result<T, AuthorityError>) -> AuthorityError {
        match result {
            Err(error) => error,
            Ok(_) => panic!("authority unexpectedly admitted an operation"),
        }
    }

    #[test]
    fn activity_tracks_pending_active_and_completion_without_delivery_claims() {
        let f = Fixture::new(PermissionMode::Standard);
        let (control, delegation) = f.authority();
        assert_eq!(
            control.activity().unwrap(),
            InputActivity {
                phase: ActivityPhase::Idle,
                actions_reserved: 0
            }
        );
        let action = f.reserve(&delegation);
        assert_eq!(action.operation(), InputOperation::Drag);
        assert_eq!(control.activity().unwrap().phase, ActivityPhase::Pending);
        assert_eq!(
            action.check_step(&f.context, &f.observation()),
            Err(AuthorityError::StaleReservation)
        );
        action.start(&f.context, &f.observation()).unwrap();
        assert_eq!(control.activity().unwrap().phase, ActivityPhase::Active);
        assert_eq!(
            action.start(&f.context, &f.observation()),
            Err(AuthorityError::StaleReservation)
        );
        action.check_step(&f.context, &f.observation()).unwrap();
        drop(action);
        assert_eq!(
            control.activity().unwrap(),
            InputActivity {
                phase: ActivityPhase::Idle,
                actions_reserved: 1
            }
        );
    }

    #[test]
    fn process_context_never_becomes_input_authority_even_in_unrestricted_mode() {
        let f = Fixture::new(PermissionMode::Unrestricted);
        let context = f
            .registry
            .compatibility_context(PermissionMode::Unrestricted, None)
            .unwrap();
        assert_eq!(
            refusal(InputControl::new(
                context,
                f.target,
                &[InputOperation::Click],
                Duration::from_secs(1),
                1
            )),
            AuthorityError::TrustedContextRequired
        );
    }

    #[test]
    fn equal_public_labels_and_modes_cannot_transfer_authority() {
        let f = Fixture::new(PermissionMode::Standard);
        let other = Fixture::new(PermissionMode::Standard);
        assert_eq!(f.context.public_session(), other.context.public_session());
        let (_control, delegation) = f.authority();
        assert_eq!(
            refusal(delegation.reserve(&other.context, &f.observation(), InputOperation::Click)),
            AuthorityError::ContextMismatch
        );
        let action = f.reserve(&delegation);
        assert_eq!(
            action.start(&other.context, &f.observation()),
            Err(AuthorityError::ContextMismatch)
        );
        action.start(&f.context, &f.observation()).unwrap();
    }

    #[test]
    fn host_drop_invalidates_unredeemed_pending_and_active_handles() {
        for active in [false, true] {
            let f = Fixture::new(PermissionMode::Standard);
            let (control, delegation) = f.authority();
            let retained_delegation = delegation.clone();
            let action = f.reserve(&delegation);
            if active {
                action.start(&f.context, &f.observation()).unwrap();
            }
            drop(control);
            let expected = AuthorityError::Revoked(RevocationReason::HostLost);
            assert_eq!(action.start(&f.context, &f.observation()), Err(expected));
            assert_eq!(
                action.check_step(&f.context, &f.observation()),
                Err(expected)
            );
            assert_eq!(
                refusal(retained_delegation.reserve(
                    &f.context,
                    &f.observation(),
                    InputOperation::Click
                )),
                expected
            );
        }
        let f = Fixture::new(PermissionMode::Standard);
        let (control, delegation) = f.authority();
        drop(control);
        assert_eq!(
            refusal(delegation.reserve(&f.context, &f.observation(), InputOperation::Click)),
            AuthorityError::Revoked(RevocationReason::HostLost)
        );
    }

    #[test]
    fn stop_is_terminal_for_unused_pending_and_active_authority() {
        for phase in [
            ActivityPhase::Idle,
            ActivityPhase::Pending,
            ActivityPhase::Active,
        ] {
            let f = Fixture::new(PermissionMode::Standard);
            let (control, delegation) = f.authority();
            let action = (phase != ActivityPhase::Idle).then(|| f.reserve(&delegation));
            if phase == ActivityPhase::Active {
                action
                    .as_ref()
                    .unwrap()
                    .start(&f.context, &f.observation())
                    .unwrap();
            }
            control.revoke(RevocationReason::OperatorStop);
            control.revoke(RevocationReason::HostLost);
            assert_eq!(
                control.activity().unwrap().phase,
                ActivityPhase::Revoked(RevocationReason::OperatorStop)
            );
            if let Some(action) = action {
                assert_eq!(
                    action.check_step(&f.context, &f.observation()),
                    Err(AuthorityError::Revoked(RevocationReason::OperatorStop))
                );
            }
            assert_eq!(
                refusal(delegation.reserve(&f.context, &f.observation(), InputOperation::Drag)),
                AuthorityError::Revoked(RevocationReason::OperatorStop)
            );
        }
    }

    #[test]
    fn registry_host_revocation_invalidates_pending_and_active_authority() {
        for active in [false, true] {
            let f = Fixture::new(PermissionMode::Standard);
            let (control, delegation) = f.authority();
            let action = f.reserve(&delegation);
            if active {
                action.start(&f.context, &f.observation()).unwrap();
            }
            assert_eq!(f.registry.revoke_host(&f.host), 1);
            assert_eq!(
                action.check_step(&f.context, &f.observation()),
                Err(AuthorityError::Revoked(RevocationReason::SessionEnded))
            );
            assert_eq!(
                control.activity().unwrap().phase,
                ActivityPhase::Revoked(RevocationReason::SessionEnded)
            );
        }
    }

    #[test]
    fn every_target_component_and_missing_observation_revoke_without_revival() {
        for component in 0..4 {
            let f = Fixture::new(PermissionMode::Standard);
            let (_control, delegation) = f.authority();
            let action = f.reserve(&delegation);
            action.start(&f.context, &f.observation()).unwrap();
            let mut target = f.target;
            match component {
                0 => target.compositor_epoch = Uuid::new_v4(),
                1 => target.target_generation = Uuid::new_v4(),
                2 => target.geometry_revision += 1,
                _ => {}
            }
            let mut changed = f.observation();
            changed.target = (component != 3).then_some(target);
            let expected = Err(AuthorityError::Revoked(RevocationReason::TargetChanged));
            assert_eq!(action.check_step(&f.context, &changed), expected);
            assert_eq!(action.check_step(&f.context, &f.observation()), expected);
        }
    }

    #[test]
    fn changed_user_or_managed_policy_revokes_before_first_event() {
        for managed in [false, true] {
            let f = Fixture::new(PermissionMode::Standard);
            let (_control, delegation) = f.authority();
            let action = f.reserve(&delegation);
            let mut changed = f.observation();
            if managed {
                changed.managed_policy_sha256 = PolicyObservation::Known(Some("changed-policy"));
            } else {
                changed.user_policy_sha256 = PolicyObservation::Known(Some("changed-policy"));
            }
            let expected = Err(AuthorityError::Revoked(RevocationReason::PolicyChanged));
            assert_eq!(action.start(&f.context, &changed), expected);
            assert_eq!(action.start(&f.context, &f.observation()), expected);
        }
    }

    #[test]
    fn expiry_is_inclusive_and_cannot_be_renewed_by_dispatch() {
        let f = Fixture::new(PermissionMode::Standard);
        let now = Instant::now();
        let ttl = Duration::from_secs(1);
        let (_control, delegation) = InputControl::new_at(
            f.context.clone(),
            f.target,
            &[InputOperation::Drag],
            ttl,
            8,
            now,
        )
        .unwrap();
        let action = delegation
            .reserve_at(&f.context, &f.observation(), InputOperation::Drag, now)
            .unwrap();
        action
            .check_phase(&f.context, &f.observation(), ActivityPhase::Pending, now)
            .unwrap();
        action
            .check_phase(
                &f.context,
                &f.observation(),
                ActivityPhase::Active,
                now + ttl - Duration::from_nanos(1),
            )
            .unwrap();
        let expired = AuthorityError::Revoked(RevocationReason::Expired);
        assert_eq!(
            action.check_phase(
                &f.context,
                &f.observation(),
                ActivityPhase::Active,
                now + ttl
            ),
            Err(expired)
        );
        assert_eq!(
            refusal(delegation.reserve_at(&f.context, &f.observation(), InputOperation::Drag, now)),
            expired
        );
    }

    #[test]
    fn operation_scope_does_not_expand_in_unrestricted_mode() {
        let f = Fixture::new(PermissionMode::Unrestricted);
        let (_control, delegation) = f.authority();
        for operation in [InputOperation::Key, InputOperation::Scroll] {
            assert_eq!(
                refusal(delegation.reserve(&f.context, &f.observation(), operation)),
                AuthorityError::OperationDenied
            );
        }
        let _action = f.reserve(&delegation);
    }

    #[test]
    fn one_pending_action_and_bounded_nonrefundable_quota() {
        let f = Fixture::new(PermissionMode::Standard);
        let (control, delegation) = InputControl::new(
            f.context.clone(),
            f.target,
            &[InputOperation::Drag],
            Duration::from_secs(60),
            2,
        )
        .unwrap();
        let first = f.reserve(&delegation);
        assert_eq!(
            refusal(delegation.reserve(&f.context, &f.observation(), InputOperation::Drag)),
            AuthorityError::Busy
        );
        assert_eq!(control.activity().unwrap().actions_reserved, 1);
        drop(first);
        let second = f.reserve(&delegation);
        assert_eq!(second.sequence, 2);
        drop(second);
        assert_eq!(
            refusal(delegation.reserve(&f.context, &f.observation(), InputOperation::Drag)),
            AuthorityError::QuotaExhausted
        );
    }

    #[test]
    fn independent_controls_revoke_only_their_own_authority() {
        let a = Fixture::new(PermissionMode::Standard);
        let b = Fixture::new(PermissionMode::Standard);
        let (control_a, delegation_a) = a.authority();
        let (_control_b, delegation_b) = b.authority();
        let action_a = a.reserve(&delegation_a);
        let action_b = b.reserve(&delegation_b);
        action_a.start(&a.context, &a.observation()).unwrap();
        action_b.start(&b.context, &b.observation()).unwrap();
        control_a.revoke(RevocationReason::OperatorStop);
        assert_eq!(
            action_a.check_step(&a.context, &a.observation()),
            Err(AuthorityError::Revoked(RevocationReason::OperatorStop))
        );
        action_b.check_step(&b.context, &b.observation()).unwrap();
    }

    #[test]
    fn invalid_scope_cannot_create_authority() {
        let f = Fixture::new(PermissionMode::Standard);
        for (ttl, quota) in [
            (Duration::ZERO, 1),
            (MAX_AUTHORITY_TTL + Duration::from_secs(1), 1),
            (Duration::from_secs(1), 0),
            (Duration::from_secs(1), MAX_AUTHORITY_ACTIONS + 1),
        ] {
            assert_eq!(
                refusal(InputControl::new(
                    f.context.clone(),
                    f.target,
                    &[InputOperation::Click],
                    ttl,
                    quota
                )),
                AuthorityError::InvalidScope
            );
        }
        for component in 0..3 {
            let mut target = f.target;
            match component {
                0 => target.compositor_epoch = Uuid::nil(),
                1 => target.target_generation = Uuid::nil(),
                _ => target.geometry_revision = 0,
            }
            assert_eq!(
                refusal(InputControl::new(
                    f.context.clone(),
                    target,
                    &[InputOperation::Click],
                    Duration::from_secs(1),
                    1
                )),
                AuthorityError::InvalidScope
            );
        }
        assert_eq!(
            refusal(InputControl::new(
                f.context.clone(),
                f.target,
                &[],
                Duration::from_secs(1),
                1
            )),
            AuthorityError::InvalidScope
        );
    }

    #[test]
    fn revoked_context_cannot_mint_fresh_authority() {
        let f = Fixture::new(PermissionMode::Standard);
        f.registry.revoke_all();
        assert_eq!(
            refusal(InputControl::new(
                f.context.clone(),
                f.target,
                &[InputOperation::Click],
                Duration::from_secs(1),
                1
            )),
            AuthorityError::TrustedContextRequired
        );
    }

    #[test]
    fn failed_policy_observation_is_not_an_absent_policy() {
        for managed in [false, true] {
            let f = Fixture::new(PermissionMode::Standard);
            let (_control, delegation) = f.authority();
            let mut unavailable = f.observation();
            if managed {
                unavailable.managed_policy_sha256 = PolicyObservation::Unavailable;
            } else {
                unavailable.user_policy_sha256 = PolicyObservation::Unavailable;
            }
            let expected = AuthorityError::Revoked(RevocationReason::PolicyChanged);
            assert_eq!(
                refusal(delegation.reserve(&f.context, &unavailable, InputOperation::Click)),
                expected
            );
            assert_eq!(
                refusal(delegation.reserve(&f.context, &f.observation(), InputOperation::Click)),
                expected
            );
        }
    }

    #[test]
    fn concurrent_action_cannot_retain_host_or_continue_after_stop_returns() {
        let f = Fixture::new(PermissionMode::Standard);
        let (control, delegation) = f.authority();
        let action = f.reserve(&delegation);
        action.start(&f.context, &f.observation()).unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let context = f.context.clone();
        let target = f.target;
        let worker = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            resume_rx.recv().unwrap();
            action.check_step(
                &context,
                &InputObservation {
                    target: Some(target),
                    user_policy_sha256: PolicyObservation::Known(context.user_policy_sha256()),
                    managed_policy_sha256: PolicyObservation::Known(
                        context.managed_policy_sha256(),
                    ),
                },
            )
        });
        ready_rx.recv().unwrap();
        control.revoke(RevocationReason::OperatorStop);
        resume_tx.send(()).unwrap();
        assert_eq!(
            worker.join().unwrap(),
            Err(AuthorityError::Revoked(RevocationReason::OperatorStop))
        );
    }

    #[test]
    fn poisoned_authority_refuses_and_remains_revocable() {
        let f = Fixture::new(PermissionMode::Standard);
        let (control, delegation) = f.authority();
        let state = control.state.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = state.lock().unwrap();
            panic!("synthetic state failure");
        });
        assert_eq!(
            refusal(delegation.reserve(&f.context, &f.observation(), InputOperation::Click)),
            AuthorityError::Unavailable
        );
        assert_eq!(control.activity(), Err(AuthorityError::Unavailable));
        control.revoke(RevocationReason::OperatorStop);
        assert_eq!(
            control
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .revoked,
            Some(RevocationReason::OperatorStop)
        );
    }

    #[test]
    fn identical_cloned_context_is_not_the_original_authority() {
        let f = Fixture::new(PermissionMode::Standard);
        let copied = Arc::new((*f.context).clone());
        let (_control, delegation) = f.authority();
        assert_eq!(
            refusal(delegation.reserve(&copied, &f.observation(), InputOperation::Click)),
            AuthorityError::ContextMismatch
        );
        let action = f.reserve(&delegation);
        assert_eq!(
            action.start(&copied, &f.observation()),
            Err(AuthorityError::ContextMismatch)
        );
        action.start(&f.context, &f.observation()).unwrap();
        assert_eq!(
            action.check_step(&copied, &f.observation()),
            Err(AuthorityError::ContextMismatch)
        );
        action.check_step(&f.context, &f.observation()).unwrap();
    }

    #[test]
    fn duplicate_operations_refuse_instead_of_ambiguous_scope_normalization() {
        let f = Fixture::new(PermissionMode::Standard);
        assert_eq!(
            refusal(InputControl::new(
                f.context.clone(),
                f.target,
                &[InputOperation::Click, InputOperation::Click],
                Duration::from_secs(1),
                1
            )),
            AuthorityError::InvalidScope
        );
    }
}
