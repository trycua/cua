use crate::{
    AccessTokenProviderError, CyclopsConfiguration, CyclopsTokenProviderConfiguration, HttpError,
    HttpHeader, HttpRequest, HttpResponse, SdkError,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use url::Url;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

const TOKEN_EXPIRY_SKEW: Duration = Duration::from_secs(30);
const AUTHENTICATED_REQUEST_OPERATION: &str = "authenticated request";
const TOKEN_OPERATION: &str = "acquire OAuth token";
const RESPONSE_BODY_LIMIT_ERROR: &str = "HTTP response exceeds configured size limit";

fn append_response_chunk(
    body: &mut Vec<u8>,
    chunk: &[u8],
    max_response_bytes: Option<u64>,
) -> Result<(), HttpError> {
    if let Some(limit) = max_response_bytes {
        let current = u64::try_from(body.len()).unwrap_or(u64::MAX);
        let chunk_len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        if current
            .checked_add(chunk_len)
            .is_none_or(|size| size > limit)
        {
            return Err(HttpError::Transport {
                reason: RESPONSE_BODY_LIMIT_ERROR.into(),
            });
        }
    }
    body.extend_from_slice(chunk);
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthenticatedRequestClass {
    ControlPlane,
    ServiceProxy,
}

#[cfg(not(target_arch = "wasm32"))]
static NATIVE_HTTP_RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn native_http_runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    match NATIVE_HTTP_RUNTIME
        .get_or_init(|| tokio::runtime::Runtime::new().map_err(|error| error.to_string()))
    {
        Ok(runtime) => Ok(runtime),
        Err(reason) => Err(reason.clone()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct NativeHttpClient {
    client: reqwest::Client,
}
#[cfg(not(target_arch = "wasm32"))]
impl NativeHttpClient {
    pub(crate) fn new() -> Result<Self, SdkError> {
        native_http_runtime().map_err(|reason| SdkError::Configuration {
            reason: format!("could not create native HTTP runtime: {reason}"),
        })?;
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|error| SdkError::Configuration {
                    reason: format!("could not create native HTTP client: {error}"),
                })?,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_request_timeout(
    request: reqwest::RequestBuilder,
    timeout_secs: Option<u64>,
) -> reqwest::RequestBuilder {
    match timeout_secs {
        Some(timeout_secs) => request.timeout(Duration::from_secs(timeout_secs)),
        None => request,
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl HttpClient for NativeHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let HttpRequest {
            method,
            url,
            headers: request_headers,
            body,
            timeout_secs,
            max_response_bytes,
        } = request;
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|error| {
            HttpError::Transport {
                reason: format!("invalid HTTP method: {error}"),
            }
        })?;
        let mut headers = reqwest::header::HeaderMap::new();
        for header in request_headers {
            let name = reqwest::header::HeaderName::from_bytes(header.name.as_bytes()).map_err(
                |error| HttpError::Transport {
                    reason: format!("invalid HTTP header name: {error}"),
                },
            )?;
            let value = reqwest::header::HeaderValue::from_str(&header.value).map_err(|error| {
                HttpError::Transport {
                    reason: format!("invalid HTTP header value: {error}"),
                }
            })?;
            headers.append(name, value);
        }
        let mut native = apply_request_timeout(
            self.client.request(method, url).headers(headers),
            timeout_secs,
        );
        if let Some(body) = body {
            native = native.body(body);
        }
        native_http_runtime()
            .map_err(|reason| HttpError::Transport {
                reason: format!("could not create native HTTP runtime: {reason}"),
            })?
            .spawn(async move {
                let mut response = native.send().await.map_err(|error| HttpError::Transport {
                    reason: format!("native HTTP request failed: {error}"),
                })?;
                let status = response.status().as_u16();
                let headers = response
                    .headers()
                    .iter()
                    .map(|(name, value)| {
                        value
                            .to_str()
                            .map(|value| HttpHeader {
                                name: name.as_str().into(),
                                value: value.into(),
                            })
                            .map_err(|error| HttpError::Transport {
                                reason: format!(
                                    "native HTTP response contained a non-text header: {error}"
                                ),
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut body = Vec::new();
                while let Some(chunk) =
                    response
                        .chunk()
                        .await
                        .map_err(|error| HttpError::Transport {
                            reason: format!("could not read native HTTP response body: {error}"),
                        })?
                {
                    append_response_chunk(&mut body, &chunk, max_response_bytes)?;
                }
                Ok(HttpResponse {
                    status,
                    headers,
                    body,
                })
            })
            .await
            .map_err(|error| HttpError::Transport {
                reason: format!("native HTTP runtime task failed: {error}"),
            })?
    }
}

#[uniffi::export(with_foreign)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait HttpClient: Send + Sync {
    /// Executes an HTTP request. Foreign implementations must enforce
    /// `request.max_response_bytes` while streaming the response body.
    /// Implementations must not follow redirects, retry requests, or add ambient
    /// authentication/cookies. Send only the supplied headers and body; signed
    /// upload requests also use this interface and must not leak credentials.
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError>;
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct BrowserHttpClient;

#[cfg(any(test, target_arch = "wasm32"))]
fn browser_timeout_millis(timeout_secs: Option<u64>) -> Result<Option<u32>, HttpError> {
    timeout_secs
        .map(|seconds| {
            seconds
                .checked_mul(1000)
                .filter(|millis| *millis <= i32::MAX as u64)
                .map(|millis| millis as u32)
                .ok_or_else(|| HttpError::Transport {
                    reason: "HTTP timeout exceeds browser timer limit".into(),
                })
        })
        .transpose()
}

#[cfg(target_arch = "wasm32")]
struct BrowserRequestDeadline {
    controller: web_sys::AbortController,
    _timer: gloo_timers::callback::Timeout,
    completed: bool,
}

#[cfg(target_arch = "wasm32")]
impl BrowserRequestDeadline {
    fn new(
        timeout_secs: Option<u64>,
        init: &web_sys::RequestInit,
    ) -> Result<Option<Self>, HttpError> {
        let Some(millis) = browser_timeout_millis(timeout_secs)? else {
            return Ok(None);
        };
        let controller = web_sys::AbortController::new().map_err(browser_transport_error)?;
        init.set_signal(Some(&controller.signal()));
        let abort = controller.clone();
        let timer = gloo_timers::callback::Timeout::new(millis, move || abort.abort());
        Ok(Some(Self {
            controller,
            _timer: timer,
            completed: false,
        }))
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for BrowserRequestDeadline {
    fn drop(&mut self) {
        if !self.completed {
            self.controller.abort();
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl HttpClient for BrowserHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;

        let max_response_bytes = request.max_response_bytes;
        let init = web_sys::RequestInit::new();
        init.set_method(&request.method);
        init.set_redirect(web_sys::RequestRedirect::Error);
        init.set_credentials(web_sys::RequestCredentials::Omit);
        if let Some(body) = request.body {
            let body = js_sys::Uint8Array::from(body.as_slice());
            init.set_body(&body.into());
        }

        let mut deadline = BrowserRequestDeadline::new(request.timeout_secs, &init)?;
        let browser_request = web_sys::Request::new_with_str_and_init(&request.url, &init)
            .map_err(browser_transport_error)?;
        let headers = browser_request.headers();
        for header in request.headers {
            headers
                .set(&header.name, &header.value)
                .map_err(browser_transport_error)?;
        }

        let window = web_sys::window().ok_or_else(|| HttpError::Transport {
            reason: "browser window is unavailable".into(),
        })?;
        let response = JsFuture::from(window.fetch_with_request(&browser_request))
            .await
            .map_err(browser_transport_error)?
            .dyn_into::<web_sys::Response>()
            .map_err(browser_transport_error)?;
        let body = read_browser_response_body(&response, max_response_bytes).await?;
        if let Some(deadline) = &mut deadline {
            deadline.completed = true;
        }

        Ok(HttpResponse {
            status: response.status(),
            headers: vec![],
            body,
        })
    }
}

#[cfg(target_arch = "wasm32")]
async fn read_browser_response_body(
    response: &web_sys::Response,
    max_response_bytes: Option<u64>,
) -> Result<Vec<u8>, HttpError> {
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    let Some(stream) = response.body() else {
        return Ok(Vec::new());
    };
    let reader = stream
        .get_reader()
        .dyn_into::<web_sys::ReadableStreamDefaultReader>()
        .map_err(|error| browser_transport_error(error.into()))?;
    let mut body = Vec::new();
    loop {
        let result = match JsFuture::from(reader.read()).await {
            Ok(result) => result,
            Err(error) => {
                reader.release_lock();
                return Err(browser_transport_error(error));
            }
        };
        let done = js_sys::Reflect::get(&result, &JsValue::from_str("done"))
            .map_err(browser_transport_error)?
            .as_bool()
            .unwrap_or(false);
        if done {
            reader.release_lock();
            return Ok(body);
        }
        let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))
            .map_err(browser_transport_error)?;
        let chunk = js_sys::Uint8Array::new(&value).to_vec();
        if let Err(error) = append_response_chunk(&mut body, &chunk, max_response_bytes) {
            let _ = JsFuture::from(reader.cancel()).await;
            reader.release_lock();
            return Err(error);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn browser_transport_error(error: wasm_bindgen::JsValue) -> HttpError {
    let reason = error
        .as_string()
        .unwrap_or_else(|| format!("browser fetch failed: {error:?}"));
    HttpError::Transport { reason }
}

#[uniffi::export(with_foreign)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait AccessTokenProvider: Send + Sync {
    async fn get_access_token(
        &self,
        force_refresh: bool,
    ) -> Result<String, AccessTokenProviderError>;
}

pub(crate) struct Transport {
    base_origin: Origin,
    authentication: Authentication,
    http_client: Arc<dyn HttpClient>,
}

enum Authentication {
    ClientCredentials {
        token_url: String,
        client_id: String,
        client_secret: String,
        cached: Mutex<Option<AccessToken>>,
    },
    TokenProvider {
        provider: Arc<dyn AccessTokenProvider>,
    },
    StaticAccessToken {
        value: String,
    },
}

#[derive(Clone)]
struct AccessToken {
    value: String,
    expires_at: Option<Instant>,
    generation: Arc<TokenGeneration>,
}

struct TokenGeneration {
    _identity: u8,
}

#[derive(PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Transport {
    pub(crate) fn new(
        configuration: &CyclopsConfiguration,
        http_client: Arc<dyn HttpClient>,
    ) -> Result<Self, SdkError> {
        Ok(Self {
            base_origin: Origin::from_url(&configuration.base_url)?,
            authentication: Authentication::ClientCredentials {
                token_url: configuration.token_url.clone(),
                client_id: configuration.client_id().to_owned(),
                client_secret: configuration.client_secret().to_owned(),
                cached: Mutex::new(None),
            },
            http_client,
        })
    }

    pub(crate) fn new_with_access_token_provider(
        configuration: &CyclopsTokenProviderConfiguration,
        provider: Arc<dyn AccessTokenProvider>,
        http_client: Arc<dyn HttpClient>,
    ) -> Result<Self, SdkError> {
        Ok(Self {
            base_origin: Origin::from_url(&configuration.base_url)?,
            authentication: Authentication::TokenProvider { provider },
            http_client,
        })
    }

    pub(crate) fn new_with_access_token(
        configuration: &CyclopsTokenProviderConfiguration,
        access_token: String,
        http_client: Arc<dyn HttpClient>,
    ) -> Result<Self, SdkError> {
        let value = access_token.trim();
        if value.is_empty() {
            return Err(SdkError::Token {
                reason: "access token must not be empty".into(),
            });
        }

        Ok(Self {
            base_origin: Origin::from_url(&configuration.base_url)?,
            authentication: Authentication::StaticAccessToken {
                value: value.into(),
            },
            http_client,
        })
    }

    pub(crate) async fn execute_authenticated(
        &self,
        request: HttpRequest,
        request_class: AuthenticatedRequestClass,
    ) -> Result<HttpResponse, SdkError> {
        let response = self
            .execute_authenticated_response(request, request_class)
            .await?;
        match request_class {
            AuthenticatedRequestClass::ControlPlane => self.finish_response(response),
            AuthenticatedRequestClass::ServiceProxy => Ok(response),
        }
    }

    async fn execute_authenticated_response(
        &self,
        request: HttpRequest,
        request_class: AuthenticatedRequestClass,
    ) -> Result<HttpResponse, SdkError> {
        if !self.is_same_origin(&request.url) {
            return self.execute_unchecked(request).await;
        }

        let token = self.access_token().await?;
        let response = self
            .execute_and_check_unauthorized(with_bearer(request.clone(), &token.value))
            .await?;
        if response.status != 401 || request_class == AuthenticatedRequestClass::ServiceProxy {
            return Ok(response);
        }

        let refreshed = self.refresh_after_unauthorized(&token).await?;
        self.execute_and_check_unauthorized(with_bearer(request, &refreshed.value))
            .await
    }

    pub(crate) async fn execute_upload(&self, request: HttpRequest) -> Result<(), SdkError> {
        let response = self
            .http_client
            .execute(request)
            .await
            .map_err(|_| SdkError::Transport {
                reason: "image upload request failed".into(),
            })?;
        if !matches!(response.status, 200 | 201 | 204) {
            return Err(SdkError::Status {
                operation: "upload image file".into(),
                status: response.status,
                body: String::new(),
            });
        }
        Ok(())
    }

    async fn execute_unchecked(&self, request: HttpRequest) -> Result<HttpResponse, SdkError> {
        self.http_client
            .execute(request)
            .await
            .map_err(map_http_error)
    }

    async fn execute_and_check_unauthorized(
        &self,
        request: HttpRequest,
    ) -> Result<HttpResponse, SdkError> {
        self.http_client
            .execute(request)
            .await
            .map_err(map_http_error)
    }

    fn finish_response(&self, response: HttpResponse) -> Result<HttpResponse, SdkError> {
        if (200..300).contains(&response.status) {
            Ok(response)
        } else {
            Err(SdkError::status(
                AUTHENTICATED_REQUEST_OPERATION,
                response.status,
                &response.body,
            ))
        }
    }

    async fn access_token(&self) -> Result<AccessToken, SdkError> {
        match &self.authentication {
            Authentication::ClientCredentials { cached, .. } => {
                let mut cached = cached.lock().await;
                if let Some(token) = cached.as_ref().filter(|token| token.is_valid()) {
                    return Ok(token.clone());
                }

                let token = self.acquire_client_credentials_token().await?;
                *cached = Some(token.clone());
                Ok(token)
            }
            Authentication::TokenProvider { provider } => {
                self.provider_token(provider, false).await
            }
            Authentication::StaticAccessToken { value } => Ok(AccessToken {
                value: value.clone(),
                expires_at: None,
                generation: Arc::new(TokenGeneration { _identity: 0 }),
            }),
        }
    }

    async fn refresh_after_unauthorized(
        &self,
        used_token: &AccessToken,
    ) -> Result<AccessToken, SdkError> {
        match &self.authentication {
            Authentication::ClientCredentials { cached, .. } => {
                let mut cached = cached.lock().await;
                match cached.as_ref() {
                    Some(token) if !token.is_same_generation(used_token) && token.is_valid() => {
                        return Ok(token.clone());
                    }
                    Some(token) if token.is_same_generation(used_token) => *cached = None,
                    _ => {}
                }

                let token = self.acquire_client_credentials_token().await?;
                *cached = Some(token.clone());
                Ok(token)
            }
            Authentication::TokenProvider { provider } => self.provider_token(provider, true).await,
            Authentication::StaticAccessToken { .. } => Err(SdkError::Token {
                reason: "static access token was rejected; create a new client with a fresh token"
                    .into(),
            }),
        }
    }

    async fn provider_token(
        &self,
        provider: &Arc<dyn AccessTokenProvider>,
        force_refresh: bool,
    ) -> Result<AccessToken, SdkError> {
        let value = provider
            .get_access_token(force_refresh)
            .await
            .map_err(|error| SdkError::Token {
                reason: error.to_string(),
            })?;
        let value = value.trim();
        if value.is_empty() {
            return Err(SdkError::Token {
                reason: "access-token provider returned an empty token".into(),
            });
        }

        Ok(AccessToken {
            value: value.into(),
            expires_at: Some(Instant::now()),
            generation: Arc::new(TokenGeneration { _identity: 0 }),
        })
    }

    async fn acquire_client_credentials_token(&self) -> Result<AccessToken, SdkError> {
        let Authentication::ClientCredentials {
            token_url,
            client_id,
            client_secret,
            ..
        } = &self.authentication
        else {
            unreachable!("client-credentials token acquisition requires client credentials")
        };
        let credentials = STANDARD.encode(format!("{client_id}:{client_secret}"));
        let body = "grant_type=client_credentials".as_bytes().to_vec();
        let response = self
            .http_client
            .execute(HttpRequest {
                method: "POST".into(),
                url: token_url.clone(),
                headers: vec![
                    HttpHeader {
                        name: "accept".into(),
                        value: "application/json".into(),
                    },
                    HttpHeader {
                        name: "content-type".into(),
                        value: "application/x-www-form-urlencoded".into(),
                    },
                    HttpHeader {
                        name: "authorization".into(),
                        value: format!("Basic {credentials}"),
                    },
                ],
                body: Some(body),
                timeout_secs: None,
                max_response_bytes: None,
            })
            .await
            .map_err(map_http_error)?;
        if !(200..300).contains(&response.status) {
            return Err(SdkError::status(
                TOKEN_OPERATION,
                response.status,
                &response.body,
            ));
        }

        let token: TokenResponse =
            serde_json::from_slice(&response.body).map_err(|error| SdkError::Token {
                reason: error.to_string(),
            })?;
        if token.access_token.is_empty() {
            return Err(SdkError::Token {
                reason: "OAuth response access_token must not be empty".into(),
            });
        }

        let expires_at = Instant::now()
            .checked_add(Duration::from_secs(token.expires_in))
            .unwrap_or_else(Instant::now);
        Ok(AccessToken {
            value: token.access_token,
            expires_at: Some(expires_at),
            generation: Arc::new(TokenGeneration { _identity: 0 }),
        })
    }

    fn is_same_origin(&self, request_url: &str) -> bool {
        Origin::from_url(request_url).is_ok_and(|origin| origin == self.base_origin)
    }
}

impl AccessToken {
    fn is_same_generation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.generation, &other.generation)
    }

    fn is_valid(&self) -> bool {
        self.expires_at.is_none_or(|expires_at| {
            expires_at
                .checked_sub(TOKEN_EXPIRY_SKEW)
                .is_some_and(|refresh_at| Instant::now() < refresh_at)
        })
    }
}

impl Origin {
    fn from_url(value: &str) -> Result<Self, SdkError> {
        let url = Url::parse(value).map_err(|error| SdkError::Configuration {
            reason: format!("URL must be valid for origin comparison: {error}"),
        })?;
        let host = url.host_str().ok_or_else(|| SdkError::Configuration {
            reason: "URL must include a host for origin comparison".into(),
        })?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| SdkError::Configuration {
                reason: "URL must have an effective port for origin comparison".into(),
            })?;
        Ok(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            port,
        })
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

fn with_bearer(mut request: HttpRequest, token: &str) -> HttpRequest {
    request
        .headers
        .retain(|header| !header.name.eq_ignore_ascii_case("authorization"));
    request.headers.push(HttpHeader {
        name: "authorization".into(),
        value: format!("Bearer {token}"),
    });
    request
}

fn map_http_error(error: HttpError) -> SdkError {
    match error {
        HttpError::Transport { reason } => SdkError::Transport { reason },
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_http_client_tests {
    use super::*;
    use std::{
        io::{ErrorKind, Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::Duration,
    };

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 512];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut chunk).unwrap();
            assert_ne!(read, 0, "client closed before completing its request");
            bytes.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8(bytes).unwrap()
    }

    fn request(method: &str, url: String, max_response_bytes: Option<u64>) -> HttpRequest {
        HttpRequest {
            method: method.into(),
            url,
            headers: vec![],
            body: None,
            timeout_secs: None,
            max_response_bytes,
        }
    }

    async fn execute_response(
        method: &str,
        response: Vec<u8>,
        max_response_bytes: Option<u64>,
    ) -> Result<HttpResponse, HttpError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/sensitive-request?token=secret",
            listener.local_addr().unwrap()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream.write_all(&response).unwrap();
        });
        let result = NativeHttpClient::new()
            .unwrap()
            .execute(request(method, url, max_response_bytes))
            .await;
        server.join().unwrap();
        result
    }

    fn fixed_response(body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn assert_limit_error(error: HttpError) {
        let HttpError::Transport { reason } = error;
        assert_eq!(reason, RESPONSE_BODY_LIMIT_ERROR);
        assert!(!reason.contains("sensitive-request"));
        assert!(!reason.contains("secret-response"));
    }

    #[tokio::test]
    async fn native_transport_preserves_duplicate_headers_and_error_bodies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/native", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream).to_ascii_lowercase();
            assert!(request.contains("x-duplicate: first"));
            assert!(request.contains("x-duplicate: second"));
            stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\nx-duplicate: one\r\nx-duplicate: two\r\ncontent-length: 17\r\nconnection: close\r\n\r\nnative error body").unwrap();
        });
        let response = NativeHttpClient::new()
            .unwrap()
            .execute(HttpRequest {
                method: "GET".into(),
                url,
                headers: vec![
                    HttpHeader {
                        name: "x-duplicate".into(),
                        value: "first".into(),
                    },
                    HttpHeader {
                        name: "x-duplicate".into(),
                        value: "second".into(),
                    },
                ],
                body: None,
                timeout_secs: None,
                max_response_bytes: None,
            })
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(response.status, 500);
        assert_eq!(
            response
                .headers
                .iter()
                .filter(|header| header.name == "x-duplicate")
                .map(|header| header.value.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(response.body, b"native error body");
    }

    #[tokio::test]
    async fn native_transport_does_not_replay_bearers_to_redirect_hosts() {
        let redirected = TcpListener::bind("127.0.0.1:0").unwrap();
        redirected.set_nonblocking(true).unwrap();
        let target = format!("http://{}/redirected", redirected.local_addr().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/initial", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(
                read_request(&mut stream)
                    .to_ascii_lowercase()
                    .contains("authorization: bearer intended-token")
            );
            stream.write_all(format!("HTTP/1.1 302 Found\r\nlocation: {target}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").as_bytes()).unwrap();
        });
        let response = NativeHttpClient::new()
            .unwrap()
            .execute(HttpRequest {
                method: "GET".into(),
                url,
                headers: vec![HttpHeader {
                    name: "authorization".into(),
                    value: "Bearer intended-token".into(),
                }],
                body: None,
                timeout_secs: None,
                max_response_bytes: None,
            })
            .await
            .unwrap();
        server.join().unwrap();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(response.status, 302);
        assert!(matches!(redirected.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
    }

    #[tokio::test]
    async fn native_transport_does_not_redirect_signed_puts() {
        let redirected = TcpListener::bind("127.0.0.1:0").unwrap();
        redirected.set_nonblocking(true).unwrap();
        let target = format!("http://{}/redirected", redirected.local_addr().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/signed?signature=secret",
            listener.local_addr().unwrap()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream).to_ascii_lowercase();
            assert!(request.starts_with("put /signed?signature=secret "));
            assert!(request.contains("x-custom-signed: preserve exactly"));
            assert!(request.contains("content-length: 3"));
            assert!(!request.contains("authorization:"));
            let response = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nlocation: {target}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let response = NativeHttpClient::new()
            .unwrap()
            .execute(HttpRequest {
                method: "PUT".into(),
                url,
                headers: vec![HttpHeader {
                    name: "x-custom-signed".into(),
                    value: "preserve exactly".into(),
                }],
                body: Some(b"abc".to_vec()),
                timeout_secs: Some(5),
                max_response_bytes: Some(4096),
            })
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(response.status, 307);
        assert!(matches!(redirected.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
    }

    #[tokio::test]
    async fn native_transport_enforces_limit_minus_one_exact_and_plus_one() {
        let below = execute_response("GET", fixed_response(b"abc"), Some(4))
            .await
            .unwrap();
        let exact = execute_response("GET", fixed_response(b"abcd"), Some(4))
            .await
            .unwrap();
        let error = execute_response("GET", fixed_response(b"abcde"), Some(4))
            .await
            .unwrap_err();

        assert_eq!(below.body, b"abc");
        assert_eq!(exact.body, b"abcd");
        assert_limit_error(error);
    }

    #[tokio::test]
    async fn native_transport_allows_empty_but_rejects_nonempty_at_zero() {
        let empty = execute_response("GET", fixed_response(b""), Some(0))
            .await
            .unwrap();
        let error = execute_response("GET", fixed_response(b"secret-response"), Some(0))
            .await
            .unwrap_err();

        assert!(empty.body.is_empty());
        assert_limit_error(error);
    }

    #[tokio::test]
    async fn native_transport_enforces_chunked_body_without_content_length() {
        let response = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n".to_vec();
        let error = execute_response("GET", response, Some(3))
            .await
            .unwrap_err();

        assert_limit_error(error);
    }

    #[tokio::test]
    async fn native_transport_drops_an_oversized_stream_without_waiting_for_eof() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/stream", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n2\r\nab\r\n")
                .unwrap();
            stream.flush().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut byte = [0; 1];
            stream.read(&mut byte)
        });

        let error = NativeHttpClient::new()
            .unwrap()
            .execute(request("GET", url, Some(1)))
            .await
            .unwrap_err();

        assert_limit_error(error);
        assert_eq!(server.join().unwrap().unwrap(), 0);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_tests {
    use super::apply_request_timeout;
    use std::time::Duration;

    #[test]
    fn applies_per_request_timeout_only_when_present() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        let default_request = apply_request_timeout(client.get("https://example.com"), None)
            .build()
            .unwrap();
        let overridden_request = apply_request_timeout(client.get("https://example.com"), Some(75))
            .build()
            .unwrap();

        assert_eq!(default_request.timeout(), None);
        assert_eq!(overridden_request.timeout(), Some(&Duration::from_secs(75)));
    }
}

#[cfg(test)]
mod browser_deadline_tests {
    use super::browser_timeout_millis;

    #[test]
    fn browser_deadline_preserves_none_and_converts_explicit_seconds() {
        assert_eq!(browser_timeout_millis(None).unwrap(), None);
        assert_eq!(browser_timeout_millis(Some(0)).unwrap(), Some(0));
        assert_eq!(browser_timeout_millis(Some(30)).unwrap(), Some(30_000));
        assert_eq!(browser_timeout_millis(Some(300)).unwrap(), Some(300_000));
    }

    #[test]
    fn browser_deadline_rejects_timer_overflow() {
        assert_eq!(
            browser_timeout_millis(Some(2_147_483)).unwrap(),
            Some(2_147_483_000)
        );
        assert!(browser_timeout_millis(Some(2_147_484)).is_err());
        assert!(browser_timeout_millis(Some(u64::MAX)).is_err());
    }
}
