//! Parity check for `set_recording` + `get_recording_state`.

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

    // 1. Disable to start clean.
    let _ = req(&mut pipe, r#"{"method":"call","name":"set_recording","args":{"enabled":false}}"#);

    // 2. get_recording_state — disabled.
    let r1 = req(&mut pipe, r#"{"method":"call","name":"get_recording_state","args":{}}"#);
    let t1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert_eq!(t1, "✅ recording: disabled", "Disabled text wrong: {t1:?}");
    println!("get_recording_state disabled OK");

    // 3. set_recording missing field.
    let r2 = req(&mut pipe, r#"{"method":"call","name":"set_recording","args":{}}"#);
    let e2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert!(e2.to_lowercase().contains("enabled") && e2.to_lowercase().contains("required"),
        "Missing-enabled wording wrong: {e2:?}");
    println!("Missing-enabled err OK");

    // 4. set_recording enabled without output_dir.
    let r3 = req(&mut pipe, r#"{"method":"call","name":"set_recording","args":{"enabled":true}}"#);
    let e3 = extract_text(&serde_json::from_str(r3.trim()).unwrap());
    assert!(e3.contains("`output_dir` is required when enabling recording"),
        "Missing-output_dir wording wrong: {e3:?}");
    println!("Missing-output_dir err OK");

    // 5. Enable with output_dir.
    let dir = std::env::temp_dir().join("cua-rec-parity").to_string_lossy().into_owned()
        .replace('\\', "\\\\");
    let r4 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"set_recording","args":{{"enabled":true,"output_dir":"{dir}"}}}}"#));
    let t4 = extract_text(&serde_json::from_str(r4.trim()).unwrap());
    println!("Enable resp: {t4:?}");
    assert!(t4.starts_with("✅ Recording enabled -> "),
        "Enable text wrong: {t4:?}");

    // 6. get_recording_state — enabled.
    let r5 = req(&mut pipe, r#"{"method":"call","name":"get_recording_state","args":{}}"#);
    let t5 = extract_text(&serde_json::from_str(r5.trim()).unwrap());
    println!("State enabled: {t5:?}");
    assert!(t5.starts_with("✅ recording: enabled output_dir=") && t5.contains("next_turn="),
        "Enabled state text wrong: {t5:?}");

    // 7. Disable.
    let r6 = req(&mut pipe, r#"{"method":"call","name":"set_recording","args":{"enabled":false}}"#);
    let t6 = extract_text(&serde_json::from_str(r6.trim()).unwrap());
    assert_eq!(t6, "✅ Recording disabled.", "Disabled text wrong: {t6:?}");

    println!("\n✅ PASS: get_recording_state + set_recording match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
