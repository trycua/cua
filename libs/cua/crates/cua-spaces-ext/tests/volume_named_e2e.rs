// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in e2e (`CUA_VOLUME_E2E=1`, the macOS / nightly lane): named volumes
//! of Cua Volume mounted into real Spaces of different kinds, through the
//! daemon's own runtime with an isolated cua home.
//!
//! - `CUA_VOLUME_E2E_LINUX_IMAGE`: a Linux container image with this
//!   branch's cua-spacesd (see `run-volume-e2e.sh`). Space A, read-write.
//! - `CUA_VOLUME_E2E_VM_IMAGE` (optional): a Linux VM image (`kind=vm`, QEMU).
//!   Space B, read-only. With `CUA_VOLUME_E2E_VM_SPACESD` (this branch's
//!   Linux cua-spacesd) the guest's daemon is replaced first, for images that
//!   predate named volumes.
//!
//! A writes a file into volume `shared`; B reads it and cannot write (EROFS);
//! a second volume neither Space mounted stays invisible; both Spaces are
//! deleted and the file is still in `volumes/shared/`. A is stopped and
//! started to see its mounts come back.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_daemon::{Runtime, RuntimeConfig};
use cua_keyvault::broker::FakePresence;
use cua_sandbox_core::placement::{Kind, On};
use cua_spaces::named_volumes::VolumeMount;
use cua_spaces::{Space, SpaceCreate, SpaceCreated, Spaces};
use cua_spaces_ext::SpacesDrive as _;
use cua_spaces_ext::daemon::CuaSpacesDaemon;
use serde_json::json;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|s| !s.is_empty())
}

async fn sh(space: &Space, cmd: &str) -> (bool, String, String) {
    let out = space
        .bash(cmd, Duration::from_secs(120))
        .await
        .unwrap_or_else(|e| panic!("{cmd}: {e}"));
    (out.success(), out.stdout, out.stderr)
}

async fn ok(space: &Space, cmd: &str) -> String {
    let (good, out, err) = sh(space, cmd).await;
    assert!(good, "{cmd}: {out}{err}");
    out
}

async fn until<T, F, Fut>(what: &str, limit: Duration, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let t0 = Instant::now();
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(t0.elapsed() < limit, "{what}: not within {limit:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn create(spaces: &Spaces, image: &str, kind: Kind, volumes: Vec<VolumeMount>) -> String {
    let created = spaces
        .create(SpaceCreate {
            image: Some(image.into()),
            on: Some(On::Local),
            kind,
            spacesd: Some(true),
            wait: Some(true),
            memory_mb: Some(4096),
            timeout: Some(Duration::from_secs(1800)),
            volumes,
            ..Default::default()
        })
        .await
        .unwrap();
    match created {
        SpaceCreated::Ready { info, .. } => info.id,
        SpaceCreated::Starting(p) => panic!("{} is still starting", p.id),
    }
}

/// Replaces the guest's cua-spacesd with `bin` and waits for it to answer
/// with named volumes.
async fn upgrade_spacesd(spaces: &Spaces, id: &str, bin: &Path) {
    let space = spaces.space(id).await.unwrap();
    space
        .upload(bin, Some("/tmp/cua-spacesd.new"))
        .await
        .unwrap();
    ok(
        &space,
        "chmod 755 /tmp/cua-spacesd.new && S='sudo -n' && \
         dest=$(readlink -f /usr/local/bin/cua-spacesd) && \
         $S install -m 755 /tmp/cua-spacesd.new \"$dest\" && \
         ($S systemctl restart cua-volume-helper >/dev/null 2>&1 || true) && \
         (nohup sh -c 'sleep 1; sudo -n systemctl restart cua-spacesd' >/dev/null 2>&1 &)",
    )
    .await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    until(
        "the new driver answers",
        Duration::from_secs(180),
        || async {
            let _ = spaces.forget_connection(id).await;
            let s = spaces.space(id).await.ok()?;
            s.supports(cua_spaces::named_volumes::BINDS_FEATURE)
                .then_some(())
        },
    )
    .await;
}

fn mount(name: &str, path: &str, read_only: bool) -> VolumeMount {
    VolumeMount {
        name: name.into(),
        path: path.into(),
        read_only,
        subpath: String::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_volume_is_shared_by_a_container_and_a_vm() {
    if env("CUA_VOLUME_E2E").as_deref() != Some("1") {
        eprintln!("skipped: set CUA_VOLUME_E2E=1 (run tests/e2e/run-volume-e2e.sh)");
        return;
    }
    let linux = env("CUA_VOLUME_E2E_LINUX_IMAGE").expect("CUA_VOLUME_E2E_LINUX_IMAGE");
    let vm = env("CUA_VOLUME_E2E_VM_IMAGE");
    let vm_bin = env("CUA_VOLUME_E2E_VM_SPACESD");
    let base = env("CUA_VOLUME_E2E_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&base).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("volume-named-")
        .tempdir_in(&base)
        .unwrap();
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().join("cua")),
        vmm: Some(Arc::new(cua_daemon::local::VmmLocal::new(
            Default::default(),
        ))),
        extensions: vec![Arc::new(CuaSpacesDaemon::with_test_presence(Arc::new(
            FakePresence::new(true),
        )))],
        ..Default::default()
    })
    .unwrap();
    let spaces = rt.spaces().clone();
    let run = {
        let spaces = spaces.clone();
        async move {
            let user = spaces.drive().session(cua_volume::Context::user());
            for name in ["shared", "secret"] {
                spaces
                    .call_extension("Cua Volume", "volumes.create", json!({ "name": name }))
                    .await
                    .unwrap();
            }
            user.write(
                "volumes/secret/private.txt",
                b"not for the Spaces".to_vec(),
                cua_volume::Condition::None,
            )
            .await
            .unwrap();

            // A: a container, read-write.
            let a = create(
                &spaces,
                &linux,
                Kind::Container,
                vec![mount("shared", "/mnt/shared", false)],
            )
            .await;
            let sa = spaces.space(&a).await.unwrap();
            let info = spaces.volume(&a).await.expect("A's volume is mounted");
            assert!(info.mount_path.starts_with('/'));
            eprintln!(
                "A (container) {}\n{}",
                ok(&sa, "uname -r; grep -m1 PRETTY /etc/os-release; dmesg 2>/dev/null | head -1; mount | grep ' /mnt/shared '").await,
                ok(&sa, "ls -la /mnt/shared").await
            );
            ok(&sa, "echo 'hello from A' > /mnt/shared/hello.txt && sync").await;
            until(
                "the host sees A's file",
                Duration::from_secs(30),
                || async {
                    user.read("volumes/shared/hello.txt", None)
                        .await
                        .ok()
                        .filter(|(b, _)| b == b"hello from A\n")
                },
            )
            .await;
            // The Space sees the volume it mounted and no other.
            let seen = ok(&sa, &format!("ls -1 '{}/volumes'", info.mount_path)).await;
            assert_eq!(seen.trim(), "shared", "{seen}");
            let (good, _, _) = sh(
                &sa,
                &format!("cat '{}/volumes/secret/private.txt'", info.mount_path),
            )
            .await;
            assert!(!good, "A read a volume it did not mount");

            // B: a VM, read-only.
            if let Some(image) = vm {
                let b = create(&spaces, &image, Kind::Vm, vec![]).await;
                if let Some(bin) = &vm_bin {
                    upgrade_spacesd(&spaces, &b, Path::new(bin)).await;
                }
                // Mounts are part of the Space's record; this image predates
                // them at create, so the record gets them now and the volume
                // is attached again.
                let mut rec = spaces.registry().get(&b).unwrap().unwrap();
                rec.volumes = vec![(&mount("shared", "/mnt/shared", true)).into()];
                spaces
                    .registry()
                    .upsert(rec, cua_spaces::registry::Credential::default())
                    .unwrap();
                spaces.volume_detach(&b).await.unwrap();
                spaces.ensure_volumes(&b).await.unwrap();
                let sb = spaces.space(&b).await.unwrap();
                eprintln!(
                    "B (vm {image}) {}",
                    ok(&sb, "uname -r; mount | grep ' /mnt/shared '").await
                );
                let got = ok(&sb, "cat /mnt/shared/hello.txt").await;
                assert_eq!(got, "hello from A\n");
                let (good, _, err) = sh(&sb, "echo no > /mnt/shared/nope.txt").await;
                assert!(!good, "B wrote to a read-only mount");
                assert!(
                    err.contains("Read-only file system"),
                    "expected EROFS, got {err}"
                );
                eprintln!("B write attempt: {err}");
                assert!(user.read("volumes/shared/nope.txt", None).await.is_err());
                // A's later writes show up in B.
                ok(&sa, "echo second > /mnt/shared/second.txt && sync").await;
                until(
                    "B sees A's second file",
                    Duration::from_secs(30),
                    || async {
                        let (good, out, _) = sh(&sb, "cat /mnt/shared/second.txt").await;
                        (good && out == "second\n").then_some(())
                    },
                )
                .await;
            }

            eprintln!(
                "host view of volumes/shared: {:?}",
                user.ls("volumes/shared/")
                    .await
                    .unwrap()
                    .iter()
                    .map(|e| e.name.clone())
                    .collect::<Vec<_>>()
            );
            // A restart brings the mounts back.
            spaces.stop(&a).await.unwrap();
            spaces.start(&a).await.unwrap();
            let sa = until("A answers again", Duration::from_secs(180), || async {
                let _ = spaces.forget_connection(&a).await;
                spaces.space(&a).await.ok()
            })
            .await;
            until("A's mounts return", Duration::from_secs(180), || async {
                let (good, out, _) = sh(&sa, "cat /mnt/shared/hello.txt").await;
                (good && out == "hello from A\n").then_some(())
            })
            .await;
        }
    };
    let outcome = tokio::spawn(run).await;
    for s in spaces.list().unwrap_or_default() {
        if s.id.starts_with("local:") {
            let _ = spaces.delete(&s.id).await;
        }
    }
    // The file outlives both Spaces.
    let user = spaces.drive().session(cua_volume::Context::user());
    if outcome.is_ok() {
        assert_eq!(
            user.read("volumes/shared/hello.txt", None).await.unwrap().0,
            b"hello from A\n"
        );
    }
    spaces.stop_drive_service().await;
    drop(rt);
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}
