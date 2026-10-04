//! Sandbox parity over a fake local runtime: sidecars, registry
//! credentials, image builds and the guest network. Nothing here starts a
//! container or reaches a registry (every test uses a temp state dir).

use async_trait::async_trait;
use cua_sandbox_core::{
    BuildSpec, CreateOptions, Error, ImageLayer, InstanceStatus, LocalEndpoints, LocalInstance,
    LocalRuntime, LocalStartSpec, LocalSummary, NetworkMode, ProviderKind, RegistryCredentials,
    RuntimeResult, Sandboxes, Sidecar,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorder {
    specs: Mutex<Vec<LocalStartSpec>>,
    builds: Mutex<Vec<BuildSpec>>,
}

#[async_trait]
impl LocalRuntime for Recorder {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        self.specs.lock().unwrap().push(spec.clone());
        // Like the container backend on an engine with gVisor.
        if !spec.sidecars.is_empty() && spec.container_runtime.as_deref() != Some("runc") {
            return Err(cua_sandbox_core::RuntimeError::Unsupported {
                backend: "container".into(),
                op: "sidecars share a network namespace, which gVisor can't do under Docker. \
                     Pass runtime='runc' to run this group on runc"
                    .into(),
            });
        }
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints {
                host: "127.0.0.1".into(),
                ..Default::default()
            },
        })
    }
    async fn stop(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Err(cua_sandbox_core::RuntimeError::NotFound(name.into()))
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, _: &str) -> RuntimeResult<InstanceStatus> {
        Ok(InstanceStatus::Running)
    }
    async fn delete(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(LocalEndpoints::default())
    }
    async fn build_image(
        &self,
        spec: &BuildSpec,
        _: Option<&RegistryCredentials>,
    ) -> RuntimeResult<String> {
        self.builds.lock().unwrap().push(spec.clone());
        if spec.from.starts_with("vm:") {
            return Err(cua_sandbox_core::RuntimeError::UnsupportedImage(
                "image layers build on container images".into(),
            ));
        }
        Ok("container:cua-vmm/build:cua-b-0123456789abcdef01234567".into())
    }
}

fn redis() -> Sidecar {
    Sidecar {
        ports: vec![6379],
        ..Sidecar::new("redis:7-alpine")
    }
}

const ROOTFS: &str = "docker.io/library/python:3.12-slim";

#[tokio::test]
async fn local_start_carries_sidecars_credentials_and_sidecar_services() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Recorder::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, "localhost:5000/private/app:1")
        .sidecar(redis())
        .service("db", 6379);
    o.registry_credentials = Some(RegistryCredentials::new("u", "p"));
    o.container_runtime = Some("runc".into());
    let sb = sbx.create(o).await.unwrap();
    let spec = rt.specs.lock().unwrap()[0].clone();
    assert_eq!(spec.sidecars, vec![redis()]);
    assert_eq!(spec.container_runtime.as_deref(), Some("runc"));
    let creds = spec
        .registry_credentials
        .expect("credentials reach the runtime");
    assert_eq!(
        creds.registry.as_deref(),
        Some("localhost:5000"),
        "scoped to the image's registry"
    );
    assert!(
        spec.ports.contains(&6379),
        "the sidecar port is published: {:?}",
        spec.ports
    );
    assert_eq!(sb.services().get("db"), Some(&6379));
    // The credentials never reach the state file.
    let state =
        std::fs::read_to_string(dir.path().join(format!("{}.json", sb.name()))).unwrap_or_default();
    assert!(!state.contains("\"p\""), "{state}");
    assert!(serde_json::to_string(&spec.sidecars).is_ok());
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn local_sidecars_without_runc_are_a_typed_unsupported_error() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder()
        .local(Arc::new(Recorder::default()))
        .state_dir(dir.path())
        .build();
    let err = sbx
        .create(CreateOptions::new(ProviderKind::Local, ROOTFS).sidecar(redis()))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Unsupported {
                provider: ProviderKind::Local,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("runtime='runc'"), "{err}");
}

#[tokio::test]
async fn local_image_layers_build_locally_then_run_the_built_image() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Recorder::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let layers = || BuildSpec {
        layers: vec![ImageLayer::PipInstall {
            packages: vec!["mcp".into()],
        }],
        ..Default::default()
    };
    let mut o = CreateOptions::new(ProviderKind::Local, ROOTFS);
    o.build = Some(layers());
    let sb = sbx.create(o).await.unwrap();
    // The build is on the sandbox's image; the sandbox runs its output.
    assert_eq!(rt.builds.lock().unwrap()[0].from, ROOTFS);
    assert_eq!(
        rt.specs.lock().unwrap()[0].image,
        "container:cua-vmm/build:cua-b-0123456789abcdef01234567"
    );
    sb.delete().await.unwrap();

    // A VM image takes no container layers: a typed error, nothing started.
    let mut o = CreateOptions::new(ProviderKind::Local, "vm:ghcr.io/trycua/linux:24.04-disk");
    o.build = Some(layers());
    let err = sbx.create(o).await.unwrap_err();
    assert!(matches!(err, Error::UnsupportedImage(_)), "{err:?}");
    assert_eq!(rt.specs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn reserved_names_and_bad_sidecars_fail_before_anything_starts() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Recorder::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    {
        let provider = ProviderKind::Local;
        for reserved in cua_sandbox_core::sidecar::RESERVED_SERVICE_NAMES {
            let mut o = CreateOptions::new(provider, ROOTFS)
                .sidecar(redis())
                .service(reserved, 8080);
            o.container_runtime = Some("runc".into());
            let err = sbx.create(o).await.unwrap_err().to_string();
            assert!(err.contains("reserved"), "{provider:?} {reserved}: {err}");
        }
        let main = Sidecar {
            name: "main".into(),
            ..redis()
        };
        let mut o = CreateOptions::new(provider, ROOTFS).sidecar(main);
        o.container_runtime = Some("runc".into());
        let err = sbx.create(o).await.unwrap_err().to_string();
        assert!(err.contains("main"), "{provider:?}: {err}");
        // Without sidecars the names are free (locally at least; nothing
        // else about this sandbox is refused).
        let free = CreateOptions::new(ProviderKind::Local, ROOTFS).service("main", 8080);
        sbx.create(free).await.unwrap().delete().await.unwrap();
    }
    assert_eq!(
        rt.specs.lock().unwrap().len(),
        1,
        "only the free sandbox started"
    );
}

#[tokio::test]
async fn network_none_reaches_the_local_runtime() {
    assert_eq!(NetworkMode::parse("").unwrap(), NetworkMode::Default);
    assert_eq!(NetworkMode::parse("Default").unwrap(), NetworkMode::Default);
    assert_eq!(NetworkMode::parse(" none ").unwrap(), NetworkMode::None);
    assert!(matches!(
        NetworkMode::parse("host"),
        Err(Error::InvalidArgument(_))
    ));

    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Recorder::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    // Default: outbound network.
    sbx.create(CreateOptions::new(ProviderKind::Local, "vm:img"))
        .await
        .unwrap();
    let mut o = CreateOptions::new(ProviderKind::Local, "vm:img");
    o.network = NetworkMode::None;
    sbx.create(o).await.unwrap();
    let specs = rt.specs.lock().unwrap().clone();
    assert!(!specs[0].restrict_network, "egress is on by default");
    assert!(
        specs[1].restrict_network,
        "network=none restricts the guest"
    );
}
