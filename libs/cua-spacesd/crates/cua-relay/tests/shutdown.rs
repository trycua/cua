// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay binary stops promptly on SIGTERM (what Kubernetes and Docker
//! send), also while a connection is still open.

#![cfg(unix)]

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(port: u16, grace_secs: u64) -> Child {
    Command::new(env!("CARGO_BIN_EXE_cua-relay"))
        .env("CUA_RELAY_LISTEN", format!("127.0.0.1:{port}"))
        .env("CUA_RELAY_TOKENS", "shutdown-test-token")
        .env("CUA_RELAY_SHUTDOWN_GRACE_SECS", grace_secs.to_string())
        .env_remove("CUA_RELAY_OIDC_ISSUER")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cua-relay")
}

fn healthz(port: u16) -> Option<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    s.write_all(b"GET /healthz HTTP/1.1\r\nhost: relay\r\nconnection: close\r\n\r\n")
        .ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    out.lines().next().map(str::to_owned)
}

fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(status) = healthz(port) {
            assert!(
                status.contains(" 204 "),
                "unexpected /healthz status: {status}"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("relay did not become ready");
}

fn sigterm(child: &Child) {
    let ok = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill")
        .success();
    assert!(ok, "kill -TERM failed");
}

fn wait_exit(child: &mut Child, within: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("relay still running {within:?} after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn sigterm_stops_an_idle_relay() {
    let port = free_port();
    let mut child = start(port, 10);
    wait_ready(port);
    sigterm(&child);
    let status = wait_exit(&mut child, Duration::from_secs(5));
    assert!(status.success(), "exit status {status:?}");
}

#[test]
fn sigterm_with_an_open_request_exits_after_the_grace() {
    let port = free_port();
    let mut child = start(port, 1);
    wait_ready(port);
    // A request whose body never arrives keeps its connection busy.
    let mut open = TcpStream::connect(("127.0.0.1", port)).unwrap();
    open.write_all(
        b"POST /v1/machines HTTP/1.1\r\nhost: relay\r\ncontent-length: 1000000\r\n\r\npartial",
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    sigterm(&child);
    let status = wait_exit(&mut child, Duration::from_secs(6));
    assert!(status.success(), "exit status {status:?}");
    assert!(started.elapsed() < Duration::from_secs(6));
    drop(open);
}
