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
    println!("cargo:rustc-link-lib=framework=Security");

    println!("cargo:rerun-if-changed=src/browser/exception_shim.m");
    cc::Build::new()
        .file("src/browser/exception_shim.m")
        .flag("-fobjc-arc")
        .compile("cua_browser_exception_shim");

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
        println!("cargo:rustc-link-search=framework={sdk_root}/System/Library/Frameworks");
    }

    // The ScreenCaptureKit bindings pull in Swift runtime libraries such as
    // libswift_Concurrency.dylib. The cua-driver binary emits these rpaths in
    // its own build.rs, but platform-macos' unit-test binary is linked from
    // this package, so it needs the same runtime search paths here.
    emit_swift_runtime_link_args();
}

fn emit_swift_runtime_link_args() {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    let mut dirs = BTreeSet::new();

    if let Ok(out) = Command::new("xcode-select").arg("-p").output() {
        if out.status.success() {
            let xcode_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let developer_dir = PathBuf::from(xcode_path);
            for sub in [
                "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx",
                "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx",
                "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-6.2/macosx",
                "usr/lib/swift/macosx",
                "usr/lib/swift-5.5/macosx",
                "usr/lib/swift-6.2/macosx",
            ] {
                dirs.insert(developer_dir.join(sub));
            }
        }
    }

    if let Ok(out) = Command::new("xcrun").args(["--find", "swiftc"]).output() {
        if out.status.success() {
            let swiftc = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
            if let Some(usr_dir) = swiftc.parent().and_then(Path::parent) {
                for sub in [
                    "lib/swift/macosx",
                    "lib/swift-5.5/macosx",
                    "lib/swift-6.2/macosx",
                ] {
                    dirs.insert(usr_dir.join(sub));
                }
            }
        }
    }

    for dir in dirs {
        if dir.is_dir() {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
    }
}
