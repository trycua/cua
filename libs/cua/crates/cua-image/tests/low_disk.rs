//! The low-disk guard on a containerDisk pull, with a fake `statvfs`
//! (`cua_vmm::disk::set_probe`) and a loopback plain-HTTP registry: a pull
//! that would leave less than the minimum free space fails with a typed
//! `InsufficientDisk` before anything is downloaded, and succeeds (marking
//! the cache entry used) once there is room. Hermetic; its own test binary,
//! since it sets env vars and the process-wide probe.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use cua_image::digest::sha256_bytes;
use cua_image::media_types::OCI_MANIFEST;
use cua_image::{ImageCache, ImageError, RegistryClient};
use cua_vmm::disk::{GIB, Space, SpaceProbe};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn serve(listener: TcpListener, docs: Arc<HashMap<String, (String, Vec<u8>)>>) {
    // Bounded: the test aborts this task; one request per connection.
    for _ in 0..200 {
        let Ok((mut s, _)) = listener.accept().await else {
            return;
        };
        let docs = docs.clone();
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
            let (status, headers, body) = if path == "/v2/" {
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

struct Fake(u64);

impl SpaceProbe for Fake {
    fn space(&self, _: &Path) -> std::io::Result<Space> {
        Ok(Space {
            available: self.0,
            total: 500 * GIB,
        })
    }
}

#[tokio::test]
async fn pull_fails_typed_before_writing_when_the_disk_is_nearly_full() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let dir = tempfile::tempdir().unwrap();
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
    let server = tokio::spawn(serve(listener, Arc::new(docs)));

    let empty = tempfile::tempdir().unwrap();
    // SAFETY: this test binary runs this one test; nothing else reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("DOCKER_CONFIG", empty.path());
        std::env::set_var("CUA_REGISTRY_CRED_HELPERS", "0");
        std::env::set_var("CUA_INSECURE_REGISTRIES", &host);
        std::env::set_var("CUA_HOME", dir.path().join("home"));
        for k in [
            "CUA_DISK_MIN_FREE",
            "CUA_DISK_WARN_FREE",
            "CUA_DISK_FAKE_AVAILABLE",
        ] {
            std::env::remove_var(k);
        }
    }
    let client = RegistryClient::new(vec![host.clone()]);
    let cache = ImageCache::new(dir.path().join("cache"));
    let reference = format!("{host}/team/disk:1");

    // 5 GiB is the default floor: 5 GiB available leaves no room.
    cua_vmm::disk::set_probe(Some(Arc::new(Fake(5 * GIB))));
    let err = cua_image::containerdisk::pull(&client, &cache, &reference, "arm64", false)
        .await
        .unwrap_err();
    let ImageError::Vmm(cua_vmm::VmmError::InsufficientDisk(e)) = &err else {
        panic!("expected InsufficientDisk, got {err:?}");
    };
    assert_eq!(e.min_free, 5 * GIB);
    assert!(err.to_string().contains("cua cache prune"), "{err}");
    // Nothing was downloaded or extracted.
    let blobs = dir.path().join("cache/blobs");
    assert!(!blobs.exists() || std::fs::read_dir(&blobs).unwrap().next().is_none());
    assert!(
        std::fs::read_dir(dir.path().join("cache/disks"))
            .map(|mut d| d.all(|e| !e.unwrap().path().join("disk.qcow2").exists()))
            .unwrap_or(true)
    );

    // A configured floor applies too (cache.json under CUA_HOME).
    cua_vmm::disk::CacheConfigFile {
        min_free: Some("1G".into()),
        ..Default::default()
    }
    .write(&dir.path().join("home/cache.json"))
    .unwrap();
    let got = cua_image::containerdisk::pull(&client, &cache, &reference, "arm64", false)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&got).unwrap(), b"qcow2-bytes-for-the-test");
    // The entry is marked used for the LRU.
    assert!(got.parent().unwrap().join(".last-used").exists());
    cua_vmm::disk::set_probe(None);
    server.abort();
}
