mod support;

use cyclops_sdk::{
    CreateSignedServiceUrlRequestBuilder, CyclopsClient, CyclopsConfiguration, CyclopsCredentials,
    HttpResponse, Sandbox, SdkError, SignedServiceUrl,
};
use std::sync::Arc;
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example:8443/control/";
const TOKEN_URL: &str = "https://auth.example/token";

#[tokio::test]
async fn creates_lists_and_revokes_signed_service_urls() {
    let created = signed_service_url(
        "31e1c9bb-8cc9-4c50-9cf4-51798b6978e4",
        Some("Customer demo"),
    );
    let second = signed_service_url(
        "347ceda8-7eec-4f34-82e1-04465e05f5cf",
        Some("Support session"),
    );
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(201, &signed_service_url_response(&created))),
        Ok(json_response(
            200,
            &vec![
                signed_service_url_response(&created),
                signed_service_url_response(&second),
            ],
        )),
        Ok(response(204, Vec::new())),
    ]));
    let client = client(Arc::clone(&http));

    let actual = Arc::clone(&client)
        .create_signed_service_url(request("Customer demo", 3600))
        .await
        .unwrap();
    let listed = Arc::clone(&client)
        .list_signed_service_urls(sandbox())
        .await
        .unwrap();
    Arc::clone(&client)
        .revoke_signed_service_url(actual.clone())
        .await
        .unwrap();

    assert_eq!(actual, created);
    assert_eq!(listed, vec![created, second]);
    let requests = http.requests().await;
    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].url,
        "https://cyclops.example:8443/control/api/signed-service-urls/tenant-a"
    );
    assert_eq!(
        requests[1].body.as_deref(),
        Some(
            br#"{"claim":"claim-a","sandbox":"sandbox-a","service":"sandbox-a-mcp","logicalService":"mcp","label":"Customer demo","expiresInSeconds":3600}"#
                .as_slice()
        )
    );
    assert_eq!(requests[2].method, "GET");
    assert_eq!(
        requests[2].url,
        "https://cyclops.example:8443/control/api/signed-service-urls/tenant-a?claim=claim-a"
    );
    assert_eq!(requests[3].method, "DELETE");
    assert_eq!(
        requests[3].url,
        "https://cyclops.example:8443/control/api/signed-service-urls/tenant-a/31e1c9bb-8cc9-4c50-9cf4-51798b6978e4"
    );
}

#[tokio::test]
async fn validates_logical_service_ttl_and_label_before_http() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let client = client(Arc::clone(&http));

    let unknown_service = Arc::clone(&client)
        .create_signed_service_url(
            CreateSignedServiceUrlRequestBuilder::new()
                .sandbox(sandbox())
                .service("missing".into())
                .expires_in_seconds(3600)
                .build()
                .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(matches!(unknown_service, SdkError::UnknownService { .. }));

    let short_ttl = Arc::clone(&client)
        .create_signed_service_url(request("Customer demo", 59))
        .await
        .unwrap_err();
    assert!(matches!(
        short_ttl,
        SdkError::InvalidResourceName { field, value, .. }
            if field == "expires_in_seconds" && value == "59"
    ));

    let long_label = client
        .create_signed_service_url(request(&"a".repeat(121), 3600))
        .await
        .unwrap_err();
    assert!(matches!(
        long_label,
        SdkError::InvalidResourceName { field, value, .. }
            if field == "label" && value.len() == 121
    ));
    assert_eq!(http.request_count().await, 0);
}

#[tokio::test]
async fn maps_503_and_preserves_other_failures() {
    let unavailable_http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(503, b"not configured".to_vec())),
    ]));
    let unavailable_error = client(unavailable_http)
        .list_signed_service_urls(sandbox())
        .await
        .unwrap_err();
    assert!(matches!(
        unavailable_error,
        SdkError::SignedServiceUrlsUnavailable
    ));

    let status_http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(
            400,
            b"invalid signed service URL request".to_vec(),
        )),
    ]));
    let status_error = client(status_http)
        .create_signed_service_url(request("Customer demo", 3600))
        .await
        .unwrap_err();
    assert!(matches!(
        status_error,
        SdkError::Status { operation, status: 400, body }
            if operation == "create signed service URL"
                && body == "invalid signed service URL request"
    ));

    let body_http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(201, b"not-json".to_vec())),
    ]));
    let body_error = client(body_http)
        .create_signed_service_url(request("Customer demo", 3600))
        .await
        .unwrap_err();
    assert!(matches!(body_error, SdkError::Body { .. }));
}

fn request(label: &str, expires_in_seconds: u32) -> cyclops_sdk::CreateSignedServiceUrlRequest {
    CreateSignedServiceUrlRequestBuilder::new()
        .sandbox(sandbox())
        .service("mcp".into())
        .label(label.into())
        .expires_in_seconds(expires_in_seconds)
        .build()
        .unwrap()
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
        namespace: "tenant-a".into(),
        claim: "claim-a".into(),
        name: "sandbox-a".into(),
        services: vec!["mcp".into(), "vnc".into()],
    }
}

fn signed_service_url(id: &str, label: Option<&str>) -> SignedServiceUrl {
    SignedServiceUrl {
        id: id.into(),
        namespace: "tenant-a".into(),
        claim: "claim-a".into(),
        sandbox: "sandbox-a".into(),
        service: "mcp".into(),
        label: label.map(str::to_owned),
        url: format!("https://signed.example/{id}"),
        created_at: "2026-08-31T12:34:56Z".into(),
        expires_at: "2026-08-31T13:34:56Z".into(),
        revoked_at: None,
    }
}

fn signed_service_url_response(signed_service_url: &SignedServiceUrl) -> serde_json::Value {
    serde_json::json!({
        "id": signed_service_url.id,
        "namespace": signed_service_url.namespace,
        "claim": signed_service_url.claim,
        "sandbox": signed_service_url.sandbox,
        "service": "sandbox-a-mcp",
        "logicalService": signed_service_url.service,
        "label": signed_service_url.label,
        "url": signed_service_url.url,
        "createdAt": signed_service_url.created_at,
        "expiresAt": signed_service_url.expires_at,
        "revokedAt": signed_service_url.revoked_at,
    })
}

fn token() -> HttpResponse {
    response(
        200,
        br#"{"access_token":"service-token","expires_in":3600}"#.to_vec(),
    )
}

fn json_response<T: serde::Serialize>(status: u16, value: &T) -> HttpResponse {
    response(status, serde_json::to_vec(value).unwrap())
}

fn response(status: u16, body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body,
    }
}
