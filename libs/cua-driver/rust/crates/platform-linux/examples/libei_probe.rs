//! Portal/libei probe that establishes the session and enumerates readiness
//! without sending keyboard or pointer input.

fn main() {
    let send = std::env::args().any(|arg| arg == "--send");
    platform_linux::wayland::libei::ensure_started().expect("start libei worker");
    let keyboard = platform_linux::wayland::libei::wait_keyboard_ready();
    let pointer = platform_linux::wayland::libei::wait_pointer_ready();
    println!("keyboard={keyboard:?}");
    println!("pointer={pointer:?}");
    if send && keyboard.is_ok() {
        println!(
            "send={:?}",
            platform_linux::wayland::libei::type_text("CUA_STANDALONE_LIBEI_TEST")
        );
    }
    platform_linux::wayland::libei::shutdown();
}
