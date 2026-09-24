//! Tenant-owned registry pull Secrets and server-side image resolution.
//!
//! A private registry image needs a pull Secret in the pool namespace. The
//! gateway lets a tenant create exactly one kind of Secret for that: a
//! `kubernetes.io/dockerconfigjson` Secret named `cua-registry-<name>` and
//! labeled `cua.ai/registry-secret: "true"`
//! (`cyclops-cs/backend/auth/tenant_secret_admission.rego`). Secrets are
//! write-only through the gateway: no read, list or update, so a credential
//! cannot be read back, and "update" is delete + create.
//!
//! Reference the Secret from `vmTemplate.imagePullSecret`; the pod runtimes
//! put it in the pod's `imagePullSecrets` and KubeVirt uses it for the
//! containerDisk pull.

use crate::{CyclopsClient, HttpHeader, HttpRequest, SdkError, routes};
use base64::{Engine, engine::general_purpose::STANDARD};
use cyclops_sdk_schema::REGISTRY_SECRET_NAME_PREFIX;
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};

const JSON_CONTENT_TYPE: &str = "application/json";
const DOCKER_CONFIG_JSON_TYPE: &str = "kubernetes.io/dockerconfigjson";
const DOCKER_CONFIG_JSON_KEY: &str = ".dockerconfigjson";

/// Credentials for one registry, stored as a `cua-registry-*` pull Secret.
#[derive(Clone, uniffi::Record, uniffi_builder_derive::UniffiBuilder)]
#[uniffi_builder(crate::SdkBuildError)]
pub struct CreateRegistrySecretRequest {
    /// Pool namespace the Secret is created in. It must already exist (the
    /// pool's namespace, created by `create_pool`, or by `create_namespace`).
    pub namespace: String,
    /// Full Secret name, `cua-registry-<dns-label>`.
    pub name: String,
    /// Registry host the credentials are for, as image refs spell it, e.g.
    /// `ghcr.io`, `registry.example.com:5000` or `docker.io`.
    pub registry: String,
    pub username: String,
    /// Password or access token. Never serialized by `Debug`.
    pub password: String,
}

impl fmt::Debug for CreateRegistrySecretRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreateRegistrySecretRequest")
            .field("namespace", &self.namespace)
            .field("name", &self.name)
            .field("registry", &self.registry)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// A created registry pull Secret. Carries no credential: Secrets are
/// write-only through the gateway.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct RegistrySecret {
    pub namespace: String,
    pub name: String,
    pub registry: String,
}

/// A registry ref pinned by the gateway (`GET /api/images/resolve`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedImage {
    /// The ref as requested.
    #[serde(rename = "ref")]
    pub reference: String,
    /// The ref actually resolved: for a canonical image and runtime
    /// `kubevirt` this is the containerDisk sibling (`…:24.04-disk`).
    pub resolved_ref: String,
    /// `repo@sha256:…` of the manifest (or index) to run.
    pub pinned_ref: String,
    pub digest: String,
    /// `rootfs` (docker/gVisor), `containerdisk` (KubeVirt) or `unknown`.
    pub variant: String,
    /// The linux/amd64 child manifest digest when the ref is an index.
    #[serde(default)]
    pub platform_digest: Option<String>,
    pub media_type: String,
}

/// The name prefix every tenant registry pull Secret must carry.
#[uniffi::export]
pub fn registry_secret_name_prefix() -> String {
    REGISTRY_SECRET_NAME_PREFIX.into()
}

#[uniffi::export]
impl CyclopsClient {
    /// Create (or replace) a `cua-registry-*` dockerconfigjson pull Secret.
    /// On a name conflict the old Secret is deleted and the new one created,
    /// since the gateway admits no Secret update.
    pub async fn create_registry_secret(
        self: Arc<Self>,
        request: CreateRegistrySecretRequest,
    ) -> Result<RegistrySecret, SdkError> {
        let collection_url =
            routes::registry_secret_collection(self.base_url(), &request.namespace)?;
        let item_url =
            routes::registry_secret_item(self.base_url(), &request.namespace, &request.name)?;
        let body = registry_secret_body(&request)?;

        let created = self
            .send_allowed(
                "create registry secret",
                json_request("POST", collection_url.clone(), Some(body.clone())),
                &[200, 201, 202, 409],
            )
            .await?;
        if created.status == 409 {
            self.send_unit_crud(
                "replace registry secret",
                json_request("DELETE", item_url, None),
                &[200, 202, 204, 404],
            )
            .await?;
            self.send_unit_crud(
                "create registry secret",
                json_request("POST", collection_url, Some(body)),
                &[200, 201, 202],
            )
            .await?;
        }
        Ok(RegistrySecret {
            namespace: request.namespace,
            name: request.name,
            registry: request.registry,
        })
    }

    pub async fn delete_registry_secret(
        self: Arc<Self>,
        namespace: String,
        name: String,
    ) -> Result<(), SdkError> {
        let item_url = routes::registry_secret_item(self.base_url(), &namespace, &name)?;
        self.send_unit_crud(
            "delete registry secret",
            json_request("DELETE", item_url, None),
            &[200, 202, 204, 404],
        )
        .await
    }

    /// Pin a public registry ref to a digest server-side. `runtime` (`gvisor`,
    /// `kubevirt`, `macos`) selects the variant for canonical cua images:
    /// `kubevirt` maps `ghcr.io/trycua/linux:24.04` to its `-disk` sibling.
    pub async fn resolve_image(
        self: Arc<Self>,
        reference: String,
        runtime: Option<String>,
    ) -> Result<ResolvedImage, SdkError> {
        let url = routes::image_resolve(self.base_url(), &reference, runtime.as_deref())?;
        self.send_json_crud("resolve image", json_request("GET", url, None), &[200])
            .await
    }
}

pub(crate) fn validate_registry_secret_name(name: &str) -> Result<(), SdkError> {
    let rest = name
        .strip_prefix(REGISTRY_SECRET_NAME_PREFIX)
        .ok_or_else(|| SdkError::Configuration {
            reason: format!("registry secret name must start with {REGISTRY_SECRET_NAME_PREFIX}"),
        })?;
    routes::validate_dns_label_for("registry secret name", rest)?;
    if name.len() > 253 {
        return Err(SdkError::Configuration {
            reason: "registry secret name must be at most 253 characters".into(),
        });
    }
    Ok(())
}

fn validate_registry_host(registry: &str) -> Result<(), SdkError> {
    let invalid = registry.is_empty()
        || registry.len() > 255
        || registry.contains("://")
        || registry.contains('/')
        || registry
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control());
    if invalid {
        return Err(SdkError::Configuration {
            reason: "registry must be a bare host[:port] such as ghcr.io".into(),
        });
    }
    Ok(())
}

/// The Secret body the gateway admits: exactly the dockerconfigjson shape,
/// with no annotations, owner references or generateName.
pub(crate) fn registry_secret_body(
    request: &CreateRegistrySecretRequest,
) -> Result<Vec<u8>, SdkError> {
    validate_registry_secret_name(&request.name)?;
    validate_registry_host(&request.registry)?;
    if request.username.is_empty() || request.password.is_empty() {
        return Err(SdkError::Configuration {
            reason: "registry username and password must not be empty".into(),
        });
    }
    let auth = STANDARD.encode(format!("{}:{}", request.username, request.password));
    let docker_config = serde_json::json!({
        "auths": {
            request.registry.clone(): {
                "username": request.username,
                "password": request.password,
                "auth": auth,
            }
        }
    });
    let docker_config = serde_json::to_vec(&docker_config).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })?;
    let secret = serde_json::json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "type": DOCKER_CONFIG_JSON_TYPE,
        "metadata": {
            "name": request.name,
            "namespace": request.namespace,
            "labels": {"cua.ai/registry-secret": "true"},
        },
        "data": {DOCKER_CONFIG_JSON_KEY: STANDARD.encode(docker_config)},
    });
    serde_json::to_vec(&secret).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })
}

fn json_request(method: &str, url: url::Url, body: Option<Vec<u8>>) -> HttpRequest {
    HttpRequest {
        method: method.into(),
        url: url.into(),
        headers: vec![
            HttpHeader {
                name: "accept".into(),
                value: JSON_CONTENT_TYPE.into(),
            },
            HttpHeader {
                name: "content-type".into(),
                value: JSON_CONTENT_TYPE.into(),
            },
        ],
        body,
        timeout_secs: None,
        max_response_bytes: Some(1 << 20),
    }
}
