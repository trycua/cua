//! Protected consent provider seam.
//!
//! Ordinary MCP elicitation, tool arguments, inherited stdio, files, and
//! environment variables never implement this trait. A provider is installed
//! in-process by a trusted host/platform adapter and returns its decision
//! directly to the daemon, bound to the exact request digest. Out-of-process
//! implementations must authenticate their private channel before adapting it
//! to this interface.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::authorization::{PermissionMode, RiskClass};

const DEFAULT_REQUEST_TTL: Duration = Duration::from_secs(2 * 60);
static CONFIGURED_PROVIDER_ID: OnceLock<&'static str> = OnceLock::new();

pub fn configured_provider_id() -> Option<&'static str> {
    CONFIGURED_PROVIDER_ID.get().copied()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDecision {
    pub action: ConsentAction,
    pub request_digest: String,
}

/// Exact, bounded request rendered by a protected provider. `resource` is
/// typed by the caller and never copied into telemetry or refusal strings.
#[derive(Debug, Clone, Serialize)]
pub struct ConsentRequest {
    pub schema: &'static str,
    pub nonce: String,
    pub generation: u64,
    pub daemon_instance: String,
    pub permission_mode: PermissionMode,
    pub managed_policy_sha256: Option<String>,
    pub user_policy_sha256: Option<String>,
    pub operation: String,
    pub risk_class: RiskClass,
    pub public_session: String,
    pub transport_session: String,
    pub resource: Value,
    pub human_summary: String,
    pub expires_unix_ms: u128,
    pub request_digest: String,
}

#[derive(Debug, Clone)]
pub struct IndicatorLease {
    pub id: String,
    revoked: Arc<AtomicBool>,
}

impl IndicatorLease {
    pub fn new(id: impl Into<String>, revoked: Arc<AtomicBool>) -> Self {
        Self {
            id: id.into(),
            revoked,
        }
    }

    pub fn revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }

    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }
}

/// Trusted integration contract. Implementors must keep approval and Stop UI
/// outside model/tool control and must not auto-accept protected requests.
/// The returned indicator lease is authoritative: the provider must bind its
/// visible state to the lease's revocation flag so session teardown remains
/// effective even when an asynchronous deactivate callback cannot run.
#[async_trait]
pub trait ProtectedConsentProvider: Send + Sync + 'static {
    fn provider_id(&self) -> &'static str;

    async fn request_consent(&self, request: &ConsentRequest) -> Result<ProviderDecision, String>;

    /// Activate persistent, non-optional visible state with a Stop control.
    /// Returning an error prevents the grant from becoming live.
    async fn activate_indicator(&self, request: &ConsentRequest) -> Result<IndicatorLease, String>;

    async fn deactivate_indicator(&self, indicator_id: &str);
}

#[derive(Debug, Clone)]
pub struct ProtectedGrant {
    pub id: Uuid,
    pub provider_id: &'static str,
    pub request_digest: String,
    pub indicator: IndicatorLease,
}

impl ProtectedGrant {
    pub fn is_live(&self) -> bool {
        !self.indicator.revoked()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsentError {
    #[error("no certified protected-consent provider is available")]
    Unavailable,
    #[error("protected consent was declined")]
    Declined,
    #[error("protected consent was canceled")]
    Canceled,
    #[error("protected consent provider failed: {0}")]
    Provider(String),
    #[error("protected consent response did not match the exact request digest")]
    DigestMismatch,
    #[error("protected consent expired before activation")]
    Expired,
    #[error("persistent consent indicator could not be activated: {0}")]
    Indicator(String),
}

#[derive(Clone)]
pub struct ApprovalBroker {
    daemon_instance: Arc<str>,
    next_generation: Arc<AtomicU64>,
    provider: Option<Arc<dyn ProtectedConsentProvider>>,
}

impl ApprovalBroker {
    pub fn unavailable() -> Self {
        Self::new(None)
    }

    pub fn new(provider: Option<Arc<dyn ProtectedConsentProvider>>) -> Self {
        if let Some(provider) = provider.as_ref() {
            let _ = CONFIGURED_PROVIDER_ID.set(provider.provider_id());
        }
        Self {
            daemon_instance: Arc::from(Uuid::new_v4().to_string()),
            next_generation: Arc::new(AtomicU64::new(1)),
            provider,
        }
    }

    pub fn provider_id(&self) -> Option<&'static str> {
        self.provider
            .as_ref()
            .map(|provider| provider.provider_id())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn request(
        &self,
        permission_mode: PermissionMode,
        operation: impl Into<String>,
        risk_class: RiskClass,
        public_session: impl Into<String>,
        transport_session: impl Into<String>,
        resource: Value,
        human_summary: impl Into<String>,
    ) -> ConsentRequest {
        let expires_unix_ms = now_unix_ms() + DEFAULT_REQUEST_TTL.as_millis();
        let mut request = ConsentRequest {
            schema: "cua-protected-consent-request-v1",
            nonce: Uuid::new_v4().to_string(),
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
            daemon_instance: self.daemon_instance.to_string(),
            permission_mode,
            managed_policy_sha256: crate::policy::managed_policy_sha256().ok().flatten(),
            user_policy_sha256: crate::policy::user_policy_sha256().ok().flatten(),
            operation: operation.into(),
            risk_class,
            public_session: public_session.into(),
            transport_session: transport_session.into(),
            resource,
            human_summary: human_summary.into(),
            expires_unix_ms,
            request_digest: String::new(),
        };
        request.request_digest = digest_request(&request);
        request
    }

    pub async fn approve(&self, request: &ConsentRequest) -> Result<ProtectedGrant, ConsentError> {
        let provider = self.provider.as_ref().ok_or(ConsentError::Unavailable)?;
        if request.expires_unix_ms < now_unix_ms() {
            return Err(ConsentError::Expired);
        }
        if request.request_digest != digest_request(request) {
            return Err(ConsentError::DigestMismatch);
        }
        let decision = tokio::time::timeout(remaining(request)?, provider.request_consent(request))
            .await
            .map_err(|_| ConsentError::Provider("request timed out".to_owned()))?
            .map_err(ConsentError::Provider)?;
        if decision.request_digest != request.request_digest {
            return Err(ConsentError::DigestMismatch);
        }
        match decision.action {
            ConsentAction::Accept => {}
            ConsentAction::Decline => return Err(ConsentError::Declined),
            ConsentAction::Cancel => return Err(ConsentError::Canceled),
        }
        if request.expires_unix_ms < now_unix_ms() {
            return Err(ConsentError::Expired);
        }
        let indicator =
            tokio::time::timeout(remaining(request)?, provider.activate_indicator(request))
                .await
                .map_err(|_| ConsentError::Indicator("activation timed out".to_owned()))?
                .map_err(ConsentError::Indicator)?;
        if request.expires_unix_ms < now_unix_ms() {
            indicator.revoke();
            provider.deactivate_indicator(&indicator.id).await;
            return Err(ConsentError::Expired);
        }
        Ok(ProtectedGrant {
            id: Uuid::new_v4(),
            provider_id: provider.provider_id(),
            request_digest: request.request_digest.clone(),
            indicator,
        })
    }

    /// Activate the mandatory indicator for an operation already covered by a
    /// launch-approved autonomous manifest. This never asks or accepts an
    /// in-band decision; the manifest remains the authorization and the
    /// provider supplies only persistent visible Stop state.
    pub async fn activate_preapproved(
        &self,
        request: &ConsentRequest,
    ) -> Result<ProtectedGrant, ConsentError> {
        let provider = self.provider.as_ref().ok_or(ConsentError::Unavailable)?;
        if request.expires_unix_ms < now_unix_ms() {
            return Err(ConsentError::Expired);
        }
        if request.request_digest != digest_request(request) {
            return Err(ConsentError::DigestMismatch);
        }
        let indicator =
            tokio::time::timeout(remaining(request)?, provider.activate_indicator(request))
                .await
                .map_err(|_| ConsentError::Indicator("activation timed out".to_owned()))?
                .map_err(ConsentError::Indicator)?;
        if request.expires_unix_ms < now_unix_ms() {
            indicator.revoke();
            provider.deactivate_indicator(&indicator.id).await;
            return Err(ConsentError::Expired);
        }
        Ok(ProtectedGrant {
            id: Uuid::new_v4(),
            provider_id: provider.provider_id(),
            request_digest: request.request_digest.clone(),
            indicator,
        })
    }

    pub async fn revoke(&self, grant: &ProtectedGrant) {
        grant.indicator.revoke();
        if let Some(provider) = &self.provider {
            if provider.provider_id() == grant.provider_id {
                provider.deactivate_indicator(&grant.indicator.id).await;
            }
        }
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn remaining(request: &ConsentRequest) -> Result<Duration, ConsentError> {
    let remaining_ms = request
        .expires_unix_ms
        .checked_sub(now_unix_ms())
        .ok_or(ConsentError::Expired)?;
    let remaining_ms = u64::try_from(remaining_ms).unwrap_or(u64::MAX);
    if remaining_ms == 0 {
        return Err(ConsentError::Expired);
    }
    Ok(Duration::from_millis(remaining_ms))
}

fn digest_request(request: &ConsentRequest) -> String {
    let canonical = serde_json::json!({
        "schema": request.schema,
        "nonce": request.nonce,
        "generation": request.generation,
        "daemon_instance": request.daemon_instance,
        "permission_mode": request.permission_mode,
        "managed_policy_sha256": request.managed_policy_sha256,
        "user_policy_sha256": request.user_policy_sha256,
        "operation": request.operation,
        "risk_class": request.risk_class,
        "public_session": request.public_session,
        "transport_session": request.transport_session,
        "resource": request.resource,
        "human_summary": request.human_summary,
        "expires_unix_ms": request.expires_unix_ms,
    });
    let bytes = serde_json::to_vec(&canonical).expect("serialize protected consent request");
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeProvider {
        action: ConsentAction,
        replace_digest: bool,
        indicator_error: bool,
        indicator_delay: Duration,
        revoked: Arc<AtomicBool>,
        deactivated: Mutex<Vec<String>>,
        requests: AtomicU64,
    }

    #[async_trait]
    impl ProtectedConsentProvider for FakeProvider {
        fn provider_id(&self) -> &'static str {
            "test.protected-provider"
        }

        async fn request_consent(
            &self,
            request: &ConsentRequest,
        ) -> Result<ProviderDecision, String> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderDecision {
                action: self.action,
                request_digest: if self.replace_digest {
                    "forged".to_owned()
                } else {
                    request.request_digest.clone()
                },
            })
        }

        async fn activate_indicator(
            &self,
            _request: &ConsentRequest,
        ) -> Result<IndicatorLease, String> {
            tokio::time::sleep(self.indicator_delay).await;
            if self.indicator_error {
                return Err("indicator unavailable".to_owned());
            }
            Ok(IndicatorLease::new("indicator-1", self.revoked.clone()))
        }

        async fn deactivate_indicator(&self, indicator_id: &str) {
            self.deactivated
                .lock()
                .unwrap()
                .push(indicator_id.to_owned());
        }
    }

    fn provider(action: ConsentAction) -> Arc<FakeProvider> {
        Arc::new(FakeProvider {
            action,
            replace_digest: false,
            indicator_error: false,
            indicator_delay: Duration::ZERO,
            revoked: Arc::new(AtomicBool::new(false)),
            deactivated: Mutex::new(Vec::new()),
            requests: AtomicU64::new(0),
        })
    }

    fn request(broker: &ApprovalBroker) -> ConsentRequest {
        broker.request(
            PermissionMode::Standard,
            "browser_prepare.existing_profile",
            RiskClass::R2,
            "public-session",
            "transport-session",
            serde_json::json!({"pid": 42, "window_id": 7}),
            "Attach to Chromium window 7",
        )
    }

    #[tokio::test]
    async fn exact_accept_activates_a_revocable_indicator() {
        let provider = provider(ConsentAction::Accept);
        let broker = ApprovalBroker::new(Some(provider.clone()));
        let grant = broker.approve(&request(&broker)).await.unwrap();
        assert!(grant.is_live());
        broker.revoke(&grant).await;
        assert!(!grant.is_live());
        assert_eq!(
            provider.deactivated.lock().unwrap().as_slice(),
            ["indicator-1"]
        );
    }

    #[tokio::test]
    async fn missing_provider_fails_closed() {
        let broker = ApprovalBroker::unavailable();
        assert_eq!(
            broker.approve(&request(&broker)).await.unwrap_err(),
            ConsentError::Unavailable
        );
    }

    #[tokio::test]
    async fn forged_digest_never_activates_a_grant() {
        let fake = FakeProvider {
            action: ConsentAction::Accept,
            replace_digest: true,
            indicator_error: false,
            indicator_delay: Duration::ZERO,
            revoked: Arc::new(AtomicBool::new(false)),
            deactivated: Mutex::new(Vec::new()),
            requests: AtomicU64::new(0),
        };
        let broker = ApprovalBroker::new(Some(Arc::new(fake)));
        assert_eq!(
            broker.approve(&request(&broker)).await.unwrap_err(),
            ConsentError::DigestMismatch
        );
    }

    #[tokio::test]
    async fn indicator_failure_prevents_grant_activation() {
        let provider = Arc::new(FakeProvider {
            action: ConsentAction::Accept,
            replace_digest: false,
            indicator_error: true,
            indicator_delay: Duration::ZERO,
            revoked: Arc::new(AtomicBool::new(false)),
            deactivated: Mutex::new(Vec::new()),
            requests: AtomicU64::new(0),
        });
        let broker = ApprovalBroker::new(Some(provider));
        assert!(matches!(
            broker.approve(&request(&broker)).await,
            Err(ConsentError::Indicator(_))
        ));
    }

    #[tokio::test]
    async fn launch_preapproved_autonomy_activates_indicator_without_a_second_prompt() {
        let provider = provider(ConsentAction::Decline);
        let broker = ApprovalBroker::new(Some(provider.clone()));
        let grant = broker
            .activate_preapproved(&request(&broker))
            .await
            .unwrap();
        assert!(grant.is_live());
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn provider_cannot_activate_an_indicator_after_request_expiry() {
        let provider = Arc::new(FakeProvider {
            action: ConsentAction::Accept,
            replace_digest: false,
            indicator_error: false,
            indicator_delay: Duration::from_millis(50),
            revoked: Arc::new(AtomicBool::new(false)),
            deactivated: Mutex::new(Vec::new()),
            requests: AtomicU64::new(0),
        });
        let broker = ApprovalBroker::new(Some(provider));
        let mut expiring = request(&broker);
        expiring.expires_unix_ms = now_unix_ms() + 15;
        expiring.request_digest = digest_request(&expiring);

        assert!(matches!(
            broker.approve(&expiring).await,
            Err(ConsentError::Indicator(message)) if message.contains("timed out")
        ));
    }
}
