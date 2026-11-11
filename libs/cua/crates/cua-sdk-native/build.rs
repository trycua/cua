use std::path::Path;

fn main() {
    // Make the released dylib loadable from any host directory (Python
    // wheels, npm platform packages, app bundles).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libcua_sdk.dylib");
    }

    // cua-sdk and this crate are both the library `cua_sdk`, and UniFFI finds
    // a library's binding settings by that name in whichever crate cargo
    // metadata lists last. Both crates carry the same uniffi.toml.
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let here = Path::new(&dir).join("uniffi.toml");
    let sdk = Path::new(&dir).join("../cua-sdk/uniffi.toml");
    println!("cargo:rerun-if-changed={}", here.display());
    println!("cargo:rerun-if-changed={}", sdk.display());
    if let (Ok(a), Ok(b)) = (std::fs::read(&here), std::fs::read(&sdk)) {
        assert_eq!(
            a, b,
            "crates/cua-sdk-native/uniffi.toml must equal crates/cua-sdk/uniffi.toml (copy it)"
        );
    }
}
