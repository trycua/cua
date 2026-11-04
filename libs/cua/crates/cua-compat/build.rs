//! Generates the Modal gRPC server (`modal.client.ModalClient` and
//! `modal.task_command_router.TaskCommandRouter`) from the committed
//! descriptor set of the pinned `modal` client (see `modal-proto/`).
//!
//! No `.proto` sources and no protoc: the descriptors come from the
//! generated modules the Apache-2.0 `modal` wheel ships, extracted by
//! `modal-proto/extract_descriptors.py`. Every RPC gets a default
//! implementation that answers `UNIMPLEMENTED`; `src/modal` overrides the
//! ones the gateway serves.

use prost::Message;
use std::{env, fs, path::PathBuf};

/// The pinned client the descriptors come from.
const MODAL_VERSION: &str = "1.6.2";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("modal-proto");
    let fds_path = dir.join(format!("modal-{MODAL_VERSION}.fds"));
    println!("cargo:rerun-if-changed={}", fds_path.display());
    println!("cargo:rerun-if-changed=build.rs");
    let fds = prost_types::FileDescriptorSet::decode(fs::read(&fds_path)?.as_slice())?;

    let mut config = prost_build::Config::new();
    config.bytes(["."]);
    tonic_prost_build::configure()
        // Client stubs only for tests (tests/modal_wire.rs).
        .build_client(std::env::var_os("CARGO_FEATURE_TESTING").is_some())
        .build_server(true)
        .build_transport(false)
        .generate_default_stubs(true)
        .compile_well_known_types(false)
        .emit_rerun_if_changed(false)
        .compile_fds_with_config(fds, config)?;
    Ok(())
}
