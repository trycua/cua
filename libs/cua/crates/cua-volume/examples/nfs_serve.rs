// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Serves a cua home's drive over localhost NFS and prints the port
//! (manual testing and the benchmark script).
//!
//!   CUA_HOME=<throwaway> cargo run --example nfs_serve --features s3,nfs
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let home = std::path::PathBuf::from(std::env::var("CUA_HOME").expect("set CUA_HOME"));
    let drive = cua_volume::Drive::open_local(&home);
    let vfs = cua_volume::vfs::Vfs::new(
        &drive,
        cua_volume::Context::user(),
        None,
        None,
        &home.join("drive"),
    )
    .expect("vfs");
    let server = cua_volume::nfs::NfsServer::start(vfs).await.expect("nfs");
    println!("port={} opts={}", server.port(), server.mount_options());
    tokio::signal::ctrl_c().await.ok();
    let _ = Arc::new(());
    server.stop().await.expect("stop");
}
