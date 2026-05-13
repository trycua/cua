//! TDD harness for the agent cursor overlay — drives the LIVE DAEMON.
//!
//! Strategy: set the cursor to pure-magenta gradient (a colour that does not
//! naturally appear on screen), move it to a target, screenshot, and search
//! the screenshot for magenta pixels.  Cleaner than diffing two screenshots
//! because we don't compete with screen activity.
//!
//! Hard 2-second timeout on every pipe round-trip.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
fn main() {
    use image::GenericImageView;

    let target_x = 700i32;
    let target_y = 500i32;

    let mut pipe = std::fs::OpenOptions::new()
        .read(true).write(true)
        .open(r"\\.\pipe\cua-driver")
        .expect("open pipe — start daemon first");
    println!("[1] connected");

    fn req(p: &mut std::fs::File, json: &str) -> String {
        p.write_all(format!("{json}\n").as_bytes()).unwrap();
        p.flush().ok();
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if Instant::now() > deadline { break; }
            let n = p.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            out.extend_from_slice(&buf[..n]);
            if out.contains(&b'\n') { break; }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    let r2 = req(&mut pipe, r#"{"method":"call","name":"set_agent_cursor_enabled","args":{"enabled":true}}"#);
    println!("    enabled resp: {}", r2.trim());
    // Set bright pure-magenta gradient — easy to find, won't match any normal screen content.
    let r3 = req(&mut pipe, r##"{"method":"call","name":"set_agent_cursor_style","args":{"gradient_colors":["#FF00FF","#FF00FF","#FF00FF"],"bloom_color":"#FF00FF"}}"##);
    println!("    style resp: {}", r3.trim());

    let cmd = format!(
        r#"{{"method":"call","name":"move_cursor","args":{{"x":{},"y":{}}}}}"#,
        target_x, target_y);
    let r4 = req(&mut pipe, &cmd);
    println!("    move resp: {}", r4.trim());

    // Poll until cursor centroid stops moving, with a 4s ceiling.  Swift
    // signals arrival via a one-shot channel; the Rust Windows overlay doesn't
    // expose one yet, so we poll instead.
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut last_centroid = (-1i64, -1i64);
    let mut stable_ticks = 0;
    let mut png;
    loop {
        std::thread::sleep(Duration::from_millis(150));
        png = platform_windows::capture::screenshot_display_bytes()
            .expect("screenshot");
        let img = image::load_from_memory(&png).expect("decode");
        let rgba = img.to_rgba8();
        let (iw, ih) = img.dimensions();
        let mut count = 0u64; let mut sx = 0i64; let mut sy = 0i64;
        for py in 0..ih { for px in 0..iw {
            let p = rgba.get_pixel(px, py).0;
            if p[0] > 180 && p[1] < 100 && p[2] > 180 {
                count += 1; sx += px as i64; sy += py as i64;
            }
        }}
        if count > 50 {
            let c = (sx / count as i64, sy / count as i64);
            let dist = ((c.0 - last_centroid.0).pow(2) + (c.1 - last_centroid.1).pow(2)) as f64;
            if dist.sqrt() < 3.0 { stable_ticks += 1; } else { stable_ticks = 0; }
            println!("    polling: centroid=({},{}) count={} stable={}", c.0, c.1, count, stable_ticks);
            last_centroid = c;
            if stable_ticks >= 3 { break; }
        }
        if Instant::now() > deadline { println!("    polling: deadline reached"); break; }
    }
    let _ = std::fs::write("O:/tmp_cursor_screen.png", &png);
    let img = image::load_from_memory(&png).expect("decode");
    let (w, h) = img.dimensions();
    let rgba = img.to_rgba8();

    // Search whole screen for magenta-ish pixels (R high, G low, B high).
    let mut magenta_count = 0u64;
    let mut sx = 0i64; let mut sy = 0i64;
    let mut min_x = w as i32; let mut max_x = 0i32;
    let mut min_y = h as i32; let mut max_y = 0i32;
    for py in 0..h {
        for px in 0..w {
            let p = rgba.get_pixel(px, py).0;
            // Tolerate antialiasing: R > 180, G < 100, B > 180
            if p[0] > 180 && p[1] < 100 && p[2] > 180 {
                magenta_count += 1;
                sx += px as i64; sy += py as i64;
                min_x = min_x.min(px as i32); max_x = max_x.max(px as i32);
                min_y = min_y.min(py as i32); max_y = max_y.max(py as i32);
            }
        }
    }
    println!("[4] screen {w}x{h}, magenta pixels: {magenta_count}");

    if magenta_count == 0 {
        eprintln!("\n❌ FAIL: NO magenta cursor pixels found anywhere on screen.");
        eprintln!("   The overlay window exists but isn't rendering, OR style cmd didn't apply.");
        std::process::exit(1);
    }

    let cx = sx / magenta_count as i64;
    let cy = sy / magenta_count as i64;
    let dx = (cx - target_x as i64).abs();
    let dy = (cy - target_y as i64).abs();
    println!("[5] centroid: ({cx},{cy}); bbox ({min_x},{min_y})-({max_x},{max_y})");
    println!("    target was ({target_x},{target_y}); offset = ({dx},{dy})");

    if dx > 100 || dy > 100 {
        eprintln!("\n❌ FAIL: cursor is rendering but at wrong location.");
        eprintln!("   Expected near ({target_x},{target_y}), found centroid ({cx},{cy}).");
        std::process::exit(2);
    }
    println!("\n✅ PASS: magenta cursor at ({cx},{cy}) close to target ({target_x},{target_y})");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
