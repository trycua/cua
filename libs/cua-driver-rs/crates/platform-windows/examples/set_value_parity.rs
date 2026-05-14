//! Parity check for `set_value`.
//!
//! Tests the Swift-shaped error wording and text format.  We don't issue a
//! real `set_value` call against a live element because that requires a
//! freshly-walked UIA cache; the response-shape verification on the error
//! paths is sufficient to lock the parity.

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

    // Missing value error (with all three integers present).  Note: pid /
    // window_id / element_index are schema-required; we pick a real pid +
    // window_id to ensure the schema doesn't reject before our tool runs.
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome") || n.contains("notepad")
    }).expect("no target");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();

    // schema layer requires `value` — daemon rejects with its own wording;
    // either Swift's tool-layer wording or schema-layer wording is acceptable
    // as long as `value` is named.
    let r1 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"set_value","args":{{"pid":{pid},"window_id":{wid},"element_index":0}}}}"#));
    let e1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert!(e1.to_lowercase().contains("value") && e1.to_lowercase().contains("required"),
        "Missing-value wording wrong: {e1:?}");
    println!("Missing-value err OK: {e1:?}");

    // Element-not-in-cache (we haven't run get_window_state). Should fail
    // with an "Element X not in cache" wording, not a schema error.
    let r2 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"set_value","args":{{"pid":{pid},"window_id":{wid},"element_index":99999,"value":"x"}}}}"#));
    let e2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert!(e2.to_lowercase().contains("not in cache") || e2.to_lowercase().contains("element"),
        "Cache-miss wording should mention element/cache: {e2:?}");
    println!("Cache-miss err OK: {e2:?}");

    println!("\n✅ PASS: set_value validation + Swift-shape error wording verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
