// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The macOS mount across daemon restarts (CUA-2112): a daemon that dies
//! without unmounting must not leave a mount macOS reports as "Server
//! connections interrupted". Each test runs the drive's services in a child
//! process (this test binary again, as `volume_daemon_child`), kills it as a
//! crash would, starts another and checks the mount.
//!
//! Opt-in, macOS: `CUA_DRIVE_NFS_TEST=1 CUA_DRIVE_TEST_DIR=<dir>` (a folder
//! of yours, not under /tmp or a protected folder). Mounts only under that
//! folder, and ends only the processes it started.

#![cfg(all(feature = "nfs", target_os = "macos"))]

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_volume::service::{DriveService, EnvKeys};
use cua_volume::{Drive, new_id};

const CHILD_ENV: &str = "CUA_VOLUME_TEST_CHILD";
const READY: &str = "VOLUME-CHILD-READY ";

/// The child: the drive's services as the daemon runs them (started with
/// the mount on, shut down on SIGTERM).
#[test]
fn volume_daemon_child() {
    if std::env::var(CHILD_ENV).as_deref() != Ok("1") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let home = PathBuf::from(std::env::var("CUA_HOME").unwrap());
        let svc = DriveService::new(Drive::open_local(&home), &home, Arc::new(EnvKeys)).unwrap();
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        svc.start().await;
        let st = svc.mount_status().await;
        println!("{READY}{}", serde_json::to_string(&st).unwrap());
        term.recv().await;
        svc.shutdown().await;
        println!("VOLUME-CHILD-STOPPED");
    });
    std::process::exit(0);
}

fn enabled() -> Option<PathBuf> {
    if std::env::var("CUA_DRIVE_NFS_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_DRIVE_NFS_TEST=1");
        return None;
    }
    Some(PathBuf::from(
        std::env::var("CUA_DRIVE_TEST_DIR").expect("CUA_DRIVE_TEST_DIR"),
    ))
}

/// One cua home and mount point under the test folder; force-unmounts its
/// own mount if a test fails with it still there.
struct Home {
    home: PathBuf,
    mnt: PathBuf,
}

impl Home {
    fn new(base: &Path) -> Home {
        let home = base.join(format!("h-{}", new_id()));
        let mnt = home.join("mnt").join("Cua Volume");
        std::fs::create_dir_all(home.join("volume")).unwrap();
        std::fs::write(
            home.join("volume").join("mount.json"),
            br#"{"enabled": true}"#,
        )
        .unwrap();
        Home { home, mnt }
    }

    fn nfs_state(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.home.join("volume/nfs.json")).unwrap()).unwrap()
    }

    fn port(&self) -> u16 {
        self.nfs_state()["port"].as_u64().unwrap() as u16
    }

    fn spawn(&self) -> Daemon {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "volume_daemon_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1")
            .env("CUA_HOME", &self.home)
            .env("CUA_DRIVE_MOUNT_PATH", &self.mnt)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Daemon {
            child: Some(child),
            lines: rx,
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if mounted(&self.mnt).is_some() {
            let _ = Command::new("/sbin/umount")
                .arg("-f")
                .arg(&self.mnt)
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

struct Daemon {
    child: Option<Child>,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Daemon {
    /// The mount status the child reported once started.
    fn ready(&self, within: Duration) -> serde_json::Value {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .expect("the child never got ready");
            // libtest prints "test volume_daemon_child ... " first.
            if let Some(at) = line.find(READY) {
                return serde_json::from_str(&line[at + READY.len()..]).unwrap();
            }
        }
    }

    /// Ends it as a crash would: SIGKILL, nothing unmounted.
    fn crash(mut self) {
        let mut c = self.child.take().unwrap();
        c.kill().unwrap();
        c.wait().unwrap();
    }

    /// Stops it as `cua daemon stop`'s fallback, launchd or logout would.
    fn terminate(mut self) {
        let mut c = self.child.take().unwrap();
        // SAFETY: kill(2) on the pid of the child this test started.
        unsafe { libc::kill(c.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if c.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = c.kill();
        let _ = c.wait();
        panic!("the child did not stop on SIGTERM");
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// The kernel's id for the file system mounted at `path` (from its mount
/// table, without asking the server), if any is.
fn mounted(path: &Path) -> Option<[i32; 2]> {
    let want = std::fs::canonicalize(path.parent()?)
        .ok()?
        .join(path.file_name()?);
    let c = |b: &[libc::c_char]| {
        let bytes: Vec<u8> = b
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    // SAFETY: a null buffer asks only for the count.
    let n = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    let cap = n.max(0) as usize + 8;
    // SAFETY: a zeroed statfs is a valid value.
    let mut buf: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; cap];
    let bytes = (cap * std::mem::size_of::<libc::statfs>()) as libc::c_int;
    // SAFETY: buf holds `cap` entries, `bytes` long.
    let got = unsafe { libc::getfsstat(buf.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    buf.truncate(got.max(0) as usize);
    buf.iter().rev().find_map(|st| {
        (Path::new(&c(&st.f_mntonname)) == want && c(&st.f_fstypename) == "nfs")
            // SAFETY: fsid_t is two i32s.
            .then(|| unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(st.f_fsid) })
    })
}

/// Reads `file` through the mount until it answers with `want` (on a
/// thread, as any program would; a dead mount blocks it), and how long
/// that took.
fn read_until(file: &Path, want: &[u8], within: Duration) -> Duration {
    let t0 = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let f = file.to_path_buf();
    let want = want.to_vec();
    std::thread::spawn(move || {
        loop {
            if std::fs::read(&f).is_ok_and(|b| b == want) {
                let _ = tx.send(());
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    rx.recv_timeout(within)
        .unwrap_or_else(|_| panic!("{} did not answer within {within:?}", file.display()));
    t0.elapsed()
}

fn write_through_mount(mnt: &Path, name: &str, body: &[u8]) {
    std::fs::write(mnt.join("public").join(name), body).unwrap();
    // NFS has no close: the upload starts after UPLOAD_IDLE.
    std::thread::sleep(cua_volume::vfs::UPLOAD_IDLE + Duration::from_secs(2));
}

/// A daemon that crashed and was started again within the NFS client's
/// timeout serves the same mount: the kernel reconnects to the same port,
/// nothing is remounted, files read again, and a stop then unmounts.
#[test]
fn a_crash_then_respawn_keeps_the_mount() {
    let Some(base) = enabled() else { return };
    let h = Home::new(&base);
    let a = h.spawn();
    let st = a.ready(Duration::from_secs(60));
    assert_eq!(st["state"], "mounted", "{st}");
    let before = mounted(&h.mnt).expect("mounted");
    let port = h.port();
    write_through_mount(&h.mnt, "before.txt", b"from before the crash");

    a.crash();
    // The mount stays, its server gone; a program keeps reading it.
    assert_eq!(mounted(&h.mnt), Some(before));
    let killed = Instant::now();
    let file = h.mnt.join("public/before.txt");
    let reader = std::thread::spawn(move || {
        read_until(&file, b"from before the crash", Duration::from_secs(60))
    });
    std::thread::sleep(Duration::from_secs(2));

    let b = h.spawn();
    let st = b.ready(Duration::from_secs(60));
    assert_eq!(st["state"], "mounted", "{st}");
    assert_eq!(h.port(), port, "the same port");
    assert_eq!(
        mounted(&h.mnt),
        Some(before),
        "the same mount, not remounted"
    );
    reader.join().unwrap();
    let recovered = killed.elapsed();
    // The NFS client reports a server not responding after about 30 s.
    assert!(
        recovered < Duration::from_secs(20),
        "recovered after {recovered:?}"
    );
    eprintln!("recovered {recovered:?} after the crash");
    // New files work too.
    write_through_mount(&h.mnt, "after.txt", b"after");
    read_until(
        &h.mnt.join("public/after.txt"),
        b"after",
        Duration::from_secs(10),
    );

    b.terminate();
    assert_eq!(mounted(&h.mnt), None, "SIGTERM unmounts");
    assert!(h.nfs_state().get("mount").is_none());
}

/// A restart that does unmount (a stop, then a start) mounts again on the
/// same port: the port is stable per cua home.
#[test]
fn a_restart_mounts_again_on_the_same_port() {
    let Some(base) = enabled() else { return };
    let h = Home::new(&base);
    let a = h.spawn();
    assert_eq!(a.ready(Duration::from_secs(60))["state"], "mounted");
    let port = h.port();
    write_through_mount(&h.mnt, "kept.txt", b"kept");
    a.terminate();
    assert_eq!(mounted(&h.mnt), None);

    let b = h.spawn();
    assert_eq!(b.ready(Duration::from_secs(60))["state"], "mounted");
    assert_eq!(h.port(), port);
    read_until(
        &h.mnt.join("public/kept.txt"),
        b"kept",
        Duration::from_secs(10),
    );
    b.terminate();
    assert_eq!(mounted(&h.mnt), None);
}

/// The port is taken while the daemon is down: the next one listens on
/// another, unmounts the mount it cannot serve and mounts again.
#[test]
fn a_taken_port_falls_back_with_a_remount() {
    let Some(base) = enabled() else { return };
    let h = Home::new(&base);
    let a = h.spawn();
    assert_eq!(a.ready(Duration::from_secs(60))["state"], "mounted");
    let before = mounted(&h.mnt).unwrap();
    let port = h.port();
    write_through_mount(&h.mnt, "x.txt", b"x");
    a.crash();
    let squatter = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();

    let b = h.spawn();
    let st = b.ready(Duration::from_secs(60));
    assert_eq!(st["state"], "mounted", "{st}");
    assert_ne!(h.port(), port, "another port");
    let after = mounted(&h.mnt).expect("mounted again");
    assert_ne!(after, before, "a new mount");
    read_until(&h.mnt.join("public/x.txt"), b"x", Duration::from_secs(10));
    drop(squatter);
    b.terminate();
    assert_eq!(mounted(&h.mnt), None);
}

/// A mount an older daemon left (no record of it; it reached a random
/// port) is unmounted and mounted again, not served.
#[test]
fn an_older_daemons_mount_is_replaced() {
    let Some(base) = enabled() else { return };
    let h = Home::new(&base);
    let a = h.spawn();
    assert_eq!(a.ready(Duration::from_secs(60))["state"], "mounted");
    let before = mounted(&h.mnt).unwrap();
    write_through_mount(&h.mnt, "y.txt", b"y");
    a.crash();
    // An older daemon kept no nfs.json.
    std::fs::remove_file(h.home.join("volume/nfs.json")).unwrap();

    let b = h.spawn();
    assert_eq!(b.ready(Duration::from_secs(60))["state"], "mounted");
    let after = mounted(&h.mnt).expect("mounted again");
    assert_ne!(after, before, "a new mount");
    read_until(&h.mnt.join("public/y.txt"), b"y", Duration::from_secs(10));
    b.terminate();
    assert_eq!(mounted(&h.mnt), None);
}
