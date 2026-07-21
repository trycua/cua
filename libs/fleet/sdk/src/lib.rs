mod claims;
mod client;
mod error;
mod pools;
mod routes;
mod services;
mod transport;
mod types;

pub use client::CyclopsClient;
pub use error::{HttpError, MAX_STATUS_BODY_BYTES, SdkError, bounded_body};
pub use routes::validate_dns_label;
pub use transport::HttpClient;
pub use types::{
    Claim, CreateClaimRequest, CreatePoolRequest, CyclopsConfiguration, CyclopsCredentials,
    HttpHeader, HttpRequest, HttpResponse, Pool, ResourceMetadata, Sandbox,
};

uniffi::setup_scaffolding!("cyclops_sdk");
