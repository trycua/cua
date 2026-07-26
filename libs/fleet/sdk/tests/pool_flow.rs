mod support;

use cyclops_sdk::{
    CreatePoolRequest, CyclopsClient, CyclopsConfiguration, CyclopsCredentials, HttpHeader,
    HttpResponse, Pool, ResourceMetadata, SdkError,
};
use cyclops_sdk_schema::PoolSpec;
use futures_timer::Delay;
use std::{sync::Arc, time::Duration};
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example:8443";
const TOKEN_URL: &str = "https://identity.example/oauth/token";
const NAMESPACE: &str = "example-pool";
const COLLECTION: &str = "https://cyclops.example:8443/api/k8s/apis/cua.ai/v1/namespaces/example-pool/osgymworkspacepools";
const ITEM: &str = "https://cyclops.example:8443/api/k8s/apis/cua.ai/v1/namespaces/example-pool/osgymworkspacepools/example-pool";
const NAMESPACE_COLLECTION: &str = "https://cyclops.example:8443/api/namespaces";
const NAMESPACE_ITEM: &str = "https://cyclops.example:8443/api/namespaces/example-pool";

#[tokio::test]
async fn creates_namespace_via_backend_collection_contract_then_pool_with_typed_json() {
    let created = pool();
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(201, br#"{}"#)),
        Ok(json_response(201, &created)),
    ]));
    let client = client(Arc::clone(&http));

    assert_eq!(
        client
            .create_pool(CreatePoolRequest {
                namespace: NAMESPACE.into(),
                spec: pool_spec(),
            })
            .await
            .unwrap(),
        created
    );

    let requests = resource_requests(&http).await;
    assert_request(
        &requests[0],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
    assert_request(&requests[1], "POST", COLLECTION, Some(&json_bytes(&pool())));
}

#[tokio::test]
async fn rolls_back_only_a_namespace_created_by_this_call_and_preserves_pool_error() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(201, br#"{}"#)),
        Ok(response(422, b"pool rejected")),
        Ok(response(500, b"rollback failed")),
    ]));
    let client = client(Arc::clone(&http));

    let error = client
        .create_pool(CreatePoolRequest {
            namespace: NAMESPACE.into(),
            spec: pool_spec(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SdkError::Status {
            ref operation,
            status: 422,
            ref body,
        } if operation == "create pool" && body == "pool rejected"
    ));

    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 3);
    assert_request(
        &requests[0],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
    assert_request(&requests[1], "POST", COLLECTION, Some(&json_bytes(&pool())));
    assert_request(&requests[2], "DELETE", NAMESPACE_ITEM, None);
}

#[tokio::test]
async fn does_not_roll_back_namespace_reported_existing_by_409() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(409, b"already exists")),
        Ok(response(422, b"pool rejected")),
    ]));
    let client = client(Arc::clone(&http));

    assert!(
        client
            .create_pool(CreatePoolRequest {
                namespace: NAMESPACE.into(),
                spec: pool_spec(),
            })
            .await
            .is_err()
    );

    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 2);
    assert_request(
        &requests[0],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
    assert_request(&requests[1], "POST", COLLECTION, Some(&json_bytes(&pool())));
}

#[tokio::test]
async fn rejects_namespace_200_without_creating_a_pool_or_rolling_back() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(200, b"unexpected success")),
    ]));
    let client = client(Arc::clone(&http));

    assert!(matches!(
        client
            .create_pool(CreatePoolRequest {
                namespace: NAMESPACE.into(),
                spec: pool_spec(),
            })
            .await,
        Err(SdkError::Status {
            ref operation,
            status: 200,
            ref body,
        }) if operation == "create namespace" && body == "unexpected success"
    ));

    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 1);
    assert_request(
        &requests[0],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
}

#[tokio::test]
async fn rejects_unexpected_namespace_status_without_creating_a_pool() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(202, b"pending")),
    ]));
    let client = client(Arc::clone(&http));

    assert!(matches!(
        client
            .create_pool(CreatePoolRequest {
                namespace: NAMESPACE.into(),
                spec: pool_spec(),
            })
            .await,
        Err(SdkError::Status {
            ref operation,
            status: 202,
            ref body,
        }) if operation == "create namespace" && body == "pending"
    ));

    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 1);
    assert_request(
        &requests[0],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
}

#[tokio::test]
async fn serializes_same_namespace_create_lifecycle_before_rollback() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    http.enqueue(Ok(token())).await;
    http.enqueue(Ok(response(201, b"{}"))).await;
    let blocked_pool = http
        .enqueue_blocking(Ok(response(422, b"first pool rejected")))
        .await;
    let client = client(Arc::clone(&http));

    let first = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .create_pool(CreatePoolRequest {
                    namespace: NAMESPACE.into(),
                    spec: pool_spec(),
                })
                .await
        }
    });
    blocked_pool.wait_until_started().await;

    let second = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .create_pool(CreatePoolRequest {
                    namespace: NAMESPACE.into(),
                    spec: pool_spec(),
                })
                .await
        }
    });
    tokio::task::yield_now().await;
    assert_eq!(resource_requests(&http).await.len(), 2);

    http.enqueue(Ok(response(204, b""))).await;
    http.enqueue(Ok(response(409, b"already exists"))).await;
    http.enqueue(Ok(json_response(201, &pool()))).await;
    blocked_pool.release();

    assert!(matches!(
        first.await.unwrap(),
        Err(SdkError::Status {
            ref operation,
            status: 422,
            ref body,
        }) if operation == "create pool" && body == "first pool rejected"
    ));
    assert_eq!(second.await.unwrap().unwrap(), pool());

    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 5);
    assert_request(
        &requests[0],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
    assert_request(&requests[1], "POST", COLLECTION, Some(&json_bytes(&pool())));
    assert_request(&requests[2], "DELETE", NAMESPACE_ITEM, None);
    assert_request(
        &requests[3],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
    assert_request(&requests[4], "POST", COLLECTION, Some(&json_bytes(&pool())));
}

#[tokio::test]
async fn creates_for_different_namespaces_proceed_concurrently() {
    const OTHER_NAMESPACE: &str = "other-pool";

    let http = Arc::new(ScriptedHttpClient::new([]));
    http.enqueue(Ok(token())).await;
    let blocked_namespace = http.enqueue_blocking(Ok(response(201, b"{}"))).await;
    http.enqueue(Ok(response(201, b"{}"))).await;
    http.enqueue(Ok(json_response(201, &pool_named(OTHER_NAMESPACE))))
        .await;
    http.enqueue(Ok(json_response(201, &pool()))).await;
    let client = client(Arc::clone(&http));

    let first = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .create_pool(CreatePoolRequest {
                    namespace: NAMESPACE.into(),
                    spec: pool_spec(),
                })
                .await
        }
    });
    blocked_namespace.wait_until_started().await;

    let mut second = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .create_pool(CreatePoolRequest {
                    namespace: OTHER_NAMESPACE.into(),
                    spec: pool_spec(),
                })
                .await
        }
    });
    let second_pool = tokio::select! {
        result = &mut second => result.unwrap().unwrap(),
        _ = Delay::new(Duration::from_millis(200)) => panic!("different namespaces were serialized"),
    };
    assert_eq!(second_pool, pool_named(OTHER_NAMESPACE));

    let other_collection = format!(
        "https://cyclops.example:8443/api/k8s/apis/cua.ai/v1/namespaces/{OTHER_NAMESPACE}/osgymworkspacepools"
    );
    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 3);
    assert_request(
        &requests[0],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
    assert_request(
        &requests[1],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"other-pool"}"#),
    );
    assert_request(
        &requests[2],
        "POST",
        &other_collection,
        Some(&json_bytes(&pool_named(OTHER_NAMESPACE))),
    );

    blocked_namespace.release();
    assert_eq!(first.await.unwrap().unwrap(), pool());
}

#[tokio::test]
async fn create_and_delete_for_the_same_namespace_do_not_interleave() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    http.enqueue(Ok(token())).await;
    http.enqueue(Ok(response(201, b"{}"))).await;
    let blocked_create = http.enqueue_blocking(Ok(json_response(201, &pool()))).await;
    let client = client(Arc::clone(&http));

    let create = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .create_pool(CreatePoolRequest {
                    namespace: NAMESPACE.into(),
                    spec: pool_spec(),
                })
                .await
        }
    });
    blocked_create.wait_until_started().await;

    let delete = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.delete_pool(pool()).await }
    });
    tokio::task::yield_now().await;
    assert_eq!(resource_requests(&http).await.len(), 2);

    http.enqueue(Ok(response(204, b""))).await;
    http.enqueue(Ok(response(204, b""))).await;
    blocked_create.release();

    assert_eq!(create.await.unwrap().unwrap(), pool());
    delete.await.unwrap().unwrap();

    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 4);
    assert_request(
        &requests[0],
        "POST",
        NAMESPACE_COLLECTION,
        Some(br#"{"name":"example-pool"}"#),
    );
    assert_request(&requests[1], "POST", COLLECTION, Some(&json_bytes(&pool())));
    assert_request(&requests[2], "DELETE", ITEM, None);
    assert_request(&requests[3], "DELETE", NAMESPACE_ITEM, None);
}

#[tokio::test]
async fn lists_gets_and_updates_typed_pools() {
    let current = pool();
    let updated = Pool {
        spec: serde_json::from_value(serde_json::json!({
            "replicas": 2,
            "template": { "containerDiskImage": "registry.example/updated:latest" },
        }))
        .unwrap(),
        ..pool()
    };
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(
            200,
            &json_bytes(&serde_json::json!({ "items": [current.clone()] })),
        )),
        Ok(json_response(200, &current)),
        Ok(json_response(200, &updated)),
    ]));
    let client = client(Arc::clone(&http));

    assert_eq!(
        client.clone().list_pools(NAMESPACE.into()).await.unwrap(),
        vec![current.clone()]
    );
    assert_eq!(
        client.clone().get_pool(current.clone()).await.unwrap(),
        current
    );
    assert_eq!(
        client.update_pool(updated.clone()).await.unwrap(),
        updated.clone()
    );

    let requests = resource_requests(&http).await;
    assert_request(&requests[0], "GET", COLLECTION, None);
    assert_request(&requests[1], "GET", ITEM, None);
    assert_request(&requests[2], "PUT", ITEM, Some(&json_bytes(&updated)));
}

#[tokio::test]
async fn deletes_pool_before_namespace_and_accepts_empty_delete_bodies() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(204, b"")),
        Ok(response(204, b"")),
    ]));
    let client = client(Arc::clone(&http));

    client.delete_pool(pool()).await.unwrap();

    let requests = resource_requests(&http).await;
    assert_request(&requests[0], "DELETE", ITEM, None);
    assert_request(&requests[1], "DELETE", NAMESPACE_ITEM, None);
}

#[tokio::test]
async fn rejects_deleting_a_pool_outside_the_namespace_owned_lifecycle() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let client = client(Arc::clone(&http));
    let pool = Pool {
        metadata: ResourceMetadata {
            namespace: NAMESPACE.into(),
            name: "other-pool".into(),
            labels: None,
        },
        ..pool()
    };

    assert!(matches!(
        client.delete_pool(pool).await,
        Err(SdkError::Configuration { ref reason })
            if reason == "pool metadata namespace and name must match for namespace lifecycle operations"
    ));
    assert_eq!(http.request_count().await, 0);
}

#[tokio::test]
async fn gets_and_updates_non_lifecycle_pool_resources_without_namespace_cleanup() {
    let other = Pool {
        metadata: ResourceMetadata {
            namespace: NAMESPACE.into(),
            name: "other-pool".into(),
            labels: None,
        },
        ..pool()
    };
    let other_item = format!("{COLLECTION}/other-pool");
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &other)),
        Ok(json_response(200, &other)),
    ]));
    let client = client(Arc::clone(&http));

    assert_eq!(client.clone().get_pool(other.clone()).await.unwrap(), other);
    assert_eq!(
        client.update_pool(other.clone()).await.unwrap(),
        other.clone()
    );

    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 2);
    assert_request(&requests[0], "GET", &other_item, None);
    assert_request(&requests[1], "PUT", &other_item, Some(&json_bytes(&other)));
}

#[tokio::test]
async fn does_not_delete_namespace_when_pool_deletion_fails() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(500, b"pool delete failed")),
    ]));
    let client = client(Arc::clone(&http));

    assert!(matches!(
        client.delete_pool(pool()).await,
        Err(SdkError::Status {
            ref operation,
            status: 500,
            ref body,
        }) if operation == "delete pool" && body == "pool delete failed"
    ));

    let requests = resource_requests(&http).await;
    assert_eq!(requests.len(), 1);
    assert_request(&requests[0], "DELETE", ITEM, None);
}

#[tokio::test]
async fn maps_invalid_json_and_non_success_responses() {
    let invalid_json_http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(200, b"not json")),
    ]));
    let invalid_json_client = client(Arc::clone(&invalid_json_http));
    assert!(matches!(
        invalid_json_client.get_pool(pool()).await,
        Err(SdkError::Body { .. })
    ));

    let status_http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(503, b"unavailable")),
    ]));
    let status_client = client(Arc::clone(&status_http));
    assert!(matches!(
        status_client.list_pools(NAMESPACE.into()).await,
        Err(SdkError::Status { status: 503, .. })
    ));
}

#[tokio::test]
async fn rejects_invalid_dns_labels_without_http_requests() {
    for value in [
        "",
        "Uppercase",
        "-leading",
        "trailing-",
        "has/slash",
        &"a".repeat(64),
    ] {
        let http = Arc::new(ScriptedHttpClient::new([]));
        let client = client(Arc::clone(&http));

        let error = client.list_pools(value.into()).await.unwrap_err();
        assert!(matches!(
            error,
            SdkError::InvalidResourceName {
                ref field,
                value: ref invalid_value,
                ref reason,
            } if field == "namespace" && invalid_value == value && !reason.is_empty()
        ));
        assert_eq!(
            http.request_count().await,
            0,
            "{value:?} must not reach HTTP"
        );
    }

    for value in [
        "",
        "Uppercase",
        "-leading",
        "trailing-",
        "has/slash",
        &"a".repeat(64),
    ] {
        let http = Arc::new(ScriptedHttpClient::new([]));
        let client = client(Arc::clone(&http));
        let error = client
            .get_pool(Pool {
                metadata: ResourceMetadata {
                    namespace: NAMESPACE.into(),
                    name: value.into(),
                    labels: None,
                },
                ..pool()
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            SdkError::InvalidResourceName {
                ref field,
                value: ref invalid_value,
                ref reason,
            } if field == "name" && invalid_value == value && !reason.is_empty()
        ));
        assert_eq!(
            http.request_count().await,
            0,
            "{value:?} must not reach HTTP"
        );
    }
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

fn pool() -> Pool {
    pool_named(NAMESPACE)
}

fn pool_named(namespace: &str) -> Pool {
    Pool {
        api_version: "cua.ai/v1".into(),
        kind: "OSGymWorkspacePool".into(),
        metadata: ResourceMetadata {
            namespace: namespace.into(),
            name: namespace.into(),
            labels: None,
        },
        spec: pool_spec(),
        status: None,
    }
}

fn pool_spec() -> PoolSpec {
    serde_json::from_value(serde_json::json!({
        "replicas": 1,
        "template": { "containerDiskImage": "registry.example/image:latest" },
    }))
    .unwrap()
}

fn token() -> HttpResponse {
    response(200, br#"{"access_token":"token-a","expires_in":3600}"#)
}

fn json_response(status: u16, value: &impl serde::Serialize) -> HttpResponse {
    response(status, &json_bytes(value))
}

fn json_bytes(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn response(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: Vec::new(),
        body: body.to_vec(),
    }
}

async fn resource_requests(http: &ScriptedHttpClient) -> Vec<cyclops_sdk::HttpRequest> {
    http.requests().await.into_iter().skip(1).collect()
}

fn assert_request(
    request: &cyclops_sdk::HttpRequest,
    method: &str,
    url: &str,
    body: Option<&[u8]>,
) {
    assert_eq!(request.method, method);
    assert_eq!(request.url, url);
    assert_eq!(request.body.as_deref(), body);
    assert_eq!(
        request.headers,
        vec![
            HttpHeader {
                name: "accept".into(),
                value: "application/json".into(),
            },
            HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            },
            HttpHeader {
                name: "authorization".into(),
                value: "Bearer token-a".into(),
            },
        ]
    );
}
