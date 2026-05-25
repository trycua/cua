//! Parity check for `get_screen_size`.
//!
//! Asserts text matches Swift exactly:
//!   `"✅ Main display: WxH points @ Sx"`
//! and `structuredContent` has `width`, `height`, and `scale_factor`
//! (snake_case) that agree with the platform's `GetSystemMetrics` and
//! `GetDpiForSystem`.
//!
//! Hard 4-second per-call timeout.

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

    let resp = req(&mut pipe, r#"{"method":"call","name":"get_screen_size","args":{}}"#);
    let v: serde_json::Value = serde_json::from_str(resp.trim()).expect("parse JSON");
    let text = v.pointer("/result/content/0/text").and_then(|t| t.as_str())
        .expect("missing text");
    println!("Response text: {text:?}");

    // Parse "✅ Main display: WxH points @ Sx" by hand.
    let rest = text.strip_prefix("✅ Main display: ")
        .unwrap_or_else(|| panic!("Text {text:?} missing Swift prefix `✅ Main display: `"));
    let (wh, scale_part) = rest.split_once(" points @ ")
        .unwrap_or_else(|| panic!("Text {text:?} missing ` points @ `"));
    let (w_str, h_str) = wh.split_once('x')
        .unwrap_or_else(|| panic!("Text {text:?} missing `x` separator"));
    let scale_str = scale_part.strip_suffix('x')
        .unwrap_or_else(|| panic!("Text {text:?} missing trailing `x`"));
    let tw: i64 = w_str.parse().unwrap();
    let th: i64 = h_str.parse().unwrap();
    let ts: f64 = scale_str.parse().unwrap();
    println!("Parsed text: width={tw} height={th} scale={ts}");

    // Verify structuredContent.
    let sw = v.pointer("/result/structuredContent/width").and_then(|n| n.as_i64()).expect("width");
    let sh = v.pointer("/result/structuredContent/height").and_then(|n| n.as_i64()).expect("height");
    let ss = v.pointer("/result/structuredContent/scale_factor").and_then(|n| n.as_f64()).expect("scale_factor");
    println!("Structured: width={sw} height={sh} scale_factor={ss}");

    assert_eq!((tw, th), (sw, sh));
    assert!((ts - ss).abs() < 0.001);

    // Sanity-check against native APIs.
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    use windows::Win32::UI::HiDpi::GetDpiForSystem;
    let (nw, nh) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as i64, GetSystemMetrics(SM_CYSCREEN) as i64) };
    let dpi = unsafe { GetDpiForSystem() };
    let ns = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
    println!("Native: width={nw} height={nh} scale={ns}");
    assert_eq!((tw, th), (nw, nh));
    assert!((ts - ns).abs() < 0.001);

    println!("\n✅ PASS: text format matches Swift, structuredContent has scale_factor, native agrees");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
