//! Parity check for `set_agent_cursor_enabled` + `set_agent_cursor_motion`.

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

    // 1. set_agent_cursor_enabled missing field error.
    let r1 = req(&mut pipe, r#"{"method":"call","name":"set_agent_cursor_enabled","args":{}}"#);
    let e1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert!(e1.to_lowercase().contains("enabled") && e1.to_lowercase().contains("required"),
        "Missing-enabled wording wrong: {e1:?}");
    println!("Missing-enabled err OK");

    // 2. set_agent_cursor_enabled true.
    let r2 = req(&mut pipe, r#"{"method":"call","name":"set_agent_cursor_enabled","args":{"enabled":true}}"#);
    let t2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert_eq!(t2, "✅ Agent cursor enabled.", "Enabled text wrong: {t2:?}");
    println!("Enabled OK");

    // 3. set_agent_cursor_enabled false.
    let r3 = req(&mut pipe, r#"{"method":"call","name":"set_agent_cursor_enabled","args":{"enabled":false}}"#);
    let t3 = extract_text(&serde_json::from_str(r3.trim()).unwrap());
    assert_eq!(t3, "✅ Agent cursor disabled.", "Disabled text wrong: {t3:?}");
    println!("Disabled OK");

    // Re-enable for subsequent tests.
    let _ = req(&mut pipe, r#"{"method":"call","name":"set_agent_cursor_enabled","args":{"enabled":true}}"#);

    // 4. set_agent_cursor_motion — tune knobs.
    let r4 = req(&mut pipe,
        r#"{"method":"call","name":"set_agent_cursor_motion","args":{"start_handle":0.4,"arc_size":0.3,"spring":0.8,"glide_duration_ms":500}}"#);
    let v4: serde_json::Value = serde_json::from_str(r4.trim()).unwrap();
    let t4 = extract_text(&v4);
    println!("Motion resp: {t4:?}");
    assert!(t4.starts_with("✅ cursor motion: startHandle=0.4 endHandle=") && t4.contains("arcSize=0.3")
        && t4.contains("spring=0.8") && t4.contains("glideDurationMs=500"),
        "Motion text doesn't match Swift format: {t4:?}");

    println!("\n✅ PASS: set_agent_cursor_enabled + set_agent_cursor_motion match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
