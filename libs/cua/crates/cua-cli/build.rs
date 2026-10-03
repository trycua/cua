//! Gives the `cua` binary's main thread an 8 MiB stack on Windows, the size
//! Linux and macOS already give it. Windows defaults to 1 MiB, and the main
//! thread parses the command tree and polls the top-level command future,
//! which overflowed it (STATUS_STACK_OVERFLOW) in unoptimized builds.

fn main() {
    const STACK: u32 = 8 * 1024 * 1024;
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" {
        if env == "msvc" {
            println!("cargo:rustc-link-arg-bins=/STACK:{STACK}");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
