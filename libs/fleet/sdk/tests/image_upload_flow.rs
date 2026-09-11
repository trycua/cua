mod support;

use cyclops_sdk::{
    CyclopsClient, CyclopsConfiguration, CyclopsCredentials, ImageUploadFileRequest,
    ImageUploadRequest,
};
use std::sync::Arc;
use support::ScriptedHttpClient;

const BASE_URL: &str = "https://cyclops.example.test";
const TOKEN_URL: &str = "https://auth.example.test/token";
const UPLOAD_URL: &str = "https://cyclops.example.test/api/image-uploads/presign";
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn presign_image_uploads_posts_typed_request_and_returns_optional_upload() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(
            200,
            br#"{"files":[{"digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","sizeBytes":12,"reference":"uploads/tenant-a/abc","upload":{"method":"PUT","url":"https://uploads.example.test/signed","headers":{"content-length":"12"}}}]}"#,
        )),
    ]));
    let client = client(Arc::clone(&http));

    let response = client
        .presign_image_uploads(ImageUploadRequest {
            namespace: "workers".into(),
            files: vec![ImageUploadFileRequest {
                digest: DIGEST.into(),
                size_bytes: 12,
                name: "worker-rootfs".into(),
            }],
        })
        .await
        .unwrap();

    assert_eq!(response.files.len(), 1);
    assert_eq!(response.files[0].reference, "uploads/tenant-a/abc");
    let upload = response.files[0].upload.as_ref().unwrap();
    assert_eq!(upload.method, "PUT");
    assert_eq!(upload.url, "https://uploads.example.test/signed");
    assert_eq!(upload.headers.get("content-length"), Some(&"12".into()));

    let requests = http.authenticated_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].url.as_str(), UPLOAD_URL);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(requests[0].body.as_deref().unwrap()).unwrap(),
        serde_json::json!({
            "namespace": "workers",
            "files": [{"digest": DIGEST, "sizeBytes": 12, "name": "worker-rootfs"}],
        })
    );
}

#[tokio::test]
async fn presign_image_uploads_accepts_only_http_ok() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(201, br#"{}"#)),
    ]));

    assert!(
        client(http)
            .presign_image_uploads(ImageUploadRequest {
                namespace: "workers".into(),
                files: vec![],
            })
            .await
            .is_err()
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

fn token() -> cyclops_sdk::HttpResponse {
    response(200, br#"{"access_token":"token-a","expires_in":3600}"#)
}

fn response(status: u16, body: &[u8]) -> cyclops_sdk::HttpResponse {
    cyclops_sdk::HttpResponse {
        status,
        headers: vec![],
        body: body.into(),
    }
}
