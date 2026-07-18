use cyclops_sdk_core::{
    AccessToken, Configuration as CoreConfiguration, HttpRequest, HttpResponse, OAuthCredentials,
    Platform, PlatformError,
};
use std::time::Duration;

pub use cyclops_sdk_core::generated_crd::*;
pub use cyclops_sdk_core::{
    Claim, CreateClaimRequest, CreatePoolRequest, Pool, Sandbox, SdkError, ServiceHttpRequest,
    ServiceHttpResponse,
};

#[derive(Debug, Clone)]
pub struct OAuthConfiguration {
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
}
#[derive(Debug, Clone)]
pub struct Configuration {
    pub base_url: String,
    pub oauth: OAuthConfiguration,
}
#[derive(Default)]
struct NativePlatform {
    oauth: Option<OAuthCredentials>,
    token: Option<AccessToken>,
}
pub struct Sdk {
    core: cyclops_sdk_core::Sdk<NativePlatform>,
}
pub struct ServiceClient<'a> {
    core: &'a mut cyclops_sdk_core::Sdk<NativePlatform>,
    sandbox: Sandbox,
    service: String,
}
pub struct ServiceRequestBuilder<'client, 'sdk> {
    client: &'client mut ServiceClient<'sdk>,
    request: ServiceHttpRequest,
}

impl<'sdk> ServiceClient<'sdk> {
    pub fn request<'client>(
        &'client mut self,
        method: impl Into<String>,
        path: impl Into<String>,
    ) -> ServiceRequestBuilder<'client, 'sdk> {
        ServiceRequestBuilder {
            client: self,
            request: ServiceHttpRequest::new(method, path, None),
        }
    }
    pub fn get<'client>(
        &'client mut self,
        path: impl Into<String>,
    ) -> ServiceRequestBuilder<'client, 'sdk> {
        self.request("GET", path)
    }
    pub fn post<'client>(
        &'client mut self,
        path: impl Into<String>,
    ) -> ServiceRequestBuilder<'client, 'sdk> {
        self.request("POST", path)
    }
}

impl ServiceRequestBuilder<'_, '_> {
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.request.headers.push((name.into(), value.into()));
        self
    }
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.request.body = Some(body.into());
        self
    }
    pub fn send(self) -> Result<ServiceHttpResponse, SdkError> {
        self.client
            .core
            .service_request(&self.client.sandbox, &self.client.service, self.request)
    }
}

impl Sdk {
    pub fn connect(configuration: Configuration) -> Result<Self, String> {
        let platform = NativePlatform {
            oauth: Some(OAuthCredentials {
                token_url: configuration.oauth.token_url,
                client_id: configuration.oauth.client_id,
                client_secret: configuration.oauth.client_secret,
            }),
            token: None,
        };
        let core = cyclops_sdk_core::Sdk::connect(
            platform,
            CoreConfiguration {
                protocol_version: cyclops_sdk_core::PROTOCOL_VERSION,
                base_url: configuration.base_url,
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(Self { core })
    }
    pub fn create_pool(&mut self, request: CreatePoolRequest) -> Result<Pool, String> {
        self.core
            .create_pool(request)
            .map_err(|error| error.to_string())
    }
    pub fn list_pools(&mut self, namespace: &str) -> Result<Vec<Pool>, String> {
        self.core
            .list_pools(namespace)
            .map_err(|error| error.to_string())
    }
    pub fn get_pool(&mut self, pool: Pool) -> Result<Pool, String> {
        self.core.get_pool(pool).map_err(|error| error.to_string())
    }
    pub fn update_pool(&mut self, pool: Pool) -> Result<Pool, String> {
        self.core
            .update_pool(pool)
            .map_err(|error| error.to_string())
    }
    pub fn delete_pool(&mut self, pool: Pool) -> Result<(), String> {
        self.core
            .delete_pool(pool)
            .map_err(|error| error.to_string())
    }
    pub fn create_claim(&mut self, request: CreateClaimRequest) -> Result<Claim, String> {
        self.core
            .create_claim(request)
            .map_err(|error| error.to_string())
    }
    pub fn list_claims(&mut self, namespace: &str) -> Result<Vec<Claim>, String> {
        self.core
            .list_claims(namespace)
            .map_err(|error| error.to_string())
    }
    pub fn get_claim(&mut self, claim: Claim) -> Result<Claim, String> {
        self.core
            .get_claim(claim)
            .map_err(|error| error.to_string())
    }
    pub fn update_claim(&mut self, claim: Claim) -> Result<Claim, String> {
        self.core
            .update_claim(claim)
            .map_err(|error| error.to_string())
    }
    pub fn delete_claim(&mut self, claim: Claim) -> Result<(), String> {
        self.core
            .delete_claim(claim)
            .map_err(|error| error.to_string())
    }
    pub fn wait_claim(&mut self, claim: Claim) -> Result<Sandbox, String> {
        self.core
            .wait_claim(claim)
            .map_err(|error| error.to_string())
    }
    pub fn service_client<'a>(
        &'a mut self,
        sandbox: &Sandbox,
        service: &str,
    ) -> Result<ServiceClient<'a>, SdkError> {
        if !sandbox
            .services
            .iter()
            .any(|available| available == service)
        {
            let mut available = sandbox.services.clone();
            available.sort();
            available.dedup();
            return Err(SdkError::UnknownService {
                requested: service.into(),
                available,
            });
        }
        Ok(ServiceClient {
            core: &mut self.core,
            sandbox: sandbox.clone(),
            service: service.into(),
        })
    }
    pub fn close(self) {}
}

fn send_without_body(
    mut request: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    headers: Vec<(String, String)>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    for (name, value) in headers {
        request = request.header(&name, &value);
    }
    request.call()
}
fn send_with_body(
    mut request: ureq::RequestBuilder<ureq::typestate::WithBody>,
    headers: Vec<(String, String)>,
    body: Option<String>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    for (name, value) in headers {
        request = request.header(&name, &value);
    }
    match body {
        Some(body) => request.send(body),
        None => request.send_empty(),
    }
}
impl Platform for NativePlatform {
    fn http(&mut self, request: HttpRequest) -> Result<HttpResponse, PlatformError> {
        let response = match request.method.as_str() {
            "GET" => send_without_body(ureq::get(&request.url), request.headers),
            "DELETE" => send_without_body(ureq::delete(&request.url), request.headers),
            "HEAD" => send_without_body(ureq::head(&request.url), request.headers),
            "OPTIONS" => send_without_body(ureq::options(&request.url), request.headers),
            "POST" => send_with_body(ureq::post(&request.url), request.headers, request.body),
            "PUT" => send_with_body(ureq::put(&request.url), request.headers, request.body),
            "PATCH" => send_with_body(ureq::patch(&request.url), request.headers, request.body),
            method => {
                return Err(PlatformError::Transport(format!(
                    "unsupported HTTP method {method}"
                )))
            }
        };
        match response {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let body = response
                    .body_mut()
                    .read_to_string()
                    .map_err(|error| PlatformError::Transport(error.to_string()))?;
                Ok(HttpResponse { status, body })
            }
            Err(ureq::Error::StatusCode(status)) => Ok(HttpResponse {
                status,
                body: String::new(),
            }),
            Err(error) => Err(PlatformError::Transport(error.to_string())),
        }
    }
    fn sleep(&mut self, milliseconds: u64) -> Result<(), PlatformError> {
        std::thread::sleep(Duration::from_millis(milliseconds));
        Ok(())
    }
    fn now_ms(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
    fn acquire_oauth_credentials(&mut self) -> Result<OAuthCredentials, PlatformError> {
        self.oauth
            .clone()
            .ok_or_else(|| PlatformError::Credentials("OAuth is not configured".into()))
    }
    fn load_access_token(&mut self) -> Result<Option<AccessToken>, PlatformError> {
        Ok(self.token.clone())
    }
    fn store_access_token(&mut self, token: &AccessToken) -> Result<(), PlatformError> {
        self.token = Some(token.clone());
        Ok(())
    }
}
