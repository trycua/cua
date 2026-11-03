// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The S3 backend against a real versioned bucket. Opt-in: without
//! `CUA_DRIVE_S3_TEST_ENDPOINT` every test prints why and passes. Run it
//! through `tests/run-minio.sh`, which starts MinIO in Docker (1 GiB memory
//! cap) on a loopback port and removes it afterwards.
//!
//! Env: `CUA_DRIVE_S3_TEST_ENDPOINT`, `CUA_DRIVE_S3_TEST_ACCESS_KEY`,
//! `CUA_DRIVE_S3_TEST_SECRET_KEY`.
#![cfg(feature = "s3")]

use std::sync::Arc;

use cua_volume::s3::{S3Backend, S3Config, S3Credentials, StaticCredentials};
use cua_volume::{Condition, Context, Drive, Mode};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

async fn backend(root: &str) -> Option<S3Backend> {
    let endpoint = env("CUA_DRIVE_S3_TEST_ENDPOINT")?;
    let bucket = "cua-volume-test";
    let b = S3Backend::new(
        S3Config {
            endpoint: Some(endpoint),
            region: "us-east-1".into(),
            bucket: bucket.into(),
            root: root.into(),
            path_style: true,
        },
        Arc::new(StaticCredentials(S3Credentials {
            access_key_id: env("CUA_DRIVE_S3_TEST_ACCESS_KEY").expect("access key"),
            secret_access_key: env("CUA_DRIVE_S3_TEST_SECRET_KEY").expect("secret key"),
            session_token: None,
            expires_ms: None,
        })),
    )
    .unwrap();
    let c = b.client();
    let _ = c.create_bucket().bucket(bucket).send().await;
    c.put_bucket_versioning()
        .bucket(bucket)
        .versioning_configuration(
            aws_sdk_s3::types::VersioningConfiguration::builder()
                .status(aws_sdk_s3::types::BucketVersioningStatus::Enabled)
                .build(),
        )
        .send()
        .await
        .expect("enable versioning");
    Some(b)
}

fn root() -> String {
    format!("run-{:08x}/", rand::random::<u32>())
}

#[tokio::test]
async fn s3_backend_conforms() {
    let Some(b) = backend(&root()).await else {
        eprintln!("skipped: set CUA_DRIVE_S3_TEST_ENDPOINT (run tests/run-minio.sh)");
        return;
    };
    cua_volume::conformance::run(&b, "conf/").await.unwrap();
}

#[tokio::test]
async fn permissions_leases_and_sync_hold_on_s3() {
    let Some(b) = backend(&root()).await else {
        eprintln!("skipped: set CUA_DRIVE_S3_TEST_ENDPOINT (run tests/run-minio.sh)");
        return;
    };
    let state = tempfile::tempdir().unwrap();
    let d = Drive::new(Arc::new(b), state.path());
    let user = d.session(Context::user());
    user.write(
        "agents/writer/outputs/r.md",
        b"draft".to_vec(),
        Condition::None,
    )
    .await
    .unwrap();
    let rs = d.session(Context::agent("researcher", Some("local:lab")));
    assert_eq!(
        rs.read("agents/writer/outputs/r.md", None)
            .await
            .unwrap_err()
            .tag(),
        "forbidden"
    );
    rs.write("agents/researcher/m.md", b"mine".to_vec(), Condition::None)
        .await
        .unwrap();
    assert!(
        d.grant("agent:researcher", "agents/writer/", Mode::Read, None, "")
            .is_err()
    );
    let lease = cua_volume::lease::Lease::acquire(&d, "researcher", "h1", 60_000)
        .await
        .unwrap();
    assert_eq!(
        cua_volume::lease::Lease::acquire(&d, "researcher", "h2", 60_000)
            .await
            .unwrap_err()
            .tag(),
        "lease_held"
    );
    lease.release(&d).await.unwrap();
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("mem")).unwrap();
    std::fs::write(src.path().join("mem/MEMORY.md"), "tea").unwrap();
    let r = cua_volume::sync::push_dir(&rs, "agents/researcher/home/", src.path())
        .await
        .unwrap();
    assert_eq!(r.uploaded, ["mem/MEMORY.md"]);
    let dst = tempfile::tempdir().unwrap();
    cua_volume::sync::pull_dir(&rs, "agents/researcher/home/", dst.path())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dst.path().join("mem/MEMORY.md")).unwrap(),
        "tea"
    );
}

#[tokio::test]
async fn ranged_reads_multipart_listings_and_copies_on_s3() {
    use cua_volume::Backend as _;
    let Some(b) = backend(&root()).await else {
        eprintln!("skipped: set CUA_DRIVE_S3_TEST_ENDPOINT (run tests/run-minio.sh)");
        return;
    };
    assert!(b.versioning_enabled().await.unwrap());
    assert!(b.remote());
    // A multipart upload (3 parts) from a file, then ranged reads of it.
    let dir = tempfile::tempdir().unwrap();
    let big: Vec<u8> = (0..(40u32 << 20)).map(|i| (i % 251) as u8).collect();
    let path = dir.path().join("big.bin");
    std::fs::write(&path, &big).unwrap();
    let m = b
        .put_file("media/big.bin", &path, Condition::IfNoneMatch)
        .await
        .unwrap();
    assert_eq!(m.size, big.len() as u64);
    assert!(!m.version.is_empty());
    assert_eq!(
        b.put_file("media/big.bin", &path, Condition::IfNoneMatch)
            .await
            .unwrap_err()
            .tag(),
        "precondition_failed"
    );
    let mid = (20u64 << 20) + 7;
    let r = b
        .get_range("media/big.bin", &m.version, mid, 4096)
        .await
        .unwrap();
    assert_eq!(r, &big[mid as usize..mid as usize + 4096]);
    let tail = b
        .get_range("media/big.bin", &m.version, big.len() as u64 - 10, 100)
        .await
        .unwrap();
    assert_eq!(tail.len(), 10);
    assert!(
        b.get_range("media/big.bin", &m.version, big.len() as u64 + 5, 10)
            .await
            .unwrap()
            .is_empty()
    );
    // A new version does not change what the pinned one reads.
    let small = dir.path().join("small.bin");
    std::fs::write(&small, b"replaced").unwrap();
    let m2 = b
        .put_file("media/big.bin", &small, Condition::IfMatch(m.etag.clone()))
        .await
        .unwrap();
    assert_eq!(
        b.get_range("media/big.bin", &m.version, 0, 4)
            .await
            .unwrap(),
        &big[..4]
    );
    assert_eq!(
        b.get_range("media/big.bin", &m2.version, 0, 8)
            .await
            .unwrap(),
        b"replaced"
    );
    // One level of a folder; a cursor read; a server-side copy.
    b.put("media/a/x", b"1".to_vec(), Condition::None)
        .await
        .unwrap();
    b.put("media/b.txt", b"2".to_vec(), Condition::None)
        .await
        .unwrap();
    let (files, folders) = b.list_dir("media/").await.unwrap();
    let names: Vec<&str> = files.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(names, ["media/b.txt", "media/big.bin"]);
    assert_eq!(folders, ["media/a/"]);
    let after = b.list_after("media/", "media/a/x", 10).await.unwrap();
    let keys: Vec<&str> = after.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(keys, ["media/b.txt", "media/big.bin"]);
    let c = b.copy("media/b.txt", "media/copy.txt").await.unwrap();
    assert_eq!(c.size, 1);
    assert_eq!(b.get("media/copy.txt", None).await.unwrap().0, b"2");
}

#[tokio::test]
async fn two_devices_on_one_bucket_sync_in_seconds_and_keep_conflicts() {
    use cua_volume::feed::{DeviceId, Feed};
    use cua_volume::vfs::Vfs;
    let r = root();
    let (Some(ba), Some(bb)) = (backend(&r).await, backend(&r).await) else {
        eprintln!("skipped: set CUA_DRIVE_S3_TEST_ENDPOINT (run tests/run-minio.sh)");
        return;
    };
    let sa = tempfile::tempdir().unwrap();
    let sb = tempfile::tempdir().unwrap();
    let da = Drive::new(Arc::new(ba), sa.path());
    let db = Drive::new(Arc::new(bb), sb.path());
    let fa = Feed::new(
        &da,
        DeviceId {
            id: "deva".into(),
            name: "A".into(),
        },
    );
    let fb = Feed::new(
        &db,
        DeviceId {
            id: "devb".into(),
            name: "B".into(),
        },
    );
    fa.spawn();
    fb.spawn();
    let cache =
        Arc::new(cua_volume::cache::BlockCache::open(sb.path().join("cache"), 256 << 20).unwrap());
    let vb = Vfs::new(
        &db,
        Context::user(),
        Some(cache.clone()),
        Some(fb.clone()),
        sb.path(),
    )
    .unwrap();
    let va = Vfs::new(&da, Context::user(), None, Some(fa.clone()), sa.path()).unwrap();
    // A writes; B hears about it through the bucket within seconds.
    let t0 = std::time::Instant::now();
    da.session(Context::user())
        .write("public/hello.md", b"from A".to_vec(), Condition::None)
        .await
        .unwrap();
    let (ev, _) = fb.events(0, std::time::Duration::from_secs(10)).await;
    assert!(
        ev.iter()
            .any(|e| e.kind == "remote_change" && e.path == "public/hello.md"),
        "{ev:?}"
    );
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(7),
        "{:?}",
        t0.elapsed()
    );
    let hello = vb.lookup_path("public/hello.md").await.unwrap();
    assert_eq!(vb.read(hello.ino, 0, 100).await.unwrap().0, b"from A");
    assert!(cache.stats().misses >= 1);
    // Both edit it; B lands last and wins; A's write is kept visibly.
    let a_ino = va.lookup_path("public/hello.md").await.unwrap().ino;
    vb.write(hello.ino, 0, b"from B").await.unwrap();
    va.write(a_ino, 0, b"edit A").await.unwrap();
    va.flush("public/hello.md").await.unwrap();
    vb.flush("public/hello.md").await.unwrap();
    let user = db.session(Context::user());
    assert_eq!(
        user.read("public/hello.md", None).await.unwrap().0,
        b"from B"
    );
    let c = fb.status().conflicts[0].clone();
    assert_eq!(
        user.read(&c.conflict_path, None).await.unwrap().0,
        b"edit A"
    );
    assert!(user.history("public/hello.md").await.unwrap().len() >= 3);
    fa.stop();
    fb.stop();
}

#[tokio::test]
async fn the_storage_test_names_each_problem_and_switches_live() {
    use cua_volume::config::S3Settings;
    use cua_volume::service::{DriveService, KeyStore, StorageUpdate};
    /// Keys kept in memory (the daemon keeps them in the credential store).
    #[derive(Default)]
    struct MemKeys(std::sync::Mutex<Option<(String, String)>>);
    impl KeyStore for MemKeys {
        fn load(&self) -> cua_volume::Result<Option<(String, String)>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, a: &str, s: &str) -> cua_volume::Result<()> {
            *self.0.lock().unwrap() = Some((a.into(), s.into()));
            Ok(())
        }
    }
    let (Some(endpoint), Some(_)) = (env("CUA_DRIVE_S3_TEST_ENDPOINT"), backend(&root()).await)
    else {
        eprintln!("skipped: set CUA_DRIVE_S3_TEST_ENDPOINT (run tests/run-minio.sh)");
        return;
    };
    let key = env("CUA_DRIVE_S3_TEST_ACCESS_KEY").unwrap();
    let secret = env("CUA_DRIVE_S3_TEST_SECRET_KEY").unwrap();
    let home = tempfile::tempdir().unwrap();
    let drive = Drive::open_local(home.path());
    let keys = Arc::new(MemKeys::default());
    let svc = DriveService::new(drive.clone(), home.path(), keys.clone()).unwrap();
    let s3 = |endpoint: &str, bucket: &str, path_style: bool| S3Settings {
        endpoint: Some(endpoint.into()),
        region: "us-east-1".into(),
        bucket: bucket.into(),
        root: root(),
        path_style,
    };
    let test = |settings: S3Settings, k: &str, s: &str| StorageUpdate {
        backend: "s3".into(),
        s3: Some(settings),
        access_key_id: Some(k.into()),
        secret_access_key: Some(s.into()),
        dry_run: true,
    };
    let problem = |c: &cua_volume::service::StorageCheck| c.problem.clone().unwrap_or_default();
    // Everything right.
    let c = svc
        .set_storage(test(s3(&endpoint, "cua-volume-test", true), &key, &secret))
        .await
        .unwrap();
    assert!(c.ok && c.problem.is_none() && !c.applied, "{c:?}");
    // Wrong secret.
    let c = svc
        .set_storage(test(
            s3(&endpoint, "cua-volume-test", true),
            &key,
            "wrong-secret-0000",
        ))
        .await
        .unwrap();
    assert_eq!(problem(&c), "bad_keys", "{c:?}");
    // No such bucket.
    let c = svc
        .set_storage(test(
            s3(&endpoint, "no-such-bucket-cua", true),
            &key,
            &secret,
        ))
        .await
        .unwrap();
    assert_eq!(problem(&c), "bucket_missing", "{c:?}");
    // Versioning off, with how to turn it on.
    let b = backend(&root()).await.unwrap();
    let _ = b
        .client()
        .create_bucket()
        .bucket("cua-volume-unversioned")
        .send()
        .await;
    let c = svc
        .set_storage(test(
            s3(&endpoint, "cua-volume-unversioned", true),
            &key,
            &secret,
        ))
        .await
        .unwrap();
    assert_eq!(problem(&c), "versioning_off", "{c:?}");
    assert!(c.detail.unwrap().contains("mc version enable"));
    // MinIO by host name needs path-style: said so, not "unreachable". (By
    // IP address the SDK uses path-style on its own.)
    let by_name = endpoint.replace("127.0.0.1", "localhost");
    let c = svc
        .set_storage(test(s3(&by_name, "cua-volume-test", false), &key, &secret))
        .await
        .unwrap();
    assert_eq!(problem(&c), "path_style_needed", "{c:?}");
    // Nothing listening.
    let c = svc
        .set_storage(test(
            s3("http://127.0.0.1:9", "cua-volume-test", true),
            &key,
            &secret,
        ))
        .await
        .unwrap();
    assert_eq!(problem(&c), "unreachable", "{c:?}");
    // A good bucket switches the running drive with no restart, and the
    // keys go to the key store (never to config.json).
    let mut u = test(s3(&endpoint, "cua-volume-test", true), &key, &secret);
    u.dry_run = false;
    let c = svc.set_storage(u).await.unwrap();
    assert!(c.ok && c.applied, "{c:?}");
    assert_eq!(drive.backend().kind(), "s3");
    drive
        .session(Context::user())
        .write("public/switched.md", b"live".to_vec(), Condition::None)
        .await
        .unwrap();
    assert_eq!(svc.storage().unwrap().backend, "s3");
    assert!(svc.storage().unwrap().has_keys);
    let config = std::fs::read_to_string(home.path().join("drive/config.json")).unwrap();
    assert!(!config.contains(&secret), "{config}");
}
