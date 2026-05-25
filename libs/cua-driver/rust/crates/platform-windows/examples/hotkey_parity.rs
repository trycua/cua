//! Parity check for `hotkey`.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
fn main() {
    let mut pipe = std::fs::OpenOptions::new()
        .read(true).write(true)
        .open(r"\\.\pipe\cua-driver")
        .expect("open pipe");

    fn req(p: &mut std::fs::File, json: &str) -> String {
        p.write_all(format!("{json}\n").as_bytes()).unwrap();
        p.flush().ok();
        let mut out = Vec::new();
        let mut buf = [0u8; 65536];
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if Instant::now() > deadline { panic!("timeout"); }
            let n = p.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            out.extend_from_slice(&buf[..n]);
            if out.contains(&b'\n') { break; }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    fn extract_text(v: &serde_json::Value) -> String {
        v.pointer("/result/content/0/text").and_then(|t| t.as_str())
            .or_else(|| v.pointer("/error").and_then(|e| e.as_str()))
            .map(|s| s.to_owned()).unwrap_or_default()
    }

    // 1. Missing pid — daemon's schema validator catches this before the
    //    tool runs; either schema-layer or tool-layer wording is acceptable
    //    as long as `pid` is named in the error.
    let r1 = req(&mut pipe, r#"{"method":"call","name":"hotkey","args":{}}"#);
    let v1: serde_json::Value = serde_json::from_str(r1.trim()).unwrap();
    let e1 = extract_text(&v1);
    assert!(e1.to_lowercase().contains("pid") && e1.to_lowercase().contains("required"),
        "Missing-pid error should mention pid + required, got: {e1:?}");
    println!("Missing-pid err OK: {e1:?}");

    // 2. Missing keys.
    let r2 = req(&mut pipe, r#"{"method":"call","name":"hotkey","args":{"pid":1}}"#);
    let v2: serde_json::Value = serde_json::from_str(r2.trim()).unwrap();
    let e2 = extract_text(&v2);
    assert!(e2.to_lowercase().contains("keys") && e2.to_lowercase().contains("required"),
        "Missing-keys error should mention keys + required, got: {e2:?}");
    println!("Missing-keys err OK: {e2:?}");

    // 3. Real hotkey against Chrome — ctrl+end (scroll to bottom; harmless).
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome") || n.contains("notepad")
    }).expect("no target");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();
    let hk = format!(
        r#"{{"method":"call","name":"hotkey","args":{{"pid":{pid},"window_id":{wid},"keys":["ctrl","end"]}}}}"#);
    let r3 = req(&mut pipe, &hk);
    let v3: serde_json::Value = serde_json::from_str(r3.trim()).unwrap();
    let t3 = extract_text(&v3);
    println!("Hotkey resp: {t3:?}");
    assert!(t3.starts_with("✅ Pressed ctrl+end on pid "),
        "Expected Swift `✅ Pressed K1+K2 on pid X.`, got: {t3:?}");

    println!("\n✅ PASS: hotkey error wording + text format match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
