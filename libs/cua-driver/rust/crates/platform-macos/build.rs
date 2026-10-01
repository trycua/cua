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

    // `swiftc` links against the SDK's Swift TBDs on current Xcode releases.
    // The toolchain runtime directories below contain compatibility libraries,
    // but are not guaranteed to contain the full Swift runtime surface.
    if let Some(sdk_root) = active_macos_sdk_root() {
        let sdk_swift_dir = sdk_root.join("usr/lib/swift");
        if sdk_swift_dir.is_dir() {
            println!("cargo:rustc-link-search=native={}", sdk_swift_dir.display());
        }
    }

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

fn active_macos_sdk_root() -> Option<std::path::PathBuf> {
    std::env::var_os("SDKROOT")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| {
            let out = std::process::Command::new("xcrun")
                .args(["--sdk", "macosx", "--show-sdk-path"])
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
                .filter(|path| path.is_dir())
        })
}
