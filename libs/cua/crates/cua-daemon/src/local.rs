//! Local runtimes and images: a [`cua_sandbox_core::LocalRuntime`] adapter
//! over `cua-vmm` (container/gVisor, QEMU, Lume) plus the image operations
//! of `cua-image`.
//!
//! Backend choice for `Sandboxes.create(provider = local)` comes from the
//! image string:
//!
//! | image | backend |
//! |---|---|
//! | `container:<ref>` / `docker:<ref>` or a bare OCI ref | container (gVisor `runsc` when available) |
//! | `vm:<ref>` (containerDisk) / `disk:<path>` / a `.qcow2`/`.img`/`.raw`/`.iso` path | QEMU |
//! | `lume:<ref>`, or any image with `os = macos` | Lume |
//!
//! A bare, `vm:` or `container:` OCI ref is resolved first by the one
//! resolver ([`cua_image::resolve`]: the variant the backend runs, pinned by
//! digest), so any Fleet or canonical image ref runs on the right backend:
//! a KubeVirt containerDisk boots under QEMU for the architecture the image
//! offers (the host's when it has one), a Lume image under Lume, a rootfs as
//! a container, and an image no local backend runs fails with
//! [`RuntimeError::UnsupportedImage`]. `fleet:<pool-or-template>` images are
//! resolved one layer up, in `cua_sandbox_core::fleet_local`.
//!
//! Operations on an existing instance find its backend by asking each one
//! (container, QEMU, Lume) for the instance's status.

use crate::{Error, Result};
use async_trait::async_trait;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError, RuntimeResult,
};
use cua_vmm::{
    BackendKind, Endpoints, GuestOs, ImageSource, Runtime as VmmRuntime, StartSpec, Status,
    VmmError,
    auto::{self, DoctorReport},
    container::{ContainerConfig, ContainerRuntime},
    lume::LumeRuntime,
    qemu::{QemuConfig, QemuRuntime},
};
use std::{path::Path, sync::Arc};

fn rt_err(e: VmmError) -> RuntimeError {
    match e {
        VmmError::NotFound(n) => RuntimeError::NotFound(n),
        VmmError::UnsupportedImage { reason, .. } => RuntimeError::UnsupportedImage(reason),
        VmmError::InsufficientDisk(d) => RuntimeError::InsufficientDisk(d.to_string()),
        VmmError::Unsupported { backend, op } => RuntimeError::Unsupported {
            backend: backend.to_string(),
            op: op.to_string(),
        },
        other => RuntimeError::Other(other.to_string()),
    }
}

/// Maps a `cua-vmm` error into the runtime error.
pub fn vmm_error(e: VmmError) -> Error {
    match e {
        VmmError::NotFound(n) => Error::NotFound(n),
        VmmError::Unsupported { backend, op } => {
            Error::Unsupported(format!("{backend} does not support {op}"))
        }
        VmmError::Invalid(m) => Error::InvalidArgument(m),
        VmmError::UnsupportedImage { reason, .. } => Error::Unsupported(reason),
        VmmError::Missing { what, hint } => {
            Error::ProviderNotConfigured(format!("{what} is not available: {hint}"))
        }
        VmmError::InsufficientDisk(d) => Error::InsufficientDisk(d.to_string()),
        other => Error::Runtime(other.to_string()),
    }
}

fn image_error(e: cua_image::ImageError) -> Error {
    match e {
        cua_image::ImageError::Vmm(v) => vmm_error(v),
        other => Error::Runtime(other.to_string()),
    }
}

/// Where an image string runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Backend.
    pub backend: BackendKind,
    /// Image for that backend.
    pub image: ImageSource,
}

/// Decides the backend for `image` / `os` (see the module docs).
pub fn place(image: &str, os: &str) -> Result<Placement> {
    let image = image.trim();
    if image.is_empty() {
        return Err(Error::InvalidArgument("image is required".into()));
    }
    let (prefix, rest) = match image.split_once(':') {
        Some((p, r))
            if ["container", "docker", "vm", "disk", "lume"].contains(&p)
                && !r.starts_with("//") =>
        {
            (Some(p), r)
        }
        _ => (None, image),
    };
    let disk_like = |s: &str| {
        let lower = s.to_ascii_lowercase();
        [".qcow2", ".img", ".raw", ".iso", ".vhdx"]
            .iter()
            .any(|e| lower.ends_with(e))
            || Path::new(s).is_file()
    };
    Ok(match prefix {
        Some("container") | Some("docker") => Placement {
            backend: BackendKind::Container,
            image: ImageSource::oci(rest),
        },
        Some("disk") => Placement {
            backend: BackendKind::Qemu,
            image: ImageSource::disk(rest),
        },
        Some("vm") => Placement {
            backend: BackendKind::Qemu,
            image: if disk_like(rest) {
                ImageSource::disk(rest)
            } else {
                ImageSource::oci(rest)
            },
        },
        Some(_) => Placement {
            backend: BackendKind::Lume,
            image: ImageSource::oci(rest),
        },
        None if os.eq_ignore_ascii_case("macos") => Placement {
            backend: BackendKind::Lume,
            image: ImageSource::oci(rest),
        },
        None if disk_like(rest) => Placement {
            backend: BackendKind::Qemu,
            image: ImageSource::disk(rest),
        },
        None => Placement {
            backend: BackendKind::Container,
            image: ImageSource::oci(rest),
        },
    })
}

/// A placement after registry resolution, with the guest architecture to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// Backend and image (digest-pinned when the registry was read).
    pub placement: Placement,
    /// Guest architecture (VM images offering only another arch run under
    /// emulation); `None` = host.
    pub arch: Option<cua_vmm::Arch>,
    /// What the resolver said, when the registry was asked.
    pub image: Option<cua_image::ResolvedImage>,
}

/// [`place`], then resolve a bare, `vm:` or `container:` OCI ref with the one
/// resolver ([`cua_image::resolve`], see the module docs): the variant the
/// backend runs, pinned by digest. A registry that does not know the ref
/// (an image that only exists in the local engine) keeps the syntactic
/// placement; a private registry that refuses our credentials is an error.
pub async fn resolve_placement(image: &str, os: &str) -> Result<Resolved> {
    resolve_placement_with(image, os, None).await
}

/// [`resolve_placement`] reading a private image with explicit registry
/// credentials (a `RegistrySecret`) at the head of the auth chain.
pub async fn resolve_placement_with(
    image: &str,
    os: &str,
    creds: Option<&cua_image::RegistryCredentials>,
) -> Result<Resolved> {
    resolve_placement_inner(image, os, creds, false).await
}

/// [`resolve_placement_with`] for an explicit kind and runtime
/// ([`cua_sandbox_core::placement`]): `container` (or `gvisor`/`runc`)
/// runs the image's rootfs variant, `vm` + `qemu` its containerDisk,
/// `vm` + `lume` its Lume image, and `vm` alone Lume for a macOS guest
/// (or an image that only offers Lume) and QEMU otherwise. An image
/// without the variant the kind or runtime needs is
/// [`Error::InvalidPlacement`], listing what it offers. A backend prefix
/// on the image (`vm:`, `container:`, `lume:`) and `auto` for both keep
/// the image's own rule.
pub async fn resolve_placement_for(
    image: &str,
    os: &str,
    kind: cua_sandbox_core::placement::Kind,
    runtime: &cua_sandbox_core::placement::Runtime,
    creds: Option<&cua_image::RegistryCredentials>,
) -> Result<Resolved> {
    use cua_sandbox_core::placement::{Kind, Runtime};
    let prefixed = image.trim().split_once(':').is_some_and(|(p, r)| {
        ["container", "docker", "vm", "disk", "lume"].contains(&p) && !r.starts_with("//")
    });
    if prefixed || (kind == Kind::Auto && *runtime == Runtime::Auto) {
        return resolve_placement_with(image, os, creds).await;
    }
    let image = image.trim();
    let macos = matches!(os.to_ascii_lowercase().as_str(), "macos" | "darwin");
    let prefix = match (kind, runtime) {
        (_, Runtime::Gvisor | Runtime::Runc) | (Kind::Container, _) => "container",
        (_, Runtime::Lume) => "lume",
        (_, Runtime::Qemu) => "vm",
        _ if macos => "lume",
        _ => "vm",
    };
    let first = resolve_placement_inner(&format!("{prefix}:{image}"), os, creds, true).await;
    // `vm` with no runtime: a Lume-only image boots on Lume.
    if prefix == "vm"
        && *runtime == Runtime::Auto
        && let Err(Error::InvalidPlacement { valid, .. }) = &first
        && valid.iter().any(|v| v == "lume")
    {
        return resolve_placement_inner(&format!("lume:{image}"), os, creds, true).await;
    }
    first
}

async fn resolve_placement_inner(
    image: &str,
    os: &str,
    creds: Option<&cua_image::RegistryCredentials>,
    strict: bool,
) -> Result<Resolved> {
    use cua_image::resolve::{Backend, Variant};
    let placement = place(image, os)?;
    let prefix = image
        .trim()
        .split_once(':')
        .filter(|(p, r)| {
            ["container", "docker", "vm", "disk", "lume"].contains(p) && !r.starts_with("//")
        })
        .map(|(p, _)| p);
    let syntactic = |placement: Placement| Resolved {
        placement,
        arch: None,
        image: None,
    };
    let (reference, backend) = match (&placement.backend, &placement.image, prefix) {
        (_, ImageSource::Oci { reference }, Some("container" | "docker")) => {
            (reference.clone(), Backend::Container)
        }
        (BackendKind::Qemu, ImageSource::Oci { reference }, Some("vm")) => {
            (reference.clone(), Backend::Vm)
        }
        (BackendKind::Lume, ImageSource::Oci { reference }, None) => {
            (reference.clone(), Backend::Lume)
        }
        // Lume asked for explicitly: check the image has a Lume variant.
        (BackendKind::Lume, ImageSource::Oci { reference }, Some("lume")) if strict => {
            (reference.clone(), Backend::Lume)
        }
        (BackendKind::Container, ImageSource::Oci { reference }, None) => {
            (reference.clone(), Backend::Local)
        }
        _ => return Ok(syntactic(placement)),
    };
    let host = cua_vmm::Arch::host();
    let found = match cua_image::resolve::resolve_with_credentials(
        &reference,
        backend,
        host.oci(),
        creds,
    )
    .await
    {
        Ok(r) => r,
        Err(cua_image::ImageError::UnsupportedVariant { found, reason, .. }) => {
            if strict && !found.is_empty() {
                // The kind or runtime was asked for explicitly and the
                // image does not have it: say what it does have.
                let offers: Vec<String> = found.iter().map(|v| v.to_string()).collect();
                let mut valid = vec!["auto".to_string()];
                for v in &found {
                    let w = match cua_image::resolve::Variant::parse(v) {
                        Some(Variant::Rootfs) => "container",
                        Some(Variant::Containerdisk) => "qemu",
                        Some(Variant::Lume) => "lume",
                        None => continue,
                    };
                    if !valid.iter().any(|x| x == w) {
                        valid.push(w.to_string());
                    }
                }
                let wanted = match backend {
                    Backend::Container => "container (rootfs)",
                    Backend::Lume => "Lume",
                    _ => "VM (containerDisk)",
                };
                return Err(Error::InvalidPlacement {
                    message: format!(
                        "{reference} has no {wanted} variant (it offers: {}); valid: {}",
                        offers.join(", "),
                        valid.join(", ")
                    ),
                    axis: "image".into(),
                    valid,
                });
            }
            if found.is_empty() || prefix.is_some() || backend == Backend::Lume {
                // Not a variant we know (a cua QEMU disk artifact, ...), or
                // the caller named the backend: keep it as given.
                tracing::debug!(reference, %reason, "image variant unknown; using the image as given");
                return Ok(syntactic(placement));
            }
            return Err(Error::Unsupported(format!("{reference}: {reason}")));
        }
        Err(e) => {
            // A private registry that refuses us: say so instead of handing
            // a VM disk to the container engine (which would fail the same
            // way, less clearly). Docker Hub answers 401 for images that only
            // exist in the local engine, so it keeps the fallback, as does
            // a ref no registry knows.
            let host = cua_image::registry::parse_ref(&reference)
                .map(|r| r.resolve_registry().to_string())
                .unwrap_or_default();
            let hub = host.is_empty()
                || host.contains("docker.io")
                || host.starts_with("localhost")
                || host.starts_with("127.0.0.1");
            if !hub && matches!(e, cua_image::ImageError::Unauthorized(_)) {
                return Err(Error::ProviderNotConfigured(format!(
                    "cannot read {reference}: {e}. Log in to {host} (`docker login`, or for ECR \
                     `aws ecr get-login-password | docker login`), or set CUA_REGISTRY_USERNAME/PASSWORD"
                )));
            }
            tracing::debug!(reference, error = %e, "registry resolution failed; using the image string as given");
            return Ok(syntactic(placement));
        }
    };
    let arch = found
        .arch
        .as_deref()
        .and_then(|a| a.parse::<cua_vmm::Arch>().ok());
    let placement = match found.variant {
        Variant::Containerdisk => Placement {
            backend: BackendKind::Qemu,
            image: ImageSource::oci(&found.pinned_ref),
        },
        // Lume pulls by tag; the digest rides along so the cached base VM
        // follows it (a moved tag must not keep cloning the old base).
        Variant::Lume => Placement {
            backend: BackendKind::Lume,
            image: ImageSource::oci(&if found.digest.starts_with("sha256:") {
                format!("{}@{}", found.variant_ref, found.digest)
            } else {
                found.variant_ref.clone()
            }),
        },
        Variant::Rootfs => Placement {
            backend: BackendKind::Container,
            image: ImageSource::oci(&found.pinned_ref),
        },
    };
    if found.emulated {
        tracing::info!(
            reference,
            ?arch,
            "image has no {host} build; running it under emulation"
        );
    }
    Ok(Resolved {
        // VMs always name their arch. A container only when the image has
        // no build for this host: the engine then pulls and runs that
        // platform (binfmt emulation; runc only, see cua-vmm's container
        // backend), where the default platform would find no manifest.
        arch: if placement.backend == BackendKind::Qemu || found.emulated {
            arch
        } else {
            None
        },
        placement,
        image: Some(found),
    })
}

fn os_of(s: &str) -> GuestOs {
    match s.to_ascii_lowercase().as_str() {
        "macos" | "darwin" => GuestOs::Macos,
        "windows" => GuestOs::Windows,
        _ => GuestOs::Linux,
    }
}

fn status_of(s: Status) -> InstanceStatus {
    match s {
        Status::Running => InstanceStatus::Running,
        Status::Paused => InstanceStatus::Paused,
        Status::Stopped => InstanceStatus::Stopped,
        Status::Provisioning => InstanceStatus::Provisioning,
        Status::Unknown(u) => InstanceStatus::Unknown(u),
    }
}

fn endpoints_of(e: Endpoints) -> LocalEndpoints {
    LocalEndpoints {
        host: if e.host.is_empty() {
            "127.0.0.1".into()
        } else {
            e.host
        },
        ports: e.ports,
        vnc: e.vnc.map(|v| format!("{}:{}", v.host, v.port)),
        qmp: e.qmp.map(|q| format!("{}:{}", q.host, q.port)),
        ssh: e.ssh.map(|s| format!("{}:{}", s.host, s.port)),
        serial_log: e.serial_log.map(|p| p.display().to_string()),
        container_id: e.container_id,
    }
}

/// `cua-vmm` behind [`LocalRuntime`].
pub struct VmmLocal {
    container_cfg: ContainerConfig,
    container: tokio::sync::OnceCell<Arc<ContainerRuntime>>,
    qemu: Arc<QemuRuntime>,
    lume: Arc<LumeRuntime>,
}

impl Default for VmmLocal {
    fn default() -> Self {
        Self::new(ContainerConfig::default())
    }
}

impl VmmLocal {
    /// Default backends (QEMU resolves containerDisks through the image
    /// cache).
    pub fn new(container_cfg: ContainerConfig) -> Self {
        let qemu = QemuRuntime::new(QemuConfig {
            resolver: Some(cua_image::ContainerDiskResolver::shared()),
            ..QemuConfig::default()
        });
        Self {
            container_cfg,
            container: tokio::sync::OnceCell::new(),
            qemu: Arc::new(qemu),
            lume: Arc::new(LumeRuntime::with_defaults()),
        }
    }

    /// The container backend (connects to the engine on first use).
    pub async fn container(&self) -> std::result::Result<Arc<ContainerRuntime>, VmmError> {
        self.container
            .get_or_try_init(|| async {
                Ok(Arc::new(
                    ContainerRuntime::connect(self.container_cfg.clone()).await?,
                ))
            })
            .await
            .cloned()
    }

    /// The QEMU backend.
    pub fn qemu(&self) -> &Arc<QemuRuntime> {
        &self.qemu
    }

    async fn runtime(
        &self,
        kind: BackendKind,
    ) -> std::result::Result<Arc<dyn VmmRuntime>, VmmError> {
        Ok(match kind {
            BackendKind::Container => self.container().await? as Arc<dyn VmmRuntime>,
            BackendKind::Lume => self.lume.clone() as Arc<dyn VmmRuntime>,
            _ => self.qemu.clone() as Arc<dyn VmmRuntime>,
        })
    }

    /// The backend that owns `name`.
    async fn owner(&self, name: &str) -> RuntimeResult<(BackendKind, Arc<dyn VmmRuntime>)> {
        for kind in [BackendKind::Container, BackendKind::Qemu, BackendKind::Lume] {
            let Ok(rt) = self.runtime(kind).await else {
                continue;
            };
            if kind == BackendKind::Lume && !cfg!(target_os = "macos") {
                continue;
            }
            match rt.status(name).await {
                Ok(_) => return Ok((kind, rt)),
                Err(_) => continue,
            }
        }
        Err(RuntimeError::NotFound(name.into()))
    }
}

/// The engine repository local image builds are stored under
/// (`cua-vmm/build:cua-b-<hash>`).
pub const LOCAL_BUILD_REPO: &str = "cua-vmm/build";

/// The base a local build runs on and the content hash it is named by:
/// the registry digest when the registry knows `from`, else the local
/// engine's image id (an image only this machine has).
async fn local_build_base(
    rt: &ContainerRuntime,
    from: &str,
    creds: Option<&cua_image::RegistryCredentials>,
) -> RuntimeResult<(String, String)> {
    use cua_image::resolve::{Backend, Variant};
    let host = cua_vmm::Arch::host();
    match cua_image::resolve::resolve_with_credentials(from, Backend::Local, host.oci(), creds)
        .await
    {
        Ok(r) if r.variant == Variant::Rootfs => Ok((r.pinned_ref.clone(), r.pinned_ref)),
        Ok(r) => Err(RuntimeError::UnsupportedImage(format!(
            "{from} is a {} image: image layers build on container images; a VM image \
             takes them after boot only when it runs cua-spacesd",
            r.variant.as_str()
        ))),
        Err(cua_image::ImageError::UnsupportedVariant { reason, .. }) => {
            Err(RuntimeError::UnsupportedImage(format!(
                "{from}: {reason} (image layers build on container images)"
            )))
        }
        Err(e) => {
            // Not in a registry we can read: an image only the local engine
            // has, named by its engine id.
            match rt.docker().inspect_image(from).await {
                Ok(i) => {
                    let id = i.id.unwrap_or_default();
                    Ok((from.to_string(), format!("{from}@{id}")))
                }
                Err(_) => Err(RuntimeError::Other(format!("cannot build on {from}: {e}"))),
            }
        }
    }
}

/// The runtime a started instance really runs on, as persisted in
/// `runtime_type`: `gvisor` for a container under runsc (what the guest
/// detects, and what `cua doctor`'s `cross.runtime` compares), else the
/// backend (`container`, `qemu`, `lume`).
fn runtime_label(inst: &cua_vmm::Instance) -> String {
    match (inst.backend, &inst.isolation) {
        (BackendKind::Container, cua_vmm::Isolation::Gvisor) => "gvisor".into(),
        (backend, _) => backend.as_str().to_string(),
    }
}

#[async_trait]
impl LocalRuntime for VmmLocal {
    fn backend(&self) -> String {
        "vmm".into()
    }

    async fn build_image(
        &self,
        spec: &cua_fleet::BuildSpec,
        creds: Option<&cua_fleet::RegistryCredentials>,
    ) -> RuntimeResult<String> {
        let from = spec.from.trim();
        let (prefix, rest) = match from.split_once(':') {
            Some((p, r))
                if ["container", "docker", "vm", "disk", "lume"].contains(&p)
                    && !r.starts_with("//") =>
            {
                (Some(p), r)
            }
            _ => (None, from),
        };
        if matches!(prefix, Some("vm" | "disk" | "lume")) {
            return Err(RuntimeError::UnsupportedImage(format!(
                "{from}: image layers build on container images, not VM images"
            )));
        }
        let rt = self.container().await.map_err(rt_err)?;
        let (base, pinned) = local_build_base(&rt, rest, creds).await?;
        let files = spec
            .files
            .iter()
            .map(cua_fleet::builds::local_build_file)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| RuntimeError::Other(e.to_string()))?;
        let recipe = cua_fleet::builds::build_recipe(spec, &pinned, files, None);
        let tag = cua_fleet::builds::build_name(&recipe);
        let reference = format!("{LOCAL_BUILD_REPO}:{tag}");
        let ledger = cua_vmm::container::ledger::PullLedger::default();
        if rt.docker().inspect_image(&reference).await.is_ok() {
            tracing::info!(%reference, "local image build cached");
            ledger.touch(&reference);
            return Ok(format!("container:{reference}"));
        }
        let mut steps = Vec::new();
        if !spec.env.is_empty() {
            steps.push(cua_image::builder::BuildStep::Env(spec.env.clone()));
        }
        steps.extend(
            spec.files
                .iter()
                .map(|f| cua_image::builder::BuildStep::Copy {
                    src: f.source.clone(),
                    dst: f.destination.clone(),
                }),
        );
        steps.extend(
            spec.layers
                .iter()
                .cloned()
                .map(cua_image::builder::BuildStep::Layer),
        );
        steps.extend(
            spec.ports
                .iter()
                .copied()
                .map(cua_image::builder::BuildStep::Expose),
        );
        let out = crate::cua_home().join("build").join(&tag);
        let mut opts = cua_image::builder::BuildOptions::new(&out);
        opts.cpus = 2;
        opts.memory_mb = 2048;
        tracing::info!(%reference, base = %base, "building image layers locally");
        let build = cua_image::builder::build_container_into_engine(
            &rt,
            &base,
            &steps,
            &opts,
            LOCAL_BUILD_REPO,
            &tag,
            creds,
        );
        let budget = spec
            .timeout
            .unwrap_or(cua_fleet::builds::DEFAULT_BUILD_TIMEOUT);
        let built = tokio::time::timeout(budget, build).await;
        // The build directory is scratch either way.
        let _ = std::fs::remove_dir_all(&out);
        let timings = built
            .map_err(|_| RuntimeError::Other(format!("image build {tag} timed out")))?
            .map_err(|e| match e {
                cua_image::ImageError::Vmm(v @ VmmError::InsufficientDisk(_)) => rt_err(v),
                e => RuntimeError::Other(format!("image build {tag}: {e}")),
            })?;
        // Built images are cache: the ledger gives GC their last use.
        if let Ok(i) = rt.docker().inspect_image(&reference).await
            && let Some(id) = i.id
        {
            ledger.record(&reference, &id);
        }
        crate::maintenance::spawn_auto_gc("build");
        tracing::info!(%reference, ?timings, "built image locally");
        Ok(format!("container:{reference}"))
    }

    async fn gpu_support(&self) -> Vec<cua_sandbox_core::gpu::GpuSupport> {
        let mut out = Vec::new();
        if cfg!(target_os = "macos") {
            out.push(self.lume.gpu_support());
        }
        out.push(self.qemu.gpu_support().await);
        match self.container().await {
            Ok(rt) => {
                out.push(rt.gpu_support_for("container").await);
                out.push(rt.gpu_support_for("gvisor").await);
            }
            Err(e) => {
                for r in ["container", "gvisor"] {
                    out.push(cua_sandbox_core::gpu::GpuSupport::none(
                        r,
                        format!("No container engine: {e}"),
                    ));
                }
            }
        }
        out
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Ok(self.start_resolved(spec).await?.0)
    }

    async fn start_resolved(
        &self,
        spec: &LocalStartSpec,
    ) -> RuntimeResult<(LocalInstance, Option<cua_sandbox_core::ImageInfo>)> {
        use cua_sandbox_core::placement::{Kind, PlacementError, Runtime};
        let kind = spec
            .kind
            .as_deref()
            .map(Kind::parse)
            .transpose()
            .map_err(RuntimeError::InvalidPlacement)?
            .unwrap_or_default();
        let runtime = spec
            .runtime
            .as_deref()
            .map(Runtime::parse)
            .transpose()
            .map_err(RuntimeError::InvalidPlacement)?
            .unwrap_or_default();
        let r = resolve_placement_for(
            &spec.image,
            &spec.os,
            kind,
            &runtime,
            spec.registry_credentials.as_ref(),
        )
        .await
        .map_err(|e| match e {
            Error::Unsupported(m) => RuntimeError::UnsupportedImage(m),
            Error::InvalidPlacement {
                message,
                axis,
                valid,
            } => RuntimeError::InvalidPlacement(PlacementError {
                axis: match axis.as_str() {
                    "on" => cua_sandbox_core::placement::Axis::On,
                    "kind" => cua_sandbox_core::placement::Axis::Kind,
                    "runtime" => cua_sandbox_core::placement::Axis::Runtime,
                    _ => cua_sandbox_core::placement::Axis::Image,
                },
                given: spec.image.clone(),
                message,
                valid,
            }),
            other => RuntimeError::Other(other.to_string()),
        })?;
        let p = r.placement.clone();
        if spec.disk_size_gb.is_some() && p.backend == BackendKind::Container {
            return Err(RuntimeError::Unsupported {
                backend: "container".into(),
                op: "a disk size (disk_gb sizes VM disks; a container uses its engine's)".into(),
            });
        }
        let rt = self.runtime(p.backend).await.map_err(rt_err)?;
        let mut s = StartSpec::new(&spec.name, p.image)
            .os(os_of(&spec.os))
            .cpus(spec.cpus.max(1))
            .memory_mb(spec.memory_mb.max(256))
            .ready_timeout(spec.ready_timeout);
        if let Some(a) = r.arch {
            s = s.arch(a);
        }
        if let Some(f) = spec.firmware.as_deref().filter(|f| !f.is_empty()) {
            s = s.firmware(
                f.parse()
                    .map_err(|e: VmmError| RuntimeError::Other(e.to_string()))?,
            );
        }
        // Publish cua-spacesd's port only when the image may carry it
        // (a probe on it keeps it: the caller expects it).
        let no_env = r.image.as_ref().and_then(|i| i.spacesd) == Some(false)
            && !spec
                .probes
                .iter()
                .any(|p| p.port == cua_image::resolve::SPACESD_PORT);
        for port in &spec.ports {
            if no_env && *port == cua_image::resolve::SPACESD_PORT {
                continue;
            }
            s = s.port(*port);
        }
        for pr in &spec.probes {
            s = s.probe(match &pr.http_path {
                Some(path) => cua_vmm::Probe::http(pr.port, path.clone()),
                None => cua_vmm::Probe::tcp(pr.port),
            });
        }
        if spec.ephemeral {
            s = s.label(cua_vmm::container::LABEL_EPHEMERAL, "true");
        }
        s.env = spec.env.clone();
        s.command = spec.command.clone().filter(|c| !c.is_empty());
        s.registry_auth = spec.registry_credentials.clone();
        s.container_runtime = spec.container_runtime.clone();
        s.restrict_network = spec.restrict_network;
        s.disk_size_gb = spec.disk_size_gb;
        // The layers from the manifest: progress knows the total, and the
        // layers the engine already has, from the first byte.
        s.pull_layers = r
            .image
            .as_ref()
            .map(|i| i.layers.clone())
            .unwrap_or_default();
        // `auto` (or empty) is the backend's own option.
        s.gpu = spec.gpu.as_deref().map(|g| match (g.trim(), p.backend) {
            ("" | "auto" | "on", BackendKind::Lume) => cua_vmm::gpu::PARAVIRTUAL.to_string(),
            ("" | "auto" | "on", BackendKind::Qemu) => cua_vmm::gpu::VIRGL.to_string(),
            ("" | "auto" | "on", BackendKind::Container) => cua_vmm::gpu::NVIDIA.to_string(),
            (other, _) => other.to_string(),
        });
        if let Some(c) = spec
            .sidecars
            .iter()
            .find(|c| c.args.is_some() || c.cpu.is_some() || c.memory.is_some())
        {
            return Err(RuntimeError::Unsupported {
                backend: "local".into(),
                op: format!(
                    "sidecar {:?} args/cpu/memory (cloud pool templates only; fold args into \
                     command)",
                    c.name
                ),
            });
        }
        s.sidecars = spec
            .sidecars
            .iter()
            .map(|c| cua_vmm::SidecarSpec {
                name: c.name.clone(),
                image: c.image.clone(),
                command: c.command.clone(),
                env: c.env.clone(),
                ports: c.ports.clone(),
            })
            .collect();
        let inst = rt.start(&s).await.map_err(rt_err)?;
        // The start may have pulled an image or a base VM.
        crate::maintenance::spawn_auto_gc("start");
        Ok((
            LocalInstance {
                backend: runtime_label(&inst),
                name: inst.name,
                status: status_of(inst.status),
                endpoints: endpoints_of(inst.endpoints),
            },
            r.image.as_ref().map(Into::into),
        ))
    }

    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        let (_, rt) = self.owner(name).await?;
        rt.stop(name).await.map_err(rt_err)
    }

    async fn suspend(&self, name: &str) -> RuntimeResult<()> {
        let (_, rt) = self.owner(name).await?;
        rt.suspend(name).await.map_err(rt_err)
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        let (kind, rt) = self.owner(name).await?;
        // QEMU resumes a paused VM over QMP; a stopped one has no process
        // to resume and no start spec to boot from.
        if kind == BackendKind::Qemu && rt.status(name).await.map_err(rt_err)? == Status::Stopped {
            return Err(RuntimeError::Unsupported {
                backend: "qemu".into(),
                op: format!(
                    "starting {name} again: a QEMU VM resumes from suspend only, and this one \
                     was stopped (its disk is kept; delete it and create a new one)"
                ),
            });
        }
        let inst = rt.resume(name).await.map_err(rt_err)?;
        Ok(LocalInstance {
            backend: runtime_label(&inst),
            name: inst.name,
            status: status_of(inst.status),
            endpoints: endpoints_of(inst.endpoints),
        })
    }

    /// Containers pause in memory (`docker pause`) and QEMU VMs pause over
    /// QMP; Lume has no pause, so a Lume VM stops and boots again.
    fn power_control(&self, runtime_type: &str) -> Option<cua_sandbox_core::PowerControl> {
        use cua_sandbox_core::PowerControl;
        match runtime_type.trim().to_ascii_lowercase().as_str() {
            "lume" => Some(PowerControl::Stop),
            "gvisor" | "runsc" | "runc" | "container" | "docker" | "qemu-docker" | "qemu" => {
                Some(PowerControl::Suspend)
            }
            _ => None,
        }
    }

    async fn fork(&self, source: &str, new_name: &str) -> RuntimeResult<()> {
        let (_, rt) = self.owner(source).await?;
        rt.fork(source, new_name).await.map_err(rt_err)
    }

    async fn checkpoint(&self, name: &str, checkpoint: &str) -> RuntimeResult<()> {
        let (_, rt) = self.owner(name).await?;
        rt.checkpoint(name, checkpoint)
            .await
            .map(drop)
            .map_err(rt_err)
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        let mut out = Vec::new();
        for kind in [BackendKind::Container, BackendKind::Qemu] {
            let Ok(rt) = self.runtime(kind).await else {
                continue;
            };
            if let Ok(list) = rt.list().await {
                out.extend(list.into_iter().map(|s| LocalSummary {
                    name: s.name,
                    backend: s.backend.as_str().to_string(),
                    status: status_of(s.status),
                }));
            }
        }
        Ok(out)
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        let (_, rt) = self.owner(name).await?;
        rt.status(name).await.map(status_of).map_err(rt_err)
    }

    async fn status_on(&self, name: &str, runtime_type: &str) -> RuntimeResult<InstanceStatus> {
        // The engine the state file names; unknown words ask every engine.
        let kind = match runtime_type.trim().to_ascii_lowercase().as_str() {
            "qemu" => BackendKind::Qemu,
            "lume" if cfg!(target_os = "macos") => BackendKind::Lume,
            "gvisor" | "runsc" | "runc" | "container" | "docker" | "qemu-docker" => {
                BackendKind::Container
            }
            // Any engine may own it: not found only when every engine was
            // reachable and said so.
            _ => {
                let mut unreachable = None;
                for kind in [BackendKind::Container, BackendKind::Qemu, BackendKind::Lume] {
                    if kind == BackendKind::Lume && !cfg!(target_os = "macos") {
                        continue;
                    }
                    let rt = match self.runtime(kind).await {
                        Ok(rt) => rt,
                        Err(e) => {
                            unreachable = Some(e);
                            continue;
                        }
                    };
                    match rt.status(name).await {
                        Ok(s) => return Ok(status_of(s)),
                        Err(VmmError::NotFound(_)) => {}
                        Err(e) => unreachable = Some(e),
                    }
                }
                return Err(match unreachable {
                    Some(e) => RuntimeError::Other(e.to_string()),
                    None => RuntimeError::NotFound(name.into()),
                });
            }
        };
        let rt = self.runtime(kind).await.map_err(rt_err)?;
        rt.status(name).await.map(status_of).map_err(rt_err)
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        let (_, rt) = self.owner(name).await?;
        rt.delete(name).await.map_err(rt_err)
    }

    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        let (_, rt) = self.owner(name).await?;
        rt.endpoints(name).await.map(endpoints_of).map_err(rt_err)
    }

    async fn guest_tcp_listening(
        &self,
        name: &str,
        guest_port: u16,
    ) -> RuntimeResult<Option<bool>> {
        let (_, rt) = self.owner(name).await?;
        rt.guest_tcp_listening(name, guest_port)
            .await
            .map_err(rt_err)
    }

    async fn guest_exec(
        &self,
        name: &str,
        script: &str,
        timeout: Option<std::time::Duration>,
        sink: tokio::sync::mpsc::Sender<cua_sandbox_core::GuestOutput>,
    ) -> RuntimeResult<i64> {
        let rt = self.agentless_owner(name, "exec").await?;
        let exec = rt.guest_exec(name).await.map_err(rt_err)?.ok_or_else(|| {
            RuntimeError::Unsupported {
                backend: "lume".into(),
                op: format!("agentless exec in {name} (not a macOS guest)"),
            }
        })?;
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let mut req = cua_vmm::ExecRequest::sh(script);
        req.timeout = timeout;
        let forward = async move {
            while let Some(c) = rx.recv().await {
                let out = match c {
                    cua_vmm::ExecChunk::Stdout(b) => cua_sandbox_core::GuestOutput::Stdout(b),
                    cua_vmm::ExecChunk::Stderr(b) => cua_sandbox_core::GuestOutput::Stderr(b),
                };
                if sink.send(out).await.is_err() {
                    break;
                }
            }
        };
        let (code, ()) = tokio::join!(exec.exec_streaming(req, tx), forward);
        code.map_err(rt_err)
    }

    async fn guest_screenshot(
        &self,
        name: &str,
    ) -> RuntimeResult<cua_sandbox_core::GuestScreenshot> {
        let rt = self.agentless_owner(name, "screenshots").await?;
        let vnc = vnc_of(rt.as_ref(), name).await?;
        let fb = cua_vmm::vnc::capture_png(&vnc, std::time::Duration::from_secs(30))
            .await
            .map_err(rt_err)?;
        Ok(cua_sandbox_core::GuestScreenshot {
            png: fb.png,
            width: fb.width,
            height: fb.height,
            via: "vnc".into(),
        })
    }

    async fn guest_display(&self, name: &str) -> RuntimeResult<cua_sandbox_core::GuestDisplay> {
        let rt = self.agentless_owner(name, "a display").await?;
        let vnc = vnc_of(rt.as_ref(), name).await?;
        let url = match &vnc.password {
            Some(p) => format!("vnc://:{p}@{}:{}", vnc.host, vnc.port),
            None => format!("vnc://{}:{}", vnc.host, vnc.port),
        };
        Ok(cua_sandbox_core::GuestDisplay {
            url,
            via: "vnc".into(),
            // `lume attach` opens the VM's own display (VNC as its fallback)
            // and handles the password.
            open_command: cua_vmm::lume::lume_bin()
                .map(|b| vec![b.display().to_string(), "attach".into(), name.to_string()]),
        })
    }
}

/// The VNC endpoint of a running instance.
async fn vnc_of(rt: &dyn VmmRuntime, name: &str) -> RuntimeResult<cua_vmm::VncEndpoint> {
    rt.endpoints(name)
        .await
        .map_err(rt_err)?
        .vnc
        .ok_or_else(|| {
            RuntimeError::Other(format!(
                "{name} has no VNC endpoint (is it running? `cua sb start {name}`)"
            ))
        })
}

impl VmmLocal {
    /// The runtime of `name` when it has agentless access (Lume), else
    /// [`RuntimeError::Unsupported`].
    async fn agentless_owner(&self, name: &str, op: &str) -> RuntimeResult<Arc<dyn VmmRuntime>> {
        let (kind, rt) = self.owner(name).await?;
        if kind != BackendKind::Lume {
            return Err(RuntimeError::Unsupported {
                backend: kind.as_str().into(),
                op: format!("{op} without cua-spacesd (Lume sandboxes only)"),
            });
        }
        Ok(rt)
    }
}

// ------------------------------------------------------------------ doctor

/// `cua runtime doctor`: the `cua-vmm` report (read-only).
pub async fn doctor_report() -> DoctorReport {
    auto::doctor().await
}

/// The report as `cua.daemon.v1` checks (one per backend).
pub fn doctor_checks(r: &DoctorReport) -> Vec<cua_proto::daemon::v1::RuntimeCheck> {
    use cua_proto::daemon::v1::{CheckStatus, RuntimeCheck};
    r.backends
        .iter()
        .map(|b| {
            let status = if b.ready {
                CheckStatus::Ok
            } else if b.provisionable {
                CheckStatus::Installable
            } else {
                CheckStatus::Error
            };
            let mut detail = b.detail.clone();
            if !b.missing.is_empty() {
                detail.push_str(&format!("; missing: {}", b.missing.join(", ")));
            }
            if !b.provisioning.is_empty() {
                detail.push_str(&format!("; setup would: {}", b.provisioning.join("; ")));
            }
            RuntimeCheck {
                name: b.backend.as_str().into(),
                status: status as i32,
                version: match b.backend {
                    BackendKind::Qemu => r.qemu.version.clone().unwrap_or_default(),
                    BackendKind::Lume => r.lume.version.clone().unwrap_or_default(),
                    BackendKind::Container => r.container.engine.clone().unwrap_or_default(),
                    _ => String::new(),
                },
                detail,
            }
        })
        .collect()
}

/// `cua runtime setup`: provisions `components` (`qemu`, `lume`; `runsc`
/// reports its install plan). `dry_run` only reports. Every step is
/// returned; nothing happens implicitly.
pub async fn setup(
    components: &[String],
    dry_run: bool,
) -> Result<Vec<cua_proto::daemon::v1::SetupStep>> {
    use cua_proto::daemon::v1::{CheckStatus, SetupStep};
    let report = doctor_report().await;
    let wanted: Vec<String> = if components.is_empty() {
        report
            .backends
            .iter()
            .filter(|b| !b.ready && b.provisionable)
            .map(|b| b.backend.as_str().to_string())
            .collect()
    } else {
        components.to_vec()
    };
    let mut steps = Vec::new();
    for c in wanted {
        let kind = match c.as_str() {
            "qemu" => Some(BackendKind::Qemu),
            "lume" => Some(BackendKind::Lume),
            "container" | "runsc" | "gvisor" => Some(BackendKind::Container),
            _ => None,
        };
        let Some(kind) = kind else {
            return Err(Error::InvalidArgument(format!("unknown component {c:?}")));
        };
        let b = report.backend(kind).cloned();
        let plan = b
            .as_ref()
            .map(|b| b.provisioning.join("; "))
            .unwrap_or_default();
        if b.as_ref().is_some_and(|b| b.ready) {
            steps.push(SetupStep {
                name: c,
                status: CheckStatus::Ok as i32,
                detail: "already available".into(),
            });
            continue;
        }
        if dry_run {
            steps.push(SetupStep {
                name: c,
                status: CheckStatus::Installable as i32,
                detail: format!("would: {plan}"),
            });
            continue;
        }
        let result = match kind {
            BackendKind::Qemu => cua_vmm::qemu::install_qemu().await,
            BackendKind::Lume => cua_vmm::lume::install_lume().await,
            _ => Err(VmmError::Unsupported {
                backend: "container",
                op: "unattended runsc install (run the plan shown by --dry-run)",
            }),
        };
        steps.push(match result {
            Ok(()) => SetupStep {
                name: c,
                status: CheckStatus::Ok as i32,
                detail: format!("installed ({plan})"),
            },
            Err(e) => SetupStep {
                name: c,
                status: CheckStatus::Error as i32,
                detail: e.to_string(),
            },
        });
    }
    Ok(steps)
}

// ------------------------------------------------------------------ images

/// A local image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalImageInfo {
    /// Reference.
    pub reference: String,
    /// `container` or `vm`.
    pub kind: String,
    /// Local path (VM disks) or engine image id (containers).
    pub location: String,
    /// Size in bytes, when known.
    pub size_bytes: u64,
}

/// Pulls `reference` for the backend its placement selects.
pub async fn pull_image(local: &VmmLocal, reference: &str) -> Result<LocalImageInfo> {
    let r = resolve_placement(reference, "linux").await?;
    let arch = r.arch.unwrap_or_else(cua_vmm::Arch::host);
    let p = r.placement;
    match (p.backend, p.image) {
        (BackendKind::Container, ImageSource::Oci { reference: r }) => {
            let rt = local.container().await.map_err(vmm_error)?;
            let platform = format!("linux/{}", cua_vmm::Arch::host().oci());
            rt.ensure_image(&r, Some(&platform))
                .await
                .map_err(vmm_error)?;
            let size = rt
                .docker()
                .inspect_image(&r)
                .await
                .ok()
                .and_then(|i| i.size)
                .unwrap_or(0);
            crate::maintenance::spawn_auto_gc("pull");
            Ok(LocalImageInfo {
                reference: r.clone(),
                kind: "container".into(),
                location: r,
                size_bytes: size.max(0) as u64,
            })
        }
        (BackendKind::Qemu, ImageSource::Oci { reference: r }) => {
            let client = cua_image::RegistryClient::default();
            let cache = cua_image::ImageCache::default();
            let disk = cua_image::containerdisk::pull(&client, &cache, &r, arch.oci(), false)
                .await
                .map_err(image_error)?;
            let size = std::fs::metadata(&disk).map(|m| m.len()).unwrap_or(0);
            crate::maintenance::spawn_auto_gc("pull");
            Ok(LocalImageInfo {
                reference: r,
                kind: "vm".into(),
                location: disk.display().to_string(),
                size_bytes: size,
            })
        }
        (b, _) => Err(Error::Unsupported(format!(
            "pulling {reference} for the {b} backend happens on first start"
        ))),
    }
}

/// Copies `reference` (host architecture) to `destination` in another
/// registry: pull manifest and blobs, push them.
pub async fn push_image(reference: &str, destination: &str) -> Result<String> {
    let client = cua_image::RegistryClient::default();
    let cache = cua_image::ImageCache::default();
    let arch = cua_vmm::Arch::host().oci();
    let (pinned, manifest, _digest) = client
        .resolve_platform(reference, arch)
        .await
        .map_err(image_error)?;
    let descs: Vec<_> = manifest
        .config
        .iter()
        .cloned()
        .chain(manifest.layers.iter().cloned())
        .collect();
    let missing: u64 = descs
        .iter()
        .filter(|d| cache.blob_path(&d.digest).is_ok_and(|p| !p.exists()))
        .map(|d| d.size)
        .sum();
    cua_vmm::disk::ensure_space(cache.root(), missing, &format!("copy {reference}"))
        .map_err(|e| vmm_error(e.into()))?;
    for d in &descs {
        let path = cache.blob_path(&d.digest).map_err(image_error)?;
        if !path.exists() {
            client
                .blob_to_file(&pinned, d, &path)
                .await
                .map_err(image_error)?;
        }
        client
            .push_blob_file(destination, &path, &d.digest)
            .await
            .map_err(image_error)?;
    }
    client
        .push_manifest(destination, &manifest)
        .await
        .map_err(image_error)
}

/// Builds an image from a `images.cua.ai/v1alpha1` Image resource (JSON)
/// on top of `base` (placement rules as for sandboxes), optionally pushing
/// it. Returns the pushed digest, or the local output directory.
pub async fn build_image(
    local: &VmmLocal,
    spec_json: &str,
    base: &str,
    out_dir: &Path,
    push: Option<String>,
) -> Result<String> {
    let resource: cua_image::ImageResource = serde_json::from_str(spec_json)
        .map_err(|e| Error::InvalidArgument(format!("image spec: {e}")))?;
    let steps = cua_image::builder::steps_from_spec(&resource).map_err(image_error)?;
    let mut opts = cua_image::builder::BuildOptions::new(out_dir);
    opts.push = push;
    let p = place(base, "linux")?;
    let out = build_with(local, p, &steps, &opts).await;
    crate::maintenance::spawn_auto_gc("build");
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            // Nothing usable is left in a failed build's directory.
            let _ = std::fs::remove_dir_all(out_dir);
            return Err(e);
        }
    };
    Ok(match out.pushed {
        Some((r, d)) => {
            // Pushed: the registry holds the result; the local copy is
            // scratch.
            let _ = std::fs::remove_dir_all(out_dir);
            format!("{r}@{d}")
        }
        None => {
            // Kept as the result; the cache GC treats it as a build output
            // (least recently used first when over budget).
            cua_vmm::disk::mark_used(out_dir);
            out_dir.display().to_string()
        }
    })
}

async fn build_with(
    local: &VmmLocal,
    p: Placement,
    steps: &[cua_image::builder::BuildStep],
    opts: &cua_image::builder::BuildOptions,
) -> Result<cua_image::builder::BuildOutput> {
    let out = match (p.backend, p.image) {
        (BackendKind::Container, ImageSource::Oci { reference }) => {
            let rt = local.container().await.map_err(vmm_error)?;
            cua_image::builder::build_container(&rt, &reference, steps, opts, None)
                .await
                .map_err(image_error)?
        }
        (BackendKind::Qemu, image) => {
            let disk = match image {
                ImageSource::Disk { path } => path,
                ImageSource::Oci { reference } => cua_image::containerdisk::pull(
                    &cua_image::RegistryClient::default(),
                    &cua_image::ImageCache::default(),
                    &reference,
                    opts.arch.oci(),
                    false,
                )
                .await
                .map_err(image_error)?,
                ImageSource::Existing => {
                    return Err(Error::InvalidArgument("base must be an image".into()));
                }
            };
            cua_image::builder::build_vm(local.qemu(), &disk, steps, opts)
                .await
                .map_err(image_error)?
        }
        (b, _) => {
            return Err(Error::Unsupported(format!(
                "local builds on the {b} backend are not supported"
            )));
        }
    };
    Ok(out)
}

#[cfg(test)]
mod tests {

    #[test]
    fn containers_under_gvisor_report_the_gvisor_runtime() {
        let inst = |backend, isolation| cua_vmm::Instance {
            name: "sb".into(),
            backend,
            status: cua_vmm::Status::Running,
            endpoints: Default::default(),
            isolation,
            arch: None,
        };
        use cua_vmm::Isolation;
        assert_eq!(
            runtime_label(&inst(BackendKind::Container, Isolation::Gvisor)),
            "gvisor"
        );
        assert_eq!(
            runtime_label(&inst(BackendKind::Container, Isolation::Runc)),
            "container"
        );
        let vm = Isolation::Vm {
            accel: "hvf".into(),
        };
        assert_eq!(runtime_label(&inst(BackendKind::Qemu, vm)), "qemu");
    }

    use super::*;

    /// The macOS path of "Spaces on your machines", verified without a VM:
    /// the canonical macOS image (what a host resolves `macos:26` to) is a
    /// Lume VM according to the registry. Reads ghcr.io; creates nothing.
    #[tokio::test]
    #[ignore = "network: reads the canonical macOS image's manifest from ghcr.io (no VM is created)"]
    async fn the_canonical_macos_image_runs_on_lume() {
        let image = cua_image::canonical::alias_with("macos:26", &|_| None)
            .expect("macos alias")
            .0;
        let r = resolve_placement_for(
            &image,
            "",
            cua_sandbox_core::placement::Kind::Auto,
            &cua_sandbox_core::placement::Runtime::Auto,
            None,
        )
        .await
        .unwrap();
        assert_eq!(r.placement.backend, BackendKind::Lume, "{image}: {r:?}");
    }

    /// A rootfs published only for the other architecture keeps that arch,
    /// so the container engine pulls (and emulates) the right platform.
    /// The fake registry is process-wide: tests that install one take turns.
    static FAKE_REGISTRY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// An explicit kind or runtime picks the image's variant, and an image
    /// without it is an invalid placement that lists what it offers.
    #[tokio::test]
    async fn kind_and_runtime_pick_the_variant() {
        use cua_sandbox_core::placement::{Kind, Runtime};
        let _turn = FAKE_REGISTRY.lock().await;
        let host = cua_vmm::Arch::host().oci();
        let mut reg = cua_image::testing::FakeRegistry::default();
        reg.index("ghcr.io/acme/app:1", &[host], false, None);
        reg.index("ghcr.io/acme/app:1-disk", &[host], true, None);
        reg.index("ghcr.io/acme/disk:1", &[host], true, None);
        reg.lume("ghcr.io/acme/mac:1");
        cua_image::resolve::set_source(Some(std::sync::Arc::new(reg)));
        let place = |image: &'static str, kind: Kind, runtime: Runtime| async move {
            resolve_placement_for(image, "linux", kind, &runtime, None).await
        };
        let auto = place("ghcr.io/acme/app:1", Kind::Auto, Runtime::Auto).await;
        let vm = place("ghcr.io/acme/app:1", Kind::Vm, Runtime::Auto).await;
        let container = place("ghcr.io/acme/app:1", Kind::Container, Runtime::Runc).await;
        let lume_on_linux = place("ghcr.io/acme/app:1", Kind::Auto, Runtime::Lume).await;
        let disk_as_container = place("ghcr.io/acme/disk:1", Kind::Container, Runtime::Auto).await;
        let lume_only_vm = place("ghcr.io/acme/mac:1", Kind::Vm, Runtime::Auto).await;
        cua_image::resolve::set_source(None);

        assert_eq!(auto.unwrap().placement.backend, BackendKind::Container);
        let vm = vm.unwrap();
        assert_eq!(vm.placement.backend, BackendKind::Qemu);
        assert!(
            matches!(&vm.placement.image, ImageSource::Oci { reference } if reference.contains("app@sha256")),
            "{:?}",
            vm.placement.image
        );
        assert_eq!(container.unwrap().placement.backend, BackendKind::Container);
        match lume_on_linux.unwrap_err() {
            Error::InvalidPlacement { valid, message, .. } => {
                assert_eq!(valid, ["auto", "container"], "{message}");
            }
            other => panic!("{other:?}"),
        }
        match disk_as_container.unwrap_err() {
            Error::InvalidPlacement { valid, message, .. } => {
                assert!(
                    message.contains("no container (rootfs) variant"),
                    "{message}"
                );
                assert_eq!(valid, ["auto", "qemu"]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(lume_only_vm.unwrap().placement.backend, BackendKind::Lume);
    }

    #[tokio::test]
    async fn foreign_arch_rootfs_keeps_its_arch() {
        let _turn = FAKE_REGISTRY.lock().await;
        let other = match cua_vmm::Arch::host() {
            cua_vmm::Arch::X86_64 => ("arm64", cua_vmm::Arch::Aarch64),
            _ => ("amd64", cua_vmm::Arch::X86_64),
        };
        let mut reg = cua_image::testing::FakeRegistry::default();
        reg.index("ghcr.io/acme/only-other:1", &[other.0], false, None);
        cua_image::resolve::set_source(Some(std::sync::Arc::new(reg)));
        let r = resolve_placement("ghcr.io/acme/only-other:1", "linux").await;
        cua_image::resolve::set_source(None);
        let r = r.unwrap();
        assert_eq!(r.placement.backend, BackendKind::Container);
        assert_eq!(r.arch, Some(other.1));
        assert!(r.image.as_ref().unwrap().emulated);
    }

    /// Live, read-only registry reads (`CUA_E2E_IMAGE=1`): the canonical
    /// images land on the backend and architecture their variant needs.
    #[tokio::test]
    async fn canonical_refs_resolve_to_their_backend() {
        if std::env::var("CUA_E2E_IMAGE").as_deref() != Ok("1") {
            eprintln!("skipping: set CUA_E2E_IMAGE=1");
            return;
        }
        let r = resolve_placement("ghcr.io/trycua/linux:24.04", "linux")
            .await
            .unwrap();
        assert_eq!(r.placement.backend, BackendKind::Container);
        assert_eq!(r.image.as_ref().unwrap().spacesd, Some(true));
        let r = resolve_placement("vm:ghcr.io/trycua/linux:24.04", "linux")
            .await
            .unwrap();
        assert_eq!(r.placement.backend, BackendKind::Qemu);
        assert_eq!(r.arch, Some(cua_vmm::Arch::host()), "multi-arch: host wins");
        // The containerDisk says it runs cua-spacesd (annotation), so
        // 3211 is published for the VM too.
        assert_eq!(r.image.as_ref().unwrap().spacesd, Some(true));
        let r = resolve_placement("ghcr.io/trycua/windows:2022", "windows")
            .await
            .unwrap();
        assert_eq!(r.placement.backend, BackendKind::Qemu);
        assert_eq!(r.arch, Some(cua_vmm::Arch::X86_64), "amd64-only index");
        let img = r.image.as_ref().unwrap();
        assert_eq!((img.os.as_str(), img.spacesd), ("windows", Some(false)));
        let r = resolve_placement("python:3.12-slim", "linux")
            .await
            .unwrap();
        assert_eq!(r.placement.backend, BackendKind::Container);
        assert_eq!(r.image.as_ref().unwrap().spacesd, Some(false));
        if cfg!(target_os = "macos") {
            let r = resolve_placement("ghcr.io/trycua/macos:26", "macos")
                .await
                .unwrap();
            assert_eq!(r.placement.backend, BackendKind::Lume);
            assert_eq!(r.image.as_ref().unwrap().spacesd, Some(false));
        }
    }

    #[test]
    fn placement_rules() {
        let c = place("nginx:alpine", "linux").unwrap();
        assert_eq!(c.backend, BackendKind::Container);
        assert_eq!(c.image, ImageSource::oci("nginx:alpine"));
        let c = place("container:ghcr.io/x/y:docker-1", "linux").unwrap();
        assert_eq!(c.image, ImageSource::oci("ghcr.io/x/y:docker-1"));
        let v = place("vm:ghcr.io/x/y:disk", "linux").unwrap();
        assert_eq!(v.backend, BackendKind::Qemu);
        assert_eq!(v.image, ImageSource::oci("ghcr.io/x/y:disk"));
        let d = place("/tmp/a.qcow2", "linux").unwrap();
        assert_eq!(d.image, ImageSource::disk("/tmp/a.qcow2"));
        let l = place("ghcr.io/trycua/macos-sequoia:latest", "macos").unwrap();
        assert_eq!(l.backend, BackendKind::Lume);
        let l = place("lume:ubuntu", "linux").unwrap();
        assert_eq!(l.backend, BackendKind::Lume);
        // A registry with a port is not a prefix.
        let r = place("localhost:5000/img:tag", "linux").unwrap();
        assert_eq!(r.image, ImageSource::oci("localhost:5000/img:tag"));
        assert!(place(" ", "linux").is_err());
    }
}
