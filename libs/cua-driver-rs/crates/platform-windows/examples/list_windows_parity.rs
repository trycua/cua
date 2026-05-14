//! Parity check for `list_windows`.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
fn main() {
    let mut pipe = std::fs::OpenOptions::new()
        .read(true).write(true)
        .open(r"\\.\pipe\cua-driver")
        .expect("open pipe — start daemon first");

    fn req(p: &mut std::fs::File, json: &str) -> String {
        p.write_all(format!("{json}\n").as_bytes()).unwrap();
        p.flush().ok();
        let mut out = Vec::new();
        let mut buf = [0u8; 65536];
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if Instant::now() > deadline { panic!("response timeout"); }
            let n = p.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            out.extend_from_slice(&buf[..n]);
            if out.contains(&b'\n') { break; }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    let resp = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let v: serde_json::Value = serde_json::from_str(resp.trim()).expect("parse JSON");
    let text = v.pointer("/result/content/0/text").and_then(|t| t.as_str()).expect("missing text");
    let header = text.lines().next().unwrap_or("");
    println!("Header: {header:?}");
    assert!(
        header.starts_with("✅ Found ") && header.contains(" window(s) across "),
        "Header does not match Swift format: {header:?}"
    );
    assert!(
        header.contains("SkyLight Space SPIs unavailable"),
        "Windows should append the SPI-unavailable note (matches Swift's else branch)"
    );

    let wins = v.pointer("/result/structuredContent/windows").and_then(|w| w.as_array())
        .expect("structuredContent.windows is not an array");
    assert!(!wins.is_empty(), "expected at least one visible window");

    for w in wins {
        for key in &["window_id","pid","app_name","title","bounds","layer","z_index","is_on_screen"] {
            assert!(w.pointer(&format!("/{key}")).is_some(),
                "window record missing field {key}: {w}");
        }
        // bounds shape
        for k in &["x","y","width","height"] {
            assert!(w.pointer(&format!("/bounds/{k}")).is_some(), "bounds missing {k}");
        }
        // Swift omits these on Windows (SPI unavailable); explicitly assert
        // they are NOT present.
        assert!(w.get("on_current_space").is_none(), "on_current_space should be omitted on Windows");
        assert!(w.get("space_ids").is_none(),       "space_ids should be omitted on Windows");
    }

    let csi = v.pointer("/result/structuredContent/current_space_id");
    assert!(csi.is_some() && csi.unwrap().is_null(),
        "current_space_id should be null on Windows; got {:?}", csi);

    // Pid-filter warning path.
    let resp2 = req(&mut pipe,
        r#"{"method":"call","name":"list_windows","args":{"pid":4294967295}}"#);
    let v2: serde_json::Value = serde_json::from_str(resp2.trim()).expect("parse JSON");
    let t2 = v2.pointer("/result/content/0/text").and_then(|t| t.as_str()).unwrap_or("");
    assert!(t2.starts_with("⚠️ No windows found for pid"),
        "Expected ⚠️-prefixed warning for bogus pid, got: {t2:?}");

    println!("\n✅ PASS: text header, structuredContent fields + bounds shape, and pid-warning path all match Swift");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
