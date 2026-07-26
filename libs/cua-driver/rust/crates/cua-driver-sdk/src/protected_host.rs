//! Trusted host adapter for protected consent and persistent Stop UI.
//!
//! This callback surface is constructor-only host configuration. It must never
//! be implemented by model-facing code, exposed as an MCP tool, or bridged to
//! ordinary elicitation. The embedding application owns the protected
//! renderer and indicator; Cua binds its decisions to the exact request digest
//! and treats host failure as revocation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::consent::{
    ConsentAction, ConsentRequest, IndicatorLease, ProtectedConsentProvider, ProviderDecision,
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ProtectedConsentAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProtectedConsentDecision {
    pub action: ProtectedConsentAction,
    pub request_digest: String,
}

/// Content-bounded request delivered only to trusted host code.
///
/// `resource_json` contains the canonical typed resource identity needed to
/// render the decision. Hosts must not log it or forward it to a model.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProtectedConsentRequest {
    pub schema: String,
    pub nonce: String,
    pub generation: u64,
    pub daemon_instance: String,
    pub permission_mode: String,
    pub managed_policy_sha256: Option<String>,
    pub user_policy_sha256: Option<String>,
    pub operation: String,
    pub risk_class: String,
    pub public_session: String,
    pub transport_session: String,
    pub resource_json: String,
    pub human_summary: String,
    pub expires_unix_ms: u64,
    pub request_digest: String,
}

#[derive(Debug, Error, uniffi::Error)]
pub enum ProtectedConsentHostError {
    #[error("protected host failed: {reason}")]
    Failed { reason: String },
}

/// Callback contract implemented by a trusted embedding application.
///
/// The host must render approval and persistent Stop state outside the
/// model/tool channel. Returning from `wait_for_indicator_stop` means Stop was
/// activated; returning an error also fails closed and revokes the grant.
#[uniffi::export(with_foreign)]
#[async_trait]
pub trait ProtectedConsentHost: Send + Sync {
    async fn request_consent(
        &self,
        request: ProtectedConsentRequest,
    ) -> Result<ProtectedConsentDecision, ProtectedConsentHostError>;

    async fn activate_indicator(
        &self,
        request: ProtectedConsentRequest,
    ) -> Result<String, ProtectedConsentHostError>;

    async fn wait_for_indicator_stop(
        &self,
        indicator_id: String,
    ) -> Result<(), ProtectedConsentHostError>;

    async fn deactivate_indicator(
        &self,
        indicator_id: String,
    ) -> Result<(), ProtectedConsentHostError>;
}

pub(crate) struct SdkProtectedConsentProvider {
    host: Arc<dyn ProtectedConsentHost>,
    indicators: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl SdkProtectedConsentProvider {
    pub(crate) fn new(host: Arc<dyn ProtectedConsentHost>) -> Arc<Self> {
        Arc::new(Self {
            host,
            indicators: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

#[async_trait]
impl ProtectedConsentProvider for SdkProtectedConsentProvider {
    fn provider_id(&self) -> &'static str {
        "trusted_sdk_host_v1"
    }

    async fn request_consent(&self, request: &ConsentRequest) -> Result<ProviderDecision, String> {
        let decision = self
            .host
            .request_consent(export_request(request)?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(ProviderDecision {
            action: match decision.action {
                ProtectedConsentAction::Accept => ConsentAction::Accept,
                ProtectedConsentAction::Decline => ConsentAction::Decline,
                ProtectedConsentAction::Cancel => ConsentAction::Cancel,
            },
            request_digest: decision.request_digest,
        })
    }

    async fn activate_indicator(&self, request: &ConsentRequest) -> Result<IndicatorLease, String> {
        let indicator_id = self
            .host
            .activate_indicator(export_request(request)?)
            .await
            .map_err(|error| error.to_string())?;
        if indicator_id.trim().is_empty() {
            return Err("protected host returned an empty indicator id".to_owned());
        }

        let revoked = Arc::new(AtomicBool::new(false));
        {
            let mut indicators = self.indicators.lock().unwrap();
            if indicators.contains_key(&indicator_id) {
                return Err("protected host reused a live indicator id".to_owned());
            }
            indicators.insert(indicator_id.clone(), revoked.clone());
        }

        // Two independent signals close the loop:
        // - the host resolves `wait_for_indicator_stop` when the human presses
        //   Stop (or on host/channel failure);
        // - Cua flips the lease flag on session/runtime-side revocation.
        // Either path deactivates the host surface and revokes the grant.
        let host = self.host.clone();
        let indicators = self.indicators.clone();
        let indicator_for_task = indicator_id.clone();
        let revoked_for_task = revoked.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = host.wait_for_indicator_stop(indicator_for_task.clone()) => {
                    revoked_for_task.store(true, Ordering::Release);
                }
                _ = async {
                    while !revoked_for_task.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                } => {}
            }
            revoked_for_task.store(true, Ordering::Release);
            let was_live = indicators
                .lock()
                .unwrap()
                .remove(&indicator_for_task)
                .is_some();
            if was_live {
                let _ = host.deactivate_indicator(indicator_for_task).await;
            }
        });

        Ok(IndicatorLease::new(indicator_id, revoked))
    }

    async fn deactivate_indicator(&self, indicator_id: &str) {
        let revoked = self.indicators.lock().unwrap().remove(indicator_id);
        if let Some(revoked) = revoked {
            revoked.store(true, Ordering::Release);
            let _ = self
                .host
                .deactivate_indicator(indicator_id.to_owned())
                .await;
        }
    }
}

fn export_request(request: &ConsentRequest) -> Result<ProtectedConsentRequest, String> {
    Ok(ProtectedConsentRequest {
        schema: request.schema.to_owned(),
        nonce: request.nonce.clone(),
        generation: request.generation,
        daemon_instance: request.daemon_instance.clone(),
        permission_mode: request.permission_mode.as_str().to_owned(),
        managed_policy_sha256: request.managed_policy_sha256.clone(),
        user_policy_sha256: request.user_policy_sha256.clone(),
        operation: request.operation.clone(),
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
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Notify;

    struct FakeHost {
        action: ProtectedConsentAction,
        stop: Notify,
        deactivated: AtomicUsize,
        fail_indicator: bool,
    }

    #[async_trait]
    impl ProtectedConsentHost for FakeHost {
        async fn request_consent(
            &self,
            request: ProtectedConsentRequest,
        ) -> Result<ProtectedConsentDecision, ProtectedConsentHostError> {
            Ok(ProtectedConsentDecision {
                action: self.action,
                request_digest: request.request_digest,
            })
        }

        async fn activate_indicator(
            &self,
            request: ProtectedConsentRequest,
        ) -> Result<String, ProtectedConsentHostError> {
            if self.fail_indicator {
                return Err(ProtectedConsentHostError::Failed {
                    reason: "indicator unavailable".to_owned(),
                });
            }
            Ok(format!("indicator-{}", request.generation))
        }

        async fn wait_for_indicator_stop(
            &self,
            _indicator_id: String,
        ) -> Result<(), ProtectedConsentHostError> {
            self.stop.notified().await;
            Ok(())
        }

        async fn deactivate_indicator(
            &self,
            _indicator_id: String,
        ) -> Result<(), ProtectedConsentHostError> {
            self.deactivated.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    fn host(action: ProtectedConsentAction, fail_indicator: bool) -> Arc<FakeHost> {
        Arc::new(FakeHost {
            action,
            stop: Notify::new(),
            deactivated: AtomicUsize::new(0),
            fail_indicator,
        })
    }

    fn request(broker: &ApprovalBroker, mode: PermissionMode) -> ConsentRequest {
        broker.request(
            mode,
            "private_observation",
            RiskClass::R2,
            "public",
            "transport",
            serde_json::json!({"pid": 42, "window_id": 7}),
            "Observe window 7 from process 42",
        )
    }

    #[tokio::test]
    async fn exact_standard_decision_activates_a_live_indicator() {
        let host = host(ProtectedConsentAction::Accept, false);
        let provider = SdkProtectedConsentProvider::new(host.clone());
        let broker = ApprovalBroker::new(Some(provider));
        let grant = broker
            .approve(&request(&broker, PermissionMode::Standard))
            .await
            .unwrap();

        assert!(grant.is_live());
        host.stop.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            while grant.is_live() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(host.deactivated.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn decline_and_indicator_failure_fail_closed() {
        let declining = host(ProtectedConsentAction::Decline, false);
        let provider = SdkProtectedConsentProvider::new(declining);
        let broker = ApprovalBroker::new(Some(provider));
        assert!(matches!(
            broker
                .approve(&request(&broker, PermissionMode::Standard))
                .await,
            Err(cua_driver_core::consent::ConsentError::Declined)
        ));

        let broken = host(ProtectedConsentAction::Accept, true);
        let provider = SdkProtectedConsentProvider::new(broken);
        let broker = ApprovalBroker::new(Some(provider));
        assert!(matches!(
            broker
                .approve(&request(&broker, PermissionMode::Standard))
                .await,
            Err(cua_driver_core::consent::ConsentError::Indicator(_))
        ));
    }

    #[tokio::test]
    async fn cua_side_revocation_deactivates_the_host_indicator() {
        let host = host(ProtectedConsentAction::Accept, false);
        let provider = SdkProtectedConsentProvider::new(host.clone());
        let broker = ApprovalBroker::new(Some(provider));
        let grant = broker
            .activate_preapproved(&request(&broker, PermissionMode::Bounded))
            .await
            .unwrap();

        broker.revoke(&grant).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while host.deactivated.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!grant.is_live());
    }
}
