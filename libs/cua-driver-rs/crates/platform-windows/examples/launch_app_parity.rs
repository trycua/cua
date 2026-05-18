//! Parity check for `launch_app`.
//!
//! Launches Notepad (safe target) in background, asserts:
//! - Text starts with `"✅ Launched ... (pid <N>) in background."`
//! - structuredContent has `{pid, bundle_id, name, running, active, windows}`
//! - `active: false` (Swift's background-launch invariant)
//! - At least one entry in `windows` array OR `windows: []` if the window
//!   didn't materialize within the 5×200ms retry budget.
//!
//! On Win11 we additionally try the packaged-app path: an explicit AUMID
//! via `bundle_id` should return a non-stub pid (i.e. the real Notepad
//! UWP process). The legacy `notepad.exe` smoke-test above stays for
//! Win10 / non-packaged-Notepad coverage; both should pass on Win11
//! because the plain `name` path now goes through the AppsFolder
//! lookup first.
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

    assert!(pid > 0, "pid must be captured (ShellExecuteEx pid or packaged-app pid)");
    assert!(running, "running must be true");
    assert!(!active, "active must be false (background launch — Swift's invariant)");
    // bundle_id is either null (plain Win32 launch — Win10 / unpackaged
    // Notepad) or an AUMID string (Win11 — `notepad` resolved through
    // shell:AppsFolder to the packaged Notepad). Both are correct.
    assert!(
        bundle_id.is_null()
            || bundle_id.as_str().map(|s| s.contains('!')).unwrap_or(false),
        "bundle_id must be null OR an AUMID (`pkg!app`); got {bundle_id}"
    );

    // Cleanup: kill notepad.
    std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().ok();

    // 3. AUMID path (Win11 only — skipped silently on Win10 since the
    //    AUMID won't be installed there).
    let aumid = "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App";
    let resp_aumid = req(
        &mut pipe,
        &format!(r#"{{"method":"call","name":"launch_app","args":{{"bundle_id":"{aumid}"}}}}"#),
    );
    let v_aumid: serde_json::Value = serde_json::from_str(resp_aumid.trim()).expect("parse");
    let pid_aumid = v_aumid
        .pointer("/result/structuredContent/pid")
        .and_then(|n| n.as_u64());
    let bundle_id_aumid = v_aumid.pointer("/result/structuredContent/bundle_id");

    // Always reap the pid first — even if the bundle_id round-trip
    // check below fails, leaving Notepad running across runs would
    // pollute later parity invocations.
    if let Some(p) = pid_aumid.filter(|p| *p > 0) {
        std::process::Command::new("taskkill")
            .args(["/PID", &p.to_string(), "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().ok();
    }

    match bundle_id_aumid {
        Some(b) => {
            let p = pid_aumid.expect("pid must be present when bundle_id is");
            assert!(p > 0, "AUMID launch returned bundle_id but pid={p}");
            println!("AUMID launch returned pid={p}, bundle_id={b}");
            assert_eq!(
                b.as_str(),
                Some(aumid),
                "bundle_id must round-trip the AUMID passed in"
            );
        }
        None => {
            // No bundle_id in structuredContent — the AUMID launch
            // returned an error (the daemon serialises tool errors as
            // {ok:false, error:"…"}, no `result` field). Inspect the
            // error text: the *only* acceptable cause for skipping is
            // "this AUMID is not installed on this host" (Win10, or a
            // stripped Win11 image without packaged Notepad). Anything
            // else — COM init failure, IApplicationActivationManager
            // broken, packaging metadata corrupt — is a real regression
            // that must surface, not be swallowed.
            let err_text = v_aumid
                .pointer("/error")
                .and_then(|e| e.as_str())
                .or_else(|| v_aumid.pointer("/result/content/0/text").and_then(|t| t.as_str()))
                .unwrap_or("");
            let err_lower = err_text.to_lowercase();
            // Windows surfaces "AUMID not registered for current user"
            // as E_INVALIDARG (HRESULT 0x80070057, "The parameter is
            // incorrect."). That HRESULT *is* the canonical sentinel for
            // "not installed" from ActivateApplication. Also accept the
            // human-readable substrings in case a wrapper rephrases.
            let looks_like_not_installed = err_lower.contains("not installed")
                || err_lower.contains("not found")
                || err_lower.contains("not registered")
                || err_text.contains("0x80070057")
                || err_text.contains("E_INVALIDARG")
                || err_lower.contains("the parameter is incorrect");
            // Session-0 short-circuit: UWP activation in non-interactive
            // sessions (services / SSH) fails fast with a descriptive
            // error rather than hanging. This is correct behavior, not a
            // regression — accept it the same way we accept "not installed".
            let looks_like_session_0 = err_lower.contains("interactive session")
                || err_lower.contains("session 0");
            assert!(
                looks_like_not_installed || looks_like_session_0,
                "AUMID launch of {aumid:?} failed with an unexpected error \
                 (not 'not installed' or 'Session 0' signal): {err_text:?}"
            );
            if looks_like_session_0 {
                println!("AUMID path: skipped — running in Session 0 (UWP needs interactive desktop) ({err_text:?})");
            } else {
                println!("AUMID path: packaged Notepad not installed on this host; skipped ({err_text:?})");
            }
        }
    }

    println!("\n✅ PASS: launch_app text format, pid capture, and Swift invariants verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
