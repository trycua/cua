//! Trusted host adapter for residual authorization boundaries.
//!
//! The callback is immutable constructor configuration. It is never exposed as
//! a tool and Cua does not prescribe or render a consent interface.

use std::sync::Arc;

use async_trait::async_trait;
use cua_driver_core::consent::{
    ConsentAction, ConsentRequest, ProtectedConsentProvider, ProviderDecision,
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DriverAuthorizationAction {
    Allow,
    Deny,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DriverAuthorizationDecision {
    pub action: DriverAuthorizationAction,
    pub request_digest: String,
}

/// Content-bounded request delivered only to trusted host code.
///
/// `resource_json` contains an attested resource identity. Hosts must avoid
/// logging it or forwarding it to a model.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DriverAuthorizationRequest {
    pub schema: String,
    pub nonce: String,
    pub generation: u64,
    pub daemon_instance: String,
    pub permission_mode: String,
    pub managed_policy_sha256: Option<String>,
    pub user_policy_sha256: Option<String>,
    pub adapter_id: String,
    pub risk_class: String,
    pub public_session: String,
    pub transport_session: String,
    pub resource_json: String,
    pub human_summary: String,
    pub expires_unix_ms: u64,
    pub request_digest: String,
}

#[derive(Debug, Error, uniffi::Error)]
pub enum DriverAuthorizationHostError {
    #[error("authorization host failed: {reason}")]
    Failed { reason: String },
}

/// Optional callback implemented by a trusted embedding application.
///
/// Cua invokes this only for a residual boundary whose active permission mode
/// requires a host grant. Routine standard-mode automation and every
/// in-manifest bounded operation bypass it.
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait DriverAuthorizationHost: Send + Sync {
    async fn authorize(
        &self,
        request: DriverAuthorizationRequest,
    ) -> Result<DriverAuthorizationDecision, DriverAuthorizationHostError>;
}

pub(crate) struct SdkDriverAuthorizationHost {
    host: Arc<dyn DriverAuthorizationHost>,
}

impl SdkDriverAuthorizationHost {
    pub(crate) fn new(host: Arc<dyn DriverAuthorizationHost>) -> Arc<Self> {
        Arc::new(Self { host })
    }

    pub(crate) fn adapt(
        host: Option<Arc<dyn DriverAuthorizationHost>>,
    ) -> Option<Arc<dyn ProtectedConsentProvider>> {
        host.map(|host| Self::new(host) as Arc<dyn ProtectedConsentProvider>)
    }
}

#[async_trait]
impl ProtectedConsentProvider for SdkDriverAuthorizationHost {
    fn provider_id(&self) -> &'static str {
        "trusted_sdk_authorization_host_v1"
    }

    async fn request_consent(&self, request: &ConsentRequest) -> Result<ProviderDecision, String> {
        let decision = self
            .host
            .authorize(export_request(request)?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(ProviderDecision {
            action: match decision.action {
                DriverAuthorizationAction::Allow => ConsentAction::Accept,
                DriverAuthorizationAction::Deny => ConsentAction::Decline,
                DriverAuthorizationAction::Cancel => ConsentAction::Cancel,
            },
            request_digest: decision.request_digest,
        })
    }
}

fn export_request(request: &ConsentRequest) -> Result<DriverAuthorizationRequest, String> {
    Ok(DriverAuthorizationRequest {
        schema: "cua-driver-authorization-request-v1".to_owned(),
        nonce: request.nonce.clone(),
        generation: request.generation,
        daemon_instance: request.daemon_instance.clone(),
        permission_mode: request.permission_mode.as_str().to_owned(),
        managed_policy_sha256: request.managed_policy_sha256.clone(),
        user_policy_sha256: request.user_policy_sha256.clone(),
        adapter_id: request.operation.clone(),
        risk_class: request.risk_class.as_str().to_owned(),
        public_session: request.public_session.clone(),
        transport_session: request.transport_session.clone(),
        resource_json: serde_json::to_string(&request.resource)
            .map_err(|error| format!("could not serialize protected resource: {error}"))?,
        human_summary: request.human_summary.clone(),
        expires_unix_ms: u64::try_from(request.expires_unix_ms).unwrap_or(u64::MAX),
        request_digest: request.request_digest.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::authorization::{PermissionMode, RiskClass};
    use cua_driver_core::consent::ApprovalBroker;

    struct FakeHost {
        action: DriverAuthorizationAction,
    }

    #[async_trait]
    impl DriverAuthorizationHost for FakeHost {
        async fn authorize(
            &self,
            request: DriverAuthorizationRequest,
        ) -> Result<DriverAuthorizationDecision, DriverAuthorizationHostError> {
            Ok(DriverAuthorizationDecision {
                action: self.action,
                request_digest: request.request_digest,
            })
        }
    }

    fn request(broker: &ApprovalBroker) -> ConsentRequest {
        broker.request(
            PermissionMode::Standard,
            "existing_profile",
            RiskClass::R2,
            "public",
            "transport",
            serde_json::json!({"pid": 42, "window_id": 7}),
            "Attach to Chromium window 7",
        )
    }

    #[tokio::test]
    async fn host_allow_and_deny_map_to_exact_core_decisions() {
        let allowing = SdkDriverAuthorizationHost::new(Arc::new(FakeHost {
            action: DriverAuthorizationAction::Allow,
        }));
        let broker = ApprovalBroker::new(Some(allowing));
        let grant = broker.approve(&request(&broker)).await.unwrap();
        assert!(grant.is_live());
        broker.revoke(&grant).await;
        assert!(!grant.is_live());

        let denying = SdkDriverAuthorizationHost::new(Arc::new(FakeHost {
            action: DriverAuthorizationAction::Deny,
        }));
        let broker = ApprovalBroker::new(Some(denying));
        assert!(matches!(
            broker.approve(&request(&broker)).await,
            Err(cua_driver_core::consent::ConsentError::Declined)
        ));
    }
}
