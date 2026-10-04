//! The Fleet image rule through the one resolver, against an in-memory
//! registry (no network). Its own test binary: it installs a process-wide
//! registry source and must not see `cua_fleet::testing` fixtures.

use std::sync::Arc;

use cua_fleet::{RuntimeKind, resolve_fleet_image};
use cua_image::testing::FakeRegistry;

#[tokio::test]
async fn canonical_images_pick_the_runtime_variant_and_pin_it() {
    let mut r = FakeRegistry::default();
    let disk = r.index(
        "ghcr.io/trycua/linux:24.04-disk",
        &["amd64", "arm64"],
        true,
        None,
    );
    let root = r.index(
        "ghcr.io/trycua/linux:24.04",
        &["amd64", "arm64"],
        false,
        None,
    );
    let win = r.index("ghcr.io/trycua/windows:2022-disk", &["amd64"], true, None);
    r.lume("ghcr.io/trycua/macos:26");
    r.darwin_index("registry.example/mac:1");
    cua_image::resolve::set_source(Some(Arc::new(r)));

    let got = resolve_fleet_image(None, "ghcr.io/trycua/linux:24.04")
        .await
        .unwrap();
    assert_eq!(got.runtime, RuntimeKind::Gvisor);
    assert_eq!(got.image, format!("ghcr.io/trycua/linux@{root}"));

    let got = resolve_fleet_image(Some(RuntimeKind::Kubevirt), "ghcr.io/trycua/linux:24.04")
        .await
        .unwrap();
    assert_eq!(got.runtime, RuntimeKind::Kubevirt);
    assert_eq!(got.image, format!("ghcr.io/trycua/linux@{disk}"));

    // Windows has only a containerDisk: the default runtime is kubevirt...
    let got = resolve_fleet_image(None, "ghcr.io/trycua/windows:2022")
        .await
        .unwrap();
    assert_eq!(got.runtime, RuntimeKind::Kubevirt);
    assert_eq!(got.image, format!("ghcr.io/trycua/windows@{win}"));
    assert_eq!(got.resolved.unwrap().os, "windows");
    // ...and gvisor is a crossed pairing, refused before any Fleet call.
    let e = resolve_fleet_image(Some(RuntimeKind::Gvisor), "ghcr.io/trycua/windows:2022")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("cannot run"), "{e}");

    // macOS is not offered on Fleet: a Lume image, a darwin image, or the
    // `macos` runtime with any image is a typed Unsupported, before any
    // Fleet call.
    for (runtime, image) in [
        (None, "ghcr.io/trycua/macos:26"),
        (Some(RuntimeKind::Kubevirt), "ghcr.io/trycua/macos:26"),
        (Some(RuntimeKind::Gvisor), "registry.example/mac:1"),
        (None, "registry.example/mac:1"),
        (Some(RuntimeKind::Macos), "ghcr.io/trycua/linux:24.04"),
        (
            Some(RuntimeKind::Macos),
            "registry.example/private:docker-1",
        ),
    ] {
        let e = resolve_fleet_image(runtime.clone(), image)
            .await
            .unwrap_err();
        assert!(
            matches!(e, cua_fleet::Error::Unsupported(_)),
            "{runtime:?} {image}: {e}"
        );
        assert_eq!(
            e.to_string(),
            format!("unsupported: {}", cua_fleet::MACOS_UNSUPPORTED)
        );
    }

    // Unreadable (not in the registry): kept as given, runtime from the reference.
    let got = resolve_fleet_image(None, "registry.example/private:docker-1")
        .await
        .unwrap();
    assert_eq!(got.image, "registry.example/private:docker-1");
    assert_eq!(got.runtime, RuntimeKind::Gvisor);
    assert!(got.resolved.is_none());
    cua_image::resolve::set_source(None);
}
