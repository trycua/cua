//! End-to-end demonstration recording test.
//! Usage: cargo run -p cua-driver-core --example demo_record -- <hwnd> <pid> [secs]
//!
//! Starts a real recording in demonstration mode on the given window (glowing
//! border + window-scoped human-input capture), waits while you interact, then
//! stops and renders TRAJECTORY.md. Real (non-injected) human input is captured;
//! events outside the target window are dropped.

use std::sync::Arc;
use cua_driver_core::recording::RecordingSession;
use cua_driver_core::recording_markdown::{process, ProcessOptions};

fn main() {
    let mut a = std::env::args().skip(1);
    let hwnd: u64 = a.next().and_then(|s| s.parse().ok()).expect("hwnd");
    let pid: i64 = a.next().and_then(|s| s.parse().ok()).expect("pid");
    let secs: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(25);

    let dir = std::env::temp_dir().join("cua-demo-chrome");
    let _ = std::fs::remove_dir_all(&dir);

    let session = Arc::new(RecordingSession::new());
    session.start(dir.to_str().unwrap(), false, None).expect("start");
    session.begin_demonstration(pid, hwnd, false).expect("begin_demonstration");

    println!("🔴 Recording demonstration on HWND {hwnd} (pid {pid}) for {secs}s.");
    println!("   Interact with that window now — click, scroll, type. (typing is redacted)");
    for i in (1..=secs).rev() {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if i % 5 == 0 {
            let st = session.current_state();
            println!("   {i}s left … captured {} human turn(s)", st.human_turns);
        }
    }

    session.stop_owner(None).expect("stop");
    let st = session.current_state();
    println!("\n✅ Stopped. {} human turn(s) captured.", st.human_turns);

    match process(&dir, &ProcessOptions::default()) {
        Ok(res) => {
            println!("📄 {}\n", res.trajectory_md.display());
            if let Ok(md) = std::fs::read_to_string(&res.trajectory_md) {
                println!("{md}");
            }
        }
        Err(e) => println!("process failed (no turns captured?): {e}"),
    }
}
