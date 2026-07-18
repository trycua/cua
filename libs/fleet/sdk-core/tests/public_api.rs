use cyclops_sdk_core::{
    AccessToken, Claim, Configuration, CreateClaimRequest, CreatePoolRequest, HttpRequest,
    HttpResponse, OAuthCredentials, Platform, PlatformError, Pool, PoolSpec, PoolTemplate, Sandbox,
    Sdk,
};
use std::collections::VecDeque;

#[derive(Default)]
struct FakePlatform {
    responses: VecDeque<HttpResponse>,
    requests: Vec<HttpRequest>,
    token: Option<AccessToken>,
}

impl Platform for FakePlatform {
    fn http(&mut self, request: HttpRequest) -> Result<HttpResponse, PlatformError> {
        self.requests.push(request);
        self.responses
            .pop_front()
            .ok_or_else(|| PlatformError::Transport("missing response".into()))
    }
    fn sleep(&mut self, _milliseconds: u64) -> Result<(), PlatformError> {
        Ok(())
    }
    fn now_ms(&mut self) -> u64 {
        1_000
    }
    fn acquire_oauth_credentials(&mut self) -> Result<OAuthCredentials, PlatformError> {
        unreachable!()
    }
    fn load_access_token(&mut self) -> Result<Option<AccessToken>, PlatformError> {
        Ok(self.token.clone())
    }
    fn store_access_token(&mut self, token: &AccessToken) -> Result<(), PlatformError> {
        self.token = Some(token.clone());
        Ok(())
    }
}

fn connected(responses: impl IntoIterator<Item = (u16, String)>) -> Sdk<FakePlatform> {
    Sdk::connect(
        FakePlatform {
            responses: responses
                .into_iter()
                .map(|(status, body)| HttpResponse { status, body })
                .collect(),
            token: Some(AccessToken {
                value: "cached".into(),
                expires_at_ms: 301_001,
            }),
            ..FakePlatform::default()
        },
        Configuration {
            protocol_version: cyclops_sdk_core::PROTOCOL_VERSION,
            base_url: "https://run.example".into(),
        },
    )
    .unwrap()
}

fn pool() -> Pool {
    serde_json::from_str(r#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"pool-one","name":"pool-one"},"spec":{"replicas":1,"template":{"containerDiskImage":"example/workspace:latest","cpuCores":4,"memory":"4Gi"}}}"#).unwrap()
}

fn claim() -> Claim {
    serde_json::from_str(r#"{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"pool-one","name":"claim-one"},"spec":{"sandboxTemplateRef":{"name":"pool-one-template"}}}"#).unwrap()
}

#[test]
fn canonical_pool_and_claim_crud_uses_typed_resources() {
    let pool_json = serde_json::to_string(&pool()).unwrap();
    let claim_json = serde_json::to_string(&claim()).unwrap();
    let list_pools = format!(r#"{{"items":[{pool_json}]}}"#);
    let list_claims = format!(r#"{{"items":[{claim_json}]}}"#);
    let bound_claim = r#"{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"pool-one","name":"claim-one"},"spec":{"sandboxTemplateRef":{"name":"pool-one-template"}},"status":{"phase":"Bound","sandbox":{"name":"sandbox-one"}}}"#;
    let mut sdk = connected([
        (201, r#"{"name":"pool-one","status":"Active"}"#.into()),
        (201, pool_json.clone()),
        (200, list_pools),
        (200, pool_json.clone()),
        (200, pool_json),
        (
            200,
            r#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"pool-one","name":"pool-one"},"spec":{"replicas":1,"template":{"containerDiskImage":"example/workspace:latest","cpuCores":4,"memory":"4Gi"}},"status":{"phase":"Ready","availableCount":1,"totalCount":1}}"#.into(),
        ),
        (201, claim_json.clone()),
        (200, list_claims),
        (200, claim_json.clone()),
        (200, claim_json),
        (200, bound_claim.into()),
        (204, String::new()),
        (204, String::new()),
        (204, String::new()),
    ]);

    let pool = sdk
        .create_pool(CreatePoolRequest {
            namespace: "pool-one".into(),
            spec: PoolSpec::builder()
                .replicas(1)
                .template(
                    PoolTemplate::builder()
                        .container_disk_image("example/workspace:latest".into())
                        .cpu_cores(4)
                        .memory("4Gi".into())
                        .build()
                        .unwrap(),
                )
                .build()
                .unwrap(),
        })
        .unwrap();
    sdk.list_pools("pool-one").unwrap();
    sdk.get_pool(pool.clone()).unwrap();
    sdk.update_pool(pool.clone()).unwrap();
    let claim = sdk
        .create_claim(CreateClaimRequest {
            pool: pool.clone(),
            spec: None,
        })
        .unwrap();
    sdk.list_claims("pool-one").unwrap();
    sdk.get_claim(claim.clone()).unwrap();
    sdk.update_claim(claim.clone()).unwrap();
    let Sandbox { sandbox, .. } = sdk.wait_claim(claim.clone()).unwrap();
    sdk.delete_claim(claim).unwrap();
    sdk.delete_pool(pool).unwrap();
    assert_eq!(sandbox, "sandbox-one");

    let methods = sdk
        .platform()
        .requests
        .iter()
        .map(|request| request.method.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        [
            "POST", "POST", "GET", "GET", "PUT", "GET", "POST", "GET", "GET", "PUT", "GET",
            "DELETE", "DELETE", "DELETE"
        ]
    );
}

#[test]
fn wit_exports_typed_resources_and_service_http() {
    let wit = format!(
        "{}\n{}",
        include_str!("../wit/cyclops-sdk.wit"),
        include_str!("../wit/generated-crd.wit")
    );
    for resource in [
        "record pool",
        "record pool-spec",
        "record claim",
        "record claim-spec",
        "record sandbox",
    ] {
        assert!(wit.contains(resource), "missing typed {resource}");
    }
    assert!(!wit.contains("pool-json"));
    assert!(!wit.contains("claim-json"));
    for operation in [
        "create-pool",
        "list-pools",
        "get-pool",
        "update-pool",
        "delete-pool",
        "create-claim",
        "list-claims",
        "get-claim",
        "update-claim",
        "delete-claim",
        "wait-claim",
        "service-request",
    ] {
        assert!(wit.contains(operation), "missing {operation}");
    }
    assert!(!wit.contains("raw-request"));
    assert!(wit.contains("record service-http-request"));
    assert!(wit.contains("record service-http-response"));
    for leaked_url in ["server-url", "mcp-url", "novnc-url"] {
        assert!(!wit.contains(leaked_url), "leaked {leaked_url}");
    }
    for prohibited in [
        concat!("create", "-namespace"),
        concat!("release", "-claim"),
        concat!("claim", "-status"),
        concat!("claimed", "-sandbox"),
        concat!("run", "-task"),
        concat!("run", "-call", "back"),
    ] {
        assert!(!wit.contains(prohibited), "prohibited {prohibited}");
    }
}
