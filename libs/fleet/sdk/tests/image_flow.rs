mod support;

use cyclops_sdk::{
    CyclopsClient, CyclopsConfiguration, CyclopsCredentials, HttpResponse, PreservedJson, SdkError,
};
use std::sync::Arc;
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example:8443/prefix";
const TOKEN_URL: &str = "https://identity.example/oauth/token";
const NAMESPACE: &str = "workers";
const IMAGE_NAME: &str = "image-demo";
const IMAGE_COLLECTION: &str = "https://cyclops.example:8443/prefix/api/k8s/apis/images.cua.ai/v1alpha1/namespaces/workers/images";
const IMAGE_ITEM: &str = "https://cyclops.example:8443/prefix/api/k8s/apis/images.cua.ai/v1alpha1/namespaces/workers/images/image-demo";
const IMAGE_JSON: &[u8] = br#"{"apiVersion":"images.cua.ai/v1alpha1","kind":"Image","metadata":{"namespace":"workers","name":"image-demo"},"spec":{"recipe":"example"}}"#;

#[tokio::test]
async fn image_crud_uses_generic_k8s_paths() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(404, br#"{"kind":"Status"}"#)),
        Ok(response(201, IMAGE_JSON)),
        Ok(response(200, br#"{"items":[]}"#)),
        Ok(response(200, IMAGE_JSON)),
        Ok(response(200, br#"{}"#)),
    ]));
    let client = client(Arc::clone(&http));
    let manifest =
        PreservedJson::from_json(String::from_utf8(IMAGE_JSON.to_vec()).unwrap()).unwrap();

    assert!(
        client
            .clone()
            .get_image(NAMESPACE.into(), IMAGE_NAME.into())
            .await
            .is_err()
    );
    client
        .clone()
        .create_image(NAMESPACE.into(), manifest)
        .await
        .unwrap();
    client.clone().list_images(NAMESPACE.into()).await.unwrap();
    client
        .clone()
        .get_image(NAMESPACE.into(), IMAGE_NAME.into())
        .await
        .unwrap();
    client
        .delete_image(NAMESPACE.into(), IMAGE_NAME.into())
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    assert_eq!(
        requests
            .iter()
            .map(|request| request.url.as_str())
            .collect::<Vec<_>>(),
        vec![
            IMAGE_ITEM,
            IMAGE_COLLECTION,
            IMAGE_COLLECTION,
            IMAGE_ITEM,
            IMAGE_ITEM
        ]
    );
    assert_eq!(requests[1].method, "POST");
    let body: serde_json::Value =
        serde_json::from_slice(requests[1].body.as_deref().unwrap()).unwrap();
    let expected: serde_json::Value = serde_json::from_slice(IMAGE_JSON).unwrap();
    assert_eq!(body, expected);
}

#[tokio::test]
async fn image_delete_accepts_only_its_documented_success_statuses() {
    for status in [200, 202, 204, 404] {
        let http = Arc::new(ScriptedHttpClient::new([
            Ok(token()),
            Ok(response(status, b"{}")),
        ]));

        client(Arc::clone(&http))
            .delete_image(NAMESPACE.into(), IMAGE_NAME.into())
            .await
            .unwrap();
        assert_eq!(http.authenticated_requests().await[0].url, IMAGE_ITEM);
    }

    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(201, b"{}")),
    ]));
    assert!(matches!(
        client(http)
            .delete_image(NAMESPACE.into(), IMAGE_NAME.into())
            .await,
        Err(SdkError::Status { status: 201, .. })
    ));
}

#[tokio::test]
async fn image_validation_and_namespace_mismatches_do_not_reach_http() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let error = client(Arc::clone(&http))
        .get_image("Workers".into(), IMAGE_NAME.into())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SdkError::InvalidResourceName { ref field, .. } if field == "namespace"
    ));
    assert_eq!(http.request_count().await, 0);

    let http = Arc::new(ScriptedHttpClient::new([]));
    let manifest = PreservedJson::from_json(
        r#"{"apiVersion":"images.cua.ai/v1alpha1","kind":"Image","metadata":{"namespace":"other","name":"image-demo"}}"#.into(),
    )
    .unwrap();
    let error = client(Arc::clone(&http))
        .create_image(NAMESPACE.into(), manifest)
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::Configuration { .. }));
    assert_eq!(http.request_count().await, 0);
}

#[tokio::test]
async fn image_create_rejects_missing_namespace_without_http() {
    assert_create_image_rejected_without_request(
        r#"{"apiVersion":"images.cua.ai/v1alpha1","kind":"Image","metadata":{"name":"image-demo"}}"#,
    )
    .await;
}

#[tokio::test]
async fn image_create_rejects_non_string_namespaces_without_http() {
    for namespace in ["null", "false", "1", "{}", "[]"] {
        let manifest = format!(
            r#"{{"apiVersion":"images.cua.ai/v1alpha1","kind":"Image","metadata":{{"namespace":{namespace},"name":"image-demo"}}}}"#
        );
        assert_create_image_rejected_without_request(&manifest).await;
    }
}

async fn assert_create_image_rejected_without_request(manifest_json: &str) {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let manifest = PreservedJson::from_json(manifest_json.into()).unwrap();

    let error = client(Arc::clone(&http))
        .create_image(NAMESPACE.into(), manifest)
        .await
        .unwrap_err();

    assert!(matches!(error, SdkError::Configuration { .. }));
    assert_eq!(http.request_count().await, 0);
}

fn client(http: Arc<ScriptedHttpClient>) -> Arc<CyclopsClient> {
    CyclopsClient::connect(
        CyclopsConfiguration {
            base_url: BASE_URL.into(),
            token_url: TOKEN_URL.into(),
            credentials: CyclopsCredentials::new("client".into(), "secret".into()),
            pool_poll_interval_ms: 1,
            pool_poll_limit: 1,
            claim_poll_interval_ms: 1,
            claim_poll_limit: 1,
        },
        http,
    )
    .unwrap()
}

fn token() -> HttpResponse {
    response(200, br#"{"access_token":"token-a","expires_in":3600}"#)
}

fn response(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: body.into(),
    }
}
