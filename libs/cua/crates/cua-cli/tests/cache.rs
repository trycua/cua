//! `cua cache` and the disk-safety paths through the real binary, in a temp
//! home: accounting, prune (dry run, budget, --all), config, the orphan
//! reaper on the next CLI run, and the low-disk guard with a fake free
//! space. The container engine and Lume are pointed at addresses nothing
//! listens on, so the host's engine, Lume VMs and cua home are never read
//! or touched.

mod common;

use common::Home;
use std::path::Path;

fn home() -> Home {
    let mut h = Home::new();
    h.set("DOCKER_HOST", "unix:///nonexistent/cua-test-docker.sock")
        .set("LUME_API", "http://127.0.0.1:9")
        .set("CUA_NO_DAEMON_AUTOSTART", "1")
        .set("CUA_DAEMON_MAINTENANCE", "0")
        .set("CUA_CACHE_AUTO_GC", "0");
    h
}

/// A cached image directory with `kib` KiB, last used `days` days ago.
fn cached(h: &Home, hex: &str, kib: usize, days: u64) -> std::path::PathBuf {
    let dir = h.cua_home().join("images/disks").join(hex);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("disk.qcow2"), vec![3u8; kib << 10]).unwrap();
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    cua_vmm::disk::mark_used_at(&dir, t);
    dir
}

fn qemu_state(h: &Home, name: &str, disk: &Path, created_at: u64) {
    let dir = h.cua_home().join("vmm/qemu").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let st = cua_vmm::qemu::QemuState {
        name: name.into(),
        kind: cua_vmm::qemu::EntryKind::Instance,
        arch: cua_vmm::Arch::Aarch64,
        os: cua_vmm::GuestOs::Linux,
        disk: disk.to_path_buf(),
        disk_format: "qcow2".into(),
        install_iso: None,
        seed_iso: None,
        firmware: cua_vmm::qemu::Firmware::Bios,
        cpus: 1,
        memory_mb: 512,
        ports: Default::default(),
        vnc_display: None,
        qmp_port: None,
        pid: None,
        accel: String::new(),
        ssh: None,
        restrict_network: false,
        extra_args: vec![],
        snapshots: vec![],
        paused: false,
        source: None,
        created_at,
        gpu: None,
    };
    std::fs::write(dir.join("state.json"), serde_json::to_vec(&st).unwrap()).unwrap();
}

#[tokio::test]
async fn du_ls_prune_and_config() {
    let mut h = home();
    let used = cached(&h, "aaaa", 256, 30);
    let old = cached(&h, "bbbb", 256, 20);
    let build = h.cua_home().join("build/out-1234");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("layer.tar.gz"), vec![1u8; 128 << 10]).unwrap();
    cua_vmm::disk::mark_used_at(
        &build,
        std::time::SystemTime::now() - std::time::Duration::from_secs(86_400),
    );
    // A stopped named sandbox on the first image.
    let overlay = h.cua_home().join("vmm/qemu/keep/disk.qcow2");
    std::fs::create_dir_all(overlay.parent().unwrap()).unwrap();
    cua_disk::qcow2::write_header(&overlay, Some(&used.join("disk.qcow2"))).unwrap();
    qemu_state(&h, "keep", &overlay, 1);

    let du = h.run(&["cache", "du", "--json"]).await;
    let v = du.ok().json();
    let cat = |name: &str| {
        v["categories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["category"] == name)
            .cloned()
            .unwrap()
    };
    assert!(cat("images")["bytes"].as_u64().unwrap() >= 512 << 10, "{v}");
    assert_eq!(cat("builds")["items"], 1);
    assert_eq!(cat("sandboxes")["items"], 1);
    assert!(
        v["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("container engine")),
        "{v}"
    );
    let text = h.run(&["cache", "du"]).await;
    assert!(text.ok().stdout.contains("CATEGORY"), "{}", text.stdout);

    let ls = h
        .run(&["cache", "ls", "--category", "images", "--json"])
        .await;
    let rows = ls.ok().json();
    let a = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["location"].as_str().unwrap().ends_with("aaaa"))
        .unwrap();
    assert_eq!(a["referenced_by"][0], "keep");
    assert_eq!(h.run(&["cache", "ls", "--category", "nope"]).await.code, 2);

    // Dry run: reported, nothing removed.
    let dry = h.run(&["cache", "prune", "--all", "--dry-run"]).await;
    assert!(dry.ok().stdout.contains("would remove"), "{}", dry.stdout);
    assert!(old.exists() && build.exists());

    // A generous budget removes nothing; --budget 0 evicts every unused
    // entry; the referenced image and the sandbox stay.
    let none = h
        .run(&["cache", "prune", "--budget", "100G", "--json"])
        .await;
    assert_eq!(
        none.ok().json()["gc"]["removed"].as_array().unwrap().len(),
        0
    );
    let p = h.run(&["cache", "prune", "--budget", "0", "--json"]).await;
    let g = p.ok().json();
    assert_eq!(g["gc"]["removed"].as_array().unwrap().len(), 2, "{g}");
    assert!(!old.exists() && !build.exists());
    assert!(used.exists() && overlay.exists());

    // Config round trip; the environment wins over the file.
    h.run(&["cache", "config", "--budget", "12G", "--min-free", "2G"])
        .await
        .ok();
    let c = h.run(&["cache", "config", "--json"]).await.ok().json();
    assert_eq!(c["budget"], "12.0 GiB");
    assert_eq!(c["min_free"], 2u64 << 30);
    let file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(h.cua_home().join("cache.json")).unwrap()).unwrap();
    assert_eq!(file["budget"], "12G");
    h.set("CUA_CACHE_BUDGET", "off");
    let c = h.run(&["cache", "config", "--json"]).await.ok().json();
    assert_eq!(c["budget"], "off");
    assert_eq!(
        h.run(&["cache", "config", "--budget", "lots"]).await.code,
        2
    );
}

#[tokio::test]
async fn orphaned_ephemeral_sandboxes_are_reaped_on_the_next_cli_run() {
    let h = home();
    // A process that is gone.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let base = cached(&h, "cccc", 64, 1);
    let overlay = h.cua_home().join("vmm/qemu/cua-eph-deadbeef/disk.qcow2");
    std::fs::create_dir_all(overlay.parent().unwrap()).unwrap();
    cua_disk::qcow2::write_header(&overlay, Some(&base.join("disk.qcow2"))).unwrap();
    qemu_state(&h, "cua-eph-deadbeef", &overlay, now - 3600);
    let leases = h.cua_home().join("sandboxes/.ephemeral");
    std::fs::create_dir_all(&leases).unwrap();
    let lease = |name: &str, pid: u32, at: u64| {
        std::fs::write(
            leases.join(format!("{name}.json")),
            serde_json::json!({"name": name, "pid": pid, "created_at": at}).to_string(),
        )
        .unwrap();
    };
    lease("cua-eph-deadbeef", pid, now - 3600);
    // Too young to reap (the default delay is 10 minutes).
    lease("cua-eph-young", pid, now - 30);

    let out = h
        .run(&["--embedded", "sb", "ls", "--local", "--json"])
        .await;
    assert!(
        out.stderr
            .contains("removed orphaned sandbox cua-eph-deadbeef"),
        "stdout:\n{}\nstderr:\n{}",
        out.stdout,
        out.stderr
    );
    assert!(!overlay.parent().unwrap().exists(), "the VM disk is freed");
    assert!(!leases.join("cua-eph-deadbeef.json").exists());
    assert!(leases.join("cua-eph-young.json").exists());
    // The cached base image stays (cache, not sandbox state).
    assert!(base.exists());
}

#[tokio::test]
async fn low_disk_guard_fails_typed_with_a_prune_hint() {
    let mut h = home();
    h.set("CUA_DISK_FAKE_AVAILABLE", "1G");
    let disk = h.dir.path().join("base.qcow2");
    cua_disk::qcow2::write_header(&disk, None).unwrap();
    let image = format!("disk:{}", disk.display());
    let out = h
        .run(&[
            "--embedded",
            "sb",
            "create",
            &image,
            "--runtime",
            "qemu",
            "--name",
            "lowdisk",
        ])
        .await;
    assert_eq!(
        out.code, 7,
        "stdout:\n{}\nstderr:\n{}",
        out.stdout, out.stderr
    );
    assert!(out.stderr.contains("insufficient disk"), "{}", out.stderr);
    assert!(out.stderr.contains("cua cache prune"), "{}", out.stderr);
    // Nothing was created.
    assert!(!h.cua_home().join("vmm/qemu/lowdisk").exists());
}

#[tokio::test]
async fn sb_ls_shows_each_local_sandbox_size() {
    let h = home();
    // A stopped named QEMU sandbox with 256 KiB of disk.
    let disk = h.cua_home().join("vmm/qemu/sized/disk.qcow2");
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, vec![5u8; 256 << 10]).unwrap();
    qemu_state(&h, "sized", &disk, 1);
    let state = cua_sandbox_core::StateStore::new(h.cua_home().join("sandboxes"));
    state
        .save(&cua_sandbox_core::SandboxState::Local(
            cua_sandbox_core::LocalState {
                name: "sized".into(),
                runtime_type: "qemu".into(),
                host: "127.0.0.1".into(),
                status: "stopped".into(),
                created_at: cua_sandbox_core::state::python_utc_now(),
                ..Default::default()
            },
        ))
        .unwrap();

    let rows = h
        .run(&["--embedded", "sb", "ls", "--local", "--json"])
        .await
        .ok()
        .json();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "sized")
        .unwrap_or_else(|| panic!("{rows}"))
        .clone();
    assert!(row["size_bytes"].as_u64().unwrap() >= 256 << 10, "{row}");
    let table = h.run(&["--embedded", "sb", "ls", "--local"]).await;
    let text = table.ok().stdout.clone();
    assert!(text.contains("SIZE") && text.contains("KiB"), "{text}");
    let fast = h
        .run(&["--embedded", "sb", "ls", "--local", "--no-size"])
        .await;
    assert!(!fast.ok().stdout.contains("SIZE"), "{}", fast.stdout);
}
