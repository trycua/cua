//! Backend auto-selection and `doctor`.
//!
//! Port of `_auto_runtime` (`cua_sandbox/sandbox.py`) extended with the
//! zero-pre-setup rules from the consolidation plan (§1.1):
//!
//! * **VM images** — macOS guest → Lume (macOS/Apple Silicon hosts). Linux
//!   and Windows guests → QEMU (KVM on Linux, HVF on macOS for same-arch
//!   guests, WHPX on Windows, TCG otherwise). QEMU is installed on demand only
//!   when permitted.
//! * **Container / gVisor images** — a Docker-API engine with `runsc` → use
//!   it; engine without `runsc` → install it into the engine VM when permitted,
//!   else run with `runc` and report `isolation: runc`; no engine at all → the
//!   managed `cua-runtime` VM ([`crate::managed`]).
//!
//! [`doctor`] reports all of this as structured data (serialisable to JSON)
//! for `cua runtime doctor`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VmmError};
use crate::host::{self, HostOs};
use crate::types::{Arch, BackendKind, GuestOs};

/// What kind of image is being run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageKind {
    /// A bootable disk (containerDisk, cloud image, Lume VM image, ISO).
    Vm,
    /// An OCI rootfs image run as a container (gVisor preferred).
    Container,
}

/// Input to [`select`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SelectRequest {
    pub kind: ImageKind,
    pub os: GuestOs,
    /// Guest arch; `None` = host arch.
    pub arch: Option<Arch>,
}

/// Output of [`select`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub backend: BackendKind,
    /// Why this backend was chosen (and any caveat, e.g. TCG is slow).
    pub reason: String,
    /// Steps the SDK will run before first use (install runsc, start lume serve…).
    pub provisioning: Vec<String>,
}

/// Status of one backend on this host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendStatus {
    pub backend: BackendKind,
    /// Usable right now without provisioning.
    pub ready: bool,
    /// Usable after the listed provisioning steps.
    pub provisionable: bool,
    pub detail: String,
    #[serde(default)]
    pub missing: Vec<String>,
    #[serde(default)]
    pub provisioning: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostReport {
    pub os: HostOs,
    pub arch: Arch,
    pub kvm: bool,
    /// Accelerator QEMU would use per guest arch.
    pub accel: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QemuReport {
    /// `qemu-system-<arch>` per guest arch.
    pub binaries: BTreeMap<String, Option<PathBuf>>,
    pub qemu_img: Option<PathBuf>,
    pub version: Option<String>,
    /// UEFI firmware code file per guest arch.
    pub uefi: BTreeMap<String, Option<PathBuf>>,
    pub ssh_keygen: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LumeReport {
    pub supported_host: bool,
    pub binary: Option<PathBuf>,
    pub url: String,
    pub serving: bool,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerReport {
    pub endpoint: Option<String>,
    pub endpoint_source: Option<String>,
    pub engine: Option<String>,
    pub reachable: bool,
    pub runtimes: Vec<String>,
    pub gvisor: bool,
    /// How runsc would be installed, or why it cannot be.
    pub gvisor_provisioning: Option<String>,
}

/// Structured `cua runtime doctor` output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub host: HostReport,
    pub backends: Vec<BackendStatus>,
    pub qemu: QemuReport,
    pub lume: LumeReport,
    pub container: ContainerReport,
}

impl DoctorReport {
    pub fn backend(&self, kind: BackendKind) -> Option<&BackendStatus> {
        self.backends.iter().find(|b| b.backend == kind)
    }
}

/// Inspect the host. Read-only: never installs or starts anything.
pub async fn doctor() -> DoctorReport {
    let host_os = HostOs::current();
    let host_arch = Arch::host();
    let mut accel = BTreeMap::new();
    for a in [Arch::Aarch64, Arch::X86_64] {
        accel.insert(a.qemu().to_string(), host::qemu_accel(a).to_string());
    }
    let host = HostReport {
        os: host_os,
        arch: host_arch,
        kvm: host::kvm_usable(),
        accel,
    };

    // QEMU.
    let mut qemu = QemuReport::default();
    for a in [Arch::Aarch64, Arch::X86_64] {
        let bin = host::which(&format!("qemu-system-{}", a.qemu()));
        #[cfg(feature = "qemu")]
        let fw = bin
            .as_ref()
            .and_then(|b| crate::qemu::firmware::find_uefi(a, b))
            .map(|u| u.code);
        #[cfg(not(feature = "qemu"))]
        let fw: Option<PathBuf> = None;
        qemu.uefi.insert(a.qemu().to_string(), fw);
        qemu.binaries.insert(a.qemu().to_string(), bin);
    }
    qemu.qemu_img = host::which("qemu-img");
    qemu.ssh_keygen = host::which("ssh-keygen");
    if let Some(b) = qemu.binaries.values().flatten().next() {
        qemu.version = host::run(b, &["--version"])
            .await
            .ok()
            .and_then(|s| s.lines().next().map(str::to_string));
    }
    let native = host_arch.qemu().to_string();
    let qemu_ready =
        qemu.binaries.get(&native).is_some_and(Option::is_some) && qemu.qemu_img.is_some();
    let mut qemu_status = BackendStatus {
        backend: BackendKind::Qemu,
        ready: qemu_ready,
        provisionable: false,
        detail: qemu
            .version
            .clone()
            .unwrap_or_else(|| "QEMU not found".into()),
        missing: vec![],
        provisioning: vec![],
    };
    if !qemu_ready {
        qemu_status
            .missing
            .push(format!("qemu-system-{native} / qemu-img"));
        let step = match host_os {
            HostOs::Macos if host::which("brew").is_some() => Some("brew install qemu"),
            HostOs::Linux if host::which("apt-get").is_some() => {
                Some("sudo apt-get install qemu-system qemu-utils")
            }
            HostOs::Linux if host::which("dnf").is_some() => {
                Some("sudo dnf install qemu-kvm qemu-img")
            }
            _ => None,
        };
        if let Some(s) = step {
            qemu_status.provisionable = true;
            qemu_status.provisioning.push(s.to_string());
        }
    }

    // Lume.
    let mut lume = LumeReport {
        supported_host: host_os == HostOs::Macos && host_arch == Arch::Aarch64,
        binary: host::which("lume"),
        url: std::env::var("LUME_API").unwrap_or_else(|_| "http://127.0.0.1:7777".into()),
        ..Default::default()
    };
    #[cfg(feature = "lume")]
    if lume.supported_host {
        let c = crate::lume::LumeClient::new(lume.url.clone());
        lume.serving = c.reachable().await;
        if let Some(b) = &lume.binary {
            lume.version = host::run(b, &["--version"])
                .await
                .ok()
                .map(|s| s.trim().to_string());
        }
    }
    let lume_status = BackendStatus {
        backend: BackendKind::Lume,
        ready: lume.supported_host && lume.serving,
        provisionable: lume.supported_host && !lume.serving,
        detail: if !lume.supported_host {
            "Lume needs a macOS Apple Silicon host".into()
        } else if lume.serving {
            format!("lume serve answering at {}", lume.url)
        } else if lume.binary.is_some() {
            "lume installed but not serving".into()
        } else {
            "lume not installed".into()
        },
        missing: if lume.supported_host && lume.binary.is_none() {
            vec!["lume".into()]
        } else {
            vec![]
        },
        provisioning: if !lume.supported_host || lume.serving {
            vec![]
        } else if lume.binary.is_some() {
            vec!["start `lume serve`".into()]
        } else {
            vec![
                "run the Lume installer (allow_install)".into(),
                "start `lume serve`".into(),
            ]
        },
    };

    // Containers.
    let mut container = ContainerReport::default();
    let mut container_status = BackendStatus {
        backend: BackendKind::Container,
        ready: false,
        provisionable: false,
        detail: "no container engine found".into(),
        missing: vec![],
        provisioning: vec![],
    };
    #[cfg(feature = "container")]
    {
        use crate::container::{ContainerConfig, ContainerRuntime, engine, gvisor};
        if let Some(ep) = engine::discover() {
            container.endpoint = Some(ep.uri.clone());
            container.endpoint_source = Some(ep.source.clone());
            container.engine = Some(format!("{:?}", ep.kind));
            let plan = gvisor::install_plan(&ep.kind);
            container.gvisor_provisioning = Some(match &plan {
                Ok(p) => p.description.clone(),
                Err(h) => h.clone(),
            });
            match ContainerRuntime::connect(ContainerConfig {
                endpoint: Some(ep.uri.clone()),
                ..Default::default()
            })
            .await
            {
                Ok(rt) => {
                    container.reachable = true;
                    container.runtimes = rt.runtimes().await.unwrap_or_default();
                    container.gvisor = container.runtimes.iter().any(|r| r == "runsc");
                    container_status.ready = true;
                    container_status.detail = if container.gvisor {
                        format!("{} with gVisor (runsc)", ep.uri)
                    } else {
                        format!(
                            "{} without runsc: containers run with runc unless runsc is installed",
                            ep.uri
                        )
                    };
                    if !container.gvisor {
                        container_status.missing.push("runsc".into());
                        if let Ok(p) = plan {
                            container_status.provisionable = true;
                            container_status.provisioning.push(p.description);
                        }
                    }
                }
                Err(e) => {
                    container_status.detail = e.to_string();
                    container_status
                        .missing
                        .push("a running container engine".into());
                }
            }
        } else {
            container_status
                .missing
                .push("a Docker-API container engine".into());
        }
    }
    if !container_status.ready {
        container_status.provisioning.push(
            "boot the managed cua-runtime VM (containerd + runsc) — see cua_vmm::managed".into(),
        );
    }

    let managed_status = BackendStatus {
        backend: BackendKind::Managed,
        ready: false,
        provisionable: qemu_status.ready || lume_status.ready,
        detail: crate::managed::STATUS.into(),
        missing: vec![],
        provisioning: vec![],
    };

    DoctorReport {
        host,
        backends: vec![lume_status, qemu_status, container_status, managed_status],
        qemu,
        lume,
        container,
    }
}

/// Choose a backend for `req` given a [`doctor`] report.
pub fn select(req: &SelectRequest, report: &DoctorReport) -> Result<Selection> {
    let host = &report.host;
    let arch = req.arch.unwrap_or(host.arch);
    let status = |k| report.backend(k).cloned();
    match req.kind {
        ImageKind::Container => {
            let c = status(BackendKind::Container).expect("doctor reports container");
            if c.ready {
                let iso = if report.container.gvisor {
                    "gVisor"
                } else {
                    "runc (runsc not installed)"
                };
                return Ok(Selection {
                    backend: BackendKind::Container,
                    reason: format!("container engine available; isolation: {iso}"),
                    provisioning: c.provisioning,
                });
            }
            Err(VmmError::missing(
                "a container engine",
                format!(
                    "{}. The managed cua-runtime VM is not available yet; install Colima (`brew install colima docker && colima start`) \
                     or Docker Engine",
                    c.detail
                ),
            ))
        }
        ImageKind::Vm => match req.os {
            GuestOs::Macos => {
                let l = status(BackendKind::Lume).expect("doctor reports lume");
                if !report.lume.supported_host {
                    return Err(VmmError::missing(
                        "Lume",
                        "macOS guests need a macOS Apple Silicon host",
                    ));
                }
                if l.ready || l.provisionable {
                    return Ok(Selection {
                        backend: BackendKind::Lume,
                        reason: "macOS guest on Apple Silicon: Virtualization.framework via lume"
                            .into(),
                        provisioning: l.provisioning,
                    });
                }
                Err(VmmError::missing("lume", l.detail))
            }
            GuestOs::Linux | GuestOs::Windows => {
                let q = status(BackendKind::Qemu).expect("doctor reports qemu");
                let accel = host
                    .accel
                    .get(arch.qemu())
                    .cloned()
                    .unwrap_or_else(|| "tcg".into());
                if q.ready || q.provisionable {
                    let mut reason = format!("{:?} {arch} guest: QEMU with {accel}", req.os);
                    if accel == "tcg" {
                        reason.push_str(" (software emulation, slow; prefer an image for the host architecture)");
                    }
                    return Ok(Selection {
                        backend: BackendKind::Qemu,
                        reason,
                        provisioning: q.provisioning,
                    });
                }
                // Linux arm64 guests can also run under Lume on Apple Silicon.
                let l = status(BackendKind::Lume).expect("doctor reports lume");
                if req.os == GuestOs::Linux && arch == Arch::Aarch64 && (l.ready || l.provisionable)
                {
                    return Ok(Selection {
                        backend: BackendKind::Lume,
                        reason: "Linux arm64 guest via lume (QEMU unavailable)".into(),
                        provisioning: l.provisioning,
                    });
                }
                Err(VmmError::missing("QEMU", q.missing.join(", ")))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(
        qemu_ready: bool,
        lume_ready: bool,
        engine: bool,
        gvisor: bool,
        host_os: HostOs,
    ) -> DoctorReport {
        let mut accel = BTreeMap::new();
        accel.insert("aarch64".into(), "hvf".into());
        accel.insert("x86_64".into(), "tcg".into());
        let st = |backend, ready: bool| BackendStatus {
            backend,
            ready,
            provisionable: false,
            detail: String::new(),
            missing: vec!["x".into()],
            provisioning: vec![],
        };
        DoctorReport {
            host: HostReport {
                os: host_os,
                arch: Arch::Aarch64,
                kvm: false,
                accel,
            },
            backends: vec![
                st(BackendKind::Lume, lume_ready),
                st(BackendKind::Qemu, qemu_ready),
                st(BackendKind::Container, engine),
                st(BackendKind::Managed, false),
            ],
            qemu: QemuReport::default(),
            lume: LumeReport {
                supported_host: host_os == HostOs::Macos,
                serving: lume_ready,
                ..Default::default()
            },
            container: ContainerReport {
                gvisor,
                reachable: engine,
                ..Default::default()
            },
        }
    }

    fn req(kind: ImageKind, os: GuestOs, arch: Option<Arch>) -> SelectRequest {
        SelectRequest { kind, os, arch }
    }

    #[test]
    fn macos_guests_go_to_lume() {
        let r = report(true, true, true, true, HostOs::Macos);
        assert_eq!(
            select(&req(ImageKind::Vm, GuestOs::Macos, None), &r)
                .unwrap()
                .backend,
            BackendKind::Lume
        );
        let r = report(true, false, true, true, HostOs::Linux);
        assert!(select(&req(ImageKind::Vm, GuestOs::Macos, None), &r).is_err());
    }

    #[test]
    fn linux_vms_prefer_qemu_and_flag_tcg() {
        let r = report(true, true, true, true, HostOs::Macos);
        let s = select(&req(ImageKind::Vm, GuestOs::Linux, None), &r).unwrap();
        assert_eq!(s.backend, BackendKind::Qemu);
        assert!(s.reason.contains("hvf"));
        let s = select(&req(ImageKind::Vm, GuestOs::Linux, Some(Arch::X86_64)), &r).unwrap();
        assert!(s.reason.contains("slow"));
    }

    #[test]
    fn linux_arm64_falls_back_to_lume_without_qemu() {
        let r = report(false, true, false, false, HostOs::Macos);
        assert_eq!(
            select(&req(ImageKind::Vm, GuestOs::Linux, None), &r)
                .unwrap()
                .backend,
            BackendKind::Lume
        );
        assert!(select(&req(ImageKind::Vm, GuestOs::Linux, Some(Arch::X86_64)), &r).is_err());
    }

    #[test]
    fn containers_report_isolation() {
        let r = report(true, true, true, false, HostOs::Macos);
        let s = select(&req(ImageKind::Container, GuestOs::Linux, None), &r).unwrap();
        assert!(s.reason.contains("runc"));
        let r = report(true, true, false, false, HostOs::Macos);
        assert!(select(&req(ImageKind::Container, GuestOs::Linux, None), &r).is_err());
    }

    /// The report `cua runtime doctor --json` prints round-trips.
    #[test]
    fn doctor_report_serialises() {
        let r = report(true, false, true, false, HostOs::Macos);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["backends"].as_array().unwrap().len(), 4);
        assert_eq!(json["backends"][0]["backend"], serde_json::json!("lume"));
        let back: DoctorReport = serde_json::from_value(json).unwrap();
        assert_eq!(back, r);
    }

    #[tokio::test]
    #[ignore = "host: probes the developer's lume, qemu and Docker; run with --ignored"]
    async fn doctor_runs_on_this_host_and_serialises() {
        let r = doctor().await;
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["backends"].as_array().unwrap().len(), 4);
        eprintln!("{}", serde_json::to_string_pretty(&r).unwrap());
    }
}
