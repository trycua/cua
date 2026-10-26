//! QEMU backend: `qemu-system-*` launched directly on the host.
//!
//! Port of `QEMUBaremetalRuntime` (`cua_sandbox/runtime/qemu.py`) with:
//! * `virt`/`q35` machine selection per guest arch, `hvf`/`kvm`/`whpx`/`tcg`
//!   accelerator detection,
//! * edk2/OVMF discovery (Homebrew ships it under `share/qemu`),
//! * qcow2 chains (image → frozen layers → instance overlay) via `qemu-img`,
//! * user-mode networking with loopback `hostfwd` for declared ports,
//! * a QMP client for screenshots/input/power/pause/snapshots,
//! * free VNC display allocation, serial console log, detached launch + pidfile,
//! * per-instance state under `~/.cua/vmm/qemu/<name>/`.
//!
//! Instances survive the process that started them: all state lives on disk
//! and is reconciled against the pid on every call.

pub mod args;
pub mod disk_arch;
pub mod export;
pub mod firmware;
pub mod img;
pub mod qmp;
pub mod snapshot;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::auto::QemuPackageManager;
use crate::error::{Result, VmmError};
use crate::exec::{GuestExec, SshExec};
use crate::host;
use crate::runtime::{CheckpointExport, DiskResolver, GuestIdentity, Runtime};
use crate::types::*;

pub use args::{Firmware, Memory};
pub use qmp::QmpClient;
pub use snapshot::MemorySnapshot;

/// Configuration for [`QemuRuntime`].
#[derive(Clone)]
pub struct QemuConfig {
    /// State root (default `~/.cua/vmm/qemu`).
    pub root: PathBuf,
    /// Resolves [`ImageSource::Oci`] references (containerDisks) to local disks.
    pub resolver: Option<Arc<dyn DiskResolver>>,
    /// Allow installing QEMU through the host package manager when missing.
    pub allow_install: bool,
    /// How long `stop` waits for an ACPI shutdown before forcing `quit`.
    pub graceful_stop: Duration,
    /// Allocate a VNC display per instance.
    pub vnc: bool,
    /// Override the accelerator (`hvf`/`kvm`/`whpx`/`tcg`); auto when `None`.
    pub accel: Option<String>,
    /// Gives a VM restored from shared memory its own identity (its
    /// cua-spacesd token, clock, entropy, machine-id). Without it no VM is
    /// restored from memory: every start boots cold and running VMs fork
    /// by disk only.
    pub identity: Option<Arc<dyn GuestIdentity>>,
    /// Warm starts: the first start of a configuration records a golden
    /// memory snapshot of a booted guest, before it has an identity, and
    /// later starts restore it ([`snapshot`]). `CUA_QEMU_WARM=0` turns it
    /// off.
    pub warm: bool,
    /// Admission limits for the VMs this runtime starts
    /// ([`crate::density`]; `CUA_VM_*`).
    pub density: crate::density::DensityConfig,
    /// Free page reporting (a virtio-balloon) for VMs with anonymous RAM:
    /// on by default where QEMU's discard returns memory to the host
    /// (Linux); `CUA_QEMU_FREE_PAGE_REPORTING=1` turns it on elsewhere,
    /// `0` off.
    pub free_page_reporting: bool,
    /// Golden snapshots kept beyond those in use: least recently used
    /// removed first. One a VM or a checkpoint (pinned or not) is layered
    /// on is never removed and does not count. `CUA_QEMU_GOLDEN_KEEP`
    /// overrides the default of 3.
    pub golden_keep: usize,
    /// Refuse a new instance before its image is pulled when
    /// `qemu-system-<arch>` or `qemu-img` is missing (a VM image can be
    /// gigabytes, and neither tool is optional).
    pub preflight_tools: bool,
}

impl Default for QemuConfig {
    fn default() -> Self {
        Self {
            root: host::cua_home().join("vmm").join("qemu"),
            resolver: None,
            allow_install: false,
            graceful_stop: Duration::from_secs(30),
            vnc: true,
            accel: None,
            identity: None,
            warm: std::env::var("CUA_QEMU_WARM").map_or(true, |v| v != "0"),
            density: crate::density::DensityConfig::from_env(),
            free_page_reporting: free_page_reporting_default(
                std::env::consts::OS,
                std::env::var("CUA_QEMU_FREE_PAGE_REPORTING")
                    .ok()
                    .as_deref(),
            ),
            golden_keep: std::env::var("CUA_QEMU_GOLDEN_KEEP")
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(3),
            preflight_tools: true,
        }
    }
}

/// The free-block size, as a page order, a guest with the balloon reports
/// from (3: 32 KiB). Linux reports only free blocks of `pageblock_order` (2
/// MiB) by default, and memory a workload frees is mostly in smaller
/// blocks: measured on KVM, a guest that freed 288 MiB of tmpfs had one
/// new 2 MiB block and returned nothing. Smaller reports split the host's
/// transparent huge pages for that memory; dense VMs that mostly wait
/// prefer the memory back.
pub const PAGE_REPORTING_ORDER: u32 = 3;

/// The cloud-config part that sets [`PAGE_REPORTING_ORDER`] at every boot.
const PAGE_REPORTING_PART: &str = "#cloud-config\nbootcmd:\n  - [sh, -c, 'echo 3 > /sys/module/page_reporting/parameters/page_reporting_order 2>/dev/null || true']\n";

/// Whether VMs with anonymous RAM get free page reporting on `os`.
///
/// QEMU returns a reported page with `madvise(MADV_DONTNEED)`. On Linux
/// that frees it at once. On macOS it is only a hint that leaves the page
/// resident and charged to QEMU (measured: a 512 MiB anonymous region keeps
/// its 512 MiB footprint after `MADV_DONTNEED`; only `MADV_FREE_REUSABLE`
/// or a fresh mapping return it), so there the guest would report for
/// nothing; it is off unless asked for (a QEMU that discards differently).
/// Whether a memory checkpoint's stream should be turned into a shared
/// `mem.ram` for copy-on-write forks: only a stream that holds the RAM, and
/// never one of a VM with free page reporting (its restores keep the
/// balloon, which must not discard pages of a RAM file other VMs map).
pub(crate) fn needs_shared_ram(snap: &MemorySnapshot) -> bool {
    snap.ram_file.is_none() && !snap.free_page_reporting
}

pub fn free_page_reporting_default(os: &str, env: Option<&str>) -> bool {
    match env.map(str::trim) {
        Some("1" | "on" | "true" | "yes") => true,
        Some("0" | "off" | "false" | "no") => false,
        _ => os == "linux",
    }
}

/// What a state entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A bootable sandbox.
    Instance,
    /// An immutable fork source created by `ensure_base`.
    Base,
    /// An immutable fork source created by `checkpoint` of a stopped instance.
    Checkpoint,
}

const BOOT_CONFIG: &str = "boot-config.json";
const KEEP_SEED_LABEL: &str = "cua.vmm.keep-seed";
/// A resize boots the VM again on the host ports it had (its forwards, QMP
/// and VNC), so whatever holds its endpoints (a daemon's cached connection,
/// an SDK handle, a viewer) reaches the new run where it reached the old.
/// A port taken meanwhile falls back to fresh ones ([`Self::launch_admitted`]).
const KEEP_PORTS_LABEL: &str = "cua.vmm.keep-ports";

#[derive(Default, Serialize, Deserialize)]
struct BootConfig {
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    cloud_init_user_data: Option<String>,
}

/// Persisted per-instance state (`<root>/<name>/state.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QemuState {
    pub name: String,
    pub kind: EntryKind,
    pub arch: Arch,
    pub os: GuestOs,
    /// Top of the disk chain (the only file QEMU writes, for instances).
    pub disk: PathBuf,
    pub disk_format: String,
    #[serde(default)]
    pub install_iso: Option<PathBuf>,
    #[serde(default)]
    pub seed_iso: Option<PathBuf>,
    pub firmware: Firmware,
    pub cpus: u32,
    pub memory_mb: u64,
    /// Guest → host forwards of the current/last run.
    #[serde(default)]
    pub ports: BTreeMap<u16, u16>,
    #[serde(default)]
    pub vnc_display: Option<u16>,
    #[serde(default)]
    pub qmp_port: Option<u16>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub accel: String,
    #[serde(default)]
    pub ssh: Option<SshAccess>,
    #[serde(default)]
    pub restrict_network: bool,
    /// The egress policy ([`crate::egress`]; `None`: never set). Forks
    /// inherit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress: Option<crate::egress::EgressPolicy>,
    /// The current run is inside its own network namespace (pasta), where
    /// the egress policy is enforced.
    #[serde(default)]
    pub netns: bool,
    /// pasta, holding that namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pasta_pid: Option<u32>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Internal (`savevm`) snapshots taken while running.
    #[serde(default)]
    pub snapshots: Vec<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub source: Option<String>,
    pub created_at: u64,
    /// The GPU option it was created with ([`crate::gpu::VIRGL`]), kept for
    /// every later boot.
    #[serde(default)]
    pub gpu: Option<String>,
    /// A memory snapshot the next start restores instead of booting (a fork
    /// of a running VM or of a memory checkpoint, or a golden snapshot).
    #[serde(default)]
    pub restore_from: Option<MemorySnapshot>,
    /// The memory snapshot a checkpoint entry holds.
    #[serde(default)]
    pub memory: Option<MemorySnapshot>,
    /// How the last start came up: `cold`, `warm` (a golden snapshot) or
    /// `restore` (a fork's or checkpoint's memory).
    #[serde(default)]
    pub last_start: Option<String>,
    /// The current (or last) run has a free-page-reporting balloon. A
    /// memory snapshot of it records this, and VMs restoring the snapshot
    /// get the same device (a migration stream loads only into the device
    /// set it was written from).
    #[serde(default)]
    pub free_page_reporting: bool,
    /// The file the current (or last) run maps as guest RAM: a snapshot's
    /// RAM shared copy-on-write with the other VMs restored from it.
    #[serde(default)]
    pub ram_file: Option<PathBuf>,
    /// The current (or last) run has no display or input hardware (its
    /// guest serves no desktop, `CUA_ENV_NO_DESKTOP`). Recorded in its
    /// memory snapshots like [`Self::free_page_reporting`].
    #[serde(default)]
    pub headless: bool,
    /// A fork source ([`StartSpec::forkable`]): guest RAM is `mem.ram` in
    /// the entry's directory, mapped shared, so a memory checkpoint clones
    /// it instead of writing RAM out.
    #[serde(default)]
    pub shared_memory: bool,
    /// A pinned checkpoint: kept by [`QemuRuntime::prune_checkpoints`] and
    /// refused by `delete_checkpoint` until unpinned.
    #[serde(default)]
    pub pinned: bool,
    /// The VM has the bulk-lane disk ([`crate::bulk`]). Decided when it boots
    /// cold; a memory restore keeps the device set of the snapshot it loads.
    #[serde(default)]
    pub bulk: bool,
    /// swtpm TPM 2.0 with state in `tpm/` of the entry directory; set at
    /// creation for Windows on ARM; forks and checkpoints keep it.
    #[serde(default)]
    pub tpm: bool,
}

/// Windows 11 on ARM: the guest that needs Secure Boot firmware and a TPM.
fn is_windows_arm(os: GuestOs, arch: Arch) -> bool {
    os == GuestOs::Windows && arch == Arch::Aarch64
}

/// Whether this host can run a Windows-on-ARM guest.
fn windows_arm_host(os: GuestOs, guest: Arch, host: Arch, host_os: host::HostOs) -> Result<()> {
    if !is_windows_arm(os, guest) {
        return Ok(());
    }
    if host != Arch::Aarch64 {
        return Err(VmmError::invalid(format!(
            "Windows on ARM guests need an arm64 host (Apple silicon, or arm64 Linux with KVM); this host is {host}"
        )));
    }
    if host_os == host::HostOs::Windows {
        return Err(VmmError::invalid(
            "Windows on ARM guests need swtpm for their TPM, which runs on macOS and Linux hosts only",
        ));
    }
    Ok(())
}

/// Windows 11 needs 4 GB; `cua-vmm` cannot tell the 2048 MB default from an
/// explicit 2048, so a smaller ask is raised.
fn guest_memory_mb(os: GuestOs, arch: Arch, asked: u64) -> u64 {
    if is_windows_arm(os, arch) {
        asked.max(4096)
    } else {
        asked
    }
}

/// A TPM VM's state cannot follow `what`: refuse before anything changes.
fn refuse_tpm(st: &QemuState, what: &str) -> Result<()> {
    if st.tpm {
        return Err(VmmError::invalid(format!(
            "'{}' has a TPM, whose state cannot follow {what}; stop it first",
            st.name
        )));
    }
    Ok(())
}

/// What `qemu` (a `qemu-system-*` binary) offers for virgl on this host:
/// its GL device and headless EGL display, and a DRM render node. Nothing is
/// probed off Linux (no macOS QEMU build has a headless GL display).
pub async fn virgl_host(qemu: &std::path::Path) -> crate::gpu::VirglHost {
    if std::env::consts::OS != "linux" {
        return crate::gpu::VirglHost::default();
    }
    let help = |what: &'static str| {
        let qemu = qemu.to_path_buf();
        async move {
            let out = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                tokio::process::Command::new(qemu)
                    .args([what, "help"])
                    .stdin(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .output(),
            )
            .await;
            match out {
                Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).into_owned(),
                _ => String::new(),
            }
        }
    };
    let render_node = std::fs::read_dir("/dev/dri").ok().and_then(|rd| {
        let mut nodes: Vec<String> = rd
            .flatten()
            .map(|e| e.path().display().to_string())
            .filter(|p| p.contains("renderD"))
            .collect();
        nodes.sort();
        nodes.into_iter().next()
    });
    crate::gpu::VirglHost {
        gl_device: help("-device").await.contains("virtio-gpu-gl-pci"),
        egl_headless: help("-display").await.contains("egl-headless"),
        render_node,
    }
}

/// The QEMU backend.
pub struct QemuRuntime {
    cfg: QemuConfig,
    lock: Mutex<()>,
    /// TPM VMs whose swtpm is being launched (until its clean-up is done) or
    /// whose TPM state is being copied: the two never overlap.
    tpm_busy: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Serializes recording golden snapshots.
    golden_lock: Mutex<()>,
    /// VMs admitted whose launch has not finished (within
    /// [`crate::density::SETTLE`]).
    pending: std::sync::Mutex<Vec<crate::density::Pending>>,
    /// Layer collections asked for / done (see `collect_layers`).
    gc_asked: std::sync::atomic::AtomicU64,
    gc_done: Mutex<u64>,
    /// Layer collections actually run.
    gc_runs: std::sync::atomic::AtomicU64,
}

const LAYERS: &str = "_layers";

/// Directory (under the VM root) of stable paths for running warm-pool
/// members. A pool member's QEMU opens its disk, seed and varstore through
/// `_run/<member>/...`, a symlink to the member's directory that a claim
/// re-points after renaming the directory.
///
/// QEMU keeps its image files open across a rename, but a migration to a
/// file (checkpoint, fork, snapshot of a running Space) ends by inactivating
/// the block layer, and file-posix then re-opens each image by the path it
/// was started with. With the old path gone that fails
/// (`bdrv_inactivate_all() failed (-1)`), so every claimed pool member
/// would be unable to checkpoint or fork while running.
const RUN_LINKS: &str = "_run";

impl QemuRuntime {
    pub fn new(cfg: QemuConfig) -> Self {
        Self {
            cfg,
            lock: Mutex::new(()),
            tpm_busy: Default::default(),
            golden_lock: Mutex::new(()),
            pending: Default::default(),
            gc_asked: Default::default(),
            gc_done: Mutex::new(0),
            gc_runs: Default::default(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(QemuConfig::default())
    }

    pub fn config(&self) -> &QemuConfig {
        &self.cfg
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.cfg.root.join(name)
    }

    /// The stable path alias of pool member `member` ([`RUN_LINKS`]).
    fn run_link(&self, member: &str) -> PathBuf {
        self.cfg.root.join(RUN_LINKS).join(member)
    }

    /// Points `member`'s stable alias at `target`, atomically (a running
    /// QEMU may re-open through it at any moment). Where there are no
    /// symlinks (Windows hosts) there is no alias and this does nothing.
    fn point_alias(&self, member: &str, target: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            let link = self.run_link(member);
            let tmp = link.with_file_name(format!(".{member}.new"));
            std::fs::create_dir_all(self.cfg.root.join(RUN_LINKS))?;
            let _ = std::fs::remove_file(&tmp);
            std::os::unix::fs::symlink(target, &tmp)?;
            std::fs::rename(&tmp, &link)?;
        }
        #[cfg(not(unix))]
        let _ = (member, target);
        Ok(())
    }

    /// Removes aliases that point at `dir` (its QEMU is gone) and any whose
    /// target no longer exists.
    fn drop_aliases(&self, dir: Option<&Path>) {
        let Ok(rd) = std::fs::read_dir(self.cfg.root.join(RUN_LINKS)) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let to_dir = dir.is_some_and(|d| std::fs::read_link(&p).is_ok_and(|t| t == d));
            if to_dir || !p.exists() {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.dir(name).join("state.json")
    }

    fn layers_dir(&self) -> PathBuf {
        self.cfg.root.join(LAYERS)
    }

    /// Load persisted state for `name`.
    pub fn load(&self, name: &str) -> Result<QemuState> {
        let p = self.state_path(name);
        let raw = std::fs::read(&p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VmmError::NotFound(name.to_string())
            } else {
                e.into()
            }
        })?;
        serde_json::from_slice(&raw).map_err(|e| VmmError::State {
            path: p,
            detail: e.to_string(),
        })
    }

    fn save(&self, st: &QemuState) -> Result<()> {
        let dir = self.dir(&st.name);
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("state.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(st)?)?;
        std::fs::rename(tmp, self.state_path(&st.name))?;
        Ok(())
    }

    /// The configuration an instance first booted with (its environment,
    /// command and raw cloud-init), kept private beside its disk so that a
    /// resize boots it the same way (a different environment is a different
    /// cloud-init instance).
    fn record_boot_config(dir: &Path, spec: &StartSpec) -> Result<()> {
        let cfg = BootConfig {
            env: spec.env.clone(),
            command: spec.command.clone(),
            cloud_init_user_data: spec.cloud_init_user_data.clone(),
        };
        host::write_private(&dir.join(BOOT_CONFIG), &serde_json::to_vec(&cfg)?)?;
        Ok(())
    }

    fn read_boot_config(dir: &Path) -> BootConfig {
        std::fs::read(dir.join(BOOT_CONFIG))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn is_running(st: &QemuState) -> bool {
        st.pid.is_some_and(host::pid_alive)
    }

    fn status_of(st: &QemuState) -> Status {
        match st.kind {
            EntryKind::Base | EntryKind::Checkpoint => Status::Stopped,
            EntryKind::Instance if Self::is_running(st) => {
                if st.paused {
                    Status::Paused
                } else {
                    Status::Running
                }
            }
            EntryKind::Instance => Status::Stopped,
        }
    }

    /// Locate (or, when allowed, install) `qemu-system-<arch>`.
    /// virgl GPU acceleration on this host ([`crate::gpu::qemu_support`]).
    pub async fn gpu_support(&self) -> crate::gpu::GpuSupport {
        let arch = crate::types::Arch::host();
        let host = match host::which(&format!("qemu-system-{}", arch.qemu())) {
            Some(bin) => virgl_host(&bin).await,
            None => crate::gpu::VirglHost::default(),
        };
        crate::gpu::qemu_support(std::env::consts::OS, &host)
    }

    pub async fn binary(&self, arch: Arch) -> Result<PathBuf> {
        let name = format!("qemu-system-{}", arch.qemu());
        if let Some(p) = host::which(&name) {
            return Ok(p);
        }
        if self.cfg.allow_install {
            install_qemu().await?;
            if let Some(p) = host::which(&name) {
                return Ok(p);
            }
        }
        Err(VmmError::missing(
            name,
            format!(
                "install QEMU (macOS: `brew install qemu`; {}) or enable allow_install to let cua provision it",
                QemuPackageManager::linux_hint()
            ),
        ))
    }

    /// `qemu-system-<arch>` and `qemu-img` are on this host (installed
    /// first when allowed), checked before a new instance pulls anything.
    async fn ensure_tools(&self, arch: Arch) -> Result<()> {
        let os = host::HostOs::current();
        if missing_tools(arch, os, host::which).is_none() {
            return Ok(());
        }
        if self.cfg.allow_install {
            install_qemu().await?;
        }
        missing_tools(arch, os, host::which).map_or(Ok(()), Err)
    }

    fn accel_for(&self, arch: Arch, kvm: &std::io::Result<()>) -> Result<String> {
        self.accel_for_with(arch, kvm, host::sudo_available)
    }

    fn accel_for_with(
        &self,
        arch: Arch,
        kvm: &std::io::Result<()>,
        sudo: impl Fn() -> bool,
    ) -> Result<String> {
        if let Some(accel) = &self.cfg.accel {
            return Ok(accel.clone());
        }
        let accel = host::qemu_accel_with(host::HostOs::current(), arch, kvm, sudo, |message| {
            tracing::warn!("{message}")
        })?;
        if let Err(e) = kvm
            && accel == "tcg"
            && arch == Arch::host()
        {
            tracing::warn!("{}", host::kvm_hint(e));
        }
        Ok(accel.to_string())
    }

    async fn resolve_source(
        &self,
        image: &ImageSource,
        arch: Arch,
        creds: Option<&RegistryCredentials>,
    ) -> Result<PathBuf> {
        match image {
            ImageSource::Disk { path } => {
                if !path.exists() {
                    return Err(VmmError::invalid(format!(
                        "disk {} does not exist",
                        path.display()
                    )));
                }
                Ok(path.clone())
            }
            ImageSource::Oci { reference } => match &self.cfg.resolver {
                // Credentials only for the image's own registry.
                Some(r) => {
                    let creds = creds.filter(|c| c.applies_to(registry_of(reference)));
                    r.resolve_with_credentials(reference, arch, creds).await
                }
                None => Err(VmmError::invalid(format!(
                    "cannot boot OCI reference '{reference}' under QEMU without a DiskResolver \
                     (use cua-image's ContainerDiskResolver)"
                ))),
            },
            ImageSource::Existing => Err(VmmError::invalid(
                "ImageSource::Existing has no source disk",
            )),
        }
    }

    /// Firmware for a new instance. aarch64 always boots UEFI. On x86 an
    /// explicit preference wins (an image's `firmware`); otherwise
    /// UEFI for Windows and SeaBIOS for everything else (KubeVirt's default
    /// for containerDisks).
    async fn setup_firmware(
        &self,
        dir: &Path,
        arch: Arch,
        os: GuestOs,
        want: Option<BootFirmware>,
    ) -> Result<Firmware> {
        let needs_uefi = arch == Arch::Aarch64
            || match want {
                Some(f) => f == BootFirmware::Efi,
                None => os == GuestOs::Windows,
            };
        if !needs_uefi {
            return Ok(Firmware::Bios);
        }
        let bin = self.binary(arch).await?;
        let windows_arm = is_windows_arm(os, arch);
        let cua_dir = host::cua_home().join("firmware").join("aavmf");
        let override_dir = std::env::var_os(firmware::AAVMF_DIR_ENV)
            .filter(|d| !d.is_empty())
            .map(PathBuf::from);
        let fw = (if windows_arm {
            let real = std::fs::canonicalize(&bin).unwrap_or_else(|_| bin.clone());
            let dirs = firmware::windows_arm_dirs(override_dir.as_deref(), &cua_dir, &bin, &real);
            firmware::find_windows_arm(&dirs, |p| p.is_file())
        } else {
            firmware::find_uefi(arch, &bin)
        })
        .ok_or_else(|| {
            VmmError::missing(
                format!("UEFI firmware for {arch}"),
                if let (true, Some(dir)) = (windows_arm, &override_dir) {
                    format!(
                        "{}={} does not hold AAVMF_CODE.secboot.fd and AAVMF_VARS.ms.fd",
                        firmware::AAVMF_DIR_ENV,
                        dir.display()
                    )
                } else if windows_arm {
                    format!(
                        "Windows on ARM needs a Secure Boot UEFI pair, AAVMF_CODE.secboot.fd and \
                         AAVMF_VARS.ms.fd (Debian's qemu-efi-aarch64 package has them): put both \
                         in {} or set {} to their directory; see \
                         https://cua.ai/docs/cua-sdk/guides/local-runtimes#windows-on-arm",
                        cua_dir.display(),
                        firmware::AAVMF_DIR_ENV
                    )
                } else {
                    format!(
                        "install edk2 (Homebrew qemu ships it; {})",
                        QemuPackageManager::linux_hint()
                    )
                },
            )
        })?;
        match fw.vars_template {
            Some(tpl) => {
                let vars = dir.join("efivars.qcow2");
                if !vars.exists() {
                    img::raw_to_qcow2(&tpl, &vars).await?;
                }
                Ok(Firmware::UefiPflash {
                    code: fw.code,
                    vars,
                    vars_format: "qcow2".into(),
                })
            }
            None => Ok(Firmware::UefiBios { code: fw.code }),
        }
    }

    /// Create the on-disk instance (overlay + firmware), not started.
    async fn create_instance(
        &self,
        name: &str,
        backing: Option<&Path>,
        install_iso: Option<PathBuf>,
        spec_like: &StartSpec,
        source: Option<String>,
    ) -> Result<QemuState> {
        crate::disk::ensure_space(
            &self.cfg.root,
            backing.map_or(crate::disk::VM_CREATE_ESTIMATE, |_| crate::disk::GIB),
            &format!("create VM {name}"),
        )?;
        let dir = self.dir(name);
        std::fs::create_dir_all(&dir)?;
        let arch = spec_like.effective_arch();
        let disk = dir.join("disk.qcow2");
        let res: Result<QemuState> = async {
            match backing {
                Some(b) => img::create_overlay(b, &disk).await?,
                None => img::create_blank(&disk, spec_like.disk_size_gb.unwrap_or(32)).await?,
            }
            if let Some(gb) = spec_like.disk_size_gb {
                img::grow_to(&disk, gb).await?;
            }
            let (os, want) = boot_hints(backing, arch, spec_like).await;
            let firmware = self.setup_firmware(&dir, arch, os, want).await?;
            let tpm = is_windows_arm(os, arch);
            if tpm {
                std::fs::create_dir_all(dir.join("tpm"))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        dir.join("tpm"),
                        std::fs::Permissions::from_mode(0o700),
                    )?;
                }
            }
            Ok(QemuState {
                name: name.to_string(),
                kind: EntryKind::Instance,
                arch,
                os,
                disk: disk.clone(),
                disk_format: "qcow2".into(),
                install_iso,
                seed_iso: None,
                firmware,
                cpus: spec_like.cpus,
                memory_mb: spec_like.memory_mb,
                ports: BTreeMap::new(),
                vnc_display: None,
                qmp_port: None,
                pid: None,
                accel: String::new(),
                ssh: spec_like.ssh.clone(),
                restrict_network: spec_like.restrict_network,
                egress: None,
                netns: false,
                pasta_pid: None,
                extra_args: spec_like.extra_args.clone(),
                snapshots: Vec::new(),
                paused: false,
                source,
                created_at: host::now_secs(),
                gpu: spec_like.gpu.clone(),
                restore_from: None,
                memory: None,
                last_start: None,
                free_page_reporting: false,
                ram_file: None,
                headless: false,
                shared_memory: spec_like.forkable,
                pinned: false,
                bulk: false,
                tpm,
            })
        }
        .await;
        match res {
            Ok(st) => {
                self.save(&st)?;
                Ok(st)
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    /// Write the cloud-init seed for this run: SSH key and/or raw user-data,
    /// plus the guest environment (the spacesd token) for Linux guests.
    async fn write_seed(&self, st: &mut QemuState, spec: &StartSpec) -> Result<()> {
        // A resize boots the VM as the same cloud-init instance it already is:
        // its seed stays as it was (a new one is a new instance, which
        // rewrites the guest's host keys and token).
        if spec.labels.contains_key(KEEP_SEED_LABEL)
            && st.seed_iso.as_ref().is_some_and(|p| p.exists())
        {
            return Ok(());
        }
        if let Some(mut seed) = crate::cloudinit::seed_for_spec(&st.name, spec)? {
            // A guest that boots (anonymous RAM) gets the balloon where it
            // is on: let it report small free blocks too.
            if self.cfg.free_page_reporting && spec.os == GuestOs::Linux {
                seed = seed.with_part(PAGE_REPORTING_PART);
            }
            let path = self.dir(&st.name).join("seed.iso");
            seed.write_iso(&path)?;
            st.seed_iso = Some(path);
        }
        Ok(())
    }

    /// Launch QEMU for an instance entry (or a golden snapshot being
    /// recorded, which is not an entry: `persist` false) in the directory
    /// holding its disk. With `boot.incoming` the VM waits paused for its
    /// state ([`QmpClient::migrate_from_file`]).
    async fn launch(
        &self,
        st: &mut QemuState,
        spec: &StartSpec,
        boot: Boot,
        persist: bool,
    ) -> Result<()> {
        windows_arm_host(st.os, st.arch, Arch::host(), host::HostOs::current())?;
        let floored;
        let spec = if guest_memory_mb(st.os, st.arch, spec.memory_mb) > spec.memory_mb {
            tracing::info!(
                sandbox = %st.name,
                asked = spec.memory_mb,
                "Windows 11 needs 4 GB; giving the VM 4096 MB"
            );
            floored = StartSpec {
                memory_mb: guest_memory_mb(st.os, st.arch, spec.memory_mb),
                ..spec.clone()
            };
            &floored
        } else {
            spec
        };
        self.admit_waiting(
            &st.name,
            spec.cpus,
            spec.memory_mb,
            admission_file(&boot.memory),
        )
        .await?;
        let res = self.launch_admitted(st, spec, boot, persist).await;
        // Refused by a start that owns the TPM: nothing here was acquired.
        if res.as_ref().is_err_and(|e| !tpm_claim_refused(e)) {
            self.release(&st.name);
            let _ = self.stop_tpm(st).await;
        }
        res
    }

    /// [`Self::admit`], waiting (up to [`crate::density::SETTLE`]) while the
    /// only thing in the way is the memory VMs still launching are expected
    /// to take: a burst of creates then runs in waves as the first ones
    /// finish booting, instead of failing the ones past the limit. A refusal
    /// that would stand with those VMs counted at nothing (the host really is
    /// full, or out of CPUs) is returned at once.
    async fn admit_waiting(
        &self,
        name: &str,
        vcpus: u32,
        memory_mb: u64,
        shared: Option<PathBuf>,
    ) -> Result<()> {
        let deadline = std::time::Instant::now() + crate::density::SETTLE;
        loop {
            match self.admit_inner(name, vcpus, memory_mb, shared.clone()) {
                Err((_, true)) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err((e, transient)) => {
                    if transient {
                        tracing::warn!(name, %e, "VM not admitted after waiting for launching VMs");
                    }
                    return Err(e);
                }
                Ok(()) => return Ok(()),
            }
        }
    }

    #[cfg(test)]
    fn admit(&self, name: &str, vcpus: u32, memory_mb: u64, shared: Option<PathBuf>) -> Result<()> {
        self.admit_inner(name, vcpus, memory_mb, shared)
            .map_err(|(e, _)| e)
    }

    /// Admits a VM of `vcpus` and `memory_mb` (mapping `shared`, a snapshot
    /// RAM file) on this host now, or says why it does not fit
    /// ([`crate::density`]) and whether it would fit once the VMs still
    /// launching have settled. An admitted VM counts as pending until its
    /// launch finishes.
    fn admit_inner(
        &self,
        name: &str,
        vcpus: u32,
        memory_mb: u64,
        shared: Option<PathBuf>,
    ) -> std::result::Result<(), (VmmError, bool)> {
        // Measure before taking the lock: reading every running VM takes time
        // that grows with their number, and creates must not queue behind it.
        // VMs admitted meanwhile are in `pending`, which `decide` counts.
        let usage = self.measure_vms(Some(name));
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|p| p.at.elapsed() < crate::density::SETTLE && p.name != name);
        let shared_file_mb = shared
            .as_deref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(0, |m| (m.len() >> 20).min(memory_mb));
        let req = crate::density::Request {
            name: name.to_string(),
            vcpus,
            memory_mb,
            shared_file: shared,
            shared_file_mb,
        };
        match crate::density::decide(&self.cfg.density, &usage, &pending, &req) {
            Ok(a) => {
                pending.push(crate::density::Pending {
                    name: name.to_string(),
                    vcpus,
                    expected_mb: a.expected_mb,
                    at: std::time::Instant::now(),
                    shared_file: req.shared_file.clone(),
                    shared_file_mb: req.shared_file_mb,
                });
                Ok(())
            }
            Err(why) => {
                let transient = !pending.is_empty()
                    && crate::density::decide(&self.cfg.density, &usage, &[], &req).is_ok();
                let totals = crate::density::pending_totals(&usage, &pending);
                // A refusal that only waits for launching VMs to settle is
                // retried every 200 ms; say so once it is final.
                if transient {
                    tracing::debug!(name, %why, "VM waits for launching VMs to settle");
                } else {
                    tracing::warn!(
                        name,
                        %why,
                        running = usage.vms.len(),
                        private_mb = usage.vms.iter().map(|v| v.private_mb).sum::<u64>(),
                        shared_mb = usage.shared_mb(),
                        pending = pending.len(),
                        pending_mb = totals.mb,
                        "VM not admitted"
                    );
                }
                Err((VmmError::HostCapacity(why.0), transient))
            }
        }
    }

    /// `name` no longer holds back other VMs as pending: its launch
    /// finished (it counts at what it measures), or it is gone or failed.
    fn release(&self, name: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|p| p.name != name);
    }

    /// The running VMs of this runtime (and the RAM files they map),
    /// measured, leaving out `except`.
    fn measure_vms(&self, except: Option<&str>) -> crate::density::Usage {
        let vms: Vec<_> = self
            .all_states()
            .unwrap_or_default()
            .into_iter()
            .filter(|st| st.kind == EntryKind::Instance && Some(st.name.as_str()) != except)
            .filter_map(|st| {
                let pid = st.pid?;
                Some((st.name, pid, st.cpus, st.memory_mb, st.ram_file))
            })
            .collect();
        crate::density::measure(&vms)
    }

    /// What this runtime's VMs use now, with the VMs admitted but not yet
    /// settled, and the limits they are held to.
    pub fn density(&self) -> Density {
        let pending: Vec<crate::density::Pending> = {
            let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            p.retain(|p| p.at.elapsed() < crate::density::SETTLE);
            p.clone()
        };
        let usage = self.measure_vms(None);
        Density {
            headroom_mb: crate::density::headroom_mb(&self.cfg.density, &usage, &pending),
            typical_vm_mb: crate::density::typical_vm_mb(&usage, &pending),
            config: self.cfg.density.clone(),
            usage,
            pending,
        }
    }

    async fn launch_admitted(
        &self,
        st: &mut QemuState,
        spec: &StartSpec,
        boot: Boot,
        persist: bool,
    ) -> Result<()> {
        // Ports are leased inside one process, but another daemon (or any
        // program) can still take a probed port before QEMU binds it. That
        // failure is QEMU exiting at once on a bind error, so re-pick
        // fresh ports and launch again. A KVM guest whose vCPU never starts
        // is retried once under TCG.
        let mut attempt = 1;
        let mut boot = boot;
        loop {
            match self
                .launch_once(st, spec, boot.clone(), persist, false)
                .await
            {
                Err(VmmError::Command { stderr, .. })
                    if attempt < LAUNCH_PORT_ATTEMPTS && is_port_conflict(&stderr) =>
                {
                    tracing::warn!(sandbox = %st.name, attempt, "host port taken before qemu bound it; retrying");
                    attempt += 1;
                    // A cold boot that wanted its last run's ports (a resize)
                    // takes fresh ones rather than failing; a memory restore
                    // moving the same VM into a new process keeps trying them.
                    if !boot.incoming {
                        boot.keep_ports = false;
                    }
                }
                Err(e) if st.accel == "kvm" && self.cfg.accel.is_none() && kvm_stalled(&e) => {
                    tracing::warn!(
                        sandbox = %st.name,
                        "KVM did not start a vCPU (QEMU stopped answering QMP; see `dmesg` for kvm errors), so retrying under slow TCG emulation"
                    );
                    if let Some(pid) = st.pid {
                        let _ = host::run("kill", &["-9", &pid.to_string()]).await;
                        if !Self::wait_exit(pid, Duration::from_secs(5)).await {
                            return Err(e);
                        }
                    }
                    st.pid = None;
                    return self.launch_once(st, spec, boot, persist, true).await;
                }
                other => return other,
            }
        }
    }

    async fn launch_once(
        &self,
        st: &mut QemuState,
        spec: &StartSpec,
        boot: Boot,
        persist: bool,
        force_tcg: bool,
    ) -> Result<()> {
        // Held until this launch's swtpm is running (and so visible to the
        // copy checks) or its clean-up is done, however the start ends.
        let busy = self.tpm_claim(st, true).await?;
        let bin = self.binary(st.arch).await?;
        let dir = self.run_dir(st);
        let tpm_dir = dir.join("tpm");
        let tpm_env: Option<(PathBuf, PathBuf)> = if st.tpm {
            #[cfg(not(unix))]
            return Err(VmmError::invalid("TPM requires a Unix host"));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let bin = host::which("swtpm").ok_or_else(|| {
                    VmmError::missing(
                        "swtpm",
                        "install it (macOS: brew install swtpm; Debian/Ubuntu: sudo apt-get install swtpm) and add it to PATH",
                    )
                })?;
                self.stop_tpm(st).await?;
                std::fs::create_dir_all(&tpm_dir)?;
                std::fs::set_permissions(&tpm_dir, std::fs::Permissions::from_mode(0o700))?;
                Some((bin, tpm_dir.clone()))
            }
        } else {
            None
        };
        let mut guest_ports: Vec<u16> = spec.ports.clone();
        if st.ssh.is_some() && !guest_ports.contains(&22) {
            guest_ports.push(22);
        }
        // Held until QEMU is up and owns the sockets (dropped on return).
        let mut lease = host::PortLease::default();
        let kept_ports = boot.keep_ports
            && st.qmp_port.is_some()
            && guest_ports.iter().all(|g| st.ports.contains_key(g));
        st.headless = boot.headless.unwrap_or_else(|| no_desktop(spec));
        let qmp_port = if kept_ports {
            st.qmp_port.expect("checked")
        } else {
            let mut free = lease.ports(guest_ports.len() + 1)?;
            let qmp_port = free.pop().expect("allocated");
            st.ports = guest_ports.iter().copied().zip(free).collect();
            st.qmp_port = Some(qmp_port);
            st.vnc_display = if self.cfg.vnc && !st.headless {
                lease.vnc_display(0)
            } else {
                None
            };
            qmp_port
        };
        // An egress policy that blocks anything: the VM runs inside its
        // own network namespace, where the policy is enforced
        // ([`netns_script`]).
        let netns_rules = match st.egress.as_ref() {
            Some(p) => Some(crate::egress::check(BackendKind::Qemu, p)?)
                .filter(crate::egress::Rules::blocks_anything),
            None => None,
        };
        st.accel = if force_tcg {
            "tcg".into()
        } else {
            self.accel_for(st.arch, &host::kvm_usable())?
        };
        st.cpus = spec.cpus;
        st.memory_mb = spec.memory_mb;
        st.restrict_network = spec.restrict_network;
        if spec.gpu.is_some() {
            st.gpu = spec.gpu.clone();
        }
        let virgl_render_node = match st.gpu.as_deref() {
            None => None,
            Some(crate::gpu::VIRGL) => {
                let host = virgl_host(&bin).await;
                let support = crate::gpu::qemu_support(std::env::consts::OS, &host);
                support.pick(crate::gpu::VIRGL).map_err(VmmError::invalid)?;
                host.render_node.map(PathBuf::from)
            }
            Some(_) => {
                return Err(VmmError::Unsupported {
                    backend: "qemu",
                    op: "a GPU other than `virgl`",
                });
            }
        };
        if !spec.extra_args.is_empty() {
            st.extra_args = spec.extra_args.clone();
        }
        st.paused = false;
        let anonymous = matches!(boot.memory, Memory::Ram);
        st.ram_file = match &boot.memory {
            Memory::File { path, .. } => Some(path.clone()),
            Memory::Ram => None,
        };
        st.free_page_reporting = boot
            .free_page_reporting
            .unwrap_or(self.cfg.free_page_reporting && anonymous && st.os == GuestOs::Linux);
        if st.free_page_reporting && !anonymous {
            return Err(VmmError::invalid(format!(
                "'{}' would restore a snapshot with free page reporting over a shared RAM \
                 file, where QEMU's discards would erase pages its other VMs read; such a \
                 snapshot must keep its RAM in its stream",
                st.name
            )));
        }

        // The bulk-lane disk: a Linux VM that boots cold gets one; a memory
        // restore keeps whatever the snapshot's device set holds.
        if !boot.incoming {
            st.bulk = st.os == GuestOs::Linux && crate::bulk::enabled();
        }
        let bulk_disk = if st.bulk {
            match crate::bulk::prepare(&dir) {
                Ok(p) => Some(p),
                Err(e) if boot.incoming => return Err(e),
                Err(e) => {
                    tracing::debug!(error = %e, "no bulk lane for this VM");
                    st.bulk = false;
                    None
                }
            }
        } else {
            None
        };

        let pidfile = dir.join("qemu.pid");
        let _ = std::fs::remove_file(&pidfile);
        self.drop_aliases(None);
        let alias = match is_pool_member(&st.name) && cfg!(unix) {
            true => {
                self.point_alias(&st.name, &dir)?;
                Some(self.run_link(&st.name))
            }
            false => None,
        };
        let mut cfg = args::LaunchConfig {
            binary: bin.clone(),
            name: st.name.clone(),
            arch: st.arch,
            os: st.os,
            accel: st.accel.clone(),
            cpus: st.cpus,
            memory_mb: st.memory_mb,
            disk: st.disk.clone(),
            disk_format: st.disk_format.clone(),
            seed_iso: st.seed_iso.clone(),
            install_iso: st.install_iso.clone(),
            tpm_socket: tpm_env.as_ref().map(|_| tpm_dir.join("swtpm.sock")),
            firmware: st.firmware.clone(),
            forwards: st.ports.iter().map(|(g, h)| (*h, *g)).collect(),
            restrict_network: st.restrict_network,
            vnc_display: st.vnc_display,
            qmp_port,
            serial_log: Some(dir.join("serial.log")),
            pidfile: Some(pidfile.clone()),
            daemonize: false,
            loadvm: None,
            memory: boot.memory,
            incoming: boot.incoming,
            virgl_render_node,
            free_page_reporting: st.free_page_reporting,
            headless: st.headless,
            bulk: bulk_disk,
            extra: st.extra_args.clone(),
        };
        if let Some(alias) = &alias {
            cfg.address_through(&dir, alias);
        }
        let argv = args::build(&cfg);
        // Who started it: a VM nothing records is only an orphan once this
        // process is gone ([`crate::hostvms::Launcher`]).
        crate::hostvms::Launcher::here().write(&dir);
        std::fs::write(
            dir.join("cmdline"),
            format!("{} {}\n", bin.display(), argv.join(" ")),
        )?;
        tracing::info!(sandbox = %st.name, accel = %st.accel, "starting qemu");
        // A create cancelled between the launch and the moment its pid is
        // recorded would leave a running QEMU nothing knows about: the
        // guard ends whatever its pid file names unless the start finished.
        let mut launch_guard = LaunchGuard::new();
        launch_guard.busy = busy.clone();
        // Each launch has its own swtpm record, so cleanup can never reach
        // another launch's TPM through a shared pidfile.
        let record = tpm_env
            .as_ref()
            .map(|_| dir.join(format!("swtpm.{}.pid", unique_suffix())));

        // Launch detached: on Unix a throwaway `sh` backgrounds QEMU and exits,
        // so QEMU is re-parented to init (no zombie in long-lived callers) and
        // survives this process, while its stderr (crash reports, hvf/tcg
        // errors) goes to qemu.log instead of /dev/null as with -daemonize.
        let log_path = dir.join("qemu.log");
        st.netns = false;
        st.pasta_pid = None;
        #[cfg(unix)]
        {
            // In its namespace: pasta runs the script that installs the
            // rules and then execs QEMU (so the rules are there before
            // the guest's first instruction).
            let (program, args): (PathBuf, Vec<String>) = match &netns_rules {
                Some(rules) => {
                    let pasta = host::which("pasta").ok_or_else(|| {
                        VmmError::missing(
                            "pasta",
                            "install passt (Ubuntu: sudo apt-get install passt): a QEMU VM with \
                             an egress policy runs in a network namespace pasta creates",
                        )
                    })?;
                    std::fs::write(dir.join(NETNS_SCRIPT), netns_script(rules))?;
                    let mut ports: Vec<u16> = st.ports.values().copied().collect();
                    ports.push(qmp_port);
                    if let Some(d) = st.vnc_display {
                        ports.push(5900 + d);
                    }
                    let mut a = netns_pasta_args(&ports);
                    a.push(NETNS_SCRIPT.into());
                    a.push(bin.display().to_string());
                    a.extend(argv.iter().cloned());
                    (pasta, a)
                }
                None => (bin.clone(), argv.clone()),
            };
            // The shell runs to completion even when this future is dropped;
            // then the undo ends exactly this launch's QEMU and TPM record.
            let tpm_log = tpm_env.as_ref().map_or(String::new(), |_| {
                format!("; see {}", dir.join("swtpm.log").display())
            });
            let (program2, args2, log2, dir2) =
                (program.clone(), args.clone(), log_path.clone(), dir.clone());
            let (record2, tpm_dir2) = (record.clone(), tpm_dir.clone());
            let (record3, tpm_dir3) = (record.clone(), tpm_dir.clone());
            let create = async move {
                let mut command = tokio::process::Command::new("/bin/sh");
                if let (Some((swtpm, tpm_dir)), Some(record)) = (&tpm_env, &record2) {
                    command
                        .env("CUA_SWTPM", swtpm)
                        .env("CUA_TPM_DIR", tpm_dir)
                        .env("CUA_TPM_RECORD", record);
                }
                let out = command
                    .arg("-c")
                    .arg(if tpm_env.is_some() {
                        TPM_LAUNCH
                    } else {
                        QEMU_LAUNCH
                    })
                    .arg(&program2)
                    .args(&args2)
                    .env("CUA_QEMU_LOG", &log2)
                    .current_dir(&dir2)
                    .output()
                    .await;
                let failed = match out {
                    Ok(out) => match String::from_utf8_lossy(&out.stdout).trim().parse::<u32>() {
                        Ok(pid) => return Ok::<u32, VmmError>(pid),
                        Err(e) => VmmError::other(format!(
                            "could not launch {}: {e}{tpm_log}",
                            program2.display()
                        )),
                    },
                    Err(e) => e.into(),
                };
                if let Some(record) = &record2 {
                    let _ = end_record(record, &tpm_dir2).await;
                }
                Err(failed)
            };
            let busy2 = busy.clone();
            let undo = move |pid: u32| async move {
                let _busy = busy2;
                end_launched(pid).await;
                if let Some(record) = &record3 {
                    let _ = end_record(record, &tpm_dir3).await;
                }
            };
            let pid = crate::cleanup::complete_or_undo(create, undo).await?;
            launch_guard.pid = Some(pid);
            launch_guard.tpm = record.map(|r| (r, tpm_dir.clone()));
            st.pid = Some(pid);
            if netns_rules.is_some() {
                st.pasta_pid = Some(pid);
                st.netns = true;
            }
        }
        #[cfg(not(unix))]
        {
            // Off Linux a policy is refused before this (egress::check).
            debug_assert!(netns_rules.is_none());
            let _ = &record; // TPM VMs are refused above off Unix.
            let log = std::fs::File::create(&log_path)?;
            let child = std::process::Command::new(&bin)
                .args(&argv)
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()?;
            launch_guard.pid = Some(child.id());
            st.pid = Some(child.id());
        }
        if persist {
            self.save(st)?;
        }

        let pid = st.pid.expect("set above");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut q = loop {
            if !host::pid_alive(pid) {
                return Err(VmmError::Command {
                    cmd: format!("{} (see {})", bin.display(), dir.join("cmdline").display()),
                    code: None,
                    stderr: crate::exec::tail(
                        &std::fs::read_to_string(&log_path).unwrap_or_default(),
                        4000,
                    ),
                });
            }
            match QmpClient::connect(&format!("127.0.0.1:{qmp_port}")).await {
                Ok(q) => {
                    if st.netns {
                        // QEMU is pasta's child: that is the VM's process.
                        st.pid = Some(netns_child(pid).ok_or_else(|| {
                            VmmError::other("cannot find the QEMU process under pasta")
                        })?);
                        if persist {
                            self.save(st)?;
                        }
                    }
                    break q;
                }
                Err(e) if tokio::time::Instant::now() >= deadline && host::pid_alive(pid) => {
                    return Err(e);
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        let status = q.status().await?;
        let want = if boot.incoming {
            "inmigrate"
        } else {
            "running"
        };
        if status != want {
            return Err(VmmError::other(format!(
                "qemu started but VM status is '{status}'"
            )));
        }
        launch_guard.disarm();
        Ok(())
    }

    fn instance(&self, st: &QemuState) -> Instance {
        Instance {
            name: st.name.clone(),
            backend: BackendKind::Qemu,
            status: Self::status_of(st),
            endpoints: self.endpoints_of(st),
            isolation: Isolation::Vm {
                accel: st.accel.clone(),
            },
            arch: Some(st.arch),
        }
    }

    fn endpoints_of(&self, st: &QemuState) -> Endpoints {
        let host = "127.0.0.1".to_string();
        Endpoints {
            host: host.clone(),
            ports: st.ports.clone(),
            vnc: st.vnc_display.map(|d| VncEndpoint {
                host: host.clone(),
                port: 5900 + d,
                password: None,
            }),
            qmp: st.qmp_port.map(|p| HostPort {
                host: host.clone(),
                port: p,
            }),
            serial_log: Some(self.dir(&st.name).join("serial.log")),
            bulk: st.bulk.then(|| self.dir(&st.name).join(crate::bulk::FILE)),
            ssh: match (&st.ssh, st.ports.get(&22)) {
                (Some(a), Some(p)) => Some(SshEndpoint {
                    host: host.clone(),
                    port: *p,
                    user: a.user.clone(),
                    private_key: Some(a.private_key.clone()),
                    password: a.password.clone(),
                }),
                _ => None,
            },
            container_id: None,
        }
    }

    /// Open a QMP session to a running instance.
    pub async fn qmp(&self, name: &str) -> Result<QmpClient> {
        let st = self.load(name)?;
        if !Self::is_running(&st) {
            return Err(VmmError::invalid(format!(
                "sandbox '{name}' is not running"
            )));
        }
        let port = st
            .qmp_port
            .ok_or_else(|| VmmError::other("no QMP port recorded"))?;
        QmpClient::connect(&format!("127.0.0.1:{port}")).await
    }

    /// Restore an internal snapshot taken by [`Runtime::checkpoint`] on a
    /// running instance (in place, RAM included).
    pub async fn restore(&self, name: &str, snapshot: &str) -> Result<()> {
        refuse_tpm(
            &self.load(name)?,
            "an in-place restore (restore by forking a stopped checkpoint)",
        )?;
        let mut q = self.qmp(name).await?;
        q.loadvm(snapshot).await?;
        let mut st = self.load(name)?;
        st.paused = false;
        self.save(&st)
    }

    /// Move an instance's writable disk into the shared layer store and give
    /// the instance a fresh overlay on top of it. Returns the frozen layer.
    async fn freeze(&self, st: &mut QemuState) -> Result<PathBuf> {
        let layers = self.layers_dir();
        std::fs::create_dir_all(&layers)?;
        let layer = layers.join(format!("{}-{}.qcow2", st.name, unique_suffix()));
        std::fs::rename(&st.disk, &layer)?;
        if let Err(e) = img::create_overlay(&layer, &st.disk).await {
            // Put the disk back so the instance is not left without one.
            let _ = std::fs::rename(&layer, &st.disk);
            return Err(e);
        }
        Ok(layer)
    }

    fn copy_vars(&self, from: &Firmware, to_dir: &Path) -> Result<Firmware> {
        Ok(match from {
            Firmware::UefiPflash {
                code,
                vars,
                vars_format,
            } => {
                std::fs::create_dir_all(to_dir)?;
                let dst = to_dir.join("efivars.qcow2");
                std::fs::copy(vars, &dst)?;
                Firmware::UefiPflash {
                    code: code.clone(),
                    vars: dst,
                    vars_format: vars_format.clone(),
                }
            }
            other => other.clone(),
        })
    }

    /// Copy the TPM state of `from` (stopped) into `to_dir/tpm`. Files are
    /// copied, never linked; anything but plain files is an error.
    fn copy_tpm(&self, from: &QemuState, to_dir: &Path) -> Result<()> {
        if !from.tpm {
            return Ok(());
        }
        // The disk of a checkpoint lives in the layer store: use the entry.
        let src = self.dir(&from.name).join("tpm");
        self.tpm_stopped(from)?;
        let dst = to_dir.join("tpm");
        std::fs::create_dir_all(&dst)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o700))?;
        }
        for entry in std::fs::read_dir(&src)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == "swtpm.sock" || name == ".lock" {
                continue;
            }
            if !entry.file_type()?.is_file() {
                return Err(VmmError::other(format!(
                    "TPM state of '{}' holds {}, which is not a plain file",
                    from.name,
                    name.to_string_lossy()
                )));
            }
            std::fs::copy(entry.path(), dst.join(&name))?;
        }
        Ok(())
    }

    /// Collects unreferenced layers after a removal. The collection reads
    /// every entry's backing chain, so deleting N VMs one collection each is
    /// quadratic. Deletes that wait together share collections: a delete
    /// returns once a collection that began after its own removal has
    /// finished (its own, or one a concurrent delete ran).
    async fn collect_layers(&self) -> Result<()> {
        use std::sync::atomic::Ordering::SeqCst;
        let ticket = self.gc_asked.fetch_add(1, SeqCst) + 1;
        let mut done = self.gc_done.lock().await;
        if *done >= ticket {
            return Ok(());
        }
        // Every removal up to `upto` happened before this collection starts.
        let upto = self.gc_asked.load(SeqCst);
        let _g = self.lock.lock().await;
        self.gc_runs.fetch_add(1, SeqCst);
        self.gc_layers().await?;
        *done = upto;
        Ok(())
    }

    /// Delete layer files no remaining entry references.
    async fn gc_layers(&self) -> Result<()> {
        let layers = self.layers_dir();
        let Ok(entries) = std::fs::read_dir(&layers) else {
            return Ok(());
        };
        let mut used: BTreeSet<PathBuf> = BTreeSet::new();
        // Overlays name their backing by path: header reads instead of a
        // `qemu-img info` per entry (a delete of one of N forks would
        // otherwise run N of them, under the lock). A fork source's empty
        // fork overlay holds its layer too.
        for st in self.all_states()? {
            let template = self.dir(&st.name).join(FORK_OVERLAY);
            for top in [st.disk.clone(), template] {
                if !top.exists() {
                    continue;
                }
                for f in std::iter::once(top.clone()).chain(backing_files(&top)) {
                    used.insert(std::fs::canonicalize(&f).unwrap_or(f));
                }
            }
        }
        for e in entries.flatten() {
            let p = std::fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
            if !used.contains(&p) {
                tracing::debug!(layer = %p.display(), "removing unreferenced layer");
                let _ = std::fs::remove_file(&p);
            }
        }
        self.gc_imported(&self.all_states()?);
        Ok(())
    }

    fn all_states(&self) -> Result<Vec<QemuState>> {
        let Ok(rd) = std::fs::read_dir(&self.cfg.root) else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == LAYERS
                || name == RUN_LINKS
                || name == export::IMPORTED
                || !e.path().join("state.json").exists()
            {
                continue;
            }
            if let Ok(st) = self.load(&name) {
                out.push(st);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn wait_exit(pid: u32, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if !host::pid_alive(pid) {
                return true;
            }
            // Short: a delete waits here, so the poll interval bounds how
            // many VMs per second can be stopped.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        !host::pid_alive(pid)
    }

    /// The directory a VM runs in (where its pidfiles and `tpm/` live).
    fn run_dir(&self, st: &QemuState) -> PathBuf {
        st.disk
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.dir(&st.name))
    }

    /// End every swtpm this VM launched; a record goes only once its swtpm
    /// has exited. The first failure is returned after trying them all.
    async fn stop_tpm(&self, st: &QemuState) -> Result<()> {
        let tpm_dir = self.run_dir(st).join("tpm");
        let mut first = None;
        for record in tpm_records(&self.run_dir(st)) {
            if let Err(e) = end_record(&record, &tpm_dir).await {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// Marks the TPM of `st` busy (a launch or a copy); `wait` retries while
    /// a previous launch's clean-up still holds it.
    async fn tpm_claim(&self, st: &QemuState, wait: bool) -> Result<Option<Arc<TpmBusy>>> {
        if !st.tpm {
            return Ok(None);
        }
        for _ in 0..100 {
            {
                let mut set = self.tpm_busy.lock().unwrap_or_else(|e| e.into_inner());
                if set.insert(st.name.clone()) {
                    return Ok(Some(Arc::new(TpmBusy {
                        set: self.tpm_busy.clone(),
                        name: st.name.clone(),
                    })));
                }
            }
            if !wait {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(VmmError::invalid(format!(
            "the TPM of '{}' {TPM_CLAIM_REFUSED}",
            st.name
        )))
    }

    /// Refuse while any swtpm of this VM is alive (or cannot be told apart
    /// from one). Signals and removes nothing.
    fn tpm_stopped(&self, st: &QemuState) -> Result<()> {
        if !st.tpm {
            return Ok(());
        }
        let run = self.run_dir(st);
        let tpm_dir = run.join("tpm");
        for record in tpm_records(&run) {
            if let Some(pid) = record_pid(&record)
                && swtpm_state(pid, &record, &tpm_dir) != Tpm::Gone
            {
                return Err(VmmError::invalid(format!(
                    "the TPM of '{}' is still running (swtpm {pid}); stop the VM first",
                    st.name
                )));
            }
        }
        Ok(())
    }

    async fn force_stop(&self, st: &mut QemuState, graceful: Duration) -> Result<()> {
        // A stopped VM holds no memory, whatever it was still expected to
        // grow to.
        self.release(&st.name);
        let Some(pid) = st.pid.filter(|p| host::pid_alive(*p)) else {
            st.pid = None;
            self.drop_aliases(Some(&self.dir(&st.name)));
            Self::end_netns(st);
            return self.stop_tpm(st).await;
        };
        if !graceful.is_zero() {
            let wait = if let Ok(mut q) = self.qmp(&st.name).await {
                let _ = q.system_powerdown().await;
                graceful
            } else {
                // No monitor to ask for a shutdown: SIGTERM is QEMU's clean
                // quit, and waiting the whole grace for nothing helps no one.
                host::signal(pid, "TERM");
                Duration::from_secs(5)
            };
            if Self::wait_exit(pid, wait).await {
                st.pid = None;
                self.drop_aliases(Some(&self.dir(&st.name)));
                Self::end_netns(st);
                return self.stop_tpm(st).await;
            }
        }
        let quit = match self.qmp(&st.name).await {
            Ok(mut q) => q.quit().await.is_ok(),
            Err(_) => false,
        };
        if !quit {
            // No monitor to ask (its port is gone, or it never listened):
            // QEMU quits cleanly on SIGTERM too.
            host::signal(pid, "TERM");
        }
        if !Self::wait_exit(pid, Duration::from_secs(5)).await {
            host::signal(pid, "KILL");
            Self::wait_exit(pid, Duration::from_secs(5)).await;
        }
        st.pid = None;
        self.drop_aliases(Some(&self.dir(&st.name)));
        Self::end_netns(st);
        self.stop_tpm(st).await
    }

    /// pasta exits with QEMU; one left behind (a VM killed with it) goes.
    fn end_netns(st: &mut QemuState) {
        if let Some(p) = st.pasta_pid.take()
            && host::pid_alive(p)
        {
            host::signal(p, "TERM");
        }
        st.netns = false;
    }
}

impl QemuRuntime {
    /// Stops the VM `name` (when it runs or is paused) and boots it again
    /// from its own disk, with `shape`'s vCPUs, memory and disk where given
    /// and its own otherwise, and the configuration it first started with.
    /// The guest's disk, ports and token are kept; its memory and running
    /// processes are not. A VM forked from a memory snapshot boots cold.
    async fn boot_from_disk(&self, name: &str, shape: &Resize) -> Result<Instance> {
        let mut st = self.load(name)?;
        {
            let _g = self.lock.lock().await;
            // The guest is shut down, not cut off: what it wrote in its last
            // seconds (host keys, the token file) must reach the disk. A
            // paused VM runs again first so that it can.
            if st.paused
                && let Ok(mut q) = self.qmp(name).await
            {
                let _ = q.cont().await;
            }
            let graceful = self.cfg.graceful_stop.max(Duration::from_secs(30));
            self.force_stop(&mut st, graceful).await?;
            st.paused = false;
            // A memory snapshot holds the old shape's machine.
            st.restore_from = None;
            self.save(&st)?;
        }
        let dir = self.dir(name);
        let boot = Self::read_boot_config(&dir);
        let mut spec = StartSpec::new(name, ImageSource::Existing);
        spec.os = st.os;
        spec.arch = Some(st.arch);
        spec.cpus = shape.cpus.unwrap_or(st.cpus);
        spec.memory_mb = shape.memory_mb.unwrap_or(st.memory_mb);
        spec.disk_size_gb = shape.disk_gb;
        // 22 is added again for managed SSH.
        spec.ports = st
            .ports
            .keys()
            .copied()
            .filter(|p| !(*p == 22 && st.ssh.is_some()))
            .collect();
        spec.env = boot.env;
        if let Some(t) = self.guest_token(&dir) {
            spec.env
                .entry(crate::cloudinit::ENV_TOKEN_VAR.into())
                .or_insert(t);
        }
        spec.command = boot.command;
        spec.cloud_init_user_data = boot.cloud_init_user_data;
        spec.ssh = st.ssh.clone();
        spec.restrict_network = st.restrict_network;
        spec.extra_args = st.extra_args.clone();
        spec.gpu = st.gpu.clone();
        spec.probes = shape.probes.clone();
        spec.labels.insert(KEEP_SEED_LABEL.into(), "1".into());
        spec.labels.insert(KEEP_PORTS_LABEL.into(), "1".into());
        self.start(&spec).await
    }

    /// Readiness probes with a slirp-aware TCP check: user-mode NAT accepts
    /// host connections before the guest does, so a TCP probe connects and
    /// then asks QEMU (`info usernet`) whether *the guest side* of that
    /// connection reached ESTABLISHED.
    async fn wait_probes(&self, st: &QemuState, ep: &Endpoints, spec: &StartSpec) -> Result<()> {
        let deadline = tokio::time::Instant::now() + spec.ready_timeout;
        for probe in &spec.probes {
            loop {
                let ok = match probe {
                    Probe::Tcp { port } => self.guest_tcp_established(st, *port).await?,
                    other => crate::probe::check(ep, other).await?,
                };
                if ok {
                    break;
                }
                if !Self::is_running(st) {
                    return Err(VmmError::other(format!(
                        "qemu for '{}' exited while waiting for {probe:?} (see {})",
                        st.name,
                        self.dir(&st.name).join("qemu.log").display()
                    )));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(VmmError::Timeout {
                        name: st.name.clone(),
                        secs: spec.ready_timeout.as_secs(),
                        detail: format!("probe {probe:?} never passed"),
                    });
                }
                tokio::time::sleep(PROBE_INTERVAL).await;
            }
        }
        Ok(())
    }

    async fn guest_tcp_established(&self, st: &QemuState, guest_port: u16) -> Result<bool> {
        let host_port = *st.ports.get(&guest_port).ok_or_else(|| {
            VmmError::invalid(format!(
                "probe port {guest_port} is not published by this sandbox"
            ))
        })?;
        let Ok(Ok(mut stream)) = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::TcpStream::connect(("127.0.0.1", host_port)),
        )
        .await
        else {
            return Ok(false);
        };
        // While our connection is open, slirp lists the guest leg of every
        // accepted forward as `TCP[<state>] … 127.0.0.1 <host_port> … <guest_port>`;
        // it only reaches ESTABLISHED once the guest accepted.
        let port = st
            .qmp_port
            .ok_or_else(|| VmmError::other("no QMP port recorded"))?;
        let mut q = QmpClient::connect(&format!("127.0.0.1:{port}")).await?;
        // The guest leg is up within a round trip or two when the guest
        // listens; when it refuses, slirp closes the host side.
        let mut buf = [0u8; 1];
        for _ in 0..25 {
            let table = q.hmp("info usernet").await?;
            if usernet_established(&table, host_port, guest_port) {
                return Ok(true);
            }
            match tokio::time::timeout(
                Duration::from_millis(20),
                tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
            )
            .await
            {
                // Closed or reset: the guest is not listening (yet).
                Ok(Ok(0)) | Ok(Err(_)) => return Ok(false),
                // The guest's service spoke first: it accepted.
                Ok(Ok(_)) => return Ok(true),
                Err(_) => {}
            }
        }
        Ok(false)
    }
}

/// Guest OS and firmware for a new x86 instance. What the caller said wins;
/// what it left open comes from the disk's partition table, so any
/// containerDisk boots without per-image flags: a GPT with an ESP and no BIOS
/// boot partition needs UEFI, and Microsoft partitions mean a Windows guest.
async fn boot_hints(
    backing: Option<&Path>,
    arch: Arch,
    spec: &StartSpec,
) -> (GuestOs, Option<BootFirmware>) {
    let (mut os, mut want) = (spec.os, spec.firmware);
    if arch != Arch::X86_64 || (want.is_some() && os != GuestOs::Linux) {
        return (os, want);
    }
    let Some(disk) = backing else {
        return (os, want);
    };
    match img::partition_layout(disk).await {
        Ok(Some(layout)) => {
            if layout.windows && os == GuestOs::Linux {
                tracing::info!(disk = %disk.display(), "Windows partitions found; booting as a Windows guest");
                os = GuestOs::Windows;
            }
            if want.is_none() && layout.efi_only() {
                tracing::info!(disk = %disk.display(), "GPT disk without a BIOS boot partition; booting UEFI");
                want = Some(BootFirmware::Efi);
            }
        }
        Ok(None) => {}
        Err(e) => tracing::debug!(error = %e, "could not read the partition table"),
    }
    (os, want)
}

/// Whether HMP `info usernet` shows an ESTABLISHED guest connection for the
/// `host_port → guest_port` forward.
pub fn usernet_established(table: &str, host_port: u16, guest_port: u16) -> bool {
    let (h, g) = (host_port.to_string(), guest_port.to_string());
    table.lines().any(|l| {
        let cols: Vec<&str> = l.split_whitespace().collect();
        cols.first() == Some(&"TCP[ESTABLISHED]")
            && cols.get(3) == Some(&h.as_str())
            && cols.get(5) == Some(&g.as_str())
    })
}

/// The backing files of qcow2 image `path`, nearest first, read from the
/// image headers (no `qemu-img`; a missing or non-qcow2 file ends the
/// chain).
pub fn backing_files(path: &Path) -> Vec<PathBuf> {
    use std::io::{Read, Seek, SeekFrom};
    let mut out = Vec::new();
    let mut cur = path.to_path_buf();
    for _ in 0..32 {
        let Ok(mut f) = std::fs::File::open(&cur) else {
            break;
        };
        let mut h = [0u8; 20];
        if f.read_exact(&mut h).is_err() || h[..4] != *b"QFI\xfb" {
            break;
        }
        let off = u64::from_be_bytes(h[8..16].try_into().expect("8 bytes"));
        let len = u32::from_be_bytes(h[16..20].try_into().expect("4 bytes")) as usize;
        if off == 0 || len == 0 || len > 4096 {
            break;
        }
        let mut name = vec![0u8; len];
        if f.seek(SeekFrom::Start(off)).is_err() || f.read_exact(&mut name).is_err() {
            break;
        }
        let name = PathBuf::from(String::from_utf8_lossy(&name).into_owned());
        let next = if name.is_absolute() {
            name
        } else {
            cur.parent().map(|d| d.join(&name)).unwrap_or(name)
        };
        out.push(next.clone());
        cur = next;
    }
    out
}

/// A fork source's empty overlay on its disk, copied to start each fork.
const FORK_OVERLAY: &str = "fork-overlay.qcow2";

/// Pause between readiness checks (each is a host connect plus an
/// `info usernet`, cheap enough to run often).
const PROBE_INTERVAL: Duration = Duration::from_millis(50);

/// How [`QemuRuntime::launch`] boots: where guest RAM lives, and whether
/// the VM waits for its state instead of booting.
#[derive(Clone, Debug, Default)]
pub(crate) struct Boot {
    pub memory: Memory,
    pub incoming: bool,
    /// The free-page-reporting device of the snapshot being restored
    /// (`Some`), or the runtime's choice for a boot (`None`).
    pub free_page_reporting: Option<bool>,
    /// The snapshot's display hardware (`Some(true)`: none), or the spec's
    /// (`None`: headless when its guest serves no desktop).
    pub headless: Option<bool>,
    /// Keep the host ports (forwards, QMP, VNC) of the last run: the same
    /// VM moved into a new process.
    pub keep_ports: bool,
}

/// The script pasta runs in a VM's namespace ([`netns_script`]).
#[cfg_attr(not(unix), allow(dead_code))]
const NETNS_SCRIPT: &str = "netns.sh";

/// The namespace's own address on pasta's link and its gateway, and where
/// pasta forwards DNS (the same as a microVM's).
#[cfg_attr(not(unix), allow(dead_code))]
const NS_IP: &str = "198.18.0.2";
#[cfg_attr(not(unix), allow(dead_code))]
const NS_GATEWAY: &str = "198.18.0.1";
const DNS_FWD: std::net::Ipv4Addr = std::net::Ipv4Addr::new(169, 254, 1, 53);
/// The resolver QEMU's user-mode network gives the guest.
const SLIRP_DNS: std::net::Ipv4Addr = std::net::Ipv4Addr::new(10, 0, 2, 3);

/// pasta's arguments for a VM's namespace: its host ports (`ports`, on
/// loopback) forwarded to the same ports in the namespace, DNS to the
/// host's resolver through [`DNS_FWD`], no way to the host's own loopback,
/// then the script.
#[cfg_attr(not(unix), allow(dead_code))]
fn netns_pasta_args(ports: &[u16]) -> Vec<String> {
    let mut a: Vec<String> = [
        "--config-net",
        "-q",
        "-f",
        "-u",
        "none",
        "-T",
        "none",
        "-U",
        "none",
        "--dns-forward",
        "169.254.1.53",
        "--no-map-gw",
        "--address",
        NS_IP,
        "--netmask",
        "30",
        "--gateway",
        NS_GATEWAY,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for p in ports {
        a.extend(["-t".into(), format!("127.0.0.1/{p}:{p}")]);
    }
    a.extend(["--".into(), "/bin/sh".into()]);
    a
}

/// The script run in a VM's namespace before QEMU: the egress rules on
/// `OUTPUT` (QEMU's user-mode network opens the guest's connections from
/// QEMU itself), a private `/etc/resolv.conf` naming pasta's DNS forward
/// (whose verdict is the policy's for the guest's resolver, `10.0.2.3`),
/// then QEMU (`$@`).
#[cfg_attr(not(unix), allow(dead_code))]
fn netns_script(rules: &crate::egress::Rules) -> String {
    let resolver = crate::egress::Resolver {
        guest: SLIRP_DNS,
        forward: DNS_FWD,
    };
    format!(
        "{rules}printf 'nameserver {DNS_FWD}\\n' > \"$PWD/resolv.conf\"\n\
         exec unshare -m sh -c 'mount --bind \"$0\" /etc/resolv.conf && exec \"$@\"' \"$PWD/resolv.conf\" \"$@\"\n",
        rules = crate::egress::ns_script(rules, crate::egress::NsHook::Output, Some(resolver)),
    )
}

/// The rules of a running namespaced VM, refilled ([`netns_script`]'s).
fn netns_rules_script(rules: &crate::egress::Rules) -> String {
    crate::egress::ns_script(
        rules,
        crate::egress::NsHook::Output,
        Some(crate::egress::Resolver {
            guest: SLIRP_DNS,
            forward: DNS_FWD,
        }),
    )
}

/// The first child of process `pid` (pasta's command).
#[cfg_attr(not(unix), allow(dead_code))]
fn netns_child(pid: u32) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|c| c.parse().ok()))
}

/// Whether `spec`'s guest serves no desktop (`CUA_ENV_NO_DESKTOP`), so its
/// VM needs no display or input hardware.
pub fn no_desktop(spec: &StartSpec) -> bool {
    spec.env
        .get("CUA_ENV_NO_DESKTOP")
        .map(|v| v.trim().to_ascii_lowercase())
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

/// What a QEMU runtime's VMs use and may still use ([`QemuRuntime::density`]).
#[derive(Clone, Debug)]
pub struct Density {
    pub config: crate::density::DensityConfig,
    pub usage: crate::density::Usage,
    pub pending: Vec<crate::density::Pending>,
    /// VM memory (MiB) the host takes now; `None` when the OS does not say.
    pub headroom_mb: Option<u64>,
    /// The median private memory of a running VM (MiB).
    pub typical_vm_mb: Option<u64>,
}

const LAUNCH_PORT_ATTEMPTS: u32 = 4;

/// Whether QEMU's stderr says a host port it was handed is already bound.
fn is_port_conflict(stderr: &str) -> bool {
    stderr.contains("Address already in use")
        || stderr.contains("Failed to find an available port")
        || stderr.contains("Could not set up host forwarding rule")
}

fn kvm_stalled(e: &VmmError) -> bool {
    matches!(e, VmmError::Qmp(m) if m.contains("timed out"))
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// How QEMU gets onto a Windows host: cua does not install it there.
pub const WINDOWS_QEMU_HINT: &str = "Windows VMs on a Windows PC need QEMU, which Cua does not \
     install for you. Install QEMU for Windows from https://qemu.weilnetz.de/ and add it to PATH, \
     or create a Linux Space instead";

/// What a QEMU VM needs that `which` does not find on an `os` host, as the
/// error a create returns before pulling its image; `None` when both
/// `qemu-system-<arch>` and `qemu-img` are there.
fn missing_tools(
    arch: Arch,
    os: host::HostOs,
    which: impl Fn(&str) -> Option<PathBuf>,
) -> Option<VmmError> {
    let system = format!("qemu-system-{}", arch.qemu());
    let missing: Vec<&str> = [system.as_str(), "qemu-img"]
        .into_iter()
        .filter(|t| which(t).is_none())
        .collect();
    if missing.is_empty() {
        return None;
    }
    let hint = match os {
        host::HostOs::Windows => WINDOWS_QEMU_HINT.to_string(),
        _ => format!(
            "install QEMU (macOS: `brew install qemu`; {})",
            QemuPackageManager::linux_hint()
        ),
    };
    Some(VmmError::missing(
        format!("QEMU ({})", missing.join(", ")),
        hint,
    ))
}

pub async fn grant_kvm(
    probe: impl Fn() -> std::io::Result<()>,
    user: Option<String>,
    dry_run: bool,
    run: impl AsyncFn(&str, &[&str]) -> Result<String>,
) -> Result<Option<String>> {
    let e = match probe() {
        Ok(()) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => e,
        Err(e) => return Ok(Some(host::kvm_hint(&e))),
    };
    let user = user.filter(|user| !user.is_empty());
    if dry_run {
        let user = user.as_deref().unwrap_or("$USER");
        return Ok(Some(format!(
            "would: add {user} to the kvm group and grant it /dev/kvm now (setfacl)"
        )));
    }
    let denied = || VmmError::PermissionDenied(host::kvm_hint(&e));
    let group = run("stat", &["-c", "%G", "/dev/kvm"]).await;
    if group.map_err(|_| denied())?.trim() != "kvm" {
        return Err(denied());
    }
    let user = match user {
        Some(user) => user,
        None => run("id", &["-un"]).await?.trim().to_string(),
    };
    run("sudo", &["-n", "usermod", "-aG", "kvm", &user])
        .await
        .map_err(|_| denied())?;
    let acl = format!("u:{user}:rw");
    let setfacl = run("sudo", &["-n", "setfacl", "-m", &acl, "/dev/kvm"]).await;
    Ok(Some(match setfacl.is_ok() && probe().is_ok() {
        true => format!("added {user} to the kvm group and granted /dev/kvm now"),
        false => {
            format!("added {user} to the kvm group; log out and back in (or run `newgrp kvm`)")
        }
    }))
}

/// Install QEMU through the host package manager (explicit opt-in).
const PACKAGE_ATTEMPTS: u32 = 3;

pub async fn install_qemu() -> Result<()> {
    install_qemu_with(
        QemuPackageManager::detect(),
        host::sudo_available,
        async |p, a| host::run(p, a).await,
    )
    .await
}

async fn install_qemu_with(
    manager: Option<(QemuPackageManager, PathBuf)>,
    sudo: impl Fn() -> bool,
    run: impl AsyncFn(&str, &[&str]) -> Result<String>,
) -> Result<()> {
    match host::HostOs::current() {
        host::HostOs::Macos => {
            let brew = host::which("brew").ok_or_else(|| {
                VmmError::missing(
                    "Homebrew",
                    "install https://brew.sh, then `brew install qemu`",
                )
            })?;
            tracing::info!("installing QEMU with Homebrew");
            run(&brew.to_string_lossy(), &["install", "qemu"]).await?;
        }
        host::HostOs::Linux => {
            let (manager, program) = manager.ok_or_else(|| {
                VmmError::missing("QEMU", "no supported package manager found (apt-get/dnf)")
            })?;
            if matches!(manager, QemuPackageManager::Apt) && !sudo() {
                let home = host::cua_home();
                tracing::info!(
                    "installing user-local QEMU at {}",
                    home.join("qemu").display()
                );
                return run(
                    "sh",
                    &[
                        "-c",
                        include_str!("install-local.sh"),
                        "cua-qemu",
                        &home.to_string_lossy(),
                    ],
                )
                .await
                .map(|_| ());
            }
            let program = program.display().to_string();
            tracing::info!("installing QEMU with {manager:?} (sudo -n)");
            for args in manager.commands(&program) {
                let refresh = args.last() == Some(&"update");
                let mut attempt = 1;
                loop {
                    match run("sudo", &args).await {
                        Ok(_) => break,
                        Err(e) if attempt < PACKAGE_ATTEMPTS => {
                            tracing::warn!(
                                "`sudo {}` failed (attempt {attempt}), retrying: {e}",
                                args.join(" ")
                            );
                            attempt += 1;
                            tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
                        }
                        Err(e) if refresh => {
                            tracing::warn!(
                                "package index refresh failed, installing from the current lists: {e}"
                            );
                            break;
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        host::HostOs::Windows => {
            return Err(VmmError::missing("QEMU", WINDOWS_QEMU_HINT));
        }
    }
    Ok(())
}

#[async_trait]
impl Runtime for QemuRuntime {
    fn kind(&self) -> BackendKind {
        BackendKind::Qemu
    }

    async fn ensure_base(&self, image: &ImageSource, base_name: &str) -> Result<CheckpointInfo> {
        validate_name(base_name)?;
        let _g = self.lock.lock().await;
        if let Ok(st) = self.load(base_name) {
            if st.kind != EntryKind::Base {
                return Err(VmmError::AlreadyExists(base_name.to_string()));
            }
            return Ok(CheckpointInfo {
                name: base_name.into(),
                backend: BackendKind::Qemu,
                created_at: st.created_at,
                source: st.source,
                shared_memory: false,
            });
        }
        let arch = Arch::host();
        let src = self.resolve_source(image, arch, None).await?;
        let fmt = img::info(&src, true).await?.format;
        let source = match image {
            ImageSource::Oci { reference } => Some(reference.clone()),
            _ => Some(src.display().to_string()),
        };
        let st = QemuState {
            name: base_name.into(),
            kind: EntryKind::Base,
            arch,
            os: GuestOs::Linux,
            disk: std::fs::canonicalize(&src)?,
            disk_format: fmt,
            install_iso: None,
            seed_iso: None,
            firmware: Firmware::Bios,
            cpus: 2,
            memory_mb: 2048,
            ports: BTreeMap::new(),
            vnc_display: None,
            qmp_port: None,
            pid: None,
            accel: String::new(),
            ssh: None,
            restrict_network: false,
            egress: None,
            netns: false,
            pasta_pid: None,
            extra_args: vec![],
            snapshots: vec![],
            paused: false,
            source: source.clone(),
            created_at: host::now_secs(),
            gpu: None,
            restore_from: None,
            memory: None,
            last_start: None,
            free_page_reporting: false,
            ram_file: None,
            headless: false,
            shared_memory: false,
            pinned: false,
            bulk: false,
            tpm: false,
        };
        self.save(&st)?;
        Ok(CheckpointInfo::now(base_name, BackendKind::Qemu, source))
    }

    async fn start(&self, spec: &StartSpec) -> Result<Instance> {
        // A local disk without an explicit arch boots as the architecture it
        // was built for (an amd64 disk emulated on an arm64 host), not the
        // host's.
        let detected;
        let spec = match (&spec.arch, &spec.image) {
            (None, ImageSource::Disk { path }) => {
                let path = path.clone();
                match tokio::task::spawn_blocking(move || disk_arch::detect(&path))
                    .await
                    .ok()
                    .flatten()
                {
                    Some(arch) => {
                        detected = spec.clone().arch(arch);
                        &detected
                    }
                    None => spec,
                }
            }
            _ => spec,
        };
        validate_name(&spec.name)?;
        crate::types::reject_sidecars(BackendKind::Qemu, spec)?;
        crate::types::reject_mounts(BackendKind::Qemu, spec)?;
        crate::types::reject_injection(BackendKind::Qemu, spec)?;
        crate::egress::check(BackendKind::Qemu, &spec.egress)?;
        crate::cloudinit::check_guest_support(spec)?;
        windows_arm_host(
            spec.os,
            spec.effective_arch(),
            Arch::host(),
            host::HostOs::current(),
        )?;
        let (mut st, fresh_backing) = {
            let _g = self.lock.lock().await;
            match self.load(&spec.name) {
                Ok(st) => {
                    if st.kind != EntryKind::Instance {
                        return Err(VmmError::invalid(format!(
                            "'{}' is a {:?} (a fork source); fork it into a new instance first",
                            spec.name, st.kind
                        )));
                    }
                    (st, None)
                }
                Err(VmmError::NotFound(_)) => {
                    crate::disk::ensure_space(
                        &self.cfg.root,
                        crate::disk::GIB,
                        &format!("create VM {}", spec.name),
                    )?;
                    if self.cfg.preflight_tools {
                        self.ensure_tools(spec.effective_arch()).await?;
                    }
                    let (backing, iso) = match &spec.image {
                        ImageSource::Existing => return Err(VmmError::NotFound(spec.name.clone())),
                        ImageSource::Disk { path }
                            if path
                                .extension()
                                .is_some_and(|e| e.eq_ignore_ascii_case("iso")) =>
                        {
                            (None, Some(path.clone()))
                        }
                        other => (
                            Some(
                                self.resolve_source(
                                    other,
                                    spec.effective_arch(),
                                    spec.registry_auth.as_ref(),
                                )
                                .await?,
                            ),
                            None,
                        ),
                    };
                    let source = match &spec.image {
                        ImageSource::Oci { reference } => Some(reference.clone()),
                        ImageSource::Disk { path } => Some(path.display().to_string()),
                        ImageSource::Existing => None,
                    };
                    let st = self
                        .create_instance(&spec.name, backing.as_deref(), iso, spec, source)
                        .await?;
                    (st, backing)
                }
                Err(e) => return Err(e),
            }
        };

        // The start spec's policy for a new VM (or one with none kept); a
        // kept one wins (`set_egress` changed it since).
        if st.egress.is_none() && !spec.egress.is_empty() {
            st.egress = Some(spec.egress.clone());
        }
        // A new VM of a configuration with a golden snapshot restores it
        // (recording it first if needed) instead of booting.
        if let Some(backing) = fresh_backing.as_deref()
            && let Some(key) = self.golden_key(&st, spec, backing).await
        {
            crate::progress::report(crate::progress::Progress::phase(
                crate::progress::Phase::Booting,
            ));
            match self.golden(&st, spec, backing, &key).await {
                Ok(snap) => self.adopt_snapshot(&mut st, &snap).await?,
                Err(e) => {
                    tracing::warn!(error = %e, "could not record a golden QEMU snapshot; booting cold")
                }
            }
        }

        if !Self::is_running(&st) {
            // Linux guests always get SSH access: the caller's, the one this
            // instance already has, or a managed per-instance key.
            let spec_owned;
            let mut spec = spec;
            if spec.ssh.is_none()
                && spec.cloud_init_user_data.is_none()
                && spec.os == GuestOs::Linux
            {
                let ssh = match st.ssh.clone() {
                    Some(s) => Some(s),
                    None => crate::cloudinit::managed_ssh(&self.dir(&st.name)).await,
                };
                if ssh.is_some() {
                    let mut s = spec.clone();
                    s.ssh = ssh;
                    spec_owned = s;
                    spec = &spec_owned;
                }
            }
            if spec.ssh.is_some() {
                st.ssh = spec.ssh.clone();
            }
            // Always seed a Linux guest (see `seed_for_spec`).
            self.write_seed(&mut st, spec).await?;
            if let Some(gb) = spec.disk_size_gb {
                img::grow_to(&st.disk, gb).await?;
            }
            crate::progress::report(crate::progress::Progress::phase(
                crate::progress::Phase::Booting,
            ));
            let dir = self.dir(&st.name);
            if let Some(snap) = st.restore_from.clone()
                && let Err(e) = self.restore_memory(&mut st, spec, &snap).await
            {
                let _ = self.force_stop(&mut st, Duration::ZERO).await;
                // No room on this host: the snapshot is fine.
                if !snap.golden || matches!(e, VmmError::HostCapacity(_)) {
                    let _ = self.save(&st);
                    return Err(e);
                }
                // A golden snapshot this host cannot load (a new QEMU, a
                // changed CPU): drop it and boot cold from the image.
                tracing::warn!(error = %e, golden = %snap.dir.display(), "golden snapshot did not restore; booting cold");
                self.drop_golden(&snap);
                st.restore_from = None;
                let backing = fresh_backing.as_deref().ok_or(e)?;
                let _ = std::fs::remove_file(&st.disk);
                img::create_overlay(backing, &st.disk).await?;
                if let Some(gb) = spec.disk_size_gb {
                    img::grow_to(&st.disk, gb).await?;
                }
            }
            if st.restore_from.is_none() && !Self::is_running(&st) {
                self.record_guest_token(
                    &dir,
                    spec.env
                        .get(crate::cloudinit::ENV_TOKEN_VAR)
                        .map(String::as_str),
                )?;
                Self::record_boot_config(&dir, spec)?;
                let boot = Boot {
                    memory: self.own_memory(&st, true)?,
                    incoming: false,
                    free_page_reporting: None,
                    headless: None,
                    keep_ports: spec.labels.contains_key(KEEP_PORTS_LABEL),
                };
                if let Err(e) = self.launch(&mut st, spec, boot, true).await {
                    // Another start owns the VM: its state and processes
                    // are not ours to stop or overwrite.
                    if !tpm_claim_refused(&e) {
                        let _ = self.force_stop(&mut st, Duration::ZERO).await;
                        let _ = self.save(&st);
                    }
                    return Err(e);
                }
                st.last_start = Some("cold".into());
                self.save(&st)?;
            }
        }
        let inst = self.instance(&st);
        if !spec.probes.is_empty() {
            crate::progress::report(crate::progress::Progress::phase(
                crate::progress::Phase::WaitingForServices,
            ));
        }
        let ready = self.wait_probes(&st, &inst.endpoints, spec).await;
        // Booted (or restored) and answering, or not going to: from here on
        // the VM counts what it really uses, not what it was expected to.
        self.release(&st.name);
        ready?;
        Ok(inst)
    }

    async fn stop(&self, name: &str) -> Result<()> {
        let mut st = self.load(name)?;
        let graceful = self.cfg.graceful_stop;
        self.force_stop(&mut st, graceful).await?;
        st.paused = false;
        self.save(&st)
    }

    async fn suspend(&self, name: &str) -> Result<()> {
        let mut q = self.qmp(name).await?;
        q.stop().await?;
        let mut st = self.load(name)?;
        st.paused = true;
        self.save(&st)
    }

    /// Runs a paused VM again over QMP. A VM that was stopped (its process
    /// ended; its disk is kept) boots again from its own disk, in the shape
    /// it had, as [`Self::resize`] boots it.
    async fn resume(&self, name: &str) -> Result<Instance> {
        let st = self.load(name)?;
        if st.kind == EntryKind::Instance && !Self::is_running(&st) {
            return self.boot_from_disk(name, &Resize::default()).await;
        }
        let mut q = self.qmp(name).await?;
        q.cont().await?;
        let mut st = self.load(name)?;
        st.paused = false;
        self.save(&st)?;
        Ok(self.instance(&st))
    }

    /// Gives a VM another shape: stops it (when it runs or is paused),
    /// applies the new vCPU count, memory and a larger disk, and boots it
    /// again from its own disk with the configuration it first started
    /// with. The guest's disk, ports and token are kept; its memory and
    /// running processes are not (a VM's shape is fixed while it runs).
    async fn resize(&self, name: &str, shape: &Resize) -> Result<Instance> {
        validate_name(name)?;
        let st = self.load(name)?;
        if st.kind != EntryKind::Instance {
            return Err(VmmError::invalid(format!("'{name}' is not a VM instance")));
        }
        if shape.is_empty() {
            return Err(VmmError::invalid(
                "resize needs a vCPU count, memory or disk size",
            ));
        }
        if shape.cpus == Some(0) {
            return Err(VmmError::invalid("a VM needs at least 1 vCPU"));
        }
        if shape.memory_mb.is_some_and(|m| m < 256) {
            return Err(VmmError::invalid("a VM needs at least 256 MiB of memory"));
        }
        if let Some(gb) = shape.disk_gb {
            let have = img::info(&st.disk, true).await?.virtual_size;
            if u64::from(gb) * 1024 * 1024 * 1024 < have {
                return Err(VmmError::invalid(format!(
                    "a disk only grows: it is {} GiB, not {gb}",
                    have / (1024 * 1024 * 1024)
                )));
            }
        }
        self.boot_from_disk(name, shape).await
    }

    /// Renames running pool member `member` to `new_name` (QEMU keeps its
    /// open files across the rename, and re-opens images through the
    /// member's stable alias, [`RUN_LINKS`]) and gives its guest the Space's token
    /// and hostname. The member restored its golden snapshot as a machine
    /// of its own (its pool secret as token) when it started, and has run
    /// since: its clock and network lease are current.
    async fn claim(
        &self,
        member: &str,
        new_name: &str,
        identity: &ClaimIdentity,
    ) -> Result<Instance> {
        validate_name(new_name)?;
        let _g = self.lock.lock().await;
        let mut st = self.load(member)?;
        if st.kind != EntryKind::Instance || !Self::is_running(&st) || st.paused {
            return Err(VmmError::invalid(format!(
                "'{member}' is not a running VM, so it cannot be claimed"
            )));
        }
        if st.tpm {
            return Err(VmmError::invalid(format!(
                "'{member}' has a TPM, whose swtpm is bound to the VM's directory; it cannot be claimed"
            )));
        }
        if self.dir(new_name).exists() {
            return Err(VmmError::AlreadyExists(new_name.to_string()));
        }
        let (from, to) = (self.dir(member), self.dir(new_name));
        std::fs::rename(&from, &to)?;
        // The member's QEMU re-opens its images through the alias.
        self.point_alias(member, &to)?;
        let moved = |p: &Path| match p.strip_prefix(&from) {
            Ok(rest) => to.join(rest),
            Err(_) => p.to_path_buf(),
        };
        st.name = new_name.to_string();
        st.disk = moved(&st.disk);
        st.seed_iso = st.seed_iso.as_deref().map(moved);
        st.install_iso = st.install_iso.as_deref().map(moved);
        if let Firmware::UefiPflash { vars, .. } = &mut st.firmware {
            *vars = moved(vars);
        }
        if let Some(ssh) = &mut st.ssh {
            ssh.private_key = moved(&ssh.private_key);
        }
        self.save(&st)?;
        // The Space's token (and hostname): a seeded guest's cua-spacesd
        // follows its token file, so `Init` cannot rotate it; the identity
        // provider rewrites it. The member became a machine of its own when
        // it restored its golden snapshot.
        if let (Some(provider), Some(token)) = (&self.cfg.identity, identity.token.as_deref()) {
            let current = self
                .guest_token(&to)
                .ok_or_else(|| snapshot::no_guest_token(member))?;
            if current != token {
                provider
                    .renew(
                        &self.endpoints_of(&st),
                        &current,
                        &ClaimIdentity {
                            token: Some(token.to_string()),
                            hostname: identity.hostname.clone().or(Some(new_name.to_string())),
                        },
                        false,
                        Duration::from_secs(30),
                    )
                    .await?;
                self.record_guest_token(&to, Some(token))?;
            }
        }
        Ok(self.instance(&st))
    }

    async fn fork(&self, source: &str, new_name: &str) -> Result<()> {
        validate_name(new_name)?;
        let _g = self.lock.lock().await;
        if self.state_path(new_name).exists() {
            return Err(VmmError::AlreadyExists(new_name.to_string()));
        }
        // `vm@snapshot` forks from an internal snapshot (disk state only).
        let (src_name, snapshot) = match source.split_once('@') {
            Some((a, b)) => (a, Some(b)),
            None => (source, None),
        };
        let mut src = self.load(src_name)?;
        if src.tpm && snapshot.is_some() {
            return refuse_tpm(&src, "a disk snapshot (fork a stopped checkpoint instead)");
        }
        if Self::is_running(&src) {
            refuse_tpm(&src, "a live fork")?;
        }
        let dir = self.dir(new_name);
        // A running VM forks with its memory: a one-time snapshot in the
        // fork's own directory, restored at the fork's first start.
        if snapshot.is_none() && Self::is_running(&src) {
            if !self.restores_memory() {
                return Err(VmmError::invalid(format!(
                    "'{src_name}' is running; stop it (or checkpoint it) before forking"
                )));
            }
            let (snap, firmware, ssh) = self.save_memory(&src, &dir.join("snapshot"), true).await?;
            let disk = dir.join("disk.qcow2");
            if let Err(e) = img::create_overlay(&snap.disk(), &disk).await {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
            let st = QemuState {
                name: new_name.into(),
                kind: EntryKind::Instance,
                disk,
                disk_format: "qcow2".into(),
                firmware: self.copy_vars(&firmware, &dir)?,
                ssh: match ssh {
                    Some(s) => Some(snapshot::copy_ssh_key(&s, &dir)?),
                    None => None,
                },
                ports: BTreeMap::new(),
                vnc_display: None,
                qmp_port: None,
                pid: None,
                snapshots: vec![],
                paused: false,
                seed_iso: None,
                source: Some(source.into()),
                created_at: host::now_secs(),
                restore_from: Some(snap),
                memory: None,
                last_start: None,
                shared_memory: false,
                pinned: false,
                ..src
            };
            return self.save(&st);
        }
        if Self::is_running(&src) {
            return Err(VmmError::invalid(format!(
                "'{src_name}' is running; stop it (or checkpoint it) before forking"
            )));
        }
        let _busy = self.tpm_claim(&src, false).await?;
        self.tpm_stopped(&src)?;
        let backing = match (snapshot, src.kind) {
            (Some(snap), _) => {
                // A full copy of the disk at that snapshot.
                crate::disk::ensure_space(
                    &self.cfg.root,
                    std::fs::metadata(&src.disk).map(|m| m.len()).unwrap_or(0),
                    &format!("fork {source}"),
                )?;
                let layers = self.layers_dir();
                let layer = layers.join(format!("{src_name}-{snap}-{}.qcow2", unique_suffix()));
                img::convert(&src.disk, &layer, false, Some(snap)).await?;
                layer
            }
            (None, EntryKind::Base | EntryKind::Checkpoint) => src.disk.clone(),
            (None, EntryKind::Instance) => {
                let layer = self.freeze(&mut src).await?;
                self.save(&src)?;
                layer
            }
        };
        let disk = dir.join("disk.qcow2");
        match (snapshot, src.kind) {
            // Every fork of a fork source starts with the same empty overlay
            // on it: made once and copied (no `qemu-img` per fork).
            (None, EntryKind::Base | EntryKind::Checkpoint) => {
                let template = self.dir(src_name).join(FORK_OVERLAY);
                if !template.is_file() {
                    let tmp = template.with_extension(format!("tmp-{}", unique_suffix()));
                    img::create_overlay(&backing, &tmp).await?;
                    std::fs::rename(&tmp, &template)?;
                }
                std::fs::create_dir_all(&dir)?;
                std::fs::copy(&template, &disk)?;
            }
            _ => img::create_overlay(&backing, &disk).await?,
        }
        // A checkpoint's key may go with it; the fork keeps its own copy.
        let ssh = match &src.ssh {
            Some(s) if src.kind == EntryKind::Checkpoint && s.private_key.exists() => {
                Some(snapshot::copy_ssh_key(s, &dir)?)
            }
            other => other.clone(),
        };
        let firmware = match src.kind {
            EntryKind::Base => self.setup_firmware(&dir, src.arch, src.os, None).await?,
            _ => self.copy_vars(&src.firmware, &dir)?,
        };
        if let Err(e) = self.copy_tpm(&src, &dir) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        let st = QemuState {
            name: new_name.into(),
            kind: EntryKind::Instance,
            disk,
            disk_format: "qcow2".into(),
            firmware,
            ports: BTreeMap::new(),
            vnc_display: None,
            qmp_port: None,
            pid: None,
            snapshots: vec![],
            paused: false,
            seed_iso: None,
            source: Some(source.into()),
            created_at: host::now_secs(),
            // A memory checkpoint's forks restore its memory (sharing its
            // RAM file's pages); others boot from the disk.
            restore_from: match (&src.memory, snapshot) {
                (Some(m), None) => {
                    if let Some(ram) = &m.ram_file {
                        snapshot::prefetch_ram_once(ram);
                    }
                    Some(m.clone())
                }
                _ => None,
            },
            memory: None,
            last_start: None,
            shared_memory: false,
            pinned: false,
            ssh,
            ..src
        };
        self.save(&st)
    }

    async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
        validate_name(checkpoint)?;
        let _g = self.lock.lock().await;
        let mut st = self.load(name)?;
        if Self::is_running(&st) {
            refuse_tpm(&st, "a live checkpoint")?;
        }
        if Self::is_running(&st) && self.restores_memory() {
            // Live: a memory checkpoint entry (RAM, devices, a copy of the
            // disk layer); forks of it restore that memory with their own
            // identity. The VM keeps running.
            if self.state_path(checkpoint).exists() {
                return Err(VmmError::AlreadyExists(checkpoint.to_string()));
            }
            let dir = self.dir(checkpoint);
            let started = std::time::Instant::now();
            let seed = st.seed_iso.clone();
            let (snap, firmware, ssh) = self.save_memory(&st, &dir, false).await?;
            let saved = started.elapsed();
            let mut ck = QemuState {
                name: checkpoint.into(),
                kind: EntryKind::Checkpoint,
                disk: snap.disk(),
                firmware,
                ssh,
                ports: BTreeMap::new(),
                pid: None,
                qmp_port: None,
                vnc_display: None,
                snapshots: vec![],
                paused: false,
                seed_iso: None,
                source: Some(name.into()),
                created_at: host::now_secs(),
                restore_from: None,
                memory: Some(snap.clone()),
                last_start: None,
                shared_memory: false,
                pinned: false,
                ..st
            };
            // Forks share the checkpoint's RAM copy-on-write only when it
            // is a file; a stream that holds the RAM is loaded into one,
            // unless the VM had free page reporting (density): its restores
            // keep that balloon, and QEMU's discards over a shared RAM file
            // would erase pages other forks read, so that RAM stays in the
            // stream and each fork loads it privately.
            if needs_shared_ram(&snap) {
                match self.share_ram(&ck, &snap, seed.as_deref()).await {
                    Ok(shared) => ck.memory = Some(shared),
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(&dir);
                        let _ = std::fs::remove_file(snap.disk());
                        return Err(e);
                    }
                }
            }
            self.save(&ck)?;
            tracing::info!(
                name,
                checkpoint,
                save_ms = saved.as_millis() as u64,
                total_ms = started.elapsed().as_millis() as u64,
                cloned = snap.ram_file.is_some(),
                "QEMU memory checkpoint written"
            );
            let shared_memory = ck.memory.as_ref().is_some_and(|m| m.ram_file.is_some());
            return Ok(CheckpointInfo {
                shared_memory,
                ..CheckpointInfo::now(checkpoint, BackendKind::Qemu, Some(name.into()))
            });
        }
        if Self::is_running(&st) {
            // Without an identity provider: an internal snapshot of RAM +
            // devices + disks inside the instance's qcow2. Restore with
            // `restore()`, or fork `name@ckpt` once stopped.
            let mut q = self.qmp(name).await?;
            q.savevm(checkpoint).await?;
            if !st.snapshots.iter().any(|s| s == checkpoint) {
                st.snapshots.push(checkpoint.to_string());
            }
            self.save(&st)?;
            return Ok(CheckpointInfo::now(
                checkpoint,
                BackendKind::Qemu,
                Some(name.into()),
            ));
        }
        // Stopped: freeze the disk into an immutable checkpoint entry.
        if self.state_path(checkpoint).exists() {
            return Err(VmmError::AlreadyExists(checkpoint.to_string()));
        }
        let _busy = self.tpm_claim(&st, false).await?;
        self.tpm_stopped(&st)?;
        let layer = self.freeze(&mut st).await?;
        self.save(&st)?;
        let dir = self.dir(checkpoint);
        let firmware = self.copy_vars(&st.firmware, &dir)?;
        if let Err(e) = self.copy_tpm(&st, &dir) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        let ck = QemuState {
            name: checkpoint.into(),
            kind: EntryKind::Checkpoint,
            disk: layer,
            firmware,
            ports: BTreeMap::new(),
            pid: None,
            qmp_port: None,
            vnc_display: None,
            snapshots: vec![],
            paused: false,
            source: Some(name.into()),
            created_at: host::now_secs(),
            restore_from: None,
            memory: None,
            last_start: None,
            shared_memory: false,
            pinned: false,
            ..st
        };
        self.save(&ck)?;
        Ok(CheckpointInfo::now(
            checkpoint,
            BackendKind::Qemu,
            Some(name.into()),
        ))
    }

    /// Removes a stopped-checkpoint entry, or an internal snapshot given as
    /// `vm@snapshot`.
    /// In the VM's network namespace: refilled at once when it runs in
    /// one; a running VM without one (started without a policy that
    /// blocks anything) is moved into one first ([`QemuRuntime::relaunch`]:
    /// its memory carries over); a stopped VM gets it at its next start.
    async fn set_egress(
        &self,
        name: &str,
        policy: &crate::egress::EgressPolicy,
    ) -> Result<crate::egress::Enforcement> {
        let rules = crate::egress::check(BackendKind::Qemu, policy)?;
        let _g = self.lock.lock().await;
        let mut st = self.load(name)?;
        if st.kind != EntryKind::Instance {
            return Err(VmmError::invalid(format!("'{name}' is not a VM instance")));
        }
        if Self::is_running(&st) && !st.netns && rules.blocks_anything() {
            refuse_tpm(&st, "a move into a new network namespace")?;
        }
        let previous = st.egress.replace(policy.clone());
        if Self::is_running(&st) {
            let applied = if st.netns {
                let pid = st.pid.expect("running");
                crate::egress::ns_apply(pid, &netns_rules_script(&rules)).await
            } else if rules.blocks_anything() {
                self.relaunch(&mut st).await
            } else {
                Ok(())
            };
            if let Err(e) = applied {
                if Self::is_running(&st) {
                    // Still in its old process: keep what it had.
                    st.egress = previous;
                }
                let _ = self.save(&st);
                return Err(e);
            }
        }
        self.save(&st)?;
        Ok(crate::egress::Enforcement::Host)
    }

    async fn egress(&self, name: &str) -> Result<crate::egress::EgressPolicy> {
        Ok(self.load(name)?.egress.unwrap_or_default())
    }

    async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
        if let Some((vm, snap)) = checkpoint.split_once('@') {
            let mut st = self.load(vm)?;
            if Self::is_running(&st) {
                self.qmp(vm).await?.delvm(snap).await?;
            } else {
                let d = st.disk.display().to_string();
                host::run(img::qemu_img()?, &["snapshot", "-d", snap, &d]).await?;
            }
            st.snapshots.retain(|s| s != snap);
            return self.save(&st);
        }
        let ck = self.load(checkpoint)?;
        match ck.kind {
            EntryKind::Checkpoint if ck.pinned => Err(VmmError::invalid(format!(
                "checkpoint '{checkpoint}' is pinned; unpin it before deleting it"
            ))),
            EntryKind::Checkpoint => self.delete(checkpoint).await,
            k => Err(VmmError::invalid(format!(
                "'{checkpoint}' is a {k:?}, not a checkpoint"
            ))),
        }
    }

    async fn export_checkpoint(&self, checkpoint: &str) -> Result<CheckpointExport> {
        self.export(checkpoint).await
    }

    fn import_dir(&self, digest: &str) -> Option<PathBuf> {
        Some(self.imported_dir(digest))
    }

    async fn import_checkpoint(
        &self,
        checkpoint: &str,
        state: serde_json::Value,
        qcow2_rebase: Vec<(PathBuf, PathBuf)>,
    ) -> Result<()> {
        self.import(checkpoint, state, qcow2_rebase).await
    }

    fn snapshot_abis(&self) -> Vec<String> {
        self.restorable_abis()
    }

    async fn pin_checkpoint(&self, checkpoint: &str, pinned: bool) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut st = self.load(checkpoint)?;
        if st.kind != EntryKind::Checkpoint {
            return Err(VmmError::invalid(format!(
                "'{checkpoint}' is a {:?}, not a checkpoint",
                st.kind
            )));
        }
        st.pinned = pinned;
        self.save(&st)
    }

    async fn list(&self) -> Result<Vec<InstanceSummary>> {
        Ok(self
            .all_states()?
            .iter()
            .map(|st| InstanceSummary {
                name: st.name.clone(),
                backend: BackendKind::Qemu,
                status: Self::status_of(st),
            })
            .collect())
    }

    async fn status(&self, name: &str) -> Result<Status> {
        Ok(Self::status_of(&self.load(name)?))
    }

    async fn delete(&self, name: &str) -> Result<()> {
        let st = self.load(name)?;
        // A fork made but not started yet restores this checkpoint's memory.
        if st.kind == EntryKind::Checkpoint {
            let dir = self.dir(name);
            if let Some(f) = self.all_states()?.into_iter().find(|f| {
                f.restore_from
                    .as_ref()
                    .is_some_and(|r| !r.golden && r.dir == dir)
            }) {
                return Err(VmmError::invalid(format!(
                    "checkpoint '{name}' is still needed by fork '{}', which has not started yet; \
                     start or delete it first",
                    f.name
                )));
            }
        }
        // Checkpoints and bases may be in use by forks being staged, so they
        // are removed under the runtime lock as a whole.
        if st.kind != EntryKind::Instance {
            let _g = self.lock.lock().await;
            let mut st = self.load(name)?;
            self.force_stop(&mut st, Duration::ZERO).await?;
            std::fs::remove_dir_all(self.dir(name))?;
            drop(_g);
            return self.collect_layers().await;
        }
        // Stopping a VM waits on its own process, so it runs outside the
        // runtime lock and deletes of many VMs overlap.
        let mut st = st;
        self.force_stop(&mut st, Duration::ZERO).await?;
        {
            let _g = self.lock.lock().await;
            // A fork that never started leaves its staged stream behind.
            if let Some(snap) = st.restore_from.as_ref().filter(|s| s.one_shot) {
                let _ = std::fs::remove_file(snap.state());
            }
            std::fs::remove_dir_all(self.dir(name))?;
        }
        self.collect_layers().await
    }

    async fn endpoints(&self, name: &str) -> Result<Endpoints> {
        Ok(self.endpoints_of(&self.load(name)?))
    }

    /// Slirp-aware: whether the guest side of a forwarded connection
    /// reaches ESTABLISHED (`info usernet`).
    async fn guest_tcp_listening(&self, name: &str, guest_port: u16) -> Result<Option<bool>> {
        let st = self.load(name)?;
        if !Self::is_running(&st) {
            return Ok(Some(false));
        }
        self.guest_tcp_established(&st, guest_port).await.map(Some)
    }

    async fn guest_exec(&self, name: &str) -> Result<Option<Arc<dyn GuestExec>>> {
        let ep = self.endpoints(name).await?;
        Ok(ep
            .ssh
            .as_ref()
            .map(|s| Arc::new(SshExec::from_endpoint(s)) as Arc<dyn GuestExec>))
    }
}

#[cfg(unix)]
const QEMU_LAUNCH: &str = "\"$0\" \"$@\" </dev/null >>\"$CUA_QEMU_LOG\" 2>&1 & echo $!";

#[cfg(unix)]
const TPM_LAUNCH: &str = r#""$CUA_SWTPM" socket --daemon --tpm2 --terminate --ctrl "type=unixio,path=$CUA_TPM_DIR/swtpm.sock" --pid "file=$CUA_TPM_RECORD" --tpmstate "dir=$CUA_TPM_DIR" </dev/null >>"$CUA_TPM_DIR/../swtpm.log" 2>&1 || exit 1
"$0" "$@" </dev/null >>"$CUA_QEMU_LOG" 2>&1 & echo $!"#;

#[cfg(all(test, unix))]
mod tpm_tests;

/// The snapshot RAM file a VM shares with others, as admission counts it
/// ([`crate::density`]): a file mapped copy-on-write (`share=off`, a
/// restore or a fork) is counted once for every VM mapping it, on top of
/// each VM's private pages. A file the VM writes itself (`share=on`, a
/// golden snapshot being recorded) is that VM's own RAM, not shared with
/// anyone: it is admitted like anonymous RAM, so its memory is not counted
/// twice (once as the guest's RAM, again as the file).
fn admission_file(memory: &Memory) -> Option<PathBuf> {
    match memory {
        Memory::File {
            path,
            shared: false,
        } => Some(path.clone()),
        Memory::File { shared: true, .. } | Memory::Ram => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn port_conflicts_in_qemu_stderr_are_recognised() {
        assert!(is_port_conflict(
            "qemu-system-x86_64: -vnc 127.0.0.1:7: Failed to find an available port: Address already in use"
        ));
        assert!(is_port_conflict(
            "Could not set up host forwarding rule 'tcp:127.0.0.1:41000-:22'"
        ));
        assert!(!is_port_conflict(
            "qemu-system-x86_64: cannot set up guest memory"
        ));
    }

    use super::*;

    fn rt(dir: &Path) -> QemuRuntime {
        QemuRuntime::new(QemuConfig {
            root: dir.to_path_buf(),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn resize_refuses_before_stopping_anything() {
        if img::qemu_img().is_err() {
            eprintln!("qemu-img missing; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let r = rt(&tmp.path().join("qemu"));
        let image = tmp.path().join("image.qcow2");
        img::create_blank(&image, 1).await.unwrap();
        r.ensure_base(&ImageSource::disk(&image), "base")
            .await
            .unwrap();
        let mut base = r.load("base").unwrap();
        base.arch = StartSpec::new("x", ImageSource::Existing)
            .arch(Arch::X86_64)
            .effective_arch();
        r.save(&base).unwrap();
        r.fork("base", "vm1").await.unwrap();
        async fn bad(r: &QemuRuntime, s: Resize) -> String {
            r.resize("vm1", &s).await.unwrap_err().to_string()
        }
        assert!(bad(&r, Resize::default()).await.contains("needs a vCPU"));
        assert!(
            bad(
                &r,
                Resize {
                    cpus: Some(0),
                    ..Default::default()
                }
            )
            .await
            .contains("at least 1 vCPU")
        );
        assert!(
            bad(
                &r,
                Resize {
                    memory_mb: Some(64),
                    ..Default::default()
                }
            )
            .await
            .contains("256 MiB")
        );
        assert!(
            bad(
                &r,
                Resize {
                    disk_gb: Some(0),
                    ..Default::default()
                }
            )
            .await
            .contains("only grows")
        );
        assert!(matches!(
            r.resize(
                "nope",
                &Resize {
                    cpus: Some(2),
                    ..Default::default()
                }
            )
            .await,
            Err(VmmError::NotFound(_))
        ));
        // The base is not an instance.
        assert!(
            r.resize(
                "base",
                &Resize {
                    cpus: Some(2),
                    ..Default::default()
                }
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn boot_config_roundtrips_privately() {
        let tmp = tempfile::tempdir().unwrap();
        let mut spec = StartSpec::new("x", ImageSource::Existing);
        spec.env.insert("A".into(), "1".into());
        spec.command = Some(vec!["sleep".into(), "9".into()]);
        QemuRuntime::record_boot_config(tmp.path(), &spec).unwrap();
        let got = QemuRuntime::read_boot_config(tmp.path());
        assert_eq!(got.env["A"], "1");
        assert_eq!(got.command.unwrap()[0], "sleep");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(tmp.path().join(BOOT_CONFIG)).unwrap();
            assert_eq!(m.permissions().mode() & 0o077, 0);
        }
        assert!(
            QemuRuntime::read_boot_config(&tmp.path().join("none"))
                .env
                .is_empty()
        );
    }

    /// Deleting many VMs together runs far fewer layer collections than
    /// deletes, and every delete returns only after a collection that began
    /// after its own removal (so its layers are gone when it returns).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_layer_collections_are_shared() {
        use std::sync::atomic::Ordering::SeqCst;
        let tmp = tempfile::tempdir().unwrap();
        let r = std::sync::Arc::new(rt(&tmp.path().join("qemu")));
        std::fs::create_dir_all(r.layers_dir()).unwrap();
        let n = 64usize;
        // Hold the runtime lock so every "delete" is waiting for a
        // collection when it opens.
        let held = r.lock.lock().await;
        let mut tasks = Vec::new();
        for i in 0..n {
            let r = r.clone();
            // Each delete leaves one unreferenced layer, then collects.
            let layer = r.layers_dir().join(format!("l{i}.qcow2"));
            std::fs::write(&layer, b"x").unwrap();
            tasks.push(tokio::spawn(async move {
                r.collect_layers().await.unwrap();
                assert!(
                    !layer.exists(),
                    "layer {i} still there after its delete returned"
                );
            }));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(held);
        for t in tasks {
            t.await.unwrap();
        }
        let runs = r.gc_runs.load(SeqCst);
        assert!(
            (1..=3).contains(&runs),
            "{runs} collections for {n} deletes"
        );
    }

    /// Exercises create → fork → checkpoint → gc against real qemu-img
    /// without booting anything.
    #[tokio::test]
    async fn pinned_checkpoints_and_fork_overlays_without_booting() {
        if img::qemu_img().is_err() {
            eprintln!("qemu-img missing; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("qemu");
        let r = rt(&root);
        let image = tmp.path().join("image.qcow2");
        img::create_blank(&image, 1).await.unwrap();
        r.ensure_base(&ImageSource::disk(&image), "base")
            .await
            .unwrap();
        let mut base = r.load("base").unwrap();
        base.arch = Arch::X86_64;
        r.save(&base).unwrap();
        r.fork("base", "vm1").await.unwrap();
        r.checkpoint("vm1", "ck1").await.unwrap();

        // Forks of a fork source copy one empty overlay made on first use.
        r.fork("ck1", "f1").await.unwrap();
        let template = root.join("ck1").join(FORK_OVERLAY);
        assert!(template.is_file());
        r.fork("ck1", "f2").await.unwrap();
        let ck = r.load("ck1").unwrap();
        for f in ["f1", "f2"] {
            let st = r.load(f).unwrap();
            assert!(!st.pinned && !st.shared_memory, "{f} inherits no pin");
            let chain = img::chain_files(&st.disk).await.unwrap();
            assert_eq!(
                std::fs::canonicalize(&chain[1]).unwrap(),
                std::fs::canonicalize(&ck.disk).unwrap(),
                "{f} sits on the checkpoint's layer"
            );
        }
        assert_ne!(r.load("f1").unwrap().disk, r.load("f2").unwrap().disk);

        // Pinning: only checkpoints; a pinned one is not deleted.
        assert!(r.pin_checkpoint("vm1", true).await.is_err());
        r.pin_checkpoint("ck1", true).await.unwrap();
        assert!(r.load("ck1").unwrap().pinned);
        let err = r.delete_checkpoint("ck1").await.unwrap_err();
        assert!(err.to_string().contains("pinned"), "{err}");
        assert!(r.load("ck1").is_ok());
        r.pin_checkpoint("ck1", false).await.unwrap();

        // A fork that has not restored the checkpoint's memory yet holds it.
        let mut f1 = r.load("f1").unwrap();
        f1.restore_from = Some(MemorySnapshot {
            dir: root.join("ck1"),
            ram_file: Some(root.join("ck1").join("mem.ram")),
            golden: false,
            one_shot: false,
            state_file: None,
            disk: Some(ck.disk.clone()),
            free_page_reporting: false,
            headless: false,
            bulk: false,
        });
        r.save(&f1).unwrap();
        let err = r.delete_checkpoint("ck1").await.unwrap_err();
        assert!(err.to_string().contains("'f1'"), "{err}");
        r.delete("f1").await.unwrap();

        // Unpinned and unused: deleted; f2 keeps the layer it sits on.
        r.delete_checkpoint("ck1").await.unwrap();
        assert!(r.load("ck1").is_err());
        let d = r.load("f2").unwrap().disk;
        assert!(img::chain_files(&d).await.is_ok(), "f2 chain broken");
        for vm in ["f2", "vm1", "base"] {
            r.delete(vm).await.unwrap();
        }
        let left = std::fs::read_dir(root.join(LAYERS))
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(left, 0, "unreferenced layers should be garbage-collected");
    }

    #[test]
    fn windows_arm_host_policy() {
        use host::HostOs::*;
        let x86 = "Windows on ARM guests need an arm64 host (Apple silicon, or arm64 Linux with KVM); this host is x86_64";
        let win = "Windows on ARM guests need swtpm for their TPM, which runs on macOS and Linux hosts only";
        let msg = |r: Result<()>| match r.unwrap_err() {
            VmmError::Invalid(m) => m,
            e => panic!("{e:?}"),
        };
        let (w, a) = (GuestOs::Windows, Arch::Aarch64);
        assert_eq!(msg(windows_arm_host(w, a, Arch::X86_64, Linux)), x86);
        assert_eq!(msg(windows_arm_host(w, a, Arch::Aarch64, Windows)), win);
        assert!(windows_arm_host(w, a, Arch::Aarch64, Macos).is_ok());
        assert!(windows_arm_host(w, a, Arch::Aarch64, Linux).is_ok());
        assert!(windows_arm_host(GuestOs::Linux, a, Arch::X86_64, Linux).is_ok());
        assert!(windows_arm_host(w, Arch::X86_64, Arch::X86_64, Linux).is_ok());
    }

    #[test]
    fn windows_arm_memory_floor() {
        let (w, a) = (GuestOs::Windows, Arch::Aarch64);
        assert_eq!(guest_memory_mb(w, a, 2048), 4096);
        assert_eq!(guest_memory_mb(w, a, 4096), 4096);
        assert_eq!(guest_memory_mb(w, a, 8192), 8192);
        assert_eq!(guest_memory_mb(GuestOs::Linux, a, 2048), 2048);
        assert_eq!(guest_memory_mb(w, Arch::X86_64, 2048), 2048);
    }

    #[cfg(unix)]
    fn invalid(r: Result<impl std::fmt::Debug>) -> String {
        match r.unwrap_err() {
            VmmError::Invalid(m) => m,
            e => panic!("{e:?}"),
        }
    }

    #[cfg(unix)]
    /// A runtime with a base, a stopped TPM VM `vm1`, and its TPM files.
    async fn tpm_runtime(tmp: &Path) -> Option<QemuRuntime> {
        if img::qemu_img().is_err() {
            eprintln!("qemu-img missing; skipping");
            return None;
        }
        let root = tmp.join("qemu");
        let r = rt(&root);
        let image = tmp.join("image.qcow2");
        img::create_blank(&image, 1).await.unwrap();
        r.ensure_base(&ImageSource::disk(&image), "base")
            .await
            .unwrap();
        let mut base = r.load("base").unwrap();
        base.arch = Arch::X86_64;
        r.save(&base).unwrap();
        r.fork("base", "vm1").await.unwrap();
        let mut vm = r.load("vm1").unwrap();
        vm.tpm = true;
        r.save(&vm).unwrap();
        let tpm = root.join("vm1/tpm");
        std::fs::create_dir_all(&tpm).unwrap();
        std::fs::write(tpm.join("tpm2-00.permall"), b"keys-1").unwrap();
        Some(r)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tpm_state_follows_stopped_fork_and_checkpoint() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let Some(r) = tpm_runtime(tmp.path()).await else {
            return;
        };
        let root = tmp.path().join("qemu");
        let _sock =
            std::os::unix::net::UnixListener::bind(root.join("vm1/tpm/swtpm.sock")).unwrap();
        r.checkpoint("vm1", "ck1").await.unwrap();
        let (src, ck) = (
            root.join("vm1/tpm/tpm2-00.permall"),
            root.join("ck1/tpm/tpm2-00.permall"),
        );
        assert_eq!(std::fs::read(&ck).unwrap(), b"keys-1");
        assert!(!root.join("ck1/tpm/swtpm.sock").exists());
        assert!(r.load("ck1").unwrap().tpm);
        assert_ne!(
            std::fs::metadata(&src).unwrap().ino(),
            std::fs::metadata(&ck).unwrap().ino()
        );
        std::fs::write(&src, b"keys-2").unwrap();
        assert_eq!(std::fs::read(&ck).unwrap(), b"keys-1");
        r.fork("ck1", "f1").await.unwrap();
        assert_eq!(
            std::fs::read(root.join("f1/tpm/tpm2-00.permall")).unwrap(),
            b"keys-1"
        );
        assert!(r.load("f1").unwrap().tpm);
        for d in ["ck1", "f1"] {
            let m = std::fs::metadata(root.join(d).join("tpm")).unwrap();
            assert_eq!(m.permissions().mode() & 0o777, 0o700, "{d}");
        }
        // A stopped fork of the VM itself carries the current state.
        r.fork("vm1", "f2").await.unwrap();
        assert_eq!(
            std::fs::read(root.join("f2/tpm/tpm2-00.permall")).unwrap(),
            b"keys-2"
        );

        let msg = "holds TPM state, which is not copied between hosts";
        let err = invalid(r.export("ck1").await);
        assert_eq!(err, format!("checkpoint 'ck1' {msg}"));
        let mut state = serde_json::to_value(r.load("ck1").unwrap()).unwrap();
        state["tpm"] = true.into();
        let err = invalid(r.import("ck2", state, vec![]).await);
        assert_eq!(err, format!("checkpoint 'ck2' {msg}"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tpm_copy_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(r) = tpm_runtime(tmp.path()).await else {
            return;
        };
        let root = tmp.path().join("qemu");
        std::fs::create_dir(root.join("vm1/tpm/nested")).unwrap();
        assert!(r.checkpoint("vm1", "ck1").await.is_err());
        assert!(!root.join("ck1").exists());
        std::fs::remove_dir_all(root.join("vm1/tpm")).unwrap();
        assert!(r.checkpoint("vm1", "ck2").await.is_err());
        assert!(!root.join("ck2").exists());
        assert!(r.load("ck1").is_err() && r.load("ck2").is_err());
        assert!(r.load("vm1").is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fork_and_checkpoint_refuse_a_live_tpm_before_freezing() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(r) = tpm_runtime(tmp.path()).await else {
            return;
        };
        let root = tmp.path().join("qemu");
        let link = super::tpm_tests::fake_swtpm_link(tmp.path());
        let mut fake = super::tpm_tests::spawn_fake(
            &link,
            &root.join("vm1/swtpm.a.pid"),
            &root.join("vm1/tpm"),
        );
        let disk = r.load("vm1").unwrap().disk;
        let count = |r: &QemuRuntime| std::fs::read_dir(r.layers_dir()).map_or(0, |d| d.count());
        let layers = count(&r);
        let e = r.checkpoint("vm1", "ck1").await.unwrap_err().to_string();
        assert!(e.contains("still running"), "{e}");
        let e = r.fork("vm1", "vm2").await.unwrap_err().to_string();
        assert!(e.contains("still running"), "{e}");
        assert_eq!(r.load("vm1").unwrap().disk, disk);
        assert_eq!(count(&r), layers);
        assert!(r.load("ck1").is_err() && r.load("vm2").is_err());
        fake.kill().unwrap();
        fake.wait().unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_tpm_vm_refusals() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(r) = tpm_runtime(tmp.path()).await else {
            return;
        };
        let root = tmp.path().join("qemu");
        let names = |r: &QemuRuntime| {
            let mut v: Vec<_> = std::fs::read_dir(&r.cfg.root)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            v.sort();
            v
        };
        let before = names(&r);
        let refused = |what: &str| {
            format!("'vm1' has a TPM, whose state cannot follow {what}; stop it first")
        };
        // A stopped TPM VM refuses a disk-snapshot fork.
        assert_eq!(
            invalid(r.fork("vm1@snap", "x").await),
            refused("a disk snapshot (fork a stopped checkpoint instead)")
        );
        let mut vm = r.load("vm1").unwrap();
        vm.pid = Some(std::process::id());
        r.save(&vm).unwrap();
        assert_eq!(invalid(r.fork("vm1", "x").await), refused("a live fork"));
        assert_eq!(
            invalid(r.checkpoint("vm1", "x").await),
            refused("a live checkpoint")
        );
        assert_eq!(
            invalid(r.restore("vm1", "s").await),
            refused("an in-place restore (restore by forking a stopped checkpoint)")
        );
        let block = crate::egress::EgressPolicy {
            deny: vec!["all".into()],
            ..Default::default()
        };
        assert_eq!(
            invalid(r.set_egress("vm1", &block).await),
            refused("a move into a new network namespace")
        );
        // A policy that blocks nothing needs no namespace, so it is saved.
        r.set_egress("vm1", &Default::default()).await.unwrap();
        assert_eq!(names(&r), before);
        assert!(!root.join("x").exists());
    }

    #[tokio::test]
    async fn disk_chain_bookkeeping_without_booting() {
        if img::qemu_img().is_err() {
            eprintln!("qemu-img missing; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("qemu");
        let r = rt(&root);
        let image = tmp.path().join("image.qcow2");
        img::create_blank(&image, 1).await.unwrap();

        // Base → fork → instance chain.
        r.ensure_base(&ImageSource::disk(&image), "base")
            .await
            .unwrap();
        r.ensure_base(&ImageSource::disk(&image), "base")
            .await
            .unwrap(); // idempotent
        let arch_spec = StartSpec::new("x", ImageSource::Existing).arch(Arch::X86_64);
        // x86 + linux → BIOS, so no firmware lookup is needed.
        let mut base = r.load("base").unwrap();
        base.arch = arch_spec.effective_arch();
        r.save(&base).unwrap();
        r.fork("base", "vm1").await.unwrap();
        assert!(matches!(
            r.fork("base", "vm1").await,
            Err(VmmError::AlreadyExists(_))
        ));
        let vm1 = r.load("vm1").unwrap();
        assert_eq!(vm1.kind, EntryKind::Instance);
        let chain = img::chain_files(&vm1.disk).await.unwrap();
        assert_eq!(chain.len(), 2);

        // Stopped checkpoint freezes vm1's disk into a layer shared by both.
        r.checkpoint("vm1", "ck1").await.unwrap();
        let vm1 = r.load("vm1").unwrap();
        let ck = r.load("ck1").unwrap();
        assert_eq!(ck.kind, EntryKind::Checkpoint);
        let chain = img::chain_files(&vm1.disk).await.unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(
            std::fs::canonicalize(&chain[1]).unwrap(),
            std::fs::canonicalize(&ck.disk).unwrap()
        );

        // Fork from the checkpoint and from a stopped instance.
        r.fork("ck1", "vm2").await.unwrap();
        r.fork("vm1", "vm3").await.unwrap();
        let names: Vec<_> = r
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["base", "ck1", "vm1", "vm2", "vm3"]);
        assert!(
            r.list()
                .await
                .unwrap()
                .iter()
                .all(|s| s.status == Status::Stopped)
        );

        // Checkpoints/bases refuse to boot.
        let err = r
            .start(&StartSpec::new("ck1", ImageSource::Existing))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("fork source"));

        // Deleting the checkpoint keeps layers still used by vm1/vm2.
        r.delete("ck1").await.unwrap();
        for vm in ["vm1", "vm2", "vm3"] {
            let d = r.load(vm).unwrap().disk;
            assert!(img::chain_files(&d).await.is_ok(), "{vm} chain broken");
        }
        for vm in ["vm1", "vm2", "vm3", "base"] {
            r.delete(vm).await.unwrap();
        }
        let left = std::fs::read_dir(root.join(LAYERS))
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(left, 0, "unreferenced layers should be garbage-collected");
        assert!(image.exists(), "the source image is never deleted");
    }

    #[test]
    fn a_vm_with_a_policy_runs_in_a_namespace_that_holds_it() {
        let rules = crate::egress::EgressPolicy {
            allow: vec!["1.1.1.1".into()],
            deny: vec!["all".into()],
        }
        .parse()
        .unwrap();
        let s = netns_script(&rules);
        // The rules, on OUTPUT (QEMU itself opens the guest's
        // connections), then QEMU: never the other way round.
        let rules_at = s.find("iptables-restore --noflush").expect("rules");
        let exec_at = s.find("exec unshare -m").expect("exec");
        assert!(rules_at < exec_at, "{s}");
        assert!(s.contains("iptables -C OUTPUT -j CUA_EGRESS"), "{s}");
        assert!(
            s.contains("-A CUA_EGRESS -m conntrack --ctorigdst 1.1.1.1/32 -j ACCEPT"),
            "{s}"
        );
        // The guest's resolver (10.0.2.3) is blocked by `all`, through
        // pasta's DNS forward, which QEMU's resolv.conf names.
        assert!(
            s.contains("-A CUA_EGRESS -d 169.254.1.53/32 -p udp --dport 53 -j DROP"),
            "{s}"
        );
        assert!(
            s.contains("printf 'nameserver 169.254.1.53\\n' > \"$PWD/resolv.conf\""),
            "{s}"
        );
        assert!(s.contains(
            "exec unshare -m sh -c 'mount --bind \"$0\" /etc/resolv.conf && exec \"$@\"' \"$PWD/resolv.conf\" \"$@\""
        ), "{s}");
        let allowed = crate::egress::EgressPolicy {
            allow: vec!["10.0.2.3".into()],
            deny: vec!["all".into()],
        }
        .parse()
        .unwrap();
        assert!(netns_script(&allowed).contains("-d 169.254.1.53/32 -p udp --dport 53 -j ACCEPT"));
        let a = netns_pasta_args(&[4000, 5901]);
        let has = |pair: [&str; 2]| a.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        assert!(has(["-t", "127.0.0.1/4000:4000"]) && has(["-t", "127.0.0.1/5901:5901"]));
        assert!(has(["--dns-forward", "169.254.1.53"]));
        assert!(
            a.contains(&"--no-map-gw".to_string()),
            "no way to the host's loopback"
        );
        assert_eq!(a[a.len() - 2..], ["--".to_string(), "/bin/sh".to_string()]);
    }

    #[test]
    fn parses_info_usernet() {
        // Captured from QEMU 11 while sshd was down, then up.
        let down = "Hub -1 (net0):\r\n  Protocol[State]    FD  Source Address  Port   Dest. Address  Port RecvQ SendQ\r\n  \
                    TCP[HOST_FORWARD]  18       127.0.0.1 52227       10.0.2.15    22     0     0\r\n  \
                    UDP[239 sec]       53       10.0.2.15  5355     224.0.0.252  5355     0     0\r\n";
        let up = "Hub -1 (net0):\r\n  Protocol[State]    FD  Source Address  Port   Dest. Address  Port RecvQ SendQ\r\n  \
                  TCP[ESTABLISHED]   56       127.0.0.1 52227       10.0.2.15    22     0     0\r\n  \
                  TCP[HOST_FORWARD]  18       127.0.0.1 52227       10.0.2.15    22     0     0\r\n";
        assert!(!usernet_established(down, 52227, 22));
        assert!(usernet_established(up, 52227, 22));
        assert!(!usernet_established(up, 52228, 22));
        assert!(!usernet_established(up, 52227, 80));
    }

    /// Admission counts VMs admitted moments ago (pending) and refuses with
    /// HostCapacity, saying which limit; a failed launch gives its slot back.
    #[test]
    fn a_recording_vm_counts_its_own_ram_file_once() {
        // A golden snapshot being recorded writes its RAM file (share=on):
        // that file is its RAM, not a shared one, so admission must not add
        // it on top of the guest's memory (a 4 GiB bake counted as 8 GiB
        // did not fit a 7.8 GiB CI runner).
        let rec = Memory::File {
            path: "/g/mem.ram".into(),
            shared: true,
        };
        assert_eq!(admission_file(&rec), None);
        let restore = Memory::File {
            path: "/g/mem.ram".into(),
            shared: false,
        };
        assert_eq!(admission_file(&restore), Some(PathBuf::from("/g/mem.ram")));
        assert_eq!(admission_file(&Memory::Ram), None);
    }

    #[test]
    fn admission_counts_pending_vms_and_refuses_with_host_capacity() {
        let d = tempfile::tempdir().unwrap();
        let cpus = crate::density::host_cpus();
        let r = QemuRuntime::new(QemuConfig {
            root: d.path().to_path_buf(),
            density: crate::density::DensityConfig {
                vcpu_overcommit: 1.0,
                // Memory is not what this test limits.
                max_rss_fraction: 1.0,
                min_available_mb: 1,
                enabled: true,
            },
            ..Default::default()
        });
        r.admit("a", cpus, 64, None).unwrap();
        let e = r.admit("b", 1, 64, None).unwrap_err();
        assert!(
            matches!(&e, VmmError::HostCapacity(m) if m.contains("VM CPU limit")),
            "{e}"
        );
        assert!(e.to_string().contains("CUA_VM_VCPU_OVERCOMMIT"), "{e}");
        // `a` failed to launch (or was stopped, or finished launching and now
        // counts what it measures): it holds nothing back as pending.
        r.release("a");
        assert!(r.density().pending.is_empty());
        r.admit("b", 1, 64, None).unwrap();
        // Admitting the same name again replaces its pending slot.
        r.admit("b", 1, 64, None).unwrap();
        assert_eq!(r.density().pending.len(), 1);
        let off = QemuRuntime::new(QemuConfig {
            root: d.path().to_path_buf(),
            density: crate::density::DensityConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        });
        off.admit("huge", 100_000, 1 << 30, None).unwrap();
    }

    #[test]
    fn checkpoints_of_vms_with_free_page_reporting_keep_ram_in_the_stream() {
        let snap = |ram_file: Option<PathBuf>, fpr: bool| MemorySnapshot {
            dir: PathBuf::from("/c"),
            ram_file,
            golden: false,
            one_shot: false,
            state_file: None,
            disk: None,
            free_page_reporting: fpr,
            headless: false,
            bulk: false,
        };
        assert!(
            needs_shared_ram(&snap(None, false)),
            "a plain stream is shared"
        );
        assert!(
            !needs_shared_ram(&snap(None, true)),
            "never under a balloon"
        );
        assert!(
            !needs_shared_ram(&snap(Some("/c/mem.ram".into()), false)),
            "already shared"
        );
    }

    #[test]
    fn free_page_reporting_only_where_qemu_returns_memory() {
        // Linux frees a discarded page; macOS QEMU's MADV_DONTNEED keeps it.
        assert!(free_page_reporting_default("linux", None));
        assert!(!free_page_reporting_default("macos", None));
        assert!(free_page_reporting_default("macos", Some("1")));
        assert!(!free_page_reporting_default("linux", Some("off")));
    }

    #[test]
    fn a_booting_guest_reports_small_free_blocks() {
        assert!(PAGE_REPORTING_PART.contains(&format!("echo {PAGE_REPORTING_ORDER} >")));
        let seed = crate::cloudinit::Seed::raw("vm1", "#cloud-config\nbootcmd:\n  - [true]\n")
            .with_part(PAGE_REPORTING_PART);
        // Both parts' bootcmd survive (lists are appended).
        assert!(seed.user_data.contains("[true]"), "{}", seed.user_data);
        assert!(seed.user_data.contains("page_reporting_order"));
        assert!(seed.user_data.contains("append"), "{}", seed.user_data);
        // Into an existing multipart: one archive with three parts.
        let three = seed.with_part("#cloud-config\nruncmd:\n  - [true]\n");
        assert_eq!(three.user_data.matches("--==cua-vmm-seed==--").count(), 1);
        assert_eq!(
            three
                .user_data
                .matches("Content-Type: text/cloud-config")
                .count(),
            3
        );
    }

    #[test]
    fn no_desktop_means_no_display_hardware() {
        let s = StartSpec::new("a", ImageSource::Existing);
        assert!(!no_desktop(&s));
        assert!(no_desktop(&s.clone().env("CUA_ENV_NO_DESKTOP", "true")));
        assert!(no_desktop(&s.clone().env("CUA_ENV_NO_DESKTOP", " 1 ")));
        assert!(!no_desktop(&s.env("CUA_ENV_NO_DESKTOP", "false")));
    }

    #[tokio::test]
    async fn missing_instance_and_bad_names() {
        let tmp = tempfile::tempdir().unwrap();
        // What a start says before any QEMU tool is needed: the same on a
        // host without them (CI's unit runners).
        let r = QemuRuntime::new(QemuConfig {
            root: tmp.path().to_path_buf(),
            preflight_tools: false,
            ..Default::default()
        });
        assert!(matches!(r.status("nope").await, Err(VmmError::NotFound(_))));
        assert!(matches!(
            r.start(&StartSpec::new("nope", ImageSource::Existing))
                .await,
            Err(VmmError::NotFound(_))
        ));
        assert!(matches!(
            r.fork("x", "bad name").await,
            Err(VmmError::Invalid(_))
        ));
        let err = r
            .start(&StartSpec::new("o", ImageSource::oci("example.com/x:1")))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("DiskResolver"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn install_without_sudo_runs_local_script() {
        let calls = std::sync::Mutex::new(Vec::new());
        install_qemu_with(
            Some((QemuPackageManager::Apt, PathBuf::from("apt-get"))),
            || false,
            async |program, args| {
                calls.lock().unwrap().push((
                    program.to_string(),
                    args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                ));
                Ok(String::new())
            },
        )
        .await
        .unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sh");
        assert_eq!(calls[0].1[1], include_str!("install-local.sh"));
        assert_eq!(calls[0].1[3], host::cua_home().to_string_lossy());
        assert!(calls.iter().all(|(program, _)| program != "sudo"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn install_with_sudo_preserves_package_commands() {
        let calls = std::sync::Mutex::new(Vec::new());
        install_qemu_with(
            Some((QemuPackageManager::Apt, PathBuf::from("apt-get"))),
            || true,
            async |program, args| {
                assert_eq!(program, "sudo");
                calls
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
                Ok(String::new())
            },
        )
        .await
        .unwrap();
        let expected: Vec<Vec<String>> = QemuPackageManager::Apt
            .commands("apt-get")
            .iter()
            .map(|args| args.iter().map(|a| a.to_string()).collect())
            .collect();
        assert_eq!(*calls.lock().unwrap(), expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn local_installer_fixtures() {
        let python = std::process::Command::new("python3")
            .arg("--version")
            .output();
        if python.is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
            return;
        }
        assert!(
            std::process::Command::new("python3")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/install_local.py"
                ))
                .status()
                .unwrap()
                .success()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_launch_refuses_unopenable_kvm_and_falls_back_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let r = rt(tmp.path());
        let denied = Err(std::io::ErrorKind::PermissionDenied.into());
        let error = r
            .accel_for_with(Arch::host(), &denied, || true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("permission denied"), "{error}");
        assert!(error.contains("sudo usermod -aG kvm \"$USER\""), "{error}");
        assert_eq!(
            r.accel_for_with(Arch::host(), &denied, || false).unwrap(),
            "tcg"
        );
        let missing = Err(std::io::ErrorKind::NotFound.into());
        assert_eq!(r.accel_for(Arch::host(), &missing).unwrap(), "tcg");
    }

    #[test]
    fn only_a_qmp_timeout_counts_as_a_stalled_kvm_launch() {
        assert!(kvm_stalled(&VmmError::Qmp(
            "timed out waiting for QEMU".into()
        )));
        assert!(!kvm_stalled(&VmmError::Qmp(
            "connection closed by QEMU".into()
        )));
        assert!(!kvm_stalled(&VmmError::other(
            "qemu started but VM status is 'paused'"
        )));
    }

    #[test]
    fn explicit_tcg_override_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let r = QemuRuntime::new(QemuConfig {
            root: tmp.path().to_path_buf(),
            accel: Some("tcg".into()),
            ..Default::default()
        });
        let denied = Err(std::io::ErrorKind::PermissionDenied.into());
        assert_eq!(r.accel_for(Arch::host(), &denied).unwrap(), "tcg");
    }

    /// Records the credentials each resolve got.
    struct SeenCreds(std::sync::Mutex<Vec<Option<String>>>);

    #[async_trait]
    impl DiskResolver for SeenCreds {
        async fn resolve(&self, _: &str, _: Arch) -> Result<PathBuf> {
            Err(VmmError::invalid("resolve without credentials"))
        }
        async fn resolve_with_credentials(
            &self,
            _: &str,
            _: Arch,
            creds: Option<&RegistryCredentials>,
        ) -> Result<PathBuf> {
            self.0
                .lock()
                .unwrap()
                .push(creds.map(|c| c.username.clone()));
            Err(VmmError::invalid("stop here"))
        }
    }

    #[tokio::test]
    async fn oci_pulls_get_the_registry_credentials_for_their_registry_only() {
        let tmp = tempfile::tempdir().unwrap();
        let seen = Arc::new(SeenCreds(Default::default()));
        let r = QemuRuntime::new(QemuConfig {
            root: tmp.path().to_path_buf(),
            resolver: Some(seen.clone()),
            preflight_tools: false,
            ..Default::default()
        });
        let mut spec = StartSpec::new("p1", ImageSource::oci("ghcr.io/me/private:1"));
        spec.registry_auth = Some(RegistryCredentials::new("me", "pw").for_registry("ghcr.io"));
        let _ = r.start(&spec).await;
        // Other registry: no credentials leak to it.
        let mut other = StartSpec::new("p2", ImageSource::oci("quay.io/x/y:1"));
        other.registry_auth = spec.registry_auth.clone();
        let _ = r.start(&other).await;
        // Unscoped credentials apply to the image's registry.
        let mut unscoped = StartSpec::new("p3", ImageSource::oci("quay.io/x/y:1"));
        unscoped.registry_auth = Some(RegistryCredentials::new("u", "pw"));
        let _ = r.start(&unscoped).await;
        assert_eq!(
            *seen.0.lock().unwrap(),
            vec![Some("me".to_string()), None, Some("u".to_string())]
        );
    }

    #[tokio::test]
    async fn windows_env_is_refused_before_anything_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let r = rt(tmp.path());
        let disk = tmp.path().join("win.qcow2");
        std::fs::write(&disk, b"not really a disk").unwrap();
        let mut spec = StartSpec::new("w1", ImageSource::disk(&disk));
        spec.os = GuestOs::Windows;
        spec.env.insert("FOO".into(), "bar".into());
        let err = r.start(&spec).await.unwrap_err();
        assert!(matches!(err, VmmError::Unsupported { .. }), "{err}");
        assert!(
            matches!(r.load("w1"), Err(VmmError::NotFound(_))),
            "no state left"
        );
    }

    #[tokio::test]
    async fn a_resolver_without_auth_refuses_credentials() {
        struct Plain;
        #[async_trait]
        impl DiskResolver for Plain {
            async fn resolve(&self, _: &str, _: Arch) -> Result<PathBuf> {
                Ok(PathBuf::from("/nonexistent"))
            }
        }
        let creds = RegistryCredentials::new("me", "pw");
        let err = Plain
            .resolve_with_credentials("ghcr.io/me/x:1", Arch::host(), Some(&creds))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                VmmError::Unsupported {
                    backend: "qemu",
                    ..
                }
            ),
            "{err}"
        );
        assert!(
            Plain
                .resolve_with_credentials("ghcr.io/me/x:1", Arch::host(), None)
                .await
                .is_ok()
        );
    }

    struct LowUnder(PathBuf, u64);

    impl crate::disk::SpaceProbe for LowUnder {
        fn space(&self, path: &Path) -> std::io::Result<crate::disk::Space> {
            let available = if path.starts_with(&self.0) {
                self.1
            } else {
                500 * crate::disk::GIB
            };
            Ok(crate::disk::Space {
                available,
                total: 500 * crate::disk::GIB,
            })
        }
    }

    struct CountingResolver(std::sync::atomic::AtomicUsize);

    #[async_trait]
    impl DiskResolver for CountingResolver {
        async fn resolve(&self, _: &str, _: Arch) -> Result<PathBuf> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(VmmError::invalid("stop here"))
        }
    }

    #[tokio::test]
    async fn a_full_disk_fails_before_the_pull_and_overlays_need_less_than_blank_disks() {
        let tmp = tempfile::tempdir().unwrap();
        let resolver = Arc::new(CountingResolver(Default::default()));
        let r = QemuRuntime::new(QemuConfig {
            root: tmp.path().to_path_buf(),
            resolver: Some(resolver.clone()),
            preflight_tools: false,
            ..Default::default()
        });
        let min_free = crate::disk::CacheConfig::load().min_free;
        let set_available = |bytes| {
            crate::disk::set_probe(Some(Arc::new(LowUnder(tmp.path().to_path_buf(), bytes))))
        };

        set_available(min_free + crate::disk::GIB / 2);
        let err = r
            .start(&StartSpec::new("full", ImageSource::oci("ghcr.io/x/y:1")))
            .await
            .unwrap_err();
        let pulls = resolver.0.load(std::sync::atomic::Ordering::SeqCst);

        set_available(min_free + 2 * crate::disk::GIB);
        let backing = tmp.path().join("base.qcow2");
        std::fs::write(&backing, b"not really a disk").unwrap();
        let spec = StartSpec::new("o1", ImageSource::disk(&backing));
        let overlay = r
            .create_instance("o1", Some(&backing), None, &spec, None)
            .await
            .err();
        let blank = r.create_instance("b1", None, None, &spec, None).await.err();
        crate::disk::set_probe(None);

        assert!(matches!(err, VmmError::InsufficientDisk(_)), "{err}");
        assert_eq!(pulls, 0, "nothing is pulled when the overlay cannot fit");
        assert!(
            !matches!(overlay, Some(VmmError::InsufficientDisk(_))),
            "{overlay:?}"
        );
        assert!(
            matches!(blank, Some(VmmError::InsufficientDisk(_))),
            "{blank:?}"
        );
    }

    #[test]
    fn missing_qemu_tools_name_what_is_missing_with_a_host_hint() {
        use host::HostOs;
        let none = |_: &str| None;
        let all = |t: &str| Some(PathBuf::from(t));
        assert!(missing_tools(Arch::X86_64, HostOs::Windows, all).is_none());
        // A Windows PC: nothing cua can install; says how, and the way out.
        let err = missing_tools(Arch::X86_64, HostOs::Windows, none).unwrap();
        assert!(matches!(err, VmmError::Missing { .. }), "{err}");
        assert_eq!(
            err.to_string(),
            format!("QEMU (qemu-system-x86_64, qemu-img) is not available: {WINDOWS_QEMU_HINT}")
        );
        assert!(err.to_string().contains("create a Linux Space"));
        // Only qemu-img missing (a bare qemu-system on PATH).
        let no_img = |t: &str| (t != "qemu-img").then(|| PathBuf::from(t));
        let err = missing_tools(Arch::X86_64, HostOs::Linux, no_img).unwrap();
        assert!(
            err.to_string()
                .starts_with("QEMU (qemu-img) is not available: install QEMU")
        );
        assert!(err.to_string().contains("brew install qemu"));
    }

    #[tokio::test]
    async fn a_new_vm_without_qemu_fails_before_its_image_is_pulled() {
        let os = host::HostOs::current();
        if missing_tools(Arch::host(), os, host::which).is_none() {
            eprintln!("QEMU is installed here; the refusal is covered by missing_tools");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let resolver = Arc::new(CountingResolver(Default::default()));
        let r = QemuRuntime::new(QemuConfig {
            root: tmp.path().to_path_buf(),
            resolver: Some(resolver.clone()),
            ..Default::default()
        });
        let mut spec = StartSpec::new("win", ImageSource::oci("ghcr.io/trycua/windows:2022"));
        spec.os = GuestOs::Windows;
        let err = r.start(&spec).await.unwrap_err();
        assert!(matches!(err, VmmError::Missing { .. }), "{err}");
        assert_eq!(
            resolver.0.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "nothing is pulled"
        );
        assert!(matches!(r.load("win"), Err(VmmError::NotFound(_))));
    }

    fn recording(
        calls: &std::sync::Mutex<Vec<String>>,
        group: &'static str,
        failing: &'static str,
    ) -> impl AsyncFn(&str, &[&str]) -> Result<String> {
        async move |program: &str, args: &[&str]| {
            calls
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            match program {
                "stat" => Ok(format!("{group}\n")),
                "id" => Ok("alice\n".to_string()),
                _ if args.contains(&failing) => {
                    Err(VmmError::PermissionDenied("a password is required".into()))
                }
                _ => Ok(String::new()),
            }
        }
    }

    fn denied() -> std::io::Result<()> {
        Err(std::io::ErrorKind::PermissionDenied.into())
    }

    fn denied_until_setfacl(
        calls: &std::sync::Mutex<Vec<String>>,
    ) -> impl Fn() -> std::io::Result<()> {
        move || match calls.lock().unwrap().iter().any(|c| c.contains("setfacl")) {
            true => Ok(()),
            false => denied(),
        }
    }

    #[tokio::test]
    async fn denied_kvm_adds_the_user_to_the_kvm_group_then_grants_it_now() {
        let calls = Default::default();
        let note = grant_kvm(
            denied_until_setfacl(&calls),
            Some("bob".into()),
            false,
            recording(&calls, "kvm", ""),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            calls.into_inner().unwrap(),
            [
                "stat -c %G /dev/kvm",
                "sudo -n usermod -aG kvm bob",
                "sudo -n setfacl -m u:bob:rw /dev/kvm",
            ]
        );
        assert_eq!(note, "added bob to the kvm group and granted /dev/kvm now");
    }

    #[tokio::test]
    async fn a_failed_setfacl_still_adds_the_group_with_a_relogin_note() {
        let calls = Default::default();
        let note = grant_kvm(
            denied_until_setfacl(&calls),
            Some("bob".into()),
            false,
            recording(&calls, "kvm", "setfacl"),
        )
        .await
        .unwrap()
        .unwrap();
        let calls = calls.into_inner().unwrap();
        assert!(calls.contains(&"sudo -n usermod -aG kvm bob".to_string()));
        assert!(note.starts_with("added bob to the kvm group; "), "{note}");
        assert!(note.contains("log out and back in"), "{note}");
        assert!(note.contains("newgrp kvm"), "{note}");
    }

    #[tokio::test]
    async fn kvm_still_denied_after_setfacl_keeps_the_relogin_note() {
        let calls = Default::default();
        let note = grant_kvm(
            denied,
            Some("bob".into()),
            false,
            recording(&calls, "kvm", ""),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            calls.into_inner().unwrap().last().unwrap(),
            "sudo -n setfacl -m u:bob:rw /dev/kvm"
        );
        assert!(note.contains("log out and back in"), "{note}");
    }

    #[tokio::test]
    async fn an_empty_user_falls_back_to_id_un() {
        let calls = Default::default();
        let note = grant_kvm(
            denied_until_setfacl(&calls),
            Some(String::new()),
            false,
            recording(&calls, "kvm", ""),
        )
        .await
        .unwrap()
        .unwrap();
        let calls = calls.into_inner().unwrap();
        assert!(calls.contains(&"id -un".to_string()), "{calls:?}");
        assert!(calls.contains(&"sudo -n usermod -aG kvm alice".to_string()));
        assert!(note.starts_with("added alice "), "{note}");
    }

    #[tokio::test]
    async fn denied_kvm_without_sudo_is_an_error_naming_the_usermod_fix() {
        let calls = Default::default();
        let err = grant_kvm(
            denied,
            Some("bob".into()),
            false,
            recording(&calls, "kvm", "usermod"),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), host::kvm_hint(&denied().unwrap_err()));
        assert!(err.to_string().contains("sudo usermod -aG kvm"), "{err}");
        let calls = calls.into_inner().unwrap();
        assert!(!calls.iter().any(|c| c.contains("setfacl")), "{calls:?}");
    }

    #[tokio::test]
    async fn kvm_owned_by_another_group_is_the_hint_without_sudo() {
        let calls = Default::default();
        let err = grant_kvm(
            denied,
            Some("bob".into()),
            false,
            recording(&calls, "root", ""),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), host::kvm_hint(&denied().unwrap_err()));
        let calls = calls.into_inner().unwrap();
        assert!(!calls.iter().any(|c| c.starts_with("sudo")), "{calls:?}");
    }

    #[tokio::test]
    async fn missing_kvm_runs_nothing_and_is_only_a_note() {
        let calls = Default::default();
        let missing = || Err(std::io::ErrorKind::NotFound.into());
        let note = grant_kvm(
            missing,
            Some("bob".into()),
            false,
            recording(&calls, "kvm", ""),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(note.contains("/dev/kvm is missing"), "{note}");
        assert!(calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dry_run_only_says_it_would_add_the_user_and_grant_kvm() {
        let calls = Default::default();
        let note = grant_kvm(
            denied,
            Some("bob".into()),
            true,
            recording(&calls, "kvm", ""),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            note.starts_with("would: add bob to the kvm group"),
            "{note}"
        );
        assert!(note.contains("grant it /dev/kvm now (setfacl)"), "{note}");
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn missing_qemu_hints_show_the_install_plan() {
        assert_eq!(
            QemuPackageManager::linux_hint(),
            format!(
                "Debian/Ubuntu: `{}`; Fedora: `{}`",
                QemuPackageManager::Apt.render(),
                QemuPackageManager::Dnf.render()
            )
        );
    }
    #[cfg(unix)]
    #[test]
    fn pool_alias_follows_the_claim_rename_and_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("qemu");
        let r = rt(&root);
        let member = root.join("cua-pool-a");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(member.join("disk.qcow2"), b"d").unwrap();
        r.point_alias("cua-pool-a", &member).unwrap();
        let via = r.run_link("cua-pool-a").join("disk.qcow2");
        assert_eq!(std::fs::read(&via).unwrap(), b"d");

        // A claim renames the directory and re-points the alias: the path
        // QEMU was started with still resolves to the same file.
        let claimed = root.join("space-1");
        std::fs::rename(&member, &claimed).unwrap();
        assert!(!via.exists(), "dangling until re-pointed");
        r.point_alias("cua-pool-a", &claimed).unwrap();
        assert_eq!(std::fs::read(&via).unwrap(), b"d");
        // The alias dir is not a VM.
        assert!(r.all_states().unwrap().is_empty());

        // Aliases of other VMs stay; this one goes with its QEMU.
        let other = root.join("cua-pool-b");
        std::fs::create_dir_all(&other).unwrap();
        r.point_alias("cua-pool-b", &other).unwrap();
        r.drop_aliases(Some(&claimed));
        assert!(std::fs::symlink_metadata(r.run_link("cua-pool-a")).is_err());
        assert!(r.run_link("cua-pool-b").exists());
        // A dangling alias (its VM deleted) is swept by the next sweep.
        std::fs::remove_dir_all(&other).unwrap();
        r.drop_aliases(None);
        assert!(std::fs::symlink_metadata(r.run_link("cua-pool-b")).is_err());
    }

    /// The bug behind `bdrv_inactivate_all() failed (-1)`: a claimed pool
    /// member could not migrate its memory to a file (checkpoint, fork) while
    /// running, because QEMU re-opens the images by the path it started
    /// with and a claim had renamed the directory. Boots a real (blank-disk)
    /// QEMU as a pool member, claims it and migrates it. Needs
    /// `qemu-system-x86_64` and `qemu-img` (KVM when present, else TCG).
    #[cfg(all(unix, target_arch = "x86_64"))]
    #[tokio::test]
    async fn claimed_pool_member_can_migrate_its_memory_to_a_file() {
        if img::qemu_img().is_err() || host::which("qemu-system-x86_64").is_none() {
            eprintln!("qemu missing; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("qemu");
        let r = rt(&root);
        let blank = tmp.path().join("blank.qcow2");
        img::create_blank(&blank, 1).await.unwrap();
        let member = format!("{POOL_MEMBER_PREFIX}t{}", std::process::id());
        let mut spec = StartSpec::new(&member, ImageSource::disk(&blank))
            .cpus(1)
            .memory_mb(256);
        spec.pool_member = true;
        r.start(&spec).await.unwrap();
        let space = format!("claimed-{}", std::process::id());
        r.claim(&member, &space, &ClaimIdentity::default())
            .await
            .unwrap();
        assert!(!r.dir(&member).exists(), "the claim renamed the directory");

        let result = async {
            let mut q = r.qmp(&space).await.map_err(|e| e.to_string())?;
            q.set_migrate_capabilities(&[("x-ignore-shared", false)])
                .await
                .map_err(|e| e.to_string())?;
            q.migrate_to_file(&tmp.path().join("state"), Duration::from_secs(120))
                .await
                .map_err(|e| e.to_string())
        }
        .await;
        r.delete(&space).await.unwrap();
        assert!(
            std::fs::symlink_metadata(r.run_link(&member)).is_err(),
            "the alias goes with the VM"
        );
        result.unwrap();
    }
}

const TPM_CLAIM_REFUSED: &str = "is being started or copied; try again";

/// Whether `e` is [`QemuRuntime::tpm_claim`]'s refusal.
fn tpm_claim_refused(e: &VmmError) -> bool {
    matches!(e, VmmError::Invalid(m) if m.ends_with(TPM_CLAIM_REFUSED))
}

/// Holds a VM's name in [`QemuRuntime::tpm_busy`] until dropped.
struct TpmBusy {
    set: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    name: String,
}

impl Drop for TpmBusy {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.name);
    }
}

/// What a recorded pid is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tpm {
    /// The swtpm of that record.
    Ours,
    /// Not alive, or something else now (a reused pid, a zombie).
    Gone,
    /// Alive, but `ps` cannot say what it is: never signalled.
    Unknown,
}

/// Whether `pid` is the swtpm launched with `record` for `tpm_dir`.
fn swtpm_state(pid: u32, record: &Path, tpm_dir: &Path) -> Tpm {
    if pid < 2 {
        return Tpm::Gone;
    }
    let Some(cmd) = host::pid_command(pid) else {
        return if host::pid_alive(pid) {
            Tpm::Unknown
        } else {
            Tpm::Gone
        };
    };
    // The shared pidfile earlier builds passed, spelled as they spelled it.
    let file = if record.file_name().is_some_and(|n| n == "swtpm.pid") {
        format!("file={}/../swtpm.pid", tpm_dir.display())
    } else {
        format!("file={}", record.display())
    };
    let dir = format!("dir={}", tpm_dir.display());
    // Linux keeps argument boundaries (and a path with spaces): the program
    // and arguments must be exactly ours. Elsewhere the program is the text
    // before the `socket` subcommand.
    let (is_swtpm, ours) = if let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        let args: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
        let exe = std::str::from_utf8(args[0]).unwrap_or_default();
        (
            Path::new(exe).file_name().is_some_and(|n| n == "swtpm"),
            args.contains(&file.as_bytes()) && args.contains(&dir.as_bytes()),
        )
    } else {
        text_identity(&cmd, &file, &dir)
    };
    if is_swtpm && ours {
        Tpm::Ours
    } else {
        Tpm::Gone
    }
}

/// Without argument boundaries the record and directory must be the last
/// options, in the launch script's order, so no longer path (`tpm -backup`,
/// `tpm backup`) can pass for it.
fn names_dir_text(cmd: &str, file: &str, dir: &str) -> bool {
    cmd.ends_with(&format!("--pid {file} --tpmstate {dir}"))
}

/// (is swtpm, is this launch's) from `ps` text: the program is the text
/// before the `socket` subcommand.
fn text_identity(cmd: &str, file: &str, dir: &str) -> (bool, bool) {
    let exe = cmd.split_once(" socket ").map_or_else(
        || cmd.split_whitespace().next().unwrap_or_default(),
        |(e, _)| e,
    );
    (
        Path::new(exe).file_name().is_some_and(|n| n == "swtpm"),
        names_dir_text(cmd, file, dir),
    )
}

/// The pid a swtpm record holds.
fn record_pid(record: &Path) -> Option<u32> {
    std::fs::read_to_string(record)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
}

/// The swtpm records (`swtpm.<id>.pid`) in a VM's run directory.
fn tpm_records(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("swtpm.") && n.ends_with(".pid")
        })
        .map(|e| e.path())
        .collect()
}

/// End the swtpm a record names: TERM, then KILL. The record is removed only
/// once that swtpm is gone; otherwise it stays and this is an error.
async fn end_record(record: &Path, tpm_dir: &Path) -> Result<()> {
    let Some(pid) = record_pid(record) else {
        return Ok(());
    };
    for (sig, polls) in [("TERM", 20), ("KILL", 10)] {
        if swtpm_state(pid, record, tpm_dir) != Tpm::Ours {
            break;
        }
        host::signal(pid, sig);
        for _ in 0..polls {
            if swtpm_state(pid, record, tpm_dir) != Tpm::Ours {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    if swtpm_state(pid, record, tpm_dir) == Tpm::Gone {
        let _ = std::fs::remove_file(record);
        return Ok(());
    }
    Err(VmmError::other(format!(
        "the TPM of '{}' (swtpm {pid}) did not exit",
        tpm_dir
            .parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
    )))
}

/// Ends the QEMU (and the TPM record) a start launched when the start does
/// not finish (its future dropped by a cancel, or an early error). The
/// launch shell itself is covered by `complete_or_undo`; this covers the
/// time from the reported pid until the start succeeds. It ends only the
/// pid and record it was handed.
struct LaunchGuard {
    armed: bool,
    /// The launched process, once known.
    pid: Option<u32>,
    /// The swtpm record and TPM directory of this launch.
    tpm: Option<(PathBuf, PathBuf)>,
    /// Keeps the VM's TPM busy until the clean-up has run.
    busy: Option<Arc<TpmBusy>>,
}

/// TERM, then KILL, a QEMU nobody will use.
async fn end_launched(pid: u32) {
    host::signal(pid, "TERM");
    for _ in 0..20 {
        if !host::pid_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    host::signal(pid, "KILL");
}

impl LaunchGuard {
    fn new() -> Self {
        Self {
            armed: true,
            pid: None,
            tpm: None,
            busy: None,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if !self.armed || (self.pid.is_none() && self.tpm.is_none()) {
            return;
        }
        let (pid, tpm, busy) = (self.pid, self.tpm.take(), self.busy.take());
        crate::cleanup::spawn(async move {
            let _busy = busy;
            if let Some(p) = pid {
                end_launched(p).await;
            }
            if let Some((r, d)) = tpm {
                let _ = end_record(&r, &d).await;
            }
        });
    }
}

#[cfg(all(test, unix))]
mod launch_guard_tests {
    use super::*;

    #[tokio::test]
    async fn a_dropped_start_ends_the_qemu_it_launched() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut g = LaunchGuard::new();
        g.pid = Some(pid);
        drop(g);
        assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
        // Reaped by the test: the signal reached it.
        let st = child.wait().unwrap();
        assert!(!st.success());
    }

    #[tokio::test]
    async fn a_finished_start_leaves_the_qemu_alone() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut g = LaunchGuard::new();
        g.pid = Some(child.id());
        g.disarm();
        drop(g);
        // The pending list is process-wide: other tests' guards may still be
        // finishing, so give them the same budget the other guard test does.
        assert!(crate::cleanup::settle(Duration::from_secs(10)).await);
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
