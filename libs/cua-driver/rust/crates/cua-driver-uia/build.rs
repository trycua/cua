// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

fn main() {
    #[cfg(target_os = "windows")]
    {
        embed_manifest::embed_manifest_file("cua-driver-uia.manifest")
            .expect("failed to embed cua-driver-uia.manifest");
        println!("cargo:rerun-if-changed=cua-driver-uia.manifest");
    }
}
