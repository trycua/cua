mod support;

use cyclops_sdk::{
    CreateTemplateRequest, CyclopsClient, CyclopsConfiguration, CyclopsCredentials, HttpHeader,
    HttpResponse, ResourceMetadata, Template,
};
use std::sync::Arc;
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example:8443/prefix";
const TOKEN_URL: &str = "https://identity.example/oauth/token";
const NAMESPACE: &str = "example-pool";
const TEMPLATE_COLLECTION: &str = "https://cyclops.example:8443/prefix/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxtemplates";
const TEMPLATE_ITEM: &str = "https://cyclops.example:8443/prefix/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/example-pool/osgymsandboxtemplates/example-pool";

#[tokio::test]
async fn reconcile_template_creates_when_the_template_is_absent() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(404, br#"{"kind":"Status","code":404}"#)),
        Ok(json_response(201, &template(None))),
    ]));

    client(Arc::clone(&http))
        .reconcile_template(create_request(None))
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url, TEMPLATE_ITEM);
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].url, TEMPLATE_COLLECTION);
}

/// The gateway refuses a merge patch that carries `containerDiskImage` without
/// also stating the pull secret, so reconciling an existing registry-backed
/// template used to fail with 403 "k8s request is not allowed"
/// (trycua/cua#3159). A desired spec with no pull secret has to say so.
#[tokio::test]
async fn reconcile_template_patches_a_null_pull_secret_when_the_desired_spec_has_none() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &template(Some("ecr-credentials")))),
        Ok(json_response(200, &template(None))),
    ]));

    client(Arc::clone(&http))
        .reconcile_template(create_request(None))
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    assert_merge_patch_request(&requests[1], TEMPLATE_ITEM);
    let body: serde_json::Value =
        serde_json::from_slice(requests[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body["spec"]["vmTemplate"]["imagePullSecret"],
        serde_json::Value::Null
    );
    assert!(
        body["spec"]["vmTemplate"]
            .as_object()
            .unwrap()
            .contains_key("imagePullSecret")
    );
}

#[tokio::test]
async fn reconcile_template_keeps_a_pull_secret_the_desired_spec_asks_for() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &template(None))),
        Ok(json_response(200, &template(Some("ecr-credentials")))),
    ]));

    client(Arc::clone(&http))
        .reconcile_template(create_request(Some("ecr-credentials")))
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    assert_merge_patch_request(&requests[1], TEMPLATE_ITEM);
    let body: serde_json::Value =
        serde_json::from_slice(requests[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body["spec"]["vmTemplate"]["imagePullSecret"],
        "ecr-credentials"
    );
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

fn spec(pull_secret: Option<&str>) -> cyclops_sdk_schema::OSGymSandboxTemplateSpec {
    serde_json::from_value(serde_json::json!({
        "vmTemplate": {
            "containerDiskImage": "ghcr.io/trycua/minecraft-agent:1.20.1",
            "imagePullSecret": pull_secret,
            "services": [{ "name": "server", "targetPort": 8000, "protocol": "TCP" }],
        },
    }))
    .unwrap()
}

fn create_request(pull_secret: Option<&str>) -> CreateTemplateRequest {
    CreateTemplateRequest {
        namespace: NAMESPACE.into(),
        name: NAMESPACE.into(),
        spec: spec(pull_secret),
    }
}

fn template(pull_secret: Option<&str>) -> Template {
    Template {
        api_version: "osgym.cua.ai/v1alpha1".into(),
        kind: "OSGymSandboxTemplate".into(),
        metadata: ResourceMetadata {
            namespace: NAMESPACE.into(),
            name: NAMESPACE.into(),
            labels: None,
            creation_timestamp: None,
        },
        spec: spec(pull_secret),
    }
}

fn token() -> HttpResponse {
    response(200, br#"{"access_token":"token-a","expires_in":3600}"#)
}

fn json_response(status: u16, value: &impl serde::Serialize) -> HttpResponse {
    response(status, &serde_json::to_vec(value).unwrap())
}

fn response(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: Vec::new(),
        body: body.to_vec(),
    }
}

fn assert_merge_patch_request(request: &cyclops_sdk::HttpRequest, url: &str) {
    assert_eq!(request.method, "PATCH");
    assert_eq!(request.url, url);
    assert_eq!(
        request.headers,
        vec![
            HttpHeader {
                name: "accept".into(),
                value: "application/json".into(),
            },
            HttpHeader {
                name: "content-type".into(),
                value: "application/merge-patch+json".into(),
            },
            HttpHeader {
                name: "authorization".into(),
                value: "Bearer token-a".into(),
            },
        ]
    );
}
