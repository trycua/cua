//! Parity check for `replay_trajectory` error wording.

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

    // 1. Empty `dir` string (schema-layer required, but should also be checked at tool layer).
    let r1 = req(&mut pipe, r#"{"method":"call","name":"replay_trajectory","args":{"dir":""}}"#);
    let e1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert!(e1.contains("`dir`") && e1.to_lowercase().contains("required"),
        "Empty-dir wording wrong: {e1:?}");
    println!("Empty-dir err OK");

    // 2. Nonexistent directory.
    let r2 = req(&mut pipe, r#"{"method":"call","name":"replay_trajectory","args":{"dir":"C:\\does\\not\\exist"}}"#);
    let e2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert!(e2.contains("Trajectory directory does not exist"),
        "Nonexistent-dir wording wrong: {e2:?}");
    println!("Nonexistent-dir err OK");

    println!("\n✅ PASS: replay_trajectory error paths match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
