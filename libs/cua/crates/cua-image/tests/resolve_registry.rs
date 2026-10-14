//! `resolve` against a real (loopback, plain HTTP) registry that requires
//! Basic auth, with credentials from a throwaway docker config. Hermetic:
//! no network beyond 127.0.0.1. Its own test binary, since it sets env vars.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use cua_image::digest::sha256_bytes;
use cua_image::resolve::{Backend, Variant, resolve_with};
use cua_image::{ImageError, RegistryClient};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Doc {
    media_type: String,
    body: Vec<u8>,
}

async fn serve(listener: TcpListener, docs: Arc<HashMap<String, Doc>>, auth: String) {
    // Bounded: the test aborts this task; each connection handles one request.
    for _ in 0..200 {
        let Ok((mut s, _)) = listener.accept().await else {
            return;
        };
        let docs = docs.clone();
        let auth = auth.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 16 << 10];
            let mut n = 0;
            while n < buf.len() {
                let Ok(k) = s.read(&mut buf[n..]).await else {
                    return;
                };
                if k == 0 {
                    return;
                }
                n += k;
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            let authorized = req
                .lines()
                .any(|l| l.to_ascii_lowercase().starts_with("authorization:") && l.contains(&auth));
            let (status, headers, body): (&str, String, Vec<u8>) = if !authorized {
                (
                    "401 Unauthorized",
                    "WWW-Authenticate: Basic realm=\"fake\"\r\n".into(),
                    br#"{"errors":[{"code":"UNAUTHORIZED","message":"auth required"}]}"#.to_vec(),
                )
            } else if path == "/v2/" {
                ("200 OK", String::new(), b"{}".to_vec())
            } else if let Some(d) = docs.get(&path) {
                (
                    "200 OK",
                    format!(
                        "Content-Type: {}\r\nDocker-Content-Digest: {}\r\n",
                        d.media_type,
                        sha256_bytes(&d.body)
                    ),
                    d.body.clone(),
                )
            } else {
                (
                    "404 Not Found",
                    "Content-Type: application/json\r\n".into(),
                    br#"{"errors":[{"code":"MANIFEST_UNKNOWN","message":"unknown"}]}"#.to_vec(),
                )
            };
            let head = format!(
                "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes()).await;
            let _ = s.write_all(&body).await;
        });
    }
}

#[tokio::test]
async fn private_registry_uses_the_auth_chain() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());

    // A rootfs image with a -disk sibling.
    let mut docs = HashMap::new();
    let mut add_image = |repo: &str, tag: &str, disk: bool| {
        let cfg = if disk {
            json!({"os": "linux", "architecture": "amd64", "config": {},
                "history": [{"created_by": "COPY disk.img /disk/disk.img"}]})
        } else {
            json!({"os": "linux", "architecture": "amd64", "config": {"Cmd": ["sh"]}})
        };
        let cfg = serde_json::to_vec(&cfg).unwrap();
        let cd = sha256_bytes(&cfg);
        let m = serde_json::to_vec(&json!({"schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": cd, "size": cfg.len()},
            "layers": []}))
        .unwrap();
        let md = sha256_bytes(&m);
        let mt = "application/vnd.oci.image.manifest.v1+json".to_string();
        docs.insert(
            format!("/v2/{repo}/blobs/{cd}"),
            Doc {
                media_type: "application/octet-stream".into(),
                body: cfg,
            },
        );
        docs.insert(
            format!("/v2/{repo}/manifests/{tag}"),
            Doc {
                media_type: mt.clone(),
                body: m.clone(),
            },
        );
        docs.insert(
            format!("/v2/{repo}/manifests/{md}"),
            Doc {
                media_type: mt,
                body: m,
            },
        );
        md
    };
    let root = add_image("team/app", "1", false);
    let disk = add_image("team/app", "1-disk", true);
    let server = tokio::spawn(serve(
        listener,
        Arc::new(docs),
        base64::engine::general_purpose::STANDARD.encode("bot:s3cret"),
    ));

    let dir = tempfile::tempdir().unwrap();
    let cfg = json!({"auths": {host.clone(): {"auth":
        base64::engine::general_purpose::STANDARD.encode("bot:s3cret")}}});
    std::fs::write(dir.path().join("config.json"), cfg.to_string()).unwrap();
    unsafe {
        std::env::set_var("DOCKER_CONFIG", dir.path());
        std::env::set_var("CUA_REGISTRY_CRED_HELPERS", "0");
        std::env::remove_var("CUA_REGISTRY_USERNAME");
    }

    let client = RegistryClient::new(vec![host.clone()]);
    let r = format!("{host}/team/app:1");
    let got = resolve_with(&client, &r, Backend::Container, "amd64")
        .await
        .unwrap();
    assert_eq!(got.variant, Variant::Rootfs);
    assert_eq!(got.pinned_ref, format!("{host}/team/app@{root}"));
    let got = resolve_with(&client, &r, Backend::Vm, "arm64")
        .await
        .unwrap();
    assert_eq!(got.variant, Variant::Containerdisk);
    assert_eq!(got.pinned_ref, format!("{host}/team/app@{disk}"));
    assert!(got.emulated);
    let e = resolve_with(
        &client,
        &format!("{host}/team/none:1"),
        Backend::Local,
        "amd64",
    )
    .await
    .unwrap_err();
    assert!(matches!(e, ImageError::NotFound(_)), "{e:?}");

    // Wrong credentials: a typed Unauthorized, not a hang.
    let cfg = json!({"auths": {host.clone(): {"auth":
        base64::engine::general_purpose::STANDARD.encode("bot:wrong")}}});
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(dir2.path().join("config.json"), cfg.to_string()).unwrap();
    unsafe { std::env::set_var("DOCKER_CONFIG", dir2.path()) };
    let fresh = RegistryClient::new(vec![host.clone()]);
    let e = resolve_with(&fresh, &r, Backend::Local, "amd64")
        .await
        .unwrap_err();
    assert!(matches!(e, ImageError::Unauthorized(_)), "{e:?}");
    server.abort();
}
