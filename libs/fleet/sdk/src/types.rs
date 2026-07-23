use cyclops_sdk_schema::{ClaimSpec, OSGymSandboxClaimStatus, OSGymWorkspacePoolStatus, PoolSpec};
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, uniffi::Record)]
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
}

/// UniFFI cannot emit aliases for external record types. Generated bindings use
/// `OSGymWorkspacePoolStatus` and `OSGymSandboxClaimStatus` from cyclops_sdk_schema.
#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    pub api_version: String,
    pub kind: String,
    pub metadata: ResourceMetadata,
    pub spec: PoolSpec,
    pub status: Option<OSGymWorkspacePoolStatus>,
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
pub struct Sandbox {
    pub namespace: String,
    pub claim: String,
    pub name: String,
    pub services: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
pub struct CreatePoolRequest {
    pub namespace: String,
    pub spec: PoolSpec,
}

impl PartialEq for CreatePoolRequest {
    fn eq(&self, other: &Self) -> bool {
        self.namespace == other.namespace && schema_values_equal(&self.spec, &other.spec)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
pub struct CreateClaimRequest {
    pub pool: Pool,
    pub spec: Option<ClaimSpec>,
}

impl PartialEq for CreateClaimRequest {
    fn eq(&self, other: &Self) -> bool {
        self.pool == other.pool && schema_values_equal(&self.spec, &other.spec)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    pub body: Option<Vec<u8>>,
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
