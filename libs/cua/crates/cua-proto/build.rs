//! Generates prost messages and tonic clients/servers from the contract.
//!
//! The .proto files are `libs/cua/proto` in the repository and `proto/` in
//! the published crate (scripts/vendor-crate-protos.sh copies them there
//! before `cargo package`). Uses `protoc` from `PROTOC` or PATH when there is
//! one, and otherwise compiles the files in-process with protox, so a crate
//! from crates.io builds without a system protoc. Well-known types map to
//! `pbjson_types`, which is wire-identical to `prost_types` and also
//! implements serde, so the generated types do not change with features.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// Every .proto file of the contract. `tests/descriptor.rs` asserts this list
/// matches the files on disk, so a new file cannot be silently left out.
const PROTOS: &[&str] = &[
    "cua/env/v1/common.proto",
    "cua/env/v1/system.proto",
    "cua/env/v1/diagnose.proto",
    "cua/env/v1/process.proto",
    "cua/env/v1/filesystem.proto",
    "cua/env/v1/computer.proto",
    "cua/env/v1/windows.proto",
    "cua/env/v1/accessibility.proto",
    "cua/env/v1/driver.proto",
    "cua/env/v1/stream.proto",
    "cua/env/v1/presence.proto",
    "cua/env/v1/teleport.proto",
    "cua/env/v1/tunnel.proto",
    "cua/env/v1/volume.proto",
    "cua/env/v1/host.proto",
    "cua/daemon/v1/sandboxes.proto",
    "cua/daemon/v1/spaces.proto",
    "cua/daemon/v1/resources.proto",
    "cua/daemon/v1/runtime.proto",
    "cua/daemon/v1/daemon.proto",
];

fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// `libs/cua/proto` in the repository (always the source of truth there, so
/// a stale vendored copy is never used); `proto/` in the published crate.
fn proto_root(manifest_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let marker = Path::new(PROTOS[0]);
    let root = [manifest_dir.join("../../proto"), manifest_dir.join("proto")]
        .into_iter()
        .find(|dir| dir.join(marker).is_file())
        .ok_or("cua-proto: no .proto files in ../../proto or proto/")?;
    // canonicalize() yields a verbatim `\\?\D:\...` path on Windows, which
    // protoc cannot use as an include root ("File not found").
    Ok(strip_verbatim(root.canonicalize()?))
}

/// A protoc to run: `PROTOC` when set, else `protoc` when it is on PATH.
/// (No rerun on PATH changes: both paths generate the same code.)
fn has_protoc() -> bool {
    if env::var_os("PROTOC").is_some() {
        return true;
    }
    let exe = if cfg!(windows) {
        "protoc.exe"
    } else {
        "protoc"
    };
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(exe).is_file()))
        .unwrap_or(false)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = proto_root(&manifest_dir)?;
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let descriptor_path = out_dir.join("cua_descriptor.bin");

    let files: Vec<PathBuf> = PROTOS.iter().map(|p| proto_root.join(p)).collect();
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        proto_root.join("buf.yaml").display()
    );
    println!("cargo:rerun-if-env-changed=PROTOC");

    let mut prost_config = prost_build::Config::new();
    prost_config.prost_types_path("::pbjson_types");

    let builder = tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        // No tonic::transport references: the generated code must also
        // compile for wasm32 (gRPC-Web in the browser) where the SDK brings
        // its own transport.
        .build_transport(false)
        .client_mod_attribute(".", r#"#[cfg(feature = "client")]"#)
        .server_mod_attribute(".", r#"#[cfg(feature = "server")]"#)
        .compile_well_known_types(false)
        .file_descriptor_set_path(&descriptor_path)
        .emit_rerun_if_changed(false);
    if has_protoc() {
        builder.compile_with_config(prost_config, &files, std::slice::from_ref(&proto_root))?;
    } else {
        // No protoc: protox parses the same files into the descriptor set
        // protoc would hand to prost (with source info, for doc comments
        // and reflection), imports included as protoc --include_imports does.
        let fds = protox::Compiler::new(std::slice::from_ref(&proto_root))?
            .include_imports(true)
            .include_source_info(true)
            .open_files(&files)?
            .file_descriptor_set();
        // compile_fds does not write file_descriptor_set_path.
        fs::write(&descriptor_path, prost::Message::encode_to_vec(&fds))?;
        builder.compile_fds_with_config(fds, prost_config)?;
    }

    // Canonical proto3 JSON (serde) impls. Always generated so the file set
    // in OUT_DIR is feature-independent; included only with `serde`.
    let descriptor_set = fs::read(&descriptor_path)?;
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_set)?
        .extern_path(".google.protobuf", "::pbjson_types")
        .build(&[".cua"])?;

    Ok(())
}
