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
pub mod firmware;
pub mod img;
pub mod qmp;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::{Result, VmmError};
use crate::exec::{GuestExec, SshExec};
use crate::host;
use crate::runtime::{DiskResolver, Runtime};
use crate::types::*;

pub use args::Firmware;
pub use qmp::QmpClient;

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
        }
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
}

const LAYERS: &str = "_layers";

impl QemuRuntime {
    pub fn new(cfg: QemuConfig) -> Self {
        Self {
            cfg,
            lock: Mutex::new(()),
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
            "install QEMU (macOS: `brew install qemu`; Debian/Ubuntu: `apt install qemu-system qemu-utils`; \
             Fedora: `dnf install qemu-kvm qemu-img`) or enable allow_install to let cua provision it",
        ))
    }

    fn accel_for(&self, arch: Arch) -> String {
        self.cfg
            .accel
            .clone()
            .unwrap_or_else(|| host::qemu_accel(arch).to_string())
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
    /// explicit preference wins (a Fleet template's `firmware`); otherwise
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
        let fw = firmware::find_uefi(arch, &bin).ok_or_else(|| {
            VmmError::missing(
                format!("UEFI firmware for {arch}"),
                "install edk2 (Homebrew qemu ships it; Debian: `apt install qemu-efi-aarch64 ovmf`)",
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
            crate::disk::VM_CREATE_ESTIMATE,
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
                extra_args: spec_like.extra_args.clone(),
                snapshots: Vec::new(),
                paused: false,
                source,
                created_at: host::now_secs(),
                gpu: spec_like.gpu.clone(),
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
        if let Some(seed) = crate::cloudinit::seed_for_spec(&st.name, spec)? {
            let path = self.dir(&st.name).join("seed.iso");
            seed.write_iso(&path)?;
            st.seed_iso = Some(path);
        }
        Ok(())
    }

    /// Launch QEMU for an existing instance entry.
    async fn launch(
        &self,
        st: &mut QemuState,
        spec: &StartSpec,
        loadvm: Option<String>,
    ) -> Result<()> {
        let bin = self.binary(st.arch).await?;
        let dir = self.dir(&st.name);
        let mut guest_ports: Vec<u16> = spec.ports.clone();
        if st.ssh.is_some() && !guest_ports.contains(&22) {
            guest_ports.push(22);
        }
        let mut free = host::free_ports(guest_ports.len() + 1)?;
        let qmp_port = free.pop().expect("allocated");
        st.ports = guest_ports.iter().copied().zip(free).collect();
        st.qmp_port = Some(qmp_port);
        st.vnc_display = if self.cfg.vnc {
            host::free_vnc_display(0)
        } else {
            None
        };
        st.accel = self.accel_for(st.arch);
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

        let pidfile = dir.join("qemu.pid");
        let _ = std::fs::remove_file(&pidfile);
        let cfg = args::LaunchConfig {
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
            firmware: st.firmware.clone(),
            forwards: st.ports.iter().map(|(g, h)| (*h, *g)).collect(),
            restrict_network: st.restrict_network,
            vnc_display: st.vnc_display,
            qmp_port,
            serial_log: Some(dir.join("serial.log")),
            pidfile: Some(pidfile.clone()),
            daemonize: false,
            loadvm,
            virgl_render_node,
            extra: st.extra_args.clone(),
        };
        let argv = args::build(&cfg);
        std::fs::write(
            dir.join("cmdline"),
            format!("{} {}\n", bin.display(), argv.join(" ")),
        )?;
        tracing::info!(sandbox = %st.name, accel = %st.accel, "starting qemu");

        // Launch detached: on Unix a throwaway `sh` backgrounds QEMU and exits,
        // so QEMU is re-parented to init (no zombie in long-lived callers) and
        // survives this process, while its stderr (crash reports, hvf/tcg
        // errors) goes to qemu.log instead of /dev/null as with -daemonize.
        let log_path = dir.join("qemu.log");
        #[cfg(unix)]
        {
            let out = tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("\"$0\" \"$@\" </dev/null >>\"$CUA_QEMU_LOG\" 2>&1 & echo $!")
                .arg(&bin)
                .args(&argv)
                .env("CUA_QEMU_LOG", &log_path)
                .output()
                .await?;
            let pid = String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u32>()
                .map_err(|e| VmmError::other(format!("could not launch {}: {e}", bin.display())))?;
            st.pid = Some(pid);
        }
        #[cfg(not(unix))]
        {
            let log = std::fs::File::create(&log_path)?;
            let child = std::process::Command::new(&bin)
                .args(&argv)
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()?;
            st.pid = Some(child.id());
        }
        self.save(st)?;

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
                Ok(q) => break q,
                Err(e) if tokio::time::Instant::now() >= deadline => return Err(e),
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        };
        let status = q.status().await?;
        if status != "running" {
            return Err(VmmError::other(format!(
                "qemu started but VM status is '{status}'"
            )));
        }
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

    /// Delete layer files no remaining entry references.
    async fn gc_layers(&self) -> Result<()> {
        let layers = self.layers_dir();
        let Ok(entries) = std::fs::read_dir(&layers) else {
            return Ok(());
        };
        let mut used: BTreeSet<PathBuf> = BTreeSet::new();
        for st in self.all_states()? {
            if st.disk.exists() {
                for f in img::chain_files(&st.disk).await.unwrap_or_default() {
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
        Ok(())
    }

    fn all_states(&self) -> Result<Vec<QemuState>> {
        let Ok(rd) = std::fs::read_dir(&self.cfg.root) else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == LAYERS || !e.path().join("state.json").exists() {
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
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        !host::pid_alive(pid)
    }

    async fn force_stop(&self, st: &mut QemuState, graceful: Duration) -> Result<()> {
        let Some(pid) = st.pid.filter(|p| host::pid_alive(*p)) else {
            st.pid = None;
            return Ok(());
        };
        if !graceful.is_zero() {
            if let Ok(mut q) = self.qmp(&st.name).await {
                let _ = q.system_powerdown().await;
            }
            if Self::wait_exit(pid, graceful).await {
                st.pid = None;
                return Ok(());
            }
        }
        if let Ok(mut q) = self.qmp(&st.name).await {
            let _ = q.quit().await;
        }
        if !Self::wait_exit(pid, Duration::from_secs(5)).await {
            host::signal(pid, "KILL");
            Self::wait_exit(pid, Duration::from_secs(5)).await;
        }
        st.pid = None;
        Ok(())
    }
}

impl QemuRuntime {
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
                if !Self::is_running(&self.load(&st.name)?) {
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
                tokio::time::sleep(Duration::from_secs(1)).await;
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
        let Ok(Ok(_stream)) = tokio::time::timeout(
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
        let mut q = self.qmp(&st.name).await?;
        for _ in 0..10 {
            let table = q.hmp("info usernet").await?;
            if usernet_established(&table, host_port, guest_port) {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(false)
    }
}

/// Guest OS and firmware for a new x86 instance. What the caller said wins;
/// what it left open comes from the disk's partition table, so any Fleet
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

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// Install QEMU through the host package manager (explicit opt-in).
pub async fn install_qemu() -> Result<()> {
    match host::HostOs::current() {
        host::HostOs::Macos => {
            let brew = host::which("brew").ok_or_else(|| {
                VmmError::missing(
                    "Homebrew",
                    "install https://brew.sh, then `brew install qemu`",
                )
            })?;
            tracing::info!("installing QEMU with Homebrew");
            host::run(brew, &["install", "qemu"]).await?;
        }
        host::HostOs::Linux => {
            if let Some(apt) = host::which("apt-get") {
                let apt = apt.display().to_string();
                tracing::info!("installing QEMU with apt (sudo -n)");
                host::run(
                    "sudo",
                    &[
                        "-n",
                        &apt,
                        "install",
                        "-y",
                        "qemu-system-x86",
                        "qemu-system-arm",
                        "qemu-utils",
                        "qemu-efi-aarch64",
                        "ovmf",
                    ],
                )
                .await?;
            } else if let Some(dnf) = host::which("dnf") {
                let dnf = dnf.display().to_string();
                host::run(
                    "sudo",
                    &[
                        "-n",
                        &dnf,
                        "install",
                        "-y",
                        "qemu-kvm",
                        "qemu-img",
                        "edk2-ovmf",
                        "edk2-aarch64",
                    ],
                )
                .await?;
            } else {
                return Err(VmmError::missing(
                    "QEMU",
                    "no supported package manager found (apt-get/dnf)",
                ));
            }
        }
        host::HostOs::Windows => {
            return Err(VmmError::missing(
                "QEMU",
                "download QEMU for Windows from https://qemu.weilnetz.de/ and put it on PATH",
            ));
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
            extra_args: vec![],
            snapshots: vec![],
            paused: false,
            source: source.clone(),
            created_at: host::now_secs(),
            gpu: None,
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
        crate::cloudinit::check_guest_support(spec)?;
        let mut st = {
            let _g = self.lock.lock().await;
            match self.load(&spec.name) {
                Ok(st) => {
                    if st.kind != EntryKind::Instance {
                        return Err(VmmError::invalid(format!(
                            "'{}' is a {:?} (a fork source); fork it into a new instance first",
                            spec.name, st.kind
                        )));
                    }
                    st
                }
                Err(VmmError::NotFound(_)) => {
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
                    self.create_instance(&spec.name, backing.as_deref(), iso, spec, source)
                        .await?
                }
                Err(e) => return Err(e),
            }
        };

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
            if let Err(e) = self.launch(&mut st, spec, None).await {
                let _ = self.force_stop(&mut st, Duration::ZERO).await;
                let _ = self.save(&st);
                return Err(e);
            }
        }
        let inst = self.instance(&st);
        if !spec.probes.is_empty() {
            crate::progress::report(crate::progress::Progress::phase(
                crate::progress::Phase::WaitingForServices,
            ));
        }
        self.wait_probes(&st, &inst.endpoints, spec).await?;
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

    async fn resume(&self, name: &str) -> Result<Instance> {
        let mut q = self.qmp(name).await?;
        q.cont().await?;
        let mut st = self.load(name)?;
        st.paused = false;
        self.save(&st)?;
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
        if Self::is_running(&src) {
            return Err(VmmError::invalid(format!(
                "'{src_name}' is running; stop it (or checkpoint it) before forking"
            )));
        }
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
        let dir = self.dir(new_name);
        let disk = dir.join("disk.qcow2");
        img::create_overlay(&backing, &disk).await?;
        let firmware = match src.kind {
            EntryKind::Base => self.setup_firmware(&dir, src.arch, src.os, None).await?,
            _ => self.copy_vars(&src.firmware, &dir)?,
        };
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
            ..src
        };
        self.save(&st)
    }

    async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
        validate_name(checkpoint)?;
        let _g = self.lock.lock().await;
        let mut st = self.load(name)?;
        if Self::is_running(&st) {
            // Live: internal snapshot of RAM + devices + disks inside the
            // instance's qcow2. Restore with `restore()`, or fork `name@ckpt`
            // once stopped.
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
        let layer = self.freeze(&mut st).await?;
        self.save(&st)?;
        let dir = self.dir(checkpoint);
        let firmware = self.copy_vars(&st.firmware, &dir)?;
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
        match self.load(checkpoint)?.kind {
            EntryKind::Checkpoint => self.delete(checkpoint).await,
            k => Err(VmmError::invalid(format!(
                "'{checkpoint}' is a {k:?}, not a checkpoint"
            ))),
        }
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
        let _g = self.lock.lock().await;
        let mut st = self.load(name)?;
        self.force_stop(&mut st, Duration::ZERO).await?;
        std::fs::remove_dir_all(self.dir(name))?;
        self.gc_layers().await
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(dir: &Path) -> QemuRuntime {
        QemuRuntime::new(QemuConfig {
            root: dir.to_path_buf(),
            ..Default::default()
        })
    }

    /// Exercises create → fork → checkpoint → gc against real qemu-img
    /// without booting anything.
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

    #[tokio::test]
    async fn missing_instance_and_bad_names() {
        let tmp = tempfile::tempdir().unwrap();
        let r = rt(tmp.path());
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
}
