mod support;

use cyclops_sdk::{
    Claim, CreateClaimRequest, CyclopsClient, CyclopsConfiguration, CyclopsCredentials, HttpHeader,
    HttpResponse, Pool, ResourceMetadata, SdkError,
};
use cyclops_sdk_schema::ClaimSpec;
use std::sync::Arc;
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example:8443/prefix";
const TOKEN_URL: &str = "https://identity.example/oauth/token";
const NAMESPACE: &str = "example-pool";
const CLAIM_COLLECTION: &str = "https://cyclops.example:8443/prefix/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxclaims";
const POOL_ITEM: &str = "https://cyclops.example:8443/prefix/api/k8s/apis/cua.ai/v1/namespaces/example-pool/osgymworkspacepools/example-pool";

#[tokio::test]
async fn creates_pending_demand_immediately_for_a_nonzero_unavailable_pool() {
    let spec = claim_spec("explicit-template");
    let created = claim("claim-1", spec.clone(), None);
    let mut unavailable_pool = pool(1, None);
    unavailable_pool.status = Some(
        serde_json::from_value(serde_json::json!({ "availableCount": 0, "phase": "Pending" }))
            .unwrap(),
    );
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(201, &created)),
    ]));

    assert_eq!(
        client(Arc::clone(&http), 2, 2)
            .create_claim(CreateClaimRequest {
                pool: unavailable_pool,
                spec: Some(spec),
            })
            .await
            .unwrap(),
        created
    );

    let requests = http.authenticated_requests().await;
    assert_eq!(requests.len(), 1);
    assert_request(
        &requests[0],
        "POST",
        CLAIM_COLLECTION,
        Some(&json_bytes(&created)),
    );
}

#[tokio::test]
async fn zero_and_nonzero_pools_post_claims_with_identical_sequencing() {
    for replicas in [0, 1] {
        let expected = claim("claim-1", claim_spec("example-pool-template"), None);
        let http = Arc::new(ScriptedHttpClient::new([
            Ok(token()),
            Ok(json_response(201, &expected)),
        ]));

        client(Arc::clone(&http), 2, 2)
            .create_claim(CreateClaimRequest {
                pool: pool(replicas, None),
                spec: None,
            })
            .await
            .unwrap();

        let requests = http.authenticated_requests().await;
        assert_eq!(requests.len(), 1, "replicas={replicas}");
        assert_request(
            &requests[0],
            "POST",
            CLAIM_COLLECTION,
            Some(&json_bytes(&expected)),
        );
    }
}
#[tokio::test]
async fn claim_crud_uses_prefixed_routes_and_expected_statuses() {
    let current = claim("named-claim", claim_spec("example-pool-template"), None);
    let item = format!("{CLAIM_COLLECTION}/named-claim");
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(
            200,
            &serde_json::json!({ "items": [current.clone()] }),
        )),
        Ok(json_response(200, &current)),
        Ok(response(204, b"")),
    ]));
    let client = client(Arc::clone(&http), 1, 1);

    assert_eq!(
        client.clone().list_claims(NAMESPACE.into()).await.unwrap(),
        vec![current.clone()]
    );
    assert_eq!(
        client.clone().get_claim(current.clone()).await.unwrap(),
        current
    );
    client.clone().delete_claim(current.clone()).await.unwrap();

    let requests = http.authenticated_requests().await;
    assert_request(&requests[0], "GET", CLAIM_COLLECTION, None);
    assert_request(&requests[1], "GET", &item, None);
    assert_request(&requests[2], "DELETE", &item, None);
}

#[tokio::test]
async fn wait_claim_returns_bound_sandbox_with_sorted_deduplicated_pool_services() {
    let current = claim(
        "named-claim",
        claim_spec("example-pool-template"),
        Some(serde_json::json!({
            "phase": "Bound",
            "sandbox": { "name": "sandbox-42", "service": "internal-only" },
        })),
    );
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &current)),
        Ok(json_response(200, &pool(1, Some(vec!["z", "a", "z"])))),
    ]));
    let client = client(Arc::clone(&http), 1, 2);

    assert_eq!(
        client.wait_claim(current.clone()).await.unwrap(),
        cyclops_sdk::Sandbox {
            namespace: NAMESPACE.into(),
            claim: "named-claim".into(),
            name: "sandbox-42".into(),
            services: vec!["a".into(), "z".into()],
        }
    );

    let requests = http.authenticated_requests().await;
    assert_request(
        &requests[0],
        "GET",
        &format!("{CLAIM_COLLECTION}/named-claim"),
        None,
    );
    assert_request(&requests[1], "GET", POOL_ITEM, None);
}

#[tokio::test]
async fn wait_claim_reports_terminal_and_timeout_statuses() {
    let failed = claim(
        "named-claim",
        claim_spec("example-pool-template"),
        Some(serde_json::json!({ "phase": "Failed", "conditions": [] })),
    );
    let failed_http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &failed)),
    ]));
    assert!(matches!(
        client(Arc::clone(&failed_http), 1, 2).wait_claim(failed).await,
        Err(SdkError::ClaimFailed { ref phase, ref status })
            if phase == "Failed" && status.contains("conditions")
    ));

    let pending = claim("named-claim", claim_spec("example-pool-template"), None);
    let timeout_http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &pending)),
        Ok(json_response(200, &pending)),
    ]));
    assert!(matches!(
        client(Arc::clone(&timeout_http), 1, 2)
            .wait_claim(pending)
            .await,
        Err(SdkError::ClaimTimeout)
    ));
    assert_eq!(timeout_http.authenticated_requests().await.len(), 2);
}

#[tokio::test]
async fn generated_claim_names_are_unique_under_concurrency_and_fit_dns_labels() {
    let pool_name = "a".repeat(63);
    let pool = pool_named(&pool_name, 0, None);
    let first = claim("placeholder", claim_spec("short-template"), None);
    let second = first.clone();
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(201, &first)),
        Ok(json_response(201, &second)),
    ]));
    let client = client(Arc::clone(&http), 1, 1);

    let (first, second) = tokio::join!(
        client.clone().create_claim(CreateClaimRequest {
            pool: pool.clone(),
            spec: Some(claim_spec("short-template"))
        }),
        client.create_claim(CreateClaimRequest {
            pool,
            spec: Some(claim_spec("short-template"))
        }),
    );
    assert!(first.is_ok());
    assert!(second.is_ok());

    let requests = http.authenticated_requests().await;
    let names: Vec<String> = requests
        .iter()
        .map(|request| {
            let body: serde_json::Value =
                serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
            body["metadata"]["name"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1]);
    assert!(names.iter().all(|name| name.len() <= 63));
    assert!(names.iter().all(|name| {
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    }));
}

#[tokio::test]
async fn validation_and_malformed_responses_fail_without_unexpected_http() {
    let invalid_http = Arc::new(ScriptedHttpClient::new([]));
    let invalid_pool = Pool {
        metadata: ResourceMetadata {
            namespace: "Uppercase".into(),
            ..pool(0, None).metadata
        },
        ..pool(0, None)
    };
    assert!(matches!(
        client(Arc::clone(&invalid_http), 1, 1)
            .create_claim(CreateClaimRequest {
                pool: invalid_pool,
                spec: None
            })
            .await,
        Err(SdkError::InvalidResourceName { .. })
    ));
    assert_eq!(invalid_http.request_count().await, 0);

    let invalid_name_http = Arc::new(ScriptedHttpClient::new([]));
    let invalid_claim = Claim {
        metadata: ResourceMetadata {
            namespace: NAMESPACE.into(),
            name: "Uppercase".into(),
            labels: None,
        },
        ..claim("valid-claim", claim_spec("example-pool-template"), None)
    };
    assert!(matches!(
        client(Arc::clone(&invalid_name_http), 1, 1)
            .get_claim(invalid_claim)
            .await,
        Err(SdkError::InvalidResourceName { .. })
    ));
    assert_eq!(invalid_name_http.request_count().await, 0);

    let malformed_http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(200, b"not json")),
    ]));
    assert!(matches!(
        client(Arc::clone(&malformed_http), 1, 1)
            .list_claims(NAMESPACE.into())
            .await,
        Err(SdkError::Body { .. })
    ));
}

fn client(http: Arc<ScriptedHttpClient>, pool_limit: u32, claim_limit: u32) -> Arc<CyclopsClient> {
    CyclopsClient::connect(
        CyclopsConfiguration {
            base_url: BASE_URL.into(),
            token_url: TOKEN_URL.into(),
            credentials: CyclopsCredentials::new("client".into(), "secret".into()),
            pool_poll_interval_ms: 1,
            pool_poll_limit: pool_limit,
            claim_poll_interval_ms: 1,
            claim_poll_limit: claim_limit,
        },
        http,
    )
    .unwrap()
}

fn pool(replicas: u32, services: Option<Vec<&str>>) -> Pool {
    pool_named(NAMESPACE, replicas, services)
}

fn pool_named(name: &str, replicas: u32, services: Option<Vec<&str>>) -> Pool {
    let services = services.map(|services| {
        services
            .into_iter()
            .map(|name| serde_json::json!({ "name": name, "targetPort": 8080 }))
            .collect::<Vec<_>>()
    });
    Pool {
        api_version: "cua.ai/v1".into(),
        kind: "OSGymWorkspacePool".into(),
        metadata: ResourceMetadata {
            namespace: name.into(),
            name: name.into(),
            labels: None,
        },
        spec: serde_json::from_value(serde_json::json!({
            "replicas": replicas,
            "template": { "containerDiskImage": "registry.example/image:latest" },
            "services": services,
        }))
        .unwrap(),
        status: None,
    }
}

fn claim(name: &str, spec: ClaimSpec, status: Option<serde_json::Value>) -> Claim {
    Claim {
        api_version: "osgym.cua.ai/v1alpha1".into(),
        kind: "OSGymSandboxClaim".into(),
        metadata: ResourceMetadata {
            namespace: NAMESPACE.into(),
            name: name.into(),
            labels: None,
        },
        spec,
        status: status.map(|status| serde_json::from_value(status).unwrap()),
    }
}

fn claim_spec(template: &str) -> ClaimSpec {
    serde_json::from_value(serde_json::json!({ "sandboxTemplateRef": { "name": template } }))
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
                value: "application/json".into()
            },
            HttpHeader {
                name: "content-type".into(),
                value: "application/json".into()
            },
            HttpHeader {
                name: "authorization".into(),
                value: "Bearer token-a".into()
            },
        ]
    );
}

#[tokio::test]
async fn default_template_ref_for_a_63_byte_pool_passes_through_without_dns_validation() {
    let pool_name = "a".repeat(63);
    let template_name = format!("{pool_name}-template");
    let pool = pool_named(&pool_name, 0, None);
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(
            201,
            &claim("claim-1", claim_spec(&template_name), None),
        )),
    ]));

    client(Arc::clone(&http), 1, 1)
        .create_claim(CreateClaimRequest { pool, spec: None })
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["spec"]["sandboxTemplateRef"]["name"], template_name);
}

#[tokio::test]
async fn explicit_non_dns_template_ref_passes_through_but_empty_ref_is_rejected_before_http() {
    let template_name = "Team_Name/Template:with spaces";
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(
            201,
            &claim("claim-1", claim_spec(template_name), None),
        )),
    ]));
    client(Arc::clone(&http), 1, 1)
        .create_claim(CreateClaimRequest {
            pool: pool(0, None),
            spec: Some(claim_spec(template_name)),
        })
        .await
        .unwrap();
    let requests = http.authenticated_requests().await;
    let body: serde_json::Value =
        serde_json::from_slice(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["spec"]["sandboxTemplateRef"]["name"], template_name);

    let empty_http = Arc::new(ScriptedHttpClient::new([]));
    assert!(matches!(
        client(Arc::clone(&empty_http), 1, 1)
            .create_claim(CreateClaimRequest {
                pool: pool(0, None),
                spec: Some(claim_spec("")),
            })
            .await,
        Err(SdkError::Configuration { .. })
    ));
    assert_eq!(empty_http.request_count().await, 0);
}

#[tokio::test]
async fn wait_claim_uses_controller_removesuffix_behavior_for_template_refs_without_suffixes() {
    let current = claim(
        "named-claim",
        claim_spec("example-pool"),
        Some(serde_json::json!({
            "phase": "Bound",
            "sandbox": { "name": "sandbox-42" },
        })),
    );
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &current)),
        Ok(json_response(200, &pool(1, Some(vec!["shell"])))),
    ]));

    assert_eq!(
        client(Arc::clone(&http), 1, 1)
            .wait_claim(current)
            .await
            .unwrap()
            .name,
        "sandbox-42"
    );
    let requests = http.authenticated_requests().await;
    assert_request(
        &requests[0],
        "GET",
        &format!("{CLAIM_COLLECTION}/named-claim"),
        None,
    );
    assert_request(&requests[1], "GET", POOL_ITEM, None);
}

#[tokio::test]
async fn wait_claim_rejects_invalid_claim_identity_before_token_or_pool_lookup() {
    let http = Arc::new(ScriptedHttpClient::new([]));
    let invalid = Claim {
        metadata: ResourceMetadata {
            namespace: NAMESPACE.into(),
            name: "Uppercase".into(),
            labels: None,
        },
        ..claim("named-claim", claim_spec("example-pool-template"), None)
    };

    assert!(matches!(
        client(Arc::clone(&http), 1, 1).wait_claim(invalid).await,
        Err(SdkError::InvalidResourceName { .. })
    ));
    assert_eq!(http.request_count().await, 0);
}
