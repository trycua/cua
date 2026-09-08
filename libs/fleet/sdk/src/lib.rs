mod claims;
mod client;
mod error;
mod namespaces;
mod pools;
mod routes;
mod services;
mod signed_service_urls;
mod status;
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
pub use status::{
    PoolDisplayStatus, PoolDisplayStatusKind, healthy_pool_display_status, pool_display_status,
    removed_pool_display_status, terminating_pool_display_status, unknown_pool_display_status,
};
pub use transport::{AccessTokenProvider, HttpClient};
pub use types::{
    Claim, CreateClaimRequest, CreateClaimRequestBuilder, CreatePoolRequest,
    CreatePoolRequestBuilder, CreateSignedServiceUrlRequest, CreateSignedServiceUrlRequestBuilder,
    CreateTemplateRequest, CreateTemplateRequestBuilder, CreateUserApiKeyRequest,
    CreateUserApiKeyRequestBuilder, CyclopsConfiguration, CyclopsCredentials,
    CyclopsTokenProviderConfiguration, CyclopsTokenProviderConfigurationBuilder, HttpHeader,
    HttpRequest, HttpRequestBuilder, HttpResponse, Namespace, NewUserApiKey, Pool,
    ResourceMetadata, Sandbox, SignedServiceUrl, Template, TemplateBuilder, UserApiKey,
};

uniffi::setup_scaffolding!("fleet_sdk");
