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

fn walk(p: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(p)
        .unwrap()
        .flat_map(|e| {
            let p = e.unwrap().path();
            if p.is_dir() { walk(&p) } else { vec![p] }
        })
        .collect()
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

fn publish(
    docs: &mut HashMap<String, (String, Vec<u8>)>,
    repo: &str,
    img: &cua_image::PackedImage,
) {
    let cfg = img.config_descriptor();
    docs.insert(
        format!("/v2/{repo}/blobs/{}", cfg.digest),
        ("application/octet-stream".to_string(), img.config.clone()),
    );
    for (d, path) in &img.layers {
        docs.insert(
            format!("/v2/{repo}/blobs/{}", d.digest),
            (
                "application/octet-stream".to_string(),
                std::fs::read(path).unwrap(),
            ),
        );
    }
    let m = serde_json::to_vec(&img.manifest()).unwrap();
    let md = sha256_bytes(&m);
    docs.insert(
        format!("/v2/{repo}/manifests/1"),
        (OCI_MANIFEST.to_string(), m.clone()),
    );
    docs.insert(
        format!("/v2/{repo}/manifests/{md}"),
        (OCI_MANIFEST.to_string(), m),
    );
}

fn truncated_disk_image(dir: &Path) -> cua_image::PackedImage {
    std::fs::create_dir_all(dir).unwrap();
    let disk = dir.join("in.qcow2");
    let mut x: u32 = 1;
    let noise: Vec<u8> = (0..1 << 20)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect();
    std::fs::write(&disk, noise).unwrap();
    let img = cua_image::containerdisk::pack(&disk, "arm64", &dir.join("packed")).unwrap();
    let (desc, layer) = &img.layers[0];
    let mut bytes = std::fs::read(layer).unwrap();
    bytes.truncate(bytes.len() / 2);
    let path = dir.join("layer.tar.gz");
    std::fs::write(&path, &bytes).unwrap();
    let desc = cua_image::manifest::Descriptor {
        digest: sha256_bytes(&bytes),
        size: bytes.len() as u64,
        ..desc.clone()
    };
    let config = serde_json::from_slice(&img.config).unwrap();
    cua_image::PackedImage::new(&img.arch, config, vec![(desc, path)]).unwrap()
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
    publish(&mut docs, "team/disk", &img);
    let broken = truncated_disk_image(&dir.path().join("broken"));
    publish(&mut docs, "team/broken", &broken);
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
    let err = cua_image::containerdisk::pull(&client, &cache, &reference, "arm64")
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
    let got = cua_image::containerdisk::pull(&client, &cache, &reference, "arm64")
        .await
        .unwrap();
    assert_eq!(std::fs::read(&got).unwrap(), b"qcow2-bytes-for-the-test");
    // The entry is marked used for the LRU.
    assert!(got.parent().unwrap().join(".last-used").exists());

    let err =
        cua_image::containerdisk::pull(&client, &cache, &format!("{host}/team/broken:1"), "arm64")
            .await
            .unwrap_err();
    assert!(!matches!(err, ImageError::Vmm(_)), "{err:?}");
    // A blob's `.partial.lock` stays: removing a lock file another pull may
    // be waiting on would let a third take the same blob at once.
    let blobs_left: Vec<_> = walk(&blobs)
        .into_iter()
        .filter(|p| !p.to_string_lossy().ends_with(".partial.lock"))
        .collect();
    assert!(blobs_left.is_empty(), "{blobs_left:?} {err:?}");
    for d in std::fs::read_dir(dir.path().join("cache/disks")).unwrap() {
        let leftovers: Vec<_> = std::fs::read_dir(d.unwrap().path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != ".lock" && n != ".last-used" && n != "disk.qcow2")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
    cua_vmm::disk::set_probe(None);
    server.abort();
}
