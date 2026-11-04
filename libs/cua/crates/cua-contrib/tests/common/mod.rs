//! Shared hermetic setup: an in-memory registry with a desktop-like image
//! (a rootfs labelled with cua-spacesd, as the canonical images are) and a
//! mock cua-spacesd.

#![allow(dead_code)]

use cua_image::testing::FakeRegistry;
use serde_json::json;
use std::sync::Arc;

pub const IMAGE: &str = "ghcr.io/trycua/linux:24.04";
pub const MOVED: &str = "ghcr.io/trycua/linux:moved";

fn put_image(r: &mut FakeRegistry, tag: &str, marker: &str) {
    let cfg = r.put_blob(&json!({"os": "linux", "architecture": "amd64",
        "config": {"Entrypoint": ["/entrypoint.sh"], "Env": [format!("MARK={marker}")],
            "Labels": {"ai.cua.spacesd": "true"}}}));
    let m = json!({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": cfg.media_type, "digest": cfg.digest, "size": cfg.size},
        "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": "sha256:00", "size": 1}]});
    r.put_manifest("ghcr.io/trycua/linux", Some(tag), &m);
}

/// Installs the in-memory registry (process-wide; every test installs the
/// same content).
pub fn registry() {
    let mut r = FakeRegistry::default();
    put_image(&mut r, IMAGE, "a");
    // Same repository, different digest: a moved tag.
    put_image(&mut r, MOVED, "b");
    r.index("ghcr.io/example/vm-only:1", &["amd64"], true, None);
    cua_image::resolve::set_source(Some(Arc::new(r)));
}

/// The image config the providers read (the desktop image's entrypoint).
pub fn image_config() -> cua_contrib::image_config::ImageConfig {
    cua_contrib::image_config::ImageConfig {
        entrypoint: vec!["/entrypoint.sh".into()],
        ..Default::default()
    }
}
