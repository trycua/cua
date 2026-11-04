//! A daemon that exited leaves its socket and discovery file behind. Those
//! must read as "not running" (never a raw transport error), be removed
//! only when the recorded pid is provably dead and nothing listens, and a
//! live daemon's files must never be touched. Everything lives in temp
//! directories; the only processes are this test's own children.
#![cfg(unix)]

use cua_daemon::{
    DAEMON_NOT_RUNNING, Discovery, Error, Runtime, RuntimeConfig,
    client::{DaemonAddress, DaemonClient},
    live_discovery, remove_stale,
    server::{self, ServerConfig},
};
use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::Duration,
};

/// A pid that belonged to a process that has exited (and was reaped).
fn dead_pid() -> u32 {
    let mut c = Command::new("true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

/// A live child process (killed by the guard, by handle).
struct Alive(Child);
impl Alive {
    fn new() -> Self {
        Self(Command::new("sleep").arg("60").spawn().unwrap())
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
}
impl Drop for Alive {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A socket file nothing listens on (bound, then the listener dropped).
fn stale_socket(path: &Path) {
    drop(UnixListener::bind(path).unwrap());
    assert!(path.exists());
}

fn discovery(pid: u32, sock: &Path) -> Discovery {
    Discovery {
        pid,
        socket_path: Some(sock.display().to_string()),
        loopback_url: None,
        token: Some("t".into()),
        version: "0".into(),
    }
}

fn paths(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    (dir.path().join("cua.sock"), dir.path().join("daemon.json"))
}

#[test]
fn a_dead_pid_and_a_refused_socket_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, disc) = paths(&dir);
    stale_socket(&sock);
    let d = discovery(dead_pid(), &sock);
    d.write(&disc).unwrap();
    assert!(!d.listening());
    assert!(d.is_stale());
    assert_eq!(live_discovery(&disc), None);
    assert!(!disc.exists(), "stale discovery file removed");
    assert!(!sock.exists(), "stale socket removed");
}

#[test]
fn stale_cleanup_removes_only_a_socket_never_the_file_or_link_it_names() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let disc = dir.path().join("daemon.json");
    // A regular file, and a symlink to one, recorded as the "socket".
    let file = dir.path().join("precious.txt");
    std::fs::write(&file, b"keep").unwrap();
    let link = dir.path().join("link.sock");
    symlink(&file, &link).unwrap();
    for named in [&file, &link] {
        let d = discovery(dead_pid(), named);
        d.write(&disc).unwrap();
        assert_eq!(live_discovery(&disc), None);
        assert!(named.symlink_metadata().is_ok(), "{} kept", named.display());
    }
    assert_eq!(std::fs::read(&file).unwrap(), b"keep");
}

#[test]
fn the_discovery_file_is_owner_only_and_never_follows_a_planted_temp() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let (sock, disc) = paths(&dir);
    // Where the previous writer put its temp file: a symlink to a decoy.
    let decoy = dir.path().join("decoy");
    std::fs::write(&decoy, b"untouched").unwrap();
    symlink(
        &decoy,
        disc.with_extension(format!("tmp.{}", std::process::id())),
    )
    .unwrap();
    discovery(std::process::id(), &sock).write(&disc).unwrap();
    let mode = std::fs::metadata(&disc).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(std::fs::read(&decoy).unwrap(), b"untouched");
    assert_eq!(Discovery::read(&disc).unwrap().token.as_deref(), Some("t"));
}

#[test]
fn a_live_pid_keeps_its_files_even_when_the_socket_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, disc) = paths(&dir);
    stale_socket(&sock);
    let alive = Alive::new();
    let d = discovery(alive.pid(), &sock);
    d.write(&disc).unwrap();
    assert!(!d.is_stale());
    // Not running (nothing listens), but nothing is deleted.
    assert_eq!(live_discovery(&disc), None);
    assert!(!remove_stale(&disc, &d));
    assert!(disc.exists() && sock.exists());
}

#[test]
fn a_listening_daemon_is_live_and_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, disc) = paths(&dir);
    let _listener = UnixListener::bind(&sock).unwrap();
    // Even with a pid that is gone, a listener is a running daemon.
    let d = discovery(dead_pid(), &sock);
    d.write(&disc).unwrap();
    assert!(d.listening());
    assert!(!d.is_stale());
    assert_eq!(live_discovery(&disc), Some(d.clone()));
    assert!(!remove_stale(&disc, &d));
    assert!(disc.exists() && sock.exists());
}

#[test]
fn a_rewritten_discovery_file_is_not_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, disc) = paths(&dir);
    stale_socket(&sock);
    let old = discovery(dead_pid(), &sock);
    // A new daemon wrote its own file meanwhile.
    let new = discovery(std::process::id(), &sock);
    new.write(&disc).unwrap();
    assert!(!remove_stale(&disc, &old));
    assert_eq!(Discovery::read(&disc), Some(new));
}

#[test]
fn a_stale_loopback_port_is_not_listening() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let d = Discovery {
        pid: dead_pid(),
        socket_path: None,
        loopback_url: Some(format!("http://127.0.0.1:{port}")),
        token: Some("t".into()),
        version: "0".into(),
    };
    assert!(!d.listening());
    assert!(d.is_stale());
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let live = Discovery {
        loopback_url: Some(format!("http://{}", l.local_addr().unwrap())),
        ..d
    };
    assert!(live.listening());
}

#[tokio::test]
async fn calls_to_a_dead_daemon_fail_with_daemon_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, _) = paths(&dir);
    stale_socket(&sock);
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    for addr in [
        // A refused socket (the reported case: ECONNREFUSED, os error 61).
        DaemonAddress::Socket(sock.clone()),
        // A socket file that is gone.
        DaemonAddress::Socket(dir.path().join("missing.sock")),
        // A loopback port nothing listens on.
        DaemonAddress::Url {
            url: format!("http://127.0.0.1:{port}"),
            token: "t".into(),
        },
    ] {
        let c = DaemonClient::new(addr.clone()).unwrap();
        let e = tokio::time::timeout(Duration::from_secs(10), c.info())
            .await
            .expect("bounded")
            .unwrap_err();
        assert_eq!(
            e,
            Error::DaemonNotRunning(DAEMON_NOT_RUNNING.into()),
            "{addr:?}"
        );
        assert_eq!(e.to_string(), DAEMON_NOT_RUNNING);
        assert!(!e.to_string().to_lowercase().contains("transport"));
    }
}

fn runtime(dir: &Path) -> Runtime {
    Runtime::new(RuntimeConfig {
        state_dir: Some(dir.join("sandboxes")),
        spaces_home: Some(dir.join("spaces")),
        ..Default::default()
    })
    .unwrap()
}

fn config(sock: &Path, disc: &Path) -> ServerConfig {
    ServerConfig {
        socket_path: Some(sock.to_path_buf()),
        loopback: None,
        token: "daemon-token".into(),
        discovery_path: Some(disc.to_path_buf()),
        bridge_ticket_ttl: Duration::from_secs(30),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_replaces_a_stale_socket_and_discovery_file() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, disc) = paths(&dir);
    stale_socket(&sock);
    discovery(dead_pid(), &sock).write(&disc).unwrap();
    let h = server::start(runtime(dir.path()), config(&sock, &disc))
        .await
        .unwrap();
    let d = Discovery::read(&disc).unwrap();
    assert_eq!(d.pid, std::process::id());
    assert!(d.listening());
    let c = DaemonClient::new(DaemonAddress::Socket(sock.clone())).unwrap();
    c.info().await.unwrap();
    h.shutdown();
    tokio::time::timeout(Duration::from_secs(5), h.wait())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_refuses_over_a_live_daemon_and_leaves_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, disc) = paths(&dir);
    // Another live daemon: its pid is alive and its socket listens.
    let alive = Alive::new();
    let _listener = UnixListener::bind(&sock).unwrap();
    let theirs = discovery(alive.pid(), &sock);
    theirs.write(&disc).unwrap();
    let other_sock = dir.path().join("other.sock");
    let e = match server::start(runtime(dir.path()), config(&other_sock, &disc)).await {
        Err(e) => e,
        Ok(_) => panic!("started over a live daemon"),
    };
    assert!(matches!(e, Error::InvalidArgument(ref m) if m.contains("already running")));
    assert_eq!(Discovery::read(&disc), Some(theirs));
    assert!(sock.exists());
    // The same socket path is refused too.
    assert!(
        server::start(
            runtime(dir.path()),
            config(&sock, &dir.path().join("x.json"))
        )
        .await
        .is_err()
    );
    assert!(sock.exists());
}
