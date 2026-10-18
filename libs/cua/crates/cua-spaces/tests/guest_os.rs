//! A Fleet Space's guest OS through the one resolver, against an in-memory
//! registry (no network): the `ai.cua.image.os` annotation and the OCI
//! platform decide, not a "windows" in the reference. Its own binary: it
//! installs a process-wide registry source.

use std::sync::Arc;

use cua_image::resolve::OS_ANNOTATION;
use cua_image::testing::FakeRegistry;
use cua_spaces::fleet_runtime::{guest_os, resolve_image};
use serde_json::json;

async fn os_of(image: &str) -> String {
    let got = resolve_image(None, image).await.unwrap();
    let resolved = got.resolved.expect("resolved from the fake registry");
    guest_os(image, Some(&resolved.os))
}

#[tokio::test]
async fn label_and_platform_decide_the_guest_os() {
    let mut r = FakeRegistry::default();
    // A Linux containerDisk in a repository named "windows-tools".
    r.index(
        "registry.example/windows-tools:1",
        &["amd64"],
        true,
        Some(json!({OS_ANNOTATION: "linux"})),
    );
    // A Windows containerDisk whose name does not say so.
    r.index(
        "registry.example/desk:2022-disk",
        &["amd64"],
        true,
        Some(json!({OS_ANNOTATION: "windows"})),
    );
    // A rootfs whose OCI platform (config os) is windows, no annotation.
    let cfg = r.put_blob(&json!({"os": "windows", "architecture": "amd64",
        "config": {"Cmd": ["cmd"]}}));
    r.put_manifest(
        "registry.example/plain",
        Some("registry.example/plain:1"),
        &json!({"schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {"mediaType": cfg.media_type, "digest": cfg.digest, "size": cfg.size},
            "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": "sha256:00", "size": 1}]}),
    );
    cua_image::resolve::set_source(Some(Arc::new(r)));

    assert_eq!(os_of("registry.example/windows-tools:1").await, "linux");
    assert_eq!(os_of("registry.example/desk:2022-disk").await, "windows");
    assert_eq!(os_of("registry.example/plain:1").await, "windows");
}
