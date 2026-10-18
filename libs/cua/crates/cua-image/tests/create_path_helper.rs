//! End to end through the resolver a Space create uses
//! ([`cua_image::resolve::resolve`]: index/manifest, then the config blob)
//! against a loopback registry, with a throwaway docker config whose
//! `credsStore` is a fake `docker-credential-r7fake` first on `PATH`.
//!
//! A public image must never start the helper (Docker Desktop's makes macOS
//! ask "Cua Spaces would like to access data from other apps"); a private
//! one (anonymous 401) reaches it exactly once. No network, no docker. Its
//! own test binary: it sets process-wide env before the resolver's client
//! exists.

use std::sync::{Arc, Mutex};

use base64::Engine;
use cua_image::digest::sha256_bytes;
use cua_image::resolve::{Backend, resolve};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const BASIC: &str = "bot:s3cret";

async fn registry() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let config = serde_json::to_vec(&json!({
        "architecture": "arm64", "os": "linux", "config": {}, "rootfs": {"type": "layers", "diff_ids": []}
    }))
    .unwrap();
    let config_digest = sha256_bytes(&config);
    let manifest = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest, "size": config.len()},
        "layers": []
    }))
    .unwrap();
    let want = base64::engine::general_purpose::STANDARD.encode(BASIC);
    let hits = Arc::new(Mutex::new(0usize));
    let task = tokio::spawn(async move {
        for _ in 0..200 {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let (manifest, config, config_digest, want) = (
                manifest.clone(),
                config.clone(),
                config_digest.clone(),
                want.clone(),
            );
            let hits = hits.clone();
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
                *hits.lock().unwrap() += 1;
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                let authed = req.lines().any(|l| {
                    l.to_ascii_lowercase().starts_with("authorization:") && l.contains(&want)
                });
                let public = path.contains("/v2/pub/app/");
                let (status, headers, body): (&str, String, Vec<u8>) = if !authed && !public {
                    (
                        "401 Unauthorized",
                        "WWW-Authenticate: Basic realm=\"fake\"\r\n".into(),
                        br#"{"errors":[{"code":"UNAUTHORIZED","message":"auth"}]}"#.to_vec(),
                    )
                } else if path == "/v2/" {
                    ("200 OK", String::new(), b"{}".to_vec())
                } else if path.contains("/manifests/") {
                    (
                        "200 OK",
                        format!(
                            "Content-Type: application/vnd.oci.image.manifest.v1+json\r\n\
                             Docker-Content-Digest: {}\r\n",
                            sha256_bytes(&manifest)
                        ),
                        manifest.clone(),
                    )
                } else if path.ends_with(&config_digest) {
                    ("200 OK", String::new(), config.clone())
                } else {
                    ("404 Not Found", String::new(), b"{}".to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
            });
        }
    });
    (host, task)
}

#[tokio::test]
async fn a_public_create_never_runs_the_helper_a_private_one_runs_it_once() {
    if cfg!(not(unix)) {
        return;
    }
    let (host, server) = registry().await;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let helper = bin.join("docker-credential-r7fake");
    let log = dir.path().join("helper.log");
    std::fs::write(
        &helper,
        "#!/bin/sh\necho \"$1 $(cat)\" >> \"$R7_HELPER_LOG\"\n\
         printf '{\"ServerURL\":\"x\",\"Username\":\"bot\",\"Secret\":\"s3cret\"}'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(dir.path().join("config.json"), r#"{"credsStore":"r7fake"}"#).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&path));
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        std::env::set_var("DOCKER_CONFIG", dir.path());
        std::env::set_var("R7_HELPER_LOG", &log);
        std::env::set_var("CUA_INSECURE_REGISTRIES", &host);
        for k in [
            "CUA_REGISTRY_USERNAME",
            "CUA_REGISTRY_PASSWORD",
            "CUA_REGISTRY_CRED_HELPERS",
            "GITHUB_TOKEN",
            "CUA_RESOLVE_IMAGES",
        ] {
            std::env::remove_var(k);
        }
    }
    let calls = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .count()
    };

    // Public image: the full create-path resolution, helper never started.
    let got = resolve(&format!("{host}/pub/app:1"), Backend::Container, "arm64")
        .await
        .unwrap();
    assert!(got.pinned_ref.contains("@sha256:"), "{got:?}");
    assert_eq!(calls(), 0, "a public image ran the credential helper");

    // Private image: refused anonymously, then the helper, once.
    let got = resolve(&format!("{host}/team/app:1"), Backend::Container, "arm64")
        .await
        .unwrap();
    assert!(got.pinned_ref.contains("@sha256:"), "{got:?}");
    assert_eq!(calls(), 1, "a private image asks the helper exactly once");
    server.abort();
}
