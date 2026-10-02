mod support;

use base64::{Engine, engine::general_purpose::STANDARD};
use cyclops_sdk::{
    CreateRegistrySecretRequest, CyclopsClient, CyclopsConfiguration, CyclopsCredentials,
    HttpResponse, RegistrySecret, SdkError, registry_secret_name_prefix,
};
use std::sync::Arc;
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example:8443/prefix";
const TOKEN_URL: &str = "https://identity.example/oauth/token";
const SECRETS: &str =
    "https://cyclops.example:8443/prefix/api/k8s/api/v1/namespaces/example-pool/secrets";

fn request() -> CreateRegistrySecretRequest {
    CreateRegistrySecretRequest {
        namespace: "example-pool".into(),
        name: "cua-registry-ghcr".into(),
        registry: "ghcr.io".into(),
        username: "octo".into(),
        password: "ghp_token".into(),
    }
}

#[tokio::test]
async fn create_registry_secret_posts_a_dockerconfigjson_secret() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(201, br#"{"kind":"Secret"}"#)),
    ]));

    let secret = client(Arc::clone(&http))
        .create_registry_secret(request())
        .await
        .unwrap();

    assert_eq!(
        secret,
        RegistrySecret {
            namespace: "example-pool".into(),
            name: "cua-registry-ghcr".into(),
            registry: "ghcr.io".into(),
        }
    );
    let requests = http.authenticated_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].url, SECRETS);
    let body: serde_json::Value =
        serde_json::from_slice(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["type"], "kubernetes.io/dockerconfigjson");
    assert_eq!(body["metadata"]["name"], "cua-registry-ghcr");
    assert_eq!(body["metadata"]["namespace"], "example-pool");
    assert!(body["metadata"].get("annotations").is_none());
    assert!(body.get("stringData").is_none());
    let config: serde_json::Value = serde_json::from_slice(
        &STANDARD
            .decode(body["data"][".dockerconfigjson"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let auth = &config["auths"]["ghcr.io"];
    assert_eq!(auth["username"], "octo");
    assert_eq!(auth["password"], "ghp_token");
    assert_eq!(auth["auth"], STANDARD.encode("octo:ghp_token"));
}

#[tokio::test]
async fn create_registry_secret_replaces_an_existing_secret() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(409, br#"{"reason":"AlreadyExists"}"#)),
        Ok(response(200, b"{}")),
        Ok(response(201, b"{}")),
    ]));

    client(Arc::clone(&http))
        .create_registry_secret(request())
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    let calls: Vec<_> = requests
        .iter()
        .map(|request| (request.method.as_str(), request.url.as_str()))
        .collect();
    let item = format!("{SECRETS}/cua-registry-ghcr");
    assert_eq!(
        calls,
        vec![
            ("POST", SECRETS),
            ("DELETE", item.as_str()),
            ("POST", SECRETS)
        ]
    );
}

#[tokio::test]
async fn registry_secret_names_and_hosts_are_validated_before_any_request() {
    for (mutate, needle) in [
        (
            (|request: &mut CreateRegistrySecretRequest| request.name = "ecr-credentials".into())
                as fn(&mut CreateRegistrySecretRequest),
            "must start with cua-registry-",
        ),
        (
            |request: &mut CreateRegistrySecretRequest| request.name = "cua-registry-".into(),
            "registry secret name",
        ),
        (
            |request: &mut CreateRegistrySecretRequest| request.registry = "https://ghcr.io".into(),
            "bare host",
        ),
        (
            |request: &mut CreateRegistrySecretRequest| request.password = String::new(),
            "must not be empty",
        ),
    ] {
        let http = Arc::new(ScriptedHttpClient::new([Ok(token())]));
        let mut request = request();
        mutate(&mut request);
        let error = client(Arc::clone(&http))
            .create_registry_secret(request)
            .await
            .unwrap_err();
        let matched = match &error {
            SdkError::Configuration { reason } => reason.contains(needle),
            SdkError::InvalidResourceName { field, .. } => field.contains(needle),
            _ => false,
        };
        assert!(matched, "{error:?}");
        assert!(http.authenticated_requests().await.is_empty());
    }
}

#[test]
fn registry_secret_request_debug_redacts_the_password() {
    let debug = format!("{:?}", request());
    assert!(!debug.contains("ghp_token"));
    assert!(debug.contains("<redacted>"));
    assert_eq!(registry_secret_name_prefix(), "cua-registry-");
}

#[tokio::test]
async fn delete_registry_secret_targets_the_item_and_tolerates_404() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(404, b"{}")),
    ]));

    client(Arc::clone(&http))
        .delete_registry_secret("example-pool".into(), "cua-registry-ghcr".into())
        .await
        .unwrap();

    let requests = http.authenticated_requests().await;
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].url, format!("{SECRETS}/cua-registry-ghcr"));
}

#[tokio::test]
async fn resolve_image_queries_the_gateway_and_decodes_the_pin() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(
            200,
            serde_json::json!({
                "ref": "ghcr.io/trycua/linux:24.04",
                "resolvedRef": "ghcr.io/trycua/linux:24.04-disk",
                "pinnedRef": format!("ghcr.io/trycua/linux@{digest}"),
                "digest": digest,
                "variant": "containerdisk",
                "platformDigest": null,
                "mediaType": "application/vnd.oci.image.index.v1+json",
            })
            .to_string()
            .as_bytes(),
        )),
    ]));

    let resolved = client(Arc::clone(&http))
        .resolve_image("ghcr.io/trycua/linux:24.04".into(), Some("kubevirt".into()))
        .await
        .unwrap();

    assert_eq!(resolved.variant, "containerdisk");
    assert_eq!(resolved.resolved_ref, "ghcr.io/trycua/linux:24.04-disk");
    let requests = http.authenticated_requests().await;
    assert_eq!(
        requests[0].url,
        "https://cyclops.example:8443/prefix/api/images/resolve?ref=ghcr.io%2Ftrycua%2Flinux%3A24.04&runtime=kubevirt"
    );

    let error = client(Arc::new(ScriptedHttpClient::new([Ok(token())])))
        .resolve_image("ghcr.io/x".into(), Some("docker".into()))
        .await
        .unwrap_err();
    assert!(matches!(error, SdkError::Configuration { .. }));
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
        headers: Vec::new(),
        body: body.to_vec(),
    }
}
