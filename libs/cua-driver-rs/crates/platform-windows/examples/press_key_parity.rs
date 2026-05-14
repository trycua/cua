//! Parity check for `press_key`.

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
            .map(|s| s.to_owned())
            .unwrap_or_else(|| format!("(no text: {v})"))
    }

    // 1. Missing key error.
    let r1 = req(&mut pipe, r#"{"method":"call","name":"press_key","args":{"pid":1}}"#);
    let v1: serde_json::Value = serde_json::from_str(r1.trim()).unwrap();
    let err1 = extract_text(&v1);
    assert!(err1.contains("Missing required string field key"),
        "Expected Swift-style key error, got: {err1:?}");
    println!("Missing-key err OK: {err1:?}");

    // 2. element_index without window_id should error per Swift.
    let r2 = req(&mut pipe, r#"{"method":"call","name":"press_key","args":{"pid":1,"key":"a","element_index":0}}"#);
    let v2: serde_json::Value = serde_json::from_str(r2.trim()).unwrap();
    let err2 = extract_text(&v2);
    assert!(err2.contains("window_id is required when element_index is used"),
        "Expected Swift-style element-without-window error, got: {err2:?}");
    println!("Element-without-window err OK");

    // 3. Press a key against a real window.
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let preferred = ["chrome.exe", "notepad.exe"];
    let target = wins.iter().find(|w| {
        let name = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        preferred.iter().any(|p| name.contains(p))
    }).expect("no chrome/notepad");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();
    // Press End key (safe — scrolls to bottom of page; doesn't modify content).
    let press_req = format!(
        r#"{{"method":"call","name":"press_key","args":{{"pid":{pid},"window_id":{wid},"key":"end"}}}}"#);
    let r3 = req(&mut pipe, &press_req);
    let v3: serde_json::Value = serde_json::from_str(r3.trim()).unwrap();
    let text3 = extract_text(&v3);
    println!("Press resp: {text3:?}");
    assert!(text3.starts_with("✅ Pressed end on pid "),
        "Expected Swift format `✅ Pressed KEY on pid X.`, got: {text3:?}");

    println!("\n✅ PASS: press_key error wording + text format match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
