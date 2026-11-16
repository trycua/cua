//! A blob download that breaks mid-way continues from the bytes that
//! arrived (`Range`), within one pull and across pulls, and the result is
//! checked against the digest. Against a loopback plain-HTTP registry that
//! cuts its answers short. Its own test binary, since it sets env vars.

use std::sync::{Arc, Mutex};

use cua_image::RegistryClient;
use cua_image::digest::sha256_bytes;
use cua_image::manifest::Descriptor;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the registry did with each blob request: the `Range` start asked
/// for (0 without one).
type Seen = Arc<Mutex<Vec<u64>>>;

/// Serves `blob` at any `/v2/<repo>/blobs/<digest>` path, honouring
/// `Range: bytes=N-`, and closes each answer after at most `cut` body bytes
/// (the header still promises the rest).
async fn serve(listener: TcpListener, blob: Arc<Vec<u8>>, cut: usize, seen: Seen) {
    for _ in 0..50 {
        let Ok((mut s, _)) = listener.accept().await else {
            return;
        };
        let (blob, seen) = (blob.clone(), seen.clone());
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
            if path == "/v2/" {
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await;
                return;
            }
            if !path.contains("/blobs/") {
                let _ = s
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                return;
            }
            let from = req
                .lines()
                .find_map(|l| {
                    let l = l.to_ascii_lowercase();
                    l.strip_prefix("range: bytes=")
                        .and_then(|r| r.trim().trim_end_matches('-').parse::<u64>().ok())
                })
                .unwrap_or(0);
            seen.lock().unwrap().push(from);
            let rest = &blob[from as usize..];
            let head = if from > 0 {
                format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    blob.len() - 1,
                    blob.len(),
                    rest.len()
                )
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    rest.len()
                )
            };
            let _ = s.write_all(head.as_bytes()).await;
            let _ = s.write_all(&rest[..rest.len().min(cut)]).await;
            let _ = s.flush().await;
            // Dropping the socket cuts the answer short.
        });
    }
}

async fn registry(blob: &[u8], cut: usize) -> (String, Seen, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Seen::default();
    let server = tokio::spawn(serve(listener, Arc::new(blob.to_vec()), cut, seen.clone()));
    let empty = tempfile::tempdir().unwrap().keep();
    unsafe {
        std::env::set_var("DOCKER_CONFIG", &empty);
        std::env::set_var("CUA_REGISTRY_CRED_HELPERS", "0");
        std::env::remove_var("CUA_REGISTRY_USERNAME");
        std::env::set_var("CUA_INSECURE_REGISTRIES", &host);
    }
    (host, seen, server)
}

fn descriptor(blob: &[u8]) -> Descriptor {
    Descriptor {
        media_type: "application/vnd.oci.image.layer.v1.tar+gzip".into(),
        digest: sha256_bytes(blob),
        size: blob.len() as u64,
        ..Default::default()
    }
}

/// Tests share the process env (`CUA_INSECURE_REGISTRIES`): one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn a_broken_download_continues_where_it_stopped() {
    let _g = SERIAL.lock().await;
    let blob: Vec<u8> = (0..30u8).collect();
    // Every answer stops after 10 bytes: three requests, each continuing.
    let (host, seen, server) = registry(&blob, 10).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("blob");
    let mut progress = Vec::new();
    RegistryClient::new(vec![host.clone()])
        .blob_to_file_with_progress(
            &format!("{host}/team/disk:1"),
            &descriptor(&blob),
            &dest,
            |n| progress.push(n),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), blob);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![0, 10, 20],
        "never downloads a byte twice"
    );
    assert_eq!(progress.last(), Some(&30));
    assert!(
        progress.windows(2).all(|w| w[0] <= w[1]),
        "progress never goes back: {progress:?}"
    );
    assert!(!dest.with_extension("partial").exists());
    server.abort();
}

#[tokio::test]
async fn a_later_pull_continues_what_an_earlier_one_left() {
    let _g = SERIAL.lock().await;
    let blob: Vec<u8> = (0..40u8).collect();
    let (host, seen, server) = registry(&blob, usize::MAX).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("blob");
    // An earlier pull that gave up after 25 bytes.
    std::fs::write(dest.with_extension("partial"), &blob[..25]).unwrap();
    RegistryClient::new(vec![host.clone()])
        .blob_to_file(&format!("{host}/team/disk:1"), &descriptor(&blob), &dest)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), blob);
    assert_eq!(*seen.lock().unwrap(), vec![25]);
    server.abort();
}

#[tokio::test]
async fn a_continued_blob_that_does_not_match_its_digest_is_refused_and_dropped() {
    let _g = SERIAL.lock().await;
    let blob: Vec<u8> = (0..40u8).collect();
    let (host, _seen, server) = registry(&blob, usize::MAX).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("blob");
    // Left by an earlier pull, but not the blob's first bytes.
    std::fs::write(dest.with_extension("partial"), [9u8; 25]).unwrap();
    let err = RegistryClient::new(vec![host.clone()])
        .blob_to_file(&format!("{host}/team/disk:1"), &descriptor(&blob), &dest)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("digest mismatch"), "{err}");
    assert!(!dest.exists());
    assert!(
        !dest.with_extension("partial").exists(),
        "a bad partial is not kept"
    );
    server.abort();
}
