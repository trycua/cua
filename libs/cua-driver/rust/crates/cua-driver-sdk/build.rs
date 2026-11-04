fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libcua_driver_sdk.dylib");

        // ScreenCaptureKit's Swift bridge links libswift_Concurrency through
        // @rpath. Linker arguments emitted by a transitive dependency do not
        // reach this final cdylib, so make the released SDK independently
        // loadable by Python, Node, and other hosts. @loader_path also permits
        // a future package to colocate a back-deployed Swift runtime without
        // changing the public ABI or loader contract.
        // rustc-link-arg applies to both the cdylib and unit-test executable.
        // The enclosing workspace may already supply the Swift path via
        // rustflags (for example cua-spacesd); avoid emitting it twice.
        let rustflags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
        if !rustflags
            .split('\x1f')
            .any(|arg| arg == "link-arg=-Wl,-rpath,/usr/lib/swift")
        {
            println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
    }
}
