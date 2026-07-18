use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

mod client;
pub mod generated_crd;
pub use client::{
    AccessToken, ClaimUrl, HttpRequest, HttpResponse, OAuthCredentials, Platform, PlatformError,
    PoolUrl, Sdk, SdkError, ServiceHttpRequest, ServiceHttpResponse,
};
pub use generated_crd::*;

pub const PROTOCOL_VERSION: u32 = 7;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Configuration {
    pub protocol_version: u32,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceMetadata {
    pub namespace: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    pub api_version: String,
    pub kind: String,
    pub metadata: ResourceMetadata,
    pub spec: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub api_version: String,
    pub kind: String,
    pub metadata: ResourceMetadata,
    pub spec: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct ResourceList<T> {
    #[serde(default)]
    pub items: Vec<T>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sandbox {
    pub namespace: String,
    pub claim: String,
    pub sandbox: String,
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreatePoolRequest {
    pub namespace: String,
    pub spec: PoolSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateClaimRequest {
    pub pool: Pool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<ClaimSpec>,
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("protocol version {0} is unsupported; expected {PROTOCOL_VERSION}")]
    Protocol(u32),
    #[error("resource names must be DNS-1123 labels")]
    InvalidName,
    #[error("base_url must use http or https")]
    InvalidBaseUrl,
}

pub(crate) fn validate_configuration(configuration: &Configuration) -> Result<(), CoreError> {
    if configuration.protocol_version != PROTOCOL_VERSION {
        return Err(CoreError::Protocol(configuration.protocol_version));
    }
    if !(configuration.base_url.starts_with("http://")
        || configuration.base_url.starts_with("https://"))
    {
        return Err(CoreError::InvalidBaseUrl);
    }
    Ok(())
}
pub(crate) fn validate_name(value: &str) -> Result<(), CoreError> {
    if is_dns_label(value) {
        Ok(())
    } else {
        Err(CoreError::InvalidName)
    }
}
fn is_dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

#[cfg(target_arch = "wasm32")]
mod component {
    wit_bindgen::generate!({ path: "wit", world: "cyclops-sdk" });
    use super::{
        AccessToken, HttpRequest, HttpResponse, OAuthCredentials, Platform, PlatformError, Sdk,
        ServiceHttpRequest as CoreServiceHttpRequest,
    };
    use std::cell::RefCell;
    use trycua::cyclops_sdk::{crd_types, platform, requests, resources, service_http};
    include!("generated_crd_component.rs");

    struct ComponentPlatform;
    impl Platform for ComponentPlatform {
        fn http(&mut self, request: HttpRequest) -> Result<HttpResponse, PlatformError> {
            platform::http(&platform::HttpRequest {
                method: request.method,
                url: request.url,
                headers: request
                    .headers
                    .into_iter()
                    .map(|(name, value)| platform::Header { name, value })
                    .collect(),
                body: request.body,
            })
            .map(|response| HttpResponse {
                status: response.status,
                body: response.body,
            })
            .map_err(PlatformError::Transport)
        }
        fn sleep(&mut self, milliseconds: u64) -> Result<(), PlatformError> {
            platform::sleep(milliseconds).map_err(PlatformError::Clock)
        }
        fn now_ms(&mut self) -> u64 {
            platform::now_ms()
        }
        fn acquire_oauth_credentials(&mut self) -> Result<OAuthCredentials, PlatformError> {
            platform::acquire_oauth_credentials()
                .map(|value| OAuthCredentials {
                    token_url: value.token_url,
                    client_id: value.client_id,
                    client_secret: value.client_secret,
                })
                .map_err(PlatformError::Credentials)
        }
        fn load_access_token(&mut self) -> Result<Option<AccessToken>, PlatformError> {
            platform::load_access_token()
                .map(|value| {
                    value.map(|token| AccessToken {
                        value: token.value,
                        expires_at_ms: token.expires_at_ms,
                    })
                })
                .map_err(PlatformError::TokenStorage)
        }
        fn store_access_token(&mut self, token: &AccessToken) -> Result<(), PlatformError> {
            platform::store_access_token(&platform::AccessToken {
                value: token.value.clone(),
                expires_at_ms: token.expires_at_ms,
            })
            .map_err(PlatformError::TokenStorage)
        }
    }
    thread_local! { static SDK: RefCell<Option<Sdk<ComponentPlatform>>> = const { RefCell::new(None) }; }
    fn with_sdk<T>(
        operation: impl FnOnce(&mut Sdk<ComponentPlatform>) -> Result<T, String>,
    ) -> Result<T, String> {
        SDK.with(|slot| {
            operation(
                slot.borrow_mut()
                    .as_mut()
                    .ok_or_else(|| "SDK is not configured".to_string())?,
            )
        })
    }
    fn pool_from_wit(value: resources::Pool) -> Result<super::Pool, String> {
        Ok(super::Pool {
            api_version: value.api_version,
            kind: value.kind,
            metadata: super::ResourceMetadata {
                namespace: value.metadata.namespace,
                name: value.metadata.name,
                labels: value
                    .metadata
                    .labels_json
                    .map(|labels| serde_json::from_str(&labels).map_err(|error| error.to_string()))
                    .transpose()?,
            },
            spec: serde_json::from_str(&value.spec_json).map_err(|error| error.to_string())?,
            status: value
                .status_json
                .map(|status| serde_json::from_str(&status).map_err(|error| error.to_string()))
                .transpose()?,
        })
    }
    fn pool_to_wit(value: super::Pool) -> Result<resources::Pool, String> {
        Ok(resources::Pool {
            api_version: value.api_version,
            kind: value.kind,
            metadata: resources::ResourceMetadata {
                namespace: value.metadata.namespace,
                name: value.metadata.name,
                labels_json: value
                    .metadata
                    .labels
                    .map(|labels| serde_json::to_string(&labels).map_err(|error| error.to_string()))
                    .transpose()?,
            },
            spec_json: serde_json::to_string(&value.spec).map_err(|error| error.to_string())?,
            status_json: value
                .status
                .map(|status| serde_json::to_string(&status).map_err(|error| error.to_string()))
                .transpose()?,
        })
    }
    fn claim_from_wit(value: resources::Claim) -> Result<super::Claim, String> {
        Ok(super::Claim {
            api_version: value.api_version,
            kind: value.kind,
            metadata: super::ResourceMetadata {
                namespace: value.metadata.namespace,
                name: value.metadata.name,
                labels: value
                    .metadata
                    .labels_json
                    .map(|labels| serde_json::from_str(&labels).map_err(|error| error.to_string()))
                    .transpose()?,
            },
            spec: serde_json::from_str(&value.spec_json).map_err(|error| error.to_string())?,
            status: value
                .status_json
                .map(|status| serde_json::from_str(&status).map_err(|error| error.to_string()))
                .transpose()?,
        })
    }
    fn claim_to_wit(value: super::Claim) -> Result<resources::Claim, String> {
        Ok(resources::Claim {
            api_version: value.api_version,
            kind: value.kind,
            metadata: resources::ResourceMetadata {
                namespace: value.metadata.namespace,
                name: value.metadata.name,
                labels_json: value
                    .metadata
                    .labels
                    .map(|labels| serde_json::to_string(&labels).map_err(|error| error.to_string()))
                    .transpose()?,
            },
            spec_json: serde_json::to_string(&value.spec).map_err(|error| error.to_string())?,
            status_json: value
                .status
                .map(|status| serde_json::to_string(&status).map_err(|error| error.to_string()))
                .transpose()?,
        })
    }
    fn sandbox_to_wit(value: super::Sandbox) -> resources::Sandbox {
        resources::Sandbox {
            namespace: value.namespace,
            claim: value.claim,
            sandbox: value.sandbox,
            services: value.services,
        }
    }
    fn sandbox_from_wit(value: resources::Sandbox) -> super::Sandbox {
        super::Sandbox {
            namespace: value.namespace,
            claim: value.claim,
            sandbox: value.sandbox,
            services: value.services,
        }
    }

    struct CyclopsComponent;
    impl Guest for CyclopsComponent {
        fn protocol_version() -> u32 {
            super::PROTOCOL_VERSION
        }
        fn configure(configuration_json: String) -> Result<(), String> {
            let configuration =
                serde_json::from_str(&configuration_json).map_err(|error| error.to_string())?;
            let sdk = Sdk::connect(ComponentPlatform, configuration)
                .map_err(|error| error.to_string())?;
            SDK.with(|slot| *slot.borrow_mut() = Some(sdk));
            Ok(())
        }
        fn create_pool(request: requests::CreatePoolRequest) -> Result<resources::Pool, String> {
            with_sdk(|sdk| {
                pool_to_wit(
                    sdk.create_pool(super::CreatePoolRequest {
                        namespace: request.namespace,
                        spec: pool_spec_from_wit(request.spec)?,
                    })
                    .map_err(|error| error.to_string())?,
                )
            })
        }
        fn list_pools(namespace: String) -> Result<Vec<resources::Pool>, String> {
            with_sdk(|sdk| {
                sdk.list_pools(&namespace)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(pool_to_wit)
                    .collect()
            })
        }
        fn get_pool(value: resources::Pool) -> Result<resources::Pool, String> {
            with_sdk(|sdk| {
                pool_to_wit(
                    sdk.get_pool(pool_from_wit(value)?)
                        .map_err(|error| error.to_string())?,
                )
            })
        }
        fn update_pool(value: resources::Pool) -> Result<resources::Pool, String> {
            with_sdk(|sdk| {
                pool_to_wit(
                    sdk.update_pool(pool_from_wit(value)?)
                        .map_err(|error| error.to_string())?,
                )
            })
        }
        fn delete_pool(value: resources::Pool) -> Result<(), String> {
            with_sdk(|sdk| {
                sdk.delete_pool(pool_from_wit(value)?)
                    .map_err(|error| error.to_string())
            })
        }
        fn create_claim(request: requests::CreateClaimRequest) -> Result<resources::Claim, String> {
            with_sdk(|sdk| {
                claim_to_wit(
                    sdk.create_claim(super::CreateClaimRequest {
                        pool: pool_from_wit(request.pool)?,
                        spec: request.spec.map(claim_spec_from_wit).transpose()?,
                    })
                    .map_err(|error| error.to_string())?,
                )
            })
        }
        fn list_claims(namespace: String) -> Result<Vec<resources::Claim>, String> {
            with_sdk(|sdk| {
                sdk.list_claims(&namespace)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(claim_to_wit)
                    .collect()
            })
        }
        fn get_claim(value: resources::Claim) -> Result<resources::Claim, String> {
            with_sdk(|sdk| {
                claim_to_wit(
                    sdk.get_claim(claim_from_wit(value)?)
                        .map_err(|error| error.to_string())?,
                )
            })
        }
        fn update_claim(value: resources::Claim) -> Result<resources::Claim, String> {
            with_sdk(|sdk| {
                claim_to_wit(
                    sdk.update_claim(claim_from_wit(value)?)
                        .map_err(|error| error.to_string())?,
                )
            })
        }
        fn delete_claim(value: resources::Claim) -> Result<(), String> {
            with_sdk(|sdk| {
                sdk.delete_claim(claim_from_wit(value)?)
                    .map_err(|error| error.to_string())
            })
        }
        fn wait_claim(value: resources::Claim) -> Result<resources::Sandbox, String> {
            with_sdk(|sdk| {
                Ok(sandbox_to_wit(
                    sdk.wait_claim(claim_from_wit(value)?)
                        .map_err(|error| error.to_string())?,
                ))
            })
        }
        fn service_request(
            value: resources::Sandbox,
            service: String,
            request: service_http::ServiceHttpRequest,
        ) -> Result<service_http::ServiceHttpResponse, String> {
            with_sdk(|sdk| {
                sdk.service_request(
                    &sandbox_from_wit(value),
                    &service,
                    CoreServiceHttpRequest {
                        method: request.method,
                        path: request.path,
                        headers: request
                            .headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        body: request.body,
                    },
                )
                .map(|response| service_http::ServiceHttpResponse {
                    status: response.status,
                    body: response.body,
                })
                .map_err(|error| error.to_string())
            })
        }
    }
    export!(CyclopsComponent);
}
