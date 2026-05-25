//! Parity check for `scroll`.

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
        let mut buf: Vec<u8> = vec![0u8; 64 * 1024];
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

    // Missing direction error.
    let r1 = req(&mut pipe, r#"{"method":"call","name":"scroll","args":{"pid":1}}"#);
    let e1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert!(e1.contains("direction") && e1.to_lowercase().contains("required"),
        "Missing-direction error should mention direction + required: {e1:?}");
    println!("Missing-direction err OK");

    // element_index without window_id.
    let r2 = req(&mut pipe, r#"{"method":"call","name":"scroll","args":{"pid":1,"direction":"down","element_index":0}}"#);
    let e2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert!(e2.contains("window_id is required when element_index is used"),
        "Element-without-window wording wrong: {e2:?}");
    println!("Element-without-window err OK");

    // Real scroll against Chrome.
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome") || n.contains("notepad")
    }).expect("no target");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();
    let sc = format!(
        r#"{{"method":"call","name":"scroll","args":{{"pid":{pid},"window_id":{wid},"direction":"down","by":"page","amount":2}}}}"#);
    let r3 = req(&mut pipe, &sc);
    let t3 = extract_text(&serde_json::from_str(r3.trim()).unwrap());
    println!("Scroll resp: {t3:?}");
    assert!(t3.starts_with(&format!("✅ Scrolled pid {pid} down via 2× SB_PAGEDOWN message(s)")),
        "Scroll text doesn't match Swift-shape format: {t3:?}");

    println!("\n✅ PASS: scroll validation + Swift-shape text format verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
