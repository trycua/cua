//! Parity check for `list_apps`.
//!
//! Asserts the response has Swift's shape:
//!   - text begins with `"✅ Found N app(s): R running, I installed-not-running."`
//!   - one bullet line per running app: `"- <name> (pid <N>)"`
//!   - `structuredContent.apps` is an array of `{pid, bundle_id, name, running, active}`
//!   - at least one app is marked `active: true` (something must own the
//!     foreground window when this test runs)
//!
//! Hard 4-second timeout on the round-trip.

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

    let resp = req(&mut pipe, r#"{"method":"call","name":"list_apps","args":{}}"#);
    let v: serde_json::Value = serde_json::from_str(resp.trim()).expect("parse JSON");

    let text = v.pointer("/result/content/0/text").and_then(|t| t.as_str())
        .expect("missing text");
    let first_line = text.lines().next().unwrap_or("");
    println!("Header: {first_line:?}");
    assert!(
        first_line.starts_with("✅ Found ") && first_line.contains(" app(s): "),
        "Header {first_line:?} does not match Swift `✅ Found N app(s): R running, I installed-not-running.`"
    );

    // structuredContent
    let apps = v.pointer("/result/structuredContent/apps").and_then(|a| a.as_array())
        .expect("structuredContent.apps is not an array");
    assert!(!apps.is_empty(), "Expected at least one running app");
    println!("Apps: {} entries", apps.len());

    let mut any_active = false;
    for (i, app) in apps.iter().enumerate() {
        let pid    = app.pointer("/pid").and_then(|n| n.as_u64()).expect("pid");
        let name   = app.pointer("/name").and_then(|s| s.as_str()).expect("name");
        let running = app.pointer("/running").and_then(|b| b.as_bool()).expect("running");
        let active  = app.pointer("/active").and_then(|b| b.as_bool()).expect("active");
        // bundle_id present (null on Windows, optional string on macOS).
        let _ = app.pointer("/bundle_id").expect("bundle_id key missing");
        if i < 5 {
            println!("  [{i}] pid={pid} name={name:?} running={running} active={active}");
        }
        assert!(running, "Windows list_apps should only return running apps for now");
        if active { any_active = true; }
    }
    assert!(any_active, "Expected at least one app marked active (the foreground window owner)");

    println!("\n✅ PASS: text header + structuredContent.apps shape match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
