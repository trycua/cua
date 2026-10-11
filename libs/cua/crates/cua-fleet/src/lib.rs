//! Fleet for the cua SDK.
//!
//! A thin layer over `cyclops-sdk` (the read-only `libs/fleet` mirror) that
//! adds cua-sandbox semantics:
//!
//! - [`FleetConfig`] from the same environment variables and defaults as
//!   cua-sandbox's `_config.py`;
//! - [`PoolSpec`] + [`FleetClient::apply_pool`]: reconcile pool, then
//!   template, rolling the pool back if the template fails (`Pool.apply`);
//! - claims: a named claim reattaches, [`ephemeral_pool_name`] gives
//!   `cua-eph-<hex>`, [`FleetClient::keep_alive`] renews the lease;
//! - service URLs, raw service requests and a gRPC-Web [`cua_spacesd_client`]
//!   connection factory for a claimed sandbox's `env` service;
//! - image and signed-service-URL APIs from the synced mirror;
//! - [`autopool::PoolManager`]: managed, autoscaled pools keyed by spec
//!   for `Sandbox.create(image)` without a pool.
//!
//! Nothing here assumes which daemon an image runs: readiness is Fleet's
//! `Bound` phase plus an optional user-declared TCP readiness probe.

pub mod autopool;
pub mod billing;
pub mod builds;
pub mod claim_secrets;
mod config;
pub mod limits;
pub mod parity;
mod pool;
pub mod pricing;
pub mod runtime;
pub mod spec;
#[cfg(feature = "testing")]
pub mod testing;

pub use autopool::{
    AcquireOpts, AutoPoolConfig, GcReport, ImageResolver, ManagedClaim, ManagedPoolInfo,
    PoolManager, PoolSpecKey,
};
pub use config::{
    DEFAULT_FLEET_BASE_URL, DEFAULT_POLL_INTERVAL_MS, DEFAULT_POLL_LIMIT, DEFAULT_TOKEN_URL,
    FleetConfig,
};
pub use cyclops_sdk as sdk;
pub use cyclops_sdk::{Claim, HttpClient, Pool, Sandbox as BoundSandbox, SdkError, Template};
pub use cyclops_sdk_schema as schema;

/// The one "no Fleet credentials" message, in every SDK and the CLI.
pub const MISSING_CREDENTIALS: &str = "Fleet credentials missing: run `cua auth login` or set \
     CUA_CLIENT_ID/CUA_CLIENT_SECRET, or pass local=True";
pub use billing::{BillingCard, BillingCredit, BillingStatus};
pub use builds::{BuildFile, BuildSpec, BuiltImage};
pub use claim_secrets::{ClaimSecretsWait, TokenProbe, TokenState};
pub use cua_image::spec::ImageLayer;
pub use cua_image::{normalize_registry, registry_of};
pub use limits::{
    CLOUD_DEFAULT_RANGE_CPUS, CLOUD_DEFAULT_RANGE_MEMORY_MB, FLEET_ABSOLUTE_CPUS,
    FLEET_ABSOLUTE_MAX_POOL_SIZE, FLEET_ABSOLUTE_MEMORY_MB, check_cloud_size, check_pool_size,
};
pub(crate) use parity::sidecars_json;
pub use parity::{
    MAIN_CONTAINER_NAME, MAX_SIDECARS, REGISTRY_SECRET_PREFIX, REMOTE_BUILDS_SUPPORTED,
    RESERVED_SERVICE_NAMES, RegistryCredentials, Sidecar, check_reserved_service_names,
    default_sidecar_name, registry_secret_name, remote_builds_unsupported, validate_sidecars,
};
pub use pool::{ClaimOptions, PoolHandle, PoolSpec, RuntimeKind, ephemeral_pool_name};
pub use pricing::{PRICING_TTL, UsagePricing, clear_pricing_cache};
pub use runtime::{
    FleetImage, ImageEvidence, ImageInspector, ImageVariant, MACOS_UNSUPPORTED, canonical_alias,
    canonical_image, check_runtime, ensure_runtime_offered, inspect_image, is_canonical_image,
    parse_runtime, resolve_fleet_image, resolve_fleet_image_with, resolve_runtime,
    runtime_from_reference, runtime_name, set_image_inspector,
};
pub use spec::{
    PoolOptions, ProcessMode, ReadinessProbe, SandboxSpec, SpecDiff, TtlPolicy, format_diffs,
    terraform_pool_block,
};

use cyclops_sdk::{
    AccessTokenProvider, AccessTokenProviderError, CyclopsClient, CyclopsConfiguration,
    CyclopsCredentials, CyclopsTokenProviderConfiguration, HttpHeader, HttpRequest, HttpResponse,
    PreservedJson,
};
use std::{sync::Arc, time::Duration};

/// Errors from this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No Fleet credentials: neither a `cua auth login` session,
    /// `FLEETS_TOKEN` nor `CUA_CLIENT_ID` + `CUA_CLIENT_SECRET`
    /// ([`MISSING_CREDENTIALS`]). Run `cua auth login`, set the client
    /// credentials, or run the sandbox locally.
    #[error("{}", MISSING_CREDENTIALS)]
    MissingCredentials,
    /// A value the SDK checks before calling Fleet is invalid (a name that
    /// is not a DNS label, a bad secret, an unusable spec). The message
    /// names the value; fix it and retry.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The Fleet API call failed: an HTTP status, a transport or token
    /// failure, or an unexpected body ([`SdkError`], listed under Fleet
    /// API client errors).
    #[error(transparent)]
    Sdk(SdkError),
    /// Fleet's admission refused a write: a sandbox size or pool size over
    /// this account's limits, or a template its policy does not admit. The
    /// message is Fleet's own and names the limit. Change the value, or
    /// ask Cua support to raise the account's limits.
    #[error("Fleet refused {operation} (HTTP {status}): {message}")]
    AdmissionDenied {
        /// The refused call (`create template`, `update template`, ...).
        operation: String,
        /// HTTP status (403 from admission, 422 from the CRD schema).
        status: u16,
        /// Fleet's message.
        message: String,
    },
    /// The account is out of Cua Cloud credit: no credit left, and no card
    /// or plan (Fleet answers HTTP 402 `cloud_credit_exhausted`). Running
    /// sandboxes keep running; new ones are refused. Add credit (a plan or
    /// a card for pay as you go) on the page at `billing_url`.
    #[error("{message} Add credit at {billing_url}")]
    CreditExhausted {
        /// Fleet's message ("You're out of Cua Cloud credit.").
        message: String,
        /// The website billing page.
        billing_url: String,
    },
    /// The claim's sandbox does not expose the service. Use one of the
    /// services its template declares (`available`), or add the service
    /// to the pool's template.
    #[error("sandbox {sandbox} exposes no service {service:?} (available: {available:?})")]
    UnknownService {
        /// Sandbox name.
        sandbox: String,
        /// Requested service.
        service: String,
        /// Services the template declares.
        available: Vec<String>,
    },
    /// A wait ran out: a pool that never became ready, or a claim that
    /// never bound. Check the pool's ready replicas and capacity, then
    /// retry with a longer timeout.
    #[error("timed out: {0}")]
    Timeout(String),
    /// The claimed sandbox's cua-spacesd failed or is unreachable.
    #[error(transparent)]
    Env(#[from] cua_spacesd_client::Error),
    /// The runtime or this Fleet release cannot run what the spec asks
    /// (for example `env` on a KubeVirt pool). The message names the
    /// field; drop it or pick a runtime that supports it.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// `CloudOptions(pool=...)` with fields that differ from the pool's
    /// template. Pass `apply=True` to update the template, or omit the
    /// differing fields to use the pool as it is.
    #[error(
        "pool {pool}'s template differs from the requested spec:\n{}\npass apply=True to update \
         the pool's template, or omit these fields to use the pool as it is",
        spec::format_diffs(diffs)
    )]
    PoolSpecMismatch {
        /// Pool name.
        pool: String,
        /// The differing fields.
        diffs: Vec<SpecDiff>,
    },
    /// The claim bound but its per-claim secret (the env token) never
    /// reached the sandbox within the bounded wait; the claim was released.
    /// Retry; if it persists, the pool's runtime is not delivering claim
    /// Secrets.
    #[error(
        "claim {claim} bound but its secrets were not delivered to the sandbox within \
         {waited:?} (runtime {runtime}); the claim was released ({detail})"
    )]
    ClaimSecretsNotDelivered {
        /// Claim name.
        claim: String,
        /// Pool runtime.
        runtime: String,
        /// How long the SDK waited after Bound.
        waited: Duration,
        /// The driver's last answer.
        detail: String,
    },
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Whether this is Fleet saying the resource does not exist: HTTP 404,
    /// or HTTP 403 on a read (`get`/`list`). Live Fleet answers reads in a
    /// deleted pool's namespace with 403, since the namespace is no longer
    /// visible to the caller. Writes answering 403 stay
    /// [`SdkError::PoolAccessDenied`] (a name taken by another account).
    pub fn is_not_found(&self) -> bool {
        match self {
            Error::Sdk(e) => sdk_error_is_not_found(e),
            Error::UnknownService { .. } => true,
            _ => false,
        }
    }
}

impl From<SdkError> for Error {
    /// A Fleet admission refusal becomes [`Error::AdmissionDenied`] with
    /// Fleet's message (never the pool-name hint of
    /// [`SdkError::PoolAccessDenied`]); everything else stays
    /// [`Error::Sdk`].
    fn from(e: SdkError) -> Self {
        if let Some((message, billing_url)) = credit_exhausted(&e) {
            return Error::CreditExhausted {
                message,
                billing_url,
            };
        }
        match admission_denial(&e) {
            Some((operation, status, message)) => Error::AdmissionDenied {
                operation,
                status,
                message,
            },
            None => Error::Sdk(e),
        }
    }
}

/// Fleet refusing a write because the account is out of credit: HTTP 402
/// with `code: cloud_credit_exhausted`, as (message, billing_url).
fn credit_exhausted(e: &SdkError) -> Option<(String, String)> {
    let (status, body) = match e {
        SdkError::Status { status, body, .. } | SdkError::PoolAccessDenied { status, body, .. } => {
            (*status, body)
        }
        _ => return None,
    };
    if status != 402 {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    (v["code"].as_str() == Some("cloud_credit_exhausted")).then(|| {
        (
            v["error"]
                .as_str()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or("You're out of Cua Cloud credit.")
                .trim()
                .to_string(),
            v["billing_url"].as_str().unwrap_or_default().to_string(),
        )
    })
}

/// Fleet's admission refusing a write, as (operation, status, message).
///
/// - HTTP 403 on a template write (`create template`, `update template`)
///   whose body carries a reason: Fleet's policy admission (size limits,
///   process and sidecar rules) answers `{"error": "<reason>"}`. The
///   template namespace already belongs to the caller (its pool was written
///   first), so this is never a pool name taken by another account; a bare
///   `forbidden` stays a plain status.
/// - HTTP 422 on any write: the CRD schema refused a value (a Kubernetes
///   `Status` whose `message` names the field).
fn admission_denial(e: &SdkError) -> Option<(String, u16, String)> {
    let (operation, status, body) = match e {
        SdkError::Status {
            operation,
            status,
            body,
        }
        | SdkError::PoolAccessDenied {
            operation,
            status,
            body,
            ..
        } => (operation, *status, body),
        _ => return None,
    };
    let write = !(operation.starts_with("get ") || operation.starts_with("list "));
    let template_write = operation.ends_with(" template") && write;
    let message = server_message(body)?;
    match status {
        403 if template_write && !message.eq_ignore_ascii_case("forbidden") => {}
        422 if write => {}
        _ => return None,
    }
    Some((operation.clone(), status, message))
}

/// The human message of a Fleet error body: `error`, else `message`, else
/// the body itself when it is not JSON.
fn server_message(body: &str) -> Option<String> {
    let body = body.trim();
    let text = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => ["error", "message"]
            .iter()
            .find_map(|k| v[*k].as_str().map(str::to_string))?,
        Err(_) => body.to_string(),
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// [`Error::is_not_found`] for a bare [`SdkError`].
pub fn sdk_error_is_not_found(e: &SdkError) -> bool {
    match e {
        SdkError::Status { status: 404, .. } => true,
        SdkError::Status {
            status: 403,
            operation,
            ..
        } => operation.starts_with("get ") || operation.starts_with("list "),
        _ => false,
    }
}

struct StaticToken(String);

#[async_trait::async_trait]
impl AccessTokenProvider for StaticToken {
    async fn get_access_token(
        &self,
        _force_refresh: bool,
    ) -> std::result::Result<String, AccessTokenProviderError> {
        Ok(self.0.clone())
    }
}

/// A Fleet client. Cheap to clone.
#[derive(Clone)]
pub struct FleetClient {
    sdk: Arc<CyclopsClient>,
    config: FleetConfig,
    /// How `acquire` waits for claim secrets (boxed: the client stays small).
    claim_wait: Arc<ClaimSecretsWait>,
    /// (namespace, claim) whose `cua-claim-*` Secret this client wrote.
    secret_claims: Arc<std::sync::Mutex<std::collections::HashSet<(String, String)>>>,
}

impl std::fmt::Debug for FleetClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FleetClient")
            .field("base_url", &self.config.base_url)
            .finish()
    }
}

impl FleetClient {
    /// Connects with `FleetConfig::from_env()` and the native HTTP client.
    pub fn from_env() -> Result<Self> {
        Self::connect(FleetConfig::from_env())
    }

    /// Connects with the native HTTP client.
    pub fn connect(config: FleetConfig) -> Result<Self> {
        cua_spacesd_client::transport::ensure_crypto_provider();
        Self::build(config, None)
    }

    /// Connects with a caller-supplied HTTP client (tests, foreign hosts).
    pub fn connect_with_http_client(
        config: FleetConfig,
        http: Arc<dyn HttpClient>,
    ) -> Result<Self> {
        Self::build(config, Some(http))
    }

    /// Connects with a caller-owned bearer source (for example a signed-in
    /// user's refreshing OAuth token) instead of the config's credentials.
    /// `config` still supplies the base URL and poll settings; its
    /// credentials are ignored. `http: None` uses the native HTTP client.
    pub fn connect_with_token_provider(
        config: FleetConfig,
        provider: Arc<dyn AccessTokenProvider>,
        http: Option<Arc<dyn HttpClient>>,
    ) -> Result<Self> {
        cua_spacesd_client::transport::ensure_crypto_provider();
        let cfg = CyclopsTokenProviderConfiguration {
            base_url: config.base_url.clone(),
            pool_poll_interval_ms: config.pool_poll_interval_ms,
            pool_poll_limit: config.pool_poll_limit,
            claim_poll_interval_ms: config.claim_poll_interval_ms,
            claim_poll_limit: config.claim_poll_limit,
        };
        let sdk = match http {
            Some(h) => CyclopsClient::connect_with_access_token_provider(cfg, provider, h)?,
            None => CyclopsClient::connect_with_access_token_provider_and_native_http_client(
                cfg, provider,
            )?,
        };
        Ok(Self::assemble(sdk, config))
    }

    fn assemble(sdk: Arc<CyclopsClient>, config: FleetConfig) -> Self {
        Self {
            sdk,
            config,
            claim_wait: Arc::new(ClaimSecretsWait::default()),
            secret_claims: Arc::default(),
        }
    }

    fn build(config: FleetConfig, http: Option<Arc<dyn HttpClient>>) -> Result<Self> {
        let sdk = if let Some(token) = &config.fleet_token {
            let cfg = CyclopsTokenProviderConfiguration {
                base_url: config.base_url.clone(),
                pool_poll_interval_ms: config.pool_poll_interval_ms,
                pool_poll_limit: config.pool_poll_limit,
                claim_poll_interval_ms: config.claim_poll_interval_ms,
                claim_poll_limit: config.claim_poll_limit,
            };
            let provider: Arc<dyn AccessTokenProvider> = Arc::new(StaticToken(token.clone()));
            match http {
                Some(h) => CyclopsClient::connect_with_access_token_provider(cfg, provider, h)?,
                None => CyclopsClient::connect_with_access_token_provider_and_native_http_client(
                    cfg, provider,
                )?,
            }
        } else {
            let (Some(id), Some(secret)) = (&config.client_id, &config.client_secret) else {
                return Err(Error::MissingCredentials);
            };
            let cfg = CyclopsConfiguration {
                base_url: config.base_url.clone(),
                token_url: config.token_url.clone(),
                credentials: CyclopsCredentials::new(id.clone(), secret.clone()),
                pool_poll_interval_ms: config.pool_poll_interval_ms,
                pool_poll_limit: config.pool_poll_limit,
                claim_poll_interval_ms: config.claim_poll_interval_ms,
                claim_poll_limit: config.claim_poll_limit,
            };
            match http {
                Some(h) => CyclopsClient::connect(cfg, h)?,
                None => CyclopsClient::connect_with_native_http_client(cfg)?,
            }
        };
        Ok(Self::assemble(sdk, config))
    }

    /// The underlying `cyclops-sdk` client (full API).
    pub fn sdk(&self) -> Arc<CyclopsClient> {
        Arc::clone(&self.sdk)
    }

    /// Configuration.
    pub fn config(&self) -> &FleetConfig {
        &self.config
    }

    /// Current bearer (refreshing when `force_refresh`).
    pub async fn access_token(&self, force_refresh: bool) -> Result<String> {
        Ok(self.sdk().access_token(force_refresh).await?)
    }

    // ------------------------------------------------------------- services

    /// `https://<fleet>/api/svc/<namespace>/<sandbox>-<service>` (no trailing
    /// slash), after checking the sandbox declares the service.
    pub fn service_url(&self, sandbox: &BoundSandbox, service: &str) -> Result<String> {
        check_service(sandbox, service)?;
        Ok(format!(
            "{}/api/svc/{}/{}-{}",
            self.config.base_url.trim_end_matches('/'),
            sandbox.namespace,
            sandbox.name,
            service
        ))
    }

    /// An HTTP request to `path` on a sandbox service through the gateway
    /// (adds the Fleet bearer and `X-Cua-Fleet-Claim`).
    pub async fn service_request(
        &self,
        sandbox: &BoundSandbox,
        service: &str,
        path: &str,
        method: &str,
        body: Option<Vec<u8>>,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse> {
        self.service_request_with_headers(sandbox, service, path, method, &[], body, timeout)
            .await
    }

    /// [`Self::service_request`] with caller headers (for example
    /// `content-type`, or `accept` and `mcp-session-id` for MCP over
    /// streamable HTTP). A caller `accept` replaces the default `*/*`. The
    /// gateway owns `authorization` and the claim header, so those are
    /// refused.
    #[allow(clippy::too_many_arguments)]
    pub async fn service_request_with_headers(
        &self,
        sandbox: &BoundSandbox,
        service: &str,
        path: &str,
        method: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        timeout: Option<Duration>,
    ) -> Result<HttpResponse> {
        check_service(sandbox, service)?;
        let mut all = Vec::with_capacity(headers.len() + 1);
        if !headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("accept"))
        {
            all.push(HttpHeader {
                name: "accept".into(),
                value: "*/*".into(),
            });
        }
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("authorization")
                || name.eq_ignore_ascii_case("x-cua-fleet-claim")
            {
                return Err(Error::InvalidArgument(format!(
                    "header {name:?} is set by the Fleet gateway client"
                )));
            }
            all.push(HttpHeader {
                name: name.clone(),
                value: value.clone(),
            });
        }
        let request = HttpRequest {
            method: method.into(),
            url: format!("https://service.invalid{path}"),
            headers: all,
            body,
            timeout_secs: timeout.map(|t| t.as_secs().max(1)),
            max_response_bytes: Some(64 * 1024 * 1024),
        };
        Ok(self
            .sdk()
            .service_request(sandbox.clone(), service.into(), path.into(), request)
            .await?)
    }

    /// Connect options for the spacesd behind `service` (normally
    /// `"env"`) of a claimed sandbox: the Fleet gateway URL, gRPC-Web, the
    /// refreshing Fleet bearer and the claim header. `env_token` is the
    /// spacesd token, if the image has one.
    pub fn env_connect_options(
        &self,
        sandbox: &BoundSandbox,
        service: &str,
        env_token: Option<String>,
    ) -> Result<cua_spacesd_client::ConnectOptions> {
        let endpoint = cua_spacesd_client::Endpoint::parse(&self.service_url(sandbox, service)?)?;
        let sdk = self.sdk();
        let bearer = move |force: bool| {
            let sdk = Arc::clone(&sdk);
            async move {
                sdk.access_token(force)
                    .await
                    .map_err(|e| Box::new(e) as cua_spacesd_client::error::BoxError)
            }
        };
        let mut opts = cua_spacesd_client::ConnectOptions::new(endpoint)
            .transport(cua_spacesd_client::TransportPreference::GrpcWeb)
            .fleet_gateway(Arc::new(bearer), Some(sandbox.claim.clone()));
        opts.token = env_token;
        Ok(opts)
    }

    /// Connects a [`cua_spacesd_client::SpacesdClient`] to a claimed sandbox's env service.
    pub async fn spacesd(
        &self,
        sandbox: &BoundSandbox,
        service: &str,
        env_token: Option<String>,
    ) -> Result<cua_spacesd_client::SpacesdClient> {
        let opts = self.env_connect_options(sandbox, service, env_token)?;
        Ok(cua_spacesd_client::SpacesdClient::connect(opts).await?)
    }

    // --------------------------------------------------------------- images

    /// Lists image resources in a namespace.
    pub async fn list_images(&self, namespace: &str) -> Result<Vec<serde_json::Value>> {
        Ok(self
            .sdk()
            .list_images(namespace.into())
            .await?
            .into_iter()
            .map(|p| p.as_value().clone())
            .collect())
    }

    /// Gets one image resource.
    pub async fn get_image(&self, namespace: &str, name: &str) -> Result<serde_json::Value> {
        Ok(self
            .sdk()
            .get_image(namespace.into(), name.into())
            .await?
            .as_value()
            .clone())
    }

    /// Creates an image resource (remote build) from a manifest whose
    /// `metadata.namespace` must equal `namespace`.
    pub async fn create_image(
        &self,
        namespace: &str,
        manifest: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let manifest = PreservedJson::from_json(manifest.to_string())
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        Ok(self
            .sdk()
            .create_image(namespace.into(), manifest)
            .await?
            .as_value()
            .clone())
    }

    /// Deletes an image resource.
    pub async fn delete_image(&self, namespace: &str, name: &str) -> Result<()> {
        Ok(self
            .sdk()
            .delete_image(namespace.into(), name.into())
            .await?)
    }

    /// Uploads a file for an image build (presigned PUT).
    pub async fn upload_image_file(
        &self,
        namespace: &str,
        name: &str,
        contents: Vec<u8>,
    ) -> Result<cyclops_sdk::ImageUploadInstruction> {
        Ok(self
            .sdk()
            .upload_image_file(namespace.into(), name.into(), contents)
            .await?)
    }

    /// Mints a signed, shareable URL for a sandbox service.
    pub async fn create_signed_service_url(
        &self,
        sandbox: &BoundSandbox,
        service: &str,
        label: Option<String>,
        expires_in: Duration,
    ) -> Result<cyclops_sdk::SignedServiceUrl> {
        Ok(self
            .sdk()
            .create_signed_service_url(cyclops_sdk::CreateSignedServiceUrlRequest {
                sandbox: sandbox.clone(),
                service: service.into(),
                label,
                expires_in_seconds: expires_in.as_secs().min(u32::MAX as u64) as u32,
            })
            .await?)
    }

    /// Lists signed URLs of a claim.
    pub async fn list_signed_service_urls(
        &self,
        sandbox: &BoundSandbox,
    ) -> Result<Vec<cyclops_sdk::SignedServiceUrl>> {
        Ok(self.sdk().list_signed_service_urls(sandbox.clone()).await?)
    }

    /// Revokes a signed URL.
    pub async fn revoke_signed_service_url(
        &self,
        url: cyclops_sdk::SignedServiceUrl,
    ) -> Result<()> {
        Ok(self.sdk().revoke_signed_service_url(url).await?)
    }
}

fn check_service(sandbox: &BoundSandbox, service: &str) -> Result<()> {
    if sandbox.services.iter().any(|s| s == service) {
        Ok(())
    } else {
        Err(Error::UnknownService {
            sandbox: sandbox.name.clone(),
            service: service.into(),
            available: sandbox.services.clone(),
        })
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    fn status(op: &str, code: u16, body: &str) -> Error {
        SdkError::status(op, code, body.as_bytes()).into()
    }

    #[test]
    fn template_admission_denials_are_typed() {
        let e = status(
            "create template",
            403,
            r#"{"error":"sandbox size is over"}"#,
        );
        assert!(
            matches!(&e, Error::AdmissionDenied { message, status: 403, .. } if message == "sandbox size is over"),
            "{e:?}"
        );
        // The typed mirror wraps template 403s as a pool access denial.
        let e: Error = SdkError::PoolAccessDenied {
            operation: "update template".into(),
            namespace: "p".into(),
            status: 403,
            body: r#"{"code":"not_admin","message":"too big"}"#.into(),
        }
        .into();
        assert!(matches!(&e, Error::AdmissionDenied { message, .. } if message == "too big"));
        // The CRD schema (422) on any write.
        let e = status(
            "patch pool",
            422,
            r#"{"kind":"Status","message":"spec.autoscaling.maxPoolSize: Invalid value: 51"}"#,
        );
        assert!(
            matches!(&e, Error::AdmissionDenied { status: 422, .. }),
            "{e:?}"
        );
    }

    #[test]
    fn other_denials_stay_plain() {
        for e in [
            status("create template", 403, r#"{"error":"forbidden"}"#),
            status("create template", 403, ""),
            status("create pool", 403, r#"{"error":"forbidden by capsule"}"#),
            status("get template", 403, r#"{"error":"hidden"}"#),
            status("get pool", 422, r#"{"message":"x"}"#),
            status("create template", 500, r#"{"error":"boom"}"#),
        ] {
            assert!(matches!(e, Error::Sdk(_)), "{e:?}");
        }
    }
}
