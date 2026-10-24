//! The bulk lane against the in-process mock spacesd, whose "disk" is a
//! plain file: large transfers move through it, and every doubt (another
//! VM's file, a file that is not private, a guest without a lane) leaves
//! them on the network. Unix only.
#![cfg(unix)]

use bytes::Bytes;
use cua_spacesd_client::{
    ConnectOptions, DownloadOptions, SpacesdClient, TransportPreference, UploadOptions,
    testing::{MockAuth, MockServer},
};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;

const TOKEN: &str = "tok";

async fn server() -> MockServer {
    MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await
}

async fn connect(srv: &MockServer) -> SpacesdClient {
    let opts = ConnectOptions::parse(&srv.url())
        .unwrap()
        .token(TOKEN)
        .transport(TransportPreference::Native);
    SpacesdClient::connect(opts).await.expect("connect")
}

fn data(len: usize, seed: u64) -> Bytes {
    let mut out = vec![0u8; len];
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for chunk in out.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let b = x.to_le_bytes();
        chunk.copy_from_slice(&b[..chunk.len()]);
    }
    out.into()
}

fn sha(d: &[u8]) -> String {
    hex::encode(Sha256::digest(d))
}

fn private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// A mock with a bulk lane whose file is `disk` (private), and a client
/// that proved it.
async fn with_lane(dir: &std::path::Path) -> (MockServer, SpacesdClient, std::path::PathBuf) {
    let srv = server().await;
    let disk = dir.join("bulk.img");
    srv.state.enable_bulk(&disk);
    private(&disk);
    let env = connect(&srv).await;
    assert!(env.attach_bulk(&disk).await, "the lane is proven");
    assert!(env.bulk_attached());
    (srv, env, disk)
}

#[tokio::test]
async fn a_large_download_moves_through_the_slots_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let (srv, env, _) = with_lane(dir.path()).await;
    let body = data(40 * 1024 * 1024 + 4099, 7);
    srv.state.put_file("/big.bin", body.to_vec());
    let got = env.download("/big.bin").await.unwrap();
    assert_eq!(sha(&got), sha(&body));
    // 16 MiB windows: three of them.
    assert_eq!(srv.state.observed.bulk_reads.load(Ordering::Relaxed), 3);
    let out = dir.path().join("out.bin");
    let r = env
        .download_to_file("/big.bin", &out, DownloadOptions::default())
        .await
        .unwrap();
    assert_eq!(r.sha256, sha(&body));
    assert_eq!(r.size, body.len() as u64);
    assert_eq!(std::fs::read(&out).unwrap(), body.to_vec());
}

#[tokio::test]
async fn ranges_tiny_and_empty_files_arrive_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let (srv, env, _) = with_lane(dir.path()).await;
    let body = data(20 * 1024 * 1024, 9);
    srv.state.put_file("/r.bin", body.to_vec());
    let mut got = Vec::new();
    env.download_with(
        "/r.bin",
        DownloadOptions {
            offset: 1000,
            length: 17 * 1024 * 1024 + 5,
            ..Default::default()
        },
        &mut got,
    )
    .await
    .unwrap();
    assert_eq!(got, body[1000..1000 + 17 * 1024 * 1024 + 5].to_vec());
    // From an offset to the end of the file.
    let mut tail = Vec::new();
    env.download_with(
        "/r.bin",
        DownloadOptions {
            offset: 19 * 1024 * 1024,
            ..Default::default()
        },
        &mut tail,
    )
    .await
    .unwrap();
    assert_eq!(tail, body[19 * 1024 * 1024..].to_vec());
    // Files under the lane's threshold never touch it (its extra round
    // trip costs more than the bytes it saves).
    let reads = srv.state.observed.bulk_reads.load(Ordering::Relaxed);
    let chunks = srv.state.observed.bulk_chunks.load(Ordering::Relaxed);
    srv.state.put_file("/tiny", b"hello".to_vec());
    assert_eq!(env.download("/tiny").await.unwrap(), Bytes::from("hello"));
    srv.state.put_file("/small", data(1 << 20, 4).to_vec());
    assert_eq!(env.download("/small").await.unwrap().len(), 1 << 20);
    env.upload("/small-up", data(1 << 20, 5), UploadOptions::default())
        .await
        .unwrap();
    assert_eq!(srv.state.observed.bulk_reads.load(Ordering::Relaxed), reads);
    assert_eq!(
        srv.state.observed.bulk_chunks.load(Ordering::Relaxed),
        chunks
    );
    srv.state.put_file("/empty", Vec::new());
    assert!(env.download("/empty").await.unwrap().is_empty());
    // A missing file still reports as missing.
    assert!(env.download("/nope").await.is_err());
}

#[tokio::test]
async fn a_large_upload_moves_through_the_slots_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let (srv, env, _) = with_lane(dir.path()).await;
    let body = data(40 * 1024 * 1024 + 123, 3);
    let r = env
        .upload("/up.bin", body.clone(), UploadOptions::default())
        .await
        .unwrap();
    assert_eq!(r.sha256, sha(&body));
    assert_eq!(srv.state.file("/up.bin").unwrap(), body.to_vec());
    assert_eq!(srv.state.observed.bulk_chunks.load(Ordering::Relaxed), 3);
    // From a local file.
    let local = dir.path().join("src.bin");
    std::fs::write(&local, &body[..9 * 1024 * 1024]).unwrap();
    env.upload("/up2.bin", local.as_path(), UploadOptions::default())
        .await
        .unwrap();
    assert_eq!(
        srv.state.file("/up2.bin").unwrap(),
        body[..9 * 1024 * 1024].to_vec()
    );
    // Small uploads stay on the network.
    let before = srv.state.observed.bulk_chunks.load(Ordering::Relaxed);
    env.upload(
        "/s.bin",
        Bytes::from_static(b"small"),
        UploadOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        srv.state.observed.bulk_chunks.load(Ordering::Relaxed),
        before
    );
}

#[tokio::test]
async fn concurrent_transfers_share_the_slots_without_mixing_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (srv, env, _) = with_lane(dir.path()).await;
    let mut tasks = Vec::new();
    for i in 0..6u64 {
        let body = data(20 * 1024 * 1024 + i as usize, i);
        srv.state.put_file(&format!("/c{i}"), body.to_vec());
        let env = env.clone();
        tasks.push(tokio::spawn(async move {
            let got = env.download(&format!("/c{i}")).await.unwrap();
            assert_eq!(sha(&got), sha(&body), "download {i}");
            let up = env
                .upload(&format!("/u{i}"), body.clone(), UploadOptions::default())
                .await
                .unwrap();
            assert_eq!(up.sha256, sha(&body), "upload {i}");
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    for i in 0..6u64 {
        let body = data(20 * 1024 * 1024 + i as usize, i);
        assert_eq!(srv.state.file(&format!("/u{i}")).unwrap(), body.to_vec());
    }
}

#[tokio::test]
async fn another_guests_file_is_not_a_lane() {
    let dir = tempfile::tempdir().unwrap();
    // Two guests, each with its own disk file.
    let (_a, _env_a, disk_a) = with_lane(dir.path()).await;
    let dir_b = tempfile::tempdir().unwrap();
    let srv_b = server().await;
    let disk_b = dir_b.path().join("bulk.img");
    srv_b.state.enable_bulk(&disk_b);
    private(&disk_b);
    let env_b = connect(&srv_b).await;
    assert!(
        !env_b.attach_bulk(&disk_a).await,
        "A's file is not B's disk"
    );
    assert!(!env_b.bulk_attached());
    let body = data(20 * 1024 * 1024, 5);
    srv_b.state.put_file("/f", body.to_vec());
    assert_eq!(env_b.download("/f").await.unwrap(), body);
    assert_eq!(srv_b.state.observed.bulk_reads.load(Ordering::Relaxed), 0);
    // Its own file is.
    assert!(env_b.attach_bulk(&disk_b).await);
}

#[tokio::test]
async fn a_file_that_is_not_private_a_missing_file_or_a_link_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let srv = server().await;
    let disk = dir.path().join("bulk.img");
    srv.state.enable_bulk(&disk);
    let env = connect(&srv).await;
    std::fs::set_permissions(&disk, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!env.attach_bulk(&disk).await, "readable by others");
    private(&disk);
    assert!(!env.attach_bulk(dir.path().join("missing")).await);
    let link = dir.path().join("link.img");
    std::os::unix::fs::symlink(&disk, &link).unwrap();
    assert!(!env.attach_bulk(&link).await, "a link");
    assert!(env.attach_bulk(&disk).await);
}

#[tokio::test]
async fn a_guest_without_a_lane_attaches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server().await;
    let env = connect(&srv).await;
    let disk = dir.path().join("bulk.img");
    std::fs::write(&disk, vec![0u8; 1 << 20]).unwrap();
    private(&disk);
    assert!(!env.attach_bulk(&disk).await);
    let body = data(20 * 1024 * 1024, 1);
    srv.state.put_file("/f", body.to_vec());
    assert_eq!(env.download("/f").await.unwrap(), body);
}

#[tokio::test]
async fn a_restored_guest_with_a_blank_file_gets_a_new_identity() {
    let dir = tempfile::tempdir().unwrap();
    let (srv, env, disk) = with_lane(dir.path()).await;
    // A fork of this guest: same memory, a new blank disk file.
    std::fs::write(&disk, b"").unwrap();
    srv.state.enable_bulk(&disk);
    private(&disk);
    // The stale proof does not matter: attaching again reads fresh.
    assert!(env.attach_bulk(&disk).await);
    let body = data(18 * 1024 * 1024, 2);
    srv.state.put_file("/f", body.to_vec());
    assert_eq!(env.download("/f").await.unwrap(), body);
}
