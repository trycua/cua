//! QEMU's containerDisk pull with the caller's registry credentials
//! (`StartSpec::registry_auth`), against a loopback plain-HTTP registry that
//! requires Basic auth. Hermetic: no network beyond 127.0.0.1, no docker
//! config credentials. Its own test binary, since it sets env vars.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use cua_image::digest::sha256_bytes;
use cua_image::media_types::OCI_MANIFEST;
use cua_image::{ContainerDiskResolver, ImageCache, RegistryClient, RegistryCredentials};
use cua_vmm::DiskResolver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn serve(listener: TcpListener, docs: Arc<HashMap<String, (String, Vec<u8>)>>, auth: String) {
    // Bounded: the test aborts this task; one request per connection.
    for _ in 0..200 {
        let Ok((mut s, _)) = listener.accept().await else {
            return;
        };
        let (docs, auth) = (docs.clone(), auth.clone());
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
            let head_only = req.starts_with("HEAD ");
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            let authorized = req
                .lines()
                .any(|l| l.to_ascii_lowercase().starts_with("authorization:") && l.contains(&auth));
            let (status, headers, body) = if !authorized {
                (
                    "401 Unauthorized",
                    "WWW-Authenticate: Basic realm=\"fake\"\r\n".to_string(),
                    br#"{"errors":[{"code":"UNAUTHORIZED","message":"auth required"}]}"#.to_vec(),
                )
            } else if path == "/v2/" {
                ("200 OK", String::new(), b"{}".to_vec())
            } else if let Some((mt, body)) = docs.get(&path) {
                (
                    "200 OK",
                    format!(
                        "Content-Type: {mt}\r\nDocker-Content-Digest: {}\r\n",
                        sha256_bytes(body)
                    ),
                    body.clone(),
                )
            } else {
                (
                    "404 Not Found",
                    "Content-Type: application/json\r\n".to_string(),
                    br#"{"errors":[{"code":"MANIFEST_UNKNOWN","message":"unknown"}]}"#.to_vec(),
                )
            };
            let head = format!(
                "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes()).await;
            if !head_only {
                let _ = s.write_all(&body).await;
            }
        });
    }
}

#[tokio::test]
async fn private_containerdisk_pulls_with_the_start_spec_credentials() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let dir = tempfile::tempdir().unwrap();

    // A tiny containerDisk.
    let disk = dir.path().join("in.qcow2");
    std::fs::write(&disk, b"qcow2-bytes-for-the-test").unwrap();
    let img = cua_image::containerdisk::pack(&disk, "arm64", &dir.path().join("packed")).unwrap();
    let mut docs = HashMap::new();
    let cfg = img.config_descriptor();
    docs.insert(
        format!("/v2/team/disk/blobs/{}", cfg.digest),
        ("application/octet-stream".to_string(), img.config.clone()),
    );
    for (d, path) in &img.layers {
        docs.insert(
            format!("/v2/team/disk/blobs/{}", d.digest),
            (
                "application/octet-stream".to_string(),
                std::fs::read(path).unwrap(),
            ),
        );
    }
    let m = serde_json::to_vec(&img.manifest()).unwrap();
    let md = sha256_bytes(&m);
    docs.insert(
        "/v2/team/disk/manifests/1".to_string(),
        (OCI_MANIFEST.to_string(), m.clone()),
    );
    docs.insert(
        format!("/v2/team/disk/manifests/{md}"),
        (OCI_MANIFEST.to_string(), m),
    );
    let server = tokio::spawn(serve(
        listener,
        Arc::new(docs),
        base64::engine::general_purpose::STANDARD.encode("bot:s3cret"),
    ));

    // No ambient credentials anywhere.
    let empty = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("DOCKER_CONFIG", empty.path());
        std::env::set_var("CUA_REGISTRY_CRED_HELPERS", "0");
        std::env::remove_var("CUA_REGISTRY_USERNAME");
        std::env::set_var("CUA_INSECURE_REGISTRIES", &host);
    }
    let resolver = ContainerDiskResolver {
        client: RegistryClient::new(vec![host.clone()]),
        cache: ImageCache::new(dir.path().join("cache")),
    };
    let reference = format!("{host}/team/disk:1");

    // Without credentials the private image is refused.
    assert!(
        resolver
            .resolve_with_credentials(&reference, cua_vmm::Arch::Aarch64, None)
            .await
            .is_err()
    );
    // Wrong credentials too.
    let wrong = RegistryCredentials::new("bot", "nope");
    assert!(
        resolver
            .resolve_with_credentials(&reference, cua_vmm::Arch::Aarch64, Some(&wrong))
            .await
            .is_err()
    );
    // The start spec's credentials pull it.
    let creds = RegistryCredentials::new("bot", "s3cret").for_registry(host.clone());
    let got = resolver
        .resolve_with_credentials(&reference, cua_vmm::Arch::Aarch64, Some(&creds))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&got).unwrap(), b"qcow2-bytes-for-the-test");
    server.abort();
}
