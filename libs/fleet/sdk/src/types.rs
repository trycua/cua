use cyclops_sdk_schema::{
    ClaimSpec, OSGymSandboxClaimStatus, OSGymSandboxTemplateSpec, OSGymSandboxWarmPoolSpec,
    OSGymSandboxWarmPoolStatus,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashMap, fmt, sync::Arc};

#[derive(uniffi::Object)]
pub struct CyclopsCredentials {
    client_id: String,
    client_secret: String,
}

#[uniffi::export]
impl CyclopsCredentials {
    #[uniffi::constructor]
    pub fn new(client_id: String, client_secret: String) -> Arc<Self> {
        Arc::new(Self {
            client_id,
            client_secret,
        })
    }
}

#[allow(dead_code)]
impl CyclopsCredentials {
    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }

    pub(crate) fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

#[derive(Clone, uniffi::Record)]
pub struct CyclopsConfiguration {
    pub base_url: String,
    pub token_url: String,
    pub credentials: Arc<CyclopsCredentials>,
    pub pool_poll_interval_ms: u64,
    pub pool_poll_limit: u32,
    pub claim_poll_interval_ms: u64,
    pub claim_poll_limit: u32,
}

impl fmt::Debug for CyclopsConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CyclopsConfiguration")
            .field("base_url", &self.base_url)
            .field("token_url", &self.token_url)
            .field("credentials", &"<redacted>")
            .field("pool_poll_interval_ms", &self.pool_poll_interval_ms)
            .field("pool_poll_limit", &self.pool_poll_limit)
            .field("claim_poll_interval_ms", &self.claim_poll_interval_ms)
            .field("claim_poll_limit", &self.claim_poll_limit)
            .finish()
    }
}

#[allow(dead_code)]
impl CyclopsConfiguration {
    pub(crate) fn client_id(&self) -> &str {
        self.credentials.client_id()
    }

    pub(crate) fn client_secret(&self) -> &str {
        self.credentials.client_secret()
    }
}

#[derive(Clone, Debug, uniffi::Record, uniffi_builder_derive::UniffiBuilder)]
#[uniffi_builder(crate::SdkBuildError)]
pub struct CyclopsTokenProviderConfiguration {
    pub base_url: String,
    pub pool_poll_interval_ms: u64,
    pub pool_poll_limit: u32,
    pub claim_poll_interval_ms: u64,
    pub claim_poll_limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ResourceMetadata {
    pub namespace: String,
    pub name: String,
    pub labels: Option<HashMap<String, String>>,
    #[serde(
        rename = "creationTimestamp",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub creation_timestamp: Option<String>,
}

/// UniFFI cannot emit aliases for external record types. Generated bindings use
/// `OSGymSandboxWarmPoolStatus` and `OSGymSandboxClaimStatus` from cyclops_sdk_schema.
///
/// A `Pool` is the `osgym.cua.ai/v1alpha1 OSGymSandboxWarmPool` CR verbatim —
/// the SDK is a naive CRUD mapper over the native CRDs, with no translation
/// layer (the legacy `cua.ai/v1 OSGymWorkspacePool` and its operator compat
/// shim are gone).
#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    pub api_version: String,
    pub kind: String,
    pub metadata: ResourceMetadata,
    pub spec: OSGymSandboxWarmPoolSpec,
    pub status: Option<OSGymSandboxWarmPoolStatus>,
}

impl PartialEq for Pool {
    fn eq(&self, other: &Self) -> bool {
        self.api_version == other.api_version
            && self.kind == other.kind
            && self.metadata == other.metadata
            && schema_values_equal(&self.spec, &other.spec)
            && schema_values_equal(&self.status, &other.status)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub api_version: String,
    pub kind: String,
    pub metadata: ResourceMetadata,
    pub spec: ClaimSpec,
    pub status: Option<OSGymSandboxClaimStatus>,
}

impl PartialEq for Claim {
    fn eq(&self, other: &Self) -> bool {
        self.api_version == other.api_version
            && self.kind == other.kind
            && self.metadata == other.metadata
            && schema_values_equal(&self.spec, &other.spec)
            && schema_values_equal(&self.status, &other.status)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct Namespace {
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct UserApiKey {
    pub id: String,
    #[serde(rename = "client_id")]
    pub client_id: String,
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_nullable_scope")]
    pub scope: Vec<String>,
}

fn deserialize_nullable_scope<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    uniffi::Record,
    uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SdkBuildError)]
pub struct CreateUserApiKeyRequest {
    pub name: String,
    pub scope: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct NewUserApiKey {
    #[serde(rename = "client_id")]
    pub client_id: String,
    #[serde(rename = "client_secret")]
    pub client_secret: String,
    #[serde(rename = "token_url")]
    pub token_url: String,
    pub name: String,
    pub scope: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Sandbox {
    pub namespace: String,
    pub claim: String,
    pub name: String,
    pub services: Vec<String>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    uniffi::Record,
    uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SdkBuildError)]
pub struct CreateSignedServiceUrlRequest {
    pub sandbox: Sandbox,
    pub service: String,
    pub label: Option<String>,
    pub expires_in_seconds: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct SignedServiceUrl {
    pub id: String,
    pub namespace: String,
    pub claim: String,
    pub sandbox: String,
    pub service: String,
    pub label: Option<String>,
    pub url: String,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

#[derive(
    Clone, Debug, Serialize, Deserialize, uniffi::Record, uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SdkBuildError)]
pub struct CreatePoolRequest {
    pub namespace: String,
    pub spec: OSGymSandboxWarmPoolSpec,
}

impl PartialEq for CreatePoolRequest {
    fn eq(&self, other: &Self) -> bool {
        self.namespace == other.namespace && schema_values_equal(&self.spec, &other.spec)
    }
}

/// The `osgym.cua.ai/v1alpha1 OSGymSandboxTemplate` CR verbatim. Warm pools
/// and claims reference one by name via `spec.sandboxTemplateRef.name`.
#[derive(
    Clone, Debug, Serialize, Deserialize, uniffi::Record, uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SdkBuildError)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub api_version: String,
    pub kind: String,
    pub metadata: ResourceMetadata,
    pub spec: OSGymSandboxTemplateSpec,
}

impl PartialEq for Template {
    fn eq(&self, other: &Self) -> bool {
        self.api_version == other.api_version
            && self.kind == other.kind
            && self.metadata == other.metadata
            && schema_values_equal(&self.spec, &other.spec)
    }
}

#[derive(
    Clone, Debug, Serialize, Deserialize, uniffi::Record, uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SdkBuildError)]
pub struct CreateTemplateRequest {
    pub namespace: String,
    pub name: String,
    pub spec: OSGymSandboxTemplateSpec,
}

impl PartialEq for CreateTemplateRequest {
    fn eq(&self, other: &Self) -> bool {
        self.namespace == other.namespace
            && self.name == other.name
            && schema_values_equal(&self.spec, &other.spec)
    }
}

#[derive(
    Clone, Debug, Serialize, Deserialize, uniffi::Record, uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SdkBuildError)]
pub struct CreateClaimRequest {
    pub pool: Pool,
    pub spec: Option<ClaimSpec>,
    /// Explicit claim name. A client-supplied name is used verbatim (after
    /// DNS-label validation); left unset, the client generates a random
    /// `claim-<petname>` so concurrent leases and retries cannot collide.
    #[serde(default)]
    #[uniffi(default = None)]
    pub name: Option<String>,
}

impl PartialEq for CreateClaimRequest {
    fn eq(&self, other: &Self) -> bool {
        self.pool == other.pool
            && schema_values_equal(&self.spec, &other.spec)
            && self.name == other.name
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    uniffi::Record,
    uniffi_builder_derive::UniffiBuilder,
)]
#[uniffi_builder(crate::SdkBuildError)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    pub body: Option<Vec<u8>>,
    /// Per-request timeout. Defaults to absent so callers written against the
    /// pre-timeout record shape keep constructing requests unchanged; absent
    /// falls back to the native client's 30-second default.
    #[serde(default)]
    #[uniffi(default = None)]
    pub timeout_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: Vec<u8>,
}

fn schema_values_equal<T: Serialize>(left: &T, right: &T) -> bool {
    serde_json::to_value(left).ok() == serde_json::to_value(right).ok()
}
