//! Parity check for `screenshot`.

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
        // Heap-allocate the 64 KiB read buffer — a stack array of this size
        // overflows the default thread stack on Windows debug builds.
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

    // Window screenshot: find Chrome.
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome")
    }).expect("no chrome");
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();

    // 1. Window screenshot.
    let ss = format!(r#"{{"method":"call","name":"screenshot","args":{{"window_id":{wid}}}}}"#);
    let r1 = req(&mut pipe, &ss);
    let v1: serde_json::Value = serde_json::from_str(r1.trim()).unwrap();
    // Last content entry is text; first should be image.
    let content = v1.pointer("/result/content").and_then(|c| c.as_array()).unwrap();
    assert!(content.iter().any(|c| c.get("type").and_then(|t| t.as_str()) == Some("image")),
        "Response should contain an image content block");
    let text = content.iter().find_map(|c| {
        if c.get("type").and_then(|t| t.as_str()) == Some("text") {
            c.get("text").and_then(|t| t.as_str())
        } else { None }
    }).unwrap_or("");
    println!("Window resp: {text:?}");
    assert!(text.starts_with("✅ Window screenshot — ") && text.contains(&format!("[window_id: {wid}]")),
        "Window text doesn't match Swift format: {text:?}");
    let sc = v1.pointer("/result/structuredContent").unwrap();
    assert!(sc.get("width").is_some() && sc.get("height").is_some() && sc.get("format").is_some(),
        "structuredContent missing width/height/format");

    // 2. JPEG format.
    let ss2 = format!(
        r#"{{"method":"call","name":"screenshot","args":{{"window_id":{wid},"format":"jpeg","quality":50}}}}"#);
    let r2 = req(&mut pipe, &ss2);
    let v2: serde_json::Value = serde_json::from_str(r2.trim()).unwrap();
    let c2 = v2.pointer("/result/content").and_then(|c| c.as_array()).unwrap();
    let img = c2.iter().find(|c| c.get("type").and_then(|t| t.as_str()) == Some("image")).unwrap();
    let mime = img.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
    assert_eq!(mime, "image/jpeg", "jpeg format should set mimeType image/jpeg, got: {mime:?}");
    let fmt = v2.pointer("/result/structuredContent/format").and_then(|f| f.as_str()).unwrap_or("");
    assert_eq!(fmt, "jpeg", "structuredContent.format should be jpeg");

    println!("\n✅ PASS: screenshot text format + image + structuredContent + jpeg path verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
