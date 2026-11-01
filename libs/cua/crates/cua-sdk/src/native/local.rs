//! `Local`: local runtimes (cua-vmm: container/gVisor, QEMU, Lume) and
//! images (cua-image). Local *sandboxes* are created with
//! `Sandboxes.create(provider = Local, image = …)`; the image string picks
//! the backend (`container:<ref>` or a bare OCI ref → container/gVisor,
//! `vm:<ref>` / `disk:<path>` → QEMU, `lume:<ref>` or `os = macos` → Lume).

use super::{Backend, run};
use crate::{CuaError, Result};
use cua_sandbox_core::runtime_kinds as kinds;

/// Status of one local runtime component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeCheckStatus {
    /// Present and usable.
    Ok,
    /// Missing; `setup` can provision it.
    Installable,
    /// Missing or broken; needs user action.
    Error,
    /// Not used on this host.
    NotApplicable,
    /// Not reported.
    Unknown,
}

/// One doctor check (one backend).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeCheck {
    /// Backend (`container`, `qemu`, `lume`, ...).
    pub name: String,
    /// Status.
    pub status: RuntimeCheckStatus,
    /// Version, when detected.
    pub version: Option<String>,
    /// Explanation, missing pieces and what `setup` would do.
    pub detail: String,
}

fn status_of(s: i32) -> RuntimeCheckStatus {
    use cua_proto::daemon::v1::CheckStatus as S;
    match S::try_from(s).unwrap_or_default() {
        S::Ok => RuntimeCheckStatus::Ok,
        S::Installable => RuntimeCheckStatus::Installable,
        S::Error => RuntimeCheckStatus::Error,
        S::NotApplicable => RuntimeCheckStatus::NotApplicable,
        S::Unspecified => RuntimeCheckStatus::Unknown,
    }
}

impl From<cua_proto::daemon::v1::RuntimeCheck> for RuntimeCheck {
    fn from(c: cua_proto::daemon::v1::RuntimeCheck) -> Self {
        RuntimeCheck {
            name: c.name,
            status: status_of(c.status),
            version: (!c.version.is_empty()).then_some(c.version),
            detail: c.detail,
        }
    }
}

/// One setup step.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SetupStep {
    /// Component.
    pub name: String,
    /// Status after the step.
    pub status: RuntimeCheckStatus,
    /// What was (or would be) done.
    pub detail: String,
}

/// Doctor output.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DoctorReport {
    /// One check per backend.
    pub checks: Vec<RuntimeCheck>,
    /// The full cua-vmm host report as JSON.
    pub report_json: String,
}

/// A local image.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LocalImage {
    /// Reference.
    pub reference: String,
    /// `container` or `vm`.
    pub kind: String,
    /// Disk path (VM) or engine image reference (container).
    pub location: String,
    /// Size in bytes, when known.
    pub size_bytes: u64,
}

/// Space on the volume a local engine writes to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StorageVolume {
    /// Bytes available to this user.
    pub available_bytes: u64,
    /// Volume size in bytes.
    pub total_bytes: u64,
    /// What a person calls it: `Macintosh HD`, `Colima`, `Docker Desktop`.
    pub name: String,
}

/// What local Spaces have room for (see [`Local::storage`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LocalStorage {
    /// This host's architecture (`arm64`, `amd64`).
    pub host_arch: String,
    /// Bytes cua keeps free on a volume: a create that would leave less
    /// fails (`CUA_DISK_MIN_FREE`, default 5 GiB).
    pub reserve_bytes: u64,
    /// Where Lume writes macOS VMs.
    pub lume: Option<StorageVolume>,
    /// Where QEMU writes VM disks and the containerDisk cache.
    pub qemu: Option<StorageVolume>,
    /// The container engine's data disk (Colima, Docker Desktop, OrbStack,
    /// `/var/lib/docker`), when its free space can be read.
    pub container: Option<StorageVolume>,
    /// Catalog image refs already pulled here at their published digest.
    pub pulled: Vec<String>,
    pub linux_runtime_download_bytes: u64,
}

impl From<cua_daemon::storage::StorageReport> for LocalStorage {
    fn from(r: cua_daemon::storage::StorageReport) -> Self {
        let vol = |v: Option<cua_daemon::storage::Volume>| {
            v.map(|v| StorageVolume {
                available_bytes: v.available,
                total_bytes: v.total,
                name: v.name,
            })
        };
        LocalStorage {
            host_arch: r.host_arch,
            reserve_bytes: r.reserve,
            lume: vol(r.lume),
            qemu: vol(r.qemu),
            container: vol(r.container),
            pulled: r.pulled,
            linux_runtime_download_bytes: r.linux_runtime_download_bytes,
        }
    }
}

/// A runtime installed on this machine for one kind of Space.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeDetected {
    /// `docker`, `podman`, `lume`, `qemu`, `container`, `firecracker`.
    pub tool: String,
    /// As people know it (`Docker`, `Apple container`).
    pub name: String,
    pub path: String,
    /// As `--version` says it (`27.3.1`).
    pub version: Option<String>,
    /// The guest system it runs, when only one (`macOS`, `Linux`).
    pub guests: Option<String>,
}

/// What a binary is, for one kind of Space (`runtime_check_path`).
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct RuntimePathProbe {
    pub path: String,
    /// A file is there.
    pub exists: bool,
    /// It can be run.
    pub executable: bool,
    /// The tool it is, when it is one the kind takes.
    pub tool: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    /// The oldest version cua runs on.
    pub min_version: Option<String>,
    /// The names of the tools the kind takes here (`Docker`, `Podman`).
    pub expected: Vec<String>,
    pub guests: Option<String>,
}

/// One kind of Space's runtime, as Settings shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeKindState {
    /// `container`, `vm` or `microvm`.
    pub kind: String,
    /// `runtime.<kind>`: `auto`, `builtin` or `system`.
    pub source: String,
    /// `runtime.<kind>_path`, when set.
    pub path: Option<String>,
    /// What is installed here for the kind, the one used first.
    pub detected: Vec<RuntimeDetected>,
    /// The path in use (the custom one, else the first detected), probed.
    pub check: Option<RuntimePathProbe>,
    /// The names of the tools the kind takes here (`Docker`, `Podman`).
    pub expected: Vec<String>,
}

impl From<kinds::PathProbe> for RuntimePathProbe {
    fn from(p: kinds::PathProbe) -> Self {
        Self {
            path: p.path.display().to_string(),
            exists: p.exists,
            executable: p.executable,
            tool: p.tool,
            name: p.name,
            version: p.version,
            min_version: p.min_version,
            expected: p.expected,
            guests: p.guests,
        }
    }
}

impl From<kinds::KindReport> for RuntimeKindState {
    fn from(r: kinds::KindReport) -> Self {
        Self {
            kind: r.kind.as_str().into(),
            source: r.source.as_str().into(),
            path: r.path.map(|p| p.display().to_string()),
            detected: r
                .detected
                .into_iter()
                .map(|d| RuntimeDetected {
                    tool: d.tool,
                    name: d.name,
                    path: d.path.display().to_string(),
                    version: d.version,
                    guests: d.guests,
                })
                .collect(),
            check: r.check.map(Into::into),
            expected: r.expected,
        }
    }
}

/// Local runtimes and images.
#[derive(uniffi::Object)]
pub struct Local {
    pub(crate) backend: Backend,
}

#[uniffi::export]
impl Local {
    /// Each kind of Space (`container`, `vm`, `microvm`) with its runtime
    /// setting (`runtime.<kind>`), the runtimes installed on this machine
    /// for it and what the path in use is. Read-only: runs each runtime's
    /// `--version`, nothing else; probed in this process (the daemon runs
    /// on the same host).
    pub async fn runtime_kinds(&self) -> Result<Vec<RuntimeKindState>> {
        run(async { Ok(kinds::report().await.into_iter().map(Into::into).collect()) }).await
    }

    /// What `path` is for `kind` (`container`, `vm`, `microvm`): there,
    /// runnable, which runtime and its version. Settings checks a typed path
    /// with it before it is saved (`runtime.<kind>_path`).
    pub async fn runtime_check_path(&self, kind: String, path: String) -> Result<RuntimePathProbe> {
        let k = kinds::RuntimeKind::parse(&kind).ok_or_else(|| {
            CuaError::InvalidArgument(format!(
                "kind: expected container, vm or microvm, got {kind:?}"
            ))
        })?;
        let p = match path.trim().strip_prefix("~/") {
            Some(rest) => {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest)
            }
            None => std::path::PathBuf::from(path.trim()),
        };
        run(async move { Ok(kinds::probe(k, &p).await.into()) }).await
    }

    /// Inspects local runtimes (read-only: never installs or starts
    /// anything).
    pub async fn doctor(&self) -> Result<DoctorReport> {
        let backend = self.backend.clone();
        run(async move {
            let r = match &backend {
                Backend::Embedded(_) => cua_daemon::local::doctor_response().await,
                Backend::Daemon(d) => d.doctor().await?,
            };
            Ok(DoctorReport {
                checks: r.checks.into_iter().map(Into::into).collect(),
                report_json: r.report_json,
            })
        })
        .await
    }

    /// Free space where local Spaces are written (Lume's VM storage, the
    /// cua home for QEMU, the container engine's data disk) and which
    /// catalog images are already pulled. Read-only and bounded; probed in
    /// this process (the daemon runs on the same host).
    pub async fn storage(&self) -> Result<LocalStorage> {
        run(async move { Ok(cua_daemon::storage::storage_report().await.into()) }).await
    }

    /// Provisions `components` (`qemu`, `lume`; `runsc` reports its plan).
    /// Empty = everything provisionable. `dry_run` only reports.
    pub async fn setup(&self, components: Vec<String>, dry_run: bool) -> Result<Vec<SetupStep>> {
        let backend = self.backend.clone();
        run(async move {
            let steps = match &backend {
                Backend::Embedded(_) => cua_daemon::local::setup(&components, dry_run).await?,
                Backend::Daemon(d) => d.setup(components, dry_run).await?,
            };
            Ok(steps
                .into_iter()
                .map(|s| SetupStep {
                    name: s.name,
                    status: status_of(s.status),
                    detail: s.detail,
                })
                .collect())
        })
        .await
    }

    /// Pulls an image (container engine for container refs, the image cache
    /// for `vm:` containerDisks).
    pub async fn pull_image(&self, reference: String) -> Result<LocalImage> {
        let backend = self.backend.clone();
        run(async move {
            Ok(match &backend {
                Backend::Embedded(rt) => {
                    let i = cua_daemon::local::pull_image(rt.vmm()?, &reference).await?;
                    LocalImage {
                        reference: i.reference,
                        kind: i.kind,
                        location: i.location,
                        size_bytes: i.size_bytes,
                    }
                }
                Backend::Daemon(d) => {
                    let i = d.pull_image(&reference).await?;
                    LocalImage {
                        reference: i.reference,
                        kind: i.kind,
                        location: i.location,
                        size_bytes: i.size_bytes,
                    }
                }
            })
        })
        .await
    }

    /// Builds an image from an `images.cua.ai/v1alpha1` Image resource
    /// (JSON) on `base`; pushes to `push` when set. Returns
    /// `reference@digest` or the local output directory.
    pub async fn build_image(
        &self,
        spec_json: String,
        base: String,
        push: Option<String>,
    ) -> Result<String> {
        let backend = self.backend.clone();
        run(async move {
            match &backend {
                Backend::Embedded(rt) => {
                    let out = cua_daemon::cua_home()
                        .join("build")
                        .join(format!("out-{}", std::process::id()));
                    Ok(
                        cua_daemon::local::build_image(rt.vmm()?, &spec_json, &base, &out, push)
                            .await?,
                    )
                }
                Backend::Daemon(d) => Ok(d.build_image(&spec_json, &base, push).await?),
            }
        })
        .await
    }

    /// Copies an image (host architecture) to `destination`; returns the
    /// pushed manifest digest.
    pub async fn push_image(&self, reference: String, destination: String) -> Result<String> {
        let backend = self.backend.clone();
        run(async move {
            match &backend {
                Backend::Embedded(_) => {
                    Ok(cua_daemon::local::push_image(&reference, &destination).await?)
                }
                Backend::Daemon(d) => Ok(d.push_image(&reference, &destination).await?),
            }
        })
        .await
    }
}

impl From<CuaError> for cua_daemon::Error {
    fn from(e: CuaError) -> Self {
        cua_daemon::Error::Internal(e.to_string())
    }
}
