// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-driver serve` outlives failed accepts and shuts down cleanly on a
//! termination signal.

#![cfg(unix)]

use cua_driver_testkit::{socket_tempdir, spawn_in_job};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(10);

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Daemon {
    fn exit_status(&mut self) -> Option<ExitStatus> {
        self.0.try_wait().expect("poll the daemon")
    }

    fn signal_and_wait(&mut self, signal: libc::c_int) -> ExitStatus {
        unsafe {
            libc::kill(self.0.id() as libc::pid_t, signal);
        }
        let deadline = Instant::now() + BOUND;
        loop {
            if let Some(status) = self.exit_status() {
                return status;
            }
            assert!(Instant::now() < deadline, "still running after {signal}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn driver(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .stdin(Stdio::null())
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        // Serve in this process instead of a disclaimed macOS child, so the
        // spawned child's PID is the daemon's PID.
        .env("CUA_DRIVER_RS_RESPONSIBILITY_DISCLAIMED", "1")
        .env("HOME", home)
        .env("XDG_STATE_HOME", home)
        .env("CUA_DRIVER_HOME", home)
        .env("CUA_DRIVER_LOCAL_HOME", home)
        .env("CUA_DRIVER_RS_HOME", home);
    command
}

fn serve(
    home: &Path,
    socket: &Path,
    pid_file: &Path,
    configure: impl FnOnce(&mut Command),
) -> Daemon {
    let mut serve = driver(home);
    serve
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .arg("--pid-file")
        .arg(pid_file)
        .args(["--no-overlay", "--no-permissions-gate"])
        .stdout(Stdio::null())
        .stderr(if std::env::var_os("CUA_TEST_DRIVER_STDERR").is_some() {
            Stdio::inherit()
        } else {
            Stdio::null()
        });
    configure(&mut serve);
    let mut daemon = Daemon(spawn_in_job(&mut serve).expect("spawn source-built daemon"));
    let deadline = Instant::now() + BOUND;
    while !answers_status(home, socket) {
        if let Some(status) = daemon.exit_status() {
            panic!("daemon exited before serving: {status}");
        }
        assert!(Instant::now() < deadline, "daemon never served {socket:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    daemon
}

fn answers_status(home: &Path, socket: &Path) -> bool {
    driver(home)
        .arg("status")
        .arg("--socket")
        .arg(socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn limit_descriptors(command: &mut Command, limit: libc::rlim_t) {
    let limit = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    unsafe {
        command.pre_exec(move || {
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[test]
fn exhausted_descriptors_do_not_stop_the_daemon() {
    const DESCRIPTOR_LIMIT: libc::rlim_t = 128;
    let home = socket_tempdir().unwrap();
    let socket = home.path().join("d.sock");
    let mut daemon = serve(home.path(), &socket, &home.path().join("d.pid"), |serve| {
        limit_descriptors(serve, DESCRIPTOR_LIMIT)
    });

    // As many connections as descriptors: accept fails with EMFILE while the
    // rest wait in the listen backlog. macOS refuses connects while that
    // backlog is full, so a refused connect is retried.
    let mut connections = Vec::new();
    let deadline = Instant::now() + BOUND;
    while connections.len() < DESCRIPTOR_LIMIT as usize && daemon.exit_status().is_none() {
        match UnixStream::connect(&socket) {
            Ok(connection) => connections.push(connection),
            Err(error) => {
                assert!(Instant::now() < deadline, "connect to the daemon: {error}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        daemon.exit_status(),
        None,
        "the daemon exited while descriptors ran out"
    );
    drop(connections);

    let deadline = Instant::now() + BOUND;
    while !answers_status(home.path(), &socket) {
        assert!(
            Instant::now() < deadline,
            "the daemon stopped answering after descriptors ran out"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_shuts_the_daemon_down_and_removes_its_socket_and_pid_file() {
    let home = socket_tempdir().unwrap();
    let socket = home.path().join("d.sock");
    let pid_file = home.path().join("d.pid");
    let mut daemon = serve(home.path(), &socket, &pid_file, |_| {});
    assert!(pid_file.exists());

    let status = daemon.signal_and_wait(libc::SIGTERM);
    assert!(
        status.success(),
        "daemon did not shut down cleanly on SIGTERM: {status}"
    );
    assert!(!socket.exists(), "SIGTERM left the socket file behind");
    assert!(!pid_file.exists(), "SIGTERM left the pid file behind");
}

#[cfg(target_os = "macos")]
#[test]
fn sigterm_to_a_relaunching_serve_stops_the_relaunched_daemon() {
    let home = socket_tempdir().unwrap();
    let socket = home.path().join("d.sock");
    let pid_file = home.path().join("d.pid");
    let mut launcher = serve(home.path(), &socket, &pid_file, |serve| {
        serve.env_remove("CUA_DRIVER_RS_RESPONSIBILITY_DISCLAIMED");
    });
    let daemon_pid: libc::pid_t = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(
        daemon_pid,
        launcher.0.id() as libc::pid_t,
        "serve did not relaunch itself as a disclaimed child"
    );

    let status = launcher.signal_and_wait(libc::SIGTERM);
    assert_eq!(
        unsafe { libc::kill(daemon_pid, 0) },
        -1,
        "the relaunched daemon outlived its launcher"
    );
    assert!(
        status.success(),
        "launcher did not mirror a clean shutdown: {status}"
    );
    assert!(!socket.exists(), "SIGTERM left the socket file behind");
    assert!(!pid_file.exists(), "SIGTERM left the pid file behind");
}
