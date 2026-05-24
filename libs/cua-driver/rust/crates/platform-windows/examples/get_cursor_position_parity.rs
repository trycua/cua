//! Parity check for the `get_cursor_position` MCP tool.
//!
//! Drives the live daemon via the named pipe.  Asserts the text response
//! matches Swift's format exactly: `"✅ Cursor at (X, Y)"` with integer
//! coordinates, and that `structuredContent` contains `x`/`y` as integers
//! that agree with the platform's native `GetCursorPos`.
//!
//! Hard 4-second timeout on every round-trip.

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
        let mut buf = [0u8; 8192];
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

    let resp = req(&mut pipe, r#"{"method":"call","name":"get_cursor_position","args":{}}"#);
    let v: serde_json::Value = serde_json::from_str(resp.trim()).expect("parse JSON");

    let text = v.pointer("/result/content/0/text").and_then(|t| t.as_str())
        .expect("missing /result/content/0/text");
    println!("Response text: {text:?}");

    // Parse "✅ Cursor at (X, Y)" by hand — no regex crate dependency.
    let stripped = text.strip_prefix("✅ Cursor at (")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or_else(|| panic!(
            "Text {text:?} does not match Swift format `✅ Cursor at (X, Y)`"
        ));
    let (xs, ys) = stripped.split_once(", ")
        .unwrap_or_else(|| panic!("missing `, ` between coords in {text:?}"));
    let tx: i64 = xs.parse().unwrap_or_else(|_| panic!("x not int: {xs}"));
    let ty: i64 = ys.parse().unwrap_or_else(|_| panic!("y not int: {ys}"));
    println!("Parsed text coords: ({tx}, {ty})");

    let sx = v.pointer("/result/structuredContent/x").and_then(|n| n.as_i64())
        .expect("structuredContent.x not present or not integer");
    let sy = v.pointer("/result/structuredContent/y").and_then(|n| n.as_i64())
        .expect("structuredContent.y not present or not integer");
    println!("Structured coords: ({sx}, {sy})");

    assert_eq!((tx, ty), (sx, sy), "text and structuredContent disagree");

    // Compare against the native GetCursorPos at this moment for a sanity check
    // (allow ±5 px slop since the cursor can drift between the daemon call
    // and our local call).
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { let _ = GetCursorPos(&mut pt); }
    let dx = (pt.x as i64 - sx).abs();
    let dy = (pt.y as i64 - sy).abs();
    println!("Native GetCursorPos: ({}, {}) — delta ({dx}, {dy})", pt.x, pt.y);
    assert!(dx <= 5 && dy <= 5,
        "get_cursor_position reports ({sx},{sy}) but native GetCursorPos says ({}, {})", pt.x, pt.y);

    println!("\n✅ PASS: text format matches Swift exactly, structuredContent agrees, native ≈ same");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
