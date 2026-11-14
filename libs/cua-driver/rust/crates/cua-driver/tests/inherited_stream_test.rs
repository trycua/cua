// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.
#![cfg(target_os = "macos")]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Host(Child);
impl Drop for Host {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            if let Some(stdin) = self.0.stdin.take() {
                drop(stdin);
            }
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
#[ignore = "signed macOS fixture: run tests/runners/macos/inherited-stream.sh"]
fn signed_host_owns_the_only_embedded_channel() {
    let fixture = std::env::var("CUA_TEST_SIGNED_HOST").expect("signed host fixture");
    let substitute = std::env::var("CUA_TEST_SUBSTITUTE_HOST").expect("substitute fixture");
    let state = tempfile::tempdir().unwrap();
    let spawn = |host: &str, arguments: &[&str], extra: &[(&str, &str)]| {
        let mut command = Command::new(host);
        command
            .current_dir(state.path())
            .arg(env!("CARGO_BIN_EXE_cua-driver"))
            .arg("cua-driver")
            .args(arguments)
            .env_clear()
            .env("HOME", state.path())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("CUA_TELEMETRY", "0")
            .env("CUA_DRIVER_RS_PERMISSIONS_GATE", "0")
            .envs(extra.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut host = Host(command.spawn().unwrap());
        let mut output = BufReader::new(host.0.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let pid = line.trim().parse::<i32>().expect("child PID");
        (host, output, pid)
    };
    let private = [
        "__private-worker",
        "--generation",
        "fixture",
        "--transport-fd=3",
    ];
    let (mut host, mut output, pid) = spawn(&fixture, &private, &[]);
    let initialization = json!({"protocol_version":1,"request_id":0,"generation":"fixture","operation":"initialize","arguments":{
        "host_bundle_id":"org.example.embedded-fixture",
        "configured_driver":{"claude_code_compatibility":false,"authorization":{
            "allowed_modes":["standard"],"compatibility_mode":"standard",
            "compatibility_capability_manifest_path":null,"compatibility_bounded_manifest_path":null,
            "unrestricted_acknowledged":false,"max_session_ttl_seconds":60,"max_idle_ttl_seconds":30
        }}
    }});
    writeln!(host.0.stdin.as_mut().unwrap(), "{initialization}").unwrap();
    let mut line = String::new();
    let mut readable = libc::pollfd {
        fd: output.get_ref().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert!(
        unsafe { libc::poll(&mut readable, 1, 10_000) } > 0,
        "embedded response timeout"
    );
    let received = output.read_line(&mut line).unwrap();
    if received == 0 {
        let mut error = String::new();
        host.0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut error)
            .unwrap();
        panic!("embedded child must answer on the inherited FD: {error}");
    }
    let ready: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ready["ok"], true, "{ready}");
    assert_eq!(ready["result"]["pid"], pid);
    writeln!(
        host.0.stdin.as_mut().unwrap(),
        "{}",
        json!({"protocol_version":1,"request_id":1,"generation":"fixture","operation":"metadata"})
    )
    .unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let status: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(status["ok"], true);
    let listing = Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &pid.to_string(), "-U", "-Fnt"])
        .output()
        .unwrap();
    assert!(listing.status.success(), "socket inventory must succeed");
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(
        listing.lines().any(|line| line == "tunix"),
        "connected Unix stream must be observed"
    );
    assert!(
        !listing.lines().any(|line| line
            .strip_prefix('n')
            .is_some_and(|name| name.starts_with('/'))),
        "no pathname endpoint"
    );
    drop(host.0.stdin.take());
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = host.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < until, "child must exit on channel EOF");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    for (parent, arguments, extra, refusal) in [
        (
            substitute.as_str(),
            private.to_vec(),
            vec![("CUA_DRIVER_EMBEDDED_PARENT_REQUIREMENT", "true")],
            "untrusted embedded parent",
        ),
        (
            fixture.as_str(),
            vec![
                "__private-worker",
                "--generation",
                "fixture",
                "--transport-fd=99",
            ],
            vec![],
            "invalid inherited stream",
        ),
        (
            fixture.as_str(),
            vec!["__private-worker", "--generation", "fixture"],
            vec![],
            "private embedded entry required",
        ),
        (
            fixture.as_str(),
            vec!["serve", "--socket", "forbidden.sock"],
            vec![],
            "private embedded entry required",
        ),
        (
            fixture.as_str(),
            vec!["mcp", "--direct"],
            vec![],
            "private embedded entry required",
        ),
        (
            fixture.as_str(),
            private.to_vec(),
            vec![("CUA_DRIVER_RS_MCP_HTTP_PORT", "9411")],
            "alternate embedded endpoint",
        ),
        (
            fixture.as_str(),
            private.to_vec(),
            vec![("CUA_DRIVER_ENVELOPE_HTTP_PORT", "9412")],
            "alternate embedded endpoint",
        ),
    ] {
        let (mut child, _, _) = spawn(parent, &arguments, &extra);
        drop(child.0.stdin.take());
        let until = Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < until, "refused entry must exit");
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut error = String::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut error)
            .unwrap();
        assert_eq!(status.code(), Some(64));
        assert!(error.contains(refusal), "expected {refusal}, got {error}");
    }
}
