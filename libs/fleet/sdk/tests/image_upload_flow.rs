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
    assert_eq!(requests[0].timeout_secs, Some(30));
    assert_eq!(requests[0].max_response_bytes, Some(1024 * 1024));
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

const FILE_DIGEST: &str = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const REFERENCE: &str =
    "uploads/tenant-ed574aa71eb87d6cefff58373c93f1c2/kIt24leq5WBfxgNlqPjhOEGiFjnQtQGa0FkULIWw0Mo";
const SIGNED_URL: &str = "https://uploads.example.test/object?signature=secret";

fn instruction() -> serde_json::Value {
    serde_json::json!({"digest": FILE_DIGEST, "sizeBytes": 3, "reference": REFERENCE,
        "upload": {"method": "PUT", "url": SIGNED_URL, "headers": {
            "Content-Length": "3", "X-Amz-Checksum-Sha256": "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=",
            "x-custom-signed": "preserve exactly"}}})
}

fn presigned(files: serde_json::Value) -> cyclops_sdk::HttpResponse {
    response(
        200,
        &serde_json::to_vec(&serde_json::json!({"files": files})).unwrap(),
    )
}

#[tokio::test]
async fn upload_hashes_bytes_and_isolates_auth_even_on_same_origin() {
    for url in [
        SIGNED_URL,
        "https://cyclops.example.test/signed?signature=secret",
    ] {
        let mut file = instruction();
        file["upload"]["url"] = url.into();
        let http = Arc::new(ScriptedHttpClient::new([
            Ok(token()),
            Ok(presigned(serde_json::json!([file]))),
            Ok(response(200, b"")),
        ]));
        let result = client(http.clone())
            .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
            .await
            .unwrap();
        assert_eq!(result.reference, REFERENCE);
        assert_eq!(result.digest, FILE_DIGEST);
        assert_eq!(result.size_bytes, 3);
        assert!(result.upload.is_none());
        let requests = http.requests().await;
        assert_eq!(requests.len(), 3);
        assert!(
            requests[1]
                .headers
                .iter()
                .any(|header| header.name.eq_ignore_ascii_case("authorization"))
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(requests[1].body.as_ref().unwrap())
                .unwrap(),
            serde_json::json!({"namespace":"workers","files":[{"digest":FILE_DIGEST,"sizeBytes":3,"name":"rootfs"}]})
        );
        assert_eq!(requests[1].timeout_secs, Some(30));
        assert_eq!(requests[1].max_response_bytes, Some(1024 * 1024));
        assert_eq!(requests[2].timeout_secs, Some(300));
        assert_eq!(requests[2].max_response_bytes, Some(4096));
        assert_eq!(requests[2].method, "PUT");
        assert_eq!(requests[2].url, url);
        assert_eq!(requests[2].body.as_deref(), Some(b"abc".as_slice()));
        assert_eq!(requests[2].headers.len(), 3);
        for header in &requests[2].headers {
            assert_eq!(file["upload"]["headers"][&header.name], header.value);
        }
    }
}

#[tokio::test]
async fn matching_existing_object_skips_put() {
    let mut file = instruction();
    file["upload"] = serde_json::Value::Null;
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(presigned(serde_json::json!([file]))),
    ]));
    let result = client(http.clone())
        .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
        .await
        .unwrap();
    assert!(result.upload.is_none());
    assert_eq!(result.reference, REFERENCE);
    assert_eq!(http.request_count().await, 2);
}

#[tokio::test]
async fn malformed_instructions_never_upload() {
    let mut cases = vec![
        serde_json::json!([]),
        serde_json::json!([instruction(), instruction()]),
    ];
    for (pointer, value) in [
        ("/digest", serde_json::json!(DIGEST)),
        ("/sizeBytes", serde_json::json!(4)),
        (
            "/reference",
            serde_json::json!("uploads/tenant-other/token"),
        ),
        ("/upload/method", serde_json::json!("POST")),
        (
            "/upload/url",
            serde_json::json!("http://uploads.example.test/secret"),
        ),
        (
            "/upload/url",
            serde_json::json!("https://user:secret@uploads.example.test/"),
        ),
        (
            "/upload/url",
            serde_json::json!("https://uploads.example.test/#secret"),
        ),
        ("/upload/url", serde_json::json!("not a URL secret")),
        ("/upload/headers/Content-Length", serde_json::json!("4")),
        (
            "/upload/headers/X-Amz-Checksum-Sha256",
            serde_json::json!("secret"),
        ),
    ] {
        let mut file = instruction();
        *file.pointer_mut(pointer).unwrap() = value;
        cases.push(serde_json::json!([file]));
    }
    for files in cases {
        let http = Arc::new(ScriptedHttpClient::new([Ok(token()), Ok(presigned(files))]));
        let error = client(http.clone())
            .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("secret"));
        assert_eq!(http.request_count().await, 2);
    }
}

#[tokio::test]
async fn put_errors_are_redacted_without_retry_refresh_or_image_creation() {
    let mut failures = vec![Err(cyclops_sdk::HttpError::Transport {
        reason: format!("timeout/network {SIGNED_URL} secret"),
    })];
    for status in [301, 307, 308, 400, 401, 403, 500] {
        failures.push(Ok(response(
            status,
            format!("{SIGNED_URL} secret").as_bytes(),
        )));
    }
    for failure in failures {
        let http = Arc::new(ScriptedHttpClient::new([
            Ok(token()),
            Ok(presigned(serde_json::json!([instruction()]))),
            failure,
        ]));
        let error = client(http.clone())
            .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("secret"));
        assert_eq!(http.request_count().await, 3);
    }
}

#[tokio::test]
async fn invalid_signed_headers_and_existing_object_bindings_are_rejected() {
    let mut cases = Vec::new();
    for (name, value) in [
        ("content-length", "3"),
        ("Authorization", "secret"),
        ("Cookie", "secret"),
        ("Transfer-Encoding", "chunked"),
        ("invalid header", "secret"),
        ("x-custom-signed", "secret\r\ninjected: yes"),
    ] {
        let mut file = instruction();
        file["upload"]["headers"][name] = value.into();
        cases.push(file);
    }
    for (field, value) in [
        ("digest", serde_json::json!(DIGEST)),
        ("sizeBytes", serde_json::json!(4)),
        ("reference", serde_json::json!("uploads/workers/secret")),
    ] {
        let mut file = instruction();
        file["upload"] = serde_json::Value::Null;
        file[field] = value;
        cases.push(file);
    }
    for url in [
        "https://uploads.example.test/?X-Amz-Checksum-Sha256=secret",
        "https://uploads.example.test/?content-length=4",
        "https://@uploads.example.test/secret",
    ] {
        let mut file = instruction();
        file["upload"]["url"] = url.into();
        cases.push(file);
    }
    for file in cases {
        let http = Arc::new(ScriptedHttpClient::new([
            Ok(token()),
            Ok(presigned(serde_json::json!([file]))),
        ]));
        let error = client(http.clone())
            .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("secret"));
        assert_eq!(http.request_count().await, 2);
    }
}

#[tokio::test]
async fn same_origin_put_401_does_not_refresh_token() {
    let mut file = instruction();
    file["upload"]["url"] = "https://cyclops.example.test/signed?signature=secret".into();
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(presigned(serde_json::json!([file]))),
        Ok(response(401, b"secret")),
    ]));
    let error = client(http.clone())
        .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        cyclops_sdk::SdkError::Status { status: 401, .. }
    ));
    let requests = http.requests().await;
    assert_eq!(requests.len(), 3);
    assert!(
        !requests[2]
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("authorization"))
    );
}

#[tokio::test]
async fn presign_errors_cannot_leak_signed_material() {
    for result in [
        Ok(response(403, SIGNED_URL.as_bytes())),
        Ok(response(200, br#"{"files":"secret"}"#)),
        Err(cyclops_sdk::HttpError::Transport {
            reason: SIGNED_URL.into(),
        }),
    ] {
        let http = Arc::new(ScriptedHttpClient::new([Ok(token()), result]));
        let error = client(http.clone())
            .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("secret"));
        assert_eq!(http.request_count().await, 2);
    }
}

#[tokio::test]
async fn upload_allows_absent_optional_signed_checksum_and_length() {
    let mut file = instruction();
    file["upload"]["headers"] = serde_json::json!({});
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(presigned(serde_json::json!([file]))),
        Ok(response(204, b"")),
    ]));
    assert!(
        client(http)
            .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
            .await
            .unwrap()
            .upload
            .is_none()
    );
}

#[tokio::test]
async fn presign_status_and_error_classification_survive_redaction() {
    for status in [403, 503] {
        let http = Arc::new(ScriptedHttpClient::new([
            Ok(token()),
            Ok(response(status, SIGNED_URL.as_bytes())),
        ]));
        let error = client(http.clone())
            .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
            .await
            .unwrap_err();
        match error {
            cyclops_sdk::SdkError::Status {
                status: actual,
                operation,
                body,
            } => {
                assert_eq!(actual, status);
                assert_eq!(operation, "presign image uploads");
                assert!(body.is_empty());
            }
            other => panic!("expected redacted status, got {other:?}"),
        }
        assert_eq!(http.request_count().await, 2);
    }
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(200, b"secret")),
    ]));
    let error = client(http)
        .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
        .await
        .unwrap_err();
    assert!(matches!(error, cyclops_sdk::SdkError::Body { .. }));
    let http = Arc::new(ScriptedHttpClient::new([Ok(response(200, b"secret"))]));
    let error = client(http.clone())
        .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
        .await
        .unwrap_err();
    assert!(matches!(error, cyclops_sdk::SdkError::Token { .. }));
    assert_eq!(http.request_count().await, 1);
}

#[tokio::test]
async fn presign_401_remains_authenticated_but_never_uploads_on_failure() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(response(401, b"secret")),
        Ok(token()),
        Ok(response(401, b"secret")),
    ]));
    let error = client(http.clone())
        .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        cyclops_sdk::SdkError::Status { status: 401, .. }
    ));
    assert!(!format!("{error:?}").contains("secret"));
    assert_eq!(http.request_count().await, 4);
}

#[tokio::test]
async fn put_202_does_not_claim_upload_completion() {
    let http = Arc::new(ScriptedHttpClient::new([
        Ok(token()),
        Ok(presigned(serde_json::json!([instruction()]))),
        Ok(response(202, b"secret")),
    ]));
    let error = client(http.clone())
        .upload_image_file("workers".into(), "rootfs".into(), b"abc".to_vec())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        cyclops_sdk::SdkError::Status { status: 202, .. }
    ));
    assert!(!format!("{error:?}").contains("secret"));
    assert_eq!(http.request_count().await, 3);
}

#[tokio::test]
async fn native_presign_caps_success_and_error_bodies_before_parsing() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    for status in [200, 503] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            let content_length = String::from_utf8(headers)
                .unwrap()
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            stream.read_exact(&mut vec![0; content_length]).unwrap();
            let body = vec![b'x'; 1024 * 1024 + 1];
            write!(
                stream,
                "HTTP/1.1 {status} Response\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            let _ = stream.write_all(&body);
        });
        let client = CyclopsClient::connect_with_access_token_and_native_http_client(
            cyclops_sdk::CyclopsTokenProviderConfiguration {
                base_url,
                pool_poll_interval_ms: 1,
                pool_poll_limit: 1,
                claim_poll_interval_ms: 1,
                claim_poll_limit: 1,
            },
            "token".into(),
        )
        .unwrap();
        let error = client
            .presign_image_uploads(ImageUploadRequest {
                namespace: "workers".into(),
                files: vec![ImageUploadFileRequest {
                    digest: FILE_DIGEST.into(),
                    size_bytes: 3,
                    name: "rootfs".into(),
                }],
            })
            .await
            .unwrap_err();
        assert!(
            matches!(error, cyclops_sdk::SdkError::Transport { ref reason } if reason == "HTTP response exceeds configured size limit"),
            "{error:?}"
        );
        server.join().unwrap();
    }
}
