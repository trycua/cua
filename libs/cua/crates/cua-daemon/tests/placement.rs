//! Local placement through the one resolver, against an in-memory registry
//! (no network, nothing started). Its own binary: it installs a
//! process-wide registry source.

use std::sync::Arc;

use cua_daemon::local::resolve_placement;
use cua_image::testing::FakeRegistry;
use cua_vmm::{BackendKind, ImageSource};

#[tokio::test]
async fn refs_land_on_the_backend_their_variant_needs() {
    let mut r = FakeRegistry::default();
    let root = r.index(
        "ghcr.io/trycua/linux:24.04",
        &["amd64", "arm64"],
        false,
        None,
    );
    let disk = r.index(
        "ghcr.io/trycua/linux:24.04-disk",
        &["amd64", "arm64"],
        true,
        None,
    );
    let py = r.index(
        "docker.io/library/python:3.12-slim",
        &["amd64", "arm64"],
        false,
        None,
    );
    let win = r.index("ghcr.io/trycua/windows:2022-disk", &["amd64"], true, None);
    // The catalog's published full-tier macOS image: what `macos` means.
    let macos_full = cua_image::catalog::entries()
        .iter()
        .find(|e| {
            e.group == "canonical"
                && e.os == "macos"
                && e.tier.as_deref() == Some("full")
                && e.published
        })
        .expect("the catalog publishes a full-tier macOS image")
        .reference
        .clone();
    let mac = r.lume(&macos_full);
    r.darwin_index("ghcr.io/me/mac:1");
    cua_image::resolve::set_source(Some(Arc::new(r)));

    // Short ref: docker.io, pinned, container.
    let p = resolve_placement("python:3.12-slim", "linux")
        .await
        .unwrap();
    assert_eq!(p.placement.backend, BackendKind::Container);
    assert_eq!(
        p.placement.image,
        ImageSource::oci(format!("docker.io/library/python@{py}"))
    );

    // Canonical Linux: rootfs by default, the -disk sibling for vm:.
    let p = resolve_placement("ghcr.io/trycua/linux:24.04", "linux")
        .await
        .unwrap();
    assert_eq!(p.placement.backend, BackendKind::Container);
    assert_eq!(
        p.placement.image,
        ImageSource::oci(format!("ghcr.io/trycua/linux@{root}"))
    );
    let p = resolve_placement("vm:ghcr.io/trycua/linux:24.04", "linux")
        .await
        .unwrap();
    assert_eq!(p.placement.backend, BackendKind::Qemu);
    assert_eq!(
        p.placement.image,
        ImageSource::oci(format!("ghcr.io/trycua/linux@{disk}"))
    );
    assert_eq!(p.arch, Some(cua_vmm::Arch::host()));

    // A containerDisk ref runs on QEMU (amd64-only: emulated on arm64 hosts).
    let p = resolve_placement("ghcr.io/trycua/windows:2022", "windows")
        .await
        .unwrap();
    assert_eq!(p.placement.backend, BackendKind::Qemu);
    assert_eq!(
        p.placement.image,
        ImageSource::oci(format!("ghcr.io/trycua/windows@{win}"))
    );
    assert_eq!(p.arch, Some(cua_vmm::Arch::X86_64));

    // macOS: Lume pulls by tag with the resolved digest riding along (so a
    // republished tag does not keep cloning the old base); the `macos` alias
    // (expanded by the SDK before placement) is the catalog's full tier. A
    // darwin image in no Lume format is a typed error.
    let alias = cua_image::canonical::alias_with("macos", &|_| None).map(|(r, _)| r);
    assert_eq!(alias.as_deref(), Some(macos_full.as_str()));
    let p = resolve_placement(&macos_full, "macos").await.unwrap();
    assert_eq!(p.placement.backend, BackendKind::Lume);
    assert_eq!(
        p.placement.image,
        ImageSource::oci(format!("{macos_full}@{mac}"))
    );
    let found = p.image.expect("resolved through the registry");
    assert_eq!(found.variant_ref, macos_full);
    assert_eq!(found.digest, mac);
    let e = resolve_placement("ghcr.io/me/mac:1", "linux")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("macOS"), "{e}");

    // A local-only tag keeps the syntactic placement (the engine has it).
    let p = resolve_placement("cua-e2e-mcp-probe:1", "linux")
        .await
        .unwrap();
    assert_eq!(p.placement.backend, BackendKind::Container);
    assert_eq!(p.placement.image, ImageSource::oci("cua-e2e-mcp-probe:1"));
    assert!(p.image.is_none());
    cua_image::resolve::set_source(None);
}
