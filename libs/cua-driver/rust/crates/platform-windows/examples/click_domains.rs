//! Click the "Domains" hyperlink in Chrome via the daemon pipe.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
fn main() {
    let chrome_pid = 62156u64;
    let chrome_wid = 4464038u64;

    let mut pipe = std::fs::OpenOptions::new()
        .read(true).write(true)
        .open(r"\\.\pipe\cua-driver")
        .expect("open pipe");

    fn req(p: &mut std::fs::File, json: &str) -> String {
        p.write_all(format!("{json}\n").as_bytes()).unwrap();
        p.flush().ok();
        let mut out = Vec::new();
        let mut buf = [0u8; 65536];
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if Instant::now() > deadline { break; }
            let n = p.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            out.extend_from_slice(&buf[..n]);
            if out.contains(&b'\n') { break; }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    // Enable cursor.
    req(&mut pipe, r#"{"method":"call","name":"set_agent_cursor_enabled","args":{"enabled":true}}"#);

    // Get window state with "Domains" query filter.
    let state_req = format!(
        r#"{{"method":"call","name":"get_window_state","args":{{"pid":{},"window_id":{},"query":"Domains"}}}}"#,
        chrome_pid, chrome_wid);
    let state_resp = req(&mut pipe, &state_req);
    // Find the Domains element index in the tree text.  Tree format:
    //   "- [N] Hyperlink \"Domains\" ..."
    // Find any clickable element whose name contains "Domains" — search the
    // \"Domains\" or "Domain" name.
    let query = std::env::args().nth(1).unwrap_or_else(|| "Domains".to_owned());
    let needle = format!("\"{}\"", query);
    let candidates: Vec<(u64, String)> = state_resp
        .split("\\n")
        .filter(|l| l.contains(&needle) && l.contains("- ["))
        .filter_map(|l| {
            let after_bracket = l.split("- [").nth(1)?;
            let n_str = after_bracket.split(']').next()?;
            let idx: u64 = n_str.parse().ok()?;
            Some((idx, l.to_owned()))
        }).collect();
    if candidates.is_empty() {
        eprintln!("No clickable element matching {needle}.");
        eprintln!("Tree dump (first 2000 chars):");
        eprintln!("{}", &state_resp.replace("\\n", "\n").chars().take(2000).collect::<String>());
        std::process::exit(1);
    }
    for (i, (idx, line)) in candidates.iter().enumerate() {
        println!("  candidate #{i}: [{idx}] {}", line.trim());
    }
    let (idx, _) = candidates[0];
    println!("Using element_index={idx}");

    let click_req = format!(
        r#"{{"method":"call","name":"click","args":{{"pid":{},"window_id":{},"element_index":{}}}}}"#,
        chrome_pid, chrome_wid, idx);
    let click_resp = req(&mut pipe, &click_req);
    println!("Click resp: {}", click_resp.trim());
}

#[cfg(not(target_os = "windows"))]
fn main() {}
