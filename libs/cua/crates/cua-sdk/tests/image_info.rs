//! The exported `Sandbox::image_info` / `SandboxInfo.image_info`, embedded
//! runtime over `FakeFleet` and an in-memory registry (no network, nothing
//! started). Its own binary: it installs a process-wide registry source.

use cua_daemon::{Runtime, RuntimeConfig, fixtures};
use cua_fleet::testing::FakeFleet;
use cua_image::testing::FakeRegistry;
use cua_sdk::{CloudOptions, Cua, ImageInfo, SandboxCreateOptions};
use std::{sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fleet_managed_sandboxes_report_the_pinned_image_and_direct_ones_none() {
    let fake = FakeFleet::new();
    // The in-memory registry, not the fixtures' inspector (which sends
    // references as given, unresolved).
    cua_fleet::set_image_inspector(None);
    let mut r = FakeRegistry::default();
    let disk = r.index(
        "ghcr.io/trycua/linux:24.04-disk",
        &["amd64", "arm64"],
        true,
        None,
    );
    r.index(
        "ghcr.io/trycua/linux:24.04",
        &["amd64", "arm64"],
        false,
        None,
    );
    cua_image::resolve::set_source(Some(Arc::new(r)));
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        fleet_client: Some(fake.client()),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    runtime.mark_share_host();
    let cua = Cua::from_runtime(runtime);
    let sandboxes = cua.sandboxes();

    // A VM in the cloud (KubeVirt) picks the containerDisk sibling, pinned.
    let mut o = SandboxCreateOptions::new("cloud", "ghcr.io/trycua/linux:24.04");
    o.kind = Some("vm".into());
    let sb = sandboxes.create(o).await.unwrap();
    let want = ImageInfo {
        reference: "ghcr.io/trycua/linux:24.04".into(),
        pinned_ref: format!("ghcr.io/trycua/linux@{disk}"),
        digest: disk.clone(),
        variant: "containerdisk".into(),
        arch: Some("amd64".into()),
        os: "linux".into(),
        emulated: false,
        spacesd: None,
    };
    assert_eq!(sb.image_info(), Some(want.clone()));
    assert_eq!(sb.info().image_info, Some(want.clone()));

    // A claim on the same pool by name reports its template's (pinned)
    // image, and `Fleet.pool_image_info` answers the same.
    let pool = sb.info().provider_details["namespace"].clone();
    let mut o = SandboxCreateOptions::new("cloud", "");
    o.cloud = Some(CloudOptions {
        pool: Some(pool.clone()),
        ..Default::default()
    });
    let named = sandboxes.create(o).await.unwrap();
    let got = named.image_info().expect("named pool image info");
    assert_eq!(got.pinned_ref, want.pinned_ref);
    assert_eq!(got.digest, disk);
    assert_eq!(got.variant, "containerdisk");
    assert_eq!(named.info().image_info, Some(got.clone()));
    let direct_info = cua.fleet().unwrap().pool_image_info(pool).await.unwrap();
    assert_eq!(direct_info, Some(got));
    sandboxes.delete(named.name()).await.unwrap();
    sandboxes.delete(sb.name()).await.unwrap();
    cua_image::resolve::set_source(None);

    // Direct: a machine by URL; nothing was resolved.
    let env = fixtures::start_env(None, None).await;
    let direct = sandboxes
        .connect_url(env.url.clone(), None, None)
        .await
        .unwrap();
    assert_eq!(direct.image_info(), None);
    assert_eq!(direct.info().image_info, None);
}
