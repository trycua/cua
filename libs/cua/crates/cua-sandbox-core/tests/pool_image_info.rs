//! `Sandbox::image_info` for claims on a named (user-owned) Fleet pool:
//! the template's image, pinned by the resolver, cached per pool, never
//! failing the claim. Fakes only (FakeFleet, an in-memory registry, temp
//! state dirs). Its own binary: it swaps the process-wide image inspector
//! and registry source, so the cases run in one test, in order.

use cua_fleet::{ImageVariant, PoolOptions, SandboxSpec, testing::FakeFleet};
use cua_image::testing::FakeRegistry;
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};
use std::sync::Arc;

const TAGGED: &str = "ghcr.io/trycua/linux:24.04";
const PRIVATE: &str = "registry.example.com/team/private:1";

fn pinned() -> String {
    format!("ghcr.io/trycua/linux@sha256:{}", "b".repeat(64))
}

fn claim_opts(pool: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Fleet, "");
    o.fleet.pool = Some(pool.into());
    o
}

#[tokio::test]
async fn named_pool_claims_report_the_template_image() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let fleet = fake.client();
    // The pools keep their template references as given (the fixtures'
    // inspector answers with the variant, without pinning).
    for image in [pinned(), TAGGED.to_string(), PRIVATE.to_string()] {
        cua_fleet::testing::set_image_variant(&image, ImageVariant::Rootfs);
    }
    for (pool, image) in [
        ("cua-e2e-pinned", pinned()),
        ("cua-e2e-tagged", TAGGED.to_string()),
        ("cua-e2e-private", PRIVATE.to_string()),
    ] {
        fleet
            .apply(pool, &SandboxSpec::new(&image), &PoolOptions::default())
            .await
            .unwrap();
    }
    // Claims resolve through an in-memory registry: it knows only the
    // tagged image and refuses the private one.
    cua_fleet::set_image_inspector(None);
    let mut r = FakeRegistry::default();
    let root = r.index(TAGGED, &["amd64", "arm64"], false, None);
    r.refuse.insert(PRIVATE.into(), "unauthorized");
    cua_image::resolve::set_source(Some(Arc::new(r)));
    let sbx = Sandboxes::builder()
        .fleet(fleet.clone())
        .state_dir(dir.path().join("sandboxes"))
        .build();

    // Digest-pinned template: used as is (the registry has no such
    // manifest, so a read would have failed).
    let sb = sbx.create(claim_opts("cua-e2e-pinned")).await.unwrap();
    let info = sb.image_info().expect("pinned template info").clone();
    assert_eq!(info.pinned_ref, pinned());
    assert_eq!(info.digest, format!("sha256:{}", "b".repeat(64)));
    assert_eq!(info.variant, "rootfs");
    assert_eq!(info.arch.as_deref(), Some("amd64"));
    assert_eq!(info.os, "linux");
    sb.delete().await.unwrap();

    // Tagged template: resolved to the index digest for gVisor (rootfs).
    let sb = sbx
        .create(claim_opts("cua-e2e-tagged").name("cua-e2e-named-claim"))
        .await
        .unwrap();
    let info = sb.image_info().expect("tagged template info").clone();
    assert_eq!(info.reference, TAGGED);
    assert_eq!(info.pinned_ref, format!("ghcr.io/trycua/linux@{root}"));
    assert_eq!(info.digest, root);
    assert_eq!(info.variant, "rootfs");
    assert_eq!(info.arch.as_deref(), Some("amd64"));
    // A reattach by name keeps it (state file).
    let claim = sb.fleet_sandbox().unwrap().claim.clone();
    let again = sbx.connect(&claim).await.unwrap();
    assert_eq!(again.image_info(), Some(&info));
    drop(again);
    sb.delete().await.unwrap();

    // Cached per pool: an empty registry does not change the answer.
    cua_image::resolve::set_source(Some(Arc::new(FakeRegistry::default())));
    let sb = sbx.create(claim_opts("cua-e2e-tagged")).await.unwrap();
    assert_eq!(sb.image_info(), Some(&info));
    sb.delete().await.unwrap();

    // Unresolvable (private, no credentials): the claim still succeeds and
    // records the template reference, without a digest.
    let sb = sbx.create(claim_opts("cua-e2e-private")).await.unwrap();
    let info = sb.image_info().expect("private template info");
    assert_eq!(info.reference, PRIVATE);
    assert_eq!(info.pinned_ref, "");
    assert_eq!(info.digest, "");
    assert_eq!(info.variant, "rootfs");
    sb.delete().await.unwrap();

    // Not cached: once the registry answers, the digest is recorded.
    let mut r = FakeRegistry::default();
    let private_root = r.index(PRIVATE, &["amd64"], false, None);
    cua_image::resolve::set_source(Some(Arc::new(r)));
    let sb = sbx.create(claim_opts("cua-e2e-private")).await.unwrap();
    assert_eq!(sb.image_info().unwrap().digest, private_root);
    sb.delete().await.unwrap();

    cua_image::resolve::set_source(None);
}
