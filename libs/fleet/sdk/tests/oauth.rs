mod support;

use cyclops_sdk::{
    AccessTokenProvider, AccessTokenProviderError, CyclopsClient, CyclopsConfiguration,
    CyclopsCredentials, CyclopsTokenProviderConfiguration, HttpError, HttpHeader, HttpRequest,
    HttpResponse, SdkError,
};
use std::{collections::VecDeque, sync::Arc};
use support::ScriptedHttpClient;
use tokio::sync::Mutex;

const BASE_URL: &str = "https://cyclops.example:8443/api";
const TOKEN_URL: &str = "https://identity.example/oauth/token";

#[tokio::test]
async fn caches_token_until_expiry_and_encodes_credentials() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token("token-a", 3600)),
        Ok(response(200, b"first")),
        Ok(response(200, b"second")),
    ]));
    let client = client(Arc::clone(&http));

    assert_eq!(
        client
            .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
            .await
            .unwrap()
            .body,
        b"first"
    );
    assert_eq!(
        client
            .execute_authenticated(request("https://cyclops.example:8443/api/claims"))
            .await
            .unwrap()
            .body,
        b"second"
    );

    let requests = http.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].url, TOKEN_URL);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].body,
        Some(b"grant_type=client_credentials&client_id=client+id%2B%2F%25&client_secret=secret+%26%3D%2B%2F%25".to_vec())
    );
    assert_eq!(
        requests[0].headers,
        vec![
            header("accept", "application/json"),
            header("content-type", "application/x-www-form-urlencoded"),
        ]
    );
    assert_bearer(&requests[1], "token-a");
    assert_bearer(&requests[2], "token-a");
    assert_eq!(
        requests[1].headers,
        vec![
            header("x-intent", "preserve-order"),
            header("x-trace", "keep"),
            header("authorization", "Bearer token-a"),
        ]
    );
}

#[tokio::test]
async fn refreshes_a_token_at_the_expiry_skew() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token("token-a", 30)),
        Ok(response(200, b"first")),
        Ok(token("token-b", 3600)),
        Ok(response(200, b"second")),
    ]));
    let client = client(Arc::clone(&http));

    client
        .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
        .await
        .unwrap();
    client
        .execute_authenticated(request("https://cyclops.example:8443/api/claims"))
        .await
        .unwrap();

    let requests = http.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.url == TOKEN_URL)
            .count(),
        2
    );
    assert_bearer(&requests[1], "token-a");
    assert_bearer(&requests[3], "token-b");
}

#[tokio::test]
async fn concurrent_cache_misses_fetch_one_token() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let blocked_token = http.enqueue_blocking(Ok(token("token-a", 3600))).await;
    http.enqueue(Ok(response(200, b"first"))).await;
    http.enqueue(Ok(response(200, b"second"))).await;
    let client = client(Arc::clone(&http));

    let first = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
                .await
        }
    });
    blocked_token.wait_until_started().await;
    let second = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .execute_authenticated(request("https://cyclops.example:8443/api/claims"))
                .await
        }
    });

    tokio::task::yield_now().await;
    assert_eq!(
        http.requests().await.len(),
        1,
        "only one token request is in flight"
    );
    blocked_token.release();
    assert_eq!(first.await.unwrap().unwrap().body, b"first");
    assert_eq!(second.await.unwrap().unwrap().body, b"second");

    let requests = http.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.url == TOKEN_URL)
            .count(),
        1
    );
}

#[tokio::test]
async fn delayed_unauthorized_does_not_invalidate_same_value_newer_generation() {
    let http = Arc::new(ScriptedHttpClient::new([Ok(token("shared-token", 3600))]));
    let delayed_unauthorized = http.enqueue_blocking(Ok(response(401, b"expired-a"))).await;
    http.enqueue(Ok(response(401, b"expired-b"))).await;
    http.enqueue(Ok(token("shared-token", 3600))).await;
    http.enqueue(Ok(response(200, b"retried-b"))).await;
    http.enqueue(Ok(response(200, b"retried-a"))).await;
    let client = client(Arc::clone(&http));

    let request_a = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .execute_authenticated(request("https://cyclops.example:8443/api/pools/a"))
                .await
        }
    });
    delayed_unauthorized.wait_until_started().await;

    let response_b = client
        .execute_authenticated(request("https://cyclops.example:8443/api/pools/b"))
        .await
        .unwrap();
    assert_eq!(response_b.body, b"retried-b");

    delayed_unauthorized.release();
    let response_a = request_a.await.unwrap().unwrap();
    assert_eq!(response_a.body, b"retried-a");

    let requests = http.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.url == TOKEN_URL)
            .count(),
        2,
        "a delayed 401 must not clear a newer same-value token generation"
    );
    assert_eq!(requests.len(), 6);
}

#[tokio::test]
async fn refreshes_once_after_unauthorized() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token("token-a", 3600)),
        Ok(response(401, b"expired")),
        Ok(token("token-b", 3600)),
        Ok(response(200, b"ok")),
    ]));
    let client = client(Arc::clone(&http));

    assert_eq!(
        client
            .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
            .await
            .unwrap()
            .body,
        b"ok"
    );

    let requests = http.requests().await;
    assert_eq!(requests.len(), 4);
    assert_bearer(&requests[1], "token-a");
    assert_bearer(&requests[3], "token-b");
}

#[tokio::test]
async fn provider_token_is_used_without_calling_the_token_endpoint() {
    let http = Arc::new(ScriptedHttpClient::new([Ok(response(200, b"ok"))]));
    let provider = Arc::new(ScriptedAccessTokenProvider::new([Ok(
        "browser-token".into()
    )]));
    let client = provider_client(Arc::clone(&http), provider);

    assert_eq!(
        client
            .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
            .await
            .unwrap()
            .body,
        b"ok"
    );

    let requests = http.requests().await;
    assert_eq!(requests.len(), 1);
    assert_bearer(&requests[0], "browser-token");
}

#[tokio::test]
async fn provider_is_forced_to_refresh_once_after_a_control_plane_401() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(response(401, b"expired")),
        Ok(response(200, b"ok")),
    ]));
    let provider = Arc::new(ScriptedAccessTokenProvider::new([
        Ok("browser-token-a".into()),
        Ok("browser-token-b".into()),
    ]));
    let client = provider_client(Arc::clone(&http), Arc::clone(&provider));

    assert_eq!(
        client
            .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
            .await
            .unwrap()
            .body,
        b"ok"
    );

    assert_eq!(provider.refresh_requests().await, vec![false, true]);
    let requests = http.requests().await;
    assert_eq!(requests.len(), 2);
    assert_bearer(&requests[0], "browser-token-a");
    assert_bearer(&requests[1], "browser-token-b");
}

#[tokio::test]
async fn provider_error_and_empty_token_are_reported_as_token_errors() {
    let provider_error = Arc::new(ScriptedAccessTokenProvider::new([Err(
        AccessTokenProviderError::Failed {
            reason: "browser session expired".into(),
        },
    )]));
    let error = provider_client(Arc::new(ScriptedHttpClient::new([])), provider_error)
        .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::Token { .. }));

    let empty_token = Arc::new(ScriptedAccessTokenProvider::new([Ok("  \n\t".into())]));
    let error = provider_client(Arc::new(ScriptedHttpClient::new([])), empty_token)
        .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::Token { .. }));
}

#[tokio::test]
async fn does_not_retry_a_second_unauthorized() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token("token-a", 3600)),
        Ok(response(401, b"expired")),
        Ok(token("token-b", 3600)),
        Ok(response(401, b"still expired")),
    ]));
    let client = client(Arc::clone(&http));

    let error = client
        .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::Status { status: 401, .. }));
    assert_eq!(http.requests().await.len(), 4);
}

#[tokio::test]
async fn maps_oauth_errors_and_invalid_json() {
    let status_http = Arc::new(ScriptedHttpClient::new([Ok(response(503, b"unavailable"))]));
    let error = client(status_http)
        .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, SdkError::Status { operation, status: 503, .. } if operation == "acquire OAuth token")
    );

    let invalid_json_http = Arc::new(ScriptedHttpClient::new([Ok(response(200, b"not json"))]));
    let error = client(invalid_json_http)
        .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::Token { .. }));

    let callback_http = Arc::new(ScriptedHttpClient::new([Err(HttpError::Transport {
        reason: "offline".into(),
    })]));
    let error = client(callback_http)
        .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::Transport { reason } if reason == "offline"));
}

#[tokio::test]
async fn does_not_attach_bearer_to_cross_origin_requests() {
    let http = Arc::new(ScriptedHttpClient::new([Ok(response(200, b"external"))]));
    let client = client(Arc::clone(&http));
    let external = HttpRequest {
        method: "GET".into(),
        url: "https://cyclops.example/api/pools".into(),
        headers: vec![header("Authorization", "Basic external")],
        body: Some(vec![0, 1, 2]),
    };

    let response = client.execute_authenticated(external).await.unwrap();
    assert_eq!(response.body, b"external");
    let requests = http.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers,
        vec![header("Authorization", "Basic external")]
    );
    assert_eq!(requests[0].body, Some(vec![0, 1, 2]));
}

fn client(http: Arc<ScriptedHttpClient>) -> Arc<CyclopsClient> {
    CyclopsClient::connect(
        CyclopsConfiguration {
            base_url: BASE_URL.into(),
            token_url: TOKEN_URL.into(),
            credentials: CyclopsCredentials::new("client id+/%".into(), "secret &=+/%".into()),
            pool_poll_interval_ms: 1,
            pool_poll_limit: 1,
            claim_poll_interval_ms: 1,
            claim_poll_limit: 1,
        },
        http,
    )
    .unwrap()
}

fn provider_client(
    http: Arc<ScriptedHttpClient>,
    provider: Arc<ScriptedAccessTokenProvider>,
) -> Arc<CyclopsClient> {
    CyclopsClient::connect_with_access_token_provider(
        CyclopsTokenProviderConfiguration {
            base_url: BASE_URL.into(),
            pool_poll_interval_ms: 1,
            pool_poll_limit: 1,
            claim_poll_interval_ms: 1,
            claim_poll_limit: 1,
        },
        provider,
        http,
    )
    .unwrap()
}

struct ScriptedAccessTokenProvider {
    values: Mutex<VecDeque<Result<String, AccessTokenProviderError>>>,
    refresh_requests: Mutex<Vec<bool>>,
}

impl ScriptedAccessTokenProvider {
    fn new(values: impl IntoIterator<Item = Result<String, AccessTokenProviderError>>) -> Self {
        Self {
            values: Mutex::new(values.into_iter().collect()),
            refresh_requests: Mutex::new(Vec::new()),
        }
    }

    async fn refresh_requests(&self) -> Vec<bool> {
        self.refresh_requests.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl AccessTokenProvider for ScriptedAccessTokenProvider {
    async fn get_access_token(
        &self,
        force_refresh: bool,
    ) -> Result<String, AccessTokenProviderError> {
        self.refresh_requests.lock().await.push(force_refresh);
        self.values
            .lock()
            .await
            .pop_front()
            .expect("access-token provider called more than scripted")
    }
}

fn request(url: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: url.into(),
        headers: vec![
            header("x-intent", "preserve-order"),
            header("Authorization", "Bearer caller-token"),
            header("x-trace", "keep"),
        ],
        body: None,
    }
}

fn token(value: &str, expires_in: u64) -> HttpResponse {
    response(
        200,
        format!(r#"{{"access_token":"{value}","expires_in":{expires_in}}}"#).as_bytes(),
    )
}

fn response(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: body.to_vec(),
    }
}

fn header(name: &str, value: &str) -> HttpHeader {
    HttpHeader {
        name: name.into(),
        value: value.into(),
    }
}

fn assert_bearer(request: &HttpRequest, token: &str) {
    assert_eq!(
        request
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("authorization"))
            .count(),
        1
    );
    assert_eq!(
        request
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("authorization"))
            .unwrap()
            .value,
        format!("Bearer {token}")
    );
}

#[tokio::test]
async fn static_access_token_is_used_without_token_provider_callback() {
    let http = Arc::new(ScriptedHttpClient::new([Ok(response(200, b"ok"))]));
    let client = static_token_client(Arc::clone(&http), "browser-token");

    assert_eq!(
        client
            .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
            .await
            .unwrap()
            .body,
        b"ok"
    );

    let requests = http.requests().await;
    assert_eq!(requests.len(), 1);
    assert_bearer(&requests[0], "browser-token");
}

fn static_token_client(http: Arc<ScriptedHttpClient>, access_token: &str) -> Arc<CyclopsClient> {
    CyclopsClient::connect_with_access_token(
        CyclopsTokenProviderConfiguration {
            base_url: BASE_URL.into(),
            pool_poll_interval_ms: 1,
            pool_poll_limit: 1,
            claim_poll_interval_ms: 1,
            claim_poll_limit: 1,
        },
        access_token.into(),
        http,
    )
    .unwrap()
}

#[tokio::test]
async fn static_access_token_is_not_retried_after_a_401() {
    let http = Arc::new(ScriptedHttpClient::new([Ok(response(401, b"expired"))]));
    let client = static_token_client(Arc::clone(&http), "browser-token");

    let error = client
        .execute_authenticated(request("https://cyclops.example:8443/api/pools"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, SdkError::Token { reason } if reason.contains("static access token was rejected"))
    );
    assert_eq!(http.requests().await.len(), 1);
}
