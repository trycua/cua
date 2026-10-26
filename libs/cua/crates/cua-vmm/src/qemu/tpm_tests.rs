use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn fake_swtpm() {
    let Ok(dir) = std::env::var("CUA_TPM_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
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
                "#!/bin/sh\n\"$CUA_TEST_BIN\" --exact qemu::tpm_tests::fake_swtpm --nocapture </dev/null >/dev/null 2>&1 &\nwhile [ ! -S \"$CUA_TPM_DIR/swtpm.sock\" ]; do sleep 0.01; done\n"
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
                .env("CUA_TEST_BIN", std::env::current_exe().unwrap())
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
                "disk":"disk", "disk_format":"qcow2", "firmware":{"type":"bios"},
                "cpus":2, "memory_mb":4096, "created_at":0
            }))
            .unwrap_or_else(|_| panic!("state fixture must deserialize"));
            assert!(dir.join("swtpm.pid").exists());
            st.pid = None;
            runtime.force_stop(&mut st, Duration::ZERO).await.unwrap();
            assert!(!dir.join("swtpm.pid").exists());
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
        "cpus":2, "memory_mb":4096, "created_at":0
    }))
    .unwrap();
    let mut spec = StartSpec::new("vm", ImageSource::Existing);
    if std::env::var("CUA_WIRING_MODE").unwrap() == "missing" {
        // A machine with a system-wide swtpm cannot exercise missing discovery.
        if host::which("swtpm").is_some() {
            return;
        }
        spec.tpm = true;
        let error = runtime
            .launch_once(&mut st, &spec, Boot::default(), true, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("swtpm is not available"));
        assert!(!dir.join("tpm").exists());
    } else {
        std::fs::create_dir(dir.join("tpm")).unwrap();
        assert!(!spec.tpm);
        let error = runtime
            .launch_once(&mut st, &spec, Boot::default(), true, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("swtpm.log"));
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
fn tpm_defaults() {
    let spec = StartSpec::new("vm", ImageSource::Existing);
    assert!(!spec.tpm);
    let mut json = serde_json::to_value(spec).unwrap();
    json.as_object_mut().unwrap().remove("tpm");
    assert!(!serde_json::from_value::<StartSpec>(json).unwrap().tpm);
}
