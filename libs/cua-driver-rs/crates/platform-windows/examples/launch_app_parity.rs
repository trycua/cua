//! Parity check for `launch_app`.
//!
//! Launches Notepad (safe target) in background, asserts:
//! - Text starts with `"✅ Launched ... (pid <N>) in background."`
//! - structuredContent has `{pid, bundle_id, name, running, active, windows}`
//! - `active: false` (Swift's background-launch invariant)
//! - At least one entry in `windows` array OR `windows: []` if the window
//!   didn't materialize within the 5×200ms retry budget.
//!
//! Cleans up: terminates the launched pid before exit.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
fn main() {
    let mut pipe = std::fs::OpenOptions::new()
        .read(true).write(true)
        .open(r"\\.\pipe\cua-driver")
        .expect("open pipe — start daemon first");

    fn req(p: &mut std::fs::File, json: &str) -> String {
        p.write_all(format!("{json}\n").as_bytes()).unwrap();
        p.flush().ok();
        let mut out = Vec::new();
        let mut buf = [0u8; 65536];
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if Instant::now() > deadline { panic!("response timeout"); }
            let n = p.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            out.extend_from_slice(&buf[..n]);
            if out.contains(&b'\n') { break; }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    // 1. Missing target error
    let r0 = req(&mut pipe, r#"{"method":"call","name":"launch_app","args":{}}"#);
    let v0: serde_json::Value = serde_json::from_str(r0.trim()).expect("parse");
    let err0 = v0.pointer("/result/content/0/text").and_then(|t| t.as_str())
        .or_else(|| v0.pointer("/error").and_then(|e| e.as_str()))
        .unwrap_or("");
    assert!(
        err0.contains("Provide either bundle_id or name to identify the app to launch"),
        "Missing-target error doesn't match Swift wording: {err0:?}"
    );
    println!("Missing-target err OK: {err0:?}");

    // 2. Launch Notepad
    let resp = req(&mut pipe, r#"{"method":"call","name":"launch_app","args":{"name":"notepad.exe"}}"#);
    let v: serde_json::Value = serde_json::from_str(resp.trim()).expect("parse");
    let text = v.pointer("/result/content/0/text").and_then(|t| t.as_str()).expect("missing text");
    println!("Launch resp text:\n{text}");

    let header = text.lines().next().unwrap_or("");
    assert!(
        header.starts_with("✅ Launched ") && header.contains(" (pid ") && header.ends_with("in background."),
        "Header {header:?} doesn't match Swift format `✅ Launched <name> (pid X) in background.`"
    );

    let pid = v.pointer("/result/structuredContent/pid").and_then(|n| n.as_u64()).expect("pid");
    let name = v.pointer("/result/structuredContent/name").and_then(|s| s.as_str()).expect("name");
    let running = v.pointer("/result/structuredContent/running").and_then(|b| b.as_bool()).expect("running");
    let active  = v.pointer("/result/structuredContent/active").and_then(|b| b.as_bool()).expect("active");
    let windows = v.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).expect("windows");
    let bundle_id = v.pointer("/result/structuredContent/bundle_id").expect("bundle_id key");

    println!("Structured: pid={pid} name={name:?} running={running} active={active} windows={} bundle_id={bundle_id}", windows.len());

    assert!(pid > 0, "pid must be captured from ShellExecuteEx");
    assert!(running, "running must be true");
    assert!(!active, "active must be false (background launch — Swift's invariant)");
    assert!(bundle_id.is_null(), "bundle_id must be null on Windows");

    // Cleanup: kill notepad.
    std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().ok();

    println!("\n✅ PASS: launch_app text format, pid capture, and Swift invariants verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
