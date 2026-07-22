use std::path::PathBuf;

fn main() {
    let check = std::env::args()
        .skip(1)
        .any(|argument| argument == "--check");
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical Rust workspace");
    let crate_dir = workspace.join("crates/cua-driver-sdk");
    let config_path = crate_dir.join("cbindgen.toml");
    let output_path = workspace.join("include/cua_driver_abi.h");
    let config = cbindgen::Config::from_file(&config_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", config_path.display()));
    let bindings = cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_config(config)
        .generate()
        .expect("generate Cua Driver ABI header");
    let mut generated = Vec::new();
    bindings.write(&mut generated);
    let generated = String::from_utf8(generated).expect("cbindgen emitted UTF-8");
    // `Option<extern "C" fn>` is ABI-compatible with a nullable C function
    // pointer. cbindgen currently emits an opaque Option typedef for an alias;
    // present the actual callback type in the public C declaration instead.
    let generated = generated
        .replace(
            "typedef struct Option_CuaDriverCompletionV1 Option_CuaDriverCompletionV1;\n\n",
            "",
        )
        .replace(
            "Option_CuaDriverCompletionV1 callback",
            "CuaDriverCompletionV1 callback",
        );

    if check {
        let existing = std::fs::read_to_string(&output_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", output_path.display()));
        if existing != generated {
            eprintln!(
                "{} is stale; run `cargo run -p cua-driver-bindgen --bin cua-driver-abi-header`",
                output_path.display()
            );
            std::process::exit(1);
        }
    } else {
        std::fs::write(&output_path, generated)
            .unwrap_or_else(|error| panic!("write {}: {error}", output_path.display()));
        println!("generated {}", output_path.display());
    }
}
