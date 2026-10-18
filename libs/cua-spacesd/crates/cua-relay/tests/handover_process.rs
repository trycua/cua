// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Two relay processes on one state directory (a rolling deploy, CUA-1774):
//! the second waits in standby, its health port answering and its main
//! port holding requests, and takes over the moment the first exits.

#![cfg(unix)]

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(state: &Path, port: u16, health: u16) -> Child {
    Command::new(env!("CARGO_BIN_EXE_cua-relay"))
        .env("CUA_RELAY_LISTEN", format!("127.0.0.1:{port}"))
        .env("CUA_RELAY_HEALTH_LISTEN", format!("127.0.0.1:{health}"))
        .env("CUA_RELAY_TOKENS", "handover-test-token")
        .env("CUA_RELAY_STATE_FILE", state.join("machines.json"))
        .env("CUA_RELAY_SHUTDOWN_GRACE_SECS", "2")
        .env_remove("CUA_RELAY_OIDC_ISSUER")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cua-relay")
}

/// The status line of `GET path`, or `None` without an answer in `wait`.
fn get(port: u16, path: &str, wait: Duration) -> Option<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(wait)).ok()?;
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nhost: relay\r\nconnection: close\r\n\r\n").as_bytes(),
    )
    .ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    out.lines().next().map(str::to_owned)
}

fn wait_for(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if get(port, "/healthz", Duration::from_secs(1)).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("nothing answered on port {port}");
}

fn sigterm(child: &Child) {
    assert!(Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill")
        .success());
}

#[test]
fn the_next_relay_waits_then_serves_what_queued() {
    let state = tempfile::tempdir().unwrap();
    let (a_port, a_health) = (free_port(), free_port());
    let mut a = start(state.path(), a_port, a_health);
    wait_for(a_port);

    let (b_port, b_health) = (free_port(), free_port());
    let mut b = start(state.path(), b_port, b_health);
    wait_for(b_health);
    // Standby: the probe port answers, the main port holds requests.
    assert!(
        get(b_port, "/healthz", Duration::from_millis(500)).is_none(),
        "the second relay served while the first still owned the state"
    );
    let held = std::thread::spawn(move || {
        let started = Instant::now();
        (
            get(b_port, "/v1/info", Duration::from_secs(15)),
            started.elapsed(),
        )
    });
    std::thread::sleep(Duration::from_millis(300));

    let stopped = Instant::now();
    sigterm(&a);
    let (status, _) = held.join().unwrap();
    let after_stop = stopped.elapsed();
    let status = status.expect("the held request was dropped");
    assert!(status.contains(" 200 "), "{status}");
    assert!(
        after_stop < Duration::from_secs(5),
        "took over {after_stop:?} after the first relay was stopped"
    );
    assert!(a.wait().unwrap().success());
    assert!(get(b_health, "/healthz", Duration::from_secs(2))
        .unwrap()
        .contains(" 204 "));
    sigterm(&b);
    assert!(b.wait().unwrap().success());
}
