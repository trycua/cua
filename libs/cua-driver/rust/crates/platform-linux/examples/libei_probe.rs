//! Portal/libei probe that establishes the session and enumerates readiness
//! without sending keyboard or pointer input.

fn main() {
    platform_linux::wayland::libei::ensure_started().expect("start libei worker");
    let keyboard = platform_linux::wayland::libei::wait_keyboard_ready();
    let pointer = platform_linux::wayland::libei::wait_pointer_ready();
    let scroll = platform_linux::wayland::libei::wait_scroll_ready();
    println!("keyboard={keyboard:?}");
    println!("pointer={pointer:?}");
    println!("scroll={scroll:?}");
    platform_linux::wayland::libei::shutdown();
}
