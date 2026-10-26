use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn fake_swtpm() {
    let Ok(dir) = std::env::var("CUA_TPM_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(dir.join("swtpm.sock")).unwrap();
    std::fs::write(dir.join("../swtpm.pid"), std::process::id().to_string()).unwrap();
    std::thread::sleep(Duration::from_secs(30));
}

#[tokio::test]
async fn daemon_script_success_failure_and_force_stop() {
    for mode in ["success", "failure"] {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = QemuRuntime::new(QemuConfig {
            root: tmp.path().into(),
            ..Default::default()
        });
        let dir = runtime.dir("vm");
        std::fs::create_dir_all(dir.join("tpm")).unwrap();
        let tpm = dir.join("swtpm");
        let body = match mode {
            "failure" => "#!/bin/sh\nexit 1\n",
            _ => {
                "#!/bin/sh\n\"$CUA_FAKE_SWTPM\" --exact qemu::tpm_tests::fake_swtpm --nocapture \"dir=$CUA_TPM_DIR\" </dev/null >/dev/null 2>&1 &\nwhile [ ! -S \"$CUA_TPM_DIR/swtpm.sock\" ]; do sleep 0.01; done\n"
            }
        };
        std::fs::write(&tpm, body).unwrap();
        std::fs::set_permissions(&tpm, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = tokio::time::timeout(
            Duration::from_secs(4),
            tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(TPM_LAUNCH)
                .arg("/bin/sh")
                .args([
                    "-c",
                    "[ -S \"$CUA_TPM_DIR/swtpm.sock\" ] && touch \"$CUA_TPM_DIR/../started\"",
                ])
                .env("CUA_FAKE_SWTPM", fake_swtpm_link(tmp.path()))
                .env("CUA_SWTPM", &tpm)
                .env("CUA_TPM_DIR", dir.join("tpm"))
                .env("CUA_QEMU_LOG", dir.join("qemu.log"))
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        if mode == "success" {
            assert!(output.status.success());
            for _ in 0..100 {
                if dir.join("started").exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(dir.join("started").exists());
            // stop_tpm only needs the persisted instance name.
            let mut st: QemuState = serde_json::from_value(serde_json::json!({
                "name":"vm", "kind":"instance", "arch":"aarch64", "os":"windows",
                "disk":dir.join("disk"), "disk_format":"qcow2", "firmware":{"type":"bios"},
                "cpus":2, "memory_mb":4096, "created_at":0, "tpm":true
            }))
            .unwrap_or_else(|_| panic!("state fixture must deserialize"));
            let pid: u32 = std::fs::read_to_string(dir.join("swtpm.pid"))
                .unwrap()
                .parse()
                .unwrap();
            assert!(host::pid_alive(pid));
            st.pid = None;
            runtime.force_stop(&mut st, Duration::ZERO).await.unwrap();
            assert!(!dir.join("swtpm.pid").exists());
            assert!(!host::pid_alive(pid));
        } else {
            assert!(!output.status.success());
            assert!(!dir.join("started").exists());
        }
    }
}

#[tokio::test]
async fn launch_once_wiring_child() {
    let Ok(root) = std::env::var("CUA_WIRING_ROOT") else {
        return;
    };
    let runtime = QemuRuntime::new(QemuConfig {
        root: root.into(),
        vnc: false,
        allow_install: false,
        ..Default::default()
    });
    let dir = runtime.dir("vm");
    std::fs::create_dir_all(&dir).unwrap();
    let mut st: QemuState = serde_json::from_value(serde_json::json!({
        "name":"vm", "kind":"instance", "arch":"aarch64", "os":"windows",
        "disk":dir.join("disk"), "disk_format":"qcow2", "firmware":{"type":"bios"},
        "cpus":2, "memory_mb":4096, "created_at":0, "tpm":true
    }))
    .unwrap();
    let spec = StartSpec::new("vm", ImageSource::Existing);
    if std::env::var("CUA_WIRING_MODE").unwrap() == "missing" {
        // A machine with a system-wide swtpm cannot exercise missing discovery.
        if host::which("swtpm").is_some() {
            return;
        }
        let error = runtime
            .launch_once(&mut st, &spec, Boot::default(), true, true)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            VmmError::Missing { ref what, ref hint }
                if what == "swtpm" && hint.starts_with("install it (macOS: brew install swtpm;")
        ));
        assert!(!dir.join("tpm").exists());
    } else {
        let error = runtime
            .launch_once(&mut st, &spec, Boot::default(), true, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("swtpm.log"), "{error}");
        assert!(error.to_string().contains("could not launch"));
        let argv = std::fs::read_to_string(dir.join("cmdline")).unwrap();
        assert!(argv.contains(&format!(
            "socket,id=chrtpm,path={}",
            dir.join("tpm/swtpm.sock").display()
        )));
        assert!(argv.contains("tpm-tis-device,tpmdev=tpm0"));
    }
}

#[test]
fn launch_once_wiring() {
    for mode in ["missing", "persisted"] {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for name in if mode == "missing" {
            vec!["qemu-system-aarch64"]
        } else {
            vec!["qemu-system-aarch64", "swtpm"]
        } {
            let path = bin.join(name);
            std::fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "qemu::tpm_tests::launch_once_wiring_child",
                "--nocapture",
            ])
            .env("PATH", &bin)
            .env("CUA_WIRING_ROOT", tmp.path().join("state"))
            .env("CUA_WIRING_MODE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn tpm_state_defaults_off() {
    let st: QemuState = serde_json::from_value(serde_json::json!({
        "name":"vm", "kind":"instance", "arch":"aarch64", "os":"windows",
        "disk":"/d", "disk_format":"qcow2", "firmware":{"type":"bios"},
        "cpus":2, "memory_mb":4096, "created_at":0
    }))
    .unwrap();
    assert!(!st.tpm);
}

#[test]
fn tpm_launch_ends_with_qemu_launch() {
    assert!(TPM_LAUNCH.ends_with(QEMU_LAUNCH));
}

/// The test binary behind a symlink named `swtpm`, so its argv passes the
/// production identity check.
fn fake_swtpm_link(dir: &Path) -> PathBuf {
    let link = dir.join("swtpm");
    if !link.exists() {
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
    }
    link
}

/// A running fake swtpm serving `tpm_dir`.
fn spawn_fake(link: &Path, tpm_dir: &Path) -> std::process::Child {
    let mut child = std::process::Command::new(link)
        .args(["--exact", "qemu::tpm_tests::fake_swtpm", "--nocapture"])
        .arg(format!("dir={}", tpm_dir.display()))
        .env("CUA_TPM_DIR", tpm_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..500 {
        if tpm_dir.join("swtpm.sock").exists() {
            return child;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("fake swtpm did not start");
}

#[tokio::test]
async fn stop_tpm_leaves_other_processes() {
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let runtime = QemuRuntime::new(QemuConfig {
        root: tmp.path().join("root"),
        ..Default::default()
    });
    let dir = runtime.dir("vm");
    std::fs::create_dir_all(dir.join("tpm")).unwrap();
    let mut st: QemuState = serde_json::from_value(serde_json::json!({
        "name":"vm", "kind":"instance", "arch":"aarch64", "os":"windows",
        "disk":dir.join("disk"), "disk_format":"qcow2", "firmware":{"type":"bios"},
        "cpus":2, "memory_mb":4096, "created_at":0, "tpm":true
    }))
    .unwrap();
    let other = tmp.path().join("other/tpm");
    let prefix = dir.join("tpm-other");
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut elsewhere = spawn_fake(&link, &other);
    let mut prefixed = spawn_fake(&link, &prefix);
    let mut spaced = spawn_fake(&link, &dir.join("tpm backup"));
    for pid in [
        sleeper.id().to_string(),
        elsewhere.id().to_string(),
        prefixed.id().to_string(),
        spaced.id().to_string(),
        "not-a-pid".into(),
        "0".into(),
        "1".into(),
    ] {
        std::fs::write(dir.join("swtpm.pid"), &pid).unwrap();
        st.pid = None;
        runtime.force_stop(&mut st, Duration::ZERO).await.unwrap();
        assert!(!dir.join("swtpm.pid").exists(), "{pid}");
    }
    for child in [&mut sleeper, &mut elsewhere, &mut prefixed, &mut spaced] {
        assert!(child.try_wait().unwrap().is_none(), "survives");
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

#[tokio::test]
async fn abandoned_launch_stops_swtpm() {
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let tpm = tmp.path().join("tpm");
    std::fs::create_dir_all(&tpm).unwrap();
    let mut fake = spawn_fake(&link, &tpm);
    let mut guard = LaunchGuard::new(tmp.path().join("qemu.pid"));
    guard.tpm_dir = Some(tmp.path().to_path_buf());
    // Written as the fake does; the guard finds no QEMU, then stops swtpm.
    std::fs::write(tmp.path().join("swtpm.pid"), fake.id().to_string()).unwrap();
    drop(guard);
    assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
    assert!(fake.wait().is_ok());
    assert!(!tmp.path().join("swtpm.pid").exists());
}
