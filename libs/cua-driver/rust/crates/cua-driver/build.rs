// Bake Swift runtime rpaths into the cua-driver binary on macOS.
//
// The `screencapturekit` dep ships a small Swift-bridge shim that links
// against the Swift Concurrency runtime (`@rpath/libswift_Concurrency.dylib`
// and friends). Its own build.rs emits `cargo:rustc-link-arg=-Wl,-rpath,…`
// directives, but those only flow through to the binary linker when the
// emitting crate is the final binary crate — for transitive deps Cargo
// silently drops them. So we re-emit the same rpaths from here.
//
// On Windows, embed the Per-Monitor V2 DPI-awareness manifest so the
// process sees physical pixels (no DWM coordinate virtualization) at
// 125%/150%/200% scaling and clicks land where screenshots say they do.

fn main() {
    #[cfg(target_os = "windows")]
    {
        embed_resource::compile("cua-driver.rc", embed_resource::NONE);
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    if let Ok(out) = std::process::Command::new("xcode-select").arg("-p").output() {
        if out.status.success() {
            let xcode_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            for sub in [
                "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx",
                "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx",
            ] {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{xcode_path}/{sub}");
            }
        }
    }
}
