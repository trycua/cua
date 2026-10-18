use std::path::Path;

fn main() {
    // Make the released dylib loadable from any host directory (Python
    // wheels, npm platform packages, app bundles).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libcua_sdk.dylib");
    }

    // The SDK version is libs/cua/VERSION (release-please bumps it with the
    // Python and npm manifests; the Cargo workspace version is not released).
    // cd-cua-sdk.yml sets CUA_SDK_RELEASE_VERSION to the version it publishes
    // so the native library always reports the released version.
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
    println!("cargo:rustc-env=CUA_SDK_VERSION={version}");
}
