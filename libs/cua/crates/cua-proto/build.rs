//! Generates prost messages and tonic clients/servers from `libs/cua/proto`.
//!
//! Requires `protoc` on PATH (or `PROTOC` set). Well-known types map to
//! `pbjson_types`, which is wire-identical to `prost_types` and also
//! implements serde, so the generated types do not change with features.

use std::{env, fs, path::PathBuf};

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
    "cua/daemon/v1/runtime.proto",
    "cua/daemon/v1/daemon.proto",
];

fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    // canonicalize() yields a verbatim `\\?\D:\...` path on Windows, which
    // protoc cannot use as an include root ("File not found").
    let proto_root = strip_verbatim(manifest_dir.join("../../proto").canonicalize()?);
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

    tonic_prost_build::configure()
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
        .emit_rerun_if_changed(false)
        .compile_with_config(prost_config, &files, std::slice::from_ref(&proto_root))?;

    // Canonical proto3 JSON (serde) impls. Always generated so the file set
    // in OUT_DIR is feature-independent; included only with `serde`.
    let descriptor_set = fs::read(&descriptor_path)?;
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_set)?
        .extern_path(".google.protobuf", "::pbjson_types")
        .build(&[".cua"])?;

    Ok(())
}
