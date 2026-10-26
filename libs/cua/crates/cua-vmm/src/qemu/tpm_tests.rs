use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn fake_swtpm() {
    // Runs only with swtpm's own options, which a normal test run never has.
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--tpmstate") {
        return;
    }
    let arg = |key: &str| {
        args.iter()
            .find_map(|a| a.strip_prefix(key))
            .map(PathBuf::from)
            .unwrap()
    };
    let (record, dir) = (arg("file="), arg("dir="));
    std::fs::create_dir_all(&dir).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(dir.join("swtpm.sock")).unwrap();
    std::fs::write(record, std::process::id().to_string()).unwrap();
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
                "#!/bin/sh\nfor a; do case \"$a\" in file=*) f=\"${a#file=}\";; dir=*) d=\"${a#dir=}\";; esac; done\n\"$CUA_FAKE_SWTPM\" --exact qemu::tpm_tests::fake_swtpm --nocapture -- --pid \"file=$f\" --tpmstate \"dir=$d\" </dev/null >/dev/null 2>&1 &\nwhile [ ! -S \"$d/swtpm.sock\" ] || [ ! -s \"$f\" ]; do sleep 0.01; done\n"
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
                .env("CUA_TPM_RECORD", dir.join("swtpm.test.pid"))
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
            let pid: u32 = std::fs::read_to_string(dir.join("swtpm.test.pid"))
                .unwrap()
                .parse()
                .unwrap();
            assert!(host::pid_alive(pid));
            st.pid = None;
            runtime.force_stop(&mut st, Duration::ZERO).await.unwrap();
            assert!(!dir.join("swtpm.test.pid").exists());
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
pub(super) fn fake_swtpm_link(dir: &Path) -> PathBuf {
    let link = dir.join("swtpm");
    if !link.exists() {
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
    }
    link
}

/// The arguments a fake swtpm runs with: they satisfy the `/proc` check and
/// the `ps`-text check (which must end with the record and directory).
fn fake_args(record: &Path, tpm_dir: &Path) -> Vec<String> {
    [
        "--exact",
        "qemu::tpm_tests::fake_swtpm",
        "--nocapture",
        "--",
        "--pid",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([
        format!("file={}", record.display()),
        "--tpmstate".into(),
        format!("dir={}", tpm_dir.display()),
    ])
    .collect()
}

fn wait_started(child: &mut std::process::Child, record: &Path, tpm_dir: &Path) {
    for _ in 0..500 {
        if tpm_dir.join("swtpm.sock").exists() && record_pid(record).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("fake swtpm did not start");
}

/// A running fake swtpm serving `tpm_dir`, recorded in `record`.
pub(super) fn spawn_fake(link: &Path, record: &Path, tpm_dir: &Path) -> std::process::Child {
    let mut child = std::process::Command::new(link)
        .args(fake_args(record, tpm_dir))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_started(&mut child, record, tpm_dir);
    child
}

/// The same, with TERM ignored (an ignored signal survives `exec`).
fn spawn_fake_ignoring_term(link: &Path, record: &Path, tpm_dir: &Path) -> std::process::Child {
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "trap \"\" TERM; exec \"$0\" \"$@\""])
        .arg(link)
        .args(fake_args(record, tpm_dir))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_started(&mut child, record, tpm_dir);
    child
}

pub(super) fn tpm_state(name: &str, dir: &Path) -> QemuState {
    serde_json::from_value(serde_json::json!({
        "name":name, "kind":"instance", "arch":"aarch64", "os":"windows",
        "disk":dir.join("disk"), "disk_format":"qcow2", "firmware":{"type":"bios"},
        "cpus":2, "memory_mb":4096, "created_at":0, "tpm":true
    }))
    .unwrap()
}

#[test]
fn fake_matches_the_identity_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let (record, tpm) = (tmp.path().join("swtpm.a.pid"), tmp.path().join("tpm"));
    let mut fake = spawn_fake(&link, &record, &tpm);
    assert_eq!(swtpm_state(fake.id(), &record, &tpm), Tpm::Ours);
    // The macOS rule, on the `ps` text.
    let cmd = host::pid_command(fake.id()).unwrap();
    assert!(names_dir_text(
        &cmd,
        &format!("file={}", record.display()),
        &format!("dir={}", tpm.display())
    ));
    fake.kill().unwrap();
    fake.wait().unwrap();
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
    let mut st = tpm_state("vm", &dir);
    let other = tmp.path().join("other/tpm");
    let prefix = dir.join("tpm-other");
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut elsewhere = spawn_fake(&link, &tmp.path().join("r1.pid"), &other);
    let mut prefixed = spawn_fake(&link, &tmp.path().join("r2.pid"), &prefix);
    let mut spaced = spawn_fake(&link, &tmp.path().join("r3.pid"), &dir.join("tpm backup"));
    // The same directory, another launch's record.
    let mut sibling = spawn_fake(&link, &tmp.path().join("r4.pid"), &dir.join("tpm"));
    for pid in [
        sleeper.id().to_string(),
        elsewhere.id().to_string(),
        prefixed.id().to_string(),
        spaced.id().to_string(),
        sibling.id().to_string(),
        "0".into(),
        "1".into(),
    ] {
        let record = dir.join("swtpm.mine.pid");
        std::fs::write(&record, &pid).unwrap();
        st.pid = None;
        runtime.force_stop(&mut st, Duration::ZERO).await.unwrap();
        assert!(!record.exists(), "{pid}");
    }
    for child in [
        &mut sleeper,
        &mut elsewhere,
        &mut prefixed,
        &mut spaced,
        &mut sibling,
    ] {
        assert!(child.try_wait().unwrap().is_none(), "survives");
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

#[tokio::test]
async fn stop_tpm_kills_a_swtpm_that_ignores_term() {
    use std::os::unix::process::ExitStatusExt;
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let runtime = QemuRuntime::new(QemuConfig {
        root: tmp.path().join("root"),
        ..Default::default()
    });
    let dir = runtime.dir("vm");
    let tpm = dir.join("tpm");
    std::fs::create_dir_all(&tpm).unwrap();
    let record = dir.join("swtpm.a.pid");
    let mut fake = spawn_fake_ignoring_term(&link, &record, &tpm);
    let st = tpm_state("vm", &dir);
    runtime.stop_tpm(&st).await.unwrap();
    assert!(!record.exists());
    assert_eq!(fake.wait().unwrap().signal(), Some(9));
}

#[test]
fn copy_refused_until_the_tpm_exits() {
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let runtime = QemuRuntime::new(QemuConfig {
        root: tmp.path().join("root"),
        ..Default::default()
    });
    let dir = runtime.dir("vm");
    let tpm = dir.join("tpm");
    std::fs::create_dir_all(&tpm).unwrap();
    let record = dir.join("swtpm.a.pid");
    let mut fake = spawn_fake(&link, &record, &tpm);
    let st = tpm_state("vm", &dir);
    let dst = tmp.path().join("dst");
    for _ in 0..2 {
        let e = runtime.copy_tpm(&st, &dst).unwrap_err().to_string();
        assert!(e.contains("still running"), "{e}");
        assert!(record.exists());
        assert!(!dst.join("tpm").exists());
    }
    fake.kill().unwrap();
    fake.wait().unwrap();
    runtime.copy_tpm(&st, &dst).unwrap();
    assert!(dst.join("tpm").exists());
}

/// Stand-ins for `swtpm` (starts the fake after a delay) and QEMU (sleeps)
/// on a PATH of their own.
#[tokio::test]
async fn cancelled_launch_child() {
    let Ok(root) = std::env::var("CUA_CANCEL_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let runtime = QemuRuntime::new(QemuConfig {
        root: root.join("state"),
        vnc: false,
        allow_install: false,
        ..Default::default()
    });
    let dir = runtime.dir("vm");
    std::fs::create_dir_all(&dir).unwrap();
    let mut st = tpm_state("vm", &dir);
    let spec = StartSpec::new("vm", ImageSource::Existing);
    let launch = runtime.launch_once(&mut st, &spec, Boot::default(), true, true);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), launch)
            .await
            .is_err()
    );
    assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
    let tpm = dir.join("tpm");
    let old = std::fs::read_to_string(root.join("fake.pid"))
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    assert!(!host::pid_alive(old), "the cancelled swtpm is gone");
    let qemu: u32 = std::fs::read_to_string(root.join("qemu.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!host::pid_alive(qemu), "the cancelled QEMU is gone");
    assert!(tpm_records(&dir).is_empty(), "its record is removed");
    // A replacement on the same directory is not touched by late cleanup.
    let _ = std::fs::remove_file(tpm.join("swtpm.sock"));
    let link = fake_swtpm_link(&root);
    let mut new = spawn_fake(&link, &dir.join("swtpm.new.pid"), &tpm);
    assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(new.try_wait().unwrap().is_none());
    new.kill().unwrap();
    new.wait().unwrap();
}

#[test]
fn cancelled_launch_cleans_up_before_settle_returns() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let link = fake_swtpm_link(tmp.path());
    let exe = |name: &str, body: String| {
        let path = bin.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    };
    exe(
        "qemu-system-aarch64",
        format!(
            "#!/bin/sh\necho $$ > {}\nexec sleep 30\n",
            tmp.path().join("qemu.pid").display()
        ),
    );
    exe(
        "swtpm",
        format!(
            "#!/bin/sh\nsleep 0.5\nfor a; do case \"$a\" in file=*) f=\"${{a#file=}}\";; dir=*) d=\"${{a#dir=}}\";; esac; done\n\"{link}\" --exact qemu::tpm_tests::fake_swtpm --nocapture -- --pid \"file=$f\" --tpmstate \"dir=$d\" </dev/null >/dev/null 2>&1 &\nwhile [ ! -S \"$d/swtpm.sock\" ] || [ ! -s \"$f\" ]; do sleep 0.01; done\ncp \"$f\" {pid}\n",
            link = link.display(),
            pid = tmp.path().join("fake.pid").display()
        ),
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "qemu::tpm_tests::cancelled_launch_child",
            "--nocapture",
        ])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("CUA_CANCEL_ROOT", tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn a_guard_without_pids_touches_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let tpm = tmp.path().join("tpm");
    let mut fake = spawn_fake(&link, &tmp.path().join("swtpm.a.pid"), &tpm);
    for f in ["swtpm.pid", "qemu.pid"] {
        std::fs::write(tmp.path().join(f), fake.id().to_string()).unwrap();
    }
    drop(LaunchGuard::new());
    assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
    assert!(fake.try_wait().unwrap().is_none());
    fake.kill().unwrap();
    fake.wait().unwrap();
}

#[tokio::test]
async fn abandoned_launch_stops_swtpm() {
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let tpm = tmp.path().join("tpm");
    let record = tmp.path().join("swtpm.a.pid");
    let mut fake = spawn_fake(&link, &record, &tpm);
    let mut guard = LaunchGuard::new();
    guard.tpm = Some((record.clone(), tpm));
    drop(guard);
    assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
    assert!(fake.wait().is_ok());
    assert!(!record.exists());
}

#[tokio::test]
async fn late_cleanup_leaves_the_replacement_swtpm() {
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let tpm = tmp.path().join("tpm");
    let record = tmp.path().join("swtpm.old.pid");
    let mut old = spawn_fake(&link, &record, &tpm);
    let mut guard = LaunchGuard::new();
    guard.tpm = Some((record.clone(), tpm.clone()));
    drop(guard);
    // A replacement takes over the directory under its own record.
    std::fs::remove_file(tpm.join("swtpm.sock")).unwrap();
    let new_record = tmp.path().join("swtpm.new.pid");
    let mut new = spawn_fake(&link, &new_record, &tpm);
    assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
    old.wait().unwrap();
    assert!(new.try_wait().unwrap().is_none(), "replacement survives");
    assert_eq!(record_pid(&new_record), Some(new.id()));
    new.kill().unwrap();
    new.wait().unwrap();
}

#[test]
fn text_fallback_requires_the_exact_last_directory() {
    let (f, n) = ("file=/vm/swtpm.a.pid", "dir=/vm/tpm");
    assert!(names_dir_text(
        "/bin/swtpm socket --tpm2 --pid file=/vm/swtpm.a.pid --tpmstate dir=/vm/tpm",
        f,
        n
    ));
    assert!(!names_dir_text(
        "/bin/swtpm socket --pid file=/vm/swtpm.a.pid --tpmstate dir=/vm/tpm -backup",
        f,
        n
    ));
    assert!(!names_dir_text(
        "/bin/swtpm socket --pid file=/vm/swtpm.a.pid --tpmstate dir=/vm/tpm backup",
        f,
        n
    ));
    assert!(!names_dir_text(
        "/bin/swtpm socket --pid file=/vm/swtpm.b.pid --tpmstate dir=/vm/tpm",
        f,
        n
    ));
}

#[test]
fn identity_survives_a_spaced_executable_and_the_legacy_pidfile() {
    let tmp = tempfile::tempdir().unwrap();
    let spaced = tmp.path().join("bin dir");
    std::fs::create_dir(&spaced).unwrap();
    let link = fake_swtpm_link(&spaced);
    let (run, tpm) = (tmp.path().join("run"), tmp.path().join("run/tpm"));
    std::fs::create_dir_all(&run).unwrap();
    let record = run.join("swtpm.a.pid");
    let mut fake = spawn_fake(&link, &record, &tpm);
    assert_eq!(swtpm_state(fake.id(), &record, &tpm), Tpm::Ours);
    fake.kill().unwrap();
    fake.wait().unwrap();
    // The previous launch script's spelling of the shared pidfile.
    let link = fake_swtpm_link(tmp.path());
    let (run, tpm) = (tmp.path().join("run2"), tmp.path().join("run2/tpm"));
    std::fs::create_dir_all(&run).unwrap();
    let old = tpm.join("../swtpm.pid");
    let mut fake = spawn_fake(&link, &old, &tpm);
    assert_eq!(
        swtpm_state(fake.id(), &run.join("swtpm.pid"), &tpm),
        Tpm::Ours
    );
    fake.kill().unwrap();
    fake.wait().unwrap();
}

#[tokio::test]
async fn a_tpm_vm_cannot_be_claimed_and_a_busy_tpm_refuses_a_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = QemuRuntime::new(QemuConfig {
        root: tmp.path().join("root"),
        ..Default::default()
    });
    let dir = runtime.dir("vm");
    std::fs::create_dir_all(dir.join("tpm")).unwrap();
    let mut st = tpm_state("vm", &dir);
    st.pid = Some(std::process::id());
    runtime.save(&st).unwrap();
    let id = crate::types::ClaimIdentity::default();
    let e = runtime
        .claim("vm", "vm2", &id)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("cannot be claimed"), "{e}");
    assert!(dir.join("tpm").exists() && !runtime.dir("vm2").exists());
    // A launch in flight holds the TPM: a copy is refused.
    let busy = runtime.tpm_claim(&st, false).await.unwrap();
    let e = runtime
        .tpm_claim(&st, false)
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("being started or copied"), "{e}");
    drop(busy);
    assert!(runtime.tpm_claim(&st, false).await.is_ok());
}
