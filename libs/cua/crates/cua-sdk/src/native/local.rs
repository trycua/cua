//! `Local`: local runtimes (cua-vmm: container/gVisor, QEMU, Lume) and
//! images (cua-image). Local *sandboxes* are created with
//! `Sandboxes.create(provider = Local, image = …)`; the image string picks
//! the backend (`container:<ref>` or a bare OCI ref → container/gVisor,
//! `vm:<ref>` / `disk:<path>` → QEMU, `lume:<ref>` or `os = macos` → Lume).

use super::{Backend, run};
use crate::{CuaError, Result};

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
    /// Inspects local runtimes (read-only: never installs or starts
    /// anything).
    pub async fn doctor(&self) -> Result<DoctorReport> {
        let backend = self.backend.clone();
        run(async move {
            let r = match &backend {
                Backend::Embedded(_) => {
                    let report = cua_daemon::local::doctor_report().await;
                    cua_proto::daemon::v1::DoctorResponse {
                        checks: cua_daemon::local::doctor_checks(&report),
                        report_json: serde_json::to_string(&report)?,
                    }
                }
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
