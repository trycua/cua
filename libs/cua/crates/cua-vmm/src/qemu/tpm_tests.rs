use super::*;
use std::os::unix::fs::PermissionsExt;

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[tokio::test]
async fn supervisor_readiness_detachment_failure_and_cleanup() {
    for mode in [
        "shutdown",
        "crash",
        "qemu-failure",
        "tpm-failure",
        "timeout",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let socket = dir.join("swtpm.sock");
        let tpm = dir.join("swtpm");
        executable(
            &tpm,
            match mode {
                "tpm-failure" => "#!/bin/sh\nexit 1\n",
                "timeout" => "#!/bin/sh\nexec sleep 30\n",
                _ => {
                    "#!/usr/bin/env python3\nimport os,socket,time\ns=socket.socket(socket.AF_UNIX)\ns.bind(os.environ['CUA_TPM_SOCKET'])\ns.listen()\ntime.sleep(30)\n"
                }
            },
        );
        let qemu = dir.join("qemu");
        executable(
            &qemu,
            if mode == "crash" {
                "#!/bin/sh\n[ -S \"$CUA_TPM_SOCKET\" ] || exit 2\ntouch \"$CUA_TPM_DIR/started\"\nexec sleep 30\n"
            } else {
                "#!/bin/sh\n[ -S \"$CUA_TPM_SOCKET\" ] || exit 2\ntouch \"$CUA_TPM_DIR/started\"\nexit 1\n"
            },
        );
        let output = tokio::time::timeout(
            Duration::from_secs(4),
            tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("( /bin/sh -c \"$CUA_TPM_SCRIPT\" \"$0\" \"$@\" ) &")
                .arg(&qemu)
                .env(
                    "CUA_TPM_SCRIPT",
                    if mode == "timeout" {
                        include_str!("tpm.sh").replace("100", "3")
                    } else {
                        include_str!("tpm.sh").to_string()
                    },
                )
                .env("CUA_QEMU_LOG", dir.join("qemu.log"))
                .env("CUA_SWTPM", &tpm)
                .env("CUA_TPM_SOCKET", &socket)
                .env("CUA_TPM_DIR", dir)
                .env("CUA_TPM_PID", dir.join("swtpm.pid"))
                .env("CUA_TPM_LOG", dir.join("swtpm.log"))
                .output(),
        )
        .await
        .expect("handshake must not wait for VM shutdown")
        .unwrap();
        let pid = String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse::<u32>();
        if matches!(mode, "timeout" | "tpm-failure") {
            assert!(pid.is_err());
            assert!(!dir.join("started").exists());
        } else {
            let pid = pid.unwrap_or_else(|e| {
                panic!(
                    "{mode}: {e}; qemu={:?}; tpm={:?}",
                    std::fs::read_to_string(dir.join("qemu.log")),
                    std::fs::read_to_string(dir.join("swtpm.log"))
                )
            });
            if mode == "crash" {
                host::signal(pid, "KILL");
            }
        }
        for _ in 0..100 {
            if !dir.join("swtpm.pid").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!dir.join("swtpm.pid").exists(), "{mode}");
        assert!(!socket.exists(), "{mode}");
    }
}

#[test]
fn tpm_defaults_and_windows_arm_seed_rejection() {
    let spec = StartSpec::new("win", ImageSource::Existing)
        .os(GuestOs::Windows)
        .arch(Arch::Aarch64);
    assert!(!spec.tpm);
    let mut json = serde_json::to_value(&spec).unwrap();
    json.as_object_mut().unwrap().remove("tpm");
    assert!(!serde_json::from_value::<StartSpec>(json).unwrap().tpm);
    let mut seeded = spec;
    seeded.cloud_init_user_data = Some("data".into());
    assert!(crate::cloudinit::check_guest_support(&seeded).is_err());
}

#[tokio::test]
async fn windows_arm_minimums_fail_before_allocation() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = QemuRuntime::new(QemuConfig {
        root: tmp.path().into(),
        ..Default::default()
    });
    let spec = StartSpec::new("win", ImageSource::disk("/missing.ISO"))
        .os(GuestOs::Windows)
        .arch(Arch::Aarch64);
    assert!(
        runtime
            .start(&spec)
            .await
            .unwrap_err()
            .to_string()
            .contains("4096")
    );
    assert!(!tmp.path().join("win").exists());
    let runtime = QemuRuntime::new(QemuConfig {
        root: tmp.path().into(),
        preflight_tools: false,
        ..Default::default()
    });
    let spec = spec.memory_mb(4096);
    let error = runtime.start(&spec).await.unwrap_err();
    assert!(matches!(error, VmmError::Io(_)), "{error:?}");
    assert!(!tmp.path().join("win").exists());
}
