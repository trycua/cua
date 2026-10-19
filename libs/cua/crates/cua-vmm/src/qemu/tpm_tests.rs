use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn fake_swtpm() {
    let Ok(socket) = std::env::var("CUA_TPM_SOCKET") else {
        return;
    };
    let _listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
    std::fs::write(
        std::env::var("CUA_TPM_PID").unwrap(),
        std::process::id().to_string(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(30));
}

#[tokio::test]
async fn readiness_failure_and_stop_tpm() {
    for mode in ["ready", "failure", "timeout"] {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = QemuRuntime::new(QemuConfig {
            root: tmp.path().into(),
            ..Default::default()
        });
        let dir = runtime.dir("vm");
        std::fs::create_dir_all(&dir).unwrap();
        let tpm = dir.join("swtpm");
        let body = match mode {
            "failure" => "#!/bin/sh\nexit 1\n",
            "timeout" => "#!/bin/sh\nexec sleep 30\n",
            _ => {
                "#!/bin/sh\nexec \"$CUA_TEST_BIN\" --exact qemu::tpm_tests::fake_swtpm --nocapture\n"
            }
        };
        std::fs::write(&tpm, body).unwrap();
        std::fs::set_permissions(&tpm, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = tokio::time::timeout(
            Duration::from_secs(4),
            tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(TPM_LAUNCH.replace("100", "3"))
                .arg("/bin/sh")
                .args([
                    "-c",
                    "[ -S \"$CUA_TPM_SOCKET\" ] && touch \"$CUA_TPM_DIR/started\"",
                ])
                .env("CUA_TEST_BIN", std::env::current_exe().unwrap())
                .env("CUA_SWTPM", &tpm)
                .env("CUA_TPM_DIR", &dir)
                .env("CUA_TPM_SOCKET", dir.join("swtpm.sock"))
                .env("CUA_TPM_PID", dir.join("swtpm.pid"))
                .env("CUA_TPM_LOG", dir.join("swtpm.log"))
                .env("CUA_QEMU_LOG", dir.join("qemu.log"))
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        if mode == "ready" {
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
            runtime.stop_tpm(&st);
            assert!(!dir.join("swtpm.pid").exists());
            st.pid = None;
            runtime.force_stop(&mut st, Duration::ZERO).await.unwrap();
        } else {
            assert!(!output.status.success());
            assert!(!dir.join("started").exists());
        }
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
