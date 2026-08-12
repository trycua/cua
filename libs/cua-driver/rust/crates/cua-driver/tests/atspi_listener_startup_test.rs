//! Linux process-level readiness when AT-SPI accepts a connection but stalls.

#![cfg(target_os = "linux")]

use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use cua_driver_testkit::{spawn_in_job, ChildReaper};

#[test]
fn serve_binds_while_reachable_atspi_initialization_is_stalled() {
    let directory = tempfile::Builder::new()
        .prefix("cua-atspi-startup-")
        .tempdir_in("/tmp")
        .expect("temporary startup-test directory");
    let bus_path = directory.path().join("stalled-session-bus.sock");
    let daemon_socket = directory.path().join("driver.sock");
    let bus = UnixListener::bind(&bus_path).expect("bind reachable stalled bus");
    bus.set_nonblocking(true)
        .expect("make stalled bus listener nonblocking");
    let accepted = Arc::new(AtomicBool::new(false));
    let accepted_in_server = accepted.clone();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let bus_server = std::thread::spawn(move || {
        let accept_deadline = Instant::now() + Duration::from_secs(10);
        let connection = loop {
            match bus.accept() {
                Ok((connection, _)) => break Some(connection),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= accept_deadline {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept driver bus connection: {error}"),
            }
        };
        if connection.is_some() {
            accepted_in_server.store(true, Ordering::SeqCst);
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        }
    });

    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .args([
            "serve",
            "--socket",
            daemon_socket.to_str().expect("UTF-8 daemon socket"),
            "--no-overlay",
            "--no-permissions-gate",
        ])
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", bus_path.display()),
        )
        .env("CUA_DRIVER_RS_DISABLE_A11Y_ADVERTISE", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut reaper = ChildReaper::new();
    let child = spawn_in_job(&mut command).expect("spawn cua-driver serve");
    reaper.push(child);

    let readiness_deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < readiness_deadline {
        if UnixStream::connect(&daemon_socket).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let ready = UnixStream::connect(&daemon_socket).is_ok();
    let _ = release_tx.send(());
    bus_server.join().expect("join stalled bus server");
    assert!(
        accepted.load(Ordering::SeqCst),
        "driver never reached the listening session-bus socket"
    );
    assert!(
        ready && daemon_socket.exists(),
        "serve did not bind after the bounded AT-SPI readiness wait"
    );
}
