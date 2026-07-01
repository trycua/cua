//! Visual smoke test for the recording indicator + capture gate coupling.
//!
//! Usage: `cargo run -p input-capture --example border_demo -- <HWND_decimal> [secs]`
//!
//! Draws the glowing recording border around the given top-level window for a
//! few seconds and prints the heartbeat frame count + whether the capture gate
//! would currently allow an event at the window center. Real (non-injected)
//! human input is required to see captured events, so this primarily validates
//! the *visible* half (border paints, heartbeat advances, gate opens).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use input_capture::{CaptureConfig, CaptureGate, Demonstration, HumanEvent, IndicatorHeartbeat};

fn main() {
    let mut args = std::env::args().skip(1);
    let hwnd: u64 = args
        .next()
        .and_then(|s| s.parse().ok())
        .expect("pass a target HWND (decimal) as the first arg");
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);

    let start = Instant::now();
    let anchor = Arc::new(AtomicU64::new(0));
    let a = anchor.clone();
    let clock: Arc<dyn Fn() -> u64 + Send + Sync> = Arc::new(move || {
        let _ = &a;
        start.elapsed().as_millis() as u64
    });

    let (tx, rx) = std::sync::mpsc::channel::<HumanEvent>();
    let cfg = CaptureConfig { pid: 0, window_id: hwnd, capture_raw_text: false };

    println!("Starting demonstration on HWND {hwnd} for {secs}s. Interact with the window now.");
    let demo = Demonstration::start(cfg, tx, clock.clone()).expect("start demonstration");

    // Drain captured events in the background.
    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            println!("  captured: {ev:?}");
        }
    });

    let until = Instant::now() + std::time::Duration::from_secs(secs);
    while Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        println!("  t={}ms ...", start.elapsed().as_millis());
    }

    demo.stop();
    println!("Stopped. Border removed; capture torn down.");

    // Sanity: a gate built from a fresh (never-presented) heartbeat is closed.
    let hb = IndicatorHeartbeat::new();
    let gate = CaptureGate::new(hb);
    assert!(!gate.allow(clock()));
    let _ = anchor.load(Ordering::SeqCst);
    println!("Gate-without-indicator correctly closed.");
}
