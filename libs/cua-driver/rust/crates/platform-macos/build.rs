fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }
    // Force AppKit, Foundation, QuartzCore and CoreGraphics to be linked.
    // objc2-app-kit's empty extern block is not enough to propagate
    // framework links through Cargo's dependency graph on all toolchain
    // versions, so we emit the directives explicitly here.
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=QuartzCore");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");

    // Point the linker at the SDK's system sub-library directory so
    // `#[link(name = "dispatch", kind = "dylib")]` can find libdispatch.tbd.
    // The path is stable across Xcode / CLT installs; SDKROOT takes precedence
    // if set (Xcode builds set it automatically).
    let sdk_root = std::env::var("SDKROOT").unwrap_or_else(|_| {
        // Fall back to the active CLT SDK.
        let out = std::process::Command::new("xcrun")
            .args(["--sdk", "macosx", "--show-sdk-path"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default();
        out.trim().to_owned()
    });

    if !sdk_root.is_empty() {
        println!("cargo:rustc-link-search={sdk_root}/usr/lib/system");
        println!("cargo:rustc-link-search={sdk_root}/usr/lib");
    }
}
