//! Smoke check for the recording tools via the Windows named-pipe daemon.
//!
//! Renamed from the old "Swift-parity" check — the API split into
//! `start_recording` + `stop_recording` (see JOURNAL_VIDEO.md) means
//! the older Swift CLI surface is no longer the reference. This file
//! verifies the new contract end-to-end against a running daemon.

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

    // 1. Stop to start clean (idempotent).
    let _ = req(&mut pipe, r#"{"method":"call","name":"stop_recording","args":{}}"#);

    // 2. get_recording_state — disabled.
    let r1 = req(&mut pipe, r#"{"method":"call","name":"get_recording_state","args":{}}"#);
    let t1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert_eq!(t1, "✅ recording: disabled", "Disabled text wrong: {t1:?}");
    println!("get_recording_state disabled OK");

    // 3. start_recording without output_dir → must error.
    let r2 = req(&mut pipe, r#"{"method":"call","name":"start_recording","args":{}}"#);
    let e2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert!(e2.to_lowercase().contains("output_dir") && e2.to_lowercase().contains("required"),
        "Missing-output_dir wording wrong: {e2:?}");
    println!("Missing-output_dir err OK");

    // 4. Start with output_dir, opt out of video (no ffmpeg dependency).
    let dir = std::env::temp_dir().join("cua-rec-parity").to_string_lossy().into_owned()
        .replace('\\', "\\\\");
    let r4 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"start_recording","args":{{"output_dir":"{dir}","record_video":false}}}}"#));
    let t4 = extract_text(&serde_json::from_str(r4.trim()).unwrap());
    println!("Start resp: {t4:?}");
    assert!(t4.starts_with("✅ Recording started -> "),
        "Start text wrong: {t4:?}");

    // 5. get_recording_state — enabled.
    let r5 = req(&mut pipe, r#"{"method":"call","name":"get_recording_state","args":{}}"#);
    let t5 = extract_text(&serde_json::from_str(r5.trim()).unwrap());
    println!("State enabled: {t5:?}");
    assert!(t5.starts_with("✅ recording: enabled output_dir=") && t5.contains("next_turn="),
        "Enabled state text wrong: {t5:?}");

    // 6. Stop.
    let r6 = req(&mut pipe, r#"{"method":"call","name":"stop_recording","args":{}}"#);
    let t6 = extract_text(&serde_json::from_str(r6.trim()).unwrap());
    assert!(t6.starts_with("✅ Recording stopped."), "Stop text wrong: {t6:?}");

    println!("\n✅ PASS: start_recording + stop_recording + get_recording_state");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
