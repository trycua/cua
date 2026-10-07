//! Experimental receipt ownership for post-dispatch supervision (RFC #4771).
//! This module does not dispatch input or establish application commitment.
//! Reserve before input, transfer supervision synchronously after dispatch,
//! and close/drain the owner before orderly runtime shutdown. Scope keys must
//! come from trusted runtime session identity, never caller-supplied labels.
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ReceiptId(Uuid);
impl From<ReceiptId> for String {
    fn from(id: ReceiptId) -> String {
        id.0.to_string()
    }
}
impl TryFrom<String> for ReceiptId {
    type Error = uuid::Error;
    fn try_from(id: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&id).map(Self)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub polled: bool,
    pub foreground_changed: bool,
    pub new_window_count: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "observation", rename_all = "snake_case")]
pub enum ReceiptState {
    Reserved,
    Pending,
    Finished(Observation),
    Failed,
    Interrupted,
}
impl ReceiptState {
    pub fn terminal(&self) -> bool {
        matches!(self, Self::Finished(_) | Self::Failed | Self::Interrupted)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Closed,
    Capacity,
    Unavailable,
    Pending,
    Timeout,
    RuntimeUnavailable,
    InvalidCapacity,
    InvalidScope,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundGuardReport {
    pub foreground_preserved: bool,
    pub physical_input_unchanged: bool,
}
type ForegroundGuard = Arc<dyn Fn() -> Option<ForegroundGuardReport> + Send + Sync>;
struct Entry {
    foreground_guard: Option<ForegroundGuard>,
    activation_signal: Option<Arc<std::sync::atomic::AtomicBool>>,
    scope: String,
    state: watch::Sender<ReceiptState>,
}
struct Inner {
    entries: HashMap<Uuid, Entry>,
    closed: bool,
}
#[derive(Clone)]
pub struct Owner {
    inner: Arc<Mutex<Inner>>,
    capacity: usize,
}
pub struct Reservation {
    owner: Owner,
    id: ReceiptId,
    runtime: tokio::runtime::Handle,
    transferred: bool,
}
impl Owner {
    pub fn new(capacity: usize) -> Result<Self, Refusal> {
        if capacity == 0 {
            return Err(Refusal::InvalidCapacity);
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                entries: HashMap::new(),
                closed: false,
            })),
            capacity,
        })
    }
    /// Atomically reserves bounded storage before input. Terminal receipts
    /// consume capacity until explicitly released; there is no silent eviction.
    pub fn reserve(&self, trusted_scope: &str) -> Result<Reservation, Refusal> {
        if trusted_scope.is_empty() {
            return Err(Refusal::InvalidScope);
        }
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| Refusal::RuntimeUnavailable)?;
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Err(Refusal::Closed);
        }
        if inner.entries.len() >= self.capacity {
            return Err(Refusal::Capacity);
        }
        let id = ReceiptId(Uuid::new_v4());
        let (state, _) = watch::channel(ReceiptState::Reserved);
        inner.entries.insert(
            id.0,
            Entry {
                foreground_guard: None,
                activation_signal: None,
                scope: trusted_scope.into(),
                state,
            },
        );
        Ok(Reservation {
            owner: self.clone(),
            id,
            runtime,
            transferred: false,
        })
    }
    fn subscribe(
        &self,
        trusted_scope: &str,
        id: &ReceiptId,
    ) -> Result<watch::Receiver<ReceiptState>, Refusal> {
        let inner = self.inner.lock().unwrap();
        let entry = inner
            .entries
            .get(&id.0)
            .filter(|e| e.scope == trusted_scope)
            .ok_or(Refusal::Unavailable)?;
        Ok(entry.state.subscribe())
    }
    pub fn read(&self, trusted_scope: &str, id: &ReceiptId) -> Result<ReceiptState, Refusal> {
        Ok(self.subscribe(trusted_scope, id)?.borrow().clone())
    }
    /// Fresh driver-bound foreground/input evidence. Clone the closure before
    /// reading OS state so native work never runs under the receipt-map lock.
    pub fn foreground_guard(
        &self,
        trusted_scope: &str,
        id: &ReceiptId,
    ) -> Result<Option<ForegroundGuardReport>, Refusal> {
        let check = {
            let inner = self.inner.lock().unwrap();
            inner
                .entries
                .get(&id.0)
                .filter(|e| e.scope == trusted_scope)
                .ok_or(Refusal::Unavailable)?
                .foreground_guard
                .clone()
        };
        Ok(check.and_then(|check| check()))
    }
    /// A native callback signal can be read while full observation remains
    /// pending. Absence is explicit; false is never application commitment.
    pub fn activation_observed(
        &self,
        trusted_scope: &str,
        id: &ReceiptId,
    ) -> Result<Option<bool>, Refusal> {
        let inner = self.inner.lock().unwrap();
        let entry = inner
            .entries
            .get(&id.0)
            .filter(|e| e.scope == trusted_scope)
            .ok_or(Refusal::Unavailable)?;
        Ok(entry
            .activation_signal
            .as_ref()
            .map(|signal| signal.load(std::sync::atomic::Ordering::Acquire)))
    }
    /// A timeout or cancellation drops only this receiver, never the observer.
    pub async fn fence(
        &self,
        trusted_scope: &str,
        id: &ReceiptId,
        deadline: Duration,
    ) -> Result<ReceiptState, Refusal> {
        let receiver = self.subscribe(trusted_scope, id)?;
        tokio::time::timeout(deadline, wait_terminal(receiver))
            .await
            .map_err(|_| Refusal::Timeout)?
    }
    pub fn release(&self, trusted_scope: &str, id: &ReceiptId) -> Result<(), Refusal> {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner
            .entries
            .get(&id.0)
            .filter(|e| e.scope == trusted_scope)
            .ok_or(Refusal::Unavailable)?;
        if !entry.state.borrow().terminal() {
            return Err(Refusal::Pending);
        }
        inner.entries.remove(&id.0);
        Ok(())
    }
    /// Stop admission without cancelling already admitted observers. Pending
    /// reservations must transfer or drop before drain can finish.
    pub fn close(&self) {
        self.inner.lock().unwrap().closed = true;
    }
    /// Runtime shutdown must await this before destroying its executor. A
    /// timed-out drain retains receipts and observers for a subsequent wait.
    pub async fn drain(&self, deadline: Duration) -> Result<Vec<ReceiptState>, Refusal> {
        let receivers: Vec<_> = {
            let inner = self.inner.lock().unwrap();
            if !inner.closed {
                return Err(Refusal::Pending);
            }
            inner
                .entries
                .values()
                .map(|e| e.state.subscribe())
                .collect()
        };
        tokio::time::timeout(deadline, async move {
            let mut states = Vec::new();
            for receiver in receivers {
                states.push(wait_terminal(receiver).await?);
            }
            Ok(states)
        })
        .await
        .map_err(|_| Refusal::Timeout)?
    }
}
async fn wait_terminal(
    mut receiver: watch::Receiver<ReceiptState>,
) -> Result<ReceiptState, Refusal> {
    loop {
        let state = receiver.borrow_and_update().clone();
        if state.terminal() {
            return Ok(state);
        }
        receiver.changed().await.map_err(|_| Refusal::Unavailable)?;
    }
}
struct FinishGuard {
    owner: Owner,
    id: ReceiptId,
    state: Option<ReceiptState>,
}
impl Drop for FinishGuard {
    fn drop(&mut self) {
        if let Some(entry) = self.owner.inner.lock().unwrap().entries.get(&self.id.0) {
            entry
                .state
                .send_replace(self.state.take().unwrap_or(ReceiptState::Interrupted));
        }
    }
}
impl Reservation {
    /// Bind an internal native check to the same owner and trusted scope.
    pub fn bind_foreground_guard(
        &mut self,
        check: impl Fn() -> Option<ForegroundGuardReport> + Send + Sync + 'static,
    ) {
        self.owner
            .inner
            .lock()
            .unwrap()
            .entries
            .get_mut(&self.id.0)
            .unwrap()
            .foreground_guard = Some(Arc::new(check));
    }
    /// Bind driver-owned callback evidence before ownership transfer. This is
    /// not a public tool argument and carries no application outcome claim.
    pub fn bind_activation_signal(&mut self, signal: Arc<std::sync::atomic::AtomicBool>) {
        self.owner
            .inner
            .lock()
            .unwrap()
            .entries
            .get_mut(&self.id.0)
            .unwrap()
            .activation_signal = Some(signal);
    }
    /// No await occurs between transfer and task launch. The task owns the
    /// observer/protection future, not the caller's request or fence future.
    pub fn supervise<F>(mut self, observer: F) -> ReceiptId
    where
        F: Future<Output = Observation> + Send + 'static,
    {
        let guard = FinishGuard {
            owner: self.owner.clone(),
            id: self.id.clone(),
            state: None,
        };
        self.owner
            .inner
            .lock()
            .unwrap()
            .entries
            .get(&self.id.0)
            .unwrap()
            .state
            .send_replace(ReceiptState::Pending);
        self.transferred = true;
        self.runtime.spawn(async move {
            let mut guard = guard;
            guard.state = Some(match AssertUnwindSafe(observer).catch_unwind().await {
                Ok(observation) if observation.polled => ReceiptState::Finished(observation),
                _ => ReceiptState::Failed,
            });
        });
        self.id.clone()
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.transferred {
            if let Some(entry) = self.owner.inner.lock().unwrap().entries.remove(&self.id.0) {
                entry.state.send_replace(ReceiptState::Interrupted);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;
    fn observation() -> Observation {
        Observation {
            polled: true,
            foreground_changed: false,
            new_window_count: 0,
        }
    }
    #[tokio::test]
    async fn foreground_guard_is_fresh_scoped_and_absence_is_explicit() {
        let owner = Owner::new(1).unwrap();
        let context = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut reservation = owner.reserve("owner").unwrap();
        let state = context.clone();
        reservation.bind_foreground_guard(move || {
            Some(ForegroundGuardReport {
                foreground_preserved: state.load(std::sync::atomic::Ordering::Acquire),
                physical_input_unchanged: true,
            })
        });
        let id = reservation.supervise(async { observation() });
        assert_eq!(
            owner
                .foreground_guard("owner", &id)
                .unwrap()
                .unwrap()
                .foreground_preserved,
            true
        );
        context.store(false, std::sync::atomic::Ordering::Release);
        assert_eq!(
            owner
                .foreground_guard("owner", &id)
                .unwrap()
                .unwrap()
                .foreground_preserved,
            false
        );
        assert_eq!(
            owner.foreground_guard("other", &id),
            Err(Refusal::Unavailable)
        );
        owner
            .fence("owner", &id, Duration::from_secs(1))
            .await
            .unwrap();
        owner.release("owner", &id).unwrap();
        let id = owner
            .reserve("owner")
            .unwrap()
            .supervise(async { observation() });
        assert_eq!(owner.foreground_guard("owner", &id), Ok(None));
    }

    #[tokio::test]
    async fn activation_signal_is_live_scoped_and_retained_while_pending() {
        let owner = Owner::new(1).unwrap();
        let signal = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut reservation = owner.reserve("owner").unwrap();
        reservation.bind_activation_signal(signal.clone());
        let (send, receive) = tokio::sync::oneshot::channel();
        let id = reservation.supervise(async move {
            receive.await.unwrap();
            observation()
        });
        assert_eq!(owner.activation_observed("owner", &id), Ok(Some(false)));
        signal.store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(owner.activation_observed("owner", &id), Ok(Some(true)));
        assert_eq!(
            owner.activation_observed("other", &id),
            Err(Refusal::Unavailable)
        );
        assert_eq!(owner.read("owner", &id), Ok(ReceiptState::Pending));
        send.send(()).unwrap();
        owner
            .fence("owner", &id, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(owner.activation_observed("owner", &id), Ok(Some(true)));
        owner.release("owner", &id).unwrap();
        assert_eq!(
            owner.activation_observed("owner", &id),
            Err(Refusal::Unavailable)
        );
    }

    #[tokio::test]
    async fn cancelled_waiter_does_not_cancel_observer_or_its_lease() {
        let owner = Owner::new(1).unwrap();
        let (complete, wait) = oneshot::channel();
        let (lease_dropped, dropped) = oneshot::channel();
        struct Lease(Option<oneshot::Sender<()>>);
        impl Drop for Lease {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let lease = Lease(Some(lease_dropped));
        let id = owner.reserve("a").unwrap().supervise(async move {
            let _lease = lease;
            wait.await.unwrap();
            observation()
        });
        let waiter_owner = owner.clone();
        let waiter_id = id.clone();
        let waiter = tokio::spawn(async move {
            waiter_owner
                .fence("a", &waiter_id, Duration::from_secs(5))
                .await
        });
        tokio::task::yield_now().await;
        waiter.abort();
        let _ = waiter.await;
        assert_eq!(owner.read("a", &id), Ok(ReceiptState::Pending));
        assert_eq!(owner.release("a", &id), Err(Refusal::Pending));
        assert_eq!(owner.reserve("a").err(), Some(Refusal::Capacity));
        complete.send(()).unwrap();
        assert_eq!(
            owner.fence("a", &id, Duration::from_secs(1)).await,
            Ok(ReceiptState::Finished(observation()))
        );
        dropped.await.unwrap();
        owner.release("a", &id).unwrap();
        assert!(owner.reserve("a").is_ok());
    }
    #[tokio::test]
    async fn timed_out_fence_and_drain_retain_admitted_work() {
        let owner = Owner::new(1).unwrap();
        let (complete, wait) = oneshot::channel();
        let id = owner.reserve("a").unwrap().supervise(async move {
            wait.await.unwrap();
            observation()
        });
        owner.close();
        assert_eq!(owner.reserve("a").err(), Some(Refusal::Closed));
        assert_eq!(
            owner.fence("a", &id, Duration::ZERO).await,
            Err(Refusal::Timeout)
        );
        assert_eq!(owner.drain(Duration::ZERO).await, Err(Refusal::Timeout));
        complete.send(()).unwrap();
        assert_eq!(
            owner.drain(Duration::from_secs(1)).await,
            Ok(vec![ReceiptState::Finished(observation())])
        );
    }
    #[tokio::test]
    async fn foreign_scope_cannot_read_wait_or_release_and_ids_are_not_reused() {
        let owner = Owner::new(1).unwrap();
        let id = owner
            .reserve("a")
            .unwrap()
            .supervise(async { observation() });
        assert_eq!(owner.read("b", &id), Err(Refusal::Unavailable));
        assert_eq!(
            owner.fence("b", &id, Duration::ZERO).await,
            Err(Refusal::Unavailable)
        );
        assert_eq!(owner.release("b", &id), Err(Refusal::Unavailable));
        owner.fence("a", &id, Duration::from_secs(1)).await.unwrap();
        owner.release("a", &id).unwrap();
        let other = owner
            .reserve("a")
            .unwrap()
            .supervise(async { observation() });
        assert_ne!(id, other);
        assert_eq!(owner.read("a", &id), Err(Refusal::Unavailable));
    }
    #[tokio::test]
    async fn abandoned_reservation_releases_capacity_and_wakes_drain() {
        let owner = Owner::new(1).unwrap();
        let reservation = owner.reserve("a").unwrap();
        owner.close();
        drop(reservation);
        assert_eq!(owner.drain(Duration::from_secs(1)).await, Ok(vec![]));
    }
    #[tokio::test]
    async fn unpolled_or_panicked_observer_cannot_claim_finished() {
        let owner = Owner::new(2).unwrap();
        let unpolled = owner.reserve("a").unwrap().supervise(async {
            Observation {
                polled: false,
                ..observation()
            }
        });
        let panicked = owner.reserve("a").unwrap().supervise(async {
            panic!("observer lost");
        });
        for id in [unpolled, panicked] {
            assert_eq!(
                owner.fence("a", &id, Duration::from_secs(1)).await,
                Ok(ReceiptState::Failed)
            );
        }
    }
    #[tokio::test]
    async fn one_failed_observer_is_not_hidden_by_another_success() {
        let owner = Owner::new(2).unwrap();
        owner.reserve("a").unwrap().supervise(async {
            Observation {
                polled: false,
                ..observation()
            }
        });
        owner
            .reserve("a")
            .unwrap()
            .supervise(async { observation() });
        owner.close();
        let states = owner.drain(Duration::from_secs(1)).await.unwrap();
        assert!(states.contains(&ReceiptState::Failed));
        assert!(states.contains(&ReceiptState::Finished(observation())));
    }
    #[test]
    fn executor_shutdown_marks_pending_observation_interrupted() {
        let owner = Owner::new(1).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let id = runtime.block_on(async {
            let id = owner.reserve("a").unwrap().supervise(async {
                std::future::pending::<()>().await;
                observation()
            });
            tokio::task::yield_now().await;
            id
        });
        assert_eq!(owner.read("a", &id), Ok(ReceiptState::Pending));
        drop(runtime);
        assert_eq!(owner.read("a", &id), Ok(ReceiptState::Interrupted));
    }

    #[test]
    fn missing_executor_and_invalid_capacity_refuse_before_admission() {
        assert!(matches!(Owner::new(0), Err(Refusal::InvalidCapacity)));
        let owner = Owner::new(1).unwrap();
        assert_eq!(owner.reserve("a").err(), Some(Refusal::RuntimeUnavailable));
        assert_eq!(owner.reserve("").err(), Some(Refusal::InvalidScope));
    }
}
