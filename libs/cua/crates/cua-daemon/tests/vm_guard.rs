//! VMs outlive the daemon that started them (QEMU is detached on purpose),
//! so a daemon that dies must not leave VMs nobody uses. These tests run the
//! daemon's guard against stand-in QEMU processes: a shell script named
//! `qemu-system-x86_64` that writes its own pid file and sleeps, started the
//! way the runtime starts QEMU (detached, re-parented to init) from a layout
//! identical to a real one (`<home>/vmm/qemu/<name>/{state.json,qemu.pid}`).
//! Nothing but the test's own temp directories and children is touched, and
//! every test judges only the VMs under its own temp home.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use cua_daemon::guard::{self, Policy, Reason};
use cua_vmm::hostvms::{self, HostVm};

/// A temp tree with the stand-in QEMU binary.
struct Host {
    dir: tempfile::TempDir,
    started: Vec<u32>,
}

const FAKE_QEMU: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in -pidfile) pf="$2";; esac
  shift
done
echo $$ > "$pf"
while :; do sleep 1; done
"#;

impl Host {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let qemu = bin.join("qemu-system-x86_64");
        std::fs::write(&qemu, FAKE_QEMU).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&qemu, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            dir,
            started: vec![],
        }
    }

    fn home(&self, name: &str) -> PathBuf {
        let h = self.dir.path().join(name);
        std::fs::create_dir_all(&h).unwrap();
        h
    }

    /// Starts a VM the way the runtime does and waits for its pid file.
    fn vm(&mut self, home: &Path, name: &str) -> u32 {
        let vmdir = home.join("vmm/qemu").join(name);
        std::fs::create_dir_all(&vmdir).unwrap();
        let qemu = self.dir.path().join("bin/qemu-system-x86_64");
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg("\"$0\" \"$@\" </dev/null >/dev/null 2>&1 & echo $!")
            .arg("/bin/sh")
            .arg(&qemu)
            .args(["-name", name, "-pidfile"])
            .arg(vmdir.join("qemu.pid"))
            .output()
            .unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        let state = serde_json::json!({
            "name": name, "kind": "instance", "arch": "x86_64", "os": "linux",
            "disk": vmdir.join("disk.qcow2"), "disk_format": "qcow2",
            "firmware": serde_json::to_value(cua_vmm::qemu::Firmware::Bios).unwrap(), "cpus": 2, "memory_mb": 4096, "created_at": 0, "pid": pid,
        });
        std::fs::write(vmdir.join("state.json"), state.to_string()).unwrap();
        std::fs::write(vmdir.join("disk.qcow2"), b"overlay").unwrap();
        self.started.push(pid);
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::read_to_string(vmdir.join("qemu.pid"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            != Some(pid)
        {
            assert!(Instant::now() < deadline, "the stand-in QEMU did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        pid
    }

    /// Records `name` as a Space of `home` (the sandbox state file).
    fn record(&self, home: &Path, name: &str) {
        let d = home.join("sandboxes");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{name}.json")), "{}").unwrap();
        // And its row in the Spaces registry, as a create writes it.
        cua_spaces::registry::Registry::new(home)
            .upsert(
                cua_proto::daemon::v1::Space {
                    id: format!("local:{name}"),
                    name: name.to_string(),
                    ..Default::default()
                },
                Default::default(),
            )
            .unwrap();
    }

    fn record_pool_member(&self, home: &Path, name: &str) {
        let d = home.join("sandboxes/.pool");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{name}.json")), "{}").unwrap();
    }

    fn lifetime(&self, home: &Path, name: &str, expires_at_unix: u64, on_expire: &str) {
        let mut all: serde_json::Map<String, serde_json::Value> =
            std::fs::read(home.join("space-lifetimes.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
        all.insert(
            format!("local:{name}"),
            serde_json::json!({"expires_at_unix": expires_at_unix, "on_expire": on_expire}),
        );
        std::fs::write(
            home.join("space-lifetimes.json"),
            serde_json::to_vec(&all).unwrap(),
        )
        .unwrap();
    }

    /// Who started the VM: a process that is gone, or one that is running.
    fn starter(&self, home: &Path, name: &str, pid: u32) {
        cua_vmm::hostvms::Launcher { pid, at: 0 }.write(&home.join("vmm/qemu").join(name));
    }

    /// The Cua VMs under this host's temp tree only.
    fn vms(&self) -> Vec<HostVm> {
        hostvms::host_vms()
            .into_iter()
            .filter(|v| v.root.starts_with(self.dir.path()))
            .collect()
    }

    fn running(&self, pid: u32) -> bool {
        self.vms().iter().any(|v| v.pid == pid)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        for pid in &self.started {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
}

/// A pid that belonged to a process that has exited.
fn dead_pid() -> u32 {
    let mut c = Command::new("true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

/// Judges at once (no waiting for ages).
fn now_policy() -> Policy {
    Policy {
        starter_gone: Duration::ZERO,
        unknown_starter: Duration::from_secs(3600),
        abandoned_warm: Duration::ZERO,
        create_max: Duration::from_secs(3600),
        recorder_max: Duration::from_secs(3600),
    }
}

fn unix_now() -> u64 {
    cua_vmm::host::now_secs()
}

async fn sweep(h: &Host, home: &Path, serving: bool, dry: bool) -> Vec<guard::Ended> {
    guard::sweep_vms(home, serving, h.vms(), vec![], &now_policy(), dry).await
}

fn names(e: &[guard::Ended]) -> Vec<String> {
    let mut n: Vec<_> = e.iter().map(|e| e.name.clone()).collect();
    n.sort();
    n
}

#[tokio::test]
async fn a_restarted_daemon_adopts_its_spaces_and_ends_the_rest() {
    let mut h = Host::new();
    let home = h.home("home");
    // The daemon that made these is gone (it is not running in this test).
    let kept = h.vm(&home, "web");
    h.record(&home, "web");
    h.starter(&home, "web", dead_pid());

    let expired = h.vm(&home, "old");
    h.record(&home, "old");
    h.lifetime(&home, "old", unix_now() - 3600, "delete");
    h.starter(&home, "old", dead_pid());

    let unexpired = h.vm(&home, "fresh");
    h.record(&home, "fresh");
    h.lifetime(&home, "fresh", unix_now() + 3600, "delete");

    let orphan = h.vm(&home, "lost");
    h.starter(&home, "lost", dead_pid());

    let warm = h.vm(&home, "cua-pool-aaaa");
    h.record_pool_member(&home, "cua-pool-aaaa");
    h.starter(&home, "cua-pool-aaaa", dead_pid());

    // Mid create: nothing records it yet, but whatever started it is alive.
    let creating = h.vm(&home, "creating");
    h.starter(&home, "creating", std::process::id());

    // A dry run says what it would do and does nothing.
    let dry = sweep(&h, &home, true, true).await;
    assert_eq!(
        names(&dry),
        ["lost", "old"],
        "the pool member is the pool's while a daemon runs"
    );
    assert!(h.running(orphan) && h.running(expired));

    // The restarted daemon (serving the home) sweeps.
    let ended = sweep(&h, &home, true, false).await;
    // A pool member is left to the running daemon's pool (it adopts it).
    assert_eq!(names(&ended), ["lost", "old"]);
    assert!(ended.iter().all(|e| e.error.is_none()), "{ended:?}");
    let reason = |n: &str| ended.iter().find(|e| e.name == n).unwrap().reason.clone();
    assert_eq!(reason("old"), Reason::Expired);
    assert_eq!(reason("lost"), Reason::Unrecorded);

    assert!(!h.running(expired) && !h.running(orphan));
    assert!(h.running(kept), "a recorded Space is adopted, not ended");
    assert!(h.running(unexpired));
    assert!(h.running(warm), "the running daemon's pool adopts it");
    assert!(h.running(creating), "a create in progress is never taken");
    // Their disks and records go with them; the rest stay.
    assert!(!home.join("vmm/qemu/old").exists());
    assert!(!home.join("sandboxes/old.json").exists());
    assert!(
        !std::fs::read_to_string(home.join("spaces.json"))
            .unwrap()
            .contains("local:old"),
        "the Space's registry row goes with its VM"
    );
    assert!(
        std::fs::read_to_string(home.join("spaces.json"))
            .unwrap()
            .contains("local:web")
    );
    assert!(!home.join("vmm/qemu/lost").exists());
    assert!(home.join("vmm/qemu/web/disk.qcow2").exists());
    assert!(home.join("sandboxes/web.json").exists());
}

#[tokio::test]
async fn expired_lifetimes_are_enforced_with_no_daemon_at_all() {
    let mut h = Host::new();
    let home = h.home("home");
    let gone = h.vm(&home, "a");
    h.record(&home, "a");
    h.lifetime(&home, "a", unix_now() - 1, "delete");
    let paused = h.vm(&home, "b");
    h.record(&home, "b");
    h.lifetime(&home, "b", unix_now() - 1, "suspend");
    let warm = h.vm(&home, "cua-pool-bbbb");
    h.record_pool_member(&home, "cua-pool-bbbb");
    h.starter(&home, "cua-pool-bbbb", dead_pid());
    let mine = h.vm(&home, "c");
    h.record(&home, "c");

    let ended = sweep(&h, &home, false, false).await;
    assert_eq!(names(&ended), ["a", "b", "cua-pool-bbbb"]);
    assert!(ended.iter().all(|e| e.error.is_none()), "{ended:?}");
    assert!(!h.running(gone));
    assert!(!h.running(paused), "suspend turns the VM off");
    assert!(
        home.join("vmm/qemu/b/disk.qcow2").exists() && home.join("sandboxes/b.json").exists(),
        "and keeps its disk and record, so start brings it back"
    );
    assert!(!h.running(warm), "warm members of a daemon that is gone go");
    assert!(h.running(mine), "an unexpired recorded Space stays up");
}

#[tokio::test]
async fn one_homes_sweep_never_reaches_another_homes_vms() {
    let mut h = Host::new();
    let a = h.home("a");
    let b = h.home("b");
    // Both homes hold an orphan and an expired Space.
    let a_orphan = h.vm(&a, "orphan");
    h.starter(&a, "orphan", dead_pid());
    let b_orphan = h.vm(&b, "orphan");
    h.starter(&b, "orphan", dead_pid());
    let b_expired = h.vm(&b, "old");
    h.record(&b, "old");
    h.lifetime(&b, "old", unix_now() - 1, "delete");

    // Home a's daemon sweeps its own home only.
    let ended = guard::sweep_vms(
        &a,
        true,
        h.vms().into_iter().filter(|v| v.home() == a).collect(),
        vec![],
        &now_policy(),
        false,
    )
    .await;
    assert_eq!(names(&ended), ["orphan"]);
    assert!(!h.running(a_orphan));
    assert!(
        h.running(b_orphan) && h.running(b_expired),
        "b is untouched"
    );
    assert!(b.join("vmm/qemu/orphan").exists());
}

#[tokio::test]
async fn a_home_with_a_live_daemon_is_left_to_that_daemon() {
    let mut h = Host::new();
    let a = h.home("a");
    let b = h.home("b");
    let b_orphan = h.vm(&b, "orphan");
    h.starter(&b, "orphan", dead_pid());
    // A daemon listening for home b.
    let sock = b.join("cua.sock");
    let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    cua_daemon::Discovery {
        pid: std::process::id(),
        socket_path: Some(sock.display().to_string()),
        loopback_url: None,
        token: Some("t".into()),
        version: "0".into(),
    }
    .write(&b.join("daemon.json"))
    .unwrap();
    assert!(guard::daemon_alive(&b));

    let ended = guard::sweep_vms(&a, true, h.vms(), vec![], &now_policy(), false).await;
    assert!(ended.is_empty(), "{ended:?}");
    assert!(h.running(b_orphan), "b's own daemon decides about its VMs");
}

#[tokio::test]
async fn a_vm_whose_home_was_deleted_is_ended() {
    let mut h = Host::new();
    let home = h.home("home");
    let pid = h.vm(&home, "lost");
    std::fs::remove_dir_all(&home).unwrap();
    // Nothing on disk to verify it by, so it is the homeless scan's.
    assert!(h.vms().is_empty());
    let homeless: Vec<HostVm> = hostvms::homeless_vms()
        .into_iter()
        .filter(|v| v.root.starts_with(h.dir.path()))
        .collect();
    assert_eq!(homeless.len(), 1);
    let ended = guard::sweep_vms(
        &h.home("other"),
        true,
        vec![],
        homeless,
        &now_policy(),
        false,
    )
    .await;
    assert_eq!(names(&ended), ["lost"]);
    assert_eq!(ended[0].reason, Reason::HomeGone);
    assert!(ended[0].error.is_none(), "{ended:?}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while hostvms::homeless_vms().iter().any(|v| v.pid == pid) {
        assert!(Instant::now() < deadline, "the process is still running");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn processes_that_are_not_cua_vms_are_never_touched() {
    let h = Host::new();
    let home = h.home("home");
    // A QEMU-looking process that is nobody's: right binary, wrong layout.
    let other = Command::new("/bin/sh")
        .arg("-c")
        .arg("exec sleep 30")
        .spawn()
        .unwrap();
    let mut other = scopeguard(other);
    // A directory that looks like a VM's but whose pid file names another pid.
    let vmdir = home.join("vmm/qemu/impostor");
    std::fs::create_dir_all(&vmdir).unwrap();
    std::fs::write(vmdir.join("state.json"), "{}").unwrap();
    std::fs::write(vmdir.join("qemu.pid"), format!("{}\n", other.0.id())).unwrap();
    let ended = sweep(&h, &home, true, false).await;
    assert!(ended.is_empty());
    assert!(hostvms::parse_ps("").is_empty());
    assert!(other.0.try_wait().unwrap().is_none(), "still running");
}

struct Guard(std::process::Child);
fn scopeguard(c: std::process::Child) -> Guard {
    Guard(c)
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The daemon's loop, killed with live VMs and started again.
#[tokio::test(flavor = "multi_thread")]
async fn killing_the_daemon_and_starting_it_again_ends_what_nobody_uses() {
    let mut h = Host::new();
    let home = h.home("home");
    let kept = h.vm(&home, "web");
    h.record(&home, "web");
    let orphan = h.vm(&home, "lost");
    h.starter(&home, "lost", dead_pid());
    let expired = h.vm(&home, "old");
    h.record(&home, "old");
    h.lifetime(&home, "old", unix_now() - 60, "delete");

    // The first daemon dies before it ever sweeps: its loop is aborted and
    // its VMs keep running unowned.
    let (tx, rx) = tokio::sync::watch::channel(false);
    let first = tokio::spawn(cua_daemon::server::guard_loop(
        home.clone(),
        false,
        now_policy(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        rx,
    ));
    first.abort();
    drop(tx);
    assert!(h.running(kept) && h.running(orphan) && h.running(expired));

    // The restarted daemon sweeps shortly after it starts.
    let (tx, rx) = tokio::sync::watch::channel(false);
    let second = tokio::spawn(cua_daemon::server::guard_loop(
        home.clone(),
        false,
        now_policy(),
        Duration::from_millis(100),
        Duration::from_secs(3600),
        rx,
    ));
    let deadline = Instant::now() + Duration::from_secs(30);
    while h.running(orphan) || h.running(expired) {
        assert!(Instant::now() < deadline, "the daemon left VMs nobody uses");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(h.running(kept), "and adopted the Space it has a record for");
    tx.send(true).unwrap();
    second.await.unwrap();
}

/// With the shipped timings an orphan dies within a minute of its owner:
/// the daemon is killed (its starter pid is gone), a restarted daemon's
/// guard runs on its own schedule and nothing waits for a 5 minute tick.
#[tokio::test(flavor = "multi_thread")]
async fn an_orphan_is_ended_within_a_minute_with_the_shipped_timings() {
    assert!(cua_daemon::server::GUARD_FIRST <= Duration::from_secs(5));
    assert!(cua_daemon::server::GUARD_EVERY <= Duration::from_secs(30));
    assert!(Policy::default().starter_gone <= Duration::from_secs(20));
    let mut h = Host::new();
    let home = h.home("home");
    let orphan = h.vm(&home, "lost");
    h.starter(&home, "lost", dead_pid());
    let kept = h.vm(&home, "web");
    h.record(&home, "web");
    let t0 = Instant::now();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(cua_daemon::server::guard_loop(
        home.clone(),
        false,
        Policy::default(),
        cua_daemon::server::GUARD_FIRST,
        cua_daemon::server::GUARD_EVERY,
        rx,
    ));
    while h.running(orphan) {
        assert!(
            t0.elapsed() < Duration::from_secs(60),
            "orphan lived over 60 s"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(h.running(kept));
    tx.send(true).unwrap();
    task.await.unwrap();
}
