//! Parity check for `get_window_state` error wording + validation guards.

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
        let deadline = Instant::now() + Duration::from_secs(6);
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

    // 1. Missing pid.
    let r1 = req(&mut pipe, r#"{"method":"call","name":"get_window_state","args":{}}"#);
    let e1 = extract_text(&serde_json::from_str(r1.trim()).unwrap());
    assert!(e1.to_lowercase().contains("pid") && e1.to_lowercase().contains("required"),
        "Missing-pid wording wrong: {e1:?}");
    println!("Missing-pid err OK");

    // 2. Missing window_id (with valid pid).
    let lw = req(&mut pipe, r#"{"method":"call","name":"list_windows","args":{}}"#);
    let lwv: serde_json::Value = serde_json::from_str(lw.trim()).unwrap();
    let wins = lwv.pointer("/result/structuredContent/windows").and_then(|w| w.as_array()).unwrap();
    let target = wins.iter().find(|w| {
        let n = w.pointer("/app_name").and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
        n.contains("chrome")
    }).expect("no chrome");
    let pid = target.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
    let wid = target.pointer("/window_id").and_then(|n| n.as_u64()).unwrap();

    let r2 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"get_window_state","args":{{"pid":{pid}}}}}"#));
    let e2 = extract_text(&serde_json::from_str(r2.trim()).unwrap());
    assert!(e2.to_lowercase().contains("window_id") && e2.to_lowercase().contains("required"),
        "Missing-window_id wording wrong: {e2:?}");
    println!("Missing-window_id err OK");

    // 3. Mismatched pid + window_id (pick another window's pid).
    let other = wins.iter().find(|w| {
        let p = w.pointer("/pid").and_then(|n| n.as_u64()).unwrap_or(0);
        p != pid
    });
    if let Some(other) = other {
        let other_pid = other.pointer("/pid").and_then(|n| n.as_u64()).unwrap();
        let r3 = req(&mut pipe, &format!(
            r#"{{"method":"call","name":"get_window_state","args":{{"pid":{other_pid},"window_id":{wid}}}}}"#));
        let e3 = extract_text(&serde_json::from_str(r3.trim()).unwrap());
        assert!(e3.contains("belongs to pid") || e3.contains("No window with window_id"),
            "Mismatched-pid wording wrong: {e3:?}");
        println!("Mismatched-pid err OK");
    }

    // 4. Bogus window_id.
    let r4 = req(&mut pipe, &format!(
        r#"{{"method":"call","name":"get_window_state","args":{{"pid":{pid},"window_id":99999999}}}}"#));
    let e4 = extract_text(&serde_json::from_str(r4.trim()).unwrap());
    assert!(e4.contains("No window with window_id"),
        "Bogus-window_id wording wrong: {e4:?}");
    println!("Bogus-window_id err OK");

    println!("\n✅ PASS: get_window_state error wording + validation guards verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
