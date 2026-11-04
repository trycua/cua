//! Parity check for `get_agent_cursor_state`.

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

    let r = req(
        &mut pipe,
        r#"{"method":"call","name":"get_agent_cursor_state","args":{"session":"parity"}}"#,
    );
    let v: serde_json::Value = serde_json::from_str(r.trim()).unwrap();
    let text = v
        .pointer("/result/content/0/text")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    println!("State text: {text:?}");

    // Verify the canonical structuredContent shape.
    let sc = v.pointer("/result/structuredContent").unwrap();
    for k in &[
        "session",
        "enabled",
        "position",
        "theme",
        "visual_state",
        "motion",
    ] {
        assert!(sc.get(k).is_some(), "structuredContent missing {k}");
    }
    let motion = &sc["motion"];
    for k in &[
        "start_handle",
        "end_handle",
        "arc_size",
        "arc_flow",
        "spring",
        "glide_duration_ms",
        "dwell_after_click_ms",
        "idle_hide_ms",
        "turn_radius",
        "style",
        "timing",
        "effects",
    ] {
        assert!(motion.get(k).is_some(), "motion missing {k}");
    }
    for k in &["trail", "glow", "magnet", "ripple", "squish"] {
        assert!(
            motion["effects"][k].is_boolean(),
            "motion.effects.{k} is not a boolean"
        );
    }

    println!("\n✅ PASS: get_agent_cursor_state structuredContent matches the contract");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
