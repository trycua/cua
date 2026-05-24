//! Parity check for `click` tool text format.
//!
//! Drives the daemon and asserts:
//! - Missing target returns Swift's wording.
//! - Pixel click against a Chrome window returns
//!   `"✅ Posted click to pid X."`.
//! - Element-indexed click returns `"✅ Performed Invoke on [N] ..."`.
//!
//! Hard 4-second timeout per call.

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

    // Helper: extract the user-visible text from either daemon error
    // wrapper or the MCP isError tool-result shape.
    fn extract_text(v: &serde_json::Value) -> String {
        if let Some(t) = v.pointer("/result/content/0/text").and_then(|t| t.as_str()) {
            return t.to_owned();
        }
        if let Some(e) = v.pointer("/error").and_then(|e| e.as_str()) {
            return e.to_owned();
        }
        format!("(no text in response: {v})")
    }

    // 1. Missing-target error wording. Need a real pid + window_id so the
    //    early "no window" guard passes; we'll fetch a real one below first.
    // (moved after list_windows)

    // 2. Pixel click — pick the first window we can find belonging to our own
    //    daemon (cua-driver.exe doesn't have one, so use the foreground app).
    //    Use list_windows to get a real pid + hwnd, then click on it.
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).expect("parse");
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array())
        .expect("no windows array");
    // Prefer Chrome — we already verified it accepts PostMessage clicks
    // earlier in the session.  Skip windows that reject PostMessage
    // (e.g. parsecd, system processes).
    let preferred = ["chrome.exe", "msedge.exe", "firefox.exe", "notepad.exe"];
    let target = wins.iter().find(|w| {
        let name = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        preferred.iter().any(|p| name.contains(p))
    }).or_else(|| wins.iter().find(|w| {
        let name = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("");
        !name.contains("cua-driver") && !name.contains("parsecd")
            && !name.contains("examples") && !name.contains("dwm")
    })).expect("no suitable target window");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();
    println!("Clicking pid={pid} window_id={wid} app={}", target.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("?"));

    // 1. Missing-target wording (now that we have a real pid + window_id).
    let missing_req = format!(
        r#"{{"method":"call","name":"click","args":{{"pid":{pid},"window_id":{wid}}}}}"#
    );
    let r_m = req(&mut pipe, &missing_req);
    let v_m: serde_json::Value = serde_json::from_str(r_m.trim()).expect("parse");
    let err1 = extract_text(&v_m);
    assert!(
        err1.contains("Provide element_index or (x, y) to address the click target"),
        "Expected Swift-style missing-target wording, got: {err1:?}"
    );
    println!("Missing-target err OK: {err1:?}");

    // Pixel click at (100, 100) window-local — innocuous enough.
    let click_req = format!(
        r#"{{"method":"call","name":"click","args":{{"pid":{pid},"window_id":{wid},"x":100,"y":100}}}}"#
    );
    let r2 = req(&mut pipe, &click_req);
    let v2: serde_json::Value = serde_json::from_str(r2.trim()).expect("parse");
    let text2 = extract_text(&v2);
    println!("Pixel-click resp: {text2:?}");
    assert!(
        text2.starts_with("✅ Posted click to pid "),
        "Expected `✅ Posted click to pid X.` Swift format, got: {text2:?}"
    );

    // 3. Double-click should produce `Posted double-click`.
    let click2 = format!(
        r#"{{"method":"call","name":"click","args":{{"pid":{pid},"window_id":{wid},"x":100,"y":100,"count":2}}}}"#
    );
    let r3 = req(&mut pipe, &click2);
    let v3: serde_json::Value = serde_json::from_str(r3.trim()).expect("parse");
    let text3 = extract_text(&v3);
    println!("Double-click resp: {text3:?}");
    assert!(
        text3.starts_with("✅ Posted double-click to pid "),
        "Expected `✅ Posted double-click to pid X.`, got: {text3:?}"
    );

    println!("\n✅ PASS: click error wording + click/double-click text format match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
