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
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_version_info("cua-driver", "cua-driver", Some("cua-driver.manifest"));
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    if let Ok(out) = std::process::Command::new("xcode-select")
        .arg("-p")
        .output()
    {
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

fn embed_windows_version_info(
    file_description: &str,
    product_name: &str,
    manifest_file: Option<&str>,
) {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let version_u64 = windows_version_u64(&version);

    let mut resource = winresource::WindowsResource::new();
    resource.set("FileDescription", file_description);
    resource.set("ProductName", product_name);
    resource.set("CompanyName", "trycua");
    resource.set("FileVersion", &version);
    resource.set("ProductVersion", &version);
    if let Some(manifest_file) = manifest_file {
        resource.set_manifest_file(manifest_file);
        println!("cargo:rerun-if-changed={manifest_file}");
    }
    resource.set_version_info(winresource::VersionInfo::FILEVERSION, version_u64);
    resource.set_version_info(winresource::VersionInfo::PRODUCTVERSION, version_u64);
    resource
        .compile()
        .expect("failed to embed Windows VERSIONINFO");
}

fn windows_version_u64(version: &str) -> u64 {
    let mut parts = [0_u64; 4];
    for (idx, part) in version.split(['.', '-']).take(4).enumerate() {
        parts[idx] = part.parse::<u64>().unwrap_or(0).min(0xffff);
    }
    (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16) | parts[3]
}
