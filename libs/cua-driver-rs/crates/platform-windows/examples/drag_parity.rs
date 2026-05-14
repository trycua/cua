//! Parity check for `drag` error wording + text format.

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
        let deadline = Instant::now() + Duration::from_secs(6);
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

    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome")
    }).expect("no chrome");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();

    // 1. Missing coordinates.
    let r1 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"drag","args":{{"pid":{pid},"window_id":{wid},"from_x":10}}}}"#));
    let e1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert!(e1.contains("from_x, from_y, to_x, and to_y are all required"),
        "Missing-coords wording wrong: {e1:?}");
    println!("Missing-coords err OK");

    // 2. Real drag — short, won't break Chrome.
    let r2 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"drag","args":{{"pid":{pid},"window_id":{wid},"from_x":100,"from_y":100,"to_x":102,"to_y":102,"duration_ms":50,"steps":2}}}}"#));
    let t2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    println!("Drag resp: {t2:?}");
    assert!(t2.starts_with("✅ Posted drag to pid ")
        && t2.contains("from window-pixel (")
        && t2.contains("screen (")
        && t2.contains("in 50ms / 2 steps."),
        "Drag text format wrong: {t2:?}");

    println!("\n✅ PASS: drag error wording + Swift-shape text format verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
