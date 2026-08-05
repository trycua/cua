mod support;

use cyclops_sdk::{
    CreateUserApiKeyRequest, CyclopsClient, CyclopsTokenProviderConfiguration, HttpHeader,
    HttpRequest, HttpResponse, Namespace, NewUserApiKey, SdkError, UserApiKey,
};
use std::{collections::HashMap, sync::Arc};
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example:8443/root";

#[tokio::test]
async fn typed_account_apis_use_authenticated_backend_contracts() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(response(
            200,
            br#"[{"name":"demo","status":"Active","createdAt":"2026-08-04T12:00:00Z","labels":{"team":"sdk"}}]"#,
        )),
        Ok(response(
            200,
            br#"{"keys":[{"id":"key-id","client_id":"ukey-demo","name":"demo key","scope":["demo"]}]}"#,
        )),
        Ok(response(
            201,
            br#"{"client_id":"ukey-ci","client_secret":"shown-once","token_url":"https://identity.example/token","name":"ci-key","scope":["demo"]}"#,
        )),
        Ok(response(204, b"")),
    ]));
    let client = client(Arc::clone(&http));

    assert_eq!(
        Arc::clone(&client).list_namespaces().await.unwrap(),
        vec![Namespace {
            name: "demo".into(),
            status: "Active".into(),
            created_at: "2026-08-04T12:00:00Z".into(),
            labels: Some(HashMap::from([("team".into(), "sdk".into())])),
        }]
    );
    assert_eq!(
        Arc::clone(&client).list_user_api_keys().await.unwrap(),
        vec![UserApiKey {
            id: "key-id".into(),
            client_id: "ukey-demo".into(),
            name: "demo key".into(),
            scope: vec!["demo".into()],
        }]
    );
    assert_eq!(
        Arc::clone(&client)
            .create_user_api_key(CreateUserApiKeyRequest {
                name: "ci-key".into(),
                scope: vec!["demo".into()],
            })
            .await
            .unwrap(),
        NewUserApiKey {
            client_id: "ukey-ci".into(),
            client_secret: "shown-once".into(),
            token_url: "https://identity.example/token".into(),
            name: "ci-key".into(),
            scope: vec!["demo".into()],
        }
    );
    client.delete_user_api_key("key/id".into()).await.unwrap();

    let requests = http.requests().await;
    assert_eq!(requests.len(), 4);
    assert_request(
        &requests[0],
        "GET",
        "https://cyclops.example:8443/root/api/namespaces",
        None,
    );
    assert_request(
        &requests[1],
        "GET",
        "https://cyclops.example:8443/root/api/user-keys",
        None,
    );
    assert_request(
        &requests[2],
        "POST",
        "https://cyclops.example:8443/root/api/user-keys",
        Some(br#"{"name":"ci-key","scope":["demo"]}"#),
    );
    assert_request(
        &requests[3],
        "DELETE",
        "https://cyclops.example:8443/root/api/user-keys/key%2Fid",
        None,
    );
}

#[tokio::test]
async fn list_user_api_keys_treats_null_scope_as_unrestricted() {
    let http = Arc::new(ScriptedHttpClient::new([Ok(response(
        200,
        br#"{"keys":[{"id":"key-id","client_id":"ukey-demo","name":"demo key","scope":null}]}"#,
    ))]));
    let client = client(Arc::clone(&http));

    assert_eq!(
        client.list_user_api_keys().await.unwrap(),
        vec![UserApiKey {
            id: "key-id".into(),
            client_id: "ukey-demo".into(),
            name: "demo key".into(),
            scope: vec![],
        }]
    );
}

#[tokio::test]
async fn typed_account_apis_validate_ids_statuses_and_json() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(response(200, b"not json")),
        Ok(response(500, b"key service unavailable")),
    ]));
    let client = client(Arc::clone(&http));

    assert!(matches!(
        Arc::clone(&client).list_namespaces().await.unwrap_err(),
        SdkError::Body { .. }
    ));
    assert!(matches!(
        Arc::clone(&client).list_user_api_keys().await.unwrap_err(),
        SdkError::Status {
            ref operation,
            status: 500,
            ref body,
        } if operation == "list user API keys" && body == "key service unavailable"
    ));
    assert!(matches!(
        client.delete_user_api_key(String::new()).await.unwrap_err(),
        SdkError::Configuration { ref reason } if reason == "user API key id must not be empty"
    ));
    assert_eq!(http.request_count().await, 2);
}

fn client(http: Arc<ScriptedHttpClient>) -> Arc<CyclopsClient> {
    CyclopsClient::connect_with_access_token(
        CyclopsTokenProviderConfiguration {
            base_url: BASE_URL.into(),
            pool_poll_interval_ms: 1,
            pool_poll_limit: 1,
            claim_poll_interval_ms: 1,
            claim_poll_limit: 1,
        },
        "offline-token".into(),
        http,
    )
    .unwrap()
}

fn response(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: body.to_vec(),
    }
}

fn assert_request(request: &HttpRequest, method: &str, url: &str, body: Option<&[u8]>) {
    assert_eq!(request.method, method);
    assert_eq!(request.url, url);
    assert_eq!(request.body.as_deref(), body);
    let mut expected_headers = vec![header("accept", "application/json")];
    if body.is_some() {
        expected_headers.push(header("content-type", "application/json"));
    }
    expected_headers.push(header("authorization", "Bearer offline-token"));
    assert_eq!(request.headers, expected_headers);
}

fn header(name: &str, value: &str) -> HttpHeader {
    HttpHeader {
        name: name.into(),
        value: value.into(),
    }
}
