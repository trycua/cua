mod claims;
mod client;
mod error;
mod fleets;
mod image_uploads;
mod images;
mod namespaces;
mod pools;
mod registry_secrets;
mod routes;
mod services;
mod signed_service_urls;
mod status;
mod templates;
mod transport;
mod types;
mod user_keys;

pub use claims::claim_env_token_key;
pub use client::CyclopsClient;
pub use cyclops_sdk_schema::PreservedJson;
pub use cyclops_sdk_schema::{CLAIM_ENV_TOKEN_KEY, CLAIM_SECRET_NAME_PREFIX, ClaimSecretRef};
pub use error::{
    AccessTokenProviderError, HttpError, MAX_STATUS_BODY_BYTES, SdkBuildError, SdkError,
    bounded_body,
};
pub use fleets::{
    FLEET_LABEL_KEY, FleetClaims, FleetPoolRequest, MAX_FLEET_CLAIMS, fleet_label_key,
};
pub use image_uploads::{
    ImageUploadFileRequest, ImageUploadInstruction, ImageUploadRequest, ImageUploadResponse,
    PresignedPut,
};
pub use registry_secrets::{
    CreateRegistrySecretRequest, CreateRegistrySecretRequestBuilder, RegistrySecret, ResolvedImage,
    registry_secret_name_prefix,
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
    ResourceMetadata, Sandbox, ServiceStreamTarget, SignedServiceUrl, Template, TemplateBuilder,
    UserApiKey,
};

uniffi::setup_scaffolding!("fleet_sdk");
