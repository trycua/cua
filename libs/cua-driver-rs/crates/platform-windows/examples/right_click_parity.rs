//! Parity check for `right_click` text format + validation guards.

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

    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome") || n.contains("notepad")
    }).expect("no target");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();

    // 1. Missing target.
    let r1 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"right_click","args":{{"pid":{pid},"window_id":{wid}}}}}"#));
    let v1: serde_json::Value = serde_json::from_str(r1.trim()).unwrap();
    assert!(extract_text(&v1).contains("Provide element_index or (x, y) to address the right-click target"));
    println!("Missing-target err OK");

    // 2. Partial xy.
    let r2 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"right_click","args":{{"pid":{pid},"window_id":{wid},"x":50}}}}"#));
    assert!(extract_text(&serde_json::from_str(r2.trim()).unwrap()).contains("Provide both x and y together"));
    println!("Partial-xy err OK");

    // 3. Both modes.
    let r3 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"right_click","args":{{"pid":{pid},"window_id":{wid},"element_index":0,"x":50,"y":50}}}}"#));
    assert!(extract_text(&serde_json::from_str(r3.trim()).unwrap()).contains("Provide either element_index or (x, y), not both"));
    println!("Both-modes err OK");

    // 4. element_index without window_id.
    let r4 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"right_click","args":{{"pid":{pid},"element_index":0}}}}"#));
    assert!(extract_text(&serde_json::from_str(r4.trim()).unwrap()).contains("window_id is required when element_index is used"));
    println!("Element-without-window err OK");

    // 5. Real pixel right-click.
    let rc = format!(
        r#"{{"method":"call","name":"right_click","args":{{"pid":{pid},"window_id":{wid},"x":300,"y":300}}}}"#);
    let r5 = req(&mut pipe, &rc);
    let v5: serde_json::Value = serde_json::from_str(r5.trim()).unwrap();
    let t5 = extract_text(&v5);
    println!("Right-click resp: {t5:?}");
    assert!(t5.starts_with("✅ Posted right-click to pid ")
        && t5.contains(" at window-pixel (")
        && t5.contains(" → screen-point ("),
        "Pixel-path text doesn't match Swift format: {t5:?}");

    println!("\n✅ PASS: right_click validation + pixel-path text match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
