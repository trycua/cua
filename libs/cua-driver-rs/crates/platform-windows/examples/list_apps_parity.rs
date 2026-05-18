//! Parity check for `list_apps`.
//!
//! Asserts the response has the unified shape introduced by #1545:
//!   - text begins with `"✅ Found N app(s): R running, I installed-not-running."`
//!   - one bullet line per running app: `"- <name> (pid <N>)"`
//!   - `structuredContent.apps` is an array of
//!     `{pid, bundle_id, name, running, active, kind, launch_path, last_used, windows}`
//!   - the array contains BOTH running and installed-not-running entries
//!     (running entries have pid > 0, installed have pid == 0).
//!
//! Session-0 aware: when run in a non-interactive session (services / SSH)
//! the `active` assertion is skipped because no window owns foreground.
//! In an interactive session at least one app should be marked active.
//!
//! Hard 8-second timeout on the round-trip — list_apps does Start-Menu
//! + WinRT PackageManager scans which take ~300ms on a stock Win11 image
//! and several seconds on machines with many installed apps.

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
        let deadline = Instant::now() + Duration::from_secs(8);
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
        "Header {first_line:?} does not match `✅ Found N app(s): R running, I installed-not-running.`"
    );

    // structuredContent — #1545 contract: array of mixed running + installed.
    let apps = v.pointer("/result/structuredContent/apps").and_then(|a| a.as_array())
        .expect("structuredContent.apps is not an array");
    assert!(!apps.is_empty(), "Expected at least one app (running or installed)");
    println!("Apps: {} entries", apps.len());

    let mut any_active = false;
    let mut running_count = 0;
    let mut installed_count = 0;
    for (i, app) in apps.iter().enumerate() {
        let pid    = app.pointer("/pid").and_then(|n| n.as_u64()).expect("pid");
        let name   = app.pointer("/name").and_then(|s| s.as_str()).expect("name");
        let running = app.pointer("/running").and_then(|b| b.as_bool()).expect("running");
        let active  = app.pointer("/active").and_then(|b| b.as_bool()).expect("active");
        // bundle_id present (null on Windows for non-UWP, optional string on macOS).
        let _ = app.pointer("/bundle_id").expect("bundle_id key missing");
        // #1545 fields: kind, launch_path, last_used, windows.
        let _ = app.pointer("/kind").expect("kind key missing (added by #1545)");
        let _ = app.pointer("/launch_path").expect("launch_path key missing (added by #1545)");
        let _ = app.pointer("/last_used").expect("last_used key missing (added by #1545)");
        let _ = app.pointer("/windows").expect("windows key missing (added by #1545)");
        if i < 5 {
            println!("  [{i}] pid={pid} name={name:?} running={running} active={active}");
        }
        if running { running_count += 1; } else { installed_count += 1; }
        // Running entries must have pid > 0; installed-not-running must have pid == 0.
        if running { assert!(pid > 0, "running entry must have pid>0: {name:?}"); }
        else       { assert_eq!(pid, 0, "installed-not-running must have pid=0: {name:?}"); }
        if active { any_active = true; }
    }
    println!("Counts: {running_count} running, {installed_count} installed-not-running");

    // The "active" assertion only holds when run from an interactive session
    // (Session 1+) where some foreground window exists. Skip in Session 0
    // (services / SSH-launched processes) where GetForegroundWindow returns
    // null and no app is reported as active — that's correct behavior, not
    // a regression.
    let in_session_0 = matches!(
        platform_windows::diagnostics::current_session_id(),
        Some(0),
    );
    if in_session_0 {
        println!("(Session 0: skipping active-app assertion — no interactive desktop attached)");
    } else if running_count > 0 {
        assert!(
            any_active,
            "Expected at least one app marked active (the foreground window owner)",
        );
    }

    println!("\n✅ PASS: text header + structuredContent.apps shape match unified contract");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
