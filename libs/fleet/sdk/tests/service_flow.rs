mod support;

use cyclops_sdk::{
    CyclopsClient, CyclopsConfiguration, CyclopsCredentials, HttpError, HttpHeader, HttpRequest,
    HttpResponse, Sandbox, SdkError,
};
use std::sync::Arc;
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example:8443/control/";
const TOKEN_URL: &str = "https://auth.example/token";

#[tokio::test]
async fn routes_known_service_under_base_prefix_and_forwards_query() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(200, vec![0, 255, 1])),
    ]));
    let client = client(Arc::clone(&http));

    let result = client
        .service_request(
            sandbox(),
            "mcp".into(),
            "/tools?cursor=next".into(),
            request(None),
        )
        .await
        .unwrap();

    assert_eq!(result.status, 200);
    assert_eq!(result.body, vec![0, 255, 1]);
    let requests = http.requests().await;
    assert_eq!(
        requests[1].url,
        "https://cyclops.example:8443/control/api/svc/pool-test/sandbox-1-mcp/tools?cursor=next"
    );
    assert_eq!(requests[1].method, "PATCH");
}

#[tokio::test]
async fn rejects_unknown_service_with_sorted_unique_available_names() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let client = client(Arc::clone(&http));
    let mut sandbox = sandbox();
    sandbox.services = vec!["zeta".into(), "mcp".into(), "api".into(), "mcp".into()];

    let error = client
        .service_request(sandbox, "missing".into(), "/".into(), request(None))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SdkError::UnknownService { requested, available }
            if requested == "missing" && available == ["api", "mcp", "zeta"]
    ));
    assert_eq!(http.request_count().await, 0);
}

#[tokio::test]
async fn rejects_ambiguous_or_normalizing_service_paths_before_http() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let client = client(Arc::clone(&http));

    for path in [
        "tools",
        "//other.example/tools",
        "https://other.example/tools",
        "/https://other.example/tools",
        "/tools#fragment",
        "/tools?cursor=next#fragment",
        "/../admin",
        "/safe/../admin",
        "/%2e%2e/admin",
        "/safe/%2E%2e/admin",
        "/safe%2fadmin",
        "/safe%5cadmin",
    ] {
        let error = Arc::clone(&client)
            .service_request(sandbox(), "mcp".into(), path.into(), request(None))
            .await
            .unwrap_err();
        assert!(
            matches!(error, SdkError::InvalidServicePath { path: rejected } if rejected == path)
        );
    }

    assert_eq!(http.request_count().await, 0);
}

#[tokio::test]
async fn rejects_literal_and_encoded_controls_before_service_http() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let client = client(Arc::clone(&http));
    let mut cases = Vec::new();

    for control in (0_u8..=0x1f).chain([0x7f]) {
        let control = char::from(control);
        cases.push(format!("/ready{control}tail"));
        cases.push(format!("/ready?note=before{control}after"));
    }
    for encoded in [
        "%00", "%09", "%0A", "%0a", "%0D", "%0d", "%1F", "%1f", "%7F", "%7f",
    ] {
        cases.push(format!("/ready{encoded}"));
        cases.push(format!("/ready?note={encoded}"));
        cases.push(format!("/ready%25{}", &encoded[1..]));
        cases.push(format!("/ready?note=%25{}", &encoded[1..]));
    }
    cases.extend([
        "/safe%252fadmin".into(),
        "/safe%255cadmin".into(),
        "/safe/%252e%252e/admin".into(),
    ]);

    for path in cases {
        let error = Arc::clone(&client)
            .service_request(sandbox(), "mcp".into(), path.clone(), request(None))
            .await
            .unwrap_err();
        assert!(
            matches!(error, SdkError::InvalidServicePath { path: rejected } if rejected == path)
        );
    }
    assert_eq!(http.request_count().await, 0);
}

#[tokio::test]
async fn validates_sandbox_identity_and_known_service_dns_labels_before_http() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let client = client(Arc::clone(&http));

    let mut invalid_namespace = sandbox();
    invalid_namespace.namespace = "Bad".into();
    let error = Arc::clone(&client)
        .service_request(invalid_namespace, "mcp".into(), "/".into(), request(None))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::InvalidResourceName { field, .. } if field == "namespace"));

    let mut invalid_sandbox = sandbox();
    invalid_sandbox.name = "bad_name".into();
    let error = Arc::clone(&client)
        .service_request(invalid_sandbox, "mcp".into(), "/".into(), request(None))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::InvalidResourceName { field, .. } if field == "sandbox"));

    let mut invalid_service = sandbox();
    invalid_service.services = vec!["Bad".into()];
    let error = client
        .service_request(invalid_service, "Bad".into(), "/".into(), request(None))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::InvalidResourceName { field, .. } if field == "service"));
    assert_eq!(http.request_count().await, 0);
}

#[tokio::test]
async fn filters_hop_by_hop_and_connection_nominated_headers_without_reordering_others() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(200, vec![])),
    ]));
    let client = client(Arc::clone(&http));
    let request = HttpRequest {
        method: "POST".into(),
        url: "https://attacker.example/ignored".into(),
        headers: vec![
            header("Connection", "Keep-Alive, X-Remove"),
            header("keep-alive", "timeout=5"),
            header("X-Remove", "smuggled"),
            header("HOST", "attacker.example"),
            header("Authorization", "Bearer attacker"),
            header("Proxy-Authorization", "Basic attacker"),
            header("Proxy-Connection", "keep-alive"),
            header("Transfer-Encoding", "chunked"),
            header("TE", "trailers"),
            header("Trailer", "x-trailer"),
            header("Upgrade", "websocket"),
            header("X-Keep", "first"),
            header("x-keep", "second"),
            header("Cookie", "session=kept"),
        ],
        body: Some(vec![0, 255, 1]),
    };

    client
        .service_request(sandbox(), "mcp".into(), "/upload".into(), request)
        .await
        .unwrap();

    let request = &http.requests().await[1];
    assert_eq!(
        request.url,
        "https://cyclops.example:8443/control/api/svc/pool-test/sandbox-1-mcp/upload"
    );
    assert_eq!(
        request.headers,
        vec![
            header("X-Keep", "first"),
            header("x-keep", "second"),
            header("Cookie", "session=kept"),
            header("authorization", "Bearer service-token"),
        ]
    );
    assert_eq!(request.body, Some(vec![0, 255, 1]));
}

#[tokio::test]
async fn preserves_absent_empty_and_nonempty_request_bodies() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(200, vec![])),
        Ok(response(200, vec![])),
        Ok(response(200, vec![])),
    ]));
    let client = client(Arc::clone(&http));

    for body in [None, Some(vec![]), Some(vec![0, 255, 1])] {
        Arc::clone(&client)
            .service_request(sandbox(), "mcp".into(), "/body".into(), request(body))
            .await
            .unwrap();
    }

    let requests = http.requests().await;
    assert_eq!(requests[1].body, None);
    assert_eq!(requests[2].body, Some(vec![]));
    assert_eq!(requests[3].body, Some(vec![0, 255, 1]));
}

#[tokio::test]
async fn preserves_valid_utf8_and_percent_encoded_query_data() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(200, vec![])),
    ]));
    let client = client(Arc::clone(&http));

    client
        .service_request(
            sandbox(),
            "mcp".into(),
            "/search?label=naïve&check=%E2%9C%93&space=%20".into(),
            request(None),
        )
        .await
        .unwrap();

    assert_eq!(
        http.requests().await[1].url,
        "https://cyclops.example:8443/control/api/svc/pool-test/sandbox-1-mcp/search?label=na%C3%AFve&check=%E2%9C%93&space=%20"
    );
}

#[tokio::test]
async fn returns_non_success_service_responses_and_binary_headers_unchanged() {
    let expected = HttpResponse {
        status: 418,
        headers: vec![header("x-first", "one"), header("x-first", "two")],
        body: vec![0, 255, 1],
    };
    let http = Arc::new(ScriptedHttpClient::new([Ok(token()), Ok(expected.clone())]));
    let client = client(http);

    let response = client
        .service_request(sandbox(), "mcp".into(), "/teapot".into(), request(None))
        .await
        .unwrap();

    assert_eq!(response, expected);
}

#[tokio::test]
async fn returns_service_unauthorized_once_without_refreshing_or_replaying() {
    let expected = HttpResponse {
        status: 401,
        headers: vec![
            header("x-upstream", "first"),
            header("x-upstream", "second"),
        ],
        body: b"service denied".to_vec(),
    };
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(expected.clone()),
        Ok(token()),
        Ok(response(201, b"replayed".to_vec())),
    ]));
    let client = client(Arc::clone(&http));
    let request = HttpRequest {
        method: "POST".into(),
        url: "https://attacker.example/ignored".into(),
        headers: vec![header("content-type", "application/octet-stream")],
        body: Some(vec![0, 255, 1]),
    };

    let response = client
        .service_request(sandbox(), "mcp".into(), "/mutate".into(), request)
        .await
        .unwrap();

    assert_eq!(response, expected);
    let requests = http.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.url == TOKEN_URL)
            .count(),
        1
    );
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].body, Some(vec![0, 255, 1]));
}

#[tokio::test]
async fn maps_foreign_service_transport_errors() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Err(HttpError::Transport {
            reason: "offline".into(),
        }),
    ]));
    let client = client(http);

    let error = client
        .service_request(sandbox(), "mcp".into(), "/health".into(), request(None))
        .await
        .unwrap_err();

    assert!(matches!(error, SdkError::Transport { reason } if reason == "offline"));
}

#[tokio::test]
async fn concurrent_service_calls_do_not_serialize_on_one_client() {
    let http = Arc::new(ScriptedHttpClient::new([Ok(token())]));
    let first_response = http
        .enqueue_blocking(Ok(response(200, b"first".to_vec())))
        .await;
    let second_response = http
        .enqueue_blocking(Ok(response(200, b"second".to_vec())))
        .await;
    let client = client(Arc::clone(&http));

    let first = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .service_request(sandbox(), "mcp".into(), "/first".into(), request(None))
                .await
        }
    });
    first_response.wait_until_started().await;

    let second = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .service_request(sandbox(), "mcp".into(), "/second".into(), request(None))
                .await
        }
    });
    second_response.wait_until_started().await;
    assert_eq!(
        http.request_count().await,
        3,
        "second service request must reach the transport while the first is blocked"
    );

    first_response.release();
    second_response.release();
    assert_eq!(first.await.unwrap().unwrap().body, b"first");
    assert_eq!(second.await.unwrap().unwrap().body, b"second");
}

fn client(http: Arc<ScriptedHttpClient>) -> Arc<CyclopsClient> {
    CyclopsClient::connect(
        CyclopsConfiguration {
            base_url: BASE_URL.into(),
            token_url: TOKEN_URL.into(),
            credentials: CyclopsCredentials::new("client-id".into(), "client-secret".into()),
            pool_poll_interval_ms: 1,
            pool_poll_limit: 3,
            claim_poll_interval_ms: 1,
            claim_poll_limit: 3,
        },
        http,
    )
    .unwrap()
}

fn sandbox() -> Sandbox {
    Sandbox {
        namespace: "pool-test".into(),
        claim: "claim-1".into(),
        name: "sandbox-1".into(),
        services: vec!["mcp".into(), "api".into()],
    }
}

fn request(body: Option<Vec<u8>>) -> HttpRequest {
    HttpRequest {
        method: "PATCH".into(),
        url: "https://attacker.example/ignored".into(),
        headers: vec![],
        body,
    }
}

fn token() -> HttpResponse {
    response(
        200,
        br#"{"access_token":"service-token","expires_in":3600}"#.to_vec(),
    )
}

fn response(status: u16, body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body,
    }
}

fn header(name: &str, value: &str) -> HttpHeader {
    HttpHeader {
        name: name.into(),
        value: value.into(),
    }
}
