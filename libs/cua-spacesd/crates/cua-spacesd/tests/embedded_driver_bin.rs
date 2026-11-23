// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The real binary starts the Cua Driver for this Mac's coding agents even
//! when it was set up to keep its screen private (`--no-desktop --no-driver
//! --no-mcp`, the new first run's "Add this computer"), and the only thing
//! that keeps it from running is the person's "Let agents use apps on this
//! Mac" switch (the pause marker). macOS only.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A stand-in cua-driver that notes each start in `started` and stays up.
fn fake_driver(dir: &Path, started: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join("cua-driver");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec sleep 30\n",
            started.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// cua-spacesd as the host service runs it on a Mac that doesn't share its
/// desktop, with `home` as the user's home.
fn host_without_desktop(home: &Path, driver: &Path) -> Kill {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    Kill(
        Command::new(env!("CARGO_BIN_EXE_cua-spacesd"))
            .args([
                "serve",
                "--no-desktop",
                "--no-driver",
                "--no-mcp",
                "--quic-port",
                "0",
            ])
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--data-dir")
            .arg(home.join("data"))
            .arg("--token-file")
            .arg(home.join("token"))
            .env("HOME", home)
            .env("CUA_ENV_TOKEN", uuid::Uuid::new_v4().to_string())
            .env("CUA_SPACESD_DRIVER_BIN", driver)
            .env("CUA_ENV_LOG", "warn")
            .env_remove("CUA_CHANNEL")
            .env_remove("CUA_HOME")
            .env_remove("CUA_SPACESD_EMBEDDED_DRIVER")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

fn wait_for(path: &Path, within: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < within {
        if std::fs::metadata(path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn a_mac_that_keeps_its_screen_private_still_runs_the_driver() {
    let home = tempfile::tempdir().unwrap();
    let started = home.path().join("driver-started");
    let driver = fake_driver(home.path(), &started);
    let _host = host_without_desktop(home.path(), &driver);
    assert!(
        wait_for(&started, Duration::from_secs(15)),
        "cua-spacesd --no-desktop --no-driver never started the embedded Cua Driver"
    );
    let args = std::fs::read_to_string(&started).unwrap();
    assert!(args.contains("--embedded"), "{args}");
}

#[test]
fn the_switch_alone_decides_whether_it_runs() {
    let home = tempfile::tempdir().unwrap();
    let run = home.path().join(".cua/run");
    std::fs::create_dir_all(&run).unwrap();
    let paused = run.join("cua-driver.off");
    std::fs::write(&paused, "1\n").unwrap();
    let started = home.path().join("driver-started");
    let driver = fake_driver(home.path(), &started);
    let _host = host_without_desktop(home.path(), &driver);
    // Switched off: nothing starts.
    assert!(!wait_for(&started, Duration::from_secs(4)));
    // Switched on: it starts within a couple of seconds, set up earlier
    // with --no-driver or not.
    std::fs::remove_file(&paused).unwrap();
    assert!(
        wait_for(&started, Duration::from_secs(10)),
        "turning the switch on didn't start the embedded Cua Driver"
    );
}
