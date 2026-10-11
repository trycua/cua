mod support;

use cyclops_sdk::{
    CreateTemplateRequest, CyclopsClient, CyclopsConfiguration, CyclopsCredentials, HttpHeader,
    HttpResponse, ResourceMetadata, SdkError, Template,
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

/// Reconcile means "make the template exactly this". A desired spec without
/// a command, args, env or processMode must clear the ones an earlier spec set,
/// which a merge patch can only say with explicit nulls.
#[tokio::test]
async fn reconcile_template_clears_process_fields_the_desired_spec_omits() {
    let mut stored = template(None);
    stored.spec.vm_template.command = Some(vec!["python".into(), "-m".into(), "old".into()]);
    stored.spec.vm_template.env = Some(std::collections::HashMap::from([(
        "OLD".into(),
        "1".into(),
    )]));
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &stored)),
        Ok(json_response(200, &template(None))),
    ]));

    client(Arc::clone(&http))
        .reconcile_template(create_request(None))
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    let body: serde_json::Value =
        serde_json::from_slice(requests[1].body.as_deref().unwrap()).unwrap();
    let vm_template = body["spec"]["vmTemplate"].as_object().unwrap();
    for key in ["command", "args", "env", "processMode"] {
        assert_eq!(
            vm_template.get(key),
            Some(&serde_json::Value::Null),
            "{key}"
        );
    }
}

#[tokio::test]
async fn reconcile_template_sends_env_command_and_process_mode_it_asks_for() {
    let mut request = create_request(None);
    let vm_template = &mut request.spec.vm_template;
    vm_template.command = Some(vec!["python".into(), "-m".into(), "server".into()]);
    vm_template.args = Some(vec!["--port".into(), "8765".into()]);
    vm_template.env = Some(std::collections::HashMap::from([(
        "FOO".into(),
        "bar".into(),
    )]));
    vm_template.process_mode = Some(cyclops_sdk_schema::ProcessMode::Run);
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &template(None))),
        Ok(json_response(200, &template(None))),
    ]));

    client(Arc::clone(&http))
        .reconcile_template(request)
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    let body: serde_json::Value =
        serde_json::from_slice(requests[1].body.as_deref().unwrap()).unwrap();
    let vm_template = &body["spec"]["vmTemplate"];
    assert_eq!(
        vm_template["command"],
        serde_json::json!(["python", "-m", "server"])
    );
    assert_eq!(vm_template["args"], serde_json::json!(["--port", "8765"]));
    assert_eq!(vm_template["env"], serde_json::json!({"FOO": "bar"}));
    assert_eq!(vm_template["processMode"], serde_json::json!("Run"));
}

/// Terraform drops `command`, the probes or `claim_secrets` by leaving them
/// out of the desired spec; the merge patch has to null them to clear them.
#[tokio::test]
async fn reconcile_template_nulls_process_fields_the_desired_spec_drops() {
    let mut configured = spec(None);
    configured.vm_template.command = Some(vec!["srv".into()]);
    configured.vm_template.claim_secrets = Some(true);
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &template(None))),
        Ok(json_response(200, &template(None))),
        Ok(json_response(200, &template(None))),
        Ok(json_response(200, &template(None))),
    ]));
    let client = client(Arc::clone(&http));

    client
        .clone()
        .reconcile_template(create_request(None))
        .await
        .unwrap();
    client
        .reconcile_template(CreateTemplateRequest {
            spec: configured,
            ..create_request(None)
        })
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    let dropped: serde_json::Value =
        serde_json::from_slice(requests[1].body.as_deref().unwrap()).unwrap();
    let vm_template = dropped["spec"]["vmTemplate"].as_object().unwrap();
    for key in ["command", "probes", "claimSecrets"] {
        assert!(
            vm_template.contains_key(key),
            "{key} must be patched to null"
        );
        assert_eq!(vm_template[key], serde_json::Value::Null);
    }
    let kept: serde_json::Value =
        serde_json::from_slice(requests[3].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        kept["spec"]["vmTemplate"]["command"],
        serde_json::json!(["srv"])
    );
    assert_eq!(kept["spec"]["vmTemplate"]["claimSecrets"], true);
}

#[tokio::test]
async fn reconcile_template_update_403_maps_to_pool_access_denied() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(json_response(200, &template(None))),
        Ok(response(403, b"k8s request is not allowed")),
    ]));

    let error = client(Arc::clone(&http))
        .reconcile_template(create_request(None))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SdkError::PoolAccessDenied {
            ref operation,
            ref namespace,
            status: 403,
            ref body,
        } if operation == "update template"
            && namespace == NAMESPACE
            && body == "k8s request is not allowed"
    ));
    let message = error.to_string();
    assert!(message.contains("globally unique"));
    assert!(message.contains("https://discord.gg/mVnXXpdE85"));
}

#[tokio::test]
async fn create_template_403_maps_to_pool_access_denied() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(403, b"k8s request is not allowed")),
    ]));

    let error = client(Arc::clone(&http))
        .create_template(create_request(None))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SdkError::PoolAccessDenied {
            ref operation,
            ref namespace,
            status: 403,
            ..
        } if operation == "create template" && namespace == NAMESPACE
    ));
}

#[tokio::test]
async fn delete_template_403_maps_to_pool_access_denied() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(403, b"k8s request is not allowed")),
    ]));

    let error = client(Arc::clone(&http))
        .delete_template(template(None))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SdkError::PoolAccessDenied {
            ref operation,
            ref namespace,
            status: 403,
            ..
        } if operation == "delete template" && namespace == NAMESPACE
    ));
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
