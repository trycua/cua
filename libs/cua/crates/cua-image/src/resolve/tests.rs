//! Resolver fixtures: an in-memory registry with the canonical layout.

use serde_json::json;

use super::*;
use crate::media_types::OCI_MANIFEST;
use crate::testing::FakeRegistry;

const LINUX: &str = "ghcr.io/trycua/linux:24.04";
const LINUX_DISK: &str = "ghcr.io/trycua/linux:24.04-disk";

/// The canonical Linux layout: a rootfs index, a `-disk` index, and the
/// variants annotation on the primary (when `annotate`).
fn canonical(annotate: bool) -> (FakeRegistry, String, String) {
    let mut r = FakeRegistry::default();
    let disk = r.index(LINUX_DISK, &["amd64", "arm64"], true, None);
    let ann = annotate.then(|| {
        json!({VARIANTS_ANNOTATION: json!({"containerdisk": format!("ghcr.io/trycua/linux@{disk}")}).to_string()})
    });
    let root = r.index(LINUX, &["amd64", "arm64"], false, ann);
    (r, root, disk)
}

async fn go(r: &FakeRegistry, reference: &str, b: Backend, arch: &str) -> Result<ResolvedImage> {
    resolve_with(r, reference, b, arch).await
}

#[tokio::test]
async fn short_refs_are_docker_hub() {
    let mut r = FakeRegistry::default();
    let d = r.index(
        "docker.io/library/python:3.12-slim",
        &["amd64", "arm64"],
        false,
        None,
    );
    let got = go(&r, "python:3.12-slim", Backend::Local, "aarch64")
        .await
        .unwrap();
    assert_eq!(got.reference, "docker.io/library/python:3.12-slim");
    assert_eq!(got.pinned_ref, format!("docker.io/library/python@{d}"));
    assert_eq!(got.variant, Variant::Rootfs);
    assert_eq!(got.arch.as_deref(), Some("arm64"));
    assert!(!got.emulated);
    assert_eq!(got.os, "linux");
    assert_eq!(
        normalize("ubuntu").unwrap(),
        "docker.io/library/ubuntu:latest"
    );
    assert_eq!(
        normalize("localhost:5000/repo:1").unwrap(),
        "localhost:5000/repo:1",
        "a host:port prefix is a registry, not ghcr.io/trycua/localhost"
    );
}

#[tokio::test]
async fn canonical_index_picks_rootfs_for_containers() {
    let (r, root, _) = canonical(true);
    for b in [
        Backend::Auto,
        Backend::Local,
        Backend::Fleet,
        Backend::Container,
    ] {
        let got = go(&r, LINUX, b, "arm64").await.unwrap();
        assert_eq!(got.variant, Variant::Rootfs, "{b:?}");
        assert_eq!(got.pinned_ref, format!("ghcr.io/trycua/linux@{root}"));
        assert_eq!(got.variant_ref, LINUX);
    }
}

#[tokio::test]
async fn vm_follows_the_variants_annotation() {
    let (r, _, disk) = canonical(true);
    let got = go(&r, LINUX, Backend::Vm, "amd64").await.unwrap();
    assert_eq!(got.variant, Variant::Containerdisk);
    assert_eq!(got.pinned_ref, format!("ghcr.io/trycua/linux@{disk}"));
    assert_eq!(got.digest, disk);
    assert_eq!(got.reference, LINUX);
}

#[tokio::test]
async fn vm_falls_back_to_the_disk_sibling_tag() {
    let (r, _, disk) = canonical(false);
    let got = go(&r, LINUX, Backend::Vm, "arm64").await.unwrap();
    assert_eq!(got.variant, Variant::Containerdisk);
    assert_eq!(got.variant_ref, LINUX_DISK);
    assert_eq!(got.pinned_ref, format!("ghcr.io/trycua/linux@{disk}"));
    // And back: a -disk ref for a container backend strips the suffix.
    let got = go(&r, LINUX_DISK, Backend::Container, "arm64")
        .await
        .unwrap();
    assert_eq!(got.variant, Variant::Rootfs);
    assert_eq!(got.variant_ref, LINUX);
    // Local runs the disk as given (QEMU).
    let got = go(&r, LINUX_DISK, Backend::Local, "arm64").await.unwrap();
    assert_eq!(got.variant, Variant::Containerdisk);
}

/// Tiers sit before the variant suffix, so `24.04-slim` finds
/// `24.04-slim-disk` and back, and never the full tier's disk.
#[tokio::test]
async fn tier_tags_keep_their_disk_sibling() {
    let (mut r, _, _) = canonical(false);
    let slim = "ghcr.io/trycua/linux:24.04-slim";
    let slim_disk = "ghcr.io/trycua/linux:24.04-slim-disk";
    let disk = r.index(slim_disk, &["amd64", "arm64"], true, None);
    let root = r.index(slim, &["amd64", "arm64"], false, None);
    let got = go(&r, slim, Backend::Vm, "arm64").await.unwrap();
    assert_eq!(got.variant_ref, slim_disk);
    assert_eq!(got.pinned_ref, format!("ghcr.io/trycua/linux@{disk}"));
    let got = go(&r, slim_disk, Backend::Container, "arm64")
        .await
        .unwrap();
    assert_eq!(got.variant, Variant::Rootfs);
    assert_eq!(got.variant_ref, slim);
    assert_eq!(got.pinned_ref, format!("ghcr.io/trycua/linux@{root}"));
    // A macOS xcode tier with its version keeps the whole tier in the tag.
    let x = format!("{}:26-xcode-26.1-disk", crate::canonical::MACOS_REPO);
    let n = NormalizedRef::parse(&x).unwrap();
    assert_eq!(n.repo(), "ghcr.io/trycua/macos");
}

#[tokio::test]
async fn fleet_is_amd64_and_flags_emulation() {
    let mut r = FakeRegistry::default();
    r.index("ghcr.io/me/armonly:1", &["arm64"], false, None);
    let got = go(&r, "ghcr.io/me/armonly:1", Backend::Fleet, "amd64")
        .await
        .unwrap();
    assert_eq!(got.arch.as_deref(), Some("arm64"));
    assert!(got.emulated);
    let (r, _, _) = canonical(true);
    let got = go(&r, LINUX, Backend::Fleet, "amd64").await.unwrap();
    assert_eq!(got.arch.as_deref(), Some("amd64"));
    assert_eq!(got.architectures, vec!["amd64", "arm64"]);
}

#[tokio::test]
async fn missing_primary_tag_uses_the_disk_sibling() {
    let mut r = FakeRegistry::default();
    let disk = r.index("ghcr.io/trycua/windows:2022-disk", &["amd64"], true, None);
    let got = go(&r, "ghcr.io/trycua/windows:2022", Backend::Local, "arm64")
        .await
        .unwrap();
    assert_eq!(got.variant, Variant::Containerdisk);
    assert_eq!(got.pinned_ref, format!("ghcr.io/trycua/windows@{disk}"));
    assert_eq!(got.os, "windows");
    assert!(got.emulated);
    // No rootfs for Windows: a typed error naming what exists.
    let e = go(
        &r,
        "ghcr.io/trycua/windows:2022-disk",
        Backend::Container,
        "amd64",
    )
    .await
    .unwrap_err();
    match e {
        ImageError::UnsupportedVariant { found, backend, .. } => {
            assert_eq!(found, vec!["containerdisk"]);
            assert_eq!(backend, "container");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn lume_and_other_macos() {
    let mut r = FakeRegistry::default();
    let lume = r.lume("ghcr.io/trycua/macos:26");
    let got = go(&r, "ghcr.io/trycua/macos:26", Backend::Local, "arm64")
        .await
        .unwrap();
    assert_eq!(got.variant, Variant::Lume);
    assert_eq!(got.os, "macos");
    assert_eq!(got.pinned_ref, format!("ghcr.io/trycua/macos@{lume}"));
    assert_eq!(got.variant_ref, "ghcr.io/trycua/macos:26");
    // Fleet runs no macOS variant: typed error.
    let e = go(&r, "ghcr.io/trycua/macos:26", Backend::Fleet, "amd64")
        .await
        .unwrap_err();
    match &e {
        ImageError::UnsupportedVariant { found, reason, .. } => {
            assert_eq!(found, &vec!["lume".to_string()]);
            assert!(reason.contains("run locally with Lume"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    // A darwin image in no Lume format: typed unsupported on every backend,
    // not a hang or a container.
    let mut r = FakeRegistry::default();
    r.darwin_index("ghcr.io/me/mac:1");
    for (backend, says) in [
        (Backend::Local, "cannot run here"),
        (Backend::Auto, "cannot run here"),
        (Backend::Fleet, "Fleet does not offer macOS sandboxes"),
    ] {
        let e = go(&r, "ghcr.io/me/mac:1", backend, "arm64")
            .await
            .unwrap_err();
        match &e {
            ImageError::UnsupportedVariant { found, reason, .. } => {
                assert_eq!(found, &vec![MACOS_FOUND.to_string()]);
                assert!(reason.contains(says), "{backend:?}: {reason}");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn digest_refs_pin_to_themselves() {
    let (r, root, _) = canonical(false);
    let pinned = format!("ghcr.io/trycua/linux@{root}");
    let got = go(&r, &pinned, Backend::Container, "arm64").await.unwrap();
    assert_eq!(got.pinned_ref, pinned);
    assert_eq!(got.digest, root);
    // No tag: no sibling to look up.
    let e = go(&r, &pinned, Backend::Vm, "arm64").await.unwrap_err();
    assert!(matches!(e, ImageError::UnsupportedVariant { .. }), "{e}");
}

#[tokio::test]
async fn local_only_tags_are_not_found() {
    let r = FakeRegistry::default();
    let e = go(&r, "cua-e2e-mcp-probe:1", Backend::Local, "arm64")
        .await
        .unwrap_err();
    assert!(matches!(e, ImageError::NotFound(_)), "{e}");
    assert!(
        e.to_string()
            .contains("docker.io/library/cua-e2e-mcp-probe:1"),
        "{e}"
    );
}

#[tokio::test]
async fn docker_hub_401_means_not_found() {
    // Docker Hub answers 401 for a repository that does not exist.
    let mut r = FakeRegistry::default();
    r.refuse
        .insert("docker.io/library/cua-e2e-probe:1".into(), "unauthorized");
    let e = go(&r, "cua-e2e-probe:1", Backend::Local, "arm64")
        .await
        .unwrap_err();
    assert!(matches!(e, ImageError::NotFound(_)), "{e}");
}

#[tokio::test]
async fn refused_credentials_are_typed() {
    let mut r = FakeRegistry::default();
    r.refuse
        .insert("ghcr.io/me/private:1".into(), "unauthorized");
    let e = go(&r, "ghcr.io/me/private:1", Backend::Local, "arm64")
        .await
        .unwrap_err();
    assert!(matches!(e, ImageError::Unauthorized(_)), "{e}");
}

#[tokio::test]
async fn unknown_artifacts_are_typed() {
    let mut r = FakeRegistry::default();
    let c = r.put_blob(&json!({}));
    let m = json!({"schemaVersion": 2, "mediaType": OCI_MANIFEST,
        "config": {"mediaType": crate::media_types::QEMU_CONFIG, "digest": c.digest, "size": c.size},
        "layers": [{"mediaType": crate::media_types::QEMU_DISK, "digest": "sha256:2", "size": 1}]});
    r.put_manifest("ghcr.io/me/q", Some("ghcr.io/me/q:1"), &m);
    let e = go(&r, "ghcr.io/me/q:1", Backend::Local, "arm64")
        .await
        .unwrap_err();
    match e {
        ImageError::UnsupportedVariant { found, reason, .. } => {
            assert!(found.is_empty());
            assert!(reason.contains("not a runnable image"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn backend_and_variant_names() {
    for (s, b) in [
        ("gvisor", Backend::Container),
        ("kubevirt", Backend::Vm),
        ("qemu", Backend::Vm),
        ("", Backend::Auto),
        ("fleet", Backend::Fleet),
    ] {
        assert_eq!(Backend::parse(s).unwrap(), b);
    }
    assert!(Backend::parse("kvm").is_err());
    for v in Variant::ALL {
        assert_eq!(Variant::parse(v.as_str()), Some(v));
    }
    let n = NormalizedRef::parse("ghcr.io/trycua/linux:24.04-disk").unwrap();
    assert_eq!(
        n.sibling(Variant::Rootfs).unwrap(),
        "ghcr.io/trycua/linux:24.04"
    );
    assert_eq!(
        n.sibling(Variant::Lume).unwrap(),
        "ghcr.io/trycua/linux:24.04-lume"
    );
}

#[tokio::test]
async fn spacesd_is_read_from_exposed_ports_and_labels() {
    let (r, _, _) = canonical(false);
    // A rootfs that does not expose 3211 has no spacesd.
    let got = go(&r, LINUX, Backend::Container, "arm64").await.unwrap();
    assert_eq!(got.spacesd, Some(false));
    // A containerDisk without the label: unknown.
    let got = go(&r, LINUX, Backend::Vm, "arm64").await.unwrap();
    assert_eq!(got.spacesd, None);
    // EXPOSE 3211 (libs/images/linux/Dockerfile).
    let mut r = FakeRegistry::default();
    let cfg = r.put_blob(&json!({"os": "linux", "architecture": "arm64",
        "config": {"Cmd": ["/init"], "ExposedPorts": {"3211/tcp": {}, "5901/tcp": {}}}}));
    let m = json!({"schemaVersion": 2, "mediaType": OCI_MANIFEST,
        "config": {"mediaType": cfg.media_type, "digest": cfg.digest, "size": cfg.size},
        "layers": []});
    r.put_manifest("ghcr.io/me/desk", Some("ghcr.io/me/desk:1"), &m);
    let got = go(&r, "ghcr.io/me/desk:1", Backend::Local, "arm64")
        .await
        .unwrap();
    assert_eq!(got.spacesd, Some(true));
}

#[tokio::test]
async fn os_annotation_wins_over_the_repository_name() {
    let mut r = FakeRegistry::default();
    // A Linux containerDisk in a repo whose name says "windows".
    r.index(
        "ghcr.io/me/windows-tools:1",
        &["amd64"],
        true,
        Some(json!({OS_ANNOTATION: "linux"})),
    );
    let got = go(&r, "ghcr.io/me/windows-tools:1", Backend::Vm, "amd64")
        .await
        .unwrap();
    assert_eq!(got.os, "linux");
    // A Windows containerDisk in a repo whose name does not say so.
    r.index(
        "ghcr.io/me/desk:2022-disk",
        &["amd64"],
        true,
        Some(json!({OS_ANNOTATION: "windows"})),
    );
    let got = go(&r, "ghcr.io/me/desk:2022-disk", Backend::Vm, "amd64")
        .await
        .unwrap();
    assert_eq!(got.os, "windows");
    // Without the annotation the repository name is the fallback.
    r.index("ghcr.io/me/windows:x-disk", &["amd64"], true, None);
    let got = go(&r, "ghcr.io/me/windows:x-disk", Backend::Vm, "amd64")
        .await
        .unwrap();
    assert_eq!(got.os, "windows");
}

#[tokio::test]
async fn spacesd_annotation_values_are_read() {
    let mut r = FakeRegistry::default();
    // Images built before the cua-spacesd rename carry an older label.
    for label in std::iter::once(SPACESD_LABEL).chain(LEGACY_SPACESD_LABELS) {
        for (tag, value, want) in [
            ("t", "true", Some(true)),
            ("p", "3211", Some(true)),
            ("f", "false", Some(false)),
            ("z", "0", Some(false)),
        ] {
            let reference = format!("ghcr.io/me/vm:{tag}-{}-disk", label.len());
            r.index(&reference, &["amd64"], true, Some(json!({label: value})));
            let got = go(&r, &reference, Backend::Vm, "amd64").await.unwrap();
            assert_eq!(got.spacesd, want, "{label}={value}");
        }
    }
    // Several present: the newest label wins.
    r.index(
        "ghcr.io/me/vm:both-disk",
        &["amd64"],
        true,
        Some(json!({SPACESD_LABEL: "false", "ai.cua.guestd": "true", "ai.cua.env-driver": "true"})),
    );
    let got = go(&r, "ghcr.io/me/vm:both-disk", Backend::Vm, "amd64")
        .await
        .unwrap();
    assert_eq!(got.spacesd, Some(false));
    r.index(
        "ghcr.io/me/vm:older-disk",
        &["amd64"],
        true,
        Some(json!({"ai.cua.guestd": "false", "ai.cua.env-driver": "true"})),
    );
    let got = go(&r, "ghcr.io/me/vm:older-disk", Backend::Vm, "amd64")
        .await
        .unwrap();
    assert_eq!(got.spacesd, Some(false));
    // An explicit `false` beats an exposed 3211 (the label is the truth).
    let cfg = r.put_blob(&json!({"os": "linux", "architecture": "arm64",
        "config": {"ExposedPorts": {"3211/tcp": {}},
                   "Labels": {SPACESD_LABEL: "false", OS_ANNOTATION: "linux"}}}));
    let m = json!({"schemaVersion": 2, "mediaType": OCI_MANIFEST,
        "config": {"mediaType": cfg.media_type, "digest": cfg.digest, "size": cfg.size},
        "layers": []});
    r.put_manifest("ghcr.io/me/app", Some("ghcr.io/me/app:1"), &m);
    let got = go(&r, "ghcr.io/me/app:1", Backend::Container, "arm64")
        .await
        .unwrap();
    assert_eq!(got.spacesd, Some(false));
    // A Lume manifest's annotations.
    let c = r.put_blob(&json!({"os": "darwin"}));
    let m = json!({"schemaVersion": 2, "mediaType": OCI_MANIFEST,
        "config": {"mediaType": crate::media_types::LUME_CONFIG, "digest": c.digest, "size": c.size},
        "layers": [{"mediaType": crate::media_types::LUME_DISK, "digest": "sha256:22", "size": 1}],
        "annotations": {SPACESD_LABEL: "false", OS_ANNOTATION: "macos"}});
    r.put_manifest("ghcr.io/me/mac", Some("ghcr.io/me/mac:26"), &m);
    let got = go(&r, "ghcr.io/me/mac:26", Backend::Lume, "arm64")
        .await
        .unwrap();
    assert_eq!((got.variant, got.os.as_str()), (Variant::Lume, "macos"));
    assert_eq!(got.spacesd, Some(false));
}

/// Live, read-only (`CUA_E2E_IMAGE=1`): the published canonical images.
#[tokio::test]
async fn live_canonical_images_resolve() {
    if std::env::var("CUA_E2E_IMAGE").as_deref() != Ok("1") {
        eprintln!("skipping: set CUA_E2E_IMAGE=1");
        return;
    }
    let c = RegistryClient::default();
    let linux =
        crate::canonical::canonical_for(crate::canonical::CanonicalOs::Linux, Some("24.04"));
    let root = resolve_with(&c, &linux, Backend::Local, "arm64")
        .await
        .unwrap();
    assert_eq!(root.variant, Variant::Rootfs, "{root:?}");
    assert_eq!(root.arch.as_deref(), Some("arm64"));
    assert!(root.pinned_ref.starts_with("ghcr.io/trycua/linux@sha256:"));
    assert_eq!(root.spacesd, Some(true), "the desktop image runs spacesd");
    assert_eq!(root.os, "linux");
    let disk = resolve_with(&c, &linux, Backend::Vm, "amd64")
        .await
        .unwrap();
    assert_eq!(disk.variant, Variant::Containerdisk, "{disk:?}");
    assert_ne!(disk.digest, root.digest);
    // The disk is annotated (`ai.cua.env-driver`, `ai.cua.image.os`), not
    // guessed: a containerDisk carries no ports.
    assert_eq!(
        (disk.spacesd, disk.os.as_str()),
        (Some(true), "linux"),
        "{disk:?}"
    );
    let disk_tag = resolve_with(&c, &format!("{linux}-disk"), Backend::Local, "arm64")
        .await
        .unwrap();
    assert_eq!(
        (disk_tag.variant, disk_tag.spacesd, disk_tag.os.as_str()),
        (Variant::Containerdisk, Some(true), "linux"),
        "{disk_tag:?}"
    );
    let fleet = resolve_with(&c, &linux, Backend::Fleet, "amd64")
        .await
        .unwrap();
    assert_eq!(
        (fleet.variant, fleet.arch.as_deref()),
        (Variant::Rootfs, Some("amd64"))
    );
    let win = resolve_with(&c, "ghcr.io/trycua/windows:2022", Backend::Local, "arm64")
        .await
        .unwrap();
    assert_eq!(
        (win.variant, win.os.as_str(), win.spacesd),
        (Variant::Containerdisk, "windows", Some(false)),
        "{win:?}"
    );
    // The OS comes from the annotation, not the repository name.
    let win_index = c
        .manifest("ghcr.io/trycua/windows:2022-disk")
        .await
        .unwrap()
        .0;
    let crate::manifest::Manifest::Index(win_index) = win_index else {
        panic!("windows:2022-disk is an index");
    };
    assert_eq!(
        win_index.annotations.get(OS_ANNOTATION).map(String::as_str),
        Some("windows")
    );
    for v in ["26", "15"] {
        let mac = resolve_with(
            &c,
            &format!("ghcr.io/trycua/macos:{v}"),
            Backend::Local,
            "arm64",
        )
        .await
        .unwrap();
        assert_eq!(
            (mac.variant, mac.os.as_str(), mac.spacesd),
            (Variant::Lume, "macos", Some(false)),
            "{mac:?}"
        );
    }
    let py = resolve_with(&c, "python:3.12-slim", Backend::Local, "arm64")
        .await
        .unwrap();
    assert_eq!(py.variant, Variant::Rootfs);
    assert!(
        py.pinned_ref
            .starts_with("docker.io/library/python@sha256:")
    );
    assert_eq!(py.spacesd, Some(false));
    // Every primary names its variants.
    for primary in [
        linux.clone(),
        format!("{linux}-disk"),
        "ghcr.io/trycua/windows:2022-disk".into(),
        "ghcr.io/trycua/macos:26".into(),
        "ghcr.io/trycua/macos:15".into(),
    ] {
        let p = probe(&c, &primary, "amd64").await.unwrap();
        assert!(
            !p.variants_annotation().is_empty(),
            "{primary} has no {VARIANTS_ANNOTATION}"
        );
    }
    eprintln!("{root:#?}\n{disk:#?}\n{win:#?}");
}
