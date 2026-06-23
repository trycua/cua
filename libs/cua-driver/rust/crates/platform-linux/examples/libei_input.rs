//! Standalone smoke for the libei input path on platform-linux.
//!
//! Drives `wayland::libei::move_absolute` + `click` and prints what happens.
//! Stress-tests the portal RemoteDesktop handshake plus the reis-based EIS
//! handshake on a real GNOME / KDE-Wayland host.
//!
//! Usage:
//!   CUA_DRIVER_RS_ENABLE_WAYLAND=1 \
//!   WAYLAND_DISPLAY=wayland-0 \
//!   cargo run --example libei_input --release

fn main() {
    let _ = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::stderr)
        .try_init();

    eprintln!("== platform-linux libei input smoke ==");
    eprintln!(
        "  WAYLAND_DISPLAY={:?}  XDG_RUNTIME_DIR={:?}  DBUS_SESSION_BUS_ADDRESS={:?}",
        std::env::var("WAYLAND_DISPLAY").ok(),
        std::env::var("XDG_RUNTIME_DIR").ok(),
        std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(),
    );

    // Try a no-op move first — opens the portal handshake + EIS context.
    eprintln!("\n-- move_absolute(500, 400) --");
    let t = std::time::Instant::now();
    match platform_linux::wayland::libei::move_absolute(500.0, 400.0) {
        Ok(()) => eprintln!("  OK in {:?}", t.elapsed()),
        Err(e) => eprintln!("  ERR after {:?}: {}", t.elapsed(), e),
    }

    eprintln!("\n-- click(500, 400, Left) --");
    let t = std::time::Instant::now();
    match platform_linux::wayland::libei::click(
        500.0, 400.0,
        platform_linux::wayland::libei::Button::Left,
    ) {
        Ok(()) => eprintln!("  OK in {:?}", t.elapsed()),
        Err(e) => eprintln!("  ERR after {:?}: {}", t.elapsed(), e),
    }

    eprintln!("\n-- type_text('hello world') --");
    let t = std::time::Instant::now();
    match platform_linux::wayland::libei::type_text("hello world") {
        Ok(()) => eprintln!("  OK in {:?}", t.elapsed()),
        Err(e) => eprintln!("  ERR after {:?}: {}", t.elapsed(), e),
    }

    eprintln!("\n-- shutdown --");
    platform_linux::wayland::libei::shutdown();
    // Give the worker a moment to drain.
    std::thread::sleep(std::time::Duration::from_millis(500));
    eprintln!("done");
}
