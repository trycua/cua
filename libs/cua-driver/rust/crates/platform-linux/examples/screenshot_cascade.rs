//! Standalone smoke for the platform-linux screenshot cascade.
//!
//! Calls `capture::screenshot_display_bytes()` and prints the PNG size +
//! magic bytes + which tier produced the result (inferred from
//! tracing-subscriber logs).
//!
//! Usage:
//!   CUA_DRIVER_RS_ENABLE_WAYLAND=1 \
//!   WAYLAND_DISPLAY=wayland-0 \
//!   cargo run --example screenshot_cascade --release

use std::io::Write as _;

fn main() {
    // Bare tracing subscriber so the debug! lines from the cascade tiers
    // show up.
    let _ = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::stderr)
        .try_init();

    eprintln!("== platform-linux screenshot cascade smoke ==");
    eprintln!(
        "  WAYLAND_DISPLAY={:?}  DISPLAY={:?}  CUA_DRIVER_RS_ENABLE_WAYLAND={:?}",
        std::env::var("WAYLAND_DISPLAY").ok(),
        std::env::var("DISPLAY").ok(),
        std::env::var("CUA_DRIVER_RS_ENABLE_WAYLAND").ok(),
    );

    let started = std::time::Instant::now();
    match platform_linux::capture::screenshot_display_bytes() {
        Ok(bytes) => {
            let dt = started.elapsed();
            eprintln!(
                "OK  {} bytes  in {dt:?}  magic={:?}",
                bytes.len(),
                &bytes[..bytes.len().min(8)]
            );
            // Write the PNG so we can also visually check it.
            let path = std::env::var("CUA_SMOKE_OUT")
                .unwrap_or_else(|_| "/tmp/cua-screenshot.png".to_string());
            if let Ok(mut f) = std::fs::File::create(&path) {
                let _ = f.write_all(&bytes);
                eprintln!("  wrote {}", path);
            }
        }
        Err(e) => {
            eprintln!("ERR {}", e);
            std::process::exit(1);
        }
    }
}
