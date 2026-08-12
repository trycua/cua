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
    Claim, CreateClaimRequest, CreateClaimRequestBuilder, CreatePoolRequest,
    CreatePoolRequestBuilder, CreateTemplateRequest, CreateTemplateRequestBuilder,
    CreateUserApiKeyRequest, CreateUserApiKeyRequestBuilder, CyclopsConfiguration,
    CyclopsCredentials, CyclopsTokenProviderConfiguration,
    CyclopsTokenProviderConfigurationBuilder, HttpHeader, HttpRequest, HttpResponse, Namespace,
    NewUserApiKey, Pool, ResourceMetadata, Sandbox, Template, TemplateBuilder, UserApiKey,
};

uniffi::setup_scaffolding!("fleet_sdk");
