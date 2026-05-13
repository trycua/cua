//! Parity check for `set_agent_cursor_style`.

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

    // 1. Set gradient + bloom — text should reflect both.
    let r1 = req(&mut pipe,
        r##"{"method":"call","name":"set_agent_cursor_style","args":{"gradient_colors":["#FF6B6B","#FF8E53"],"bloom_color":"#A855F7"}}"##);
    let t1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    println!("Style resp: {t1:?}");
    assert!(t1.contains("✅ cursor style:") && t1.contains("gradient_colors=[#FF6B6B,#FF8E53]")
        && t1.contains("bloom_color=#A855F7"),
        "Set-style text doesn't match Swift format: {t1:?}");
    println!("Set OK");

    // 2. Revert all — should produce "✅ cursor style: reverted to default".
    let r2 = req(&mut pipe,
        r#"{"method":"call","name":"set_agent_cursor_style","args":{"gradient_colors":[],"bloom_color":"","image_path":""}}"#);
    let t2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    println!("Revert resp: {t2:?}");
    assert_eq!(t2, "✅ cursor style: reverted to default",
        "Revert text wrong: {t2:?}");
    println!("Revert OK");

    println!("\n✅ PASS: set_agent_cursor_style text format matches Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
