use cyclops_sdk_core::{
    AccessToken, Claim, ClaimUrl, Configuration, CreateClaimRequest, CreatePoolRequest,
    HttpRequest, HttpResponse, OAuthCredentials, Platform, PlatformError, PoolSpec, PoolTemplate,
    PoolUrl, Sandbox, Sdk, SdkError, ServiceHttpRequest,
};
use serde_json::json;
use std::collections::VecDeque;

#[derive(Default)]
struct FakePlatform {
    responses: VecDeque<HttpResponse>,
    requests: Vec<HttpRequest>,
    sleeps: Vec<u64>,
    token: Option<AccessToken>,
    oauth: Option<OAuthCredentials>,
    credential_requests: usize,
}

impl Platform for FakePlatform {
    fn http(&mut self, request: HttpRequest) -> Result<HttpResponse, PlatformError> {
        self.requests.push(request);
        self.responses
            .pop_front()
            .ok_or_else(|| PlatformError::Transport("missing response".into()))
    }
    fn sleep(&mut self, milliseconds: u64) -> Result<(), PlatformError> {
        self.sleeps.push(milliseconds);
        Ok(())
    }
    fn now_ms(&mut self) -> u64 {
        1_000
    }
    fn acquire_oauth_credentials(&mut self) -> Result<OAuthCredentials, PlatformError> {
        self.credential_requests += 1;
        Ok(self.oauth.clone().unwrap_or_else(|| OAuthCredentials {
            token_url: "https://auth.example/token".into(),
            client_id: "id".into(),
            client_secret: "secret".into(),
        }))
    }
    fn load_access_token(&mut self) -> Result<Option<AccessToken>, PlatformError> {
        Ok(self.token.clone())
    }
    fn store_access_token(&mut self, token: &AccessToken) -> Result<(), PlatformError> {
        self.token = Some(token.clone());
        Ok(())
    }
}

fn claim() -> Claim {
    serde_json::from_value(json!({
        "apiVersion":"osgym.cua.ai/v1alpha1", "kind":"OSGymSandboxClaim",
        "metadata":{"namespace":"pool-a","name":"claim-a"},
        "spec":{"sandboxTemplateRef":{"name":"pool-a-template"}}
    }))
    .unwrap()
}

fn connected(responses: Vec<HttpResponse>) -> Sdk<FakePlatform> {
    let platform = FakePlatform {
        responses: responses.into(),
        token: Some(AccessToken {
            value: "cached".into(),
            expires_at_ms: 301_001,
        }),
        ..FakePlatform::default()
    };
    Sdk::connect(
        platform,
        Configuration {
            protocol_version: cyclops_sdk_core::PROTOCOL_VERSION,
            base_url: "https://run.example".into(),
        },
    )
    .unwrap()
}

#[test]
fn canonical_resource_urls_are_namespaced() {
    let pool = PoolUrl::new("https://run.example/", "pool-a", "pool-a").unwrap();
    assert_eq!(
        pool.collection().as_str(),
        "https://run.example/api/k8s/apis/cua.ai/v1/namespaces/pool-a/osgymworkspacepools"
    );
    assert_eq!(
        pool.item().as_str(),
        "https://run.example/api/k8s/apis/cua.ai/v1/namespaces/pool-a/osgymworkspacepools/pool-a"
    );
    let claim = ClaimUrl::new("https://run.example", "pool-a", "claim-a").unwrap();
    assert_eq!(claim.collection().as_str(), "https://run.example/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/pool-a/osgymsandboxclaims");
    assert_eq!(claim.item().as_str(), "https://run.example/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/pool-a/osgymsandboxclaims/claim-a");
}

fn sandbox_with_services() -> Sandbox {
    Sandbox {
        namespace: "pool-a".into(),
        claim: "claim-a".into(),
        sandbox: "sandbox-a".into(),
        services: vec!["mcp".into(), "novnc".into()],
    }
}

#[test]
fn service_request_resolves_relative_path_and_reuses_auth_retry() {
    let mut sdk = connected(vec![
        HttpResponse {
            status: 401,
            body: "expired".into(),
        },
        HttpResponse {
            status: 200,
            body: r#"{"access_token":"fresh","expires_in":300}"#.into(),
        },
        HttpResponse {
            status: 200,
            body: "initialized".into(),
        },
    ]);
    let response = sdk
        .service_request(
            &sandbox_with_services(),
            "mcp",
            ServiceHttpRequest {
                method: "POST".into(),
                path: "/mcp?session=one".into(),
                headers: vec![
                    ("content-type".into(), "application/json".into()),
                    ("authorization".into(), "Caller token".into()),
                    ("host".into(), "cyclops.invalid".into()),
                    ("connection".into(), "keep-alive".into()),
                ],
                body: Some("{}".into()),
            },
        )
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "initialized");
    assert_eq!(sdk.platform().requests.len(), 3);
    assert_eq!(
        sdk.platform().requests[0].url,
        "https://run.example/api/svc/pool-a/sandbox-a-mcp/mcp?session=one"
    );
    assert_eq!(
        sdk.platform().requests[2].url,
        sdk.platform().requests[0].url
    );
    assert!(sdk.platform().requests[0]
        .headers
        .contains(&("authorization".into(), "Bearer cached".into())));
    assert!(!sdk.platform().requests[0]
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host")
            || name.eq_ignore_ascii_case("connection")));
    assert!(sdk.platform().requests[2]
        .headers
        .contains(&("authorization".into(), "Bearer fresh".into())));
}

#[test]
fn service_request_rejects_unknown_service_with_available_names() {
    let mut sdk = connected(vec![]);
    let error = sdk
        .service_request(
            &sandbox_with_services(),
            "metrics",
            ServiceHttpRequest::get("/health"),
        )
        .unwrap_err();
    assert_eq!(
        error,
        SdkError::UnknownService {
            requested: "metrics".into(),
            available: vec!["mcp".into(), "novnc".into()],
        }
    );
    assert!(sdk.platform().requests.is_empty());
}

#[test]
fn service_request_rejects_absolute_urls() {
    let mut sdk = connected(vec![]);
    let error = sdk
        .service_request(
            &sandbox_with_services(),
            "mcp",
            ServiceHttpRequest::get("https://attacker.example/mcp"),
        )
        .unwrap_err();
    assert_eq!(
        error,
        SdkError::InvalidServicePath("https://attacker.example/mcp".into())
    );
    assert!(sdk.platform().requests.is_empty());
}

#[test]
fn service_request_rejects_forged_route_parts() {
    let mut sdk = connected(vec![]);
    let mut sandbox = sandbox_with_services();
    sandbox.sandbox = "../other".into();
    let error = sdk
        .service_request(&sandbox, "mcp", ServiceHttpRequest::get("/mcp"))
        .unwrap_err();
    assert!(matches!(error, SdkError::Configuration(_)));
    assert!(sdk.platform().requests.is_empty());
}

#[test]
fn wait_claim_reuses_get_claim_until_bound() {
    let pending = json!({
        "apiVersion":"osgym.cua.ai/v1alpha1", "kind":"OSGymSandboxClaim",
        "metadata":{"namespace":"pool-a","name":"claim-a"},
        "spec":{"sandboxTemplateRef":{"name":"pool-a-template"}},
        "status":{"phase":"Pending"}
    })
    .to_string();
    let bound = json!({
        "apiVersion":"osgym.cua.ai/v1alpha1", "kind":"OSGymSandboxClaim",
        "metadata":{"namespace":"pool-a","name":"claim-a"},
        "spec":{"sandboxTemplateRef":{"name":"pool-a-template"}},
        "status":{"phase":"Bound","sandbox":{"name":"sandbox-a"}}
    })
    .to_string();
    let pool = json!({
        "apiVersion":"cua.ai/v1", "kind":"OSGymWorkspacePool",
        "metadata":{"namespace":"pool-a","name":"pool-a"},
        "spec":{"replicas":1,"services":[{"name":"mcp","targetPort":3000}],"template":{"containerDiskImage":"example/workspace:latest","cpuCores":4,"memory":"4Gi"}}
    })
    .to_string();
    let mut sdk = connected(vec![
        HttpResponse {
            status: 200,
            body: pool,
        },
        HttpResponse {
            status: 200,
            body: pending,
        },
        HttpResponse {
            status: 200,
            body: bound,
        },
    ]);
    let sandbox = sdk.wait_claim(claim()).unwrap();
    assert_eq!(sandbox.sandbox, "sandbox-a");
    assert_eq!(sdk.platform().requests.len(), 3);
    assert_eq!(sandbox.services, ["mcp"]);
    assert!(sdk
        .platform()
        .requests
        .iter()
        .all(|request| request.method == "GET"));
    assert_eq!(sdk.platform().sleeps, [5_000]);
}

#[test]
fn oauth_credentials_are_form_encoded() {
    let platform = FakePlatform {
        responses: vec![
            HttpResponse {
                status: 200,
                body: r#"{"access_token":"fresh","expires_in":300}"#.into(),
            },
            HttpResponse {
                status: 200,
                body: "ok".into(),
            },
        ]
        .into(),
        oauth: Some(OAuthCredentials {
            token_url: "https://auth.example/token".into(),
            client_id: "id&+= %".into(),
            client_secret: "s/e?c#r&et=+ %".into(),
        }),
        ..FakePlatform::default()
    };
    let mut sdk = Sdk::connect(
        platform,
        Configuration {
            protocol_version: cyclops_sdk_core::PROTOCOL_VERSION,
            base_url: "https://run.example".into(),
        },
    )
    .unwrap();
    sdk.service_request(
        &sandbox_with_services(),
        "mcp",
        ServiceHttpRequest::get("/mcp"),
    )
    .unwrap();
    assert_eq!(
        sdk.platform().requests[0].body.as_deref(),
        Some("grant_type=client_credentials&client_id=id%26%2B%3D+%25&client_secret=s%2Fe%3Fc%23r%26et%3D%2B+%25")
    );
}

#[test]
fn compact_specs_are_expanded_into_canonical_resources() {
    let pool_body = r#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"pool-one","name":"pool-one"},"spec":{"replicas":1,"template":{"containerDiskImage":"example/workspace:latest","cpuCores":4,"memory":"4Gi"}}}"#;
    let pool_response = pool_body.to_string();
    let claim_response = serde_json::to_string(&claim()).unwrap();
    let claim_body = r#"{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"pool-one","name":"claim-3e8-0"},"spec":{"sandboxTemplateRef":{"name":"pool-one-template"}}}"#;
    let mut sdk = connected(vec![
        HttpResponse {
            status: 201,
            body: r#"{"name":"pool-one","status":"Active"}"#.into(),
        },
        HttpResponse {
            status: 201,
            body: pool_response.clone(),
        },
        HttpResponse {
            status: 200,
            body: ready_pool_body(),
        },
        HttpResponse {
            status: 201,
            body: claim_response,
        },
    ]);
    let created_pool = sdk
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
    sdk.create_claim(CreateClaimRequest {
        pool: created_pool,
        spec: None,
    })
    .unwrap();
    assert_eq!(sdk.platform().requests.len(), 4);
    assert_eq!(
        sdk.platform().requests[0].url,
        "https://run.example/api/namespaces"
    );
    assert_eq!(
        sdk.platform().requests[0].body.as_deref(),
        Some(r#"{"name":"pool-one"}"#)
    );
    assert_eq!(sdk.platform().requests[1].body.as_deref(), Some(pool_body));
    assert_eq!(sdk.platform().requests[2].method, "GET");
    assert_eq!(sdk.platform().requests[3].body.as_deref(), Some(claim_body));
}

#[test]
fn create_claim_waits_for_a_requested_pool_replica_to_become_available() {
    let pending = serde_json::from_str::<cyclops_sdk_core::Pool>(
        r#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"pool-one","name":"pool-one"},"spec":{"replicas":1,"template":{"containerDiskImage":"example/workspace:latest","cpuCores":4,"memory":"4Gi"}},"status":{"phase":"Provisioning","availableCount":0,"totalCount":1}}"#,
    )
    .unwrap();
    let pool = pool_resource();
    let claim_response = serde_json::to_string(&claim()).unwrap();
    let mut sdk = connected(vec![
        HttpResponse {
            status: 200,
            body: serde_json::to_string(&pending).unwrap(),
        },
        HttpResponse {
            status: 200,
            body: ready_pool_body(),
        },
        HttpResponse {
            status: 201,
            body: claim_response,
        },
    ]);
    sdk.create_claim(CreateClaimRequest { pool, spec: None })
        .unwrap();
    assert_eq!(sdk.platform().requests.len(), 3);
    assert_eq!(
        sdk.platform()
            .requests
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        ["GET", "GET", "POST"]
    );
    assert_eq!(sdk.platform().sleeps, [5_000]);
}

#[test]
fn create_claim_does_not_wait_for_a_zero_replica_pool() {
    let mut pool = pool_resource();
    pool.spec["replicas"] = serde_json::Value::from(0);
    let mut sdk = connected(vec![HttpResponse {
        status: 201,
        body: serde_json::to_string(&claim()).unwrap(),
    }]);
    sdk.create_claim(CreateClaimRequest { pool, spec: None })
        .unwrap();
    assert_eq!(sdk.platform().requests.len(), 1);
    assert_eq!(sdk.platform().requests[0].method, "POST");
}

#[test]
fn create_pool_reuses_an_existing_namespace() {
    let pool_body = r#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"pool-one","name":"pool-one"},"spec":{"replicas":1,"template":{"containerDiskImage":"example/workspace:latest","cpuCores":4,"memory":"4Gi"}}}"#;
    let mut sdk = connected(vec![
        HttpResponse {
            status: 409,
            body: "namespace already exists".into(),
        },
        HttpResponse {
            status: 201,
            body: pool_body.into(),
        },
    ]);
    sdk.create_pool(pool_request()).unwrap();
    assert_eq!(sdk.platform().requests.len(), 2);
}

#[test]
fn create_pool_rolls_back_a_new_namespace_when_pool_creation_fails() {
    let mut sdk = connected(vec![
        HttpResponse {
            status: 201,
            body: r#"{"name":"pool-one","status":"Active"}"#.into(),
        },
        HttpResponse {
            status: 403,
            body: "forbidden".into(),
        },
        HttpResponse {
            status: 202,
            body: String::new(),
        },
    ]);
    let error = sdk.create_pool(pool_request()).unwrap_err();
    assert!(error.to_string().contains("create pool returned HTTP 403"));
    assert_eq!(sdk.platform().requests.len(), 3);
    assert_eq!(sdk.platform().requests[2].method, "DELETE");
    assert_eq!(
        sdk.platform().requests[2].url,
        "https://run.example/api/namespaces/pool-one"
    );
}

#[test]
fn delete_pool_deletes_the_pool_before_its_namespace() {
    let mut sdk = connected(vec![
        HttpResponse {
            status: 202,
            body: String::new(),
        },
        HttpResponse {
            status: 202,
            body: String::new(),
        },
    ]);
    let pool = pool_resource();
    sdk.delete_pool(pool).unwrap();
    assert_eq!(sdk.platform().requests.len(), 2);
    assert!(sdk.platform().requests[0]
        .url
        .ends_with("/osgymworkspacepools/pool-one"));
    assert_eq!(
        sdk.platform().requests[1].url,
        "https://run.example/api/namespaces/pool-one"
    );
}

fn pool_resource() -> cyclops_sdk_core::Pool {
    serde_json::from_str(
        r#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"pool-one","name":"pool-one"},"spec":{"replicas":1,"template":{"containerDiskImage":"example/workspace:latest","cpuCores":4,"memory":"4Gi"}}}"#,
    )
    .unwrap()
}

fn ready_pool_body() -> String {
    r#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"pool-one","name":"pool-one"},"spec":{"replicas":1,"template":{"containerDiskImage":"example/workspace:latest","cpuCores":4,"memory":"4Gi"}},"status":{"phase":"Ready","availableCount":1,"totalCount":1}}"#.into()
}

fn pool_request() -> CreatePoolRequest {
    CreatePoolRequest {
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
    }
}
