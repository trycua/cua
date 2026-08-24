//! Trusted-host credential binding and release primitives.
//!
//! This module is intentionally absent from the tool registry and generated
//! contracts. It establishes the provider-neutral, target-bound foundation
//! used by later credential discovery and delivery slices without making a
//! secret-bearing operation invocable through MCP, CLI, ABI, or UniFFI.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::browser::store::{parse_ref, SecureFieldKind};
use crate::browser::types::ProcessFingerprint;
use crate::session::{
    is_runtime_scope_suspended, is_session_ended, register_scoped_runtime_suspend_hook,
    register_scoped_session_end_hook, RuntimeSuspendHookRegistration, SessionEndHookRegistration,
};

pub const MAX_SECRET_BYTES: usize = 4 * 1024;
pub const MAX_BOOTSTRAP_SECRET_BYTES: usize = 64 * 1024;
pub const DEFAULT_CREDENTIAL_HANDLE_TTL: Duration = Duration::from_secs(60);
const BOOTSTRAP_RECORD_MAGIC: &[u8; 4] = b"CBS1";
const BOOTSTRAP_RECORD_HEADER_BYTES: usize = BOOTSTRAP_RECORD_MAGIC.len() + 8;
const MAX_ACTIVE_HANDLES: usize = 4_096;
const MAX_ACTIVE_HANDLES_PER_SESSION: usize = 256;
const MAX_HANDLE_TOMBSTONES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialField {
    Password,
}

impl CredentialField {
    fn digest_tag(self) -> &'static [u8] {
        match self {
            Self::Password => b"password",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialProviderId(Arc<str>);

impl CredentialProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self, CredentialConfigurationError> {
        let value = value.into();
        validate_identifier(&value, 96)
            .map_err(|_| CredentialConfigurationError::InvalidProvider)?;
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialProviderId([private])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialProviderClass {
    Fake,
    ServiceAccountVault,
    InteractiveDesktop,
}

impl CredentialProviderClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fake => "fake",
            Self::ServiceAccountVault => "service_account_vault",
            Self::InteractiveDesktop => "interactive_desktop",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SafeCredentialLabel(Arc<str>);

impl SafeCredentialLabel {
    pub fn new(value: impl Into<String>) -> Result<Self, CredentialConfigurationError> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
            return Err(CredentialConfigurationError::InvalidSafeLabel);
        }
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SafeCredentialLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SafeCredentialLabel")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone)]
pub struct PrivateCredentialFieldLocator(Arc<Zeroizing<Vec<u8>>>);

impl PrivateCredentialFieldLocator {
    pub fn new(value: impl Into<String>) -> Result<Self, CredentialConfigurationError> {
        let value = value.into();
        if value.is_empty() || value.len() > 4_096 || value.chars().any(char::is_control) {
            return Err(CredentialConfigurationError::InvalidPrivateLocator);
        }
        Ok(Self(Arc::new(Zeroizing::new(value.into_bytes()))))
    }

    pub fn as_str(&self) -> &str {
        // Construction accepts a Rust String, so UTF-8 remains invariant.
        std::str::from_utf8(self.0.as_slice()).expect("private locator UTF-8 invariant")
    }
}

impl fmt::Debug for PrivateCredentialFieldLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrivateCredentialFieldLocator([private])")
    }
}

#[derive(Clone)]
pub struct PrivateCredentialField {
    pub field: CredentialField,
    pub locator: PrivateCredentialFieldLocator,
}

impl fmt::Debug for PrivateCredentialField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivateCredentialField")
            .field("field", &self.field)
            .field("locator", &"[private]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretReleaseAuthorization(Arc<str>);

impl SecretReleaseAuthorization {
    pub fn trusted_scope(value: impl Into<String>) -> Result<Self, CredentialConfigurationError> {
        let value = value.into();
        validate_identifier(&value, 128)
            .map_err(|_| CredentialConfigurationError::InvalidAuthorizationScope)?;
        Ok(Self(value.into()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretReleaseAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretReleaseAuthorization([private])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExactBrowserOrigin(Arc<str>);

impl ExactBrowserOrigin {
    pub fn parse(value: &str) -> Result<Self, CredentialConfigurationError> {
        let parsed = url::Url::parse(value)
            .map_err(|_| CredentialConfigurationError::InvalidTargetConstraint)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(CredentialConfigurationError::InvalidTargetConstraint);
        }
        let origin = parsed.origin().ascii_serialization();
        if origin == "null" {
            return Err(CredentialConfigurationError::InvalidTargetConstraint);
        }
        Ok(Self(origin.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSecretTargetConstraint {
    pub origin: ExactBrowserOrigin,
    pub secure_field: SecureFieldKind,
}

impl BrowserSecretTargetConstraint {
    pub fn exact_password_origin(origin: &str) -> Result<Self, CredentialConfigurationError> {
        Ok(Self {
            origin: ExactBrowserOrigin::parse(origin)?,
            secure_field: SecureFieldKind::Password,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretTargetConstraint {
    Browser(BrowserSecretTargetConstraint),
}

#[derive(Clone)]
pub struct CredentialBindingSpec {
    /// Runtime-private lifecycle session key, never the caller's bare label.
    pub session: String,
    pub provider: CredentialProviderId,
    pub private_fields: Vec<PrivateCredentialField>,
    pub safe_label: Option<SafeCredentialLabel>,
    pub allowed_targets: Vec<SecretTargetConstraint>,
    pub expires_at: Option<SystemTime>,
    pub max_releases: Option<u32>,
    pub authorization: SecretReleaseAuthorization,
}

impl fmt::Debug for CredentialBindingSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialBindingSpec")
            .field("session", &"[private]")
            .field("provider", &self.provider)
            .field("private_fields", &"[private]")
            .field("safe_label", &self.safe_label)
            .field("allowed_target_count", &self.allowed_targets.len())
            .field("expires_at", &self.expires_at)
            .field("max_releases", &self.max_releases)
            .field("authorization", &self.authorization)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingRegistrationId(Uuid);

impl fmt::Debug for BindingRegistrationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BindingRegistrationId([private])")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BindingDigest([u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSecretFrameKind {
    Main,
    Iframe,
    Oopif,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSecretFrameIdentity {
    pub kind: BrowserSecretFrameKind,
    pub frame_id: String,
    pub loader_id: String,
}

/// Browser identity proven before target-first credential discovery.
///
/// Fields remain private so only the browser engine can mint one from a live
/// semantic-v2 ref. Tests in this module use the same checked constructor.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedBrowserSecretTarget {
    runtime_scope: String,
    session: String,
    process: ProcessFingerprint,
    endpoint_generation: u64,
    target_id: String,
    tab_id: String,
    frame: BrowserSecretFrameIdentity,
    origin: ExactBrowserOrigin,
    semantic_ref: String,
    secure_field: SecureFieldKind,
}

impl fmt::Debug for VerifiedBrowserSecretTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedBrowserSecretTarget")
            .field("runtime_scope", &"[private]")
            .field("session", &"[private]")
            .field("process", &"[bound process]")
            .field("endpoint_generation", &"[bound generation]")
            .field("target_id", &"[private]")
            .field("tab_id", &"[private]")
            .field("frame", &"[bound frame]")
            .field("origin", &"[bound origin]")
            .field("semantic_ref", &"[private]")
            .field("secure_field", &self.secure_field)
            .finish()
    }
}

impl VerifiedBrowserSecretTarget {
    // Used by the browser engine in the delivery slice; tests exercise the
    // checked minting contract before that route is published.
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        runtime_scope: String,
        session: String,
        process: ProcessFingerprint,
        endpoint_generation: u64,
        target_id: String,
        tab_id: String,
        frame: BrowserSecretFrameIdentity,
        live_url: &str,
        semantic_ref: String,
        secure_field: SecureFieldKind,
    ) -> Result<Self, SecretTargetError> {
        if validate_runtime_scope(&runtime_scope).is_err()
            || !session.starts_with(&format!("__cua_runtime_{runtime_scope}:"))
            || process.pid <= 0
            || (process.start_time.is_none() && process.executable.is_none())
            || endpoint_generation == 0
            || target_id.is_empty()
            || tab_id.is_empty()
            || frame.frame_id.is_empty()
            || frame.loader_id.is_empty()
            || parse_ref(&semantic_ref).is_none()
            || secure_field != SecureFieldKind::Password
        {
            return Err(SecretTargetError::Unproven);
        }
        let parsed = url::Url::parse(live_url).map_err(|_| SecretTargetError::Unproven)?;
        let origin = ExactBrowserOrigin::parse(&parsed.origin().ascii_serialization())
            .map_err(|_| SecretTargetError::Unproven)?;
        Ok(Self {
            runtime_scope,
            session,
            process,
            endpoint_generation,
            target_id,
            tab_id,
            frame,
            origin,
            semantic_ref,
            secure_field,
        })
    }

    pub(crate) fn session(&self) -> &str {
        &self.session
    }

    pub(crate) fn runtime_scope(&self) -> &str {
        &self.runtime_scope
    }

    pub(crate) fn origin(&self) -> &ExactBrowserOrigin {
        &self.origin
    }

    pub(crate) fn secure_field(&self) -> SecureFieldKind {
        self.secure_field
    }

    fn context_matches(
        &self,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
    ) -> bool {
        if context.runtime_scope_key() != self.runtime_scope {
            return false;
        }
        context
            .public_session()
            .is_none_or(|session| context.runtime_session_key(session) == self.session)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SecretTargetError {
    #[error("the browser secret target cannot be proven")]
    Unproven,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialHandle(Arc<str>);

impl CredentialHandle {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialHandle([opaque])")
    }
}

/// Exact, content-free authorization resource prepared by the broker before a
/// provider can be invoked. It is intentionally non-serializable and its Debug
/// representation never exposes binding, handle, provider, or target identity.
pub(crate) struct SecretReleaseResource {
    handle_digest: [u8; 32],
    binding_digest: BindingDigest,
    authorization: SecretReleaseAuthorization,
    provider: CredentialProviderId,
    provider_class: CredentialProviderClass,
    field: CredentialField,
    target: VerifiedBrowserSecretTarget,
}

impl fmt::Debug for SecretReleaseResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretReleaseResource")
            .field("provider_class", &self.provider_class)
            .field("field", &self.field)
            .field("target", &"[bound target]")
            .finish()
    }
}

impl SecretReleaseResource {
    pub(crate) fn validate_context(
        &self,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
    ) -> Result<(), SecretReleaseAuthorizationError> {
        if context.is_expired() {
            return Err(SecretReleaseAuthorizationError::ContextExpired);
        }
        if !self.target.context_matches(context) {
            return Err(SecretReleaseAuthorizationError::ScopeMismatch);
        }
        Ok(())
    }

    pub(crate) fn policy_args(&self) -> serde_json::Value {
        serde_json::json!({
            "resource_kind": "bound_secret_release_to_verified_target",
            "provider_class": self.provider_class.as_str(),
            "field": self.field.as_str(),
            "target_kind": "browser_password",
            "origin": self.target.origin.as_str(),
        })
    }

    pub(crate) fn manifest_resource(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": "bound_secret_release_to_verified_target",
            "authorization": self.authorization.as_str(),
            "origin": self.target.origin.as_str(),
            "field": self.field.as_str(),
            "secure_field": self.target.secure_field.as_str(),
        })
    }

    pub(crate) fn authorize(
        &self,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
    ) -> SecretReleasePermit {
        SecretReleasePermit {
            digest: secret_release_authorization_digest(self, context),
            context: context.clone(),
        }
    }
}

/// Single-use process-local proof that the exact broker resource passed the R3
/// permission, policy, and manifest boundary.
pub(crate) struct SecretReleasePermit {
    digest: [u8; 32],
    context: crate::session_authorization::EffectiveAuthorizationContext,
}

impl SecretReleasePermit {
    fn validate(
        &self,
        resource: &SecretReleaseResource,
    ) -> Result<(), SecretReleaseAuthorizationError> {
        resource.validate_context(&self.context)?;
        if self.digest != secret_release_authorization_digest(resource, &self.context) {
            return Err(SecretReleaseAuthorizationError::ScopeMismatch);
        }
        Ok(())
    }

    fn ensure_live(&self) -> Result<(), SecretReleaseAuthorizationError> {
        if self.context.is_expired() {
            Err(SecretReleaseAuthorizationError::ContextExpired)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SecretReleaseAuthorizationError {
    #[error("secret release is denied in standard mode")]
    StandardDenied,
    #[error("secret release requires an approved capability manifest")]
    ManifestRequired,
    #[error("secret release is outside the capability manifest")]
    ManifestDenied,
    #[error("secret release is denied by policy")]
    PolicyDenied,
    #[error("secret release authorization context expired")]
    ContextExpired,
    #[error("secret release authorization scope does not match")]
    ScopeMismatch,
}

#[derive(Clone)]
pub struct CredentialDescriptor {
    pub handle: CredentialHandle,
    pub label: Option<SafeCredentialLabel>,
    pub fields: Vec<CredentialField>,
    pub provider_class: Option<CredentialProviderClass>,
}

impl fmt::Debug for CredentialDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialDescriptor")
            .field("handle", &self.handle)
            .field("label", &self.label)
            .field("fields", &self.fields)
            .field("provider_class", &self.provider_class)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderReleaseId(Uuid);

impl fmt::Debug for ProviderReleaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderReleaseId([private])")
    }
}

pub struct ProviderReleaseRequest {
    release_id: ProviderReleaseId,
    field: CredentialField,
    locator: PrivateCredentialFieldLocator,
    target: VerifiedBrowserSecretTarget,
    cancelled: Arc<AtomicBool>,
}

impl ProviderReleaseRequest {
    pub fn release_id(&self) -> ProviderReleaseId {
        self.release_id
    }

    pub fn field(&self) -> CredentialField {
        self.field
    }

    pub fn locator(&self) -> &PrivateCredentialFieldLocator {
        &self.locator
    }

    pub fn target(&self) -> &VerifiedBrowserSecretTarget {
        &self.target
    }

    /// Providers must call this immediately before every external effect,
    /// including prompts, unlock attempts, IPC writes, and secret reads.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(AtomicOrdering::Acquire)
            || is_runtime_scope_suspended(self.target.runtime_scope())
            || is_session_ended(self.target.session())
    }

    pub fn ensure_not_cancelled(&self) -> Result<(), CredentialProviderError> {
        if self.is_cancelled() {
            Err(CredentialProviderError::new(
                CredentialProviderErrorKind::Cancelled,
            ))
        } else {
            Ok(())
        }
    }
}

pub struct SecretLease {
    #[allow(dead_code)]
    value: Zeroizing<Vec<u8>>,
}

impl SecretLease {
    pub fn from_utf8_bytes(value: Vec<u8>) -> Result<Self, SecretLeaseError> {
        if value.is_empty() {
            return Err(SecretLeaseError::Empty);
        }
        if value.len() > MAX_SECRET_BYTES {
            return Err(SecretLeaseError::TooLarge);
        }
        std::str::from_utf8(&value).map_err(|_| SecretLeaseError::InvalidUtf8)?;
        Ok(Self {
            value: Zeroizing::new(value),
        })
    }

    #[allow(dead_code)]
    pub(crate) fn expose_for_delivery(&self) -> &str {
        std::str::from_utf8(self.value.as_slice()).expect("secret lease UTF-8 invariant")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SecretLeaseError {
    #[error("the provider returned an empty secret")]
    Empty,
    #[error("the provider returned an oversized secret")]
    TooLarge,
    #[error("the provider returned a non-UTF-8 secret")]
    InvalidUtf8,
}

pub struct ProviderFillSession {
    _opaque: Zeroizing<Vec<u8>>,
}

impl ProviderFillSession {
    pub fn from_opaque_bytes(value: Vec<u8>) -> Result<Self, CredentialProviderError> {
        if value.is_empty() || value.len() > 1_024 {
            return Err(CredentialProviderError::new(
                CredentialProviderErrorKind::MalformedResponse,
            ));
        }
        Ok(Self {
            _opaque: Zeroizing::new(value),
        })
    }
}

pub struct UserPresenceHandle {
    _opaque: Zeroizing<Vec<u8>>,
}

impl UserPresenceHandle {
    pub fn from_opaque_bytes(value: Vec<u8>) -> Result<Self, CredentialProviderError> {
        if value.is_empty() || value.len() > 1_024 {
            return Err(CredentialProviderError::new(
                CredentialProviderErrorKind::MalformedResponse,
            ));
        }
        Ok(Self {
            _opaque: Zeroizing::new(value),
        })
    }
}

pub enum ReleasePlan {
    RuntimeDelivers(SecretLease),
    ProviderFills(ProviderFillSession),
    NeedsUserPresence(UserPresenceHandle),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleasePlanKind {
    RuntimeDelivers,
    ProviderFills,
    NeedsUserPresence,
}

/// Content-free result of a target-bound credential delivery request.
/// Provider-specific metadata never crosses this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialDeliveryOutcome {
    Delivered,
    ProviderFillUnavailable,
    UserPresenceRequired,
}

impl CredentialDeliveryOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::ProviderFillUnavailable => "provider_fill_unavailable",
            Self::UserPresenceRequired => "user_presence_required",
        }
    }
}

impl ReleasePlan {
    pub fn kind(&self) -> ReleasePlanKind {
        match self {
            Self::RuntimeDelivers(_) => ReleasePlanKind::RuntimeDelivers,
            Self::ProviderFills(_) => ReleasePlanKind::ProviderFills,
            Self::NeedsUserPresence(_) => ReleasePlanKind::NeedsUserPresence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHealthState {
    Available,
    Locked,
    Unavailable,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderHealth {
    pub state: ProviderHealthState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialProviderErrorKind {
    Unavailable,
    PermissionDenied,
    NotFound,
    Locked,
    UserPresenceRequired,
    MalformedResponse,
    OversizedSecret,
    InvalidUtf8,
    Timeout,
    Cancelled,
    ProviderDied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("credential provider release failed")]
pub struct CredentialProviderError {
    kind: CredentialProviderErrorKind,
}

impl CredentialProviderError {
    pub fn new(kind: CredentialProviderErrorKind) -> Self {
        Self { kind }
    }

    pub fn kind(self) -> CredentialProviderErrorKind {
        self.kind
    }
}

#[async_trait]
pub trait CredentialProvider: Send + Sync {
    fn id(&self) -> &CredentialProviderId;
    fn class(&self) -> CredentialProviderClass;

    /// Implementations must recheck [`ProviderReleaseRequest::is_cancelled`]
    /// immediately before each provider-side effect.
    async fn release(
        &self,
        request: &ProviderReleaseRequest,
    ) -> Result<ReleasePlan, CredentialProviderError>;

    /// Cancellation is idempotent and may arrive before `release` observes the
    /// ID. Providers must retain a bounded tombstone so that a later release
    /// cannot prompt, unlock, fetch, fill, or otherwise create an effect.
    async fn cancel(&self, release: ProviderReleaseId) -> Result<(), CredentialProviderError>;

    async fn health(&self) -> ProviderHealth;
}

pub struct BootstrapSecret {
    value: Zeroizing<Vec<u8>>,
}

impl BootstrapSecret {
    pub fn from_bytes(value: Vec<u8>) -> Result<Self, BootstrapStoreError> {
        if value.is_empty() || value.len() > MAX_BOOTSTRAP_SECRET_BYTES {
            return Err(BootstrapStoreError::InvalidValue);
        }
        Ok(Self {
            value: Zeroizing::new(value),
        })
    }

    pub fn into_lease(self) -> BootstrapSecretLease {
        BootstrapSecretLease { value: self.value }
    }
}

pub struct BootstrapSecretLease {
    #[allow(dead_code)]
    value: Zeroizing<Vec<u8>>,
}

impl BootstrapSecretLease {
    /// Trusted provider adapters may inspect the lease only while resolving one
    /// authorized release. The lease is never serializable or agent-visible.
    #[doc(hidden)]
    pub fn expose_to_provider(&self) -> &[u8] {
        self.value.as_slice()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BootstrapNamespace(Arc<str>);

impl BootstrapNamespace {
    pub fn new(value: impl Into<String>) -> Result<Self, BootstrapStoreError> {
        let value = value.into();
        validate_identifier(&value, 128).map_err(|_| BootstrapStoreError::InvalidNamespace)?;
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BootstrapNamespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BootstrapNamespace([private])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapVersion(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapStoreHealth {
    Available,
    Missing,
    Locked,
    Unavailable,
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BootstrapStoreError {
    #[error("invalid bootstrap namespace")]
    InvalidNamespace,
    #[error("invalid bootstrap secret")]
    InvalidValue,
    #[error("bootstrap secret is already enrolled")]
    AlreadyEnrolled,
    #[error("bootstrap secret is missing")]
    Missing,
    #[error("bootstrap secret store is locked")]
    Locked,
    #[error("bootstrap secret store is unavailable")]
    Unavailable,
    #[error("bootstrap secret record is corrupt")]
    Corrupt,
}

/// Encode one platform credential-store record without exposing the bootstrap
/// value through a serializable public type. Platform adapters own persistence;
/// this helper keeps their record format and validation identical.
#[doc(hidden)]
pub fn encode_bootstrap_store_record(
    version: BootstrapVersion,
    value: BootstrapSecret,
) -> Result<Zeroizing<Vec<u8>>, BootstrapStoreError> {
    if version.0 == 0 {
        return Err(BootstrapStoreError::Corrupt);
    }
    let mut record = Zeroizing::new(Vec::with_capacity(
        BOOTSTRAP_RECORD_HEADER_BYTES + value.value.len(),
    ));
    record.extend_from_slice(BOOTSTRAP_RECORD_MAGIC);
    record.extend_from_slice(&version.0.to_be_bytes());
    record.extend_from_slice(value.value.as_slice());
    Ok(record)
}

/// Decode a platform credential-store record into a zeroizing lease.
#[doc(hidden)]
pub fn decode_bootstrap_store_record(
    record: Vec<u8>,
) -> Result<(BootstrapVersion, BootstrapSecretLease), BootstrapStoreError> {
    let record = Zeroizing::new(record);
    if record.len() <= BOOTSTRAP_RECORD_HEADER_BYTES
        || record.len() > BOOTSTRAP_RECORD_HEADER_BYTES + MAX_BOOTSTRAP_SECRET_BYTES
        || &record[..BOOTSTRAP_RECORD_MAGIC.len()] != BOOTSTRAP_RECORD_MAGIC
    {
        return Err(BootstrapStoreError::Corrupt);
    }
    let version = u64::from_be_bytes(
        record[BOOTSTRAP_RECORD_MAGIC.len()..BOOTSTRAP_RECORD_HEADER_BYTES]
            .try_into()
            .map_err(|_| BootstrapStoreError::Corrupt)?,
    );
    if version == 0 {
        return Err(BootstrapStoreError::Corrupt);
    }
    Ok((
        BootstrapVersion(version),
        BootstrapSecretLease {
            value: Zeroizing::new(record[BOOTSTRAP_RECORD_HEADER_BYTES..].to_vec()),
        },
    ))
}

pub trait BootstrapSecretStore: Send + Sync {
    fn enroll(
        &self,
        namespace: BootstrapNamespace,
        value: BootstrapSecret,
    ) -> Result<BootstrapVersion, BootstrapStoreError>;
    fn load(
        &self,
        namespace: &BootstrapNamespace,
    ) -> Result<BootstrapSecretLease, BootstrapStoreError>;
    fn rotate(
        &self,
        namespace: &BootstrapNamespace,
        value: BootstrapSecret,
    ) -> Result<BootstrapVersion, BootstrapStoreError>;
    fn revoke(&self, namespace: &BootstrapNamespace) -> Result<(), BootstrapStoreError>;
    fn health(&self, namespace: &BootstrapNamespace) -> BootstrapStoreHealth;
}

mod service_account_provider;

pub use service_account_provider::{
    DedicatedServiceAccountBootstrap, DedicatedServiceAccountProvider,
    ServiceAccountReleaseCancellation, ServiceAccountVaultClient,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CredentialConfigurationError {
    #[error("invalid runtime scope")]
    InvalidRuntimeScope,
    #[error("invalid credential provider")]
    InvalidProvider,
    #[error("duplicate credential provider")]
    DuplicateProvider,
    #[error("credential binding references a missing provider")]
    MissingProvider,
    #[error("credential binding belongs to another runtime generation")]
    RuntimeMismatch,
    #[error("credential binding has an invalid lifecycle session")]
    InvalidSession,
    #[error("credential binding belongs to an ended lifecycle session")]
    SessionEnded,
    #[error("credential broker runtime is terminally suspended")]
    RuntimeSuspended,
    #[error("credential binding has no private fields")]
    MissingPrivateField,
    #[error("credential binding repeats a private field")]
    DuplicatePrivateField,
    #[error("invalid private credential locator")]
    InvalidPrivateLocator,
    #[error("invalid safe credential label")]
    InvalidSafeLabel,
    #[error("credential binding has no target constraint")]
    MissingTargetConstraint,
    #[error("invalid credential target constraint")]
    InvalidTargetConstraint,
    #[error("invalid credential release limit")]
    InvalidReleaseLimit,
    #[error("invalid credential authorization scope")]
    InvalidAuthorizationScope,
    #[error("credential bindings repeat an authorization scope")]
    DuplicateAuthorizationScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SecretBrokerError {
    #[error("credential broker runtime is terminally suspended")]
    RuntimeSuspended,
    #[error("credential lifecycle session has ended")]
    SessionEnded,
    #[error("credential handle capacity is exhausted")]
    Capacity,
    #[error("credential handle is invalid")]
    InvalidHandle,
    #[error("credential handle expired")]
    HandleExpired,
    #[error("credential handle was revoked")]
    HandleRevoked,
    #[error("credential handle was already consumed")]
    HandleConsumed,
    #[error("credential handle belongs to another target")]
    TargetMismatch,
    #[error("credential handle does not admit that field")]
    FieldMismatch,
    #[error("credential binding expired")]
    BindingExpired,
    #[error("credential binding was revoked")]
    BindingRevoked,
    #[error("credential binding release limit was reached")]
    ReleaseLimitReached,
    #[error("credential provider is unavailable")]
    ProviderUnavailable,
    #[error("credential provider release failed")]
    Provider(CredentialProviderErrorKind),
    #[error("credential release authorization failed")]
    Authorization(SecretReleaseAuthorizationError),
}

pub struct SecretBrokerBuilder {
    runtime_scope: String,
    handle_ttl: Duration,
    providers: HashMap<CredentialProviderId, Arc<dyn CredentialProvider>>,
    bindings: Vec<(BindingRegistrationId, CredentialBindingSpec)>,
}

impl SecretBrokerBuilder {
    pub fn new(runtime_scope: impl Into<String>) -> Result<Self, CredentialConfigurationError> {
        let runtime_scope = runtime_scope.into();
        validate_runtime_scope(&runtime_scope)
            .map_err(|_| CredentialConfigurationError::InvalidRuntimeScope)?;
        Ok(Self {
            runtime_scope,
            handle_ttl: DEFAULT_CREDENTIAL_HANDLE_TTL,
            providers: HashMap::new(),
            bindings: Vec::new(),
        })
    }

    pub fn runtime_session_key(
        &self,
        public_session: &str,
    ) -> Result<String, CredentialConfigurationError> {
        if public_session.is_empty()
            || public_session == "default"
            || public_session.starts_with("__cua_runtime_")
            || public_session.chars().any(char::is_control)
        {
            return Err(CredentialConfigurationError::InvalidSession);
        }
        Ok(format!(
            "__cua_runtime_{}:{public_session}",
            self.runtime_scope
        ))
    }

    pub fn with_handle_ttl(
        mut self,
        handle_ttl: Duration,
    ) -> Result<Self, CredentialConfigurationError> {
        if handle_ttl.is_zero() || handle_ttl > Duration::from_secs(300) {
            return Err(CredentialConfigurationError::InvalidReleaseLimit);
        }
        self.handle_ttl = handle_ttl;
        Ok(self)
    }

    pub fn add_provider(
        &mut self,
        provider: Arc<dyn CredentialProvider>,
    ) -> Result<(), CredentialConfigurationError> {
        let id = provider.id().clone();
        if self.providers.contains_key(&id) {
            return Err(CredentialConfigurationError::DuplicateProvider);
        }
        self.providers.insert(id, provider);
        Ok(())
    }

    pub fn add_binding(
        &mut self,
        spec: CredentialBindingSpec,
    ) -> Result<BindingRegistrationId, CredentialConfigurationError> {
        validate_binding_spec(&self.runtime_scope, &spec)?;
        if is_session_ended(&spec.session) {
            return Err(CredentialConfigurationError::SessionEnded);
        }
        if self
            .bindings
            .iter()
            .any(|(_, existing)| existing.authorization == spec.authorization)
        {
            return Err(CredentialConfigurationError::DuplicateAuthorizationScope);
        }
        let id = BindingRegistrationId(Uuid::new_v4());
        self.bindings.push((id, spec));
        Ok(id)
    }

    pub fn build(self) -> Result<Arc<SecretBroker>, CredentialConfigurationError> {
        if is_runtime_scope_suspended(&self.runtime_scope) {
            return Err(CredentialConfigurationError::RuntimeSuspended);
        }
        let mut records = Vec::with_capacity(self.bindings.len());
        for (id, spec) in self.bindings {
            if is_session_ended(&spec.session) {
                return Err(CredentialConfigurationError::SessionEnded);
            }
            let provider = self
                .providers
                .get(&spec.provider)
                .ok_or(CredentialConfigurationError::MissingProvider)?;
            records.push(BindingRecord {
                id,
                digest: binding_digest(&self.runtime_scope, &spec),
                provider_class: provider.class(),
                spec,
                releases_reserved: 0,
                revoked: false,
            });
        }
        let broker = Arc::new(SecretBroker {
            runtime_scope: self.runtime_scope,
            handle_ttl: self.handle_ttl,
            providers: self.providers,
            state: Arc::new(Mutex::new(BrokerState {
                bindings: records,
                handles: HashMap::new(),
                handle_tombstones: HashMap::new(),
                revoked_providers: HashSet::new(),
                active_releases: HashMap::new(),
                runtime_revoked: false,
            })),
            session_end_hook: Mutex::new(None),
            runtime_suspend_hook: Mutex::new(None),
        });
        let weak: Weak<SecretBroker> = Arc::downgrade(&broker);
        let registration = register_scoped_session_end_hook(move |session| {
            if let Some(broker) = weak.upgrade() {
                broker.revoke_session(session);
            }
        });
        *broker.session_end_hook.lock().unwrap() = Some(registration);
        let weak: Weak<SecretBroker> = Arc::downgrade(&broker);
        let registration = register_scoped_runtime_suspend_hook(move |runtime_scope| {
            if let Some(broker) = weak.upgrade() {
                if broker.runtime_scope == runtime_scope {
                    broker.revoke_runtime();
                }
            }
        });
        *broker.runtime_suspend_hook.lock().unwrap() = Some(registration);

        // Close races where teardown completed before either hook registration.
        if is_runtime_scope_suspended(&broker.runtime_scope) {
            broker.revoke_runtime();
            return Err(CredentialConfigurationError::RuntimeSuspended);
        }
        if broker
            .state
            .lock()
            .unwrap()
            .bindings
            .iter()
            .any(|binding| is_session_ended(&binding.spec.session))
        {
            return Err(CredentialConfigurationError::SessionEnded);
        }
        Ok(broker)
    }
}

pub struct SecretBroker {
    runtime_scope: String,
    handle_ttl: Duration,
    providers: HashMap<CredentialProviderId, Arc<dyn CredentialProvider>>,
    state: Arc<Mutex<BrokerState>>,
    session_end_hook: Mutex<Option<SessionEndHookRegistration>>,
    runtime_suspend_hook: Mutex<Option<RuntimeSuspendHookRegistration>>,
}

struct BrokerState {
    bindings: Vec<BindingRecord>,
    handles: HashMap<String, HandleRecord>,
    handle_tombstones: HashMap<String, HandleTombstone>,
    revoked_providers: HashSet<CredentialProviderId>,
    active_releases: HashMap<ProviderReleaseId, ActiveRelease>,
    runtime_revoked: bool,
}

struct ActiveRelease {
    binding_id: BindingRegistrationId,
    provider_id: CredentialProviderId,
    provider: Arc<dyn CredentialProvider>,
    session: String,
    revoked: Arc<AtomicBool>,
    runtime: Option<tokio::runtime::Handle>,
}

struct BindingRecord {
    id: BindingRegistrationId,
    digest: BindingDigest,
    provider_class: CredentialProviderClass,
    spec: CredentialBindingSpec,
    releases_reserved: u32,
    revoked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandleTerminalState {
    Consumed,
    Revoked,
    Expired,
}

#[derive(Clone)]
struct HandleRecord {
    binding_id: BindingRegistrationId,
    binding_digest: BindingDigest,
    provider: CredentialProviderId,
    field: CredentialField,
    target: VerifiedBrowserSecretTarget,
    expires_at: SystemTime,
}

struct HandleTombstone {
    state: HandleTerminalState,
    forget_after: SystemTime,
}

impl SecretBroker {
    pub fn runtime_scope(&self) -> &str {
        &self.runtime_scope
    }

    #[cfg(test)]
    pub(crate) fn active_handle_count(&self) -> usize {
        self.state.lock().unwrap().handles.len()
    }

    pub fn find_credentials(
        &self,
        target: &VerifiedBrowserSecretTarget,
        now: SystemTime,
    ) -> Result<Vec<CredentialDescriptor>, SecretBrokerError> {
        if target.runtime_scope() != self.runtime_scope {
            return Err(SecretBrokerError::TargetMismatch);
        }
        let mut state = self.state.lock().unwrap();
        self.prune_handle_state(&mut state, now);
        self.ensure_runtime_live(&state)?;
        if is_session_ended(target.session()) {
            return Ok(Vec::new());
        }

        let mut candidates = Vec::new();
        for (binding_index, binding) in state.bindings.iter().enumerate() {
            if binding.revoked
                || binding.spec.session != target.session()
                || binding
                    .spec
                    .expires_at
                    .is_some_and(|expires_at| expires_at <= now)
                || binding
                    .spec
                    .max_releases
                    .is_some_and(|maximum| binding.releases_reserved >= maximum)
                || state.revoked_providers.contains(&binding.spec.provider)
                || !binding
                    .spec
                    .allowed_targets
                    .iter()
                    .any(|constraint| target_matches(constraint, target))
            {
                continue;
            }
            if !self.providers.contains_key(&binding.spec.provider) {
                return Err(SecretBrokerError::ProviderUnavailable);
            }
            for (field_index, _) in binding.spec.private_fields.iter().enumerate() {
                candidates.push((binding_index, field_index));
            }
        }

        let active_for_session = state
            .handles
            .values()
            .filter(|handle| handle.target.session() == target.session())
            .count();
        if state.handles.len().saturating_add(candidates.len()) > MAX_ACTIVE_HANDLES
            || active_for_session.saturating_add(candidates.len()) > MAX_ACTIVE_HANDLES_PER_SESSION
        {
            return Err(SecretBrokerError::Capacity);
        }

        let mut descriptors = Vec::with_capacity(candidates.len());
        let mut minted = Vec::with_capacity(candidates.len());
        for (binding_index, field_index) in candidates {
            let binding = &state.bindings[binding_index];
            let private_field = &binding.spec.private_fields[field_index];
            let expires_at = now
                .checked_add(self.handle_ttl)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let expires_at = binding
                .spec
                .expires_at
                .map_or(expires_at, |binding_expiry| expires_at.min(binding_expiry));
            let raw_handle = format!("ch-{}", Uuid::new_v4().simple());
            minted.push((
                raw_handle.clone(),
                HandleRecord {
                    binding_id: binding.id,
                    binding_digest: binding.digest,
                    provider: binding.spec.provider.clone(),
                    field: private_field.field,
                    target: target.clone(),
                    expires_at,
                },
            ));
            descriptors.push(CredentialDescriptor {
                handle: CredentialHandle(raw_handle.into()),
                label: binding.spec.safe_label.clone(),
                fields: vec![private_field.field],
                provider_class: Some(binding.provider_class),
            });
        }
        state.handles.extend(minted);
        Ok(descriptors)
    }

    pub(crate) fn prepare_secret_release(
        &self,
        handle: &CredentialHandle,
        field: CredentialField,
        target: &VerifiedBrowserSecretTarget,
        now: SystemTime,
    ) -> Result<SecretReleaseResource, SecretBrokerError> {
        let mut state = self.state.lock().unwrap();
        self.prune_handle_state(&mut state, now);
        let (record, binding_index) =
            self.validate_release_state(&state, handle, field, target, now)?;
        Ok(secret_release_resource(
            handle,
            &record,
            &state.bindings[binding_index],
            field,
            target,
        ))
    }

    fn validate_release_state(
        &self,
        state: &BrokerState,
        handle: &CredentialHandle,
        field: CredentialField,
        target: &VerifiedBrowserSecretTarget,
        now: SystemTime,
    ) -> Result<(HandleRecord, usize), SecretBrokerError> {
        self.ensure_runtime_live(state)?;
        let record = match state.handles.get(handle.as_str()).cloned() {
            Some(record) => record,
            None => {
                return Err(
                    match state
                        .handle_tombstones
                        .get(handle.as_str())
                        .map(|tombstone| tombstone.state)
                    {
                        Some(HandleTerminalState::Consumed) => SecretBrokerError::HandleConsumed,
                        Some(HandleTerminalState::Revoked) => SecretBrokerError::HandleRevoked,
                        Some(HandleTerminalState::Expired) => SecretBrokerError::HandleExpired,
                        None => SecretBrokerError::InvalidHandle,
                    },
                )
            }
        };
        if is_session_ended(target.session()) {
            return Err(SecretBrokerError::SessionEnded);
        }
        if record.expires_at <= now {
            return Err(SecretBrokerError::HandleExpired);
        }
        if record.field != field {
            return Err(SecretBrokerError::FieldMismatch);
        }
        if record.target != *target || target.runtime_scope() != self.runtime_scope {
            return Err(SecretBrokerError::TargetMismatch);
        }
        let binding_index = state
            .bindings
            .iter()
            .position(|binding| binding.id == record.binding_id)
            .ok_or(SecretBrokerError::BindingRevoked)?;
        let binding = &state.bindings[binding_index];
        if binding.digest != record.binding_digest
            || binding.spec.provider != record.provider
            || binding.revoked
        {
            return Err(SecretBrokerError::BindingRevoked);
        }
        if binding
            .spec
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(SecretBrokerError::BindingExpired);
        }
        if binding
            .spec
            .max_releases
            .is_some_and(|maximum| binding.releases_reserved >= maximum)
        {
            return Err(SecretBrokerError::ReleaseLimitReached);
        }
        if state.revoked_providers.contains(&record.provider)
            || !self.providers.contains_key(&record.provider)
        {
            return Err(SecretBrokerError::ProviderUnavailable);
        }
        if !binding
            .spec
            .private_fields
            .iter()
            .any(|private_field| private_field.field == field)
        {
            return Err(SecretBrokerError::FieldMismatch);
        }
        Ok((record, binding_index))
    }

    pub(crate) async fn release(
        &self,
        handle: &CredentialHandle,
        field: CredentialField,
        target: &VerifiedBrowserSecretTarget,
        now: SystemTime,
        permit: SecretReleasePermit,
    ) -> Result<BrokeredRelease, SecretBrokerError> {
        let release_id = ProviderReleaseId(Uuid::new_v4());
        let revoked = Arc::new(AtomicBool::new(false));
        let (provider, request) = {
            let mut state = self.state.lock().unwrap();
            self.prune_handle_state(&mut state, now);
            let (record, binding_index) =
                self.validate_release_state(&state, handle, field, target, now)?;
            let resource = secret_release_resource(
                handle,
                &record,
                &state.bindings[binding_index],
                field,
                target,
            );
            permit
                .validate(&resource)
                .map_err(SecretBrokerError::Authorization)?;
            let locator = state.bindings[binding_index]
                .spec
                .private_fields
                .iter()
                .find(|private_field| private_field.field == field)
                .map(|private_field| private_field.locator.clone())
                .ok_or(SecretBrokerError::FieldMismatch)?;
            let provider = self
                .providers
                .get(&record.provider)
                .cloned()
                .ok_or(SecretBrokerError::ProviderUnavailable)?;
            let consumed = state
                .handles
                .remove(handle.as_str())
                .expect("validated handle remains present");
            insert_handle_tombstone(
                &mut state,
                handle.as_str().to_owned(),
                HandleTerminalState::Consumed,
                consumed.expires_at,
            );
            state.bindings[binding_index].releases_reserved = state.bindings[binding_index]
                .releases_reserved
                .saturating_add(1);
            state.active_releases.insert(
                release_id,
                ActiveRelease {
                    binding_id: record.binding_id,
                    provider_id: record.provider,
                    provider: provider.clone(),
                    session: record.target.session().to_owned(),
                    revoked: revoked.clone(),
                    runtime: tokio::runtime::Handle::try_current().ok(),
                },
            );
            (
                provider,
                ProviderReleaseRequest {
                    release_id,
                    field,
                    locator,
                    target: target.clone(),
                    cancelled: revoked.clone(),
                },
            )
        };

        let mut cancellation = ProviderCancellation::new(
            provider.clone(),
            release_id,
            Arc::downgrade(&self.state),
            revoked.clone(),
        );
        if request.is_cancelled() {
            cancellation.disarm();
            return Err(SecretBrokerError::HandleRevoked);
        }
        if let Err(error) = permit.ensure_live() {
            return Err(SecretBrokerError::Authorization(error));
        }
        let plan = match provider.release(&request).await {
            Ok(plan) => plan,
            Err(_) if request.is_cancelled() || cancellation.is_revoked() => {
                cancellation.disarm();
                return Err(SecretBrokerError::HandleRevoked);
            }
            Err(error) => return Err(SecretBrokerError::Provider(error.kind())),
        };
        if request.is_cancelled() || cancellation.is_revoked() {
            drop(plan);
            cancellation.disarm();
            return Err(SecretBrokerError::HandleRevoked);
        }
        if let Err(error) = permit.ensure_live() {
            drop(plan);
            return Err(SecretBrokerError::Authorization(error));
        }
        Ok(BrokeredRelease {
            cancellation,
            plan: Some(plan),
            target: target.clone(),
            permit,
        })
    }

    pub fn revoke_binding(&self, registration: BindingRegistrationId) -> bool {
        let mut state = self.state.lock().unwrap();
        let mut changed = false;
        for binding in &mut state.bindings {
            if binding.id == registration && !binding.revoked {
                binding.revoked = true;
                changed = true;
            }
        }
        if changed {
            revoke_matching_handles(&mut state, |handle| handle.binding_id == registration);
        }
        let cancellations =
            take_active_releases(&mut state, |release| release.binding_id == registration);
        drop(state);
        spawn_provider_cancellations(cancellations);
        changed
    }

    pub fn revoke_session(&self, session: &str) -> usize {
        let mut state = self.state.lock().unwrap();
        let revoked = state
            .bindings
            .iter_mut()
            .filter(|binding| binding.spec.session == session && !binding.revoked)
            .map(|binding| {
                binding.revoked = true;
                binding.id
            })
            .collect::<HashSet<_>>();
        revoke_matching_handles(&mut state, |handle| revoked.contains(&handle.binding_id));
        let cancellations = take_active_releases(&mut state, |release| release.session == session);
        drop(state);
        spawn_provider_cancellations(cancellations);
        revoked.len()
    }

    pub fn revoke_provider(&self, provider: &CredentialProviderId) -> bool {
        if !self.providers.contains_key(provider) {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        let changed = state.revoked_providers.insert(provider.clone());
        if changed {
            for binding in &mut state.bindings {
                if &binding.spec.provider == provider {
                    binding.revoked = true;
                }
            }
            revoke_matching_handles(&mut state, |handle| &handle.provider == provider);
        }
        let cancellations =
            take_active_releases(&mut state, |release| &release.provider_id == provider);
        drop(state);
        spawn_provider_cancellations(cancellations);
        changed
    }

    fn revoke_runtime(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.runtime_revoked {
            return false;
        }
        state.runtime_revoked = true;
        for binding in &mut state.bindings {
            binding.revoked = true;
        }
        revoke_matching_handles(&mut state, |_| true);
        let cancellations = take_active_releases(&mut state, |_| true);
        drop(state);
        spawn_provider_cancellations(cancellations);
        true
    }

    fn ensure_runtime_live(&self, state: &BrokerState) -> Result<(), SecretBrokerError> {
        if state.runtime_revoked || is_runtime_scope_suspended(&self.runtime_scope) {
            return Err(SecretBrokerError::RuntimeSuspended);
        }
        Ok(())
    }

    fn prune_handle_state(&self, state: &mut BrokerState, now: SystemTime) {
        state
            .handle_tombstones
            .retain(|_, tombstone| tombstone.forget_after > now);
        let expired = state
            .handles
            .iter()
            .filter_map(|(handle, record)| (record.expires_at <= now).then_some(handle.clone()))
            .collect::<Vec<_>>();
        for handle in expired {
            if let Some(record) = state.handles.remove(&handle) {
                let forget_after = now.checked_add(self.handle_ttl).unwrap_or(now);
                insert_handle_tombstone(
                    state,
                    handle,
                    HandleTerminalState::Expired,
                    forget_after.max(record.expires_at),
                );
            }
        }
    }
}

fn revoke_matching_handles(
    state: &mut BrokerState,
    mut should_revoke: impl FnMut(&HandleRecord) -> bool,
) {
    let handles = state
        .handles
        .iter()
        .filter_map(|(handle, record)| should_revoke(record).then_some(handle.clone()))
        .collect::<Vec<_>>();
    for handle in handles {
        if let Some(record) = state.handles.remove(&handle) {
            insert_handle_tombstone(
                state,
                handle,
                HandleTerminalState::Revoked,
                record.expires_at,
            );
        }
    }
}

fn insert_handle_tombstone(
    state: &mut BrokerState,
    handle: String,
    terminal: HandleTerminalState,
    forget_after: SystemTime,
) {
    if state.handle_tombstones.len() >= MAX_HANDLE_TOMBSTONES
        && !state.handle_tombstones.contains_key(&handle)
    {
        if let Some(oldest) = state
            .handle_tombstones
            .iter()
            .min_by_key(|(_, tombstone)| tombstone.forget_after)
            .map(|(handle, _)| handle.clone())
        {
            state.handle_tombstones.remove(&oldest);
        }
    }
    state.handle_tombstones.insert(
        handle,
        HandleTombstone {
            state: terminal,
            forget_after,
        },
    );
}

pub struct BrokeredRelease {
    cancellation: ProviderCancellation,
    plan: Option<ReleasePlan>,
    target: VerifiedBrowserSecretTarget,
    permit: SecretReleasePermit,
}

impl fmt::Debug for BrokeredRelease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrokeredRelease")
            .field("plan", &self.plan.as_ref().map(ReleasePlan::kind))
            .field("target", &"[bound target]")
            .finish()
    }
}

impl BrokeredRelease {
    pub fn plan_kind(&self) -> Option<ReleasePlanKind> {
        self.plan.as_ref().map(ReleasePlan::kind)
    }

    pub fn target(&self) -> &VerifiedBrowserSecretTarget {
        &self.target
    }

    pub fn take_runtime_lease(&mut self) -> Result<SecretLease, BrokeredReleaseError> {
        self.ensure_plan_live()?;
        match self.plan.take() {
            Some(ReleasePlan::RuntimeDelivers(lease)) => Ok(lease),
            Some(other) => {
                self.plan = Some(other);
                Err(BrokeredReleaseError::WrongPlan)
            }
            None => Err(BrokeredReleaseError::AlreadyTaken),
        }
    }

    pub fn take_provider_fill_session(
        &mut self,
    ) -> Result<ProviderFillSession, BrokeredReleaseError> {
        self.ensure_plan_live()?;
        match self.plan.take() {
            Some(ReleasePlan::ProviderFills(session)) => Ok(session),
            Some(other) => {
                self.plan = Some(other);
                Err(BrokeredReleaseError::WrongPlan)
            }
            None => Err(BrokeredReleaseError::AlreadyTaken),
        }
    }

    pub fn take_user_presence_handle(
        &mut self,
    ) -> Result<UserPresenceHandle, BrokeredReleaseError> {
        self.ensure_plan_live()?;
        match self.plan.take() {
            Some(ReleasePlan::NeedsUserPresence(handle)) => Ok(handle),
            Some(other) => {
                self.plan = Some(other);
                Err(BrokeredReleaseError::WrongPlan)
            }
            None => Err(BrokeredReleaseError::AlreadyTaken),
        }
    }

    fn ensure_plan_live(&mut self) -> Result<(), BrokeredReleaseError> {
        if self.cancellation.is_revoked() {
            self.plan.take();
            return Err(BrokeredReleaseError::Revoked);
        }
        if self.permit.ensure_live().is_err() {
            self.plan.take();
            return Err(BrokeredReleaseError::AuthorizationExpired);
        }
        Ok(())
    }

    pub fn complete(mut self) {
        self.cancellation.disarm();
    }

    pub async fn cancel(mut self) -> Result<(), CredentialProviderError> {
        self.plan.take();
        self.cancellation.cancel().await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BrokeredReleaseError {
    #[error("credential release was revoked")]
    Revoked,
    #[error("credential release does not contain a runtime lease")]
    WrongPlan,
    #[error("credential release plan was already taken")]
    AlreadyTaken,
    #[error("credential release authorization expired")]
    AuthorizationExpired,
}

struct ProviderCancellation {
    provider: Arc<dyn CredentialProvider>,
    release_id: ProviderReleaseId,
    state: Weak<Mutex<BrokerState>>,
    revoked: Arc<AtomicBool>,
    runtime: Option<tokio::runtime::Handle>,
    armed: bool,
}

impl ProviderCancellation {
    fn new(
        provider: Arc<dyn CredentialProvider>,
        release_id: ProviderReleaseId,
        state: Weak<Mutex<BrokerState>>,
        revoked: Arc<AtomicBool>,
    ) -> Self {
        Self {
            provider,
            release_id,
            state,
            revoked,
            runtime: tokio::runtime::Handle::try_current().ok(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.remove_active();
    }

    fn is_revoked(&self) -> bool {
        self.revoked.load(AtomicOrdering::Acquire)
    }

    async fn cancel(&mut self) -> Result<(), CredentialProviderError> {
        self.armed = false;
        self.revoked.store(true, AtomicOrdering::Release);
        self.remove_active();
        self.provider.cancel(self.release_id).await
    }

    fn remove_active(&self) {
        if let Some(state) = self.state.upgrade() {
            state
                .lock()
                .unwrap()
                .active_releases
                .remove(&self.release_id);
        }
    }
}

impl Drop for ProviderCancellation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        self.revoked.store(true, AtomicOrdering::Release);
        self.remove_active();
        let provider = self.provider.clone();
        let release_id = self.release_id;
        let runtime = self
            .runtime
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok());
        if let Some(runtime) = runtime {
            runtime.spawn(async move {
                let _ = provider.cancel(release_id).await;
            });
        }
    }
}

fn take_active_releases(
    state: &mut BrokerState,
    mut should_cancel: impl FnMut(&ActiveRelease) -> bool,
) -> Vec<PendingProviderCancellation> {
    let release_ids = state
        .active_releases
        .iter()
        .filter_map(|(release_id, release)| should_cancel(release).then_some(*release_id))
        .collect::<Vec<_>>();
    release_ids
        .into_iter()
        .filter_map(|release_id| {
            state.active_releases.remove(&release_id).map(|release| {
                release.revoked.store(true, AtomicOrdering::Release);
                PendingProviderCancellation {
                    provider: release.provider,
                    release_id,
                    runtime: release.runtime,
                }
            })
        })
        .collect()
}

struct PendingProviderCancellation {
    provider: Arc<dyn CredentialProvider>,
    release_id: ProviderReleaseId,
    runtime: Option<tokio::runtime::Handle>,
}

fn spawn_provider_cancellations(cancellations: Vec<PendingProviderCancellation>) {
    for cancellation in cancellations {
        let runtime = cancellation
            .runtime
            .or_else(|| tokio::runtime::Handle::try_current().ok());
        let Some(runtime) = runtime else {
            continue;
        };
        runtime.spawn(async move {
            let _ = cancellation.provider.cancel(cancellation.release_id).await;
        });
    }
}

fn validate_identifier(value: &str, maximum: usize) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > maximum
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(());
    }
    Ok(())
}

fn validate_runtime_scope(value: &str) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(());
    }
    Ok(())
}

fn validate_binding_spec(
    runtime_scope: &str,
    spec: &CredentialBindingSpec,
) -> Result<(), CredentialConfigurationError> {
    if !spec
        .session
        .starts_with(&format!("__cua_runtime_{runtime_scope}:"))
    {
        return Err(CredentialConfigurationError::RuntimeMismatch);
    }
    if spec.session.ends_with(':') || spec.session.chars().any(char::is_control) {
        return Err(CredentialConfigurationError::InvalidSession);
    }
    if spec.private_fields.is_empty() {
        return Err(CredentialConfigurationError::MissingPrivateField);
    }
    let mut fields = HashSet::new();
    if spec
        .private_fields
        .iter()
        .any(|field| !fields.insert(field.field))
    {
        return Err(CredentialConfigurationError::DuplicatePrivateField);
    }
    if spec.allowed_targets.is_empty() {
        return Err(CredentialConfigurationError::MissingTargetConstraint);
    }
    if spec.max_releases == Some(0) {
        return Err(CredentialConfigurationError::InvalidReleaseLimit);
    }
    Ok(())
}

fn target_matches(
    constraint: &SecretTargetConstraint,
    target: &VerifiedBrowserSecretTarget,
) -> bool {
    match constraint {
        SecretTargetConstraint::Browser(browser) => {
            browser.origin == *target.origin() && browser.secure_field == target.secure_field()
        }
    }
}

fn secret_release_resource(
    handle: &CredentialHandle,
    record: &HandleRecord,
    binding: &BindingRecord,
    field: CredentialField,
    target: &VerifiedBrowserSecretTarget,
) -> SecretReleaseResource {
    let mut handle_digest = Sha256::new();
    digest_field(&mut handle_digest, b"cua-credential-handle-v1");
    digest_field(&mut handle_digest, handle.as_str().as_bytes());
    SecretReleaseResource {
        handle_digest: handle_digest.finalize().into(),
        binding_digest: record.binding_digest,
        authorization: binding.spec.authorization.clone(),
        provider: record.provider.clone(),
        provider_class: binding.provider_class,
        field,
        target: target.clone(),
    }
}

fn secret_release_authorization_digest(
    resource: &SecretReleaseResource,
    context: &crate::session_authorization::EffectiveAuthorizationContext,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest_field(&mut digest, b"cua-secret-release-authorization-v1");
    digest_field(&mut digest, &resource.handle_digest);
    digest_field(&mut digest, &resource.binding_digest.0);
    digest_field(&mut digest, resource.authorization.as_str().as_bytes());
    digest_field(&mut digest, resource.provider.as_str().as_bytes());
    digest_field(&mut digest, resource.provider_class.as_str().as_bytes());
    digest_field(&mut digest, resource.field.digest_tag());
    digest_browser_secret_target(&mut digest, &resource.target);
    digest_field(&mut digest, context.mode().as_str().as_bytes());
    digest_field(
        &mut digest,
        context.public_session().unwrap_or("").as_bytes(),
    );
    digest_field(
        &mut digest,
        context.transport_session().unwrap_or("").as_bytes(),
    );
    digest_field(
        &mut digest,
        context
            .capability_manifest_sha256()
            .unwrap_or("")
            .as_bytes(),
    );
    digest_field(
        &mut digest,
        context.managed_policy_sha256().unwrap_or("").as_bytes(),
    );
    digest_field(
        &mut digest,
        context.user_policy_sha256().unwrap_or("").as_bytes(),
    );
    digest.finalize().into()
}

fn digest_browser_secret_target(digest: &mut Sha256, target: &VerifiedBrowserSecretTarget) {
    digest_field(digest, target.runtime_scope.as_bytes());
    digest_field(digest, target.session.as_bytes());
    digest_field(digest, &target.process.pid.to_be_bytes());
    digest_field(
        digest,
        &target.process.start_time.unwrap_or(u64::MAX).to_be_bytes(),
    );
    digest_field(
        digest,
        target
            .process
            .executable
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
    );
    digest_field(digest, &target.endpoint_generation.to_be_bytes());
    digest_field(digest, target.target_id.as_bytes());
    digest_field(digest, target.tab_id.as_bytes());
    digest_field(
        digest,
        match target.frame.kind {
            BrowserSecretFrameKind::Main => b"main",
            BrowserSecretFrameKind::Iframe => b"iframe",
            BrowserSecretFrameKind::Oopif => b"oopif",
        },
    );
    digest_field(digest, target.frame.frame_id.as_bytes());
    digest_field(digest, target.frame.loader_id.as_bytes());
    digest_field(digest, target.origin.as_str().as_bytes());
    digest_field(digest, target.semantic_ref.as_bytes());
    digest_field(digest, target.secure_field.as_str().as_bytes());
}

fn binding_digest(runtime_scope: &str, spec: &CredentialBindingSpec) -> BindingDigest {
    let mut digest = Sha256::new();
    digest_field(&mut digest, b"cua-credential-binding-v1");
    digest_field(&mut digest, runtime_scope.as_bytes());
    digest_field(&mut digest, spec.session.as_bytes());
    digest_field(&mut digest, spec.provider.as_str().as_bytes());
    for private_field in &spec.private_fields {
        digest_field(&mut digest, private_field.field.digest_tag());
        digest_field(&mut digest, private_field.locator.as_str().as_bytes());
    }
    digest_field(
        &mut digest,
        spec.safe_label
            .as_ref()
            .map(SafeCredentialLabel::as_str)
            .unwrap_or("")
            .as_bytes(),
    );
    for target in &spec.allowed_targets {
        match target {
            SecretTargetConstraint::Browser(browser) => {
                digest_field(&mut digest, b"browser");
                digest_field(&mut digest, browser.origin.as_str().as_bytes());
                digest_field(&mut digest, browser.secure_field.as_str().as_bytes());
            }
        }
    }
    let expires = spec
        .expires_at
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis())
        .unwrap_or(u128::MAX);
    digest_field(&mut digest, &expires.to_be_bytes());
    digest_field(
        &mut digest,
        &spec.max_releases.unwrap_or(u32::MAX).to_be_bytes(),
    );
    digest_field(&mut digest, spec.authorization.as_str().as_bytes());
    BindingDigest(digest.finalize().into())
}

fn digest_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Notify;

    use super::*;

    enum FakeOutcome {
        Runtime(Vec<u8>),
        ProviderFill,
        UserPresence,
        Error(CredentialProviderErrorKind),
        Wait {
            entered: Arc<Notify>,
            resume: Arc<Notify>,
            value: Vec<u8>,
        },
    }

    struct FakeProvider {
        id: CredentialProviderId,
        outcomes: Mutex<VecDeque<FakeOutcome>>,
        releases: AtomicUsize,
        effects: AtomicUsize,
        cancellations: AtomicUsize,
        cancelled_releases: Mutex<HashSet<ProviderReleaseId>>,
    }

    impl FakeProvider {
        fn new(outcomes: impl IntoIterator<Item = FakeOutcome>) -> Arc<Self> {
            Arc::new(Self {
                id: CredentialProviderId::new("fake-provider").unwrap(),
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                releases: AtomicUsize::new(0),
                effects: AtomicUsize::new(0),
                cancellations: AtomicUsize::new(0),
                cancelled_releases: Mutex::new(HashSet::new()),
            })
        }

        fn before_effect(
            &self,
            request: &ProviderReleaseRequest,
        ) -> Result<(), CredentialProviderError> {
            request.ensure_not_cancelled()?;
            if self
                .cancelled_releases
                .lock()
                .unwrap()
                .contains(&request.release_id())
            {
                return Err(CredentialProviderError::new(
                    CredentialProviderErrorKind::Cancelled,
                ));
            }
            self.effects.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait]
    impl CredentialProvider for FakeProvider {
        fn id(&self) -> &CredentialProviderId {
            &self.id
        }

        fn class(&self) -> CredentialProviderClass {
            CredentialProviderClass::Fake
        }

        async fn release(
            &self,
            request: &ProviderReleaseRequest,
        ) -> Result<ReleasePlan, CredentialProviderError> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            request.ensure_not_cancelled()?;
            let outcome = self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(FakeOutcome::Error(CredentialProviderErrorKind::NotFound));
            match outcome {
                FakeOutcome::Runtime(value) => {
                    self.before_effect(request)?;
                    SecretLease::from_utf8_bytes(value)
                        .map(ReleasePlan::RuntimeDelivers)
                        .map_err(|error| match error {
                            SecretLeaseError::Empty => CredentialProviderError::new(
                                CredentialProviderErrorKind::MalformedResponse,
                            ),
                            SecretLeaseError::TooLarge => CredentialProviderError::new(
                                CredentialProviderErrorKind::OversizedSecret,
                            ),
                            SecretLeaseError::InvalidUtf8 => CredentialProviderError::new(
                                CredentialProviderErrorKind::InvalidUtf8,
                            ),
                        })
                }
                FakeOutcome::ProviderFill => {
                    self.before_effect(request)?;
                    Ok(ReleasePlan::ProviderFills(
                        ProviderFillSession::from_opaque_bytes(b"fill-session".to_vec()).unwrap(),
                    ))
                }
                FakeOutcome::UserPresence => {
                    self.before_effect(request)?;
                    Ok(ReleasePlan::NeedsUserPresence(
                        UserPresenceHandle::from_opaque_bytes(b"presence-session".to_vec())
                            .unwrap(),
                    ))
                }
                FakeOutcome::Error(kind) => Err(CredentialProviderError::new(kind)),
                FakeOutcome::Wait {
                    entered,
                    resume,
                    value,
                } => {
                    entered.notify_waiters();
                    resume.notified().await;
                    self.before_effect(request)?;
                    SecretLease::from_utf8_bytes(value)
                        .map(ReleasePlan::RuntimeDelivers)
                        .map_err(|_| {
                            CredentialProviderError::new(
                                CredentialProviderErrorKind::MalformedResponse,
                            )
                        })
                }
            }
        }

        async fn cancel(&self, release: ProviderReleaseId) -> Result<(), CredentialProviderError> {
            if self.cancelled_releases.lock().unwrap().insert(release) {
                self.cancellations.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }

        async fn health(&self) -> ProviderHealth {
            ProviderHealth {
                state: ProviderHealthState::Available,
            }
        }
    }

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn target(
        runtime: &str,
        session: &str,
        origin: &str,
        reference: &str,
    ) -> VerifiedBrowserSecretTarget {
        VerifiedBrowserSecretTarget::new(
            runtime.to_owned(),
            session.to_owned(),
            ProcessFingerprint {
                pid: 42,
                start_time: Some(9),
                executable: Some("/synthetic/browser".to_owned()),
            },
            3,
            "bt-synthetic".to_owned(),
            "tab-synthetic".to_owned(),
            BrowserSecretFrameIdentity {
                kind: BrowserSecretFrameKind::Main,
                frame_id: "frame-main".to_owned(),
                loader_id: "loader-1".to_owned(),
            },
            origin,
            reference.to_owned(),
            SecureFieldKind::Password,
        )
        .unwrap()
    }

    fn binding_spec(session: &str, authorization: &str) -> CredentialBindingSpec {
        CredentialBindingSpec {
            session: session.to_owned(),
            provider: CredentialProviderId::new("fake-provider").unwrap(),
            private_fields: vec![PrivateCredentialField {
                field: CredentialField::Password,
                locator: PrivateCredentialFieldLocator::new("provider://automation/item/password")
                    .unwrap(),
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
            authorization: SecretReleaseAuthorization::trusted_scope(authorization).unwrap(),
        }
    }

    fn broker_with(
        provider: Arc<FakeProvider>,
        max_releases: Option<u32>,
    ) -> (
        Arc<SecretBroker>,
        BindingRegistrationId,
        String,
        Arc<crate::session_authorization::EffectiveAuthorizationContext>,
    ) {
        static NEXT_SESSION: AtomicUsize = AtomicUsize::new(1);
        let ceiling = crate::session_authorization::SessionModeCeiling::for_trusted_sessions(
            [crate::authorization::PermissionMode::Unrestricted],
            true,
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap();
        let context =
            crate::session_authorization::SessionAuthorizationRegistry::with_ceiling(ceiling)
                .compatibility_context(crate::authorization::PermissionMode::Unrestricted, None)
                .unwrap();
        let mut builder = SecretBrokerBuilder::new(context.runtime_scope_key()).unwrap();
        let public_session = format!("agent-{}", NEXT_SESSION.fetch_add(1, Ordering::Relaxed));
        let session = builder.runtime_session_key(&public_session).unwrap();
        builder.add_provider(provider).unwrap();
        let mut spec = binding_spec(&session, "login-recovery");
        spec.max_releases = max_releases;
        let registration = builder.add_binding(spec).unwrap();
        (builder.build().unwrap(), registration, session, context)
    }

    fn permit(
        broker: &SecretBroker,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        handle: &CredentialHandle,
        field: CredentialField,
        target: &VerifiedBrowserSecretTarget,
        at: SystemTime,
    ) -> SecretReleasePermit {
        let resource = broker
            .prepare_secret_release(handle, field, target, at)
            .unwrap();
        crate::authorization::authorize_secret_release(context, &resource).unwrap()
    }

    #[test]
    fn checked_targets_require_runtime_document_and_semantic_ref_proof() {
        let session = "__cua_runtime_runtime-a:agent-a";
        let valid = target(
            "runtime-a",
            session,
            "https://accounts.example.test/login",
            "p7:2",
        );
        assert_eq!(valid.origin().as_str(), "https://accounts.example.test");
        assert!(VerifiedBrowserSecretTarget::new(
            "runtime-a".to_owned(),
            session.to_owned(),
            ProcessFingerprint {
                pid: 42,
                start_time: None,
                executable: None,
            },
            3,
            "bt-synthetic".to_owned(),
            "tab-synthetic".to_owned(),
            BrowserSecretFrameIdentity {
                kind: BrowserSecretFrameKind::Main,
                frame_id: "frame-main".to_owned(),
                loader_id: "loader-1".to_owned(),
            },
            "https://accounts.example.test/login",
            "legacy-ref".to_owned(),
            SecureFieldKind::Password,
        )
        .is_err());
    }

    #[test]
    fn builder_is_immutable_after_build_and_rejects_cross_runtime_bindings() {
        let provider = FakeProvider::new([]);
        let mut builder = SecretBrokerBuilder::new("runtime-a").unwrap();
        builder.add_provider(provider.clone()).unwrap();
        assert_eq!(
            builder.add_provider(provider).unwrap_err(),
            CredentialConfigurationError::DuplicateProvider
        );
        let error = builder
            .add_binding(CredentialBindingSpec {
                session: "__cua_runtime_runtime-b:agent-a".to_owned(),
                provider: CredentialProviderId::new("fake-provider").unwrap(),
                private_fields: vec![PrivateCredentialField {
                    field: CredentialField::Password,
                    locator: PrivateCredentialFieldLocator::new("private-locator").unwrap(),
                }],
                safe_label: None,
                allowed_targets: vec![SecretTargetConstraint::Browser(
                    BrowserSecretTargetConstraint::exact_password_origin(
                        "https://accounts.example.test",
                    )
                    .unwrap(),
                )],
                expires_at: None,
                max_releases: Some(1),
                authorization: SecretReleaseAuthorization::trusted_scope("test").unwrap(),
            })
            .unwrap_err();
        assert_eq!(error, CredentialConfigurationError::RuntimeMismatch);
    }

    #[test]
    fn runtime_scope_encoding_is_unambiguous_and_ended_sessions_are_rejected() {
        assert!(matches!(
            SecretBrokerBuilder::new("runtime:ambiguous"),
            Err(CredentialConfigurationError::InvalidRuntimeScope)
        ));

        let runtime = format!("runtime-ended-{}", Uuid::new_v4().simple());
        let mut builder = SecretBrokerBuilder::new(&runtime).unwrap();
        let provider = FakeProvider::new([]);
        builder.add_provider(provider.clone()).unwrap();
        let ended_before_add = builder.runtime_session_key("ended-before-add").unwrap();
        crate::session::fire_session_end(&ended_before_add);
        assert_eq!(
            builder
                .add_binding(binding_spec(&ended_before_add, "ended-before-add"))
                .unwrap_err(),
            CredentialConfigurationError::SessionEnded
        );

        let ended_before_build = builder.runtime_session_key("ended-before-build").unwrap();
        builder
            .add_binding(binding_spec(&ended_before_build, "ended-before-build"))
            .unwrap();
        crate::session::fire_session_end(&ended_before_build);
        assert!(matches!(
            builder.build(),
            Err(CredentialConfigurationError::SessionEnded)
        ));
    }

    #[test]
    fn target_debug_omits_origin_endpoint_generation_and_private_identity() {
        let value = target(
            "runtime-safe",
            "__cua_runtime_runtime-safe:agent",
            "https://accounts.example.test/login",
            "p99:1",
        );
        let debug = format!("{value:?}");
        assert!(!debug.contains("accounts.example.test"));
        assert!(!debug.contains("endpoint_generation: 3"));
        assert!(!debug.contains("p99:1"));
        assert!(!debug.contains("tab-synthetic"));
    }

    #[test]
    fn secret_release_authorization_is_distinct_across_permission_modes() {
        let load_manifest = |source: &str| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("manifest.yaml");
            std::fs::write(&path, source).unwrap();
            Arc::new(crate::session_manifest::load_manifest(&path).unwrap())
        };
        let exact_manifest = load_manifest(
            r#"
version: 3
expires_after: 1h
idle_timeout: 5m
resources:
  credentials:
    secret_release:
      - authorization: login-recovery
        origins: [https://accounts.example.test]
allow:
  tools: [secret_release]
"#,
        );
        let generic_input_manifest = load_manifest(
            r#"
version: 3
expires_after: 1h
idle_timeout: 5m
resources:
  browser:
    origins: [https://accounts.example.test]
allow:
  tools: [browser_type]
"#,
        );
        let ceiling = crate::session_authorization::SessionModeCeiling::for_trusted_sessions(
            [
                crate::authorization::PermissionMode::Standard,
                crate::authorization::PermissionMode::Bounded,
                crate::authorization::PermissionMode::Unrestricted,
            ],
            true,
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap();
        let registry =
            crate::session_authorization::SessionAuthorizationRegistry::with_ceiling(ceiling);
        let standard = registry
            .compatibility_context(
                crate::authorization::PermissionMode::Standard,
                Some(exact_manifest.clone()),
            )
            .unwrap();
        let bounded = registry
            .compatibility_context(
                crate::authorization::PermissionMode::Bounded,
                Some(exact_manifest),
            )
            .unwrap();
        let bounded_generic = registry
            .compatibility_context(
                crate::authorization::PermissionMode::Bounded,
                Some(generic_input_manifest.clone()),
            )
            .unwrap();
        let unrestricted = registry
            .compatibility_context(crate::authorization::PermissionMode::Unrestricted, None)
            .unwrap();
        let unrestricted_narrowed = registry
            .compatibility_context(
                crate::authorization::PermissionMode::Unrestricted,
                Some(generic_input_manifest),
            )
            .unwrap();

        let provider = FakeProvider::new([]);
        let mut builder = SecretBrokerBuilder::new(standard.runtime_scope_key()).unwrap();
        let session = builder.runtime_session_key("authorization-matrix").unwrap();
        builder.add_provider(provider).unwrap();
        builder
            .add_binding(CredentialBindingSpec {
                session: session.clone(),
                provider: CredentialProviderId::new("fake-provider").unwrap(),
                private_fields: vec![PrivateCredentialField {
                    field: CredentialField::Password,
                    locator: PrivateCredentialFieldLocator::new("private-locator").unwrap(),
                }],
                safe_label: None,
                allowed_targets: vec![SecretTargetConstraint::Browser(
                    BrowserSecretTargetConstraint::exact_password_origin(
                        "https://accounts.example.test",
                    )
                    .unwrap(),
                )],
                expires_at: None,
                max_releases: Some(1),
                authorization: SecretReleaseAuthorization::trusted_scope("login-recovery").unwrap(),
            })
            .unwrap();
        let broker = builder.build().unwrap();
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p11:1",
        );
        let descriptor = broker.find_credentials(&target, now()).unwrap().remove(0);
        let resource = broker
            .prepare_secret_release(
                &descriptor.handle,
                CredentialField::Password,
                &target,
                now(),
            )
            .unwrap();

        assert_eq!(
            crate::authorization::authorize_secret_release(&standard, &resource)
                .err()
                .unwrap(),
            SecretReleaseAuthorizationError::StandardDenied
        );
        assert_eq!(
            crate::authorization::authorize_secret_release(&bounded_generic, &resource)
                .err()
                .unwrap(),
            SecretReleaseAuthorizationError::ManifestDenied
        );
        assert_eq!(
            crate::authorization::authorize_secret_release(&unrestricted_narrowed, &resource)
                .err()
                .unwrap(),
            SecretReleaseAuthorizationError::ManifestDenied
        );
        assert!(crate::authorization::authorize_secret_release(&bounded, &resource).is_ok());
        assert!(crate::authorization::authorize_secret_release(&unrestricted, &resource).is_ok());
    }

    #[test]
    fn discovery_is_target_first_safe_and_mints_fresh_handles() {
        let provider = FakeProvider::new([]);
        let (broker, _, session, _) = broker_with(provider.clone(), Some(1));
        let wrong = target(
            broker.runtime_scope(),
            &session,
            "https://other.example.test/login",
            "p1:1",
        );
        assert!(broker.find_credentials(&wrong, now()).unwrap().is_empty());
        assert_eq!(provider.releases.load(Ordering::SeqCst), 0);

        let expected = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p2:1",
        );
        let first = broker.find_credentials(&expected, now()).unwrap();
        let second = broker.find_credentials(&expected, now()).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].label.as_ref().unwrap().as_str(), "Synthetic login");
        assert_eq!(first[0].fields, vec![CredentialField::Password]);
        assert_eq!(first[0].provider_class, Some(CredentialProviderClass::Fake));
        assert_ne!(first[0].handle.as_str(), second[0].handle.as_str());
        assert!(!format!("{:?}", first[0]).contains(first[0].handle.as_str()));
        assert_eq!(provider.releases.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn handle_capacity_is_atomic_per_session_and_expiry_frees_it() {
        let runtime = format!("runtime-capacity-{}", Uuid::new_v4().simple());
        let provider = FakeProvider::new([]);
        let mut builder = SecretBrokerBuilder::new(&runtime).unwrap();
        let session_a = builder.runtime_session_key("capacity-a").unwrap();
        let session_b = builder.runtime_session_key("capacity-b").unwrap();
        builder.add_provider(provider).unwrap();
        let mut first_registration = None;
        for index in 0..MAX_ACTIVE_HANDLES_PER_SESSION {
            let registration = builder
                .add_binding(binding_spec(&session_a, &format!("capacity-a-{index}")))
                .unwrap();
            first_registration.get_or_insert(registration);
        }
        builder
            .add_binding(binding_spec(&session_b, "capacity-b"))
            .unwrap();
        let broker = builder.build().unwrap();
        let target_a = target(
            broker.runtime_scope(),
            &session_a,
            "https://accounts.example.test/login",
            "p100:1",
        );
        let target_b = target(
            broker.runtime_scope(),
            &session_b,
            "https://accounts.example.test/login",
            "p100:2",
        );

        assert_eq!(
            broker.find_credentials(&target_a, now()).unwrap().len(),
            MAX_ACTIVE_HANDLES_PER_SESSION
        );
        assert_eq!(
            broker.find_credentials(&target_a, now()).unwrap_err(),
            SecretBrokerError::Capacity
        );
        assert_eq!(
            broker.state.lock().unwrap().handles.len(),
            MAX_ACTIVE_HANDLES_PER_SESSION,
            "a failed discovery must not mint a partial batch"
        );
        assert_eq!(broker.find_credentials(&target_b, now()).unwrap().len(), 1);

        assert!(broker.revoke_binding(first_registration.unwrap()));
        assert_eq!(
            broker
                .state
                .lock()
                .unwrap()
                .handles
                .values()
                .filter(|handle| handle.target.session() == session_a)
                .count(),
            MAX_ACTIVE_HANDLES_PER_SESSION - 1
        );

        let after_expiry = now() + DEFAULT_CREDENTIAL_HANDLE_TTL;
        assert_eq!(
            broker
                .find_credentials(&target_a, after_expiry)
                .unwrap()
                .len(),
            MAX_ACTIVE_HANDLES_PER_SESSION - 1
        );
    }

    #[test]
    fn handles_cannot_cross_brokers_even_with_the_same_runtime_and_target() {
        let runtime = format!("runtime-cross-broker-{}", Uuid::new_v4().simple());
        let provider = FakeProvider::new([]);
        let build = || {
            let mut builder = SecretBrokerBuilder::new(&runtime).unwrap();
            let session = builder.runtime_session_key("same-session").unwrap();
            builder.add_provider(provider.clone()).unwrap();
            builder
                .add_binding(binding_spec(&session, "same-authorization"))
                .unwrap();
            (builder.build().unwrap(), session)
        };
        let (first, session) = build();
        let (second, _) = build();
        let target = target(
            &runtime,
            &session,
            "https://accounts.example.test/login",
            "p101:1",
        );
        let descriptor = first.find_credentials(&target, now()).unwrap().remove(0);

        assert_eq!(
            second
                .prepare_secret_release(
                    &descriptor.handle,
                    CredentialField::Password,
                    &target,
                    now(),
                )
                .unwrap_err(),
            SecretBrokerError::InvalidHandle
        );
        assert_eq!(provider.releases.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn handle_is_exact_target_bound_and_runtime_lease_is_single_use() {
        let provider = FakeProvider::new([FakeOutcome::Runtime(
            b"CREDENTIAL_CANARY_NOT_PUBLIC".to_vec(),
        )]);
        let (broker, _, session, context) = broker_with(provider.clone(), Some(2));
        let expected = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p3:1",
        );
        let other_ref = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p3:2",
        );
        let descriptor = broker.find_credentials(&expected, now()).unwrap().remove(0);
        let mismatch_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &expected,
            now(),
        );
        assert_eq!(
            broker
                .release(
                    &descriptor.handle,
                    CredentialField::Password,
                    &other_ref,
                    now(),
                    mismatch_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::TargetMismatch
        );
        assert_eq!(provider.releases.load(Ordering::SeqCst), 0);

        let success_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &expected,
            now(),
        );
        let replay_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &expected,
            now(),
        );
        let mut release = broker
            .release(
                &descriptor.handle,
                CredentialField::Password,
                &expected,
                now(),
                success_permit,
            )
            .await
            .unwrap();
        assert_eq!(release.plan_kind(), Some(ReleasePlanKind::RuntimeDelivers));
        let lease = release.take_runtime_lease().expect("runtime lease");
        assert_eq!(release.plan_kind(), None);
        assert_eq!(lease.expose_for_delivery(), "CREDENTIAL_CANARY_NOT_PUBLIC");
        release.complete();
        drop(lease);
        assert_eq!(
            broker
                .release(
                    &descriptor.handle,
                    CredentialField::Password,
                    &expected,
                    now(),
                    replay_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::HandleConsumed
        );
        assert_eq!(provider.releases.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn release_limit_is_reserved_before_provider_await() {
        let entered = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let provider = FakeProvider::new([FakeOutcome::Wait {
            entered: entered.clone(),
            resume: resume.clone(),
            value: b"synthetic-secret".to_vec(),
        }]);
        let (broker, _, session, context) = broker_with(provider.clone(), Some(1));
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p4:1",
        );
        let first = broker.find_credentials(&target, now()).unwrap().remove(0);
        let second = broker.find_credentials(&target, now()).unwrap().remove(0);
        let first_permit = permit(
            &broker,
            &context,
            &first.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let second_permit = permit(
            &broker,
            &context,
            &second.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let broker_for_task = broker.clone();
        let target_for_task = target.clone();
        let first_handle = first.handle.clone();
        let pending = tokio::spawn(async move {
            broker_for_task
                .release(
                    &first_handle,
                    CredentialField::Password,
                    &target_for_task,
                    now(),
                    first_permit,
                )
                .await
        });
        entered.notified().await;
        assert_eq!(
            broker
                .release(
                    &second.handle,
                    CredentialField::Password,
                    &target,
                    now(),
                    second_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::ReleaseLimitReached
        );
        assert_eq!(provider.releases.load(Ordering::SeqCst), 1);
        resume.notify_waiters();
        pending.await.unwrap().unwrap().complete();
    }

    #[tokio::test]
    async fn provider_failure_consumes_handle_and_returns_fixed_error() {
        let provider = FakeProvider::new([FakeOutcome::Error(
            CredentialProviderErrorKind::ProviderDied,
        )]);
        let (broker, _, session, context) = broker_with(provider.clone(), Some(2));
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p5:1",
        );
        let descriptor = broker.find_credentials(&target, now()).unwrap().remove(0);
        let failure_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let replay_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        assert_eq!(
            broker
                .release(
                    &descriptor.handle,
                    CredentialField::Password,
                    &target,
                    now(),
                    failure_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::Provider(CredentialProviderErrorKind::ProviderDied)
        );
        assert_eq!(
            broker
                .release(
                    &descriptor.handle,
                    CredentialField::Password,
                    &target,
                    now(),
                    replay_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::HandleConsumed
        );
    }

    #[tokio::test]
    async fn cancelling_provider_resolution_calls_provider_cancel() {
        let entered = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let provider = FakeProvider::new([FakeOutcome::Wait {
            entered: entered.clone(),
            resume,
            value: b"synthetic-secret".to_vec(),
        }]);
        let (broker, _, session, context) = broker_with(provider.clone(), Some(2));
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p6:1",
        );
        let descriptor = broker.find_credentials(&target, now()).unwrap().remove(0);
        let pending_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let replay_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let broker_for_task = broker.clone();
        let target_for_task = target.clone();
        let handle = descriptor.handle.clone();
        let pending = tokio::spawn(async move {
            broker_for_task
                .release(
                    &handle,
                    CredentialField::Password,
                    &target_for_task,
                    now(),
                    pending_permit,
                )
                .await
        });
        entered.notified().await;
        pending.abort();
        let _ = pending.await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while provider.cancellations.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("provider cancellation");
        assert_eq!(provider.cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(
            broker
                .release(
                    &descriptor.handle,
                    CredentialField::Password,
                    &target,
                    now(),
                    replay_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::HandleConsumed
        );
    }

    #[tokio::test]
    async fn dropping_or_revoking_a_returned_release_cancels_exactly_once() {
        let dropped_provider =
            FakeProvider::new([FakeOutcome::Runtime(b"drop-cancellation-canary".to_vec())]);
        let (dropped_broker, _, dropped_session, dropped_context) =
            broker_with(dropped_provider.clone(), Some(1));
        let dropped_target = target(
            dropped_broker.runtime_scope(),
            &dropped_session,
            "https://accounts.example.test/login",
            "p102:1",
        );
        let dropped_descriptor = dropped_broker
            .find_credentials(&dropped_target, now())
            .unwrap()
            .remove(0);
        let dropped_permit = permit(
            &dropped_broker,
            &dropped_context,
            &dropped_descriptor.handle,
            CredentialField::Password,
            &dropped_target,
            now(),
        );
        let dropped_release = dropped_broker
            .release(
                &dropped_descriptor.handle,
                CredentialField::Password,
                &dropped_target,
                now(),
                dropped_permit,
            )
            .await
            .unwrap();
        drop(dropped_release);
        tokio::time::timeout(Duration::from_secs(1), async {
            while dropped_provider.cancellations.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("drop cancellation");
        assert_eq!(dropped_provider.cancellations.load(Ordering::SeqCst), 1);

        let revoked_provider = FakeProvider::new([FakeOutcome::Runtime(
            b"post-return-revocation-canary".to_vec(),
        )]);
        let (revoked_broker, registration, revoked_session, revoked_context) =
            broker_with(revoked_provider.clone(), Some(1));
        let revoked_target = target(
            revoked_broker.runtime_scope(),
            &revoked_session,
            "https://accounts.example.test/login",
            "p103:1",
        );
        let revoked_descriptor = revoked_broker
            .find_credentials(&revoked_target, now())
            .unwrap()
            .remove(0);
        let revoked_permit = permit(
            &revoked_broker,
            &revoked_context,
            &revoked_descriptor.handle,
            CredentialField::Password,
            &revoked_target,
            now(),
        );
        let mut revoked_release = revoked_broker
            .release(
                &revoked_descriptor.handle,
                CredentialField::Password,
                &revoked_target,
                now(),
                revoked_permit,
            )
            .await
            .unwrap();
        assert!(revoked_broker.revoke_binding(registration));
        assert_eq!(
            revoked_release.take_runtime_lease().err(),
            Some(BrokeredReleaseError::Revoked)
        );
        drop(revoked_release);
        tokio::time::timeout(Duration::from_secs(1), async {
            while revoked_provider.cancellations.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("post-return cancellation");
        assert_eq!(revoked_provider.cancellations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn release_plan_variants_remain_distinct() {
        let provider = FakeProvider::new([FakeOutcome::ProviderFill, FakeOutcome::UserPresence]);
        let (broker, _, session, context) = broker_with(provider, Some(2));
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p7:1",
        );
        let first = broker.find_credentials(&target, now()).unwrap().remove(0);
        let first_permit = permit(
            &broker,
            &context,
            &first.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let mut first_release = broker
            .release(
                &first.handle,
                CredentialField::Password,
                &target,
                now(),
                first_permit,
            )
            .await
            .unwrap();
        assert_eq!(
            first_release.plan_kind(),
            Some(ReleasePlanKind::ProviderFills)
        );
        assert!(first_release.take_provider_fill_session().is_ok());
        assert_eq!(first_release.plan_kind(), None);
        first_release.complete();

        let second = broker.find_credentials(&target, now()).unwrap().remove(0);
        let second_permit = permit(
            &broker,
            &context,
            &second.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let mut second_release = broker
            .release(
                &second.handle,
                CredentialField::Password,
                &target,
                now(),
                second_permit,
            )
            .await
            .unwrap();
        assert_eq!(
            second_release.plan_kind(),
            Some(ReleasePlanKind::NeedsUserPresence)
        );
        assert!(second_release.take_user_presence_handle().is_ok());
        assert_eq!(second_release.plan_kind(), None);
        second_release.complete();
    }

    #[tokio::test]
    async fn expiry_and_revocation_fail_before_provider_release() {
        let provider = FakeProvider::new([]);
        let (broker, registration, session, context) = broker_with(provider.clone(), Some(2));
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p8:1",
        );
        let expired = broker.find_credentials(&target, now()).unwrap().remove(0);
        let expired_permit = permit(
            &broker,
            &context,
            &expired.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        assert_eq!(
            broker
                .release(
                    &expired.handle,
                    CredentialField::Password,
                    &target,
                    now() + DEFAULT_CREDENTIAL_HANDLE_TTL,
                    expired_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::HandleExpired
        );
        let revoked = broker.find_credentials(&target, now()).unwrap().remove(0);
        let revoked_permit = permit(
            &broker,
            &context,
            &revoked.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        assert!(broker.revoke_binding(registration));
        assert_eq!(
            broker
                .release(
                    &revoked.handle,
                    CredentialField::Password,
                    &target,
                    now(),
                    revoked_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::HandleRevoked
        );
        assert_eq!(provider.releases.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn lifecycle_session_end_revokes_bindings_and_handles() {
        let provider = FakeProvider::new([]);
        let (broker, _, session, context) = broker_with(provider.clone(), Some(2));
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p9:1",
        );
        let descriptor = broker.find_credentials(&target, now()).unwrap().remove(0);
        let release_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        crate::session::fire_session_end(&session);
        assert_eq!(
            broker
                .release(
                    &descriptor.handle,
                    CredentialField::Password,
                    &target,
                    now(),
                    release_permit,
                )
                .await
                .unwrap_err(),
            SecretBrokerError::HandleRevoked
        );
        assert!(broker.find_credentials(&target, now()).unwrap().is_empty());
        assert_eq!(provider.releases.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn lifecycle_session_end_cancels_inflight_release_and_discards_late_plan() {
        let entered = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let provider = FakeProvider::new([FakeOutcome::Wait {
            entered: entered.clone(),
            resume: resume.clone(),
            value: b"late-synthetic-secret".to_vec(),
        }]);
        let (broker, _, session, context) = broker_with(provider.clone(), Some(2));
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p10:1",
        );
        let descriptor = broker.find_credentials(&target, now()).unwrap().remove(0);
        let release_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let broker_for_task = broker.clone();
        let target_for_task = target.clone();
        let handle = descriptor.handle.clone();
        let pending = tokio::spawn(async move {
            broker_for_task
                .release(
                    &handle,
                    CredentialField::Password,
                    &target_for_task,
                    now(),
                    release_permit,
                )
                .await
        });
        entered.notified().await;

        crate::session::fire_session_end(&session);
        tokio::time::timeout(Duration::from_secs(1), async {
            while provider.cancellations.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("in-flight provider cancellation");
        resume.notify_waiters();

        assert_eq!(
            pending.await.unwrap().unwrap_err(),
            SecretBrokerError::HandleRevoked
        );
        assert_eq!(provider.cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(provider.effects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn provider_revocation_cancels_before_effect_and_blocks_discovery() {
        let entered = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let provider = FakeProvider::new([FakeOutcome::Wait {
            entered: entered.clone(),
            resume: resume.clone(),
            value: b"provider-revocation-canary".to_vec(),
        }]);
        let (broker, _, session, context) = broker_with(provider.clone(), Some(2));
        let target = target(
            broker.runtime_scope(),
            &session,
            "https://accounts.example.test/login",
            "p104:1",
        );
        let descriptor = broker.find_credentials(&target, now()).unwrap().remove(0);
        let release_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let broker_for_task = broker.clone();
        let target_for_task = target.clone();
        let handle = descriptor.handle.clone();
        let pending = tokio::spawn(async move {
            broker_for_task
                .release(
                    &handle,
                    CredentialField::Password,
                    &target_for_task,
                    now(),
                    release_permit,
                )
                .await
        });
        entered.notified().await;

        assert!(broker.revoke_provider(provider.id()));
        resume.notify_waiters();
        assert_eq!(
            pending.await.unwrap().unwrap_err(),
            SecretBrokerError::HandleRevoked
        );
        assert_eq!(provider.effects.load(Ordering::SeqCst), 0);
        assert!(broker.find_credentials(&target, now()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn provider_cancel_tombstone_blocks_a_later_release_effect() {
        let provider = FakeProvider::new([FakeOutcome::Runtime(
            b"cancel-before-release-canary".to_vec(),
        )]);
        let release_id = ProviderReleaseId(Uuid::new_v4());
        provider.cancel(release_id).await.unwrap();
        let request = ProviderReleaseRequest {
            release_id,
            field: CredentialField::Password,
            locator: PrivateCredentialFieldLocator::new("provider://cancelled/password").unwrap(),
            target: target(
                "runtime-cancelled",
                "__cua_runtime_runtime-cancelled:agent",
                "https://accounts.example.test/login",
                "p106:1",
            ),
            cancelled: Arc::new(AtomicBool::new(false)),
        };

        assert_eq!(
            provider
                .release(&request)
                .await
                .err()
                .map(|error| error.kind()),
            Some(CredentialProviderErrorKind::Cancelled)
        );
        assert_eq!(provider.effects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn terminal_runtime_suspension_revokes_broker_and_inflight_release() {
        let entered = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let provider = FakeProvider::new([FakeOutcome::Wait {
            entered: entered.clone(),
            resume: resume.clone(),
            value: b"runtime-suspension-canary".to_vec(),
        }]);
        let (broker, _, session, context) = broker_with(provider.clone(), Some(2));
        let runtime_scope = broker.runtime_scope().to_owned();
        let target = target(
            &runtime_scope,
            &session,
            "https://accounts.example.test/login",
            "p105:1",
        );
        let descriptor = broker.find_credentials(&target, now()).unwrap().remove(0);
        let release_permit = permit(
            &broker,
            &context,
            &descriptor.handle,
            CredentialField::Password,
            &target,
            now(),
        );
        let broker_for_task = broker.clone();
        let target_for_task = target.clone();
        let handle = descriptor.handle.clone();
        let pending = tokio::spawn(async move {
            broker_for_task
                .release(
                    &handle,
                    CredentialField::Password,
                    &target_for_task,
                    now(),
                    release_permit,
                )
                .await
        });
        entered.notified().await;

        assert!(crate::session::suspend_runtime_scope(&runtime_scope));
        resume.notify_waiters();
        assert_eq!(
            pending.await.unwrap().unwrap_err(),
            SecretBrokerError::HandleRevoked
        );
        assert_eq!(provider.effects.load(Ordering::SeqCst), 0);
        assert_eq!(
            broker.find_credentials(&target, now()).unwrap_err(),
            SecretBrokerError::RuntimeSuspended
        );
        assert!(crate::session::forget_suspended_runtime_scope(
            &runtime_scope
        ));
    }

    #[test]
    fn secret_and_bootstrap_bounds_are_fixed_and_content_free() {
        assert_eq!(
            SecretLease::from_utf8_bytes(Vec::new()).err(),
            Some(SecretLeaseError::Empty)
        );
        assert_eq!(
            SecretLease::from_utf8_bytes(vec![b'x'; MAX_SECRET_BYTES + 1]).err(),
            Some(SecretLeaseError::TooLarge)
        );
        assert_eq!(
            SecretLease::from_utf8_bytes(vec![0xff]).err(),
            Some(SecretLeaseError::InvalidUtf8)
        );
        let bootstrap = BootstrapSecret::from_bytes(b"synthetic-bootstrap".to_vec()).unwrap();
        let lease = bootstrap.into_lease();
        assert_eq!(lease.expose_to_provider(), b"synthetic-bootstrap");
        assert_eq!(
            BootstrapSecret::from_bytes(vec![0; MAX_BOOTSTRAP_SECRET_BYTES + 1]).err(),
            Some(BootstrapStoreError::InvalidValue)
        );
    }

    #[test]
    fn bootstrap_store_records_round_trip_variable_length_values() {
        for value in [
            b"x".to_vec(),
            b"synthetic-bootstrap-value".to_vec(),
            vec![0x5a; MAX_BOOTSTRAP_SECRET_BYTES],
        ] {
            let encoded = encode_bootstrap_store_record(
                BootstrapVersion(7),
                BootstrapSecret::from_bytes(value.clone()).unwrap(),
            )
            .unwrap();
            let (version, decoded) = decode_bootstrap_store_record(encoded.to_vec()).unwrap();
            assert_eq!(version, BootstrapVersion(7));
            assert_eq!(decoded.expose_to_provider(), value.as_slice());
        }
    }

    #[test]
    fn bootstrap_store_records_reject_zero_version_and_bad_magic() {
        assert_eq!(
            encode_bootstrap_store_record(
                BootstrapVersion(0),
                BootstrapSecret::from_bytes(b"synthetic-bootstrap".to_vec()).unwrap(),
            )
            .err(),
            Some(BootstrapStoreError::Corrupt)
        );

        let mut zero_version = Vec::from(BOOTSTRAP_RECORD_MAGIC.as_slice());
        zero_version.extend_from_slice(&0_u64.to_be_bytes());
        zero_version.push(b'x');
        assert_eq!(
            decode_bootstrap_store_record(zero_version).err(),
            Some(BootstrapStoreError::Corrupt)
        );

        let mut bad_magic = b"BAD!".to_vec();
        bad_magic.extend_from_slice(&1_u64.to_be_bytes());
        bad_magic.push(b'x');
        assert_eq!(
            decode_bootstrap_store_record(bad_magic).err(),
            Some(BootstrapStoreError::Corrupt)
        );
    }

    #[test]
    fn bootstrap_store_records_reject_truncation_and_oversize() {
        for length in 0..=BOOTSTRAP_RECORD_HEADER_BYTES {
            assert_eq!(
                decode_bootstrap_store_record(vec![0; length]).err(),
                Some(BootstrapStoreError::Corrupt)
            );
        }

        let mut oversized = Vec::from(BOOTSTRAP_RECORD_MAGIC.as_slice());
        oversized.extend_from_slice(&1_u64.to_be_bytes());
        oversized.resize(
            BOOTSTRAP_RECORD_HEADER_BYTES + MAX_BOOTSTRAP_SECRET_BYTES + 1,
            b'x',
        );
        assert_eq!(
            decode_bootstrap_store_record(oversized).err(),
            Some(BootstrapStoreError::Corrupt)
        );
    }

    #[test]
    fn bootstrap_namespace_debug_output_is_redacted() {
        let namespace = BootstrapNamespace::new("private-provider-namespace").unwrap();
        let debug = format!("{namespace:?}");
        assert_eq!(debug, "BootstrapNamespace([private])");
        assert!(!debug.contains(namespace.as_str()));
    }
}
