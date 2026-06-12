//! Parity check for `get_agent_cursor_state`.

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

    let r = req(&mut pipe, r#"{"method":"call","name":"get_agent_cursor_state","args":{}}"#);
    let v: serde_json::Value = serde_json::from_str(r.trim()).unwrap();
    let text = v.pointer("/result/content/0/text").and_then(|t| t.as_str()).unwrap_or("");
    println!("State text: {text:?}");

    // Verify Swift's exact key=value vocabulary.
    let needed = [
        "✅ cursor: enabled=",
        "startHandle=",
        "endHandle=",
        "arcSize=",
        "arcFlow=",
        "spring=",
        "glideDurationMs=",
        "dwellAfterClickMs=",
        "idleHideMs=",
    ];
    for k in needed {
        assert!(text.contains(k), "Missing key {k:?} in text: {text:?}");
    }

    // Verify structuredContent.
    let sc = v.pointer("/result/structuredContent").unwrap();
    for k in &["enabled","start_handle","end_handle","arc_size","arc_flow","spring",
               "glide_duration_ms","dwell_after_click_ms","idle_hide_ms","cursors"] {
        assert!(sc.get(k).is_some(), "structuredContent missing {k}");
    }

    println!("\n✅ PASS: get_agent_cursor_state matches Swift vocabulary + structuredContent");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
