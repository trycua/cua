use std::sync::Arc;

use cyclops_sdk::{
    Claim, CreateClaimRequest, CreatePoolRequest, CyclopsClient, CyclopsConfiguration,
    CyclopsCredentials, HttpClient, HttpError, HttpHeader, HttpRequest, HttpResponse, Pool,
    ResourceMetadata, Sandbox, SdkError,
};
use cyclops_sdk_schema::{ClaimSpec, OSGymSandboxClaimStatus, OSGymWorkspacePoolStatus, PoolSpec};

struct RecordingHttpClient;

#[async_trait::async_trait]
impl HttpClient for RecordingHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        assert_eq!(request.headers[0].name, "x-first");
        assert_eq!(request.headers[1].name, "x-second");
        assert_eq!(request.body, Some(vec![0, 255]));

        Ok(HttpResponse {
            status: 200,
            headers: vec![
                HttpHeader {
                    name: "x-first".into(),
                    value: "one".into(),
                },
                HttpHeader {
                    name: "x-second".into(),
                    value: "two".into(),
                },
            ],
            body: vec![255, 0],
        })
    }
}

fn configuration() -> CyclopsConfiguration {
    CyclopsConfiguration {
        base_url: "https://run.cua.ai".into(),
        token_url: "https://auth.cua.ai/token".into(),
        credentials: CyclopsCredentials::new("client".into(), "super-secret-value".into()),
        pool_poll_interval_ms: 1_000,
        pool_poll_limit: 300,
        claim_poll_interval_ms: 1_000,
        claim_poll_limit: 300,
    }
}

fn pool_spec() -> PoolSpec {
    serde_json::from_value(serde_json::json!({
        "replicas": 1,
        "template": { "containerDiskImage": "registry.example/image:latest" },
    }))
    .unwrap()
}

fn claim_spec() -> ClaimSpec {
    serde_json::from_value(serde_json::json!({
        "sandboxTemplateRef": { "name": "pool-template" },
    }))
    .unwrap()
}

fn pool() -> Pool {
    Pool {
        api_version: "cua.ai/v1".into(),
        kind: "OSGymWorkspacePool".into(),
        metadata: ResourceMetadata {
            namespace: "default".into(),
            name: "pool".into(),
            labels: None,
        },
        spec: pool_spec(),
        status: None,
    }
}

fn claim() -> Claim {
    Claim {
        api_version: "osgym.cua.ai/v1alpha1".into(),
        kind: "OSGymSandboxClaim".into(),
        metadata: ResourceMetadata {
            namespace: "default".into(),
            name: "claim".into(),
            labels: None,
        },
        spec: claim_spec(),
        status: None,
    }
}

#[test]
fn public_configuration_and_transport_are_constructible() {
    let configuration = configuration();
    assert_eq!(configuration.pool_poll_limit, 300);

    let client = CyclopsClient::connect(configuration, Arc::new(RecordingHttpClient));
    assert!(client.is_ok());
}

#[test]
fn configuration_rejects_invalid_urls_and_polling_values() {
    let mut invalid_url = configuration();
    invalid_url.base_url = "not a URL".into();
    assert!(matches!(
        CyclopsClient::connect(invalid_url, Arc::new(RecordingHttpClient)),
        Err(SdkError::Configuration { .. })
    ));

    let mut invalid_token_url = configuration();
    invalid_token_url.token_url = "not a URL".into();
    assert!(matches!(
        CyclopsClient::connect(invalid_token_url, Arc::new(RecordingHttpClient)),
        Err(SdkError::Configuration { .. })
    ));

    let mut zero_pool_interval = configuration();
    zero_pool_interval.pool_poll_interval_ms = 0;
    assert!(matches!(
        CyclopsClient::connect(zero_pool_interval, Arc::new(RecordingHttpClient)),
        Err(SdkError::Configuration { .. })
    ));

    let mut zero_interval = configuration();
    zero_interval.claim_poll_interval_ms = 0;
    assert!(matches!(
        CyclopsClient::connect(zero_interval, Arc::new(RecordingHttpClient)),
        Err(SdkError::Configuration { .. })
    ));

    let mut zero_limit = configuration();
    zero_limit.pool_poll_limit = 0;
    assert!(matches!(
        CyclopsClient::connect(zero_limit, Arc::new(RecordingHttpClient)),
        Err(SdkError::Configuration { .. })
    ));

    let mut zero_claim_limit = configuration();
    zero_claim_limit.claim_poll_limit = 0;
    assert!(matches!(
        CyclopsClient::connect(zero_claim_limit, Arc::new(RecordingHttpClient)),
        Err(SdkError::Configuration { .. })
    ));
}

#[test]
fn configuration_debug_redacts_client_secret() {
    let rendered = format!("{:?}", configuration());

    assert!(rendered.contains("<redacted>"));
    assert!(!rendered.contains("super-secret-value"));
}

#[tokio::test]
async fn foreign_http_client_preserves_ordered_headers_and_byte_bodies() {
    let response = RecordingHttpClient
        .execute(HttpRequest {
            method: "POST".into(),
            url: "https://run.cua.ai/v1/pools".into(),
            headers: vec![
                HttpHeader {
                    name: "x-first".into(),
                    value: "one".into(),
                },
                HttpHeader {
                    name: "x-second".into(),
                    value: "two".into(),
                },
            ],
            body: Some(vec![0, 255]),
        })
        .await
        .unwrap();

    assert_eq!(response.body, vec![255, 0]);
}

#[test]
fn http_request_distinguishes_absent_and_empty_bodies() {
    let absent = HttpRequest {
        method: "GET".into(),
        url: "https://run.cua.ai/v1/pools".into(),
        headers: vec![],
        body: None,
    };
    let empty = HttpRequest {
        body: Some(vec![]),
        ..absent.clone()
    };

    assert_ne!(absent, empty);
    assert_eq!(empty.body, Some(vec![]));
}

#[test]
fn resources_use_canonical_schema_specs_and_statuses() {
    let metadata = ResourceMetadata {
        namespace: "default".into(),
        name: "example".into(),
        labels: None,
    };
    assert_eq!(metadata.namespace, "default");

    fn pool_status(_: Option<OSGymWorkspacePoolStatus>) {}
    fn claim_status(_: Option<OSGymSandboxClaimStatus>) {}

    fn assert_pool_types(pool: Pool) {
        let Pool { spec, status, .. } = pool;
        let _: PoolSpec = spec;
        pool_status(status);
    }

    fn assert_claim_types(claim: Claim) {
        let Claim { spec, status, .. } = claim;
        let _: ClaimSpec = spec;
        claim_status(status);
    }

    fn assert_create_pool_request(request: CreatePoolRequest) {
        let CreatePoolRequest { namespace, spec } = request;
        let _: String = namespace;
        let _: PoolSpec = spec;
    }

    fn assert_create_claim_request(request: CreateClaimRequest) {
        let CreateClaimRequest { pool, spec } = request;
        assert_pool_types(pool);
        let _: Option<ClaimSpec> = spec;
    }

    let _: fn(Claim) = assert_claim_types;
    let _: fn(CreatePoolRequest) = assert_create_pool_request;
    let _: fn(CreateClaimRequest) = assert_create_claim_request;
}

#[test]
fn resources_support_equality_and_kubernetes_camel_case_json() {
    let pool = pool();
    let claim = claim();
    let sandbox = Sandbox {
        namespace: "default".into(),
        claim: "claim".into(),
        name: "bound-sandbox".into(),
        services: vec!["mcp".into(), "vnc".into()],
    };
    let create_pool = CreatePoolRequest {
        namespace: "default".into(),
        spec: pool.spec.clone(),
    };
    let create_claim = CreateClaimRequest {
        pool: pool.clone(),
        spec: Some(claim.spec.clone()),
    };

    assert_eq!(pool, pool.clone());
    assert_eq!(claim, claim.clone());
    assert_eq!(sandbox, sandbox.clone());
    assert_eq!(create_pool, create_pool.clone());
    assert_eq!(create_claim, create_claim.clone());

    let pool_json = serde_json::to_value(pool).unwrap();
    assert_eq!(pool_json["apiVersion"], "cua.ai/v1");
    assert!(pool_json.get("api_version").is_none());
    let claim_json = serde_json::to_value(claim).unwrap();
    assert_eq!(claim_json["apiVersion"], "osgym.cua.ai/v1alpha1");
    assert!(claim_json.get("api_version").is_none());
}

#[test]
fn sandbox_is_a_claim_bound_identity() {
    let sandbox = Sandbox {
        namespace: "default".into(),
        claim: "claim-a".into(),
        name: "sandbox-a".into(),
        services: vec!["mcp".into(), "vnc".into()],
    };

    assert_eq!(sandbox.namespace, "default");
    assert_eq!(sandbox.claim, "claim-a");
    assert_eq!(sandbox.name, "sandbox-a");
    assert_eq!(sandbox.services, vec!["mcp", "vnc"]);
}

#[test]
fn invalid_resource_names_have_a_distinct_error() {
    let error = SdkError::InvalidResourceName {
        field: "name".into(),
        value: "Invalid_Name".into(),
        reason: "must contain only lowercase ASCII letters, digits, and hyphens".into(),
    };

    assert!(matches!(error, SdkError::InvalidResourceName { .. }));
}

#[test]
fn status_error_body_is_lossy_and_bounded_to_4096_bytes() {
    let mut body = vec![b'a'; 4_093];
    body.extend("😀".as_bytes());
    let error = SdkError::status("list pools", 500, &body);

    match error {
        SdkError::Status { body, .. } => {
            assert_eq!(body, "a".repeat(4_093));
            assert!(body.len() <= 4_096);
            assert!(!body.contains('\u{fffd}'));
        }
        _ => panic!("expected a status error"),
    }
}
