mod claims;
mod client;
mod error;
mod pools;
mod routes;
mod services;
mod templates;
mod transport;
mod types;

pub use client::CyclopsClient;
pub use error::{
    AccessTokenProviderError, HttpError, MAX_STATUS_BODY_BYTES, SdkError, bounded_body,
};
pub use routes::validate_dns_label;
pub use transport::{AccessTokenProvider, HttpClient};
pub use types::{
    Claim, CreateClaimRequest, CreatePoolRequest, CreateTemplateRequest, CyclopsConfiguration,
    CyclopsCredentials, CyclopsTokenProviderConfiguration, HttpHeader, HttpRequest, HttpResponse,
    Pool, ResourceMetadata, Sandbox, Template,
};

uniffi::setup_scaffolding!("fleet_sdk");
