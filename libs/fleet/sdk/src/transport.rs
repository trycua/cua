use crate::{CyclopsConfiguration, HttpError, HttpHeader, HttpRequest, HttpResponse, SdkError};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;

const TOKEN_EXPIRY_SKEW: Duration = Duration::from_secs(30);
const AUTHENTICATED_REQUEST_OPERATION: &str = "authenticated request";
const TOKEN_OPERATION: &str = "acquire OAuth token";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthenticatedRequestClass {
    ControlPlane,
    ServiceProxy,
}

#[uniffi::export(with_foreign)]
#[async_trait::async_trait]
pub trait HttpClient: Send + Sync {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError>;
}

pub(crate) struct Transport {
    base_origin: Origin,
    token_url: String,
    client_id: String,
    client_secret: String,
    http_client: Arc<dyn HttpClient>,
    access_token: Mutex<Option<AccessToken>>,
}

#[derive(Clone)]
struct AccessToken {
    value: String,
    expires_at: Instant,
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
            token_url: configuration.token_url.clone(),
            client_id: configuration.client_id().to_owned(),
            client_secret: configuration.client_secret().to_owned(),
            http_client,
            access_token: Mutex::new(None),
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
        let mut cached = self.access_token.lock().await;
        if let Some(token) = cached.as_ref().filter(|token| token.is_valid()) {
            return Ok(token.clone());
        }

        let token = self.acquire_token().await?;
        *cached = Some(token.clone());
        Ok(token)
    }

    async fn refresh_after_unauthorized(
        &self,
        used_token: &AccessToken,
    ) -> Result<AccessToken, SdkError> {
        let mut cached = self.access_token.lock().await;
        match cached.as_ref() {
            Some(token) if !token.is_same_generation(used_token) && token.is_valid() => {
                return Ok(token.clone());
            }
            Some(token) if token.is_same_generation(used_token) => *cached = None,
            _ => {}
        }

        let token = self.acquire_token().await?;
        *cached = Some(token.clone());
        Ok(token)
    }

    async fn acquire_token(&self) -> Result<AccessToken, SdkError> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "client_credentials")
            .append_pair("client_id", &self.client_id)
            .append_pair("client_secret", &self.client_secret)
            .finish();
        let response = self
            .http_client
            .execute(HttpRequest {
                method: "POST".into(),
                url: self.token_url.clone(),
                headers: vec![
                    HttpHeader {
                        name: "accept".into(),
                        value: "application/json".into(),
                    },
                    HttpHeader {
                        name: "content-type".into(),
                        value: "application/x-www-form-urlencoded".into(),
                    },
                ],
                body: Some(body.into_bytes()),
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
            expires_at,
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
        self.expires_at
            .checked_sub(TOKEN_EXPIRY_SKEW)
            .is_some_and(|refresh_at| Instant::now() < refresh_at)
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
