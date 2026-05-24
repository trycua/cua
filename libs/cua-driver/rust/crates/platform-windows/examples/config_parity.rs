//! Parity check for `get_config` + `set_config`.

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

    // 1. get_config — text starts with "✅ " followed by pretty JSON.
    let r1 = req(&mut pipe, r#"{"method":"call","name":"get_config","args":{}}"#);
    let v1: serde_json::Value = serde_json::from_str(r1.trim()).unwrap();
    let t1 = extract_text(&v1);
    assert!(t1.starts_with("✅ "), "get_config text should start with '✅ ': {t1:?}");
    // Pretty JSON should contain schema_version and capture_mode keys.
    assert!(t1.contains("\"schema_version\"") && t1.contains("\"capture_mode\""),
        "get_config JSON missing keys: {t1:?}");
    // structuredContent should have the same fields.
    let sc1 = v1.pointer("/result/structuredContent").unwrap();
    assert!(sc1.get("schema_version").is_some());
    assert!(sc1.get("capture_mode").is_some());
    assert!(sc1.get("max_image_dimension").is_some());
    println!("get_config OK");

    // 2. set_config Swift-style {key, value}.
    let r2 = req(&mut pipe,
        r#"{"method":"call","name":"set_config","args":{"key":"capture_mode","value":"vision"}}"#);
    let v2: serde_json::Value = serde_json::from_str(r2.trim()).unwrap();
    let cm = v2.pointer("/result/structuredContent/capture_mode").and_then(|s| s.as_str());
    assert_eq!(cm, Some("vision"), "set_config didn't update capture_mode via {{key, value}}");
    println!("set_config Swift-style OK (capture_mode=vision)");

    // 3. set_config legacy per-field shape.
    let r3 = req(&mut pipe,
        r#"{"method":"call","name":"set_config","args":{"capture_mode":"som","max_image_dimension":1024}}"#);
    let v3: serde_json::Value = serde_json::from_str(r3.trim()).unwrap();
    let cm3 = v3.pointer("/result/structuredContent/capture_mode").and_then(|s| s.as_str());
    let md3 = v3.pointer("/result/structuredContent/max_image_dimension").and_then(|n| n.as_u64());
    assert_eq!(cm3, Some("som"));
    assert_eq!(md3, Some(1024));
    println!("set_config legacy OK (capture_mode=som, max=1024)");

    // 4. set_config unknown key error.
    let r4 = req(&mut pipe,
        r#"{"method":"call","name":"set_config","args":{"key":"bogus","value":42}}"#);
    let e4 = extract_text(&serde_json::from_str(r4.trim()).unwrap());
    assert!(e4.contains("Unknown config key"), "Unknown-key wording wrong: {e4:?}");
    println!("set_config unknown-key err OK");

    // 5. set_config missing inputs error.
    let r5 = req(&mut pipe, r#"{"method":"call","name":"set_config","args":{}}"#);
    let e5 = extract_text(&serde_json::from_str(r5.trim()).unwrap());
    assert!(e5.to_lowercase().contains("required") || e5.to_lowercase().contains("missing"),
        "Missing-input wording should mention required/missing: {e5:?}");
    println!("set_config missing-input err OK");

    // Restore defaults.
    let _ = req(&mut pipe,
        r#"{"method":"call","name":"set_config","args":{"capture_mode":"som","max_image_dimension":0}}"#);

    println!("\n✅ PASS: get_config + set_config (Swift-style and legacy) verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
