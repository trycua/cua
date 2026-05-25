//! Parity check for the `type_text_chars` → `type_text` deprecated alias.
//!
//! Swift's ToolRegistry.swift resolves `type_text_chars` to `type_text` at
//! invoke time AND emits a stderr deprecation warning. The aliased name
//! is NOT in `tools/list`. Rust now does the same.

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

    // 1. `tools/list` (or equivalent) should NOT include `type_text_chars`.
    //    The daemon's pipe has a `list` method that returns all registered
    //    tools — we look for the alias and assert it's absent.
    let r1 = req(&mut pipe, r#"{"method":"list"}"#);
    println!("list resp (first 1000 chars): {:?}", r1.chars().take(1000).collect::<String>());
    assert!(!r1.contains("\"type_text_chars\""),
        "type_text_chars should NOT appear in tools/list (Swift alias convention)");
    assert!(r1.contains("\"type_text\""),
        "type_text should appear in tools/list");
    println!("tools/list excludes type_text_chars ✓");

    // 2. Invoking `type_text_chars` should still work — resolves to `type_text`.
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome") || n.contains("notepad")
    }).expect("no target");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();
    let r2 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"type_text_chars","args":{{"pid":{pid},"window_id":{wid},"text":"x"}}}}"#));
    let t2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    println!("type_text_chars resp: {t2:?}");
    // Should produce the same response as `type_text` (which uses
    // "✅ Typed N char(s) on pid X via PostMessage (Yms delay).").
    assert!(t2.starts_with("✅ Typed 1 char(s) on pid "),
        "type_text_chars should resolve to type_text response: {t2:?}");

    println!("\n✅ PASS: type_text_chars excluded from list AND aliased to type_text on invoke");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
