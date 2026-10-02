//! `Sandbox::image_info`: the image a sandbox runs, as resolved and pinned
//! at create time. Fakes only (no network, no host processes, temp state
//! dirs). Its own binary: the Fleet case installs a process-wide registry
//! source.

use async_trait::async_trait;
use cua_fleet::testing::FakeFleet;
use cua_image::testing::FakeRegistry;
use cua_sandbox_core::{
    CreateOptions, ImageInfo, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime,
    LocalStartSpec, LocalSummary, ProviderKind, RuntimeError, RuntimeResult, Sandboxes,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// A local runtime that reports `resolved` as the image it pinned (or,
/// with `None`, relies on the trait's default `start_resolved`).
struct Runtime {
    resolved: Option<ImageInfo>,
    instances: Mutex<HashMap<String, InstanceStatus>>,
}

impl Runtime {
    fn new(resolved: Option<ImageInfo>) -> Arc<Self> {
        Arc::new(Self {
            resolved,
            instances: Mutex::default(),
        })
    }

    fn instance(name: &str) -> LocalInstance {
        LocalInstance {
            name: name.into(),
            backend: "container".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints {
                host: "127.0.0.1".into(),
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "container".into()
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        self.instances
            .lock()
            .unwrap()
            .insert(spec.name.clone(), InstanceStatus::Running);
        Ok(Self::instance(&spec.name))
    }

    async fn start_resolved(
        &self,
        spec: &LocalStartSpec,
    ) -> RuntimeResult<(LocalInstance, Option<ImageInfo>)> {
        let inst = self.start(spec).await?;
        Ok((inst, self.resolved.clone()))
    }

    async fn stop(&self, _name: &str) -> RuntimeResult<()> {
        Ok(())
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Ok(Self::instance(name))
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(Vec::new())
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        self.instances
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.instances.lock().unwrap().remove(name);
        Ok(())
    }

    async fn endpoints(&self, _name: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(Self::instance("x").endpoints)
    }
}

fn pinned() -> ImageInfo {
    ImageInfo {
        reference: "docker.io/library/python:3.12-slim".into(),
        pinned_ref: "docker.io/library/python@sha256:abc".into(),
        digest: "sha256:abc".into(),
        variant: "rootfs".into(),
        arch: Some("arm64".into()),
        os: "linux".into(),
        emulated: false,
        spacesd: None,
    }
}

#[tokio::test]
async fn local_sandboxes_report_the_pinned_image_and_keep_it_across_connect() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder()
        .local(Runtime::new(Some(pinned())))
        .state_dir(dir.path())
        .build();

    // Ephemeral: from the runtime, no state file.
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Local, "python:3.12-slim"))
        .await
        .unwrap();
    assert_eq!(sb.image_info(), Some(&pinned()));
    sb.delete().await.unwrap();

    // Named: persisted with the state file, so a reattach reports it too.
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Local, "python:3.12-slim").name("cua-e2e-ii"))
        .await
        .unwrap();
    assert_eq!(sb.image_info(), Some(&pinned()));
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("cua-e2e-ii.json")).unwrap())
            .unwrap();
    assert_eq!(raw["image_info"]["digest"], "sha256:abc");
    assert_eq!(raw["image_info"]["variant"], "rootfs");
    let again = sbx.connect("cua-e2e-ii").await.unwrap();
    assert_eq!(again.image_info(), Some(&pinned()));
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn local_state_records_the_os_the_image_resolved_to() {
    // The Spaces app and the SDK create a macOS Space by registry reference,
    // not by the `macos` alias, so the requested OS stays at its `linux`
    // default. The state must record the OS the image resolved to.
    let dir = tempfile::tempdir().unwrap();
    let macos = ImageInfo {
        reference: "ghcr.io/trycua/macos:26".into(),
        pinned_ref: "ghcr.io/trycua/macos@sha256:fc7d".into(),
        digest: "sha256:fc7d".into(),
        variant: "lume".into(),
        arch: None,
        os: "macos".into(),
        emulated: false,
        spacesd: Some(true),
    };
    let sbx = Sandboxes::builder()
        .local(Runtime::new(Some(macos)))
        .state_dir(dir.path())
        .build();
    let o = CreateOptions::new(ProviderKind::Local, "ghcr.io/trycua/macos:26").name("cua-e2e-os");
    assert_eq!(o.os, "linux", "a plain reference requests the default");
    let sb = sbx.create(o).await.unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("cua-e2e-os.json")).unwrap())
            .unwrap();
    assert_eq!(raw["os_type"], "macos");
    assert_eq!(raw["image"]["os_type"], "macos");
    assert_eq!(raw["image_info"]["os"], "macos");
    sb.delete().await.unwrap();

    // Nothing resolved: the requested OS is kept.
    let sbx = Sandboxes::builder()
        .local(Runtime::new(None))
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "img").name("cua-e2e-os2");
    o.os = "windows".into();
    let sb = sbx.create(o).await.unwrap();
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("cua-e2e-os2.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(raw["os_type"], "windows");
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn unresolved_local_images_and_direct_urls_report_none() {
    let dir = tempfile::tempdir().unwrap();
    // A runtime that resolved nothing (the trait default does the same).
    let sbx = Sandboxes::builder()
        .local(Runtime::new(None))
        .state_dir(dir.path())
        .build();
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Local, "img-only-in-engine").name("cua-e2e-n"))
        .await
        .unwrap();
    assert_eq!(sb.image_info(), None);
    assert_eq!(sbx.connect("cua-e2e-n").await.unwrap().image_info(), None);
    sb.delete().await.unwrap();

    // Direct: a machine by URL, nothing was resolved.
    let direct = sbx.connect_url("http://127.0.0.1:1", None).unwrap();
    assert_eq!(direct.image_info(), None);
}

#[tokio::test]
async fn fleet_managed_claims_report_the_pinned_template_image() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    // Resolve through an in-memory registry instead of the fixtures'
    // inspector (which sends references as given, unresolved).
    cua_fleet::set_image_inspector(None);
    let mut r = FakeRegistry::default();
    let root = r.index(
        "ghcr.io/trycua/linux:24.04",
        &["amd64", "arm64"],
        false,
        None,
    );
    cua_image::resolve::set_source(Some(Arc::new(r)));
    let sbx = Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir.path().join("sandboxes"))
        .build();

    let sb = sbx
        .create(
            CreateOptions::new(ProviderKind::Fleet, "ghcr.io/trycua/linux:24.04")
                .name("cua-e2e-ii-fleet"),
        )
        .await
        .unwrap();
    let want = ImageInfo {
        reference: "ghcr.io/trycua/linux:24.04".into(),
        pinned_ref: format!("ghcr.io/trycua/linux@{root}"),
        digest: root.clone(),
        variant: "rootfs".into(),
        arch: Some("amd64".into()),
        os: "linux".into(),
        emulated: false,
        // The fake registry image carries no `ai.cua.spacesd` label.
        spacesd: Some(false),
    };
    assert_eq!(sb.image_info(), Some(&want));
    // The claim's state file keeps it for a reattach.
    let claim = sb.fleet_sandbox().unwrap().claim.clone();
    let again = sbx.connect(&claim).await.unwrap();
    assert_eq!(again.image_info(), Some(&want));
    drop(again);

    // A claim on a named pool reports its template's image: here the
    // managed pool's digest-pinned one, used as is.
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    let mut o = CreateOptions::new(ProviderKind::Fleet, "");
    o.fleet.pool = Some(pool);
    let named = sbx.create(o).await.unwrap();
    let info = named.image_info().expect("named pool image info");
    assert_eq!(info.digest, root);
    assert_eq!(info.pinned_ref, format!("ghcr.io/trycua/linux@{root}"));
    assert_eq!(info.variant, "rootfs");
    named.delete().await.unwrap();
    sb.delete().await.unwrap();
    cua_image::resolve::set_source(None);
}
