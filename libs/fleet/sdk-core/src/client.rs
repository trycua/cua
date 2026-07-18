use crate::{
    validate_configuration, validate_name, Claim, ClaimSpec, Configuration, CreateClaimRequest,
    CreatePoolRequest, Pool, ResourceList, ResourceMetadata, Sandbox, SandboxTemplateRef,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;
use url::{form_urlencoded, Url as ParsedUrl};

const TOKEN_EXPIRY_SKEW_MS: u64 = 30_000;
const POOL_POLL_INTERVAL_MS: u64 = 5_000;
const POOL_MAX_POLL_ATTEMPTS: u32 = 180;
const CLAIM_POLL_INTERVAL_MS: u64 = 5_000;
const CLAIM_MAX_POLL_ATTEMPTS: u32 = 180;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self::new("GET", url, None)
    }
    pub fn post(url: impl Into<String>, body: impl Into<String>) -> Self {
        Self::new("POST", url, Some(body.into()))
    }
    pub fn put(url: impl Into<String>, body: impl Into<String>) -> Self {
        Self::new("PUT", url, Some(body.into()))
    }
    pub fn delete(url: impl Into<String>) -> Self {
        Self::new("DELETE", url, None)
    }
    pub fn new(method: impl Into<String>, url: impl Into<String>, body: Option<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            headers: vec![
                ("accept".into(), "application/json".into()),
                ("content-type".into(), "application/json".into()),
            ],
            body,
        }
    }
    fn with_bearer(mut self, token: &str) -> Self {
        self.headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
        self.headers
            .push(("authorization".into(), format!("Bearer {token}")));
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceHttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl ServiceHttpRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self::new("GET", path, None)
    }
    pub fn post(path: impl Into<String>, body: impl Into<String>) -> Self {
        Self::new("POST", path, Some(body.into()))
    }
    pub fn new(method: impl Into<String>, path: impl Into<String>, body: Option<String>) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            headers: Vec::new(),
            body,
        }
    }
}

pub type ServiceHttpResponse = HttpResponse;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthCredentials {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessToken {
    pub value: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, Error, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlatformError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("clock error: {0}")]
    Clock(String),
    #[error("credential error: {0}")]
    Credentials(String),
    #[error("token storage error: {0}")]
    TokenStorage(String),
}

pub trait Platform {
    fn http(&mut self, request: HttpRequest) -> Result<HttpResponse, PlatformError>;
    fn sleep(&mut self, milliseconds: u64) -> Result<(), PlatformError>;
    fn now_ms(&mut self) -> u64;
    fn acquire_oauth_credentials(&mut self) -> Result<OAuthCredentials, PlatformError>;
    fn load_access_token(&mut self) -> Result<Option<AccessToken>, PlatformError>;
    fn store_access_token(&mut self, token: &AccessToken) -> Result<(), PlatformError>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SdkError {
    #[error("invalid configuration: {0}")]
    Configuration(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("token error: {0}")]
    Token(String),
    #[error("response body error: {0}")]
    Body(String),
    #[error("{operation} returned HTTP {status}: {body}")]
    Status {
        operation: String,
        status: u16,
        body: String,
    },
    #[error("pool entered terminal phase {phase}: {status}")]
    PoolFailed { phase: String, status: String },
    #[error("pool did not become available before the polling limit")]
    PoolTimeout,
    #[error("claim entered terminal phase {phase}: {status}")]
    ClaimFailed { phase: String, status: String },
    #[error("claim did not bind before the polling limit")]
    ClaimTimeout,
    #[error("unknown sandbox service {requested:?}; available services: {available:?}")]
    UnknownService {
        requested: String,
        available: Vec<String>,
    },
    #[error("service request path must be relative and start with '/': {0}")]
    InvalidServicePath(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url(String);
impl Url {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolUrl {
    collection: Url,
    item: Url,
}
impl PoolUrl {
    pub fn new(base: &str, namespace: &str, name: &str) -> Result<Self, SdkError> {
        validate_url_parts(base, namespace, name)?;
        let collection = format!(
            "{}/api/k8s/apis/cua.ai/v1/namespaces/{namespace}/osgymworkspacepools",
            base.trim_end_matches('/')
        );
        Ok(Self {
            item: Url(format!("{collection}/{name}")),
            collection: Url(collection),
        })
    }
    pub fn collection(&self) -> &Url {
        &self.collection
    }
    pub fn item(&self) -> &Url {
        &self.item
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimUrl {
    collection: Url,
    item: Url,
}
impl ClaimUrl {
    pub fn new(base: &str, namespace: &str, name: &str) -> Result<Self, SdkError> {
        validate_url_parts(base, namespace, name)?;
        let collection = format!(
            "{}/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/{namespace}/osgymsandboxclaims",
            base.trim_end_matches('/')
        );
        Ok(Self {
            item: Url(format!("{collection}/{name}")),
            collection: Url(collection),
        })
    }
    pub fn collection(&self) -> &Url {
        &self.collection
    }
    pub fn item(&self) -> &Url {
        &self.item
    }
}

pub struct Sdk<P> {
    platform: P,
    configuration: Configuration,
    next_claim_id: u64,
    claim_services: BTreeMap<String, Vec<String>>,
}

impl<P: Platform> Sdk<P> {
    pub fn connect(platform: P, configuration: Configuration) -> Result<Self, SdkError> {
        validate_configuration(&configuration)
            .map_err(|error| SdkError::Configuration(error.to_string()))?;
        Ok(Self {
            platform,
            configuration,
            next_claim_id: 0,
            claim_services: BTreeMap::new(),
        })
    }
    pub fn platform(&self) -> &P {
        &self.platform
    }
    pub fn platform_mut(&mut self) -> &mut P {
        &mut self.platform
    }
    pub fn into_platform(self) -> P {
        self.platform
    }
    pub fn service_request(
        &mut self,
        sandbox: &Sandbox,
        service: &str,
        request: ServiceHttpRequest,
    ) -> Result<ServiceHttpResponse, SdkError> {
        validate_service(sandbox, service)?;
        let url = service_url(
            &self.configuration.base_url,
            sandbox,
            service,
            &request.path,
        )?;
        let mut headers = request.headers;
        headers.retain(|(name, _)| !is_transport_owned_header(name));
        self.send_raw(HttpRequest {
            method: request.method,
            url,
            headers,
            body: request.body,
        })
    }

    pub fn create_pool(&mut self, request: CreatePoolRequest) -> Result<Pool, SdkError> {
        validate_name(&request.namespace)
            .map_err(|error| SdkError::Configuration(error.to_string()))?;
        let namespace = request.namespace.clone();
        let namespace_created = self.ensure_namespace(&namespace)?;
        let pool = Pool {
            api_version: "cua.ai/v1".into(),
            kind: "OSGymWorkspacePool".into(),
            metadata: ResourceMetadata {
                namespace: namespace.clone(),
                name: request.namespace,
                labels: None,
            },
            spec: serde_json::to_value(request.spec)
                .map_err(|error| SdkError::Body(error.to_string()))?,
            status: None,
        };
        let url = self.pool_url(&pool)?;
        let result = self.send_json(
            "create pool",
            HttpRequest::post(url.collection().as_str(), to_json(&pool)?),
            &[200, 201, 202],
        );
        if result.is_err() && namespace_created {
            let _ = self.delete_namespace(&namespace);
        }
        result
    }
    pub fn list_pools(&mut self, namespace: &str) -> Result<Vec<Pool>, SdkError> {
        let url = PoolUrl::new(&self.configuration.base_url, namespace, "list")?;
        let list: ResourceList<Pool> = self.send_json(
            "list pools",
            HttpRequest::get(url.collection().as_str()),
            &[200],
        )?;
        Ok(list.items)
    }
    pub fn get_pool(&mut self, pool: Pool) -> Result<Pool, SdkError> {
        let url = self.pool_url(&pool)?;
        self.send_json("get pool", HttpRequest::get(url.item().as_str()), &[200])
    }
    pub fn update_pool(&mut self, pool: Pool) -> Result<Pool, SdkError> {
        let url = self.pool_url(&pool)?;
        self.send_json(
            "update pool",
            HttpRequest::put(url.item().as_str(), to_json(&pool)?),
            &[200],
        )
    }
    pub fn delete_pool(&mut self, pool: Pool) -> Result<(), SdkError> {
        let namespace = pool.metadata.namespace.clone();
        let url = self.pool_url(&pool)?;
        self.send_unit(
            "delete pool",
            HttpRequest::delete(url.item().as_str()),
            &[200, 202, 204, 404],
        )?;
        self.delete_namespace(&namespace)
    }
    pub fn create_claim(&mut self, request: CreateClaimRequest) -> Result<Claim, SdkError> {
        let CreateClaimRequest { pool, spec } = request;
        let requested_replicas = pool
            .spec
            .get("replicas")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let pool = if requested_replicas > 0 {
            self.wait_pool_ready(pool)?
        } else {
            pool
        };
        let services = service_names(&pool);
        let namespace = pool.metadata.namespace;
        let pool_name = pool.metadata.name;
        validate_name(&namespace).map_err(|error| SdkError::Configuration(error.to_string()))?;
        validate_name(&pool_name).map_err(|error| SdkError::Configuration(error.to_string()))?;
        let spec = match spec {
            Some(spec) => spec,
            None => ClaimSpec::builder()
                .sandbox_template_ref(
                    SandboxTemplateRef::builder()
                        .name(format!("{pool_name}-template"))
                        .build()
                        .map_err(SdkError::Configuration)?,
                )
                .build()
                .map_err(SdkError::Configuration)?,
        };
        validate_name(&spec.sandbox_template_ref.name)
            .map_err(|error| SdkError::Configuration(error.to_string()))?;
        let claim_id = self.next_claim_id;
        self.next_claim_id = self.next_claim_id.wrapping_add(1);
        let claim = Claim {
            api_version: "osgym.cua.ai/v1alpha1".into(),
            kind: "OSGymSandboxClaim".into(),
            metadata: ResourceMetadata {
                namespace,
                name: format!("claim-{:x}-{claim_id:x}", self.platform.now_ms()),
                labels: None,
            },
            spec: serde_json::to_value(spec).map_err(|error| SdkError::Body(error.to_string()))?,
            status: None,
        };
        let url = self.claim_url(&claim)?;
        let created = self.send_json(
            "create claim",
            HttpRequest::post(url.collection().as_str(), to_json(&claim)?),
            &[200, 201, 202],
        )?;
        self.claim_services.insert(claim_key(&created), services);
        Ok(created)
    }
    pub fn list_claims(&mut self, namespace: &str) -> Result<Vec<Claim>, SdkError> {
        let url = ClaimUrl::new(&self.configuration.base_url, namespace, "list")?;
        let list: ResourceList<Claim> = self.send_json(
            "list claims",
            HttpRequest::get(url.collection().as_str()),
            &[200],
        )?;
        Ok(list.items)
    }
    pub fn get_claim(&mut self, claim: Claim) -> Result<Claim, SdkError> {
        let url = self.claim_url(&claim)?;
        self.send_json("get claim", HttpRequest::get(url.item().as_str()), &[200])
    }
    pub fn update_claim(&mut self, claim: Claim) -> Result<Claim, SdkError> {
        let url = self.claim_url(&claim)?;
        self.send_json(
            "update claim",
            HttpRequest::put(url.item().as_str(), to_json(&claim)?),
            &[200],
        )
    }
    pub fn delete_claim(&mut self, claim: Claim) -> Result<(), SdkError> {
        let key = claim_key(&claim);
        let url = self.claim_url(&claim)?;
        self.send_unit(
            "delete claim",
            HttpRequest::delete(url.item().as_str()),
            &[200, 202, 204, 404],
        )?;
        self.claim_services.remove(&key);
        Ok(())
    }
    pub fn wait_claim(&mut self, claim: Claim) -> Result<Sandbox, SdkError> {
        let services = self.services_for_claim(&claim)?;
        for _ in 0..CLAIM_MAX_POLL_ATTEMPTS {
            let current = self.get_claim(claim.clone())?;
            let phase = current
                .status
                .as_ref()
                .and_then(|status| status.get("phase"))
                .and_then(Value::as_str)
                .unwrap_or("Pending");
            if phase == "Bound" {
                return sandbox_from_claim(&current, services.clone());
            }
            if matches!(phase, "Failed" | "Error" | "Expired") {
                return Err(SdkError::ClaimFailed {
                    phase: phase.into(),
                    status: current
                        .status
                        .as_ref()
                        .map(Value::to_string)
                        .unwrap_or_else(|| "null".into()),
                });
            }
            self.platform
                .sleep(CLAIM_POLL_INTERVAL_MS)
                .map_err(map_platform_error)?;
        }
        Err(SdkError::ClaimTimeout)
    }

    fn services_for_claim(&mut self, claim: &Claim) -> Result<Vec<String>, SdkError> {
        if let Some(services) = self.claim_services.get(&claim_key(claim)) {
            return Ok(services.clone());
        }
        let template = claim
            .spec
            .get("sandboxTemplateRef")
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .ok_or_else(|| SdkError::Body("claim omitted spec.sandboxTemplateRef.name".into()))?;
        let pool_name = template.strip_suffix("-template").ok_or_else(|| {
            SdkError::Body(format!("cannot resolve pool from template {template:?}"))
        })?;
        let url = PoolUrl::new(
            &self.configuration.base_url,
            &claim.metadata.namespace,
            pool_name,
        )?;
        let pool: Pool = self.send_json(
            "get pool services",
            HttpRequest::get(url.item().as_str()),
            &[200],
        )?;
        Ok(service_names(&pool))
    }

    fn wait_pool_ready(&mut self, pool: Pool) -> Result<Pool, SdkError> {
        for _ in 0..POOL_MAX_POLL_ATTEMPTS {
            let current = self.get_pool(pool.clone())?;
            let status = current.status.as_ref();
            let available = status
                .and_then(|value| value.get("availableCount"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if available > 0 {
                return Ok(current);
            }
            let phase = status
                .and_then(|value| value.get("phase"))
                .and_then(Value::as_str)
                .unwrap_or("Provisioning");
            if matches!(phase, "Failed" | "Error") {
                return Err(SdkError::PoolFailed {
                    phase: phase.into(),
                    status: status
                        .map(Value::to_string)
                        .unwrap_or_else(|| "null".into()),
                });
            }
            self.platform
                .sleep(POOL_POLL_INTERVAL_MS)
                .map_err(map_platform_error)?;
        }
        Err(SdkError::PoolTimeout)
    }

    fn ensure_namespace(&mut self, name: &str) -> Result<bool, SdkError> {
        validate_name(name).map_err(|error| SdkError::Configuration(error.to_string()))?;
        let url = format!(
            "{}/api/namespaces",
            self.configuration.base_url.trim_end_matches('/')
        );
        let response = self.send_allowed(
            "create namespace",
            HttpRequest::post(url, to_json(&serde_json::json!({ "name": name }))?),
            &[200, 201, 202, 409],
        )?;
        Ok(response.status != 409)
    }
    fn delete_namespace(&mut self, name: &str) -> Result<(), SdkError> {
        validate_name(name).map_err(|error| SdkError::Configuration(error.to_string()))?;
        let url = format!(
            "{}/api/namespaces/{name}",
            self.configuration.base_url.trim_end_matches('/')
        );
        self.send_unit(
            "delete namespace",
            HttpRequest::delete(url),
            &[200, 202, 204, 404],
        )
    }

    fn pool_url(&self, pool: &Pool) -> Result<PoolUrl, SdkError> {
        PoolUrl::new(
            &self.configuration.base_url,
            &pool.metadata.namespace,
            &pool.metadata.name,
        )
    }
    fn claim_url(&self, claim: &Claim) -> Result<ClaimUrl, SdkError> {
        ClaimUrl::new(
            &self.configuration.base_url,
            &claim.metadata.namespace,
            &claim.metadata.name,
        )
    }
    fn send_json<T: DeserializeOwned>(
        &mut self,
        operation: &str,
        request: HttpRequest,
        allowed: &[u16],
    ) -> Result<T, SdkError> {
        let response = self.send_allowed(operation, request, allowed)?;
        serde_json::from_str(&response.body).map_err(|error| SdkError::Body(error.to_string()))
    }
    fn send_unit(
        &mut self,
        operation: &str,
        request: HttpRequest,
        allowed: &[u16],
    ) -> Result<(), SdkError> {
        self.send_allowed(operation, request, allowed).map(|_| ())
    }
    fn send_allowed(
        &mut self,
        operation: &str,
        request: HttpRequest,
        allowed: &[u16],
    ) -> Result<HttpResponse, SdkError> {
        let response = self.send_raw(request)?;
        if allowed.contains(&response.status) {
            Ok(response)
        } else {
            Err(status_error(operation, response))
        }
    }
    fn send_raw(&mut self, request: HttpRequest) -> Result<HttpResponse, SdkError> {
        if !same_origin(&self.configuration.base_url, &request.url) {
            return self.platform.http(request).map_err(map_platform_error);
        }
        let token = self.access_token(false)?;
        let response = self
            .platform
            .http(request.clone().with_bearer(&token.value))
            .map_err(map_platform_error)?;
        if response.status != 401 {
            return Ok(response);
        }
        let refreshed = self.access_token(true)?;
        self.platform
            .http(request.with_bearer(&refreshed.value))
            .map_err(map_platform_error)
    }
    fn access_token(&mut self, force_refresh: bool) -> Result<AccessToken, SdkError> {
        let now = self.platform.now_ms();
        if !force_refresh {
            if let Some(token) = self
                .platform
                .load_access_token()
                .map_err(map_platform_error)?
            {
                if token.expires_at_ms > now.saturating_add(TOKEN_EXPIRY_SKEW_MS) {
                    return Ok(token);
                }
            }
        }
        let credentials = self
            .platform
            .acquire_oauth_credentials()
            .map_err(map_platform_error)?;
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "client_credentials")
            .append_pair("client_id", &credentials.client_id)
            .append_pair("client_secret", &credentials.client_secret)
            .finish();
        let mut request = HttpRequest::post(credentials.token_url, body);
        request.headers = vec![
            ("accept".into(), "application/json".into()),
            (
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            ),
        ];
        let response = self.platform.http(request).map_err(map_platform_error)?;
        if !(200..300).contains(&response.status) {
            return Err(status_error("acquire OAuth token", response));
        }
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
        }
        let body: TokenResponse = serde_json::from_str(&response.body)
            .map_err(|error| SdkError::Token(error.to_string()))?;
        let token = AccessToken {
            value: body.access_token,
            expires_at_ms: now.saturating_add(body.expires_in.saturating_mul(1_000)),
        };
        self.platform
            .store_access_token(&token)
            .map_err(map_platform_error)?;
        Ok(token)
    }
}

fn same_origin(base: &str, request: &str) -> bool {
    match (ParsedUrl::parse(base), ParsedUrl::parse(request)) {
        (Ok(base), Ok(request)) => base.origin() == request.origin(),
        _ => false,
    }
}
fn validate_url_parts(base: &str, namespace: &str, name: &str) -> Result<(), SdkError> {
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(SdkError::Configuration(
            "base_url must use http or https".into(),
        ));
    }
    validate_name(namespace).map_err(|error| SdkError::Configuration(error.to_string()))?;
    validate_name(name).map_err(|error| SdkError::Configuration(error.to_string()))
}
fn to_json<T: Serialize>(value: &T) -> Result<String, SdkError> {
    serde_json::to_string(value).map_err(|error| SdkError::Body(error.to_string()))
}
fn status_error(operation: &str, response: HttpResponse) -> SdkError {
    SdkError::Status {
        operation: operation.into(),
        status: response.status,
        body: response.body,
    }
}
fn map_platform_error(error: PlatformError) -> SdkError {
    SdkError::Transport(error.to_string())
}
fn claim_key(claim: &Claim) -> String {
    format!("{}/{}", claim.metadata.namespace, claim.metadata.name)
}

fn service_names(pool: &Pool) -> Vec<String> {
    let mut services = pool
        .spec
        .get("services")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|service| service.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    services.sort();
    services.dedup();
    services
}

fn validate_service(sandbox: &Sandbox, service: &str) -> Result<(), SdkError> {
    if sandbox
        .services
        .iter()
        .any(|available| available == service)
    {
        return Ok(());
    }
    let mut available = sandbox.services.clone();
    available.sort();
    available.dedup();
    Err(SdkError::UnknownService {
        requested: service.into(),
        available,
    })
}

fn is_transport_owned_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "connection"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn service_url(
    base_url: &str,
    sandbox: &Sandbox,
    service: &str,
    path: &str,
) -> Result<String, SdkError> {
    validate_url_parts(base_url, &sandbox.namespace, &sandbox.sandbox)?;
    validate_name(service).map_err(|error| SdkError::Configuration(error.to_string()))?;
    if !path.starts_with('/') || path.starts_with("//") || path.contains('#') {
        return Err(SdkError::InvalidServicePath(path.into()));
    }
    Ok(format!(
        "{}/api/svc/{}/{}-{}{}",
        base_url.trim_end_matches('/'),
        sandbox.namespace,
        sandbox.sandbox,
        service,
        path
    ))
}

fn sandbox_from_claim(claim: &Claim, services: Vec<String>) -> Result<Sandbox, SdkError> {
    let status = claim
        .status
        .as_ref()
        .ok_or_else(|| SdkError::Body("bound claim omitted status".into()))?;
    let sandbox = status
        .get("sandbox")
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| SdkError::Body("bound claim omitted status.sandbox.name".into()))?
        .to_string();
    Ok(Sandbox {
        namespace: claim.metadata.namespace.clone(),
        claim: claim.metadata.name.clone(),
        sandbox,
        services,
    })
}
