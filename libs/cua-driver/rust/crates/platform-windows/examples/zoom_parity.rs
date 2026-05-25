//! Parity check for `zoom` error wording.

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

    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome")
    }).expect("no chrome");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();

    // 1. Invalid region (x2 ≤ x1).
    let r1 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"zoom","args":{{"pid":{pid},"window_id":{wid},"x1":100,"y1":100,"x2":50,"y2":200}}}}"#));
    let e1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert!(e1.contains("Invalid region: x2 must be > x1 and y2 must be > y1"),
        "Invalid-region wording wrong: {e1:?}");
    println!("Invalid-region err OK");

    // 2. Region too wide.
    let r2 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"zoom","args":{{"pid":{pid},"window_id":{wid},"x1":0,"y1":0,"x2":600,"y2":100}}}}"#));
    let e2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert!(e2.contains("Zoom region too wide") && e2.contains("> 500 px max"),
        "Too-wide wording wrong: {e2:?}");
    println!("Too-wide err OK");

    // 3. Real zoom — small region.
    let r3 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"zoom","args":{{"pid":{pid},"window_id":{wid},"x1":50,"y1":50,"x2":150,"y2":150}}}}"#));
    let v3: serde_json::Value = serde_json::from_str(r3.trim()).unwrap();
    let content = v3.pointer("/result/content").and_then(|c| c.as_array()).unwrap();
    let img_present = content.iter().any(|c| c.get("type").and_then(|t| t.as_str()) == Some("image"));
    let text = content.iter().find_map(|c| {
        if c.get("type").and_then(|t| t.as_str()) == Some("text") {
            c.get("text").and_then(|t| t.as_str())
        } else { None }
    }).unwrap_or("");
    assert!(img_present, "zoom response should include image content");
    assert!(text.starts_with("✅ Zoomed region captured at native resolution.")
        && text.contains("from_zoom=true"),
        "zoom text doesn't match Swift format: {text:?}");
    println!("Real zoom OK");

    println!("\n✅ PASS: zoom error wording + Swift text format verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
