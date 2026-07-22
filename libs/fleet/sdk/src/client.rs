use crate::{
    AccessTokenProvider, CyclopsConfiguration, CyclopsTokenProviderConfiguration, HttpClient,
    HttpRequest, HttpResponse, SdkError,
    transport::{AuthenticatedRequestClass, Transport},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Mutex as AsyncMutex;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::OwnedMutexGuard;
use url::Url;

#[derive(uniffi::Object)]
pub struct CyclopsClient {
    base_url: Url,
    claim_sequence: AtomicU64,
    claim_poll_interval_ms: u64,
    claim_poll_limit: u32,
    lifecycle_locks: Arc<NamespaceLifecycleLocks>,
    transport: Transport,
}

struct NamespaceLifecycleLocks {
    namespaces: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

pub(crate) struct NamespaceLifecycleGuard {
    #[cfg(not(target_arch = "wasm32"))]
    guard: Option<OwnedMutexGuard<()>>,
    #[cfg(not(target_arch = "wasm32"))]
    lock: Arc<AsyncMutex<()>>,
    #[cfg(not(target_arch = "wasm32"))]
    locks: Arc<NamespaceLifecycleLocks>,
    #[cfg(not(target_arch = "wasm32"))]
    namespace: String,
}

#[uniffi::export]
impl CyclopsClient {
    #[uniffi::constructor]
    pub fn connect(
        configuration: CyclopsConfiguration,
        http_client: Arc<dyn HttpClient>,
    ) -> Result<Arc<Self>, SdkError> {
        validate_configuration(&configuration)?;
        let base_url =
            Url::parse(&configuration.base_url).map_err(|error| SdkError::Configuration {
                reason: format!("base_url must be a valid URL: {error}"),
            })?;
        let transport = Transport::new(&configuration, http_client)?;

        Ok(Arc::new(Self {
            base_url,
            claim_sequence: AtomicU64::new(0),
            claim_poll_interval_ms: configuration.claim_poll_interval_ms,
            claim_poll_limit: configuration.claim_poll_limit,
            lifecycle_locks: Arc::new(NamespaceLifecycleLocks {
                namespaces: Mutex::new(HashMap::new()),
            }),
            transport,
        }))
    }

    #[uniffi::constructor]
    pub fn connect_with_access_token_provider(
        configuration: CyclopsTokenProviderConfiguration,
        token_provider: Arc<dyn AccessTokenProvider>,
        http_client: Arc<dyn HttpClient>,
    ) -> Result<Arc<Self>, SdkError> {
        validate_token_provider_configuration(&configuration)?;
        let base_url =
            Url::parse(&configuration.base_url).map_err(|error| SdkError::Configuration {
                reason: format!("base_url must be a valid URL: {error}"),
            })?;
        let transport =
            Transport::new_with_access_token_provider(&configuration, token_provider, http_client)?;

        Ok(Arc::new(Self {
            base_url,
            claim_sequence: AtomicU64::new(0),
            claim_poll_interval_ms: configuration.claim_poll_interval_ms,
            claim_poll_limit: configuration.claim_poll_limit,
            lifecycle_locks: Arc::new(NamespaceLifecycleLocks {
                namespaces: Mutex::new(HashMap::new()),
            }),
            transport,
        }))
    }

    #[uniffi::constructor]
    pub fn connect_with_access_token(
        configuration: CyclopsTokenProviderConfiguration,
        access_token: String,
        http_client: Arc<dyn HttpClient>,
    ) -> Result<Arc<Self>, SdkError> {
        validate_token_provider_configuration(&configuration)?;
        let base_url =
            Url::parse(&configuration.base_url).map_err(|error| SdkError::Configuration {
                reason: format!("base_url must be a valid URL: {error}"),
            })?;
        let transport =
            Transport::new_with_access_token(&configuration, access_token, http_client)?;

        Ok(Arc::new(Self {
            base_url,
            claim_sequence: AtomicU64::new(0),
            claim_poll_interval_ms: configuration.claim_poll_interval_ms,
            claim_poll_limit: configuration.claim_poll_limit,
            lifecycle_locks: Arc::new(NamespaceLifecycleLocks {
                namespaces: Mutex::new(HashMap::new()),
            }),
            transport,
        }))
    }

    #[uniffi::constructor]
    pub fn connect_browser_with_access_token(
        configuration: CyclopsTokenProviderConfiguration,
        access_token: String,
    ) -> Result<Arc<Self>, SdkError> {
        #[cfg(target_arch = "wasm32")]
        {
            return Self::connect_with_access_token(
                configuration,
                access_token,
                Arc::new(crate::transport::BrowserHttpClient),
            );
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (configuration, access_token);
            Err(SdkError::Configuration {
                reason: "connect_browser_with_access_token is only available for wasm32 targets"
                    .into(),
            })
        }
    }
}

impl CyclopsClient {
    pub(crate) fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub(crate) fn next_claim_sequence(&self) -> Result<u64, SdkError> {
        self.claim_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map(|previous| previous + 1)
            .map_err(|_| SdkError::Configuration {
                reason: "claim name sequence is exhausted".into(),
            })
    }

    pub(crate) fn claim_poll_interval_ms(&self) -> u64 {
        self.claim_poll_interval_ms
    }

    pub(crate) fn claim_poll_limit(&self) -> u32 {
        self.claim_poll_limit
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn namespace_lifecycle_guard(
        &self,
        namespace: &str,
    ) -> NamespaceLifecycleGuard {
        let locks = Arc::clone(&self.lifecycle_locks);
        let lock = {
            let mut namespaces = locks
                .namespaces
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            namespaces.retain(|_, existing| existing.strong_count() > 0);
            match namespaces.get(namespace).and_then(Weak::upgrade) {
                Some(existing) => existing,
                None => {
                    let created = Arc::new(AsyncMutex::new(()));
                    namespaces.insert(namespace.into(), Arc::downgrade(&created));
                    created
                }
            }
        };
        let guard = Arc::clone(&lock).lock_owned().await;

        NamespaceLifecycleGuard {
            guard: Some(guard),
            lock,
            locks,
            namespace: namespace.into(),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn namespace_lifecycle_guard(
        &self,
        _namespace: &str,
    ) -> NamespaceLifecycleGuard {
        NamespaceLifecycleGuard {}
    }

    pub async fn execute_authenticated(
        &self,
        request: HttpRequest,
    ) -> Result<HttpResponse, SdkError> {
        self.transport
            .execute_authenticated(request, AuthenticatedRequestClass::ControlPlane)
            .await
    }

    pub(crate) async fn execute_authenticated_service(
        &self,
        request: HttpRequest,
    ) -> Result<HttpResponse, SdkError> {
        self.transport
            .execute_authenticated(request, AuthenticatedRequestClass::ServiceProxy)
            .await
    }
}

fn validate_configuration(configuration: &CyclopsConfiguration) -> Result<(), SdkError> {
    validate_http_url("token_url", &configuration.token_url)?;
    validate_client_configuration(
        &configuration.base_url,
        configuration.pool_poll_interval_ms,
        configuration.pool_poll_limit,
        configuration.claim_poll_interval_ms,
        configuration.claim_poll_limit,
    )
}

fn validate_token_provider_configuration(
    configuration: &CyclopsTokenProviderConfiguration,
) -> Result<(), SdkError> {
    validate_client_configuration(
        &configuration.base_url,
        configuration.pool_poll_interval_ms,
        configuration.pool_poll_limit,
        configuration.claim_poll_interval_ms,
        configuration.claim_poll_limit,
    )
}

fn validate_client_configuration(
    base_url: &str,
    pool_poll_interval_ms: u64,
    pool_poll_limit: u32,
    claim_poll_interval_ms: u64,
    claim_poll_limit: u32,
) -> Result<(), SdkError> {
    validate_http_url("base_url", base_url)?;

    for (name, value) in [
        ("pool_poll_interval_ms", pool_poll_interval_ms),
        ("claim_poll_interval_ms", claim_poll_interval_ms),
    ] {
        if value == 0 {
            return Err(SdkError::Configuration {
                reason: format!("{name} must be greater than zero"),
            });
        }
    }

    for (name, value) in [
        ("pool_poll_limit", pool_poll_limit),
        ("claim_poll_limit", claim_poll_limit),
    ] {
        if value == 0 {
            return Err(SdkError::Configuration {
                reason: format!("{name} must be greater than zero"),
            });
        }
    }

    Ok(())
}

fn validate_http_url(field: &str, value: &str) -> Result<(), SdkError> {
    let url = Url::parse(value).map_err(|error| SdkError::Configuration {
        reason: format!("{field} must be a valid URL: {error}"),
    })?;

    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(SdkError::Configuration {
            reason: format!("{field} must be an absolute HTTP or HTTPS URL"),
        });
    }

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for NamespaceLifecycleGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
        let mut namespaces = self
            .locks
            .namespaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let remove = namespaces.get(&self.namespace).is_some_and(|existing| {
            Weak::ptr_eq(existing, &Arc::downgrade(&self.lock)) && existing.strong_count() == 1
        });
        if remove {
            namespaces.remove(&self.namespace);
        }
    }
}
