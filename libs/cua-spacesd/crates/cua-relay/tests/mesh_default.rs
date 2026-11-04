// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay binary serves the mesh only when `CUA_RELAY_MESH=on` is set
//! (CUA-1426): a relay deployed without it (e.g. a pin promoted from
//! staging) keeps the HTTPS path only.

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

struct Relay(Child);

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(port: u16, mesh: Option<&str>) -> Relay {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cua-relay"));
    cmd.env("CUA_RELAY_LISTEN", format!("127.0.0.1:{port}"))
        .env("CUA_RELAY_TOKENS", "mesh-default-test-token")
        .env_remove("CUA_RELAY_OIDC_ISSUER")
        .env_remove("CUA_RELAY_MESH")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(v) = mesh {
        cmd.env("CUA_RELAY_MESH", v);
    }
    Relay(cmd.spawn().expect("spawn cua-relay"))
}

/// Status line and body of an HTTP/1.1 GET with `extra` header lines.
fn get_with(port: u16, path: &str, extra: &str) -> Option<(String, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(s, "GET {path} HTTP/1.1\r\nhost: relay\r\n{extra}\r\n").ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    let status = out.lines().next()?.to_owned();
    let body = out.split_once("\r\n\r\n").map(|(_, b)| b.to_owned())?;
    Some((status, body))
}

fn get(port: u16, path: &str) -> Option<(String, String)> {
    get_with(port, path, "connection: close\r\n")
}

/// A WebSocket upgrade request, as a mesh node sends it.
const UPGRADE: &str = "upgrade: websocket\r\nconnection: upgrade, close\r\n\
     sec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n";

fn info(port: u16) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some((status, body)) = get(port, "/v1/info") {
            assert!(status.contains(" 200 "), "/v1/info: {status}");
            return serde_json::from_str(&body).expect("/v1/info json");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("relay did not come up");
}

#[test]
fn the_mesh_is_off_unless_turned_on() {
    let port = free_port();
    let _relay = start(port, None);
    let info = info(port);
    assert!(
        info.get("mesh").is_none_or(serde_json::Value::is_null),
        "mesh advertised by default: {info}"
    );
    let (status, _) = get_with(port, "/v1/mesh/connect", UPGRADE).expect("mesh connect");
    assert!(
        status.contains(" 404 "),
        "/v1/mesh/connect by default: {status}"
    );
}

#[test]
fn cua_relay_mesh_on_serves_the_mesh() {
    let port = free_port();
    let _relay = start(port, Some("on"));
    let info = info(port);
    assert!(
        info["mesh"].is_object(),
        "no mesh with CUA_RELAY_MESH=on: {info}"
    );
}
