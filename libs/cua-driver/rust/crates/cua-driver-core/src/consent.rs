//! Trusted host authorization seam.
//!
//! Ordinary MCP elicitation, tool arguments, inherited stdio, files, and
//! environment variables never implement this trait. A provider is installed
//! in-process by a trusted host/platform adapter and returns its decision
//! directly to the daemon, bound to the exact request digest. Out-of-process
//! implementations must authenticate their private channel before adapting it
//! to this interface.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::authorization::{ModeBehavior, PermissionMode, RiskClass, ENFORCEMENT_ADAPTERS};

const DEFAULT_REQUEST_TTL: Duration = Duration::from_secs(2 * 60);

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

/// Trusted integration contract. The embedding host decides how to obtain
/// authorization. Cua never requires the host to render a particular UI.
#[async_trait]
pub trait ProtectedConsentProvider: Send + Sync + 'static {
    fn provider_id(&self) -> &'static str;

    async fn request_consent(&self, request: &ConsentRequest) -> Result<ProviderDecision, String>;
}

#[derive(Debug, Clone)]
pub struct ProtectedGrant {
    pub id: Uuid,
    pub provider_id: &'static str,
    pub request_digest: String,
    revoked: Arc<AtomicBool>,
}

impl ProtectedGrant {
    pub fn is_live(&self) -> bool {
        !self.revoked.load(Ordering::Acquire)
    }

    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsentError {
    #[error("no request-bound confirmation provider is available")]
    Unavailable,
    #[error("confirmation was declined")]
    Declined,
    #[error("confirmation was canceled")]
    Canceled,
    #[error("confirmation provider failed: {0}")]
    Provider(String),
    #[error("confirmation response did not match the exact request digest")]
    DigestMismatch,
    #[error("confirmation expired before activation")]
    Expired,
    #[error("protected resource is outside the bounded session policy: {0}")]
    BoundedResource(String),
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
        self.request_bound(
            permission_mode,
            operation,
            risk_class,
            public_session,
            transport_session,
            resource,
            human_summary,
            crate::policy::managed_policy_sha256().ok().flatten(),
            crate::policy::user_policy_sha256().ok().flatten(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn request_bound(
        &self,
        permission_mode: PermissionMode,
        operation: impl Into<String>,
        risk_class: RiskClass,
        public_session: impl Into<String>,
        transport_session: impl Into<String>,
        resource: Value,
        human_summary: impl Into<String>,
        managed_policy_sha256: Option<String>,
        user_policy_sha256: Option<String>,
    ) -> ConsentRequest {
        let expires_unix_ms = now_unix_ms() + DEFAULT_REQUEST_TTL.as_millis();
        let mut request = ConsentRequest {
            schema: "cua-protected-consent-request-v1",
            nonce: Uuid::new_v4().to_string(),
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
            daemon_instance: self.daemon_instance.to_string(),
            permission_mode,
            managed_policy_sha256,
            user_policy_sha256,
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
        Ok(ProtectedGrant {
            id: Uuid::new_v4(),
            provider_id: provider.provider_id(),
            request_digest: request.request_digest.clone(),
            revoked: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn revoke(&self, grant: &ProtectedGrant) {
        grant.revoke();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ResourceGrantKey {
    runtime_scope: String,
    public_session: String,
    transport_session: String,
    adapter_id: String,
    resource_digest: String,
}

#[derive(Debug, Clone)]
pub struct ResourceGrant {
    pub adapter_id: String,
    pub resource_digest: String,
    pub protected: ProtectedGrant,
    mode: PermissionMode,
    managed_policy_sha256: Option<String>,
    user_policy_sha256: Option<String>,
    created_at: Instant,
    last_used_at: Instant,
    idle_ttl: Duration,
    absolute_ttl: Duration,
}

impl ResourceGrant {
    pub fn is_live(&self) -> bool {
        self.protected.is_live()
    }

    fn expired(&self, now: Instant) -> bool {
        !self.is_live()
            || now.duration_since(self.last_used_at) >= self.idle_ttl
            || now.duration_since(self.created_at) >= self.absolute_ttl
    }
}

/// Per-runtime store and admission coordinator for every protected resource
/// adapter beyond existing-profile attachment.
///
/// Only canonical resource digests become map keys. Raw paths, text, page
/// content, selectors, and typed input are never retained by this store.
pub struct ProtectedResourceGrants {
    broker: Arc<ApprovalBroker>,
    grants: Mutex<HashMap<ResourceGrantKey, ResourceGrant>>,
    admission: tokio::sync::Mutex<()>,
}

/// Runtime-owned proof that a process was created and remains managed by Cua.
///
/// Tool arguments can select a PID but can never populate this store. Every
/// record includes an OS-attested fingerprint so PID reuse cannot inherit
/// ownership. The caller must re-attest the process at each use.
#[derive(Default)]
pub struct ProtectedResourceOwnershipStore {
    processes_by_session: Mutex<HashMap<String, HashMap<i64, crate::browser::ProcessFingerprint>>>,
}

impl ProtectedResourceOwnershipStore {
    pub fn mark_driver_owned_process(
        &self,
        session: &str,
        fingerprint: crate::browser::ProcessFingerprint,
    ) {
        self.processes_by_session
            .lock()
            .unwrap()
            .entry(session.to_owned())
            .or_default()
            .insert(fingerprint.pid, fingerprint);
    }

    /// Transitional helper for tests and adapters that have not yet learned a
    /// complete fingerprint. A PID-only record never validates against a
    /// platform fingerprint that contains a start time.
    pub fn mark_driver_owned_pid(&self, session: &str, pid: i64) {
        self.mark_driver_owned_process(
            session,
            crate::browser::ProcessFingerprint {
                pid,
                start_time: None,
                executable: None,
            },
        );
    }

    pub fn is_driver_owned_pid(&self, session: &str, pid: i64) -> bool {
        self.processes_by_session
            .lock()
            .unwrap()
            .get(session)
            .is_some_and(|processes| processes.contains_key(&pid))
    }

    /// Re-prove ownership against a fresh platform fingerprint. Identity drift
    /// is terminal for the record and removes it immediately.
    pub fn reprove_driver_owned_process(
        &self,
        session: &str,
        current: &crate::browser::ProcessFingerprint,
    ) -> bool {
        let mut sessions = self.processes_by_session.lock().unwrap();
        let Some(processes) = sessions.get_mut(session) else {
            return false;
        };
        let matches = processes
            .get(&current.pid)
            .is_some_and(|recorded| recorded.matches(current));
        if !matches {
            processes.remove(&current.pid);
        }
        if processes.is_empty() {
            sessions.remove(session);
        }
        matches
    }

    pub fn remove_session(&self, session: &str) {
        self.processes_by_session.lock().unwrap().remove(session);
    }
}

impl ProtectedResourceGrants {
    pub fn new(broker: Arc<ApprovalBroker>) -> Self {
        Self {
            broker,
            grants: Mutex::new(HashMap::new()),
            admission: tokio::sync::Mutex::new(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn authorize(
        &self,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        adapter_id: &str,
        risk_class: RiskClass,
        lifecycle_session: Option<&str>,
        resource: Value,
        human_summary: &str,
        idle_ttl: Duration,
        absolute_ttl: Duration,
    ) -> Result<Option<ResourceGrant>, ConsentError> {
        let descriptor = ENFORCEMENT_ADAPTERS
            .iter()
            .find(|descriptor| descriptor.id == adapter_id)
            .ok_or_else(|| {
                ConsentError::Provider(format!(
                    "protected resource adapter '{adapter_id}' is not registered"
                ))
            })?;
        match descriptor.behavior_by_mode.for_mode(context.mode()) {
            ModeBehavior::Allow => return Ok(None),
            ModeBehavior::Deny => {
                return Err(ConsentError::Provider(format!(
                    "adapter '{adapter_id}' is denied in {} mode",
                    context.mode().as_str()
                )))
            }
            ModeBehavior::Manifest => {
                let manifest = context.bounded_manifest().ok_or_else(|| {
                    ConsentError::BoundedResource(
                        "bounded mode has no approved session manifest".to_owned(),
                    )
                })?;
                manifest
                    .authorize_protected_resource(adapter_id, &resource)
                    .map_err(ConsentError::BoundedResource)?;
                return Ok(None);
            }
            ModeBehavior::RequireGrant => {}
        }
        let resource_digest = digest_resource(adapter_id, &resource);
        let key = resource_grant_key(context, lifecycle_session, adapter_id, &resource_digest);
        if let Some(grant) = self.lookup(&key, context) {
            return Ok(Some(grant));
        }

        // Collapse concurrent requests for the same or different protected
        // resources into a single provider turn. Re-check after waiting so a
        // peer that just approved the same exact scope supplies the grant.
        let _admission = self.admission.lock().await;
        if let Some(grant) = self.lookup(&key, context) {
            return Ok(Some(grant));
        }

        let public_session = context
            .public_session()
            .or(lifecycle_session)
            .unwrap_or("compatibility");
        let transport_session = context.transport_session().unwrap_or(public_session);
        let request = self.broker.request_bound(
            context.mode(),
            adapter_id,
            risk_class,
            public_session,
            transport_session,
            resource,
            human_summary,
            context.managed_policy_sha256().map(str::to_owned),
            context.user_policy_sha256().map(str::to_owned),
        );
        let protected = match context.mode() {
            PermissionMode::Standard => self.broker.approve(&request).await?,
            PermissionMode::Bounded | PermissionMode::Unrestricted => {
                unreachable!("handled before protected admission")
            }
        };
        let now = Instant::now();
        let grant = ResourceGrant {
            adapter_id: adapter_id.to_owned(),
            resource_digest,
            protected,
            mode: context.mode(),
            managed_policy_sha256: context.managed_policy_sha256().map(str::to_owned),
            user_policy_sha256: context.user_policy_sha256().map(str::to_owned),
            created_at: now,
            last_used_at: now,
            idle_ttl,
            absolute_ttl,
        };
        self.grants.lock().unwrap().insert(key, grant.clone());
        Ok(Some(grant))
    }

    fn lookup(
        &self,
        key: &ResourceGrantKey,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
    ) -> Option<ResourceGrant> {
        let now = Instant::now();
        let mut grants = self.grants.lock().unwrap();
        let grant = grants.get_mut(key)?;
        if grant.expired(now)
            || grant.mode != context.mode()
            || grant.managed_policy_sha256.as_deref() != context.managed_policy_sha256()
            || grant.user_policy_sha256.as_deref() != context.user_policy_sha256()
        {
            if let Some(expired) = grants.remove(key) {
                expired.protected.revoke();
            }
            return None;
        }
        grant.last_used_at = now;
        Some(grant.clone())
    }

    pub fn revoke_session(&self, session: &str) -> Vec<ProtectedGrant> {
        let mut removed = Vec::new();
        self.grants.lock().unwrap().retain(|key, grant| {
            let keep = key.public_session != session && key.transport_session != session;
            if !keep {
                grant.protected.revoke();
                removed.push(grant.protected.clone());
            }
            keep
        });
        removed
    }

    pub fn revoke_resource(
        &self,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        lifecycle_session: Option<&str>,
        adapter_id: &str,
        resource: &Value,
    ) -> Option<ProtectedGrant> {
        let digest = digest_resource(adapter_id, resource);
        let key = resource_grant_key(context, lifecycle_session, adapter_id, &digest);
        self.grants.lock().unwrap().remove(&key).map(|grant| {
            grant.protected.revoke();
            grant.protected
        })
    }

    pub fn revoke_all(&self) -> Vec<ProtectedGrant> {
        let mut grants = self.grants.lock().unwrap();
        let removed = grants
            .drain()
            .map(|(_, grant)| {
                grant.protected.revoke();
                grant.protected
            })
            .collect();
        removed
    }

    pub fn broker(&self) -> &Arc<ApprovalBroker> {
        &self.broker
    }
}

impl Drop for ProtectedResourceGrants {
    fn drop(&mut self) {
        for (_, grant) in self.grants.get_mut().unwrap().drain() {
            grant.protected.revoke();
        }
    }
}

fn resource_grant_key(
    context: &crate::session_authorization::EffectiveAuthorizationContext,
    lifecycle_session: Option<&str>,
    adapter_id: &str,
    resource_digest: &str,
) -> ResourceGrantKey {
    let public_session = context
        .public_session()
        .or(lifecycle_session)
        .unwrap_or("compatibility");
    ResourceGrantKey {
        runtime_scope: context.runtime_scope_key(),
        public_session: context.runtime_session_key(public_session),
        transport_session: context
            .transport_session()
            .unwrap_or(public_session)
            .to_owned(),
        adapter_id: adapter_id.to_owned(),
        resource_digest: resource_digest.to_owned(),
    }
}

fn digest_resource(adapter_id: &str, resource: &Value) -> String {
    let canonical = serde_json::json!({
        "adapter_id": adapter_id,
        "resource": resource,
    });
    let bytes = serde_json::to_vec(&canonical).expect("serialize protected resource");
    format!("{:x}", Sha256::digest(bytes))
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

    struct FakeProvider {
        action: ConsentAction,
        replace_digest: bool,
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
    }

    fn provider(action: ConsentAction) -> Arc<FakeProvider> {
        Arc::new(FakeProvider {
            action,
            replace_digest: false,
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

    fn authorization_context(
        mode: PermissionMode,
    ) -> Arc<crate::session_authorization::EffectiveAuthorizationContext> {
        let ceiling = crate::session_authorization::SessionModeCeiling::for_trusted_sessions(
            [mode],
            mode == PermissionMode::Unrestricted,
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap();
        crate::session_authorization::SessionAuthorizationRegistry::with_ceiling(ceiling)
            .compatibility_context(mode, None)
            .unwrap()
    }

    #[test]
    fn process_ownership_requires_the_same_attested_process_instance() {
        let store = ProtectedResourceOwnershipStore::default();
        let recorded = crate::browser::ProcessFingerprint {
            pid: 42,
            start_time: Some(100),
            executable: Some("/Applications/Fixture.app/Contents/MacOS/Fixture".to_owned()),
        };
        store.mark_driver_owned_process("runtime:session", recorded.clone());
        assert!(store.reprove_driver_owned_process("runtime:session", &recorded));

        let reused = crate::browser::ProcessFingerprint {
            start_time: Some(101),
            ..recorded.clone()
        };
        assert!(!store.reprove_driver_owned_process("runtime:session", &reused));
        assert!(
            !store.is_driver_owned_pid("runtime:session", 42),
            "identity drift must remove launch provenance"
        );
    }

    #[test]
    fn provider_identity_is_runtime_scoped() {
        let with_provider = crate::tool::ToolRegistry::new_with_protected_consent_provider(Some(
            provider(ConsentAction::Accept),
        ));
        let without_provider = crate::tool::ToolRegistry::new();

        assert_eq!(
            with_provider.authorization_status_json()["authorization_host"],
            "test.protected-provider"
        );
        assert_eq!(
            without_provider.authorization_status_json()["authorization_host"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn standard_routine_resources_never_request_host_authorization() {
        let provider = provider(ConsentAction::Accept);
        let broker = Arc::new(ApprovalBroker::new(Some(provider.clone())));
        let grants = ProtectedResourceGrants::new(broker);
        let context = authorization_context(PermissionMode::Standard);

        assert!(grants
            .authorize(
                &context,
                "private_observation",
                RiskClass::R2,
                Some("public-a"),
                serde_json::json!({"pid": 42, "window_id": 7}),
                "Observe app window 7",
                Duration::from_secs(30),
                Duration::from_secs(60),
            )
            .await
            .unwrap()
            .is_none());
        assert!(grants
            .authorize(
                &context,
                "private_observation",
                RiskClass::R2,
                Some("public-a"),
                serde_json::json!({"pid": 42, "window_id": 7}),
                "Observe app window 7",
                Duration::from_secs(30),
                Duration::from_secs(60),
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn standard_and_unrestricted_routine_actions_skip_the_broker() {
        let provider = provider(ConsentAction::Accept);
        let broker = Arc::new(ApprovalBroker::new(Some(provider.clone())));
        let grants = ProtectedResourceGrants::new(broker);
        let standard = authorization_context(PermissionMode::Standard);
        assert!(grants
            .authorize(
                &standard,
                "desktop_input",
                RiskClass::R1,
                Some("public-a"),
                serde_json::json!({"pid": 42, "window_id": 7}),
                "Control app window 7",
                Duration::from_secs(30),
                Duration::from_secs(60),
            )
            .await
            .unwrap()
            .is_none());
        let revoked = grants.revoke_session(&standard.runtime_session_key("public-a"));
        assert!(revoked.is_empty());

        let unrestricted = authorization_context(PermissionMode::Unrestricted);
        assert!(grants
            .authorize(
                &unrestricted,
                "desktop_input",
                RiskClass::R1,
                Some("public-b"),
                serde_json::json!({"pid": 9, "window_id": 2}),
                "Control app window 2",
                Duration::from_secs(30),
                Duration::from_secs(60),
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_accept_creates_a_revocable_grant() {
        let provider = provider(ConsentAction::Accept);
        let broker = ApprovalBroker::new(Some(provider.clone()));
        let grant = broker.approve(&request(&broker)).await.unwrap();
        assert!(grant.is_live());
        broker.revoke(&grant).await;
        assert!(!grant.is_live());
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
            requests: AtomicU64::new(0),
        };
        let broker = ApprovalBroker::new(Some(Arc::new(fake)));
        assert_eq!(
            broker.approve(&request(&broker)).await.unwrap_err(),
            ConsentError::DigestMismatch
        );
    }
}
