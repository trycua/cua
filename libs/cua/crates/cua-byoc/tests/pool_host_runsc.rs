// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The gVisor registration a pool host boots with ([`RUNSC_ARGS`]), on a
//! real Docker + runsc: the desktop image's supervisord can create its
//! socket in the gofer-backed root filesystem (`--host-uds=create`), and a
//! Space still cannot connect to a socket on the host, even one in a
//! directory it is given.
//!
//! Opt-in (it registers a Docker runtime, needs root, Docker and runsc):
//! `CUA_TEST_POOL_RUNSC=1 cargo test -p cua-byoc --test pool_host_runsc -- --ignored`.

use std::io::{BufRead, Write};
use std::process::Command;

use cua_byoc::hosts::RUNSC_ARGS;

const RUNTIME: &str = "cua-test-pool-runsc";
const IMAGE: &str = "python:3.12-alpine";

fn sh(cmd: &str, args: &[&str]) -> (bool, String) {
    let o = Command::new(cmd).args(args).output().expect(cmd);
    let mut s = String::from_utf8_lossy(&o.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&o.stderr));
    (o.status.success(), s)
}

struct Registered;

impl Drop for Registered {
    fn drop(&mut self) {
        let _ = sh("runsc", &["uninstall", "--runtime", RUNTIME]);
        let _ = sh("systemctl", &["reload", "docker"]);
    }
}

#[test]
#[ignore = "needs root, Docker and runsc (CUA_TEST_POOL_RUNSC=1)"]
fn a_space_on_a_pool_host_creates_its_sockets_but_never_reaches_the_hosts() {
    if std::env::var("CUA_TEST_POOL_RUNSC").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_TEST_POOL_RUNSC=1");
        return;
    }
    let mut install = vec!["install", "--runtime", RUNTIME, "--"];
    install.extend(RUNSC_ARGS.split_whitespace());
    let (ok, out) = sh("runsc", &install);
    assert!(ok, "runsc install: {out}");
    let _registered = Registered;
    assert!(sh("systemctl", &["reload", "docker"]).0);
    std::thread::sleep(std::time::Duration::from_secs(2));

    // A host service listening on a Unix socket in a directory the Space
    // is given (a bind mount).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
        for c in listener.incoming().flatten() {
            let mut c = c;
            let _ = c.write_all(b"HOST-SECRET\n");
        }
    });
    let mut probe = std::os::unix::net::UnixStream::connect(&path).unwrap();
    let mut line = String::new();
    std::io::BufReader::new(&mut probe)
        .read_line(&mut line)
        .unwrap();
    assert_eq!(
        line, "HOST-SECRET\n",
        "the host service answers on the host"
    );

    let script = r#"
import socket
s = socket.socket(socket.AF_UNIX)
try:
    s.connect("/h/host.sock"); print("CONNECTED", s.recv(64))
except OSError as e:
    print("refused", e.errno)
t = socket.socket(socket.AF_UNIX); t.bind("/run/own.sock"); t.listen(1); print("own-socket-ok")
"#;
    let mount = format!("{}:/h", dir.path().display());
    let (ok, out) = sh(
        "docker",
        &[
            "run",
            "--rm",
            "--runtime",
            RUNTIME,
            "-v",
            &mount,
            IMAGE,
            "python3",
            "-c",
            script,
        ],
    );
    assert!(ok, "docker run: {out}");
    // Its own sockets work (supervisord's /run/supervisor.sock)...
    assert!(out.contains("own-socket-ok"), "{out}");
    // ...and the host's stay out of reach.
    assert!(
        !out.contains("CONNECTED"),
        "a Space reached a host socket: {out}"
    );
    assert!(out.contains("refused"), "{out}");
}
