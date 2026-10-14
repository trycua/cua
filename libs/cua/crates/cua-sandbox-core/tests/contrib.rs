//! Contrib providers through the core: image resolution and capability
//! checks before any provider call, refs (`e2b:<name>`), state files,
//! reattach, services, spacesd over the provider's port URL, and delete.
//! Hermetic: an in-memory provider, an in-memory registry and a mock
//! cua-spacesd on loopback. Its own binary: it installs a process-wide
//! registry source.

use async_trait::async_trait;
use cua_image::testing::FakeRegistry;
use cua_sandbox_core::{
    CreateOptions, Error, ImageMode, PortExposure, Provider, ProviderCapabilities, ProviderCreate,
    ProviderInstance, ProviderKind, Result, RunKind, SandboxRef, Sandboxes, ServiceEndpoint,
    Status,
};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// An in-memory provider named `e2b` whose every port is the mock spacesd.
struct Fake {
    spacesd: String,
    env_to_entrypoint: bool,
    created: Mutex<Vec<ProviderCreate>>,
    live: Mutex<BTreeMap<String, ProviderInstance>>,
    /// `create` makes the sandbox, then never returns (the platform is
    /// still booting it when a create is cancelled).
    hang: std::sync::atomic::AtomicBool,
}

impl Fake {
    fn new(spacesd: String, env_to_entrypoint: bool) -> Arc<Self> {
        Arc::new(Self {
            spacesd,
            env_to_entrypoint,
            created: Mutex::default(),
            live: Mutex::default(),
            hang: Default::default(),
        })
    }
}

#[async_trait]
impl Provider for Fake {
    fn name(&self) -> &'static str {
        "e2b"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            kinds: vec![RunKind::Container],
            runtime: "firecracker",
            arches: vec!["amd64"],
            image_mode: ImageMode::Template,
            ports: PortExposure::Https,
            command: true,
            env_to_entrypoint: self.env_to_entrypoint,
            private_registry: false,
            suspend: false,
            max_cpus: Some(8),
            max_memory_mb: None,
            credential_env: &["E2B_API_KEY"],
            gpus: vec![cua_sandbox_core::gpu::GpuOption {
                id: "T4".into(),
                label: "NVIDIA T4".into(),
                supported: true,
                ..Default::default()
            }],
        }
    }

    fn check_configured(&self) -> Result<()> {
        Ok(())
    }

    async fn create(&self, spec: &ProviderCreate) -> Result<ProviderInstance> {
        self.created.lock().unwrap().push(spec.clone());
        let id = format!("sbx{}", self.created.lock().unwrap().len());
        let mut i = ProviderInstance::new(&id, &spec.name, Status::Running);
        i.details.insert("template".into(), "cua-test".into());
        i.details.insert("_secret".into(), "never shown".into());
        self.live.lock().unwrap().insert(id, i.clone());
        if self.hang.load(std::sync::atomic::Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(i)
    }

    async fn get(&self, id: &str) -> Result<ProviderInstance> {
        self.live
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(id.into()))
    }

    async fn list(&self) -> Result<Vec<ProviderInstance>> {
        Ok(self.live.lock().unwrap().values().cloned().collect())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.live.lock().unwrap().remove(id);
        Ok(())
    }

    fn endpoint(&self, _: &ProviderInstance, port: u16) -> Result<ServiceEndpoint> {
        Ok(ServiceEndpoint {
            url: if port == 3211 {
                self.spacesd.clone()
            } else {
                format!("http://127.0.0.1:9/{port}")
            },
            headers: vec![],
        })
    }
}

/// A registry with a desktop-like rootfs (cua-spacesd label), a VM-only
/// image and an arm64-only image.
fn registry() -> FakeRegistry {
    let mut r = FakeRegistry::default();
    let cfg = r.put_blob(&json!({"os": "linux", "architecture": "amd64",
        "config": {"Entrypoint": ["/start"], "Labels": {"ai.cua.spacesd": "true"}}}));
    let m = json!({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": cfg.media_type, "digest": cfg.digest, "size": cfg.size},
        "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": "sha256:00", "size": 1}]});
    r.put_manifest(
        "ghcr.io/trycua/linux",
        Some("ghcr.io/trycua/linux:24.04"),
        &m,
    );
    r.index("ghcr.io/example/vm-only:1", &["amd64"], true, None);
    r.index("ghcr.io/example/arm-only:1", &["arm64"], false, None);
    r
}

struct Env {
    _dir: tempfile::TempDir,
    _mock: MockServer,
    fake: Arc<Fake>,
    sbx: Sandboxes,
}

async fn env(env_to_entrypoint: bool) -> Env {
    cua_image::resolve::set_source(Some(Arc::new(registry())));
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start(MockAuth::default()).await;
    let fake = Fake::new(mock.url(), env_to_entrypoint);
    let sbx = Sandboxes::builder()
        .provider(fake.clone())
        .state_dir(dir.path().join("sandboxes"))
        .build();
    Env {
        _dir: dir,
        _mock: mock,
        fake,
        sbx,
    }
}

fn opts(image: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Contrib, image);
    o.contrib = Some("e2b".into());
    o
}

#[tokio::test]
async fn named_sandbox_lifecycle_refs_state_and_spacesd() {
    let e = env(false).await;
    let sb = e
        .sbx
        .create(opts("ghcr.io/trycua/linux:24.04").name("cua-e2e-contrib-a"))
        .await
        .unwrap();
    assert_eq!(sb.provider(), ProviderKind::Contrib);
    assert_eq!(sb.runtime_type(), "e2b");
    assert_eq!(sb.id(), "e2b:cua-e2e-contrib-a");
    assert_eq!(sb.location(), "e2b");
    // The provider got the pinned image, the spacesd port and labels.
    let spec = e.fake.created.lock().unwrap()[0].clone();
    assert!(
        spec.image
            .pinned_ref
            .starts_with("ghcr.io/trycua/linux@sha256:")
    );
    assert_eq!(spec.image.kind, RunKind::Container);
    assert_eq!(spec.image.spacesd, Some(true));
    assert!(spec.ports.contains(&3211));
    assert_eq!(spec.labels["cua.name"], "cua-e2e-contrib-a");
    // env_to_entrypoint = false: no token in the guest environment (the
    // SDK installs one with Init instead).
    assert!(!spec.env.contains_key("CUA_ENV_TOKEN"));
    // Private details never show.
    let details = sb.provider_details();
    assert_eq!(details["provider"], "e2b");
    assert_eq!(details["template"], "cua-test");
    assert!(!details.contains_key("_secret"));
    // Readiness included cua-spacesd (the image declares it); attach works
    // over the provider's port URL.
    let client = sb.spacesd().await.unwrap();
    client.health().await.unwrap();
    assert_eq!(
        sb.service("env").unwrap().url(),
        e._mock.url().trim_end_matches('/')
    );

    // Refs and state: reattach by name and by `e2b:<name>`.
    let r = SandboxRef::parse("e2b:cua-e2e-contrib-a").unwrap();
    assert_eq!(r.location_word(), Some("e2b"));
    let again = e.sbx.connect_ref(&r).await.unwrap();
    assert_eq!(again.id(), "e2b:cua-e2e-contrib-a");
    let bare = SandboxRef::parse("cua-e2e-contrib-a").unwrap();
    assert_eq!(e.sbx.resolve_ref(&bare).await.unwrap(), r);
    let listed = e.sbx.list().await.unwrap();
    let row = listed
        .iter()
        .find(|i| i.name == "cua-e2e-contrib-a")
        .unwrap();
    assert_eq!(row.provider, ProviderKind::Contrib);
    assert_eq!(row.id, "e2b:cua-e2e-contrib-a");
    assert_eq!(
        e.sbx.get("cua-e2e-contrib-a").await.unwrap().status,
        Status::Running
    );

    // Suspend is refused with a typed error naming the provider.
    let err = sb.suspend().await.unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err}");

    e.sbx.delete("cua-e2e-contrib-a").await.unwrap();
    assert!(e.fake.live.lock().unwrap().is_empty());
    assert!(e.sbx.connect("cua-e2e-contrib-a").await.is_err());
}

#[tokio::test]
async fn token_is_delivered_when_the_entrypoint_sees_the_environment() {
    let e = env(true).await;
    let sb = e
        .sbx
        .create(opts("ghcr.io/trycua/linux:24.04"))
        .await
        .unwrap();
    assert!(sb.is_ephemeral());
    let spec = e.fake.created.lock().unwrap()[0].clone();
    assert_eq!(spec.env.get("CUA_ENV_TOKEN").map(String::len), Some(32));
    assert!(spec.name.starts_with("cua-eph-"));
    sb.delete().await.unwrap();
    assert!(e.fake.live.lock().unwrap().is_empty());
}

#[tokio::test]
async fn impossible_requests_fail_typed_before_any_provider_call() {
    let e = env(false).await;
    let cases: Vec<(CreateOptions, &str)> = vec![
        (opts("ghcr.io/example/vm-only:1"), "no variant e2b can run"),
        (
            opts("vm:ghcr.io/trycua/linux:24.04"),
            "does not run vm sandboxes",
        ),
        (opts("ghcr.io/example/arm-only:1"), "no amd64 build"),
        (
            {
                let mut o = opts("ghcr.io/trycua/linux:24.04");
                o.os = "windows".into();
                o
            },
            "Linux sandboxes only",
        ),
        (
            {
                let mut o = opts("ghcr.io/trycua/linux:24.04");
                o.cpus = 64;
                o
            },
            "at most 8 vCPUs",
        ),
        (
            {
                let mut o = opts("ghcr.io/trycua/linux:24.04");
                o.network = cua_sandbox_core::NetworkMode::None;
                o
            },
            "network",
        ),
        (
            {
                let mut o = opts("ghcr.io/trycua/linux:24.04");
                o.contrib = Some("daytona".into());
                o
            },
            "this build has no daytona provider",
        ),
    ];
    for (o, want) in cases {
        let err = e.sbx.create(o).await.unwrap_err();
        assert!(
            matches!(
                err,
                Error::UnsupportedImage(_) | Error::Unsupported { .. } | Error::InvalidArgument(_)
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains(want), "{err} (wanted {want:?})");
    }
    assert!(e.fake.created.lock().unwrap().is_empty());
}

#[test]
fn contrib_refs_parse_and_print() {
    for s in ["e2b:box", "daytona:box", "modal:box"] {
        assert_eq!(SandboxRef::parse(s).unwrap().to_string(), s);
    }
    assert!(SandboxRef::parse("e2b:").is_err());
    assert!(SandboxRef::parse("nosuch:box").is_err());
}

/// A cancelled create stops the provider's create and deletes the sandbox
/// it made; a sandbox of that name made before is never touched.
#[tokio::test]
async fn a_cancelled_create_deletes_what_it_made_and_nothing_else() {
    let e = env(false).await;
    e.fake.hang.store(true, std::sync::atomic::Ordering::SeqCst);
    let cancel = cua_sandbox_core::CancellationToken::new();
    let create = e.sbx.create_cancellable(
        opts("ghcr.io/trycua/linux:24.04").name("cua-cancel-a"),
        cancel.clone(),
    );
    let fire = async {
        // Once the provider has made it.
        while e.fake.live.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        cancel.cancel();
    };
    let (r, ()) = tokio::join!(create, fire);
    match r {
        Err(Error::Cancelled(what)) => assert_eq!(what, "removed cua-cancel-a"),
        other => panic!("{:?}", other.map(|s| s.id())),
    }
    assert!(
        e.fake.live.lock().unwrap().is_empty(),
        "the sandbox is gone"
    );
    // Twice is harmless: the token is spent, a new create runs anew.
    e.fake
        .hang
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let kept = e
        .sbx
        .create(opts("ghcr.io/trycua/linux:24.04").name("cua-cancel-b"))
        .await
        .unwrap();
    // A create under a name in use, cancelled: the existing one stays.
    e.fake.hang.store(true, std::sync::atomic::Ordering::SeqCst);
    let cancel = cua_sandbox_core::CancellationToken::new();
    cancel.cancel();
    let r = e
        .sbx
        .create_cancellable(
            opts("ghcr.io/trycua/linux:24.04").name("cua-cancel-b"),
            cancel,
        )
        .await;
    match r {
        Err(Error::Cancelled(what)) => assert!(what.contains("existed before"), "{what}"),
        other => panic!("{:?}", other.map(|s| s.id())),
    }
    assert!(
        e.fake
            .live
            .lock()
            .unwrap()
            .values()
            .any(|i| i.name == "cua-cancel-b"),
        "the earlier sandbox stays"
    );
    drop(kept);
}

/// Dropping the create (a closed request) cleans up the same way.
#[tokio::test]
async fn a_dropped_create_deletes_what_it_made() {
    let e = env(false).await;
    e.fake.hang.store(true, std::sync::atomic::Ordering::SeqCst);
    let create = e.sbx.create_cancellable(
        opts("ghcr.io/trycua/linux:24.04"),
        cua_sandbox_core::CancellationToken::new(),
    );
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), create).await;
    assert_eq!(e.fake.live.lock().unwrap().len(), 1, "made before the drop");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !e.fake.live.lock().unwrap().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "never cleaned up");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A GPU type reaches the provider; one it does not offer is refused
/// before any call.
#[tokio::test]
async fn a_gpu_type_reaches_the_provider() {
    let e = env(false).await;
    let mut o = opts("ghcr.io/trycua/linux:24.04").name("cua-gpu-a");
    o.gpu = Some("auto".into());
    e.sbx.create(o).await.unwrap();
    assert_eq!(e.fake.created.lock().unwrap()[0].gpu.as_deref(), Some("T4"));
    let mut o = opts("ghcr.io/trycua/linux:24.04").name("cua-gpu-b");
    o.gpu = Some("H100".into());
    match e.sbx.create(o).await {
        Err(Error::InvalidArgument(m)) => assert!(m.contains("offers T4"), "{m}"),
        other => panic!("{:?}", other.map(|s| s.id())),
    }
    assert_eq!(
        e.fake.created.lock().unwrap().len(),
        1,
        "refused before a call"
    );
}
