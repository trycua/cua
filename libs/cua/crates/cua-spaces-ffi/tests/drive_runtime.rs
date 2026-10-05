// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The drive's runtime methods (Settings > Storage, the mount, sync, the
//! cache) through the SDK with the Cua Spaces extensions registered, embedded in a throwaway cua home. Nothing here
//! mounts anything or touches the real home.

use cua_sdk::{Cua, CuaConfig, DriveStorageUpdate};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_mount_sync_and_cache_answer_through_the_sdk() {
    // The Cua Spaces extensions with a test presence gate (nothing reaches
    // the OS key store), registered before the runtime is built.
    cua_daemon::extension::register(std::sync::Arc::new(
        cua_spaces_ext::daemon::CuaSpacesDaemon::with_test_presence(std::sync::Arc::new(
            cua_keyvault::broker::FakePresence::new(true),
        )),
    ));
    let dir = tempfile::tempdir().unwrap();
    let c = Cua::embedded(CuaConfig {
        state_dir: Some(dir.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dir.path().join("cua").display().to_string()),
        fleet_pool_home: Some(dir.path().join("pools").display().to_string()),
        fleet_from_env: false,
        fleet_from_session: false,
        ..Default::default()
    })
    .unwrap();
    let spaces = c.spaces();
    let st = spaces.volume_storage().await.unwrap();
    assert_eq!(st.backend, "fs");
    assert!(!st.cloud_available);
    let check = spaces
        .volume_storage_set(DriveStorageUpdate {
            backend: "fs".into(),
            s3: None,
            access_key_id: None,
            secret_access_key: None,
            dry_run: true,
        })
        .await
        .unwrap();
    assert!(check.ok && !check.applied, "{check:?}");
    let refused = spaces
        .volume_storage_set(DriveStorageUpdate {
            backend: "cloud".into(),
            s3: None,
            access_key_id: None,
            secret_access_key: None,
            dry_run: true,
        })
        .await;
    assert!(refused.is_err());
    let m = spaces.volume_mount_status().await.unwrap();
    // Off, or unsupported in a build without mount support (the daemon
    // has it; an embedded SDK may not): never a pretend mount.
    assert!(
        !m.enabled && (m.state == "off" || m.state == "unsupported"),
        "{m:?}"
    );
    assert_eq!(m.volume_name, "Cua Volume");
    let s = spaces.volume_sync_status().await.unwrap();
    assert_eq!(s.feed, "off", "a store on this machine");
    assert!(s.devices.iter().any(|d| d.this_device));
    spaces
        .volume_write("public/a.md".into(), b"x".to_vec(), None, false, None)
        .await
        .unwrap();
    let ev = spaces.volume_sync_events(Some(0), Some(200)).await.unwrap();
    assert!(ev.next_seq >= ev.events.len() as u64);
    let cache = spaces.volume_cache_set(300 << 20).await.unwrap();
    assert_eq!(cache.capacity_bytes, 300 << 20);
    assert_eq!(
        spaces.volume_cache_stats().await.unwrap().block_bytes,
        1 << 20
    );
    assert_eq!(spaces.volume_cache_clear().await.unwrap().blocks, 0);
    assert!(spaces.volume_sync_resolve("none".into()).await.is_err());
}
