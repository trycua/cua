//! P2 sandbox parity over fake providers: sidecars, registry credentials,
//! remote image builds and warm capacity for canonical images. Nothing here
//! starts a container or reaches a registry (the fake Fleet installs an
//! offline image inspector; every test uses a temp state dir).
//!
//! Cloud remote builds need `cua-fleet/fleet-remote-builds` (Fleet's
//! builder does not run them yet); without it they check the "not deployed
//! yet" error instead.

use async_trait::async_trait;
use cua_fleet::{REMOTE_BUILDS_SUPPORTED, testing::FakeFleet};
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

fn fleet(fake: &FakeFleet, dir: &std::path::Path) -> Sandboxes {
    cua_fleet::testing::set_image_variant(ROOTFS, cua_fleet::ImageVariant::Rootfs);
    Sandboxes::builder()
        .fleet(fake.client())
        .state_dir(dir)
        .build()
}

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
async fn cloud_vm_images_carry_sidecars_addressed_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let disk = "ghcr.io/trycua/linux:24.04-disk";
    cua_fleet::testing::set_image_variant(disk, cua_fleet::ImageVariant::ContainerDisk);
    let sbx = fleet(&fake, dir.path());
    let db = Sidecar {
        name: "db".into(),
        ..redis()
    };
    let sb = sbx
        .create(
            CreateOptions::new(ProviderKind::Fleet, disk)
                .sidecar(db)
                .service("db", 6379),
        )
        .await
        .unwrap();
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    let vm = fake.object("template", &pool, &pool).unwrap()["spec"]["vmTemplate"].clone();
    assert_eq!(vm["runtime"], "kubevirt");
    assert_eq!(vm["sidecars"][0]["name"], "db");
    assert_eq!(vm["sidecars"][0]["ports"], serde_json::json!([6379]));
    assert_eq!(sb.services().get("db"), Some(&6379));
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn reserved_names_and_bad_sidecars_fail_before_anything_starts() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let rt = Arc::new(Recorder::default());
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .fleet(fake.client())
        .state_dir(dir.path())
        .build();
    cua_fleet::testing::set_image_variant(ROOTFS, cua_fleet::ImageVariant::Rootfs);
    for provider in [ProviderKind::Local, ProviderKind::Fleet] {
        for reserved in cua_fleet::RESERVED_SERVICE_NAMES {
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
    assert!(fake.all_namespaces().is_empty(), "nothing was created");
    assert_eq!(
        rt.specs.lock().unwrap().len(),
        2,
        "only the free sandboxes started"
    );
}

#[tokio::test]
async fn cloud_sidecars_and_private_images() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let sbx = fleet(&fake, dir.path());
    let private = "ghcr.io/me/private:1";
    cua_fleet::testing::set_image_variant(private, cua_fleet::ImageVariant::Rootfs);
    let mut o = CreateOptions::new(ProviderKind::Fleet, private)
        .sidecar(redis())
        .service("db", 6379);
    o.registry_credentials = Some(RegistryCredentials::new("me", "tok"));
    let sb = sbx.create(o).await.unwrap();
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    let t = fake.object("template", &pool, &pool).unwrap();
    let vm = &t["spec"]["vmTemplate"];
    assert_eq!(vm["sidecars"][0]["image"], "redis:7-alpine");
    let secret = cua_fleet::registry_secret_name("ghcr.io", "me");
    assert_eq!(vm["imagePullSecret"], secret.as_str());
    assert!(fake.exists("secret", &pool, &secret));
    let services: Vec<_> = vm["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().to_string(),
                s["targetPort"].as_u64().unwrap(),
            )
        })
        .collect();
    assert!(services.contains(&("db".into(), 6379)), "{services:?}");
    assert_eq!(sb.services().get("db"), Some(&6379));
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn canonical_images_start_warm_by_default_and_others_do_not() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let sbx = fleet(&fake, dir.path());
    let canonical = "ghcr.io/trycua/linux:24.04";
    cua_fleet::testing::set_image_variant(canonical, cua_fleet::ImageVariant::Rootfs);
    let initial = |sb: &cua_sandbox_core::Sandbox| {
        let pool = sb.fleet_sandbox().unwrap().namespace.clone();
        fake.object("pool", &pool, &pool).unwrap()["spec"]["autoscaling"]["initialPoolSize"]
            .as_u64()
    };
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Fleet, canonical))
        .await
        .unwrap();
    assert_eq!(initial(&sb), Some(1), "canonical: warm by default");
    sb.delete().await.unwrap();
    let sb = sbx
        .create(CreateOptions::new(ProviderKind::Fleet, ROOTFS))
        .await
        .unwrap();
    assert_eq!(initial(&sb), Some(0), "other images stay cold");
    sb.delete().await.unwrap();
    // An explicit choice wins either way (a different pool shape, so a new
    // pool whose creation shows it).
    let mut o = CreateOptions::new(ProviderKind::Fleet, canonical);
    o.fleet.warm = Some(false);
    o.cpus = 3;
    let sb = sbx.create(o).await.unwrap();
    assert_eq!(initial(&sb), Some(0));
    sb.delete().await.unwrap();
    assert_eq!(
        CreateOptions::new(ProviderKind::Fleet, "linux").default_warm(),
        Some(true),
        "the alias is canonical too"
    );
    assert_eq!(
        CreateOptions::new(ProviderKind::Fleet, ROOTFS).default_warm(),
        None
    );
}

#[tokio::test]
async fn cloud_image_layers_build_remotely_then_run_the_built_image() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let sbx = fleet(&fake, dir.path());
    let mut o = CreateOptions::new(ProviderKind::Fleet, ROOTFS);
    o.fleet.runtime = Some(cua_fleet::RuntimeKind::Gvisor);
    o.build = Some(BuildSpec {
        layers: vec![ImageLayer::PipInstall {
            packages: vec!["mcp".into()],
        }],
        ..Default::default()
    });
    let result = sbx.create(o.clone()).await;
    if !REMOTE_BUILDS_SUPPORTED {
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not deployed yet"), "{err}");
        return;
    }
    let sb = result.unwrap();
    let pool = sb.fleet_sandbox().unwrap().namespace.clone();
    let t = fake.object("template", &pool, &pool).unwrap();
    let image = t["spec"]["vmTemplate"]["containerDiskImage"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(image.starts_with("registry.fleet.test/builds/"), "{image}");
    assert!(image.contains("@sha256:"), "{image}");
    sb.delete().await.unwrap();
    // The same layers again: no second build.
    let builds = || {
        fake.requests()
            .iter()
            .filter(|r| r.method == "POST" && r.path.ends_with("/images"))
            .count()
    };
    assert_eq!(builds(), 1);
    let sb = sbx.create(o).await.unwrap();
    assert_eq!(builds(), 1);
    sb.delete().await.unwrap();
}

#[tokio::test]
async fn network_none_reaches_the_local_runtime_and_the_cloud_refuses_it() {
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

    let fake = FakeFleet::new();
    let dir = tempfile::tempdir().unwrap();
    let sbx = fleet(&fake, dir.path());
    let mut o = CreateOptions::new(ProviderKind::Fleet, "ghcr.io/trycua/linux:latest");
    o.network = NetworkMode::None;
    let err = sbx.create(o).await.unwrap_err();
    assert!(
        matches!(&err, Error::Unsupported { op, .. } if op.contains("network=\"none\"")),
        "{err}"
    );
}
