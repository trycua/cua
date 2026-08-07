mod claims;
mod client;
mod error;
mod namespaces;
mod pools;
mod routes;
mod services;
mod templates;
mod transport;
mod types;
mod user_keys;

pub use client::CyclopsClient;
pub use error::{
    AccessTokenProviderError, HttpError, MAX_STATUS_BODY_BYTES, SdkBuildError, SdkError,
    bounded_body,
};
pub use routes::validate_dns_label;
pub use transport::{AccessTokenProvider, HttpClient};
pub use types::{
    Claim, CreateClaimRequest, CreatePoolRequest, CreatePoolRequestBuilder, CreateTemplateRequest,
    CreateTemplateRequestBuilder, CreateUserApiKeyRequest, CyclopsConfiguration,
    CyclopsCredentials, CyclopsTokenProviderConfiguration, HttpHeader, HttpRequest, HttpResponse,
    Namespace, NewUserApiKey, Pool, ResourceMetadata, Sandbox, Template, UserApiKey,
};

uniffi::setup_scaffolding!("fleet_sdk");
