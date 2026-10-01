//! The default topology over a daemon that exited (the Spaces app's
//! "transport: Connection refused (os error 61)"): its socket and
//! discovery file are left in `$CUA_HOME`. `Cua::auto` must fall back to
//! embedded, `Cua::connect` must fail with `DaemonNotRunning` (never a raw
//! transport error), stale files go only when the pid is provably dead, and
//! a live daemon is used and never touched.
//!
//! One test in its own binary: it points `CUA_HOME` at a temp directory
//! for the whole process (never the user's `~/.cua`).
#![cfg(unix)]

use cua_daemon::{
    Discovery, Runtime, RuntimeConfig,
    server::{self, ServerConfig},
};
use cua_sdk::{Cua, CuaConfig, CuaError, CuaMode};
use std::{
    os::unix::net::UnixListener,
    path::Path,
    process::{Child, Command},
    time::Duration,
};

fn dead_pid() -> u32 {
    let mut c = Command::new("true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

struct Alive(Child);
impl Drop for Alive {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn leave_stale(sock: &Path, disc: &Path, pid: u32) -> Discovery {
    let _ = std::fs::remove_file(sock);
    drop(UnixListener::bind(sock).unwrap());
    let d = Discovery {
        pid,
        socket_path: Some(sock.display().to_string()),
        loopback_url: None,
        token: Some("t".into()),
        version: "0".into(),
    };
    d.write(disc).unwrap();
    d
}

fn embedded_config(home: &Path) -> CuaConfig {
    CuaConfig {
        state_dir: Some(home.join("sandboxes").display().to_string()),
        spaces_home: Some(home.display().to_string()),
        teleport_home: Some(home.join("teleport").display().to_string()),
        fleet_from_env: false,
        ..Default::default()
    }
}

#[test]
fn a_dead_daemon_falls_back_or_reads_as_not_running() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("cua");
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: the only test in this binary; set before any other thread
    // reads the environment.
    unsafe { std::env::set_var("CUA_HOME", &home) };
    assert_eq!(cua_daemon::cua_home(), home);
    let sock = home.join("cua.sock");
    let disc = home.join("daemon.json");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    // 1. Stale socket + discovery file with a dead pid: explicit daemon mode
    //    fails with the typed error and its plain message.
    leave_stale(&sock, &disc, dead_pid());
    let c = Cua::connect(None, None).unwrap();
    let e = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), c.info())
            .await
            .unwrap()
            .unwrap_err()
    });
    assert!(matches!(e, CuaError::DaemonNotRunning(_)), "{e:?}");
    assert_eq!(e.to_string(), cua_daemon::DAEMON_NOT_RUNNING);
    let e = rt.block_on(c.spaces().list()).unwrap_err();
    assert!(matches!(e, CuaError::DaemonNotRunning(_)), "{e:?}");
    // The default topology falls back to embedded and removes the files.
    let cua = Cua::auto(embedded_config(&home)).unwrap();
    assert_eq!(cua.mode(), CuaMode::Embedded);
    assert!(!disc.exists() && !sock.exists(), "stale files removed");

    // 2. Same, but the pid is alive (a daemon starting, or a reused pid):
    //    embedded, and nothing is deleted.
    let alive = Alive(Command::new("sleep").arg("60").spawn().unwrap());
    let kept = leave_stale(&sock, &disc, alive.0.id());
    let cua = Cua::auto(embedded_config(&home)).unwrap();
    assert_eq!(cua.mode(), CuaMode::Embedded);
    assert_eq!(Discovery::read(&disc), Some(kept));
    assert!(sock.exists());
    drop(alive);
    std::fs::remove_file(&sock).unwrap();
    std::fs::remove_file(&disc).unwrap();

    // 3. A live daemon: used, and its files stay.
    rt.block_on(async {
        let runtime = Runtime::new(RuntimeConfig {
            state_dir: Some(home.join("daemon-sandboxes")),
            spaces_home: Some(home.join("daemon-spaces")),
            ..Default::default()
        })
        .unwrap();
        let h = server::start(
            runtime,
            ServerConfig {
                socket_path: Some(sock.clone()),
                loopback: None,
                token: "daemon-token".into(),
                discovery_path: Some(disc.clone()),
                bridge_ticket_ttl: Duration::from_secs(30),
            },
        )
        .await
        .unwrap();
        let before = Discovery::read(&disc).unwrap();
        let cua = Cua::auto(embedded_config(&home)).unwrap();
        assert_eq!(cua.mode(), CuaMode::Daemon);
        let info = cua.info().await.unwrap();
        assert_eq!(info.daemon_pid, Some(std::process::id()));
        assert_eq!(Discovery::read(&disc), Some(before));
        assert!(sock.exists());
        h.shutdown();
        tokio::time::timeout(Duration::from_secs(5), h.wait())
            .await
            .unwrap();
    });
}
