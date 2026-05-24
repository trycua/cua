//! Parity check for `double_click` text format + error wording.

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

    // Find a Chrome window to click on (same approach as click_parity).
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome") || n.contains("notepad")
    }).expect("no chrome/notepad");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();

    // 1. Missing target with valid pid+window_id.
    let r1 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"double_click","args":{{"pid":{pid},"window_id":{wid}}}}}"#));
    let v1: serde_json::Value = serde_json::from_str(r1.trim()).unwrap();
    let e1 = extract_text(&v1);
    assert!(e1.contains("Provide element_index or (x, y) to address the double-click target"),
        "Missing-target wording wrong: {e1:?}");
    println!("Missing-target err OK");

    // 2. Partial xy: x without y.
    let r2 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"double_click","args":{{"pid":{pid},"window_id":{wid},"x":100}}}}"#));
    let v2: serde_json::Value = serde_json::from_str(r2.trim()).unwrap();
    let e2 = extract_text(&v2);
    assert!(e2.contains("Provide both x and y together"),
        "Partial-xy wording wrong: {e2:?}");
    println!("Partial-xy err OK");

    // 3. Both element_index AND xy.
    let r3 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"double_click","args":{{"pid":{pid},"window_id":{wid},"element_index":0,"x":100,"y":100}}}}"#));
    let v3: serde_json::Value = serde_json::from_str(r3.trim()).unwrap();
    let e3 = extract_text(&v3);
    assert!(e3.contains("Provide either element_index or (x, y), not both"),
        "Both-modes wording wrong: {e3:?}");
    println!("Both-modes err OK");

    // 4. element_index without window_id.
    let r4 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"double_click","args":{{"pid":{pid},"element_index":0}}}}"#));
    let v4: serde_json::Value = serde_json::from_str(r4.trim()).unwrap();
    let e4 = extract_text(&v4);
    assert!(e4.contains("window_id is required when element_index is used"),
        "Element-without-window wording wrong: {e4:?}");
    println!("Element-without-window err OK");

    // 5. Real pixel double-click.
    let dc_req = format!(
        r#"{{"method":"call","name":"double_click","args":{{"pid":{pid},"window_id":{wid},"x":300,"y":300}}}}"#);
    let r5 = req(&mut pipe, &dc_req);
    let v5: serde_json::Value = serde_json::from_str(r5.trim()).unwrap();
    let t5 = extract_text(&v5);
    println!("Double-click resp: {t5:?}");
    assert!(t5.starts_with("✅ Posted double-click to pid ")
        && t5.contains(" at window-pixel (")
        && t5.contains(" → screen-point ("),
        "Pixel-path text doesn't match Swift format: {t5:?}");

    println!("\n✅ PASS: double_click validation + pixel-path text match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
