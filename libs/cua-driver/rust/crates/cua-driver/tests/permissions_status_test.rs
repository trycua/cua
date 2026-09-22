//! Exercise the real CLI against an isolated, read-only mock daemon.
//! HOME isolates Unix sockets; Windows named pipes need a separate fixture.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn status(reply: Option<Value>, json_output: bool) -> String {
    // Keep the socket path below macOS's Unix-domain path limit.
    let home = tempfile::tempdir_in("/tmp").unwrap();
    #[cfg(target_os = "macos")]
    let directory = home.path().join("Library/Caches/cua-driver");
    #[cfg(not(target_os = "macos"))]
    let directory = home.path().join(".cache/cua-driver");
    std::fs::create_dir_all(&directory).unwrap();
    let server = reply.map(|reply| {
        let listener = UnixListener::bind(directory.join("cua-driver.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            for method in ["list", "call"] {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "CLI never sent {method}");
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["method"], method);
                let response = if method == "list" {
                    json!({"ok": true, "result": []})
                } else {
                    assert_eq!(request["name"], "check_permissions");
                    assert_eq!(request["args"]["prompt"], false);
                    reply.clone()
                };
                writeln!(stream, "{response}").unwrap();
            }
        })
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .args(["permissions", "status"])
        .env("HOME", home.path())
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false");
    if json_output {
        command.arg("--json");
    }
    let output = command.output().unwrap();
    if let Some(server) = server {
        server.join().unwrap();
    }
    assert!(output.status.success(), "{:?}", output);
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn permissions_status_unknown_preserves_socket_liveness() {
    let cases = [
        None,
        Some(json!({"ok": false, "error": "permissions_pending", "exit_code": 1})),
        Some(json!({"ok": true, "result": {"structuredContent": {
            "accessibility": true,
            "screen_recording": true,
            "source": {"attribution": "embedded-host"}
        }}})),
    ];
    for reply in cases {
        let listening = reply.is_some();
        let payload: Value = serde_json::from_str(&status(reply.clone(), true)).unwrap();
        assert_eq!(payload["daemon_running"], listening);
        assert_eq!(payload["status"], "unknown");
        assert!(payload.get("accessibility").is_none());
        assert!(payload.get("screen_recording").is_none());
        let reason = payload["reason"].as_str().unwrap();
        assert!(reason.contains("permissions grant"));
        assert_eq!(reason.contains("daemon is listening"), listening);
        assert_eq!(reason.contains("no CuaDriver daemon"), !listening);

        let text = status(reply, false);
        assert!(text.contains("unknown"));
        assert!(text.contains("permissions grant"));
        assert_eq!(text.contains("start the daemon"), !listening);
        assert_eq!(text.contains("daemon is running, but"), listening);
    }
}

#[test]
fn permissions_status_success_preserves_fields_and_reports_running() {
    // Both macOS attribution and the source-less non-TCC response are trusted.
    for source in [None, Some(json!({"attribution": "driver-daemon"}))] {
        let mut structured = json!({"accessibility": true, "screen_recording": false});
        if let Some(source) = source {
            structured["source"] = source;
        }
        let reply = json!({"ok": true, "result": {"structuredContent": structured}});
        let payload: Value = serde_json::from_str(&status(Some(reply.clone()), true)).unwrap();
        structured["daemon_running"] = json!(true);
        assert_eq!(payload, structured);
        let text = status(Some(reply), false);
        assert!(text.contains("Accessibility:    ✅ granted"));
        assert!(text.contains("Screen Recording: ❌ not granted"));
        assert!(!text.contains("start the daemon"));
    }
}
