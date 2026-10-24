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
//! | any rootfs image with `kind = microvm` (or `runtime = firecracker`) | Firecracker (Linux/KVM) |
//! | the same on a Mac (or `runtime = vz`) | Virtualization.framework under `cua-vz` (macOS 14+; checkpoints and forks) |
//! | the same on a Mac without `cua-vz` (or `runtime = containerization`) | Apple's Containerization (macOS 26) |
//! | bare ref without a container engine, with ready QEMU and a containerDisk | QEMU |
//!
//! A bare, `vm:` or `container:` OCI ref is resolved first by the one
//! resolver ([`cua_image::resolve`]: the variant the backend runs, pinned by
//! digest), so any canonical image ref runs on the right backend:
//! a KubeVirt containerDisk boots under QEMU for the architecture the image
//! offers (the host's when it has one), a Lume image under Lume, a rootfs as
//! a container, and an image no local backend runs fails with
//! [`RuntimeError::UnsupportedImage`].
//!
//! Operations on an existing instance find its backend by asking each one
//! (container, QEMU, Lume, Firecracker, vz, Containerization) for the
//! instance's status, all at once.

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
    containerization::ContainerizationRuntime,
    lume::LumeRuntime,
    managed::{self, LinuxSource},
    microvm::{MicroVmConfig, MicroVmRuntime},
    qemu::{QemuConfig, QemuRuntime},
};
use std::{collections::HashMap, path::Path, sync::Arc};

/// The engine that answered for `name` among `answers` (each engine's
/// status lookup, in the order asked). Not found only when every engine
/// answered and none has it: an engine that could not be asked (`lume serve`
/// gone, say) may own the instance, so its error is returned instead of a
/// "not found" that sends the caller to recreate a Space that still exists.
fn pick_owner<R>(
    name: &str,
    answers: Vec<(BackendKind, std::result::Result<(), VmmError>, R)>,
) -> RuntimeResult<(BackendKind, R)> {
    let mut unreachable = None;
    for (kind, status, rt) in answers {
        match status {
            Ok(()) => return Ok((kind, rt)),
            Err(VmmError::NotFound(_)) => {}
            Err(e) => unreachable = unreachable.or(Some(e)),
        }
    }
    Err(match unreachable {
        Some(e) => rt_err(e),
        None => RuntimeError::NotFound(name.into()),
    })
}

fn rt_err(e: VmmError) -> RuntimeError {
    match e {
        VmmError::NotFound(n) => RuntimeError::NotFound(n),
        VmmError::UnsupportedImage { reason, .. } => RuntimeError::UnsupportedImage(reason),
        VmmError::InsufficientDisk(d) => RuntimeError::InsufficientDisk(d.to_string()),
        VmmError::HostCapacity(m) => RuntimeError::HostCapacity(m),
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
        e @ (VmmError::Missing { .. } | VmmError::NotRunning { .. }) => {
            Error::ProviderNotConfigured(e.to_string())
        }
        VmmError::InsufficientDisk(d) => Error::InsufficientDisk(d.to_string()),
        VmmError::HostCapacity(m) => Error::HostCapacity(m),
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

fn backend_prefix(image: &str) -> Option<(&str, &str)> {
    image.trim().split_once(':').filter(|(p, r)| {
        ["container", "docker", "vm", "disk", "lume"].contains(p) && !r.starts_with("//")
    })
}

fn missing_fallback_allowed(
    kind: cua_sandbox_core::placement::Kind,
    runtime: &cua_sandbox_core::placement::Runtime,
    prefix: Option<(&str, &str)>,
    backend: BackendKind,
    source: LinuxSource,
) -> bool {
    use cua_sandbox_core::placement::{Kind, Runtime};
    kind == Kind::Auto
        && *runtime == Runtime::Auto
        && prefix.is_none()
        && backend == BackendKind::Container
        && source == LinuxSource::Auto
}

async fn vm_instead(
    image: &str,
    os: &str,
    creds: Option<&cua_image::RegistryCredentials>,
    qemu_ready: bool,
) -> Option<Resolved> {
    if !qemu_ready {
        return None;
    }
    let r = resolve_placement_inner(&format!("vm:{}", image.trim()), os, creds, true)
        .await
        .ok()?;
    r.image
        .as_ref()
        .is_some_and(|i| i.variant == cua_image::resolve::Variant::Containerdisk)
        .then_some(r)
}

/// Decides the backend for `image` / `os` (see the module docs).
pub fn place(image: &str, os: &str) -> Result<Placement> {
    let image = image.trim();
    if image.is_empty() {
        return Err(Error::InvalidArgument("image is required".into()));
    }
    let (prefix, rest) = match backend_prefix(image) {
        Some((p, r)) => (Some(p), r),
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
    let prefixed = backend_prefix(image).is_some();
    if prefixed || (kind == Kind::Auto && *runtime == Runtime::Auto) {
        return resolve_placement_with(image, os, creds).await;
    }
    let image = image.trim();
    // A microVM boots the image's container rootfs: Firecracker on Linux;
    // on a Mac cua's own monitor (checkpoints and forks), else Apple's
    // Containerization.
    if kind == Kind::MicroVm
        || matches!(
            runtime,
            Runtime::Firecracker | Runtime::Vz | Runtime::Containerization
        )
    {
        let mut r = resolve_placement_inner(&format!("container:{image}"), os, creds, true).await?;
        r.placement.backend = match runtime {
            Runtime::Firecracker => BackendKind::Firecracker,
            Runtime::Vz => BackendKind::Vz,
            Runtime::Containerization => BackendKind::Containerization,
            _ if cfg!(target_os = "macos") => {
                if cua_sandbox_core::placement::HostFacts::current().vz {
                    BackendKind::Vz
                } else {
                    BackendKind::Containerization
                }
            }
            _ => BackendKind::Firecracker,
        };
        r.arch = None;
        return Ok(r);
    }
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

/// What to do instead of a VM for a bare OCI image: a VM boots a disk with a
/// kernel and a bootloader, which only Cua's VM images carry.
fn vm_hint(backend: cua_image::resolve::Backend, offers: &[String]) -> &'static str {
    let rootfs_only = !offers.is_empty() && offers.iter().all(|v| v == "rootfs");
    if rootfs_only
        && !matches!(
            backend,
            cua_image::resolve::Backend::Container | cua_image::resolve::Backend::Lume
        )
    {
        ". A VM needs a Cua disk image: use kind container or microvm (cua-spacesd is put into \
         the image), or `cua build` FROM a Cua image"
    } else {
        ""
    }
}

#[rustfmt::skip]
fn refuse_gpu(image: &str, gpu: bool, r: &Resolved) -> RuntimeResult<()> {
    let digest = r.image.as_ref().map(|i| i.digest.as_str());
    let Some(e) = gpu.then(|| cua_image::catalog::entries().iter().find(|e| {
        e.gpu == Some(false) && (e.reference == image.trim() || e.digest.is_some() && e.digest.as_deref() == digest)
    })).flatten() else { return Ok(()) };
    let name = e.distro.as_ref().map_or(e.reference.as_str(), |d| d.name.as_str());
    Err(RuntimeError::Unsupported {
        backend: r.placement.backend.as_str().into(),
        op: format!("GPU acceleration on {name}"),
    })
}

async fn resolve_placement_inner(
    image: &str,
    os: &str,
    creds: Option<&cua_image::RegistryCredentials>,
    strict: bool,
) -> Result<Resolved> {
    use cua_image::resolve::{Backend, Variant};
    let placement = place(image, os)?;
    let prefix = backend_prefix(image).map(|(p, _)| p);
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
                        "{reference} has no {wanted} variant (it offers: {}); valid: {}{}",
                        offers.join(", "),
                        valid.join(", "),
                        vm_hint(backend, &found),
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
        bulk: e.bulk.map(|p| p.display().to_string()),
    }
}

/// `cua-vmm` behind [`LocalRuntime`].
pub struct VmmLocal {
    container_cfg: ContainerConfig,
    /// The container engine, and the `runtime.linux` setting it was chosen
    /// for (chosen again when the setting changes).
    container: tokio::sync::Mutex<Option<(LinuxSource, Engine)>>,
    qemu: Arc<QemuRuntime>,
    lume: Arc<LumeRuntime>,
    microvm: Arc<MicroVmRuntime>,
    /// microVMs on a Mac (Apple's Containerization).
    apple: Arc<ContainerizationRuntime>,
    /// The warm pool, once [`VmmLocal::enable_pool`] turned it on (the
    /// daemon does; an embedded runtime never pools).
    pool: std::sync::OnceLock<crate::pool::WarmPool>,
    /// Named pools as last applied: shape key, or why not.
    named_state: std::sync::Mutex<HashMap<String, (Option<String>, String)>>,
    /// Image placements as last resolved, served at once on a warm-pool
    /// host and refreshed in the background, so a create never waits on a
    /// registry for a tag it resolved before.
    resolved: Arc<std::sync::Mutex<HashMap<ResolveKey, (std::time::Instant, Resolved)>>>,
}

/// What an image placement is resolved for: image, OS, kind, runtime.
type ResolveKey = (String, String, String, String);

/// How old a cached placement may be before a create refreshes it (in the
/// background; the create uses the cached one).
const RESOLVE_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// The container engine [`VmmLocal`] uses.
#[derive(Clone)]
enum Engine {
    /// This Mac's own (Docker Desktop, Colima, OrbStack, a Linux dockerd).
    System(Arc<ContainerRuntime>),
    /// The built-in Linux runtime (its VM boots on demand).
    Builtin(Arc<managed::ManagedContainers>),
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
            identity: Some(Arc::new(crate::guest_identity::SpacesdIdentity)),
            ..QemuConfig::default()
        });
        Self {
            container_cfg,
            container: tokio::sync::Mutex::new(None),
            qemu: Arc::new(qemu),
            lume: Arc::new(LumeRuntime::with_defaults()),
            microvm: Arc::new(MicroVmRuntime::new(MicroVmConfig {
                resolver: Some(cua_image::RootfsLayerResolver::shared()),
                ..MicroVmConfig::default()
            })),
            apple: Arc::new(ContainerizationRuntime::with_defaults()),
            pool: std::sync::OnceLock::new(),
            named_state: Default::default(),
            resolved: Default::default(),
        }
    }

    /// Reports what this machine's QEMU VMs use and may still use to the
    /// host's capacity (`HostNodeStatus.vm_*`, `on="auto"` placement), as
    /// [`cua_vmm::density`] measures it: shared snapshot memory counted
    /// once. Measured at most once a second. The daemon does, at start.
    #[cfg(feature = "spaces")]
    pub fn report_vm_capacity(self: &Arc<Self>) {
        let qemu = Arc::downgrade(&self.qemu);
        let overcommit = self.qemu.config().density.vcpu_overcommit as f32;
        type Cached = Option<(std::time::Instant, Option<cua_cluster::sysinfo::VmCapacity>)>;
        let cache: std::sync::Mutex<Cached> = std::sync::Mutex::new(None);
        cua_cluster::sysinfo::set_vm_capacity_report(overcommit, move || {
            let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, v)) = *c
                && at.elapsed() < std::time::Duration::from_secs(1)
            {
                return v;
            }
            let d = qemu.upgrade()?.density();
            let v = d.headroom_mb.map(|free| cua_cluster::sysinfo::VmCapacity {
                memory_free_mb: free,
                typical_mb: d.typical_vm_mb.unwrap_or(0),
                vcpu_overcommit: overcommit,
            });
            *c = Some((std::time::Instant::now(), v));
            v
        });
    }

    /// Turns on warm pools for the Spaces this runtime starts (`cua
    /// daemon` does, at start): adopts the members a previous daemon left,
    /// then keeps the pool in shape every [`POOL_TICK`]. Members are leased
    /// in `state`. With `pool.auto = off` only explicit pools run.
    pub fn enable_pool(
        self: &Arc<Self>,
        state: cua_sandbox_core::StateStore,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let settings = crate::pool::PoolSettings::load();
        let host = Arc::new(PoolHost {
            local: Arc::downgrade(self),
        });
        let pool = crate::pool::WarmPool::new(host, state, settings);
        self.pool.set(pool.clone()).ok()?;
        // `on="auto"` placement prefers a machine with a ready member of the
        // requested shape.
        #[cfg(feature = "spaces")]
        {
            let p = pool.clone();
            cua_cluster::node::set_warm_report(move || {
                let mut out = std::collections::HashMap::new();
                for r in p.ready_by_request() {
                    for (cpus, memory_mb) in r.sizes() {
                        let key = cua_cluster::placement::warm_key(
                            &r.image, &r.kind, &r.runtime, cpus, memory_mb,
                        );
                        *out.entry(key).or_insert(0) += r.ready;
                    }
                }
                out
            });
        }
        let me = Arc::downgrade(self);
        Some(tokio::spawn(async move {
            if let Some(local) = me.upgrade() {
                let kinds = [
                    BackendKind::Container,
                    BackendKind::Qemu,
                    BackendKind::Lume,
                    BackendKind::Firecracker,
                    BackendKind::Vz,
                    BackendKind::Containerization,
                ];
                let mut rts = HashMap::new();
                for k in kinds {
                    if let Ok(rt) = local.runtime(k).await {
                        rts.insert(k, rt);
                    }
                }
                drop(local);
                pool.adopt(|k| rts.get(&k).cloned()).await;
            }
            let mut ticks = 0u64;
            loop {
                tokio::select! {
                    _ = shutdown.changed() => return,
                    _ = tokio::time::sleep(POOL_TICK) => {}
                }
                let Some(local) = me.upgrade() else { return };
                ticks += 1;
                // Settings and `pool.warm` shapes are read every half
                // minute; the policy runs every tick.
                if ticks % 15 == 1 {
                    pool.set_settings(crate::pool::PoolSettings::load());
                    local.apply_pools().await;
                }
                drop(local);
                pool.tick().await;
            }
        }))
    }

    /// The warm pool, when on.
    pub fn pool(&self) -> Option<&crate::pool::WarmPool> {
        self.pool.get()
    }

    /// Applies the explicit pools: the shapes `pool.warm` lists (as a plain
    /// local create of each image would start them) and the named pools
    /// (`ApplyPool`). Pools no longer listed are withdrawn; their members
    /// drain at the next tick.
    pub async fn apply_pools(&self) {
        let Some(pool) = self.pool.get() else { return };
        use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes, placement::On};
        let mut sources = std::collections::HashSet::new();
        for (image, count) in pool.settings().warm {
            let mut o = CreateOptions::new(ProviderKind::Local, image.clone());
            if let Ok(r) = cua_sandbox_core::settings::Settings::load()
                .and_then(|s| s.resolve(Some(On::Local), None, None))
            {
                o.kind = r.kind;
                o.runtime = r.runtime;
            }
            if let Err(e) = o.apply_placement() {
                tracing::warn!(%image, error = %e, "pool.warm: cannot run this image here");
                continue;
            }
            let spec = Sandboxes::local_start_spec(&o, "warm", Some("warm"));
            match self
                .explicit(pool, &spec, &format!("warm:{image}"), count, None, None)
                .await
            {
                Ok(_) => {
                    sources.insert(format!("warm:{image}"));
                }
                Err(e) => tracing::warn!(%image, "pool.warm: {e}"),
            }
        }
        for np in pool.named_pools() {
            let source = format!("pool:{}", np.name);
            let outcome = async {
                let o = crate::runtime::local_options(np.request().map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                let spec = Sandboxes::local_start_spec(&o, "pool", Some("pool"));
                self.explicit(pool, &spec, &source, np.min, Some(np.max), np.idle())
                    .await
            }
            .await;
            let state = match outcome {
                Ok(key) => {
                    sources.insert(source.clone());
                    (Some(key), String::new())
                }
                Err(e) => (None, e),
            };
            self.named_state
                .lock()
                .unwrap()
                .insert(np.name.clone(), state);
        }
        pool.retain_explicit(|src| {
            !(src.starts_with("warm:") || src.starts_with("pool:")) || sources.contains(src)
        });
    }

    /// Keeps `min` (at most `max`) members of `spec`'s shape for explicit
    /// pool `source`; the shape key, or why it does not pool here.
    async fn explicit(
        &self,
        pool: &crate::pool::WarmPool,
        spec: &LocalStartSpec,
        source: &str,
        min: u32,
        max: Option<u32>,
        idle: Option<std::time::Duration>,
    ) -> std::result::Result<String, String> {
        let p = self.prepare(spec).await.map_err(|e| e.to_string())?;
        if p.injected {
            return Err(
                "an image without cua-spacesd cannot keep Spaces ready: its cua-spacesd is \
                 injected when a Space starts; use an image that carries cua-spacesd"
                    .to_string(),
            );
        }
        let spacesd = p.image.as_ref().and_then(|i| i.spacesd) == Some(true);
        let key = pool
            .set_explicit(
                source, &p.spec, p.backend, spacesd, p.rt, min, max, None, idle,
            )
            .ok_or_else(|| {
                "only images that run cua-spacesd, without sidecars, a GPU, a disk size or a \
                 build, keep Spaces ready"
                    .to_string()
            })?;
        pool.note_request(&p.spec, p.backend, spacesd, requested_words(spec));
        Ok(key)
    }

    /// A named pool's state: its shape key once applied, or why it could
    /// not be.
    pub fn named_pool_state(&self, name: &str) -> (Option<String>, String) {
        self.named_state
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or((None, "not applied yet".into()))
    }

    /// [`resolve_placement_for`], served from the last resolution of the
    /// same request on a warm-pool host (refreshed in the background when
    /// older than [`RESOLVE_REFRESH`]). Private-registry requests always
    /// ask the registry.
    async fn resolve_cached(
        &self,
        image: &str,
        os: &str,
        kind: cua_sandbox_core::placement::Kind,
        runtime: &cua_sandbox_core::placement::Runtime,
        creds: Option<&cua_image::RegistryCredentials>,
    ) -> Result<Resolved> {
        if self.pool.get().is_none() || creds.is_some() {
            return resolve_placement_for(image, os, kind, runtime, creds).await;
        }
        let key = (
            image.trim().to_string(),
            os.to_string(),
            kind.to_string(),
            runtime.to_string(),
        );
        let cached = self.resolved.lock().unwrap().get(&key).cloned();
        if let Some((at, r)) = cached {
            if at.elapsed() >= RESOLVE_REFRESH {
                let cache = self.resolved.clone();
                let (image, os, runtime) = (image.to_string(), os.to_string(), runtime.clone());
                tokio::spawn(async move {
                    if let Ok(r) = resolve_placement_for(&image, &os, kind, &runtime, None).await
                        && r.image.is_some()
                    {
                        cache
                            .lock()
                            .unwrap()
                            .insert(key, (std::time::Instant::now(), r));
                    }
                });
            }
            return Ok(r);
        }
        let r = resolve_placement_for(image, os, kind, runtime, None).await?;
        // Only what a registry answered (a syntactic fallback may be a
        // registry that was unreachable this once).
        if r.image.is_some() {
            self.resolved
                .lock()
                .unwrap()
                .insert(key, (std::time::Instant::now(), r.clone()));
        }
        Ok(r)
    }

    /// The microVM (Firecracker) backend.
    pub fn microvm(&self) -> &Arc<MicroVmRuntime> {
        &self.microvm
    }

    /// The engine local Linux Spaces use ([`managed::choose`]): this Mac's
    /// own, or the built-in runtime. Chosen on first use, without booting
    /// anything.
    async fn engine(&self) -> std::result::Result<Engine, VmmError> {
        let source = LinuxSource::current();
        let mut slot = self.container.lock().await;
        if let Some((s, e)) = slot.as_ref()
            && *s == source
        {
            return Ok(e.clone());
        }
        let e = match managed::choose(self.container_cfg.clone(), source).await? {
            managed::Choice::System(rt) => Engine::System(Arc::new(*rt)),
            managed::Choice::Builtin(m) => Engine::Builtin(m),
        };
        *slot = Some((source, e.clone()));
        Ok(e)
    }

    /// Stops the built-in Linux runtime VM now when no Linux Space or
    /// kept-warm member needs it, and says what it did. A stopping daemon
    /// calls this: the VM must not outlive it (it held host file
    /// descriptors and memory for nobody). Spaces still running keep it up;
    /// it stops when they do, or when the next daemon finds it idle
    /// ([`Self::stop_idle_linux_runtime`]).
    pub async fn stop_linux_runtime_now(&self) -> managed::RuntimeStop {
        if !managed::running() {
            return managed::RuntimeStop::NotRunning;
        }
        match self.engine().await {
            Ok(Engine::Builtin(m)) => m.stop_now().await,
            _ => managed::RuntimeStop::NotRunning,
        }
    }

    /// The container backend (connects to the engine on first use, and
    /// boots the built-in Linux runtime when that is the engine).
    pub async fn container(&self) -> std::result::Result<Arc<ContainerRuntime>, VmmError> {
        match self.engine().await? {
            Engine::System(rt) => Ok(rt),
            Engine::Builtin(m) => m.engine().await,
        }
    }

    /// The container backend when its engine is up; never boots the
    /// built-in Linux runtime.
    pub async fn container_if_up(&self) -> Option<Arc<ContainerRuntime>> {
        match self.engine().await.ok()? {
            Engine::System(rt) => Some(rt),
            Engine::Builtin(m) => m.engine_if_running().await,
        }
    }

    /// Stops the built-in Linux runtime VM (after its idle grace) when it
    /// runs but no Linux Space does, giving its memory back to the host.
    /// Deleting the last Space does this too; this covers a VM that a
    /// previous daemon left running with no Space in it, which nothing
    /// would delete again. Cheap when the VM is down.
    pub async fn stop_idle_linux_runtime(&self) {
        if !managed::running() {
            return;
        }
        if let Ok(Engine::Builtin(m)) = self.engine().await {
            m.stop_if_idle().await;
        }
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
            BackendKind::Container => match self.engine().await? {
                Engine::System(rt) => rt as Arc<dyn VmmRuntime>,
                Engine::Builtin(m) => m as Arc<dyn VmmRuntime>,
            },
            BackendKind::Lume => self.lume.clone() as Arc<dyn VmmRuntime>,
            BackendKind::Firecracker | BackendKind::Vz => {
                self.microvm.clone() as Arc<dyn VmmRuntime>
            }
            BackendKind::Containerization => self.apple.clone() as Arc<dyn VmmRuntime>,
            _ => self.qemu.clone() as Arc<dyn VmmRuntime>,
        })
    }

    /// The backends an instance may live on here, in lookup order.
    fn backends() -> Vec<BackendKind> {
        let mut v = vec![BackendKind::Container, BackendKind::Qemu];
        if cfg!(target_os = "macos") {
            v.push(BackendKind::Lume);
            v.push(BackendKind::Vz);
            v.push(BackendKind::Containerization);
        }
        if cfg!(target_os = "linux") {
            v.push(BackendKind::Firecracker);
        }
        v
    }

    /// The backend a persisted `runtime_type` names, if it names one.
    fn backend_of(runtime_type: &str) -> Option<BackendKind> {
        match runtime_type.trim().to_ascii_lowercase().as_str() {
            "qemu" => Some(BackendKind::Qemu),
            "lume" if cfg!(target_os = "macos") => Some(BackendKind::Lume),
            "firecracker" | "microvm" if cfg!(target_os = "linux") => {
                Some(BackendKind::Firecracker)
            }
            "containerization" if cfg!(target_os = "macos") => Some(BackendKind::Containerization),
            "vz" if cfg!(target_os = "macos") => Some(BackendKind::Vz),
            "gvisor" | "runsc" | "runc" | "container" | "docker" | "qemu-docker" => {
                Some(BackendKind::Container)
            }
            _ => None,
        }
    }

    /// The backend that owns `name`.
    /// Asks every backend at once (a slow engine does not hold up the
    /// others), in [`Self::backends`] order.
    async fn owner(&self, name: &str) -> RuntimeResult<(BackendKind, Arc<dyn VmmRuntime>)> {
        let asks = Self::backends().into_iter().map(|kind| async move {
            let rt = self.runtime(kind).await.ok()?;
            let status = rt.status(name).await.map(|_| ());
            Some((kind, status, rt))
        });
        let answers = futures_util::future::join_all(asks)
            .await
            .into_iter()
            .flatten()
            .collect();
        pick_owner(name, answers)
    }
}

/// How often the warm pool's policy runs (pre-warm windows open on time;
/// settings, memory and strays are checked every 15th tick).
pub const POOL_TICK: std::time::Duration = std::time::Duration::from_secs(2);

/// What the warm pool asks of this machine: cua-spacesd of its members, and
/// the memory they draw on.
struct PoolHost {
    local: std::sync::Weak<VmmLocal>,
}

/// A client for the cua-spacesd at `endpoints` with `token`: native gRPC,
/// no capability probe (the pool knows what it talks to).
async fn member_client(
    endpoints: &Endpoints,
    token: &str,
) -> std::result::Result<cua_spacesd_client::SpacesdClient, String> {
    let port = endpoints
        .ports
        .get(&cua_image::resolve::SPACESD_PORT)
        .copied()
        .ok_or("the member publishes no cua-spacesd port")?;
    let host = if endpoints.host.is_empty() {
        "127.0.0.1"
    } else {
        endpoints.host.as_str()
    };
    let opts = cua_spacesd_client::ConnectOptions::parse(&format!("http://{host}:{port}"))
        .map_err(|e| e.to_string())?
        .token(token)
        .transport(cua_spacesd_client::TransportPreference::Native)
        .probe(false);
    cua_spacesd_client::SpacesdClient::connect(opts)
        .await
        .map_err(|e| e.to_string())
}

#[async_trait]
impl crate::pool::PoolHost for PoolHost {
    async fn member_ready(
        &self,
        endpoints: &Endpoints,
        secret: &str,
        desktop: bool,
        timeout: std::time::Duration,
    ) -> std::result::Result<(), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut last = String::from("starting");
        loop {
            let attempt = async {
                let c = member_client(endpoints, secret).await?;
                c.metrics().await.map_err(|e| e.to_string())?;
                if desktop {
                    let ready = c.desktop_readiness().await.map_err(|e| e.to_string())?;
                    if !ready.ready {
                        return Err(format!("desktop: {}", ready.detail));
                    }
                }
                // The first command and screenshot of a fresh guest pay
                // for warming its caches: the member pays, not the Space.
                let out = c
                    .run(cua_spacesd_client::Command::shell("true"))
                    .await
                    .map_err(|e| e.to_string())?;
                if !out.success() {
                    return Err("the first command failed".into());
                }
                if desktop {
                    c.screenshot(Default::default())
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            };
            match tokio::time::timeout_at(deadline, attempt).await {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(e)) => last = e,
                Err(_) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "the member was not ready within {timeout:?}: {last}"
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    async fn rotate(
        &self,
        endpoints: &Endpoints,
        backend: BackendKind,
        secret: &str,
        token: &str,
    ) -> std::result::Result<(), String> {
        use cua_spacesd_client::{Error as E, pb};
        let request = |rotate: bool| {
            let mut entropy = vec![0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::rng(), &mut entropy);
            pb::InitRequest {
                token: if rotate {
                    token.to_string()
                } else {
                    String::new()
                },
                now: Some(crate::convert::ts(std::time::SystemTime::now())),
                new_machine: true,
                entropy,
                ..Default::default()
            }
        };
        let r = if claim_delivers_token(backend) {
            // The claim gave the guest the Space's token already: initialise
            // it with that token, never with the pool secret.
            let c = member_client(endpoints, token).await?;
            c.init(request(false)).await.map_err(|e| e.to_string())?
        } else {
            let c = member_client(endpoints, secret).await?;
            match c.init(request(true)).await {
                Ok(r) => r,
                // The backend delivered the Space's token its own way: the
                // guest is initialised with that token.
                Err(E::Unauthenticated(_) | E::PermissionDenied(_) | E::NotInitialized(_)) => {
                    let c = member_client(endpoints, token).await?;
                    c.init(request(false)).await.map_err(|e| e.to_string())?
                }
                Err(e) => return Err(e.to_string()),
            }
        };
        if !r.new_machine {
            // A cua-spacesd that predates `new_machine`: the token is the
            // Space's, but the guest keeps the per-machine secrets it booted
            // with (as a cold Space of that image does).
            tracing::info!(
                "this image's cua-spacesd predates new_machine; the claimed Space keeps its boot-time machine secrets"
            );
        }
        Ok(())
    }

    async fn memory(&self, backend: BackendKind) -> Option<crate::pool::Memory> {
        let host = host_memory();
        if backend != BackendKind::Container {
            return host;
        }
        let local = self.local.upgrade()?;
        let rt = local.container_if_up().await?;
        let total = rt.memory_total_mb().await?;
        let names = rt.running_containers().await.ok()?;
        let used: u64 = futures_util::future::join_all(names.iter().map(|n| rt.memory_used_mb(n)))
            .await
            .into_iter()
            .flatten()
            .sum();
        // An engine in a VM (the built-in runtime, Docker Desktop) has its
        // own memory; a Linux engine shares the host's.
        let engine = crate::pool::Memory {
            total_mb: total,
            available_mb: total.saturating_sub(used),
        };
        Some(match host {
            Some(h) if cfg!(target_os = "linux") => crate::pool::Memory {
                total_mb: engine.total_mb.min(h.total_mb),
                available_mb: engine.available_mb.min(h.available_mb),
            },
            _ => engine,
        })
    }

    async fn wake(&self, backend: BackendKind) {
        if backend == BackendKind::Container
            && let Some(local) = self.local.upgrade()
            && let Err(e) = local.container().await
        {
            tracing::warn!("pool.warm: the container engine did not start: {e}");
        }
    }

    fn on_battery(&self) -> bool {
        on_battery()
    }

    fn busy(&self) -> bool {
        cpu_saturated()
    }

    async fn members_present(&self, backend: BackendKind) -> Option<Vec<String>> {
        if backend != BackendKind::Container {
            return None;
        }
        let local = self.local.upgrade()?;
        let rt = local.container_if_up().await?;
        let members = rt.pool_members().await.ok()?;
        Some(members.into_iter().map(|m| m.name).collect())
    }

    async fn used_mb(&self, backend: BackendKind, name: &str) -> Option<u64> {
        if backend != BackendKind::Container {
            return None;
        }
        let local = self.local.upgrade()?;
        local.container_if_up().await?.memory_used_mb(name).await
    }
}

/// Whether the CPUs are saturated by other work: the one-minute load
/// average is at least 0.9 per CPU. Warm members the policy merely wants
/// do not start on such a machine.
#[cfg(unix)]
fn cpu_saturated() -> bool {
    let mut v = [0f64; 1];
    // SAFETY: an out array of one double and its length.
    let n = unsafe { libc::getloadavg(v.as_mut_ptr(), 1) };
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    n >= 1 && v[0] >= 0.9 * cpus
}

#[cfg(not(unix))]
fn cpu_saturated() -> bool {
    false
}

/// Whether this machine draws on its battery now (checked at most once a
/// minute): a Mac's `pmset -g ps`, Linux's `/sys/class/power_supply`.
fn on_battery() -> bool {
    use std::sync::Mutex;
    static LAST: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *last
        && at.elapsed() < std::time::Duration::from_secs(60)
    {
        return v;
    }
    let v = battery_now();
    *last = Some((std::time::Instant::now(), v));
    v
}

fn battery_now() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "ps"])
            .output()
            .ok()
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("'Battery Power'"))
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else {
            return false;
        };
        let mut battery_discharging = false;
        let mut mains_online = false;
        for e in rd.flatten() {
            let p = e.path();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default();
            match read("type").trim() {
                "Battery" => battery_discharging |= read("status").trim() == "Discharging",
                "Mains" => mains_online |= read("online").trim() == "1",
                _ => {}
            }
        }
        battery_discharging && !mains_online
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// This machine's memory, in MiB: `MemAvailable` on Linux; on a Mac the
/// share the kernel reports as available (`kern.memorystatus_level`).
fn host_memory() -> Option<crate::pool::Memory> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let field = |name: &str| -> Option<u64> {
            let line = text.lines().find(|l| l.starts_with(name))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        };
        Some(crate::pool::Memory {
            total_mb: field("MemTotal:")? / 1024,
            available_mb: field("MemAvailable:")? / 1024,
        })
    }
    #[cfg(target_os = "macos")]
    {
        let total = sysctl_u64(c"hw.memsize")? / (1024 * 1024);
        let level = sysctl_u64(c"kern.memorystatus_level")?.min(100);
        Some(crate::pool::Memory {
            total_mb: total,
            available_mb: total * level / 100,
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn sysctl_u64(name: &std::ffi::CStr) -> Option<u64> {
    let mut buf = [0u8; 8];
    let mut len = buf.len();
    // SAFETY: a NUL-terminated name, an out buffer and its length;
    // sysctlbyname writes at most `len` bytes and updates `len`.
    let r = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    match (r, len) {
        (0, 8) => Some(u64::from_ne_bytes(buf)),
        (0, 4) => Some(u64::from(u32::from_ne_bytes([
            buf[0], buf[1], buf[2], buf[3],
        ]))),
        _ => None,
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
        (BackendKind::Firecracker, _) => "firecracker".into(),
        (backend, _) => backend.as_str().to_string(),
    }
}

/// A start, resolved: what to start, on which backend.
struct Prepared {
    spec: StartSpec,
    backend: BackendKind,
    rt: Arc<dyn VmmRuntime>,
    image: Option<cua_image::ResolvedImage>,
    /// A static cua-spacesd is injected into the image (which has none).
    injected: bool,
}

/// Whether an image gets a static cua-spacesd injected: a Linux container
/// rootfs without one, run by a backend that boots OCI images directly
/// (containers and microVMs), unless the caller asked for it as it is.
/// A VM keeps needing a Cua disk image.
fn wants_injection(
    requested: Option<bool>,
    image: Option<&cua_image::ResolvedImage>,
    backend: BackendKind,
) -> bool {
    requested != Some(false)
        && matches!(
            backend,
            BackendKind::Container
                | BackendKind::Containerization
                | BackendKind::Firecracker
                | BackendKind::Vz
        )
        && image.is_some_and(|i| {
            i.os == "linux"
                && i.variant == cua_image::resolve::Variant::Rootfs
                && i.spacesd != Some(true)
        })
}

/// The image's info as the sandbox layer records it.
fn image_info_of(
    image: Option<&cua_image::ResolvedImage>,
    injected: bool,
) -> Option<cua_sandbox_core::ImageInfo> {
    image.map(|i| {
        let mut info: cua_sandbox_core::ImageInfo = i.into();
        info.injected = injected;
        info
    })
}

/// The image, kind and runtime a local start asked for, as given.
fn requested_words(spec: &LocalStartSpec) -> (String, String, String) {
    (
        spec.image.trim().to_string(),
        spec.kind.clone().unwrap_or_default(),
        spec.runtime.clone().unwrap_or_default(),
    )
}

/// A started [`cua_vmm::Instance`] as the sandbox layer sees it.
fn local_instance(inst: cua_vmm::Instance) -> LocalInstance {
    LocalInstance {
        backend: runtime_label(&inst),
        name: inst.name,
        status: status_of(inst.status),
        endpoints: endpoints_of(inst.endpoints),
    }
}

impl VmmLocal {
    /// Resolves `spec` (image placement, backend, runtime) into the
    /// [`StartSpec`] its backend starts.
    async fn prepare(&self, spec: &LocalStartSpec) -> RuntimeResult<Prepared> {
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
        let mut r = self
            .resolve_cached(
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
        if missing_fallback_allowed(
            kind,
            &runtime,
            backend_prefix(&spec.image),
            r.placement.backend,
            LinuxSource::current(),
        ) && let Err(e @ VmmError::Missing { .. }) = self.engine().await
        {
            let ready = doctor_report()
                .await
                .backend(BackendKind::Qemu)
                .is_some_and(|b| b.ready);
            r = vm_instead(
                &spec.image,
                &spec.os,
                spec.registry_credentials.as_ref(),
                ready,
            )
            .await
            .ok_or_else(|| rt_err(e))?;
        }
        let p = r.placement.clone();
        if p.backend == BackendKind::Firecracker && !cfg!(target_os = "linux") {
            return Err(RuntimeError::Unsupported {
                backend: "firecracker".into(),
                op: "a host other than Linux (microVMs need Linux with KVM)".into(),
            });
        }
        if p.backend == BackendKind::Vz && !cfg!(target_os = "macos") {
            return Err(RuntimeError::Unsupported {
                backend: "vz".into(),
                op: "a host other than a Mac (macOS microVMs need Virtualization.framework)".into(),
            });
        }
        if p.backend == BackendKind::Containerization && !cfg!(target_os = "macos") {
            return Err(RuntimeError::Unsupported {
                backend: "containerization".into(),
                op: "a host other than a Mac (Apple's Containerization needs macOS 26)".into(),
            });
        }
        if spec.disk_size_gb.is_some() && p.backend == BackendKind::Container {
            return Err(RuntimeError::Unsupported {
                backend: "container".into(),
                op: "a disk size (disk_gb sizes VM disks; a container uses its engine's)".into(),
            });
        }
        refuse_gpu(&spec.image, spec.gpu.is_some(), &r)?;
        // An image without cua-spacesd gets a static one, so any OCI image
        // is a full Space on the container and microVM kinds.
        let inject = if wants_injection(spec.spacesd, r.image.as_ref(), p.backend) {
            let arch = r.arch.unwrap_or_else(cua_vmm::Arch::host);
            Some(cua_vmm::inject::require_binary(arch).map_err(rt_err)?)
        } else {
            None
        };
        if inject.is_some()
            && let Some(i) = r.image.as_mut()
        {
            i.spacesd = Some(true);
        }
        // The machine checks (public's preflight) cover the container, QEMU
        // and Lume backends; microVMs and Apple Containerization refuse
        // above with their own reasons.
        if matches!(
            p.backend,
            BackendKind::Container | BackendKind::Qemu | BackendKind::Lume
        ) {
            preflight_refusal(os_of(&spec.os), p.backend).await?;
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
        let has_spacesd = r.image.as_ref().and_then(|i| i.spacesd) == Some(true);
        let injected = inject.is_some();
        s.inject_spacesd = inject;
        let own_command = spec.command.as_ref().is_some_and(|c| !c.is_empty());
        // A headless container or microVM runs cua-spacesd as its command
        // (an image's entrypoint starts its desktop first).
        let headless_command = spec.headless
            && has_spacesd
            && !injected
            && !own_command
            && matches!(
                p.backend,
                BackendKind::Container
                    | BackendKind::Containerization
                    | BackendKind::Firecracker
                    | BackendKind::Vz
            );
        // An injected cua-spacesd runs whatever the command is.
        let runs_spacesd = has_spacesd && (!own_command || headless_command || injected);
        if s.probes.is_empty() && runs_spacesd {
            // A microVM or VM Space is ready once cua-spacesd listens: that
            // is the point its warm snapshot is taken at (and a restore
            // waits for).
            if matches!(
                p.backend,
                BackendKind::Firecracker | BackendKind::Vz | BackendKind::Qemu
            ) {
                s = s.probe(cua_vmm::Probe::tcp(cua_image::resolve::SPACESD_PORT));
            }
            // Behind a port forward, once cua-spacesd answers through the
            // host-side endpoint the caller dials: a guest that listens is
            // not enough while the forward to it (Lima's for the built-in
            // Linux runtime, Docker's, slirp for QEMU) is still being set
            // up, and the first call would fail with a reset (3 of 64
            // gVisor creates at once, 4 of 64 QEMU VMs). A microVM is
            // dialed at its own address, with no forward in between; probing
            // it before its network is up cost every start about a second.
            if matches!(p.backend, BackendKind::Container | BackendKind::Qemu) {
                s = s.probe(cua_vmm::Probe::http(
                    cua_image::resolve::SPACESD_PORT,
                    cua_proto::metadata::HEALTH_PATH,
                ));
            }
        }
        if spec.ephemeral {
            s = s.label(cua_vmm::container::LABEL_EPHEMERAL, "true");
        }
        s.env = spec.env.clone();
        s.command = spec.command.clone().filter(|c| !c.is_empty());
        if spec.headless && has_spacesd {
            s.env.extend(headless_env());
            if headless_command {
                s.command = Some(headless_command_line());
            }
        }
        s.registry_auth = spec.registry_credentials.clone();
        s.container_runtime = spec.container_runtime.clone();
        s.restrict_network = spec.restrict_network;
        s.forkable = spec.forkable;
        s.egress = spec.egress.clone();
        s.disk_size_gb = spec.disk_size_gb;
        s.mounts = spec.mounts.clone();
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
                    "sidecar {:?} args/cpu/memory (fold args into command)",
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
        Ok(Prepared {
            spec: s,
            backend: p.backend,
            rt,
            image: r.image,
            injected,
        })
    }
}

#[async_trait]
impl LocalRuntime for VmmLocal {
    fn backend(&self) -> String {
        "vmm".into()
    }

    /// VMs nobody uses first (a lifetime that passed, an owner that is
    /// gone: [`crate::guard`]), then the warm pool's idle members for what
    /// is still short. A VM ended counts as the 2 CPUs and 4 GiB a Space
    /// reserves.
    async fn warm_held(&self) -> Vec<cua_sandbox_core::WarmHeld> {
        self.pool().map(|p| p.held()).unwrap_or_default()
    }

    async fn reclaim(&self, need: cua_sandbox_core::Shortfall) -> cua_sandbox_core::Reclaimed {
        let mut out = cua_sandbox_core::Reclaimed::default();
        let home = cua_home::cua_home();
        let ended = crate::guard::sweep(&home, true, &crate::guard::Policy::default(), false).await;
        let freed = ended.iter().filter(|e| e.error.is_none()).count() as u32;
        out.ended = ended
            .into_iter()
            .filter(|e| e.error.is_none())
            .map(|e| e.name)
            .collect();
        let need = cua_sandbox_core::Shortfall {
            millicpus: need.millicpus.saturating_sub(freed * 2000),
            memory_mb: need.memory_mb.saturating_sub(u64::from(freed) * 4096),
        };
        if (need.millicpus > 0 || need.memory_mb > 0)
            && let Some(pool) = self.pool()
        {
            out.warm_members = pool.reclaim(need).await;
        }
        out
    }

    async fn build_image(
        &self,
        spec: &cua_image::build::BuildSpec,
        creds: Option<&cua_image::RegistryCredentials>,
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
            .map(cua_image::build::local_build_file)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| RuntimeError::Other(e.to_string()))?;
        let recipe = cua_image::build::build_recipe(spec, &pinned, files, None);
        let tag = cua_image::build::build_name(&recipe);
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
            .unwrap_or(cua_image::build::DEFAULT_BUILD_TIMEOUT);
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
        match self.engine().await {
            Ok(Engine::System(rt)) => {
                out.push(rt.gpu_support_for("container").await);
                out.push(rt.gpu_support_for("gvisor").await);
            }
            // The built-in runtime's VM has no GPU (and is not booted to
            // say so).
            Ok(Engine::Builtin(_)) => {
                for r in ["container", "gvisor"] {
                    out.push(cua_vmm::gpu::container_support("macos", r, false));
                }
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
        let started = std::time::Instant::now();
        let Prepared {
            spec: s,
            backend,
            rt,
            image,
            injected,
        } = self.prepare(spec).await?;
        let resolved_ms = started.elapsed().as_millis() as u64;
        let spacesd = image.as_ref().and_then(|i| i.spacesd) == Some(true);
        // A warm pool keeps members of images that carry cua-spacesd: an
        // injected one is started for its Space. A warm member ran before
        // this Space had a policy: one that takes a policy at once (bound to
        // its running network, atomically) may be claimed; a QEMU VM would
        // have to move into its namespace, so it starts cold instead.
        let pool = self
            .pool
            .get()
            .filter(|_| spec.warm != Some(false) && !injected)
            .filter(|_| s.egress.is_empty() || backend != BackendKind::Qemu);
        if let Some(pool) = pool
            && let Some(inst) = pool.claim(&s, backend, spacesd).await
        {
            if !s.egress.is_empty()
                && let Err(e) = rt.set_egress(&inst.name, &s.egress).await
            {
                // Never hand out a Space without its policy.
                let _ = rt.delete(&inst.name).await;
                return Err(rt_err(e));
            }
            tracing::debug!(
                name = spec.name,
                backend = %backend,
                resolve_ms = resolved_ms,
                claim_ms = started.elapsed().as_millis() as u64 - resolved_ms,
                "local start (warm)"
            );
            pool.note_request(&s, backend, spacesd, requested_words(spec));
            return Ok((
                local_instance(inst),
                image_info_of(image.as_ref(), injected),
            ));
        }
        let start_at = std::time::Instant::now();
        let inst = rt.start(&s).await.map_err(rt_err)?;
        // Behind a port forward (the probe on cua-spacesd's port says so),
        // ready once it serves an authenticated call with the Space's
        // token: see `guest_identity::wait_accepts`.
        let spacesd_probe = s.probes.iter().any(|p| {
            matches!(p, cua_vmm::Probe::Http { port, .. } if *port == cua_image::resolve::SPACESD_PORT)
        });
        if spacesd_probe
            && let Some(token) = ["CUA_ENV_TOKEN", "CUA_SPACESD_TOKEN"]
                .iter()
                .find_map(|k| s.env.get(*k))
                .filter(|t| !t.is_empty())
        {
            // A failed create is cleaned up (or kept, `keep_on_failure`)
            // by the caller, as for a failed start.
            crate::guest_identity::wait_accepts(&inst.endpoints, token, s.ready_timeout)
                .await
                .map_err(rt_err)?;
        }
        tracing::debug!(
            name = spec.name,
            backend = %backend,
            resolve_ms = resolved_ms,
            start_ms = started.elapsed().as_millis() as u64 - resolved_ms,
            "local start"
        );
        if let Some(pool) = pool {
            pool.observe(
                &s,
                backend,
                spacesd,
                rt.clone(),
                started,
                start_at.elapsed().as_secs_f64(),
            );
            pool.note_request(&s, backend, spacesd, requested_words(spec));
        }
        // The start may have pulled an image or a base VM.
        crate::maintenance::spawn_auto_gc("start");
        Ok((
            local_instance(inst),
            image_info_of(image.as_ref(), injected),
        ))
    }

    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        let (_, rt) = self.owner(name).await?;
        rt.stop(name).await.map_err(rt_err)
    }

    async fn set_egress(
        &self,
        name: &str,
        policy: &cua_vmm::egress::EgressPolicy,
    ) -> RuntimeResult<cua_vmm::egress::Enforcement> {
        let (_, rt) = self.owner(name).await?;
        rt.set_egress(name, policy).await.map_err(rt_err)
    }

    async fn egress(&self, name: &str) -> RuntimeResult<cua_vmm::egress::EgressPolicy> {
        let (_, rt) = self.owner(name).await?;
        rt.egress(name).await.map_err(rt_err)
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

    async fn resize(&self, name: &str, shape: &cua_vmm::Resize) -> RuntimeResult<LocalInstance> {
        let (_, rt) = self.owner(name).await?;
        let inst = rt.resize(name, shape).await.map_err(rt_err)?;
        Ok(LocalInstance {
            backend: runtime_label(&inst),
            name: inst.name,
            status: status_of(inst.status),
            endpoints: endpoints_of(inst.endpoints),
        })
    }

    /// Containers pause in memory (`docker pause`) and QEMU VMs pause over
    /// QMP; Lume has no pause, so a Lume VM stops and boots again. On the
    /// built-in Linux runtime containers stop too, so its VM can stop once
    /// no Linux Space runs (a paused one would hold it up).
    fn power_control(&self, runtime_type: &str) -> Option<cua_sandbox_core::PowerControl> {
        use cua_sandbox_core::PowerControl;
        match runtime_type.trim().to_ascii_lowercase().as_str() {
            // Neither has a pause: the VM stops and boots again.
            "lume" | "containerization" => Some(PowerControl::Stop),
            "gvisor" | "runsc" | "runc" | "container" | "docker" if managed::in_use() => {
                Some(PowerControl::Stop)
            }
            "gvisor" | "runsc" | "runc" | "container" | "docker" | "qemu-docker" | "qemu"
            | "firecracker" | "vz" => Some(PowerControl::Suspend),
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

    async fn checkpoint_shared(&self, name: &str, checkpoint: &str) -> RuntimeResult<bool> {
        let (_, rt) = self.owner(name).await?;
        rt.checkpoint(name, checkpoint)
            .await
            .map(|info| info.shared_memory)
            .map_err(rt_err)
    }

    async fn pin_checkpoint(
        &self,
        runtime_type: &str,
        checkpoint: &str,
        pinned: bool,
    ) -> RuntimeResult<()> {
        let kind = Self::backend_of(runtime_type).ok_or_else(|| {
            RuntimeError::Other(format!("no local engine runs {runtime_type:?} sandboxes"))
        })?;
        let rt = self.runtime(kind).await.map_err(rt_err)?;
        match rt.pin_checkpoint(checkpoint, pinned).await {
            // An engine without pins of its own: the record's pin guards
            // delete and prune.
            Ok(()) | Err(VmmError::Unsupported { .. }) => Ok(()),
            Err(e) => Err(rt_err(e)),
        }
    }

    async fn fork_on(&self, runtime_type: &str, source: &str, new_name: &str) -> RuntimeResult<()> {
        let kind = Self::backend_of(runtime_type).ok_or_else(|| {
            RuntimeError::Other(format!("no local engine runs {runtime_type:?} sandboxes"))
        })?;
        let rt = self.runtime(kind).await.map_err(rt_err)?;
        rt.fork(source, new_name).await.map_err(rt_err)
    }

    async fn delete_checkpoint(&self, runtime_type: &str, checkpoint: &str) -> RuntimeResult<()> {
        let kind = Self::backend_of(runtime_type).ok_or_else(|| {
            RuntimeError::Other(format!("no local engine runs {runtime_type:?} sandboxes"))
        })?;
        let rt = self.runtime(kind).await.map_err(rt_err)?;
        rt.delete_checkpoint(checkpoint).await.map_err(rt_err)
    }

    async fn export_checkpoint_on(
        &self,
        runtime_type: &str,
        checkpoint: &str,
    ) -> RuntimeResult<cua_vmm::CheckpointExport> {
        let kind = Self::backend_of(runtime_type).ok_or_else(|| {
            RuntimeError::Other(format!("no local engine runs {runtime_type:?} sandboxes"))
        })?;
        let rt = self.runtime(kind).await.map_err(rt_err)?;
        rt.export_checkpoint(checkpoint).await.map_err(rt_err)
    }

    async fn import_dir_on(&self, runtime_type: &str, digest: &str) -> Option<std::path::PathBuf> {
        let kind = Self::backend_of(runtime_type)?;
        self.runtime(kind).await.ok()?.import_dir(digest)
    }

    async fn import_checkpoint_on(
        &self,
        runtime_type: &str,
        checkpoint: &str,
        state: serde_json::Value,
        qcow2_rebase: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    ) -> RuntimeResult<()> {
        let kind = Self::backend_of(runtime_type).ok_or_else(|| {
            RuntimeError::Other(format!("no local engine runs {runtime_type:?} sandboxes"))
        })?;
        let rt = self.runtime(kind).await.map_err(rt_err)?;
        rt.import_checkpoint(checkpoint, state, qcow2_rebase)
            .await
            .map_err(rt_err)
    }

    async fn snapshot_abis(&self) -> Vec<String> {
        // Only QEMU checkpoints are copied between hosts today.
        match self.runtime(BackendKind::Qemu).await {
            Ok(rt) => rt.snapshot_abis(),
            Err(_) => vec![],
        }
    }

    async fn start_fork(
        &self,
        runtime_type: &str,
        spec: &LocalStartSpec,
    ) -> RuntimeResult<LocalInstance> {
        let rt = match Self::backend_of(runtime_type) {
            Some(kind) => self.runtime(kind).await.map_err(rt_err)?,
            None => self.owner(&spec.name).await?.1,
        };
        let mut s = StartSpec::new(&spec.name, ImageSource::Existing)
            .cpus(spec.cpus.max(1))
            .memory_mb(spec.memory_mb.max(256))
            .ready_timeout(spec.ready_timeout);
        for p in &spec.ports {
            s = s.port(*p);
        }
        for pr in &spec.probes {
            s = s.probe(match &pr.http_path {
                Some(path) => cua_vmm::Probe::http(pr.port, path.clone()),
                None => cua_vmm::Probe::tcp(pr.port),
            });
        }
        s.env = spec.env.clone();
        s.forkable = spec.forkable;
        let inst = rt.start(&s).await.map_err(rt_err)?;
        Ok(LocalInstance {
            backend: runtime_label(&inst),
            name: inst.name,
            status: status_of(inst.status),
            endpoints: endpoints_of(inst.endpoints),
        })
    }

    /// A microVM fork gets its own token over vsock when it starts; a
    /// container fork is created again with its own token in its
    /// environment before its first start (a memory fork's restored
    /// cua-spacesd is given it with `Init`); a QEMU VM's through cua-spacesd
    /// when it restores its source's memory (a fork of a disk boots with
    /// the new token in its cloud-init seed).
    fn fork_has_own_identity(&self, runtime_type: &str) -> bool {
        matches!(
            Self::backend_of(runtime_type),
            Some(
                BackendKind::Firecracker
                    | BackendKind::Vz
                    | BackendKind::Container
                    | BackendKind::Qemu
            )
        )
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        let mut out = Vec::new();
        let mut kinds = vec![BackendKind::Container, BackendKind::Qemu];
        if cfg!(target_os = "linux") {
            kinds.push(BackendKind::Firecracker);
        }
        if cfg!(target_os = "macos") {
            kinds.push(BackendKind::Vz);
            kinds.push(BackendKind::Containerization);
        }
        for kind in kinds {
            let Ok(rt) = self.runtime(kind).await else {
                continue;
            };
            if let Ok(list) = rt.list().await {
                // Warm-pool members are not Spaces until claimed.
                out.extend(
                    list.into_iter()
                        .filter(|s| !cua_vmm::is_pool_member(&s.name))
                        .map(|s| LocalSummary {
                            name: s.name,
                            backend: s.backend.as_str().to_string(),
                            status: status_of(s.status),
                        }),
                );
            }
        }
        Ok(out)
    }

    /// A container's exit record (its code and `OOMKilled`), read from the
    /// engine only while it is up: the built-in Linux runtime is never
    /// booted for it. Other runtimes keep no such record.
    async fn stop_reason(
        &self,
        name: &str,
        runtime_type: &str,
    ) -> RuntimeResult<Option<cua_sandbox_core::StopReason>> {
        if !matches!(
            runtime_type.trim().to_ascii_lowercase().as_str(),
            "gvisor" | "runsc" | "runc" | "container" | "docker"
        ) {
            return Ok(None);
        }
        let Some(rt) = self.container_if_up().await else {
            return Ok(None);
        };
        Ok(rt
            .exit_of(name)
            .await
            .map_err(rt_err)?
            .map(|e| cua_sandbox_core::StopReason {
                exit_code: e.exit_code,
                out_of_memory: e.oom_killed,
            }))
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        let (_, rt) = self.owner(name).await?;
        rt.status(name).await.map(status_of).map_err(rt_err)
    }

    async fn status_on(&self, name: &str, runtime_type: &str) -> RuntimeResult<InstanceStatus> {
        // The engine the state file names; unknown words ask every engine.
        let kind = match Self::backend_of(runtime_type) {
            Some(k) => k,
            // Any engine may own it: not found only when every engine was
            // reachable and said so.
            None => {
                let mut unreachable = None;
                for kind in Self::backends() {
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
            // Run with the channel's Lume environment, so a Local Lume
            // attaches to a Local VM.
            open_command: cua_vmm::lume::lume_cmd()
                .map_err(rt_err)?
                .map(|l| l.argv(["attach", name])),
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
        // A microVM's init (or `container exec`) runs commands (no display
        // without cua-spacesd).
        if matches!(
            kind,
            BackendKind::Firecracker | BackendKind::Vz | BackendKind::Containerization
        ) && op == "exec"
        {
            return Ok(rt);
        }
        if kind != BackendKind::Lume {
            return Err(RuntimeError::Unsupported {
                backend: kind.as_str().into(),
                op: format!("{op} without cua-spacesd (Lume sandboxes only)"),
            });
        }
        Ok(rt)
    }
}

// ------------------------------------------------------------------ preflight

/// Refuses a create the machine cannot run, in plain words (the first
/// failed check of [`cua_vmm::preflight`]), before anything is pulled. The
/// disk is left to the guards of the create itself, which know the image's
/// real size.
async fn preflight_refusal(os: cua_vmm::GuestOs, backend: BackendKind) -> RuntimeResult<()> {
    use cua_create_failure::preflight::Runs;
    let runs = if backend == BackendKind::Container {
        Runs::Container
    } else {
        Runs::Vm
    };
    let req = cua_vmm::preflight::request(cua_vmm::preflight::guest_of(os), runs, None);
    let mut facts = cua_vmm::preflight::gather(&req).await;
    facts.free_bytes = None;
    match cua_create_failure::preflight::evaluate(&req, &facts)
        .into_iter()
        .next()
    {
        Some(issue) => Err(RuntimeError::Other(issue.message)),
        None => Ok(()),
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
                    BackendKind::Firecracker => r
                        .microvm
                        .as_ref()
                        .and_then(|m| m.firecracker_version.clone())
                        .unwrap_or_default(),
                    BackendKind::Containerization => r
                        .containerization
                        .as_ref()
                        .map(|c| c.version.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                },
                detail,
            }
        })
        .collect()
}

/// The `spacesd-static` row of `cua runtime doctor`: the static cua-spacesd
/// that containers and microVMs put into an OCI image that carries none.
/// Only a host with such a backend ready needs it.
pub fn static_spacesd_check(needed: bool) -> cua_proto::daemon::v1::RuntimeCheck {
    use cua_proto::daemon::v1::{CheckStatus, RuntimeCheck};
    let arch = cua_vmm::Arch::host();
    let (status, detail) = match cua_vmm::inject::require_binary(arch) {
        Ok(p) => (
            CheckStatus::Ok,
            format!("{} (images without cua-spacesd get it)", p.display()),
        ),
        Err(e) if needed => (CheckStatus::Error, e.to_string()),
        Err(_) => (
            CheckStatus::NotApplicable,
            "no container or microVM runtime is ready here".to_string(),
        ),
    };
    RuntimeCheck {
        name: "spacesd-static".into(),
        status: status as i32,
        version: String::new(),
        detail,
    }
}

/// How long a provider's `doctor` may take in `cua runtime doctor`.
const EXTERNAL_DOCTOR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// One `cua runtime doctor` row per installed external runtime provider
/// ([`cua_sandbox_core::external`]), from its `doctor` answer, and one per
/// `cua-runtime-*` executable that is ignored (with why). Empty when none
/// is installed.
pub async fn external_doctor(
    registry: &cua_sandbox_core::external::Registry,
) -> Vec<(cua_proto::daemon::v1::RuntimeCheck, serde_json::Value)> {
    use cua_proto::daemon::v1::{CheckStatus, RuntimeCheck};
    use cua_sandbox_core::external::ExternalRuntime;
    let found = registry.discover();
    let asks = found.providers.iter().cloned().map(|p| async move {
        let rt = ExternalRuntime::new(p.clone()).with_call_timeout(EXTERNAL_DOCTOR_TIMEOUT);
        let answer = rt.doctor().await;
        let m = &p.manifest;
        let (ready, mut detail, missing) = match answer {
            Ok(d) => (d.ready, d.detail, d.missing),
            Err(e) => (false, format!("doctor failed: {e}"), vec![]),
        };
        let json = serde_json::json!({
            "name": m.name,
            "display_name": m.label(),
            "kind": p.kind.as_str(),
            "path": p.path.display().to_string(),
            "os": m.os,
            "arch": m.arch,
            "images": m.images,
            "ready": ready,
            "detail": detail,
            "missing": missing,
        });
        if !missing.is_empty() {
            detail.push_str(&format!("; missing: {}", missing.join(", ")));
        }
        let detail = if detail.is_empty() {
            format!("{} ({})", m.label(), p.kind)
        } else {
            format!("{} ({}): {detail}", m.label(), p.kind)
        };
        let check = RuntimeCheck {
            name: m.name.clone(),
            status: if ready {
                CheckStatus::Ok
            } else {
                CheckStatus::Error
            } as i32,
            version: String::new(),
            detail,
        };
        (check, json)
    });
    let mut out = futures_util::future::join_all(asks).await;
    for s in &found.skipped {
        let name = format!(
            "{}{}",
            cua_sandbox_core::external::EXECUTABLE_PREFIX,
            s.name
        );
        out.push((
            RuntimeCheck {
                name: name.clone(),
                status: CheckStatus::Error as i32,
                version: String::new(),
                detail: format!("ignored: {}", s.reason),
            },
            serde_json::json!({
                "name": s.name,
                "path": s.path.display().to_string(),
                "ignored": s.reason,
            }),
        ));
    }
    out
}

/// `cua runtime doctor` as the daemon answers it: one check per built-in
/// backend, then one per external runtime provider. The JSON report is
/// the `cua-vmm` report, with an `external` list only when a provider is
/// installed.
pub async fn doctor_response() -> cua_proto::daemon::v1::DoctorResponse {
    let (report, external) = tokio::join!(
        doctor_report(),
        external_doctor(cua_sandbox_core::external::Registry::global())
    );
    let mut checks = doctor_checks(&report);
    let needs_static = [
        BackendKind::Container,
        BackendKind::Containerization,
        BackendKind::Firecracker,
        BackendKind::Vz,
    ]
    .into_iter()
    .any(|k| report.backend(k).is_some_and(|b| b.ready));
    checks.push(static_spacesd_check(needs_static));
    let mut json = serde_json::to_value(&report).unwrap_or_default();
    // On macOS: Local Network access as this process has it and which app
    // macOS holds responsible for it (`local_network`). This process is the
    // one that talks to the VMs (the daemon, or the SDK embedding it); a
    // CLI's own connect would test the terminal's grant instead (CUA-1807).
    if cfg!(target_os = "macos")
        && let Some(map) = json.as_object_mut()
    {
        let ln = cua_vmm::local_network::report(&cua_vmm::local_network::lume_base()).await;
        map.insert(
            "local_network".into(),
            serde_json::to_value(ln).unwrap_or_default(),
        );
    }
    if let Some(map) = json.as_object_mut() {
        // What would stop a create here, per Space the host can run, in
        // words the apps and the CLI show as they are.
        map.insert(
            "preflight".into(),
            serde_json::to_value(cua_vmm::preflight::report().await).unwrap_or_default(),
        );
    }
    if !external.is_empty() {
        let (rows, list): (Vec<_>, Vec<_>) = external.into_iter().unzip();
        checks.extend(rows);
        if let serde_json::Value::Object(o) = &mut json {
            o.insert("external".into(), serde_json::Value::Array(list));
        }
    }
    cua_proto::daemon::v1::DoctorResponse {
        checks,
        report_json: json.to_string(),
    }
}

/// `cua runtime setup`: provisions `components` (`qemu`, `lume`; `runsc`
/// reports its install plan). `dry_run` only reports. Every step is
/// returned; nothing happens implicitly.
pub async fn setup(
    components: &[String],
    dry_run: bool,
) -> Result<Vec<cua_proto::daemon::v1::SetupStep>> {
    setup_with(
        doctor_report().await,
        components,
        dry_run,
        cua_vmm::qemu::install_qemu,
        cua_vmm::lume::install_lume,
        kvm_step,
    )
    .await
}

async fn setup_with(
    report: DoctorReport,
    components: &[String],
    dry_run: bool,
    install_qemu: impl AsyncFn() -> cua_vmm::Result<()>,
    install_lume: impl AsyncFn() -> cua_vmm::Result<()>,
    kvm: impl AsyncFn(bool) -> Option<cua_proto::daemon::v1::SetupStep>,
) -> Result<Vec<cua_proto::daemon::v1::SetupStep>> {
    use cua_proto::daemon::v1::{CheckStatus, SetupStep};
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
            "firecracker" => Some(BackendKind::Firecracker),
            "microvm" | "vz" if cfg!(target_os = "macos") => Some(BackendKind::Vz),
            "microvm" => Some(BackendKind::Firecracker),
            "containerization" | "apple" => Some(BackendKind::Containerization),
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
            BackendKind::Qemu => install_qemu().await,
            BackendKind::Lume => install_lume().await,
            BackendKind::Firecracker => install_microvm().await,
            BackendKind::Vz => install_vz().await,
            BackendKind::Containerization => {
                ContainerizationRuntime::with_defaults()
                    .ensure_ready()
                    .await
            }
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
    let qemu_installed = report.qemu.qemu_img.is_some()
        && (report.qemu.binaries)
            .get(report.host.arch.qemu())
            .is_some_and(Option::is_some);
    let qemu_present = match steps.iter().find(|s| s.name == "qemu") {
        Some(s) => s.status != CheckStatus::Error as i32,
        None => components.is_empty() && qemu_installed,
    };
    if qemu_present {
        steps.extend(kvm(dry_run).await);
    }
    Ok(steps)
}

/// The guest environment of a headless Space: the image's entrypoint skips
/// its desktop (`CUA_HEADLESS`) and cua-spacesd serves no desktop services
/// and no QUIC media socket.
pub(crate) fn headless_env() -> [(String, String); 3] {
    [
        ("CUA_HEADLESS".into(), "1".into()),
        ("CUA_ENV_NO_DESKTOP".into(), "true".into()),
        ("CUA_ENV_QUIC_PORT".into(), "0".into()),
    ]
}

/// The command of a headless container or microVM: cua-spacesd alone, at
/// the path every Cua image installs it (`CUA_SPACESD_BIN` in the images'
/// start scripts). Its token comes from the environment, else from the
/// token files the images' start scripts read (a warm-pool member's secret
/// is written to `/etc/cua/env-token`, and a claim rewrites both).
pub(crate) fn headless_command_line() -> Vec<String> {
    vec![
        "/bin/sh".into(),
        "-c".into(),
        concat!(
            "if [ -z \"$CUA_ENV_TOKEN\" ]; then ",
            "for f in /run/cua/env-token /etc/cua/env-token; do ",
            "if [ -r \"$f\" ]; then CUA_ENV_TOKEN=$(tr -d '\\r\\n' < \"$f\"); export CUA_ENV_TOKEN; break; fi; ",
            "done; fi; ",
            "exec \"${CUA_SPACESD_BIN:-/usr/local/bin/cua-spacesd}\""
        )
        .into(),
    ]
}

/// `cua runtime setup firecracker`: the pinned Firecracker release into
/// `~/.cua/bin` and the guest kernel into the microVM cache.
/// macOS microVMs: the pinned guest kernel and gvproxy (`cua-vz` and the
/// guest init ship with cua).
async fn install_vz() -> std::result::Result<(), VmmError> {
    use cua_vmm::microvm::{assets, vz};
    if let Some(why) = vz::unsupported_reason() {
        return Err(VmmError::other(why));
    }
    if vz::vz_bin().is_none() || assets::init_bin().is_none() {
        return Err(VmmError::missing(
            "cua-vz and cua-microvm-init",
            "they ship next to `cua` (a cua release, or libs/cua/vz/build.sh and a static \
             aarch64-unknown-linux-musl cua-microvm-init in ~/.cua/bin)",
        ));
    }
    vz::ensure_gvproxy().await?;
    assets::ensure_kernel(&MicroVmConfig::default().root).await?;
    Ok(())
}

async fn install_microvm() -> std::result::Result<(), VmmError> {
    if cua_vmm::microvm::assets::firecracker_bin().is_none() {
        cua_vmm::microvm::assets::install_firecracker().await?;
    }
    cua_vmm::microvm::assets::ensure_kernel(&MicroVmConfig::default().root).await?;
    Ok(())
}

async fn kvm_step(dry_run: bool) -> Option<cua_proto::daemon::v1::SetupStep> {
    use cua_proto::daemon::v1::{CheckStatus, SetupStep};
    if cua_vmm::host::HostOs::current() != cua_vmm::host::HostOs::Linux {
        return None;
    }
    let denied = cua_vmm::host::kvm_usable()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied);
    if denied && !dry_run && !cua_vmm::host::sudo_available() {
        return Some(SetupStep {
            name: "kvm".into(),
            status: CheckStatus::Ok as i32,
            detail: "KVM access denied; using slow TCG emulation (sudo unavailable)".into(),
        });
    }
    let user = std::env::var("USER").ok();
    let run = async |program: &str, args: &[&str]| cua_vmm::host::run(program, args).await;
    let (status, detail) =
        match cua_vmm::qemu::grant_kvm(cua_vmm::host::kvm_usable, user, dry_run, run).await {
            Ok(detail) if dry_run && denied => (CheckStatus::Installable, detail),
            Ok(detail) => (CheckStatus::Ok, detail),
            Err(e) => (CheckStatus::Error, Some(e.to_string())),
        };
    detail.map(|detail| SetupStep {
        name: "kvm".into(),
        status: status as i32,
        detail,
    })
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
            let disk = cua_image::containerdisk::pull(&client, &cache, &r, arch.oci())
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

/// Whether `backend`'s `Runtime::claim` delivers the Space's token to the
/// guest itself (a microVM over vsock, a QEMU VM through its token file), so
/// the pool secret no longer opens it and is never presented to it.
fn claim_delivers_token(backend: BackendKind) -> bool {
    matches!(
        backend,
        BackendKind::Firecracker | BackendKind::Vz | BackendKind::Qemu
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_backends_whose_claim_delivers_the_token_skip_the_pool_secret() {
        assert!(claim_delivers_token(BackendKind::Firecracker));
        assert!(claim_delivers_token(BackendKind::Vz));
        assert!(claim_delivers_token(BackendKind::Qemu));
        assert!(!claim_delivers_token(BackendKind::Container));
        assert!(!claim_delivers_token(BackendKind::Lume));
        assert!(!claim_delivers_token(BackendKind::Containerization));
    }

    #[test]
    fn refuse_gpu_follows_the_catalog() {
        use super::{BackendKind, Placement, Resolved, RuntimeError, refuse_gpu};
        use cua_vmm::ImageSource;

        let digest = cua_image::catalog::entries()
            .iter()
            .find(|e| e.reference == "ghcr.io/trycua/macos:15")
            .and_then(|e| e.digest.clone())
            .expect("macos:15 digest");
        let resolved = |digest: Option<&str>| Resolved {
            placement: Placement {
                backend: BackendKind::Lume,
                image: ImageSource::oci("ignored"),
            },
            arch: None,
            image: digest.map(|d| cua_image::ResolvedImage {
                reference: "ghcr.io/trycua/macos".into(),
                variant_ref: "ghcr.io/trycua/macos:15".into(),
                pinned_ref: format!("ghcr.io/trycua/macos@{d}"),
                digest: d.into(),
                variant: cua_image::Variant::Lume,
                arch: Some("arm64".into()),
                architectures: vec!["arm64".into()],
                emulated: false,
                os: "macos".into(),
                spacesd: Some(false),
                layers: vec![],
            }),
        };
        let refused = RuntimeError::Unsupported {
            backend: "lume".into(),
            op: "GPU acceleration on macOS Sequoia 15".into(),
        };
        assert_eq!(
            refuse_gpu("ghcr.io/trycua/macos:15", true, &resolved(None)).unwrap_err(),
            refused
        );
        assert_eq!(
            refuse_gpu(
                &format!("ghcr.io/trycua/macos@{digest}"),
                true,
                &resolved(Some(&digest))
            )
            .unwrap_err(),
            refused
        );
        assert!(refuse_gpu("ghcr.io/trycua/macos:15", false, &resolved(None)).is_ok());
        assert!(refuse_gpu("ghcr.io/trycua/macos:26", true, &resolved(None)).is_ok());
    }

    #[test]
    fn an_engine_that_could_not_be_asked_is_not_reported_as_not_found() {
        use super::{BackendKind, RuntimeError, VmmError, pick_owner};
        let gone = || VmmError::missing("lume serve at http://127.0.0.1:7777", "not reachable");
        // `lume serve` exited: the Lume Space still exists, so the caller hears
        // why it could not be reached, not that it is gone.
        let r = pick_owner(
            "space",
            vec![
                (
                    BackendKind::Container,
                    Err(VmmError::NotFound("space".into())),
                    (),
                ),
                (
                    BackendKind::Qemu,
                    Err(VmmError::NotFound("space".into())),
                    (),
                ),
                (BackendKind::Lume, Err(gone()), ()),
            ],
        );
        match r {
            Err(RuntimeError::Other(why)) => assert!(why.contains("lume serve"), "{why}"),
            other => panic!("expected the Lume error, got {other:?}"),
        }
        // Every engine answered and none has it: not found.
        let r = pick_owner(
            "space",
            vec![
                (
                    BackendKind::Container,
                    Err(VmmError::NotFound("space".into())),
                    (),
                ),
                (
                    BackendKind::Lume,
                    Err(VmmError::NotFound("space".into())),
                    (),
                ),
            ],
        );
        assert!(matches!(r, Err(RuntimeError::NotFound(n)) if n == "space"));
        // An unreachable engine does not hide the one that owns it.
        let r = pick_owner(
            "space",
            vec![
                (BackendKind::Container, Err(gone()), ()),
                (BackendKind::Lume, Ok(()), ()),
            ],
        );
        assert!(matches!(r, Ok((BackendKind::Lume, ()))));
    }

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
        let vz = Isolation::Vm { accel: "vz".into() };
        assert_eq!(
            runtime_label(&inst(BackendKind::Containerization, vz)),
            "containerization"
        );
    }

    /// A Mac's microVMs live on Apple's Containerization: found by their
    /// label, and stopped (no pause) when idle.
    #[test]
    fn a_macs_microvms_are_containerization_spaces() {
        use cua_sandbox_core::{LocalRuntime, PowerControl};
        let local = VmmLocal::default();
        if cfg!(target_os = "macos") {
            assert_eq!(
                VmmLocal::backend_of("containerization"),
                Some(BackendKind::Containerization)
            );
            assert!(VmmLocal::backends().contains(&BackendKind::Containerization));
            assert!(!VmmLocal::backends().contains(&BackendKind::Firecracker));
        } else {
            assert_eq!(VmmLocal::backend_of("containerization"), None);
            assert!(!VmmLocal::backends().contains(&BackendKind::Containerization));
        }
        assert_eq!(
            local.power_control("containerization"),
            Some(PowerControl::Stop)
        );
        assert!(!local.fork_has_own_identity("containerization"));
        // A container fork is created again with its own token.
        assert!(local.fork_has_own_identity("gvisor"));
        assert!(local.fork_has_own_identity("runc"));
        // A QEMU fork gets its own through cua-spacesd (memory) or its seed.
        assert!(local.fork_has_own_identity("qemu"));
    }

    /// A Mac's microVMs under cua-vz: found by their label, paused when
    /// idle (in memory, like Firecracker's), and a fork restored from
    /// memory gets its own identity from the guest init.
    #[test]
    fn a_macs_vz_microvms_suspend_and_forks_get_their_own_identity() {
        use cua_sandbox_core::{LocalRuntime, PowerControl};
        let local = VmmLocal::default();
        if cfg!(target_os = "macos") {
            assert_eq!(VmmLocal::backend_of("vz"), Some(BackendKind::Vz));
            assert!(VmmLocal::backends().contains(&BackendKind::Vz));
            assert!(local.fork_has_own_identity("vz"));
        } else {
            assert_eq!(VmmLocal::backend_of("vz"), None);
            assert!(!VmmLocal::backends().contains(&BackendKind::Vz));
        }
        assert_eq!(local.power_control("vz"), Some(PowerControl::Suspend));
        assert_eq!(
            cua_sandbox_core::placement_of_backend("vz"),
            (
                cua_sandbox_core::placement::Kind::MicroVm,
                cua_sandbox_core::placement::Runtime::Vz
            )
        );
    }

    use super::*;

    #[test]
    fn missing_fallback_requires_auto_linux_source() {
        use cua_sandbox_core::placement::{Kind, Runtime};
        for source in [LinuxSource::Auto, LinuxSource::Builtin, LinuxSource::System] {
            assert_eq!(
                missing_fallback_allowed(
                    Kind::Auto,
                    &Runtime::Auto,
                    None,
                    BackendKind::Container,
                    source,
                ),
                source == LinuxSource::Auto,
            );
        }
    }

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

    /// Any OCI rootfs without cua-spacesd is injected one on the kinds that
    /// run OCI images directly; images that carry it, VMs and an explicit
    /// opt-out are left as they are.
    #[tokio::test]
    async fn an_image_without_spacesd_gets_one_on_containers_and_microvms() {
        use cua_sandbox_core::placement::{Kind, Runtime};
        let _turn = FAKE_REGISTRY.lock().await;
        let host = cua_vmm::Arch::host().oci();
        let mut reg = cua_image::testing::FakeRegistry::default();
        reg.index("docker.io/library/alpine:3.20", &[host], false, None);
        reg.index(
            "ghcr.io/acme/cua:1",
            &[host],
            false,
            Some(serde_json::json!({"ai.cua.spacesd": "true"})),
        );
        cua_image::resolve::set_source(Some(Arc::new(reg)));
        let resolve = |image: &'static str, kind: Kind| async move {
            resolve_placement_for(image, "linux", kind, &Runtime::Auto, None)
                .await
                .unwrap()
        };
        let plain = resolve("docker.io/library/alpine:3.20", Kind::Container).await;
        let cua = resolve("ghcr.io/acme/cua:1", Kind::Container).await;
        let microvm = resolve("docker.io/library/alpine:3.20", Kind::MicroVm).await;
        cua_image::resolve::set_source(None);

        let plain_image = plain.image.as_ref();
        assert_eq!(plain_image.unwrap().spacesd, Some(false));
        for backend in [
            BackendKind::Container,
            BackendKind::Containerization,
            BackendKind::Firecracker,
            BackendKind::Vz,
        ] {
            assert!(wants_injection(None, plain_image, backend), "{backend}");
            assert!(
                wants_injection(Some(true), plain_image, backend),
                "{backend}"
            );
            assert!(
                !wants_injection(Some(false), plain_image, backend),
                "spacesd=false keeps today's behaviour on {backend}"
            );
        }
        for backend in [BackendKind::Qemu, BackendKind::Lume] {
            assert!(!wants_injection(None, plain_image, backend), "{backend}");
        }
        assert!(!wants_injection(None, None, BackendKind::Container));
        assert!(
            !wants_injection(None, cua.image.as_ref(), BackendKind::Container),
            "an image that carries cua-spacesd is left alone"
        );
        assert!(matches!(
            microvm.placement.backend,
            BackendKind::Firecracker | BackendKind::Vz | BackendKind::Containerization
        ));
        assert!(wants_injection(
            None,
            microvm.image.as_ref(),
            microvm.placement.backend
        ));
    }

    /// A VM of a bare OCI image is refused with the way out.
    #[tokio::test]
    async fn a_vm_of_a_bare_oci_image_says_to_use_a_container_or_microvm() {
        use cua_sandbox_core::placement::{Kind, Runtime};
        let _turn = FAKE_REGISTRY.lock().await;
        let host = cua_vmm::Arch::host().oci();
        let mut reg = cua_image::testing::FakeRegistry::default();
        reg.index("docker.io/library/alpine:3.20", &[host], false, None);
        cua_image::resolve::set_source(Some(Arc::new(reg)));
        let vm = resolve_placement_for(
            "docker.io/library/alpine:3.20",
            "linux",
            Kind::Vm,
            &Runtime::Auto,
            None,
        )
        .await;
        cua_image::resolve::set_source(None);
        match vm.unwrap_err() {
            Error::InvalidPlacement { message, .. } => {
                assert!(message.contains("kind container or microvm"), "{message}");
                assert!(message.contains("cua build"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn doctor_reports_the_static_spacesd_where_a_runtime_needs_it() {
        use cua_proto::daemon::v1::CheckStatus;
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("cua-spacesd");
        // A minimal static ELF for the host's architecture.
        let mut elf = vec![0u8; 64 + 56];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        let machine: u16 = if cfg!(target_arch = "aarch64") {
            183
        } else {
            62
        };
        elf[18..20].copy_from_slice(&machine.to_le_bytes());
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        std::fs::write(&good, &elf).unwrap();
        // SAFETY: this is the only test of this crate that sets the variable.
        unsafe { std::env::set_var(cua_vmm::inject::ENV_BINARY, &good) };
        let ok = static_spacesd_check(true);
        assert_eq!(
            (ok.name.as_str(), ok.status),
            ("spacesd-static", CheckStatus::Ok as i32)
        );
        assert!(ok.detail.contains("cua-spacesd"), "{}", ok.detail);
        // A dynamic or foreign file is no use: an error where it is needed.
        std::fs::write(dir.path().join("bad"), b"not an elf").unwrap();
        unsafe {
            std::env::set_var(cua_vmm::inject::ENV_BINARY, dir.path().join("bad"));
        }
        assert_eq!(static_spacesd_check(true).status, CheckStatus::Error as i32);
        assert_eq!(
            static_spacesd_check(false).status,
            CheckStatus::NotApplicable as i32,
            "nothing to inject into on this host"
        );
        unsafe { std::env::remove_var(cua_vmm::inject::ENV_BINARY) };
    }

    #[test]
    fn the_vm_hint_is_only_for_rootfs_only_images_asked_for_a_vm() {
        use cua_image::resolve::Backend;
        let rootfs = vec!["rootfs".to_string()];
        assert!(vm_hint(Backend::Vm, &rootfs).contains("microvm"));
        assert_eq!(vm_hint(Backend::Container, &rootfs), "");
        assert_eq!(vm_hint(Backend::Vm, &["containerdisk".to_string()]), "");
        assert_eq!(vm_hint(Backend::Vm, &[]), "");
    }

    #[tokio::test]
    async fn automatic_vm_fallback_requires_ready_verified_disk() {
        let _turn = FAKE_REGISTRY.lock().await;
        let host = cua_vmm::Arch::host();
        let mut reg = cua_image::testing::FakeRegistry::default();
        reg.index("ghcr.io/acme/app:1", &[host.oci()], false, None);
        reg.index("ghcr.io/acme/app:1-disk", &[host.oci()], true, None);
        reg.index("ghcr.io/acme/rootfs:1", &[host.oci()], false, None);
        cua_image::resolve::set_source(Some(Arc::new(reg)));
        let disk = vm_instead("ghcr.io/acme/app:1", "linux", None, true).await;
        let rootfs = vm_instead("ghcr.io/acme/rootfs:1", "linux", None, true).await;
        let missing = vm_instead("ghcr.io/acme/missing:1", "linux", None, true).await;
        cua_image::resolve::set_source(None);
        let disk = disk.unwrap();
        assert_eq!(disk.placement.backend, BackendKind::Qemu);
        assert_eq!(disk.arch, Some(host));
        assert_eq!(
            disk.image.as_ref().unwrap().variant,
            cua_image::resolve::Variant::Containerdisk
        );
        assert!(
            matches!(&disk.placement.image, ImageSource::Oci { reference } if reference.contains("app@sha256"))
        );
        assert!(rootfs.is_none());
        assert!(missing.is_none());
        assert!(vm_instead("invalid", "linux", None, false).await.is_none());
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

    fn qemu_report(ready: bool) -> cua_vmm::auto::DoctorReport {
        use cua_vmm::auto::{BackendStatus, DoctorReport, HostReport};
        DoctorReport {
            host: HostReport {
                os: cua_vmm::host::HostOs::Linux,
                arch: cua_vmm::Arch::X86_64,
                kvm: false,
                accel: Default::default(),
            },
            backends: vec![BackendStatus {
                backend: super::BackendKind::Qemu,
                ready,
                provisionable: true,
                detail: String::new(),
                missing: vec![],
                provisioning: vec!["apt-get install qemu".into()],
            }],
            qemu: Default::default(),
            lume: Default::default(),
            container: Default::default(),
            containerization: Default::default(),
            microvm: Default::default(),
            vz: Default::default(),
        }
    }

    #[test]
    fn doctor_preserves_backend_contract() {
        use cua_proto::daemon::v1::CheckStatus;
        let mut report = qemu_report(false);
        assert_eq!(
            super::doctor_checks(&report)[0].status,
            CheckStatus::Installable as i32
        );
        report.backends[0].provisionable = false;
        assert_eq!(
            super::doctor_checks(&report)[0].status,
            CheckStatus::Error as i32
        );
    }

    fn kvm_step() -> Option<cua_proto::daemon::v1::SetupStep> {
        Some(cua_proto::daemon::v1::SetupStep {
            name: "kvm".into(),
            status: cua_proto::daemon::v1::CheckStatus::Ok as i32,
            detail: "added".into(),
        })
    }

    async fn setup_steps(
        report: cua_vmm::auto::DoctorReport,
        components: &[&str],
        dry_run: bool,
        installs: bool,
    ) -> Vec<(String, cua_proto::daemon::v1::CheckStatus)> {
        let components: Vec<String> = components.iter().map(|c| c.to_string()).collect();
        let install = async || match installs {
            true => Ok(()),
            false => Err(super::VmmError::PermissionDenied("sudo".into())),
        };
        let kvm = async |_| kvm_step();
        super::setup_with(report, &components, dry_run, &install, &install, kvm)
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.name, s.status.try_into().unwrap()))
            .collect()
    }

    fn steps(
        steps: &[(&str, cua_proto::daemon::v1::CheckStatus)],
    ) -> Vec<(String, cua_proto::daemon::v1::CheckStatus)> {
        steps.iter().map(|(n, s)| (n.to_string(), *s)).collect()
    }

    #[tokio::test]
    async fn the_kvm_step_follows_qemu_once_it_is_present() {
        use cua_proto::daemon::v1::CheckStatus::{Installable, Ok};
        assert_eq!(
            setup_steps(qemu_report(false), &[], false, true).await,
            steps(&[("qemu", Ok), ("kvm", Ok)])
        );
        assert_eq!(
            setup_steps(qemu_report(true), &["qemu"], false, true).await,
            steps(&[("qemu", Ok), ("kvm", Ok)])
        );
        assert_eq!(
            setup_steps(qemu_report(false), &["qemu"], true, true).await,
            steps(&[("qemu", Installable), ("kvm", Ok)])
        );
    }

    #[tokio::test]
    async fn a_failed_qemu_install_has_no_kvm_step() {
        use cua_proto::daemon::v1::CheckStatus::Error;
        assert_eq!(
            setup_steps(qemu_report(false), &["qemu"], false, false).await,
            steps(&[("qemu", Error)])
        );
    }

    #[tokio::test]
    async fn components_without_qemu_have_no_kvm_step() {
        use cua_proto::daemon::v1::CheckStatus::Error;
        assert_eq!(
            setup_steps(qemu_report(false), &["lume"], false, false).await,
            steps(&[("lume", Error)])
        );
        assert!(
            setup_steps(qemu_report(true), &[], false, true)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn installed_qemu_denied_kvm_still_gets_a_kvm_step() {
        use cua_proto::daemon::v1::CheckStatus::Ok;
        let mut report = qemu_report(false);
        report.backends[0].provisionable = false;
        report.qemu.qemu_img = Some("/usr/bin/qemu-img".into());
        let system = Some("/usr/bin/qemu-system-x86_64".into());
        report.qemu.binaries.insert("x86_64".into(), system);
        assert_eq!(
            setup_steps(report, &[], false, true).await,
            steps(&[("kvm", Ok)])
        );
    }
}

#[cfg(all(test, unix))]
mod external_doctor_tests {
    use super::external_doctor;
    use cua_proto::daemon::v1::CheckStatus;
    use cua_sandbox_core::external::Registry;
    use std::os::unix::fs::PermissionsExt;

    fn install(dir: &std::path::Path, name: &str, doctor: &str) {
        let exe = dir.join(format!("cua-runtime-{name}"));
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\ncase \"$1\" in\n\
             info) echo '{{\"protocol\":1,\"name\":\"{name}\",\"display_name\":\"macOS VMs\",\"kind\":\"vm\"}}';;\n\
             doctor) echo '{doctor}';;\nesac\n"
        );
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[tokio::test]
    async fn doctor_has_a_row_per_provider_and_none_without() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::with_dirs(vec![dir.path().into()]);
        assert!(external_doctor(&reg).await.is_empty());

        install(dir.path(), "acme", r#"{"ready":true,"detail":"up"}"#);
        install(
            dir.path(),
            "beta",
            r#"{"ready":false,"detail":"not set up","missing":["service"]}"#,
        );
        install(dir.path(), "qemu", r#"{"ready":true}"#);
        let mut rows = external_doctor(&reg).await;
        rows.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        let names: Vec<&str> = rows.iter().map(|r| r.0.name.as_str()).collect();
        assert_eq!(names, ["acme", "beta", "cua-runtime-qemu"]);
        assert_eq!(rows[0].0.status, CheckStatus::Ok as i32);
        assert_eq!(rows[0].0.detail, "macOS VMs (vm): up");
        assert_eq!(rows[1].0.status, CheckStatus::Error as i32);
        assert_eq!(
            rows[1].0.detail,
            "macOS VMs (vm): not set up; missing: service"
        );
        assert_eq!(rows[1].1["missing"][0], "service");
        assert_eq!(rows[2].0.status, CheckStatus::Error as i32);
        assert!(
            rows[2].0.detail.starts_with("ignored: "),
            "{}",
            rows[2].0.detail
        );
    }
}
