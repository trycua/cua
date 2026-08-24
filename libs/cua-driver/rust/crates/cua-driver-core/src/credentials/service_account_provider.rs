use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::{
    BootstrapNamespace, BootstrapSecret, BootstrapSecretLease, BootstrapSecretStore,
    BootstrapStoreError, BootstrapStoreHealth, BootstrapVersion, CredentialConfigurationError,
    CredentialProvider, CredentialProviderClass, CredentialProviderError,
    CredentialProviderErrorKind, CredentialProviderId, PrivateCredentialFieldLocator,
    ProviderHealth, ProviderHealthState, ProviderReleaseId, ProviderReleaseRequest, ReleasePlan,
    SecretBroker, SecretLease,
};

const MAX_CANCELLATION_TOMBSTONES: usize = 4_096;

/// Private provider client used only by trusted host configuration.
///
/// Implementations receive the bootstrap lease and private locator only for a
/// single authorized release. They must never include either value, provider
/// diagnostics, or returned secret material in errors, logs, stdout, or stderr.
#[async_trait]
pub trait ServiceAccountVaultClient: Send + Sync {
    async fn release(
        &self,
        bootstrap: &BootstrapSecretLease,
        locator: &PrivateCredentialFieldLocator,
        release: ProviderReleaseId,
        cancellation: &ServiceAccountReleaseCancellation,
    ) -> Result<SecretLease, CredentialProviderError>;

    async fn cancel(&self, release: ProviderReleaseId) -> Result<(), CredentialProviderError>;

    async fn health(&self) -> ProviderHealth;
}

/// Cancellation state visible to a trusted provider client without revealing
/// the target, session, binding, or authorization context.
pub struct ServiceAccountReleaseCancellation {
    broker_cancelled: Arc<AtomicBool>,
    provider_cancelled: Arc<AtomicBool>,
    provider_enabled: Arc<AtomicBool>,
}

impl ServiceAccountReleaseCancellation {
    pub fn is_cancelled(&self) -> bool {
        self.broker_cancelled.load(Ordering::Acquire)
            || self.provider_cancelled.load(Ordering::Acquire)
            || !self.provider_enabled.load(Ordering::Acquire)
    }

    pub fn ensure_not_cancelled(&self) -> Result<(), CredentialProviderError> {
        if self.is_cancelled() {
            Err(provider_error(CredentialProviderErrorKind::Cancelled))
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for ServiceAccountReleaseCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ServiceAccountReleaseCancellation([private])")
    }
}

/// Provider for a dedicated automation service account and least-privilege
/// vault. Bootstrap credentials remain in the platform store and are loaded
/// only for the duration of one release.
pub struct DedicatedServiceAccountProvider {
    id: CredentialProviderId,
    namespace: BootstrapNamespace,
    store: Arc<dyn BootstrapSecretStore>,
    client: Arc<dyn ServiceAccountVaultClient>,
    enabled: Arc<AtomicBool>,
    releases: Arc<Mutex<ServiceAccountReleaseState>>,
}

impl DedicatedServiceAccountProvider {
    pub fn new(
        id: CredentialProviderId,
        namespace: BootstrapNamespace,
        store: Arc<dyn BootstrapSecretStore>,
        client: Arc<dyn ServiceAccountVaultClient>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            namespace,
            store,
            client,
            enabled: Arc::new(AtomicBool::new(true)),
            releases: Arc::new(Mutex::new(ServiceAccountReleaseState::default())),
        })
    }

    fn begin_release(
        &self,
        request: &ProviderReleaseRequest,
    ) -> Result<ActiveServiceAccountRelease, CredentialProviderError> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err(provider_error(CredentialProviderErrorKind::Unavailable));
        }
        let mut state = self.releases.lock().unwrap();
        if state.tombstones.contains(&request.release_id()) {
            return Err(provider_error(CredentialProviderErrorKind::Cancelled));
        }
        if state.active.contains_key(&request.release_id()) {
            return Err(provider_error(
                CredentialProviderErrorKind::MalformedResponse,
            ));
        }
        let provider_cancelled = Arc::new(AtomicBool::new(false));
        state
            .active
            .insert(request.release_id(), provider_cancelled.clone());
        Ok(ActiveServiceAccountRelease {
            id: request.release_id(),
            state: self.releases.clone(),
            provider_cancelled: provider_cancelled.clone(),
            cancellation: ServiceAccountReleaseCancellation {
                broker_cancelled: request.cancelled.clone(),
                provider_cancelled,
                provider_enabled: self.enabled.clone(),
            },
        })
    }

    fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
        let mut state = self.releases.lock().unwrap();
        let active = state.active.keys().copied().collect::<Vec<_>>();
        for release in active {
            if let Some(cancelled) = state.active.get(&release) {
                cancelled.store(true, Ordering::Release);
            }
            state.insert_tombstone(release);
        }
    }

    fn store_health(&self) -> BootstrapStoreHealth {
        self.store.health(&self.namespace)
    }
}

impl fmt::Debug for DedicatedServiceAccountProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DedicatedServiceAccountProvider")
            .field("id", &self.id)
            .field("namespace", &self.namespace)
            .field("class", &CredentialProviderClass::ServiceAccountVault)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CredentialProvider for DedicatedServiceAccountProvider {
    fn id(&self) -> &CredentialProviderId {
        &self.id
    }

    fn class(&self) -> CredentialProviderClass {
        CredentialProviderClass::ServiceAccountVault
    }

    async fn release(
        &self,
        request: &ProviderReleaseRequest,
    ) -> Result<ReleasePlan, CredentialProviderError> {
        request.ensure_not_cancelled()?;
        let active = self.begin_release(request)?;
        active.cancellation.ensure_not_cancelled()?;

        request.ensure_not_cancelled()?;
        active.cancellation.ensure_not_cancelled()?;
        let store = self.store.clone();
        let namespace = self.namespace.clone();
        let bootstrap = tokio::task::spawn_blocking(move || store.load(&namespace))
            .await
            .map_err(|_| provider_error(CredentialProviderErrorKind::ProviderDied))?
            .map_err(map_bootstrap_error)?;

        request.ensure_not_cancelled()?;
        active.cancellation.ensure_not_cancelled()?;
        let secret = self
            .client
            .release(
                &bootstrap,
                request.locator(),
                request.release_id(),
                &active.cancellation,
            )
            .await?;
        request.ensure_not_cancelled()?;
        active.cancellation.ensure_not_cancelled()?;
        Ok(ReleasePlan::RuntimeDelivers(secret))
    }

    async fn cancel(&self, release: ProviderReleaseId) -> Result<(), CredentialProviderError> {
        let active = {
            let mut state = self.releases.lock().unwrap();
            state.insert_tombstone(release);
            state.active.remove(&release)
        };
        if let Some(cancelled) = active {
            cancelled.store(true, Ordering::Release);
        }
        self.client.cancel(release).await
    }

    async fn health(&self) -> ProviderHealth {
        if !self.enabled.load(Ordering::Acquire) {
            return ProviderHealth {
                state: ProviderHealthState::Unavailable,
            };
        }
        let store = self.store.clone();
        let namespace = self.namespace.clone();
        let store_health = match tokio::task::spawn_blocking(move || store.health(&namespace)).await
        {
            Ok(health) => health,
            Err(_) => BootstrapStoreHealth::Unavailable,
        };
        match store_health {
            BootstrapStoreHealth::Available => self.client.health().await,
            BootstrapStoreHealth::Locked => ProviderHealth {
                state: ProviderHealthState::Locked,
            },
            BootstrapStoreHealth::Missing
            | BootstrapStoreHealth::Unavailable
            | BootstrapStoreHealth::Corrupt => ProviderHealth {
                state: ProviderHealthState::Unavailable,
            },
        }
    }
}

/// Host-only bootstrap lifecycle controller. Rotation and revocation first
/// disable the provider and revoke all broker bindings, handles, and active
/// releases. Re-enabling requires rebuilding trusted host configuration.
pub struct DedicatedServiceAccountBootstrap {
    provider: Arc<DedicatedServiceAccountProvider>,
    broker: Arc<SecretBroker>,
}

impl DedicatedServiceAccountBootstrap {
    pub fn new(
        provider: Arc<DedicatedServiceAccountProvider>,
        broker: Arc<SecretBroker>,
    ) -> Result<Self, CredentialConfigurationError> {
        let configured = broker
            .providers
            .get(provider.id())
            .ok_or(CredentialConfigurationError::MissingProvider)?;
        let provider_trait: Arc<dyn CredentialProvider> = provider.clone();
        if !Arc::ptr_eq(configured, &provider_trait) {
            return Err(CredentialConfigurationError::MissingProvider);
        }
        Ok(Self { provider, broker })
    }

    pub fn rotate(&self, value: BootstrapSecret) -> Result<BootstrapVersion, BootstrapStoreError> {
        self.invalidate();
        self.provider.store.rotate(&self.provider.namespace, value)
    }

    pub fn revoke(&self) -> Result<(), BootstrapStoreError> {
        self.invalidate();
        self.provider.store.revoke(&self.provider.namespace)
    }

    pub fn health(&self) -> BootstrapStoreHealth {
        self.provider.store_health()
    }

    fn invalidate(&self) {
        self.provider.disable();
        self.broker.revoke_provider(self.provider.id());
    }
}

impl fmt::Debug for DedicatedServiceAccountBootstrap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DedicatedServiceAccountBootstrap([private])")
    }
}

#[derive(Default)]
struct ServiceAccountReleaseState {
    active: HashMap<ProviderReleaseId, Arc<AtomicBool>>,
    tombstones: HashSet<ProviderReleaseId>,
    tombstone_order: VecDeque<ProviderReleaseId>,
}

impl ServiceAccountReleaseState {
    fn insert_tombstone(&mut self, release: ProviderReleaseId) {
        if self.tombstones.insert(release) {
            self.tombstone_order.push_back(release);
        }
        while self.tombstone_order.len() > MAX_CANCELLATION_TOMBSTONES {
            if let Some(expired) = self.tombstone_order.pop_front() {
                self.tombstones.remove(&expired);
            }
        }
    }
}

struct ActiveServiceAccountRelease {
    id: ProviderReleaseId,
    state: Arc<Mutex<ServiceAccountReleaseState>>,
    provider_cancelled: Arc<AtomicBool>,
    cancellation: ServiceAccountReleaseCancellation,
}

impl Drop for ActiveServiceAccountRelease {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if state
            .active
            .get(&self.id)
            .is_some_and(|active| Arc::ptr_eq(active, &self.provider_cancelled))
        {
            state.active.remove(&self.id);
        }
    }
}

fn map_bootstrap_error(error: BootstrapStoreError) -> CredentialProviderError {
    provider_error(match error {
        BootstrapStoreError::Locked => CredentialProviderErrorKind::Locked,
        BootstrapStoreError::Missing | BootstrapStoreError::Unavailable => {
            CredentialProviderErrorKind::Unavailable
        }
        BootstrapStoreError::InvalidNamespace
        | BootstrapStoreError::InvalidValue
        | BootstrapStoreError::AlreadyEnrolled
        | BootstrapStoreError::Corrupt => CredentialProviderErrorKind::MalformedResponse,
    })
}

fn provider_error(kind: CredentialProviderErrorKind) -> CredentialProviderError {
    CredentialProviderError::new(kind)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::time::SystemTime;

    use tokio::sync::Notify;
    use zeroize::Zeroizing;

    use super::*;
    use crate::browser::store::SecureFieldKind;
    use crate::browser::types::ProcessFingerprint;
    use crate::credentials::{
        BrowserSecretFrameIdentity, BrowserSecretFrameKind, BrowserSecretTargetConstraint,
        CredentialBindingSpec, CredentialField, PrivateCredentialField, SafeCredentialLabel,
        SecretReleaseAuthorization, SecretTargetConstraint, VerifiedBrowserSecretTarget,
    };

    #[derive(Default)]
    struct MemoryBootstrapStore {
        record: Mutex<Option<(BootstrapVersion, Zeroizing<Vec<u8>>)>>,
        load_error: Mutex<Option<BootstrapStoreError>>,
        loads: AtomicUsize,
    }

    impl MemoryBootstrapStore {
        fn with_secret(value: &[u8]) -> Arc<Self> {
            let store = Arc::new(Self::default());
            store
                .enroll(
                    BootstrapNamespace::new("synthetic-service-account").unwrap(),
                    BootstrapSecret::from_bytes(value.to_vec()).unwrap(),
                )
                .unwrap();
            store
        }

        fn fail_load_with(&self, error: BootstrapStoreError) {
            *self.load_error.lock().unwrap() = Some(error);
        }
    }

    impl BootstrapSecretStore for MemoryBootstrapStore {
        fn enroll(
            &self,
            _namespace: BootstrapNamespace,
            value: BootstrapSecret,
        ) -> Result<BootstrapVersion, BootstrapStoreError> {
            let mut record = self.record.lock().unwrap();
            if record.is_some() {
                return Err(BootstrapStoreError::AlreadyEnrolled);
            }
            *record = Some((
                BootstrapVersion(1),
                Zeroizing::new(value.into_lease().expose_to_provider().to_vec()),
            ));
            Ok(BootstrapVersion(1))
        }

        fn load(
            &self,
            _namespace: &BootstrapNamespace,
        ) -> Result<BootstrapSecretLease, BootstrapStoreError> {
            self.loads.fetch_add(1, AtomicOrdering::SeqCst);
            if let Some(error) = self.load_error.lock().unwrap().take() {
                return Err(error);
            }
            let record = self.record.lock().unwrap();
            let (_, value) = record.as_ref().ok_or(BootstrapStoreError::Missing)?;
            Ok(BootstrapSecret::from_bytes(value.as_slice().to_vec())?.into_lease())
        }

        fn rotate(
            &self,
            _namespace: &BootstrapNamespace,
            value: BootstrapSecret,
        ) -> Result<BootstrapVersion, BootstrapStoreError> {
            let mut record = self.record.lock().unwrap();
            let (version, _) = record.as_ref().ok_or(BootstrapStoreError::Missing)?;
            let next = BootstrapVersion(
                version
                    .0
                    .checked_add(1)
                    .ok_or(BootstrapStoreError::Corrupt)?,
            );
            *record = Some((
                next,
                Zeroizing::new(value.into_lease().expose_to_provider().to_vec()),
            ));
            Ok(next)
        }

        fn revoke(&self, _namespace: &BootstrapNamespace) -> Result<(), BootstrapStoreError> {
            self.record.lock().unwrap().take();
            Ok(())
        }

        fn health(&self, _namespace: &BootstrapNamespace) -> BootstrapStoreHealth {
            if self.record.lock().unwrap().is_some() {
                BootstrapStoreHealth::Available
            } else {
                BootstrapStoreHealth::Missing
            }
        }
    }

    struct SyntheticVaultClient {
        bootstrap: Zeroizing<Vec<u8>>,
        locator: Zeroizing<Vec<u8>>,
        secret: Zeroizing<Vec<u8>>,
        entered: Option<Arc<Notify>>,
        resume: Option<Arc<Notify>>,
        releases: AtomicUsize,
        effects: AtomicUsize,
        cancellations: AtomicUsize,
    }

    impl SyntheticVaultClient {
        fn immediate() -> Arc<Self> {
            Arc::new(Self {
                bootstrap: Zeroizing::new(b"synthetic-bootstrap".to_vec()),
                locator: Zeroizing::new(b"vault://private/password".to_vec()),
                secret: Zeroizing::new(b"synthetic-delivery-canary".to_vec()),
                entered: None,
                resume: None,
                releases: AtomicUsize::new(0),
                effects: AtomicUsize::new(0),
                cancellations: AtomicUsize::new(0),
            })
        }

        fn blocking(entered: Arc<Notify>, resume: Arc<Notify>) -> Arc<Self> {
            Arc::new(Self {
                entered: Some(entered),
                resume: Some(resume),
                ..Self::immediate().as_ref().clone_for_test()
            })
        }

        fn clone_for_test(&self) -> Self {
            Self {
                bootstrap: Zeroizing::new(self.bootstrap.as_slice().to_vec()),
                locator: Zeroizing::new(self.locator.as_slice().to_vec()),
                secret: Zeroizing::new(self.secret.as_slice().to_vec()),
                entered: self.entered.clone(),
                resume: self.resume.clone(),
                releases: AtomicUsize::new(0),
                effects: AtomicUsize::new(0),
                cancellations: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ServiceAccountVaultClient for SyntheticVaultClient {
        async fn release(
            &self,
            bootstrap: &BootstrapSecretLease,
            locator: &PrivateCredentialFieldLocator,
            _release: ProviderReleaseId,
            cancellation: &ServiceAccountReleaseCancellation,
        ) -> Result<SecretLease, CredentialProviderError> {
            self.releases.fetch_add(1, AtomicOrdering::SeqCst);
            cancellation.ensure_not_cancelled()?;
            if bootstrap.expose_to_provider() != self.bootstrap.as_slice() {
                return Err(provider_error(
                    CredentialProviderErrorKind::PermissionDenied,
                ));
            }
            if locator.as_str().as_bytes() != self.locator.as_slice() {
                return Err(provider_error(CredentialProviderErrorKind::NotFound));
            }
            if let (Some(entered), Some(resume)) = (&self.entered, &self.resume) {
                entered.notify_waiters();
                resume.notified().await;
            }
            cancellation.ensure_not_cancelled()?;
            self.effects.fetch_add(1, AtomicOrdering::SeqCst);
            SecretLease::from_utf8_bytes(self.secret.as_slice().to_vec())
                .map_err(|_| provider_error(CredentialProviderErrorKind::MalformedResponse))
        }

        async fn cancel(&self, _release: ProviderReleaseId) -> Result<(), CredentialProviderError> {
            self.cancellations.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }

        async fn health(&self) -> ProviderHealth {
            ProviderHealth {
                state: ProviderHealthState::Available,
            }
        }
    }

    fn provider(
        store: Arc<MemoryBootstrapStore>,
        client: Arc<SyntheticVaultClient>,
    ) -> Arc<DedicatedServiceAccountProvider> {
        DedicatedServiceAccountProvider::new(
            CredentialProviderId::new("dedicated-service-account").unwrap(),
            BootstrapNamespace::new("synthetic-service-account").unwrap(),
            store,
            client,
        )
    }

    fn request(release_id: ProviderReleaseId) -> ProviderReleaseRequest {
        ProviderReleaseRequest {
            release_id,
            field: CredentialField::Password,
            locator: PrivateCredentialFieldLocator::new("vault://private/password").unwrap(),
            target: target(
                "service-account-runtime",
                "__cua_runtime_service-account-runtime:agent",
            ),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn target(runtime: &str, session: &str) -> VerifiedBrowserSecretTarget {
        VerifiedBrowserSecretTarget::new(
            runtime.to_owned(),
            session.to_owned(),
            ProcessFingerprint {
                pid: 42,
                start_time: Some(9),
                executable: Some("/synthetic/browser".to_owned()),
            },
            3,
            "browser-target".to_owned(),
            "tab".to_owned(),
            BrowserSecretFrameIdentity {
                kind: BrowserSecretFrameKind::Main,
                frame_id: "main-frame".to_owned(),
                loader_id: "loader".to_owned(),
            },
            "https://accounts.example.test/login",
            "p1:1".to_owned(),
            SecureFieldKind::Password,
        )
        .unwrap()
    }

    fn binding(session: &str, provider: &CredentialProviderId) -> CredentialBindingSpec {
        CredentialBindingSpec {
            session: session.to_owned(),
            provider: provider.clone(),
            private_fields: vec![PrivateCredentialField {
                field: CredentialField::Password,
                locator: PrivateCredentialFieldLocator::new("vault://private/password").unwrap(),
            }],
            safe_label: Some(SafeCredentialLabel::new("Synthetic login").unwrap()),
            allowed_targets: vec![SecretTargetConstraint::Browser(
                BrowserSecretTargetConstraint::exact_password_origin(
                    "https://accounts.example.test",
                )
                .unwrap(),
            )],
            expires_at: None,
            max_releases: None,
            authorization: SecretReleaseAuthorization::trusted_scope(
                "synthetic-service-account-release",
            )
            .unwrap(),
        }
    }

    #[tokio::test]
    async fn provider_loads_bootstrap_only_for_release_and_returns_one_runtime_lease() {
        let store = MemoryBootstrapStore::with_secret(b"synthetic-bootstrap");
        let client = SyntheticVaultClient::immediate();
        let provider = provider(store.clone(), client.clone());
        assert_eq!(store.loads.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(
            provider.health().await.state,
            ProviderHealthState::Available
        );
        assert_eq!(store.loads.load(AtomicOrdering::SeqCst), 0);

        let plan = provider
            .release(&request(ProviderReleaseId(uuid::Uuid::new_v4())))
            .await
            .unwrap();
        let ReleasePlan::RuntimeDelivers(secret) = plan else {
            panic!("dedicated service account returned a non-runtime plan");
        };
        assert_eq!(secret.expose_for_delivery(), "synthetic-delivery-canary");
        assert_eq!(store.loads.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(client.releases.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(client.effects.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancel_before_release_is_tombstoned_before_store_or_client_access() {
        let store = MemoryBootstrapStore::with_secret(b"synthetic-bootstrap");
        let client = SyntheticVaultClient::immediate();
        let provider = provider(store.clone(), client.clone());
        let release = ProviderReleaseId(uuid::Uuid::new_v4());
        provider.cancel(release).await.unwrap();

        assert_eq!(
            provider
                .release(&request(release))
                .await
                .err()
                .map(|e| e.kind()),
            Some(CredentialProviderErrorKind::Cancelled)
        );
        assert_eq!(store.loads.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(client.releases.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(client.cancellations.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancel_during_provider_io_prevents_the_synthetic_effect() {
        let entered = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let store = MemoryBootstrapStore::with_secret(b"synthetic-bootstrap");
        let client = SyntheticVaultClient::blocking(entered.clone(), resume.clone());
        let provider = provider(store, client.clone());
        let release = ProviderReleaseId(uuid::Uuid::new_v4());
        let request = request(release);
        let provider_for_task = provider.clone();
        let pending = tokio::spawn(async move { provider_for_task.release(&request).await });
        entered.notified().await;

        provider.cancel(release).await.unwrap();
        resume.notify_waiters();
        assert_eq!(
            pending.await.unwrap().err().map(|error| error.kind()),
            Some(CredentialProviderErrorKind::Cancelled)
        );
        assert_eq!(client.effects.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(client.cancellations.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn bootstrap_failures_map_to_fixed_provider_errors() {
        for (store_error, expected) in [
            (
                BootstrapStoreError::Locked,
                CredentialProviderErrorKind::Locked,
            ),
            (
                BootstrapStoreError::Missing,
                CredentialProviderErrorKind::Unavailable,
            ),
            (
                BootstrapStoreError::Corrupt,
                CredentialProviderErrorKind::MalformedResponse,
            ),
        ] {
            let store = MemoryBootstrapStore::with_secret(b"synthetic-bootstrap");
            store.fail_load_with(store_error);
            let client = SyntheticVaultClient::immediate();
            let provider = provider(store, client.clone());
            assert_eq!(
                provider
                    .release(&request(ProviderReleaseId(uuid::Uuid::new_v4())))
                    .await
                    .err()
                    .map(|error| error.kind()),
                Some(expected)
            );
            assert_eq!(client.releases.load(AtomicOrdering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn bootstrap_rotation_revokes_broker_state_and_requires_rebuild() {
        let store = MemoryBootstrapStore::with_secret(b"synthetic-bootstrap");
        let client = SyntheticVaultClient::immediate();
        let provider = provider(store, client);
        let mut builder =
            super::super::SecretBrokerBuilder::new("service-account-runtime").unwrap();
        let session = builder.runtime_session_key("agent").unwrap();
        builder.add_provider(provider.clone()).unwrap();
        builder
            .add_binding(binding(&session, provider.id()))
            .unwrap();
        let broker = builder.build().unwrap();
        let target = target("service-account-runtime", &session);
        assert_eq!(
            broker
                .find_credentials(&target, SystemTime::now())
                .unwrap()
                .len(),
            1
        );

        let lifecycle =
            DedicatedServiceAccountBootstrap::new(provider.clone(), broker.clone()).unwrap();
        assert_eq!(
            lifecycle
                .rotate(
                    BootstrapSecret::from_bytes(b"synthetic-bootstrap-rotated".to_vec()).unwrap()
                )
                .unwrap(),
            BootstrapVersion(2)
        );
        assert!(broker
            .find_credentials(&target, SystemTime::now())
            .unwrap()
            .is_empty());
        assert_eq!(
            provider
                .release(&request(ProviderReleaseId(uuid::Uuid::new_v4())))
                .await
                .err()
                .map(|error| error.kind()),
            Some(CredentialProviderErrorKind::Unavailable)
        );
    }

    #[test]
    fn provider_debug_output_omits_provider_and_bootstrap_identifiers() {
        let store = MemoryBootstrapStore::with_secret(b"synthetic-bootstrap-private");
        let client = SyntheticVaultClient::immediate();
        let provider = provider(store, client);
        let debug = format!("{provider:?}");
        assert!(!debug.contains("dedicated-service-account"));
        assert!(!debug.contains("synthetic-service-account"));
        assert!(!debug.contains("synthetic-bootstrap-private"));
        let request = request(ProviderReleaseId(uuid::Uuid::new_v4()));
        let active = provider.begin_release(&request).unwrap();
        assert_eq!(
            format!("{:?}", active.cancellation),
            "ServiceAccountReleaseCancellation([private])"
        );
    }
}
