use std::path::Path;

fn main() {
    // The daemon reports the released cua SDK version (libs/cua/VERSION, or
    // CUA_SDK_RELEASE_VERSION from cd-cua-sdk.yml), like cua-sdk's build.rs,
    // so a client can tell that a running daemon is older than itself. The
    // Cargo workspace version is not released.
    println!("cargo:rerun-if-env-changed=CUA_SDK_RELEASE_VERSION");
    // Read at run time: a build-script binary reused from another checkout
    // sharing the target dir must not read that checkout's VERSION.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let version_file = Path::new(&manifest_dir).join("../../VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());
    let version = std::env::var("CUA_SDK_RELEASE_VERSION")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::fs::read_to_string(&version_file)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=CUA_DAEMON_VERSION={version}");
}
