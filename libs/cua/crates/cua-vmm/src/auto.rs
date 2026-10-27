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

impl QemuReport {
    fn installed(&self, arch: Arch) -> bool {
        self.binaries.get(arch.qemu()).is_some_and(Option::is_some) && self.qemu_img.is_some()
    }
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
    /// Whether runsc is registered with `--allow-suid` (setuid, so `sudo`
    /// and `fusermount3`, work for the Space user); `None` when unknown.
    #[serde(default)]
    pub gvisor_allow_suid: Option<bool>,
}

/// A host check for `cua runtime doctor`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MicroVmReport {
    /// `/dev/kvm` opens read-write.
    pub kvm: bool,
    pub firecracker: Option<PathBuf>,
    pub firecracker_version: Option<String>,
    pub init: Option<PathBuf>,
    pub pasta: Option<PathBuf>,
    pub sqfstar: Option<PathBuf>,
    #[serde(default)]
    pub debugfs: Option<PathBuf>,
    /// The guest init is statically linked (a dynamic one cannot boot).
    #[serde(default)]
    pub init_static: bool,
    pub mkfs_ext4: Option<PathBuf>,
    pub iptables: Option<PathBuf>,
    pub ip: Option<PathBuf>,
    /// The guest kernel is cached (it is downloaded on first use).
    pub kernel_cached: bool,
    /// Unprivileged user namespaces are allowed (pasta needs them).
    pub userns: bool,
}

impl MicroVmReport {
    /// What is missing, as words for the doctor.
    pub fn missing(&self) -> Vec<String> {
        let mut m = Vec::new();
        if !self.kvm {
            m.push("/dev/kvm (read-write)".into());
        }
        if self.firecracker.is_none() {
            m.push("firecracker".into());
        }
        if self.init.is_none() {
            m.push("cua-microvm-init".into());
        } else if !self.init_static {
            m.push(
                "a static cua-microvm-init (this one is dynamically linked; build it with \
                 --target x86_64-unknown-linux-musl)"
                    .into(),
            );
        }
        for (what, have) in [
            ("pasta (passt)", &self.pasta),
            ("debugfs (e2fsprogs)", &self.debugfs),
            ("mkfs.ext4 (e2fsprogs)", &self.mkfs_ext4),
            ("iptables", &self.iptables),
            ("ip (iproute2)", &self.ip),
        ] {
            if have.is_none() {
                m.push(what.into());
            }
        }
        if std::env::var("CUA_MICROVM_BASE").as_deref() == Ok("squashfs") && self.sqfstar.is_none()
        {
            m.push("sqfstar (squashfs-tools)".into());
        }
        if !self.userns {
            m.push("unprivileged user namespaces".into());
        }
        m
    }
}

/// microVMs on a Mac under cua's own monitor (Virtualization.framework,
/// `cua-vz`) for `cua runtime doctor`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VzReport {
    /// An Apple silicon Mac on macOS 14+ (or why not).
    pub supported_host: bool,
    #[serde(default)]
    pub unsupported: Option<String>,
    /// `cua-vz`, which ships next to `cua`.
    pub monitor: Option<PathBuf>,
    /// The guest init (shipped next to `cua`), and whether it is static.
    pub init: Option<PathBuf>,
    #[serde(default)]
    pub init_static: bool,
    /// The network gateway (`cua-netgw`), which ships next to `cua`.
    #[serde(default)]
    pub gateway: Option<PathBuf>,
    /// The guest kernel is cached (downloaded on first use).
    pub kernel_cached: bool,
}

impl VzReport {
    /// What is missing that first use cannot download.
    pub fn missing(&self) -> Vec<String> {
        let mut m = Vec::new();
        if self.monitor.is_none() {
            m.push("cua-vz (ships next to `cua`)".into());
        }
        if self.init.is_none() {
            m.push("cua-microvm-init (ships next to `cua`)".into());
        } else if !self.init_static {
            m.push("a static cua-microvm-init (this one is dynamically linked)".into());
        }
        if self.gateway.is_none() {
            m.push("cua-netgw, the network gateway (ships next to `cua`)".into());
        }
        m
    }
}

/// microVMs on a Mac (Apple's Containerization) for `cua runtime doctor`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerizationReport {
    /// An Apple silicon Mac on macOS 26+.
    pub supported_host: bool,
    pub macos_version: Option<String>,
    /// cua's own `container` binary, when installed.
    pub binary: Option<PathBuf>,
    /// The pinned version cua installs.
    pub version: String,
    /// `running` (cua's own services), `stopped`, or `other` (another
    /// install of `container` runs the per-user services).
    pub services: String,
    /// The other install's application root, when `services` is `other`.
    #[serde(default)]
    pub other_app_root: Option<String>,
}

/// Structured `cua runtime doctor` output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub host: HostReport,
    pub backends: Vec<BackendStatus>,
    pub qemu: QemuReport,
    pub lume: LumeReport,
    pub container: ContainerReport,
    /// Firecracker microVMs (Linux hosts).
    #[serde(default)]
    pub microvm: Option<MicroVmReport>,
    /// Apple's Containerization microVMs (Mac hosts).
    #[serde(default)]
    pub containerization: Option<ContainerizationReport>,
    /// microVMs under cua's own monitor (Mac hosts).
    #[serde(default)]
    pub vz: Option<VzReport>,
}

impl DoctorReport {
    pub fn backend(&self, kind: BackendKind) -> Option<&BackendStatus> {
        self.backends.iter().find(|b| b.backend == kind)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QemuPackageManager {
    Apt,
    Dnf,
}

impl QemuPackageManager {
    pub fn detect() -> Option<(Self, PathBuf)> {
        [Self::Apt, Self::Dnf]
            .into_iter()
            .find_map(|manager| host::which(manager.program()).map(|path| (manager, path)))
    }

    fn program(self) -> &'static str {
        match self {
            Self::Apt => "apt-get",
            Self::Dnf => "dnf",
        }
    }

    fn packages(self) -> &'static [&'static str] {
        match self {
            Self::Apt => &[
                "qemu-system-x86",
                "qemu-system-arm",
                "qemu-utils",
                "qemu-efi-aarch64",
                "ovmf",
                "openssh-client",
            ],
            Self::Dnf => &[
                "qemu-kvm",
                "qemu-img",
                "edk2-ovmf",
                "edk2-aarch64",
                "openssh",
            ],
        }
    }

    pub fn commands(self, program: &str) -> Vec<Vec<&str>> {
        let install = [&["-n", program, "install", "-y"], self.packages()].concat();
        match self {
            Self::Apt => vec![vec!["-n", program, "update"], install],
            Self::Dnf => vec![install],
        }
    }

    pub fn render(self) -> String {
        self.commands(self.program())
            .iter()
            .map(|args| format!("sudo {}", args.join(" ")))
            .collect::<Vec<_>>()
            .join(" && ")
    }

    pub fn linux_hint() -> String {
        format!(
            "Debian/Ubuntu: `{}`; Fedora: `{}`",
            Self::Apt.render(),
            Self::Dnf.render()
        )
    }
}

/// Inspect the host. Read-only: never installs or starts anything.
pub async fn doctor() -> DoctorReport {
    let host_os = HostOs::current();
    let host_arch = Arch::host();
    let kvm = host::kvm_usable();
    let mut accel = BTreeMap::new();
    for a in [Arch::Aarch64, Arch::X86_64] {
        if let Ok(acc) = host::qemu_accel(host_os, a, &kvm) {
            accel.insert(a.qemu().to_string(), acc.to_string());
        }
    }
    let host = HostReport {
        os: host_os,
        arch: host_arch,
        kvm: kvm.is_ok(),
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
    let qemu_ready = qemu.installed(host_arch);
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
            HostOs::Macos if host::which("brew").is_some() => Some("brew install qemu".into()),
            HostOs::Linux if host::which("apt-get").is_some() && !host::sudo_available() => {
                Some(format!(
                    "user-local QEMU at {}",
                    host::cua_home().join("qemu").display()
                ))
            }
            HostOs::Linux => QemuPackageManager::detect().map(|(manager, _)| manager.render()),
            _ => None,
        };
        if let Some(s) = step {
            qemu_status.provisionable = true;
            qemu_status.provisioning.push(s);
        }
    }
    if host::cua_home().join("qemu").is_dir() {
        qemu_status.detail.push_str(&format!(
            "; user-local wrappers at {}",
            host::cua_home().join("bin").display()
        ));
    }
    note_kvm(&mut qemu_status, host_os, &kvm);

    // Lume.
    let mut lume = LumeReport {
        supported_host: host_os == HostOs::Macos && host_arch == Arch::Aarch64,
        binary: lume_binary(),
        url: crate::local_network::lume_base(),
        ..Default::default()
    };
    #[cfg(feature = "lume")]
    if lume.supported_host {
        let c = crate::lume::LumeClient::new(lume.url.clone());
        lume.serving = c.reachable().await;
        if lume.binary.is_some() {
            // Run as Lume runs for this channel: a bare `lume --version`
            // initialises the pinned Lume's settings and telemetry under
            // Stable's `~/.config/lume` and `~/.lume`. (For Local this
            // writes only its own config under its home.)
            if let Ok(Some(l)) = crate::lume::lume_cmd() {
                lume.version = crate::lume::version_text(&l).await;
            }
        }
    }
    // Without any Lume, the built-in one is set up on first use unless the
    // setting says to use only this Mac's own.
    let builtin_ok = lume_builtin_allowed();
    let lume_status = BackendStatus {
        backend: BackendKind::Lume,
        ready: lume.supported_host && lume.serving,
        provisionable: lume.supported_host
            && !lume.serving
            && (lume.binary.is_some() || builtin_ok),
        detail: if !lume.supported_host {
            "Lume needs a macOS Apple Silicon host".into()
        } else if lume.serving {
            format!("lume serve answering at {}", lume.url)
        } else if lume.binary.is_some() {
            "lume installed but not serving".into()
        } else if builtin_ok {
            "built-in Lume, set up on first use".into()
        } else {
            "Lume is not installed on this Mac (macOS VMs are set to use this Mac's own Lume)"
                .into()
        },
        missing: if lume.supported_host && lume.binary.is_none() && !builtin_ok {
            vec!["lume".into()]
        } else {
            vec![]
        },
        provisioning: if !lume.supported_host || lume.serving {
            vec![]
        } else if lume.binary.is_some() {
            vec!["start `lume serve`".into()]
        } else if builtin_ok {
            vec![
                "download the built-in Lume (about 6 MB, signed)".into(),
                "start `lume serve`".into(),
            ]
        } else {
            vec![]
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
                    if container.gvisor {
                        container.gvisor_allow_suid =
                            rt.runtime_args("runsc").await.ok().flatten().map(|a| {
                                a.iter()
                                    .any(|x| x == "--allow-suid" || x == "--allow-suid=true")
                            });
                    }
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
    let managed_status = managed_status(
        crate::managed::LinuxSource::current(),
        container_status.ready,
    );
    if !container_status.ready && managed_status.provisionable {
        container_status
            .provisioning
            .push("use the built-in Linux runtime (a VM with Docker and gVisor)".into());
    }

    let mut backends = vec![lume_status, qemu_status, container_status, managed_status];
    let microvm = microvm_status().await.map(|(status, report)| {
        backends.push(status);
        report
    });
    let containerization = containerization_status().await.map(|(status, report)| {
        backends.push(status);
        report
    });
    let vz = vz_status().map(|(status, report)| {
        backends.push(status);
        report
    });
    DoctorReport {
        host,
        backends,
        qemu,
        lume,
        container,
        microvm,
        containerization,
        vz,
    }
}

/// microVMs on a Mac under `cua-vz`: ready when the monitor, the guest init
/// and the network gateway are installed (they ship next to `cua`); the
/// guest kernel downloads on first use.
#[cfg(feature = "microvm")]
fn vz_status() -> Option<(BackendStatus, VzReport)> {
    use crate::microvm::{assets, vz};
    if HostOs::current() != HostOs::Macos {
        return None;
    }
    let root = crate::microvm::MicroVmConfig::default().root;
    let unsupported = vz::unsupported_reason();
    let init = assets::init_bin();
    let r = VzReport {
        supported_host: unsupported.is_none(),
        unsupported: unsupported.clone(),
        monitor: vz::vz_bin(),
        init_static: init
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| crate::microvm::rootfs::elf_interpreter(&b))
            .is_some_and(|interp| interp.is_none()),
        init,
        gateway: crate::netgw::bin(),
        kernel_cached: assets::kernel_path(&root).is_file(),
    };
    let missing = if r.supported_host {
        r.missing()
    } else {
        vec![]
    };
    let mut provisioning = Vec::new();
    if r.supported_host && missing.is_empty() && !r.kernel_cached {
        provisioning.push(format!(
            "download the guest kernel {} (pinned, about 17 MB)",
            assets::KERNEL_VERSION
        ));
    }
    let status = BackendStatus {
        backend: BackendKind::Vz,
        ready: r.supported_host && missing.is_empty(),
        provisionable: false,
        detail: match (&unsupported, missing.is_empty()) {
            (Some(why), _) => why.clone(),
            (None, true) if provisioning.is_empty() => {
                "microVMs with checkpoints and forks (Virtualization.framework)".into()
            }
            (None, true) => {
                "microVMs with checkpoints and forks; the guest kernel downloads on first use"
                    .into()
            }
            (None, false) => "macOS microVMs are not installed with this cua".into(),
        },
        missing,
        provisioning,
    };
    Some((status, r))
}

#[cfg(not(feature = "microvm"))]
fn vz_status() -> Option<(BackendStatus, VzReport)> {
    None
}

/// microVMs on a Mac: ready once cua's own `container` is installed and no
/// other install holds the per-user services; `cua runtime setup
/// containerization` (or the first microVM Space) installs it and starts
/// them.
#[cfg(feature = "containerization")]
async fn containerization_status() -> Option<(BackendStatus, ContainerizationReport)> {
    use crate::containerization::ContainerizationRuntime;
    if HostOs::current() != HostOs::Macos {
        return None;
    }
    let r = ContainerizationRuntime::with_defaults().report().await;
    Some((containerization_backend(&r), r))
}

#[cfg(not(feature = "containerization"))]
async fn containerization_status() -> Option<(BackendStatus, ContainerizationReport)> {
    None
}

/// The doctor's row for a [`ContainerizationReport`].
#[cfg(feature = "containerization")]
pub fn containerization_backend(r: &ContainerizationReport) -> BackendStatus {
    use crate::containerization::install;
    let mut s = BackendStatus {
        backend: BackendKind::Containerization,
        ready: false,
        provisionable: false,
        detail: String::new(),
        missing: vec![],
        provisioning: vec![],
    };
    if !r.supported_host {
        s.detail = install::unsupported_reason()
            .unwrap_or_else(|| "Apple's Containerization needs macOS 26 on Apple silicon".into());
        return s;
    }
    if r.services == "other" {
        s.detail = format!(
            "another install of Apple's `container` runs its per-user services (data in {});              stop them (`container system stop`) to run microVM Spaces",
            r.other_app_root.as_deref().unwrap_or("another directory")
        );
        s.missing
            .push("Apple container services (held by another install)".into());
        return s;
    }
    if r.binary.is_none() {
        s.provisionable = true;
        s.detail = "microVM Spaces: Apple's container is set up on first use".into();
        s.provisioning.push(format!(
            "download Apple's container {} (about {} MB, signed by Apple) into ~/.cua",
            r.version,
            install::PKG_BYTES >> 20
        ));
        s.provisioning
            .push("start its services with cua's own data directory".into());
        return s;
    }
    s.ready = true;
    s.detail = if r.services == "running" {
        format!("Apple container {} running", r.version)
    } else {
        format!("Apple container {}, started on demand", r.version)
    };
    s
}

/// The microVM backend on a Linux host: ready when KVM, Firecracker, the
/// guest init and the host tools are there. Firecracker and the kernel
/// install on the first create (provisionable), as `cua runtime setup
/// firecracker` does; passt is the host's to install ([`crate::microvm::assets::PASTA_HINT`]).
#[cfg(feature = "microvm")]
async fn microvm_status() -> Option<(BackendStatus, MicroVmReport)> {
    if HostOs::current() != HostOs::Linux {
        return None;
    }
    let root = crate::microvm::MicroVmConfig::default().root;
    let r = crate::microvm::assets::report(&root).await;
    let missing = r.missing();
    let only_fc = missing.iter().all(|m| m == "firecracker");
    let mut provisioning = Vec::new();
    if r.firecracker.is_none() && r.kvm {
        provisioning.push(format!(
            "download Firecracker {} into ~/.cua/bin",
            crate::microvm::assets::FIRECRACKER_VERSION
        ));
    }
    if !r.kernel_cached {
        provisioning.push(format!(
            "download the guest kernel {} (pinned, about 44 MB)",
            crate::microvm::assets::KERNEL_VERSION
        ));
    }
    let status = BackendStatus {
        backend: BackendKind::Firecracker,
        ready: missing.is_empty(),
        provisionable: !missing.is_empty() && only_fc && r.kvm,
        detail: if !r.kvm {
            "microVMs need /dev/kvm (read-write) on this Linux host".into()
        } else if missing.is_empty() {
            format!(
                "{} with KVM{}",
                r.firecracker_version
                    .clone()
                    .unwrap_or_else(|| "Firecracker".into()),
                if r.kernel_cached {
                    ""
                } else {
                    "; guest kernel downloads on first use"
                }
            )
        } else if r.pasta.is_none() {
            crate::microvm::assets::PASTA_HINT.to_string()
        } else {
            "Firecracker microVMs are not set up".into()
        },
        missing,
        provisioning,
    };
    Some((status, r))
}

#[cfg(not(feature = "microvm"))]
async fn microvm_status() -> Option<(BackendStatus, MicroVmReport)> {
    None
}

fn note_kvm(status: &mut BackendStatus, host_os: HostOs, kvm: &std::io::Result<()>) {
    note_kvm_with(status, host_os, kvm, host::sudo_available);
}

fn note_kvm_with(
    status: &mut BackendStatus,
    host_os: HostOs,
    kvm: &std::io::Result<()>,
    sudo: impl Fn() -> bool,
) {
    if let Err(e) = kvm
        && host_os == HostOs::Linux
    {
        if host::qemu_accel_with(host_os, Arch::host(), kvm, sudo, |_| {}).is_err() {
            status.ready = false;
            status.missing.push(host::kvm_hint(e));
        } else {
            status.detail = format!(
                "{}; {}{}",
                status.detail,
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    "TCG ready; "
                } else {
                    ""
                },
                host::kvm_hint(e)
            );
        }
    }
}

/// The built-in Linux runtime ([`crate::managed`]) for `doctor`: ready
/// while its VM runs; else used (set up or booted on demand) when the
/// `runtime.linux` setting picks it here.
fn managed_status(source: crate::managed::LinuxSource, engine_ready: bool) -> BackendStatus {
    use crate::managed::{self, LinuxSource};
    let mb = managed::first_use_download() >> 20;
    let picked = source.allows_builtin()
        && (source == LinuxSource::Builtin || managed::exists() || !engine_ready);
    let running = picked && managed::running();
    BackendStatus {
        backend: BackendKind::Managed,
        ready: running,
        provisionable: picked && !running,
        detail: if !managed::supported() {
            "the built-in Linux runtime runs on macOS".into()
        } else if source == LinuxSource::System {
            "Linux Spaces are set to use this Mac's own container engine".into()
        } else if running {
            "built-in Linux runtime running (gVisor)".into()
        } else if picked && managed::exists() {
            "built-in Linux runtime, started on demand".into()
        } else if picked {
            format!("{}, about {mb} MB", managed::STATUS)
        } else {
            "not used: this Mac's own container engine runs Linux Spaces".into()
        },
        missing: vec![],
        provisioning: if picked && !running && !managed::exists() {
            vec![
                format!("download the built-in Linux runtime (about {mb} MB)"),
                "boot its VM".into(),
            ]
        } else if picked && !running {
            vec!["boot its VM".into()]
        } else {
            vec![]
        },
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
            if let Some(m) = status(BackendKind::Managed).filter(|m| m.ready || m.provisionable) {
                return Ok(Selection {
                    backend: BackendKind::Container,
                    reason: "the built-in Linux runtime (Docker in a VM); isolation: gVisor".into(),
                    provisioning: m.provisioning,
                });
            }
            // An engine was found but is stopped or refused: its own
            // message says how to start it or get access.
            if report.container.endpoint.is_some() {
                return Err(VmmError::other(c.detail));
            }
            Err(VmmError::missing(
                "a container engine",
                format!("{}; {}", c.detail, engine_install_hint()),
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
                if q.ready
                    || q.provisionable
                    || arch != host.arch && report.qemu.installed(host.arch)
                {
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
                Err(VmmError::missing(
                    "QEMU",
                    if host.os == HostOs::Windows {
                        crate::preflight::QEMU_MISSING_WINDOWS.to_string()
                    } else {
                        q.missing.join(", ")
                    },
                ))
            }
        },
    }
}

/// How to get a container engine here.
fn engine_install_hint() -> &'static str {
    #[cfg(feature = "container")]
    {
        crate::container::engine::install_hint()
    }
    #[cfg(not(feature = "container"))]
    {
        "install Docker or Podman and start it"
    }
}

/// The Lume binary the setting picks (see [`crate::lume::builtin`]).
fn lume_binary() -> Option<PathBuf> {
    #[cfg(feature = "lume")]
    {
        crate::lume::lume_bin()
    }
    #[cfg(not(feature = "lume"))]
    {
        host::which(cua_home::channel::CuaChannel::current().lume_bin_name())
    }
}

/// Whether `runtime.lume` lets the built-in Lume be set up.
fn lume_builtin_allowed() -> bool {
    #[cfg(feature = "lume")]
    {
        crate::lume::builtin::LumeSource::current().allows_builtin()
    }
    #[cfg(not(feature = "lume"))]
    {
        false
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
            microvm: None,
            containerization: None,
            vz: None,
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

    #[test]
    fn doctor_reports_why_native_qemu_has_no_kvm() {
        let noted = |kvm: std::io::Result<()>, sudo| {
            let mut q = report(true, false, false, false, HostOs::Linux).backends[1].clone();
            q.detail = "QEMU 10".into();
            q.missing.clear();
            note_kvm_with(&mut q, HostOs::Linux, &kvm, || sudo);
            q
        };
        let missing = noted(Err(std::io::ErrorKind::NotFound.into()), true);
        assert!(missing.ready && missing.missing.is_empty(), "{missing:?}");
        assert!(missing.detail.starts_with("QEMU 10; "), "{missing:?}");
        assert!(
            missing.detail.contains("nested virtualization"),
            "{missing:?}"
        );
        let denied = noted(Err(std::io::ErrorKind::PermissionDenied.into()), true);
        assert!(!denied.ready && !denied.provisionable, "{denied:?}");
        assert!(
            denied.missing[0].contains("sudo usermod -aG kvm"),
            "{denied:?}"
        );
        let denied = noted(Err(std::io::ErrorKind::PermissionDenied.into()), false);
        assert!(denied.ready && denied.missing.is_empty(), "{denied:?}");
        assert!(denied.detail.contains("TCG ready"), "{denied:?}");
        let ok = noted(Ok(()), true);
        assert!(
            ok.ready && ok.missing.is_empty() && ok.detail == "QEMU 10",
            "{ok:?}"
        );
    }

    #[test]
    fn unopenable_kvm_still_selects_qemu_for_foreign_guests() {
        let mut r = report(false, false, false, false, HostOs::Linux);
        r.qemu = QemuReport {
            qemu_img: Some("qemu-img".into()),
            ..Default::default()
        };
        r.qemu
            .binaries
            .insert("aarch64".into(), Some("qemu-system".into()));
        let s = select(&req(ImageKind::Vm, GuestOs::Linux, Some(Arch::X86_64)), &r).unwrap();
        assert_eq!(s.backend, BackendKind::Qemu);
        assert!(select(&req(ImageKind::Vm, GuestOs::Linux, None), &r).is_err());
    }

    /// The report `cua runtime doctor --json` prints round-trips.
    #[test]
    fn doctor_report_serialises() {
        let r = report(true, false, true, false, HostOs::Macos);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["host"]["kvm"], serde_json::json!(false));
        assert_eq!(json["host"]["accel"]["x86_64"], serde_json::json!("tcg"));
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

    #[test]
    fn apt_qemu_plan_refreshes_before_install() {
        let commands = QemuPackageManager::Apt.commands("/usr/bin/apt-get");
        assert_eq!(
            commands,
            [
                vec!["-n", "/usr/bin/apt-get", "update"],
                vec![
                    "-n",
                    "/usr/bin/apt-get",
                    "install",
                    "-y",
                    "qemu-system-x86",
                    "qemu-system-arm",
                    "qemu-utils",
                    "qemu-efi-aarch64",
                    "ovmf",
                    "openssh-client",
                ],
            ]
        );
    }

    #[test]
    fn apt_qemu_plan_render_matches_command_arguments() {
        assert_eq!(
            QemuPackageManager::Apt.render(),
            "sudo -n apt-get update && sudo -n apt-get install -y qemu-system-x86 \
             qemu-system-arm qemu-utils qemu-efi-aarch64 ovmf openssh-client"
        );
    }

    #[test]
    fn dnf_qemu_plan_render_matches_command_arguments() {
        assert_eq!(
            QemuPackageManager::Dnf.commands("dnf"),
            [vec![
                "-n",
                "dnf",
                "install",
                "-y",
                "qemu-kvm",
                "qemu-img",
                "edk2-ovmf",
                "edk2-aarch64",
                "openssh",
            ]]
        );
        assert_eq!(
            QemuPackageManager::Dnf.render(),
            "sudo -n dnf install -y qemu-kvm qemu-img edk2-ovmf edk2-aarch64 openssh"
        );
    }

    #[test]
    fn qemu_plan_preserves_both_guest_architectures() {
        let apt = QemuPackageManager::Apt.render();
        let dnf = QemuPackageManager::Dnf.render();
        for package in [
            "qemu-system-x86",
            "ovmf",
            "qemu-system-arm",
            "qemu-efi-aarch64",
        ] {
            assert!(apt.contains(package), "{package}");
        }
        for package in ["qemu-kvm", "edk2-ovmf", "edk2-aarch64"] {
            assert!(dnf.contains(package), "{package}");
        }
    }
}
