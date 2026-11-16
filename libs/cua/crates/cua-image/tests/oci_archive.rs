//! `pull_oci_archive`: an image pulled with the caller's registry
//! credential comes out as an OCI image layout archive that keeps every
//! blob byte for byte (so a runtime with its own image store, Apple's
//! `container`, can load it), against a loopback registry that wants a
//! login. Its own test binary, since it sets env vars.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

use base64::Engine;
use cua_image::digest::sha256_bytes;
use cua_image::{ImageCache, RegistryClient, RegistryCredentials, RootfsLayerResolver};
use cua_vmm::{Arch, RootfsResolver};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Docs = Arc<BTreeMap<String, (String, Vec<u8>)>>;

/// Serves `docs` (path -> content type and bytes); `/v2/` and everything
/// else answers 401 (Basic) unless the request carries `auth`.
async fn serve(listener: TcpListener, docs: Docs, auth: String) {
    loop {
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
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            let authed = req
                .lines()
                .any(|l| l.to_ascii_lowercase().starts_with("authorization:") && l.contains(&auth));
            if !authed {
                let _ = s
                    .write_all(
                        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"fixture\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                return;
            }
            if path == "/v2/" {
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await;
                return;
            }
            match docs.get(&path) {
                Some((ct, body)) => {
                    let digest = sha256_bytes(body);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nDocker-Content-Digest: {digest}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(body).await;
                }
                None => {
                    let _ = s
                        .write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                }
            }
        });
    }
}

#[tokio::test]
async fn a_private_image_is_packed_unchanged_with_the_callers_credential() {
    let layer1 = b"layer one bytes (stand-in for a gzip tar)".to_vec();
    let layer2 = vec![7u8; 4096];
    let config = serde_json::to_vec(&serde_json::json!({
        "architecture": "arm64", "os": "linux",
        "config": {"Cmd": ["/bin/true"], "User": "cua"},
        "rootfs": {"type": "layers", "diff_ids": []}
    }))
    .unwrap();
    let layer = |b: &[u8]| {
        serde_json::json!({
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": sha256_bytes(b), "size": b.len()
        })
    };
    let manifest = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": sha256_bytes(&config), "size": config.len()
        },
        "layers": [layer(&layer1), layer(&layer2)]
    }))
    .unwrap();
    let mdigest = sha256_bytes(&manifest);
    let mt = "application/vnd.oci.image.manifest.v1+json".to_string();
    let mut docs: BTreeMap<String, (String, Vec<u8>)> = BTreeMap::new();
    docs.insert(
        "/v2/test/img/manifests/v1".into(),
        (mt.clone(), manifest.clone()),
    );
    docs.insert(
        format!("/v2/test/img/manifests/{mdigest}"),
        (mt, manifest.clone()),
    );
    for blob in [&config, &layer1, &layer2] {
        docs.insert(
            format!("/v2/test/img/blobs/{}", sha256_bytes(blob)),
            ("application/octet-stream".into(), blob.clone()),
        );
    }
    let auth = base64::engine::general_purpose::STANDARD.encode("reader:s3cret");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(serve(listener, Arc::new(docs), auth));
    let empty = tempfile::tempdir().unwrap().keep();
    unsafe {
        std::env::set_var("DOCKER_CONFIG", &empty);
        std::env::set_var("CUA_REGISTRY_CRED_HELPERS", "0");
        std::env::remove_var("CUA_REGISTRY_USERNAME");
        std::env::remove_var("CUA_REGISTRY_PASSWORD");
        std::env::remove_var("GITHUB_TOKEN");
        std::env::set_var("CUA_INSECURE_REGISTRIES", &host);
    }
    let tmp = tempfile::tempdir().unwrap();
    let resolver = RootfsLayerResolver {
        client: RegistryClient::default(),
        cache: ImageCache::new(tmp.path().join("images")),
    };
    let reference = format!("{host}/test/img:v1");
    let out = tmp.path().join("run").join("image.tar");

    // No credential: refused, and nothing is left behind.
    let e = resolver
        .pull_oci_archive(&reference, Arch::Aarch64, None, &out)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.to_lowercase().contains("not authorized")
            || e.to_lowercase().contains("unauthorized")
            || e.contains("sign-in")
            || e.contains("private"),
        "{e}"
    );
    assert!(!out.exists());
    assert!(!out.with_extension("partial").exists());

    // With the credential: the archive holds the blobs unchanged.
    let creds = RegistryCredentials::new("reader", "s3cret").for_registry(&host);
    let digest = resolver
        .pull_oci_archive(&reference, Arch::Aarch64, Some(&creds), &out)
        .await
        .unwrap();
    assert_eq!(digest, mdigest);
    assert!(!out.with_extension("partial").exists());

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut ar = tar::Archive::new(std::fs::File::open(&out).unwrap());
    for entry in ar.entries().unwrap() {
        let mut e = entry.unwrap();
        let name = e.path().unwrap().to_string_lossy().to_string();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        files.insert(name, b);
    }
    assert_eq!(files["oci-layout"], br#"{"imageLayoutVersion":"1.0.0"}"#);
    let blob = |b: &[u8]| format!("blobs/sha256/{}", &sha256_bytes(b)[7..]);
    assert_eq!(files[&blob(&manifest)], manifest, "the manifest as served");
    assert_eq!(files[&blob(&config)], config);
    assert_eq!(files[&blob(&layer1)], layer1);
    assert_eq!(files[&blob(&layer2)], layer2);
    let index: serde_json::Value = serde_json::from_slice(&files["index.json"]).unwrap();
    let m = &index["manifests"][0];
    assert_eq!(m["digest"], mdigest);
    assert_eq!(m["size"], manifest.len());
    assert_eq!(m["mediaType"], "application/vnd.oci.image.manifest.v1+json");
    assert_eq!(
        m["annotations"]["io.containerd.image.name"],
        format!("{host}/test/img:v1")
    );
    assert_eq!(m["annotations"]["org.opencontainers.image.ref.name"], "v1");

    // The credential is nowhere in the archive.
    let all: Vec<u8> = files.values().flatten().copied().collect();
    assert!(!String::from_utf8_lossy(&all).contains("s3cret"));
}
