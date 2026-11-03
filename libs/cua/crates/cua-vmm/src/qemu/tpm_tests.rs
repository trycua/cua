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
    // A TPM launch without the VM lock is refused before anything is made.
    let error = runtime
        .launch_once(&mut st, &spec, Boot::default(), true, true)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid request: the TPM of 'vm' is only launched by a start that owns the VM"
    );
    assert!(!dir.join("tpm").exists());
    assert!(tpm_records(&dir).unwrap().is_empty());
    let owned = || async {
        Boot {
            owner: Some(runtime.own("vm").await),
            ..Default::default()
        }
    };
    if std::env::var("CUA_WIRING_MODE").unwrap() == "missing" {
        // A machine with a system-wide swtpm cannot exercise missing discovery.
        if host::which("swtpm").is_some() {
            return;
        }
        let error = runtime
            .launch_once(&mut st, &spec, owned().await, true, true)
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
            .launch_once(&mut st, &spec, owned().await, true, true)
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
        "socket",
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
        if tpm_dir.join("swtpm.sock").exists() && record_pid(record).ok().flatten().is_some() {
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
    let boot = Boot {
        owner: Some(runtime.own("vm").await),
        ..Default::default()
    };
    let launch = runtime.launch_once(&mut st, &spec, boot, true, true);
    // Early: cancelled while the launch shell runs. Late: cancelled after the
    // pid is persisted, while the launch waits for QMP.
    let late = std::env::var("CUA_CANCEL_LATE").is_ok();
    let wait = if late {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(100)
    };
    assert!(tokio::time::timeout(wait, launch).await.is_err());
    // The VM is released only once the cancelled launch's own cleanup is done.
    let _owner = tokio::time::timeout(Duration::from_secs(10), runtime.own("vm"))
        .await
        .expect("the VM is released");
    let tpm = dir.join("tpm");
    // Early cancellation can precede either stub's pid write, including when
    // the launch shell has already backgrounded QEMU. Late cancellation must
    // still exercise both processes after they started.
    for file in ["fake.pid", "qemu.pid"] {
        match std::fs::read_to_string(root.join(file)) {
            Ok(pid) => assert!(!host::pid_alive(pid.trim().parse().unwrap()), "{file}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !late => {}
            Err(e) => panic!("{file}: {e}"),
        }
    }
    // Check even when a stub never wrote its pid. Both stand-ins retain the
    // unique run directory in argv, including after the QEMU stub execs sleep.
    let ps = std::process::Command::new("ps")
        .args(["-ww", "-axo", "command="])
        .output()
        .unwrap();
    assert!(ps.status.success());
    let commands = String::from_utf8_lossy(&ps.stdout);
    let live: Vec<_> = commands
        .lines()
        .filter(|line| line.contains(root.to_str().unwrap()))
        .collect();
    assert!(live.is_empty(), "processes survived VM release: {live:?}");
    assert!(
        tpm_records(&dir).unwrap().is_empty(),
        "its record is removed"
    );

    drop(_owner);
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
    cancelled_launch(false);
}

/// Production guard ownership: cancelled after the pid is persisted.
#[test]
fn a_launch_cancelled_while_waiting_for_qmp_keeps_the_vm_until_cleanup() {
    cancelled_launch(true);
}

fn cancelled_launch(late: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let link = fake_swtpm_link(tmp.path());
    let sleeper = tmp.path().join("qemu-sleep");
    std::os::unix::fs::symlink(host::which("sleep").unwrap(), &sleeper).unwrap();
    let exe = |name: &str, body: String| {
        let path = bin.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    };
    exe(
        "qemu-system-aarch64",
        format!(
            "#!/bin/sh\necho $$ > {}\nexec {} 30\n",
            tmp.path().join("qemu.pid").display(),
            sleeper.display()
        ),
    );
    exe(
        "swtpm",
        format!(
            "#!/bin/sh\n{nap}for a; do case \"$a\" in file=*) f=\"${{a#file=}}\";; dir=*) d=\"${{a#dir=}}\";; esac; done\n\"{link}\" --exact qemu::tpm_tests::fake_swtpm --nocapture -- --pid \"file=$f\" --tpmstate \"dir=$d\" </dev/null >/dev/null 2>&1 &\nwhile [ ! -S \"$d/swtpm.sock\" ] || [ ! -s \"$f\" ]; do sleep 0.01; done\ncp \"$f\" {pid}\n",
            link = link.display(),
            pid = tmp.path().join("fake.pid").display(),
            nap = if late { "" } else { "sleep 0.5\n" }
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
        .envs(late.then_some(("CUA_CANCEL_LATE", "1")))
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
    assert_eq!(record_pid(&new_record).unwrap(), Some(new.id()));
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
    // The non-Linux path: the whole identity from the `ps` text.
    let cmd = host::pid_command(fake.id()).unwrap();
    let (f, d) = (
        format!("file={}", record.display()),
        format!("dir={}", tpm.display()),
    );
    assert_eq!(text_identity(&cmd, &f, &d), (true, true), "{cmd}");
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
async fn a_tpm_vm_cannot_be_claimed() {
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
}

/// Waits until a second caller is queued on VM `name`'s lock: the registry,
/// the holder's guard and the waiter's clone each hold the lock.
pub(super) async fn wait_queued(runtime: &QemuRuntime, name: &str) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let n = runtime
                .vm_locks
                .lock()
                .unwrap()
                .get(name)
                .map_or(0, Arc::strong_count);
            if n >= 3 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the caller queued");
}

/// B acquires the VM after A's start succeeded: it reads A's state, returns
/// that instance and touches neither A's TPM nor its record.
#[tokio::test]
async fn a_queued_start_returns_the_vm_its_predecessor_started() {
    let tmp = tempfile::tempdir().unwrap();
    let link = fake_swtpm_link(tmp.path());
    let runtime = Arc::new(QemuRuntime::new(QemuConfig {
        root: tmp.path().join("root"),
        vnc: false,
        allow_install: false,
        preflight_tools: false,
        ..Default::default()
    }));
    let dir = runtime.dir("vm");
    std::fs::create_dir_all(&dir).unwrap();
    let mut st = tpm_state("vm", &dir);
    runtime.save(&st).unwrap();
    let a = runtime.own("vm").await;
    let b = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .start(&StartSpec::new("vm", ImageSource::Existing))
                .await
        })
    };
    wait_queued(&runtime, "vm").await;
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    st.pid = Some(sleeper.id());
    runtime.save(&st).unwrap();
    let record = dir.join("swtpm.a.pid");
    let mut fake = spawn_fake(&link, &record, &dir.join("tpm"));
    drop(a);
    let inst = b.await.unwrap().unwrap();
    assert_eq!(inst.status, Status::Running);
    assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
    assert!(fake.try_wait().unwrap().is_none(), "A's TPM is alive");
    assert!(record.exists());
    assert_eq!(runtime.load("vm").unwrap().pid, Some(sleeper.id()));
    assert!(!dir.join("cmdline").exists(), "nothing was launched");
    fake.kill().unwrap();
    fake.wait().unwrap();
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
}

/// A dropped launch keeps the VM locked until its own cleanup has ended its
/// swtpm (here one that needs KILL).
#[tokio::test]
async fn a_dropped_launch_keeps_the_vm_until_its_cleanup_ends() {
    use futures::FutureExt;
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
    let owner = runtime.own("vm").await;
    let mut guard = LaunchGuard::new();
    guard.tpm = Some((record.clone(), tpm));
    guard.owner = Some(owner.clone());
    drop(guard);
    drop(owner);
    assert!(runtime.own("vm").now_or_never().is_none(), "still held");
    let _again = tokio::time::timeout(Duration::from_secs(30), runtime.own("vm"))
        .await
        .expect("released after the cleanup");
    assert_eq!(fake.wait().unwrap().signal(), Some(9));
    assert!(!record.exists());
}

/// The failure handler of `start` runs under the VM lock: when a waiting
/// caller gets the VM, the leftover only the VM-wide stop reaches is gone.
#[test]
fn failed_start_holds_the_vm_through_its_cleanup() {
    if Arch::host() != Arch::Aarch64 || host::HostOs::current() == host::HostOs::Windows {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // One spelling for every path: scripts rebuild paths from `$PWD`.
    let tmp_root = tmp.path().canonicalize().unwrap();
    let tmp = tmp_root.as_path();
    let bin = tmp.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let link = fake_swtpm_link(tmp);
    let exe = |name: &str, body: String| {
        let path = bin.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    };
    exe(
        "swtpm",
        format!(
            "#!/bin/sh\nfor a; do case \"$a\" in file=*) f=\"${{a#file=}}\";; dir=*) d=\"${{a#dir=}}\";; esac; done\n\"{link}\" --exact qemu::tpm_tests::fake_swtpm --nocapture -- --pid \"file=$f\" --tpmstate \"dir=$d\" </dev/null >/dev/null 2>&1 &\nwhile [ ! -S \"$d/swtpm.sock\" ] || [ ! -s \"$f\" ]; do sleep 0.01; done\ncp \"$f\" {pid}\n",
            link = link.display(),
            pid = tmp.join("fake.pid").display()
        ),
    );
    exe(
        "qemu-system-aarch64",
        format!(
            "#!/bin/sh\nrm -f \"$PWD/tpm/swtpm.sock\"\n(trap \"\" TERM; exec \"{link}\" socket --exact qemu::tpm_tests::fake_swtpm --nocapture -- --pid \"file=$PWD/swtpm.extra.pid\" --tpmstate \"dir=$PWD/tpm\" </dev/null >/dev/null 2>&1) &\nwhile [ ! -s \"$PWD/swtpm.extra.pid\" ]; do sleep 0.01; done\ncp \"$PWD/swtpm.extra.pid\" {pid}\nexit 1\n",
            link = link.display(),
            pid = tmp.join("extra.pid").display()
        ),
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "qemu::tpm_tests::failed_start_child",
            "--nocapture",
        ])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("CUA_FAILED_START_ROOT", tmp)
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
async fn failed_start_child() {
    let Ok(root) = std::env::var("CUA_FAILED_START_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let runtime = Arc::new(QemuRuntime::new(QemuConfig {
        root: root.join("state"),
        accel: Some("tcg".into()),
        vnc: false,
        allow_install: false,
        preflight_tools: false,
        density: crate::density::DensityConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    }));
    let dir = runtime.dir("vm");
    std::fs::create_dir_all(&dir).unwrap();
    let mut st = tpm_state("vm", &dir);
    st.arch = Arch::host();
    runtime.save(&st).unwrap();
    let mut spec = StartSpec::new("vm", ImageSource::Existing);
    spec.os = GuestOs::Windows;
    spec.arch = Some(Arch::host());
    let a = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.start(&spec).await })
    };
    for _ in 0..3000 {
        if dir.join("cmdline").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(dir.join("cmdline").exists());
    let extra = dir.join("swtpm.extra.pid");
    let b = {
        let (runtime, extra) = (runtime.clone(), extra.clone());
        tokio::spawn(async move {
            let _owner = runtime.own("vm").await;
            extra.exists()
        })
    };
    let e = a.await.unwrap().unwrap_err();
    assert!(
        matches!(&e, VmmError::Command { code: None, cmd, .. } if cmd.contains("cmdline")),
        "{e}"
    );
    assert!(!b.await.unwrap(), "the leftover was gone when B acquired");
    assert_eq!(runtime.load("vm").unwrap().pid, None);
    assert!(tpm_records(&dir).unwrap().is_empty());
    for f in ["fake.pid", "extra.pid"] {
        let pid: u32 = std::fs::read_to_string(root.join(f))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!host::pid_alive(pid), "{f}");
    }
}

/// A run directory that cannot be listed is an error everywhere, never "no
/// TPM": nothing is stopped, copied or removed.
#[tokio::test]
async fn an_unreadable_run_dir_stops_and_copies_nothing() {
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
    assert_eq!(
        tpm_records(&tmp.path().join("missing")).unwrap(),
        Vec::<PathBuf>::new()
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300)).unwrap();
    if std::fs::read_dir(&dir).is_ok() {
        // Root reads anything.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        fake.kill().unwrap();
        fake.wait().unwrap();
        return;
    }
    let mut st = tpm_state("vm", &dir);
    let dst = tmp.path().join("dst");
    let denied = |e: VmmError| {
        assert!(
            matches!(&e, VmmError::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied),
            "{e:?}"
        )
    };
    denied(tpm_records(&dir).unwrap_err());
    denied(runtime.stop_tpm(&st).await.unwrap_err());
    st.pid = None;
    denied(
        runtime
            .force_stop(&mut st, Duration::ZERO)
            .await
            .unwrap_err(),
    );
    denied(runtime.tpm_stopped(&st).unwrap_err());
    denied(runtime.copy_tpm(&st, &dst).unwrap_err());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!dst.join("tpm").exists());
    assert!(fake.try_wait().unwrap().is_none());
    assert!(record.exists());
    fake.kill().unwrap();
    fake.wait().unwrap();
}

#[tokio::test]
async fn an_unreadable_or_malformed_record_stops_and_copies_nothing() {
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
    let text = std::fs::read_to_string(&record).unwrap();
    let mut st = tpm_state("vm", &dir);
    st.pid = None;
    let dst = tmp.path().join("dst");
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = std::fs::read_to_string(&record).is_err();
    for malformed in [false, true] {
        if malformed {
            std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::write(&record, "garbage").unwrap();
        } else if !unreadable {
            continue; // root reads anything
        }
        assert!(runtime.stop_tpm(&st).await.is_err());
        assert!(runtime.force_stop(&mut st, Duration::ZERO).await.is_err());
        assert!(runtime.tpm_stopped(&st).is_err());
        assert!(runtime.copy_tpm(&st, &dst).is_err());
        assert!(!dst.join("tpm").exists());
        assert!(fake.try_wait().unwrap().is_none(), "swtpm survives");
        assert!(record.exists() && tpm.exists());
    }
    std::fs::write(&record, text).unwrap();
    fake.kill().unwrap();
    fake.wait().unwrap();
}
