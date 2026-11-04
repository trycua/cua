//! Parity check for `set_agent_cursor_enabled` + `set_agent_cursor_motion`.

#[cfg(target_os = "windows")]
use std::io::{Read, Write};
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
fn main() {
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\pipe\cua-driver")
        .expect("open pipe");

    fn req(p: &mut std::fs::File, json: &str) -> String {
        p.write_all(format!("{json}\n").as_bytes()).unwrap();
        p.flush().ok();
        let mut out = Vec::new();
        let mut buf: Vec<u8> = vec![0u8; 64 * 1024];
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if Instant::now() > deadline {
                panic!("timeout");
            }
            let n = p.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if out.contains(&b'\n') {
                break;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    fn extract_text(v: &serde_json::Value) -> String {
        v.pointer("/result/content/0/text")
            .and_then(|t| t.as_str())
            .or_else(|| v.pointer("/error").and_then(|e| e.as_str()))
            .map(|s| s.to_owned())
            .unwrap_or_default()
    }

    // 1. set_agent_cursor_enabled missing field error.
    let r1 = req(
        &mut pipe,
        r#"{"method":"call","name":"set_agent_cursor_enabled","args":{}}"#,
    );
    let e1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert!(
        e1.to_lowercase().contains("enabled") && e1.to_lowercase().contains("required"),
        "Missing-enabled wording wrong: {e1:?}"
    );
    println!("Missing-enabled err OK");

    // 2. set_agent_cursor_enabled true.
    let r2 = req(
        &mut pipe,
        r#"{"method":"call","name":"set_agent_cursor_enabled","args":{"enabled":true}}"#,
    );
    let t2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert_eq!(t2, "✅ Agent cursor enabled.", "Enabled text wrong: {t2:?}");
    println!("Enabled OK");

    // 3. set_agent_cursor_enabled false.
    let r3 = req(
        &mut pipe,
        r#"{"method":"call","name":"set_agent_cursor_enabled","args":{"enabled":false}}"#,
    );
    let t3 = extract_text(&serde_json::from_str(r3.trim()).unwrap());
    assert_eq!(
        t3, "✅ Agent cursor disabled.",
        "Disabled text wrong: {t3:?}"
    );
    println!("Disabled OK");

    // Re-enable for subsequent tests.
    let _ = req(
        &mut pipe,
        r#"{"method":"call","name":"set_agent_cursor_enabled","args":{"enabled":true}}"#,
    );

    // 4. set_agent_cursor_motion: tune knobs, style, timing and effects.
    let r4 = req(
        &mut pipe,
        r#"{"method":"call","name":"set_agent_cursor_motion","args":{"session":"parity","start_handle":0.4,"arc_size":0.3,"spring":0.8,"glide_duration_ms":500,"style":"dc-comet-swoop","timing":"fixed","effects":{"trail":false}}}"#,
    );
    let v4: serde_json::Value = serde_json::from_str(r4.trim()).unwrap();
    println!("Motion resp: {:?}", extract_text(&v4));
    let motion = v4
        .pointer("/result/structuredContent/motion")
        .expect("structuredContent.motion");
    assert_eq!(motion["start_handle"], 0.4);
    assert_eq!(motion["arc_size"], 0.3);
    assert_eq!(motion["spring"], 0.8);
    assert_eq!(motion["glide_duration_ms"], 500.0);
    assert_eq!(motion["style"], "comet_swoop");
    assert_eq!(motion["timing"], "fixed");
    assert_eq!(motion["effects"]["trail"], false);
    assert!(motion["effects"]["ripple"].is_boolean());

    // 5. An unknown style is a tool error.
    let r5 = req(
        &mut pipe,
        r#"{"method":"call","name":"set_agent_cursor_motion","args":{"session":"parity","style":"zigzag"}}"#,
    );
    let e5 = extract_text(&serde_json::from_str(r5.trim()).unwrap());
    assert!(e5.contains("zigzag"), "Unknown-style error wrong: {e5:?}");

    println!("\n✅ PASS: set_agent_cursor_enabled + set_agent_cursor_motion");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
