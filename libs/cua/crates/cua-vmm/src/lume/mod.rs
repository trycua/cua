//! Lume backend: macOS and Linux arm64 guests on Apple Virtualization.framework
//! through `lume serve` (port of `cua_sandbox/runtime/lume.py`).
//!
//! * **macOS / OCI images**: pull once into a stopped `cua-base-<sha12>` VM
//!   (same naming as the Python runtime, so bases are shared), then clone
//!   (APFS clonefile, instant) and run.
//! * **Linux disk images** ([`ImageSource::Disk`]): the qcow2/raw disk is
//!   converted to a raw image in `~/.cua/vmm/lume/<name>/disk.img`, a Linux VM
//!   shell is created through the API, and the VM is launched with
//!   `lume run --detach --disk-path <disk> --mount <cloud-init seed>` so the
//!   same NoCloud seed as the QEMU backend provisions SSH access.
//!
//! [`LumeRuntime::ensure_serving`] makes the backend zero-setup: if the API is
//! not reachable it starts `lume serve`, and when the binary is missing it runs
//! the official installer only if [`LumeConfig::allow_install`] is set.

pub mod client;
pub mod gpu;
pub mod lease;
pub mod owned;
pub mod ssh;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::error::{Result, VmmError};
use crate::exec::{GuestExec, PrefixExec};
use crate::host;
use crate::runtime::Runtime;
use crate::types::*;

pub use client::{CreateVm, LumeClient, RunVm, SetVm, SharedDir, VmDetails};
pub use owned::{OwnedKind, OwnedVm, OwnedVms};
pub use ssh::LumeSshExec;

/// File name of the env token in the macOS setup share (guest:
/// `/Volumes/My Shared Files/env-token`).
pub const MACOS_SETUP_TOKEN_FILE: &str = "env-token";

/// Official installer (same one the docs and the Python SDK point to).
pub const INSTALL_URL: &str =
    "https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh";

/// Configuration for [`LumeRuntime`].
#[derive(Clone, Debug)]
pub struct LumeConfig {
    /// API base URL (default `$LUME_API` or `http://127.0.0.1:7777`).
    pub url: String,
    /// Start `lume serve` when the API is unreachable.
    pub spawn_serve: bool,
    /// Run the official installer when the `lume` binary is missing.
    pub allow_install: bool,
    /// State for Linux disk conversions and seeds (default `~/.cua/vmm/lume`).
    pub root: PathBuf,
    /// How long to wait for a booted VM to report an IP.
    pub ip_timeout: Duration,
    /// How long a registry pull may take.
    pub pull_timeout: Duration,
    /// Host DHCP lease file consulted (read-only) for guest IPs.
    pub leases_path: String,
}

impl Default for LumeConfig {
    fn default() -> Self {
        Self {
            url: std::env::var("LUME_API").unwrap_or_else(|_| "http://127.0.0.1:7777".into()),
            spawn_serve: true,
            allow_install: false,
            root: host::cua_home().join("vmm").join("lume"),
            ip_timeout: Duration::from_secs(300),
            pull_timeout: Duration::from_secs(3600),
            leases_path: lease::LEASES_PATH.into(),
        }
    }
}

/// The Lume backend.
pub struct LumeRuntime {
    cfg: LumeConfig,
    client: LumeClient,
    /// The host preference GPU acceleration sets ([`gpu`]).
    gpu_pref: Arc<dyn gpu::GpuPreference>,
}

/// The reference `lume pull` takes: `tag-ref@sha256:…` (how the daemon
/// names a Lume image so its cached base follows the digest) pulls by tag,
/// since lume cannot pull by digest; anything else is used as given.
pub fn lume_pull_ref(reference: &str) -> &str {
    match reference.rsplit_once('@') {
        Some((tag_ref, digest)) if digest.starts_with("sha256:") && tag_ref.contains(':') => {
            tag_ref
        }
        _ => reference,
    }
}

/// Stable base-VM name for an OCI reference: `cua-base-<sha256[:12]>`
/// (identical to `_base_vm_name` in the Python runtime). Pass
/// `tag-ref@sha256:…` so a moved tag (`macos:26`) gets a fresh base
/// instead of cloning the stale one cached under the tag.
pub fn base_vm_name(reference: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(reference.as_bytes());
    let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
    format!("cua-base-{}", &hex[..12])
}

/// Locate the `lume` binary.
pub fn lume_bin() -> Option<PathBuf> {
    host::which("lume")
}

/// Deletes a clone whose create was cut off (dropped) before its first
/// boot was requested: Lume finishes a clone whose request was dropped, so
/// the delete waits (bounded) for the VM to appear.
struct CloneGuard {
    client: LumeClient,
    name: String,
    root: PathBuf,
    finished: bool,
}

impl CloneGuard {
    fn new(client: LumeClient, name: &str, root: &std::path::Path) -> Self {
        Self {
            client,
            name: name.into(),
            root: root.to_path_buf(),
            finished: false,
        }
    }

    fn done(&mut self) {
        self.finished = true;
    }
}

impl Drop for CloneGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let (client, name, dir) = (
            self.client.clone(),
            self.name.clone(),
            self.root.join(&self.name),
        );
        crate::cleanup::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                match client.get(&name).await {
                    Ok(Some(vm)) if vm.status != "provisioning" => {
                        if vm.status == "running" {
                            let _ = client.stop_for_delete(&name).await;
                        }
                        match client.delete(&name).await {
                            Ok(()) => tracing::info!(vm = %name, "deleted a cancelled clone"),
                            Err(e) => {
                                tracing::warn!(vm = %name, error = %e, "could not delete a cancelled clone")
                            }
                        }
                        break;
                    }
                    _ if tokio::time::Instant::now() >= deadline => break,
                    _ => tokio::time::sleep(Duration::from_millis(250)).await,
                }
            }
            OwnedVms::default().forget(&name);
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

impl LumeRuntime {
    pub fn new(cfg: LumeConfig) -> Self {
        let client = LumeClient::new(cfg.url.clone());
        Self {
            cfg,
            client,
            gpu_pref: Arc::new(gpu::Defaults),
        }
    }

    /// Uses `pref` for the GPU preference instead of the host's `defaults`
    /// (tests).
    pub fn with_gpu_preference(mut self, pref: Arc<dyn gpu::GpuPreference>) -> Self {
        self.gpu_pref = pref;
        self
    }

    fn gpu(&self) -> gpu::Gpu<'_> {
        gpu::Gpu::new(&self.cfg.root, self.gpu_pref.as_ref())
    }

    /// GPU acceleration on this host ([`crate::gpu::lume_support`]).
    pub fn gpu_support(&self) -> crate::gpu::GpuSupport {
        crate::gpu::lume_support(lume_bin().is_some())
    }

    /// After `name` was deleted: GPU acceleration's host preference goes
    /// when cua set it and no VM needs it any more.
    fn forget_gpu(&self, name: &str) {
        if let Err(e) = self.gpu().after_delete(name) {
            tracing::warn!(vm = name, error = %e, "could not update the GPU preference");
        }
    }

    /// Before a macOS VM boots: GPU acceleration's host preference, when
    /// the VM has it.
    fn before_macos_boot(&self, name: &str) -> Result<()> {
        self.gpu()
            .before_start(name)
            .map_err(|e| VmmError::other(format!("GPU acceleration for {name}: {e}")))
    }

    pub fn with_defaults() -> Self {
        Self::new(LumeConfig::default())
    }

    pub fn client(&self) -> &LumeClient {
        &self.client
    }

    fn port(&self) -> u16 {
        self.cfg
            .url
            .rsplit(':')
            .next()
            .and_then(|p| p.trim_end_matches('/').parse().ok())
            .unwrap_or(7777)
    }

    /// Make sure `lume serve` answers, starting (and, if allowed, installing)
    /// it as needed. Never restarts or stops a server that is already running.
    pub async fn ensure_serving(&self) -> Result<()> {
        if self.client.reachable().await {
            return Ok(());
        }
        if !cfg!(target_os = "macos") {
            return Err(VmmError::missing(
                "Lume",
                "Lume runs on macOS (Apple Silicon) hosts only",
            ));
        }
        if !self.cfg.spawn_serve {
            return Err(VmmError::missing(
                format!("lume serve at {}", self.cfg.url),
                "start it with `lume serve`",
            ));
        }
        let bin = match lume_bin() {
            Some(b) => b,
            None if self.cfg.allow_install => {
                install_lume().await?;
                if self.client.reachable().await {
                    return Ok(()); // installer's LaunchAgent started it
                }
                lume_bin().ok_or_else(|| {
                    VmmError::missing("lume", "installer finished but `lume` is not on PATH")
                })?
            }
            None => {
                return Err(VmmError::missing(
                    "lume",
                    format!(
                        "install it with `/bin/bash -c \"$(curl -fsSL {INSTALL_URL})\"` or enable allow_install to let cua do it"
                    ),
                ));
            }
        };
        std::fs::create_dir_all(&self.cfg.root)?;
        let log = std::fs::File::create(self.cfg.root.join("serve.log"))?;
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(["serve", "--port", &self.port().to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0); // outlive this process; not tied to our terminal
        }
        let child = cmd.spawn()?;
        tracing::info!(pid = child.id(), "started lume serve");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while tokio::time::Instant::now() < deadline {
            if self.client.reachable().await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(VmmError::other(format!(
            "started `lume serve` but {} did not answer within 30s (see {})",
            self.cfg.url,
            self.cfg.root.join("serve.log").display()
        )))
    }

    /// Wait until the guest has an IP. Each poll asks `lume serve` and, when
    /// it has no address yet, looks the VM's MAC up in the host's vmnet DHCP
    /// leases and ARP table ([`lease`]); `lume serve` can lag the lease by
    /// many seconds or miss it. Polls back off from 0.5 s to 3 s. A VM that
    /// never reaches `running`, or stops after it did, fails fast with the
    /// tail of its run log instead of waiting out the timeout.
    async fn wait_for_ip(&self, name: &str) -> Result<VmDetails> {
        let start = tokio::time::Instant::now();
        let deadline = start + self.cfg.ip_timeout;
        let never_ran_after = Duration::from_secs(45);
        let mut saw_running = false;
        let mut delay = Duration::from_millis(500);
        let mut last_state = String::new();
        let leases_path = std::path::PathBuf::from(&self.cfg.leases_path);
        let before = lease::read_leases(&leases_path).await;
        // Bounded by the deadline; at most ip_timeout / 0.5 s iterations.
        while tokio::time::Instant::now() < deadline {
            let vm = self.client.get(name).await?;
            if let Some(mut vm) = vm {
                let state = format!(
                    "status={} api_ip={:?} mac={:?}",
                    vm.status, vm.ip_address, vm.mac_address
                );
                if state != last_state {
                    tracing::debug!(vm = name, elapsed = ?start.elapsed(), %state, "lume VM state");
                    last_state = state;
                }
                match vm.status.as_str() {
                    "running" => saw_running = true,
                    "stopped" if saw_running => {
                        return Err(VmmError::other(format!(
                            "lume VM '{name}' stopped while booting{}",
                            self.run_log_tail(name)
                        )));
                    }
                    "stopped" if start.elapsed() > never_ran_after => {
                        return Err(VmmError::other(format!(
                            "lume VM '{name}' never started ({}s after `lume run`){}",
                            never_ran_after.as_secs(),
                            self.run_log_tail(name)
                        )));
                    }
                    _ => {}
                }
                if vm.ip().is_some() {
                    tracing::debug!(vm = name, elapsed = ?start.elapsed(), ip = ?vm.ip_address, "IP from lume serve");
                    return Ok(vm);
                }
                let mac = vm
                    .mac_address
                    .clone()
                    .or_else(|| lease::config_mac(&host::home_dir(), name));
                if saw_running
                    && let Some((ip, source)) =
                        lease::lookup(mac.as_deref(), name, &before, &leases_path).await
                {
                    tracing::info!(vm = name, elapsed = ?start.elapsed(), %ip, source, "guest IP found on the host before lume serve reported it");
                    vm.ip_address = Some(ip);
                    return Ok(vm);
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 3 / 2).min(Duration::from_secs(3));
        }
        Err(VmmError::Timeout {
            name: name.into(),
            secs: self.cfg.ip_timeout.as_secs(),
            detail: format!(
                "VM never reported an IP address (lume serve, {} and ARP checked; last: {last_state}){}",
                self.cfg.leases_path,
                self.run_log_tail(name)
            ),
        })
    }

    fn run_log_tail(&self, name: &str) -> String {
        let log = self.cfg.root.join(name).join("run.log");
        match std::fs::read_to_string(&log) {
            Ok(s) if !s.trim().is_empty() => {
                format!("; {}: {}", log.display(), crate::exec::tail(&s, 800))
            }
            _ => String::new(),
        }
    }

    fn endpoints_from(vm: &VmDetails, ssh: Option<&SshAccess>, ports: &[u16]) -> Endpoints {
        let ip = vm.ip().unwrap_or("").to_string();
        let vnc =
            vm.vnc_url
                .as_deref()
                .and_then(client::parse_vnc_url)
                .map(|(host, port, password)| VncEndpoint {
                    host,
                    port,
                    password,
                });
        let ssh = match ssh {
            Some(a) => Some(SshEndpoint {
                host: ip.clone(),
                port: 22,
                user: a.user.clone(),
                private_key: Some(a.private_key.clone()),
                password: a.password.clone(),
            }),
            None if vm.os == "macOS" || vm.os == "macos" => Some(SshEndpoint {
                host: ip.clone(),
                port: 22,
                user: "lume".into(),
                private_key: None,
                password: Some("lume".into()),
            }),
            None => None,
        };
        Endpoints {
            host: ip,
            // Lume VMs sit on a host-reachable NAT network: guest ports are
            // reachable on the VM IP directly, so the mapping is the identity.
            ports: ports
                .iter()
                .map(|p| (*p, *p))
                .chain(std::iter::once((22, 22)))
                .collect(),
            vnc,
            ssh,
            ..Default::default()
        }
    }

    /// `<root>/<name>/ports.json`: the guest ports `start` published.
    fn ports_file(&self, name: &str) -> PathBuf {
        self.cfg.root.join(name).join("ports.json")
    }

    fn save_ports(&self, name: &str, ports: &[u16]) {
        let path = self.ports_file(name);
        let saved = path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&path, serde_json::to_vec(ports).unwrap_or_default()));
        if let Err(e) = saved {
            tracing::warn!(name, error = %e, "could not record the published ports");
        }
    }

    fn load_ports(&self, name: &str) -> Vec<u16> {
        std::fs::read(self.ports_file(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn instance(vm: &VmDetails, ssh: Option<&SshAccess>, ports: &[u16]) -> Instance {
        Instance {
            name: vm.name.clone(),
            backend: BackendKind::Lume,
            status: map_status(&vm.status),
            endpoints: Self::endpoints_from(vm, ssh, ports),
            isolation: Isolation::Vm { accel: "vz".into() },
            arch: Some(Arch::Aarch64),
        }
    }

    /// Prepare the raw disk + seed for a Linux disk-image VM.
    async fn prepare_linux(
        &self,
        spec: &StartSpec,
        src: &std::path::Path,
    ) -> Result<(PathBuf, Option<PathBuf>)> {
        let dir = self.cfg.root.join(&spec.name);
        std::fs::create_dir_all(&dir)?;
        let raw = dir.join("disk.img");
        if !raw.exists() {
            // The raw copy is sparse; the guest's first writes land on top.
            crate::disk::ensure_space(
                &dir,
                std::fs::metadata(src)
                    .map(|m| m.len())
                    .unwrap_or(0)
                    .saturating_add(crate::disk::VM_CREATE_ESTIMATE),
                &format!("create VM {}", spec.name),
            )?;
            let qemu_img = host::which("qemu-img").ok_or_else(|| {
                VmmError::missing(
                    "qemu-img",
                    "needed to convert Linux disks for Lume (`brew install qemu`)",
                )
            })?;
            let (s, d) = (src.display().to_string(), raw.display().to_string());
            host::run(&qemu_img, &["convert", "-q", "-O", "raw", &s, &d]).await?;
            if let Some(gb) = spec.disk_size_gb {
                host::run(
                    &qemu_img,
                    &["resize", "-q", "-f", "raw", &d, &format!("{gb}G")],
                )
                .await?;
            }
        }
        // SSH key / raw user-data plus the guest env (the spacesd token).
        let seed = crate::cloudinit::seed_for_spec(&spec.name, spec)?;
        let seed_path = match seed {
            Some(s) => {
                let p = dir.join("seed.iso");
                s.write_iso(&p)?;
                Some(p)
            }
            None => None,
        };
        Ok((raw, seed_path))
    }

    async fn run_linux_detached(
        &self,
        name: &str,
        disk: &std::path::Path,
        seed: Option<&std::path::Path>,
    ) -> Result<()> {
        let bin = lume_bin()
            .ok_or_else(|| VmmError::missing("lume", "binary needed for Linux disk VMs"))?;
        let log = self.cfg.root.join(name).join("run.log");
        let mut args = vec![
            "run".to_string(),
            name.to_string(),
            "--detach".into(),
            "--display".into(),
            "none".into(),
            "--disk-path".into(),
            disk.display().to_string(),
            "--log-file".into(),
            log.display().to_string(),
        ];
        if let Some(s) = seed {
            args.push("--mount".into());
            args.push(s.display().to_string());
        }
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        host::run(bin, &argv).await?;
        Ok(())
    }

    /// The macOS setup share: a read-only host directory mounted in the guest
    /// at `/Volumes/My Shared Files`, holding `env-token` (0600) when the start
    /// spec carries `CUA_ENV_TOKEN`. The golden's spacesd launcher copies
    /// it to `~/.cua/spacesd/token` before serving, so the driver starts
    /// with this token instead of in bootstrap mode. Returns the directories
    /// to share (the existing share when `spec` is `None`, e.g. a restart).
    fn macos_setup_share(&self, name: &str, spec: Option<&StartSpec>) -> Result<Vec<SharedDir>> {
        let dir = self.cfg.root.join(name).join("setup");
        if let Some(spec) = spec {
            match spec
                .env
                .get(crate::cloudinit::ENV_TOKEN_VAR)
                .filter(|t| !t.is_empty())
            {
                Some(token) => {
                    std::fs::create_dir_all(&dir)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
                    }
                    // Owner-only from creation (never briefly world-readable).
                    crate::host::write_private(
                        &dir.join(MACOS_SETUP_TOKEN_FILE),
                        token.as_bytes(),
                    )?;
                }
                // No token in this spec: never share a stale one.
                None => {
                    let _ = std::fs::remove_file(dir.join(MACOS_SETUP_TOKEN_FILE));
                }
            }
        }
        Ok(if dir.join(MACOS_SETUP_TOKEN_FILE).is_file() {
            vec![SharedDir {
                host_path: dir.display().to_string(),
                read_only: true,
            }]
        } else {
            Vec::new()
        })
    }

    fn macos_run(&self, name: &str, spec: Option<&StartSpec>) -> Result<RunVm> {
        if spec.is_some_and(|s| s.command.as_ref().is_some_and(|c| !c.is_empty())) {
            return Err(VmmError::Unsupported {
                backend: "lume",
                op: "a sandbox command on a macOS guest (bake it into the image)",
            });
        }
        Ok(RunVm {
            no_display: true,
            shared_directories: self.macos_setup_share(name, spec)?,
            ..Default::default()
        })
    }

    /// [`Runtime::ensure_base`] with registry credentials for a private
    /// image (used when they apply to the image's registry).
    pub async fn ensure_base_with(
        &self,
        image: &ImageSource,
        base_name: &str,
        creds: Option<&RegistryCredentials>,
    ) -> Result<CheckpointInfo> {
        self.ensure_serving().await?;
        let ImageSource::Oci { reference } = image else {
            return Err(VmmError::invalid(
                "lume bases are pulled from OCI references",
            ));
        };
        let pinned = reference;
        let reference = &lume_pull_ref(reference).to_string();
        if let Some(vm) = self.client.get(base_name).await? {
            if vm.status == "running" {
                return Err(VmmError::invalid(format!(
                    "base VM '{base_name}' is running; stop it before using it as a clone source"
                )));
            }
            if vm.status != "pulling" {
                OwnedVms::default().touch(base_name);
                return Ok(CheckpointInfo::now(
                    base_name,
                    BackendKind::Lume,
                    Some(reference.clone()),
                ));
            }
        }
        crate::disk::ensure_space(
            &host::home_dir().join(".lume"),
            crate::disk::LUME_PULL_ESTIMATE,
            &format!("pull {reference}"),
        )?;
        tracing::info!(
            reference,
            base_name,
            "pulling lume base image (first time only)"
        );
        crate::progress::report(
            crate::progress::Progress::phase(crate::progress::Phase::Pulling).detail(reference),
        );
        // Recorded before the pull: a pull an older Lume finishes in the
        // background after a cancelled create is still the SDK's base (the
        // cache GC can remove it); a cancelled pull's record goes with it.
        OwnedVms::default().mark(base_name, OwnedKind::Base, Some(pinned));
        let pulled = match creds.filter(|c| c.applies_to(crate::registry_of(reference))) {
            // `lume serve`'s pull API takes no credentials: the `lume pull`
            // CLI reads them from its environment (never argv).
            Some(c) => match lume_bin() {
                Some(bin) => cli_pull(&bin, reference, base_name, c, self.cfg.pull_timeout).await,
                None => Err(VmmError::missing(
                    "lume",
                    "binary needed to pull a private image",
                )),
            },
            None => {
                self.client
                    .pull(reference, base_name, self.cfg.pull_timeout)
                    .await
            }
        };
        if let Err(e) = pulled {
            if !matches!(self.client.get(base_name).await, Ok(Some(_))) {
                OwnedVms::default().forget(base_name);
            }
            return Err(e);
        }
        OwnedVms::default().mark(base_name, OwnedKind::Base, Some(pinned));
        Ok(CheckpointInfo::now(
            base_name,
            BackendKind::Lume,
            Some(reference.clone()),
        ))
    }

    /// Installs the start spec's workload environment in a booted macOS
    /// guest ([`apply_macos_env`]), or clears one an earlier start installed.
    /// Nothing happens (no SSH round trip) when neither applies. `exec` is
    /// `None` when the `lume` binary is missing.
    async fn sync_macos_env(
        &self,
        name: &str,
        env: &std::collections::BTreeMap<String, String>,
        exec: Option<&dyn GuestExec>,
    ) -> Result<()> {
        let env = crate::cloudinit::workload_env(env);
        let marker = self.cfg.root.join(name).join(MACOS_ENV_MARKER);
        if env.is_empty() && !marker.exists() {
            return Ok(());
        }
        let exec = exec.ok_or_else(|| {
            VmmError::missing(
                "lume",
                "needed to set a macOS guest's environment (`lume ssh`)",
            )
        })?;
        apply_macos_env(exec, name, &env, MACOS_ENV_BUDGET, MACOS_ENV_RETRY).await?;
        if env.is_empty() {
            let _ = std::fs::remove_file(&marker);
        } else {
            std::fs::create_dir_all(self.cfg.root.join(name))?;
            std::fs::write(&marker, b"")?;
        }
        Ok(())
    }

    /// `lume set` the stopped clone `spec.name` to the spec's CPU, memory and
    /// (grow-only) disk size; a no-op when it already matches.
    async fn apply_clone_resources(&self, spec: &StartSpec) -> Result<()> {
        let vm = self
            .client
            .get(&spec.name)
            .await?
            .ok_or_else(|| VmmError::NotFound(spec.name.clone()))?;
        let set = clone_resources(&vm, spec);
        if set.is_empty() {
            return Ok(());
        }
        tracing::info!(vm = %spec.name, ?set, "applying requested resources to the lume clone");
        self.client.set(&spec.name, &set).await
    }

    fn linux_state(&self, name: &str) -> Option<(PathBuf, Option<PathBuf>)> {
        let dir = self.cfg.root.join(name);
        let disk = dir.join("disk.img");
        disk.exists().then(|| {
            let seed = dir.join("seed.iso");
            (disk, seed.exists().then_some(seed))
        })
    }
}

/// The `lume set` a fresh clone `vm` needs to match `spec`: CPU and memory
/// when they differ, and the disk only when the spec asks for more than the
/// clone has (lume grows disks but never shrinks them). The clone is new, so
/// lume's pre-resize backup is skipped.
pub fn clone_resources(vm: &VmDetails, spec: &StartSpec) -> client::SetVm {
    let mut set = client::SetVm::default();
    if spec.cpus > 0 && spec.cpus != vm.cpu_count {
        set.cpu = Some(spec.cpus);
    }
    if spec.memory_mb > 0 && spec.memory_mb.saturating_mul(1 << 20) != vm.memory_size {
        set.memory = Some(format!("{}MB", spec.memory_mb));
    }
    if let Some(gb) = spec.disk_size_gb.filter(|g| *g > 0) {
        let want = u64::from(gb) << 30;
        match client::disk_total_bytes(&vm.disk_size) {
            Some(have) if want <= have => {
                if want < have {
                    tracing::warn!(
                        vm = %vm.name,
                        requested_gb = gb,
                        current_gb = have >> 30,
                        "lume cannot shrink a cloned disk; keeping the base image's size"
                    );
                }
            }
            _ => {
                set.disk_size = Some(format!("{gb}GB"));
                set.no_backup = Some(true);
            }
        }
    }
    set
}

/// `lume pull` arguments for `reference` into VM `name` (same split as
/// [`client::pull_body`]).
pub fn cli_pull_args(reference: &str, name: &str) -> Vec<String> {
    let b = client::pull_body(reference, name);
    let mut args = vec![
        "pull".to_string(),
        b["image"].as_str().unwrap_or(reference).to_string(),
        name.to_string(),
    ];
    for key in ["registry", "organization"] {
        if let Some(v) = b[key].as_str() {
            args.push(format!("--{key}"));
            args.push(v.to_string());
        }
    }
    args
}

/// Pulls a private image with the `lume` CLI, which reads registry
/// credentials from `GITHUB_USERNAME` / `GITHUB_TOKEN` in its environment.
/// The credentials never go on argv or into a log line or error.
pub async fn cli_pull(
    bin: &std::path::Path,
    reference: &str,
    name: &str,
    creds: &RegistryCredentials,
    timeout: Duration,
) -> Result<()> {
    let args = cli_pull_args(reference, name);
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(&args)
        .env("GITHUB_USERNAME", &creds.username)
        .env("GITHUB_TOKEN", &creds.password)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    tracing::info!(
        reference,
        name,
        "pulling private lume image with credentials"
    );
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| VmmError::Timeout {
            name: name.to_string(),
            secs: timeout.as_secs(),
            detail: format!("lume pull of {reference}"),
        })??;
    if out.status.success() {
        return Ok(());
    }
    let mut stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if stderr.trim().is_empty() {
        stderr = String::from_utf8_lossy(&out.stdout).into_owned();
    }
    // Belt and braces: never echo the secret back.
    let stderr = stderr.replace(&creds.password, "***");
    Err(VmmError::Command {
        cmd: format!("lume pull {reference}"),
        code: out.status.code(),
        stderr: crate::exec::tail(&stderr, 2000),
    })
}

/// Host-side marker: this VM got a workload environment from cua-vmm (so a
/// later start without one clears it).
const MACOS_ENV_MARKER: &str = "env-applied";
/// How long to keep retrying the environment write while the guest's sshd
/// comes up after boot.
const MACOS_ENV_BUDGET: Duration = Duration::from_secs(180);
const MACOS_ENV_RETRY: Duration = Duration::from_secs(3);

/// The script that installs a workload environment in a macOS guest, run as
/// the login user through `lume ssh` (no agent in the image needed):
///
/// - `~/.cua/env.sh` (`0600`, `export K='v'` lines), sourced from
///   `~/.zshenv` and `~/.profile`, so every shell (Terminal, `ssh <cmd>`,
///   `zsh -c`) sees it;
/// - `launchctl setenv` for each variable (apps launched afterwards inherit
///   it), re-applied at every login by the `ai.cua.env` LaunchAgent;
/// - the previous variables are unset first, so an empty `env` clears them.
///
/// The contents travel base64-encoded inside the script (`lume ssh` does not
/// forward stdin); values are plain environment, not secrets.
pub fn macos_env_script(env: &std::collections::BTreeMap<String, String>) -> Result<String> {
    use base64::Engine as _;
    crate::cloudinit::validate_guest_env(env)?;
    let q = crate::exec::shell_quote;
    let mut env_sh = String::from("# Written by cua-vmm: the sandbox environment.\n");
    let mut setenv = String::new();
    let mut unset = String::new();
    for (k, v) in env {
        env_sh.push_str(&format!("export {k}={}\n", q(v)));
        setenv.push_str(&format!("launchctl setenv {k} {}\n", q(v)));
        unset.push_str(&format!("launchctl unsetenv {k}\n"));
    }
    let plist = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>ai.cua.env</string>\n  \
<key>ProgramArguments</key>\n  <array>\n    <string>/bin/sh</string>\n    <string>-c</string>\n    \
<string>[ -r \"$HOME/.cua/env-launchctl.sh\" ] &amp;&amp; /bin/sh \"$HOME/.cua/env-launchctl.sh\"</string>\n  \
</array>\n  <key>RunAtLoad</key><true/>\n</dict>\n</plist>\n";
    let b = |t: &str| base64::engine::general_purpose::STANDARD.encode(t);
    let line = "[ -r \"$HOME/.cua/env.sh\" ] && . \"$HOME/.cua/env.sh\" # cua-vmm env";
    Ok(format!(
        "set -e; d=\"$HOME/.cua\"; mkdir -p \"$d\" \"$HOME/Library/LaunchAgents\"; umask 077; \
         if [ -f \"$d/env-unset.sh\" ]; then /bin/sh \"$d/env-unset.sh\" || true; fi; \
         printf %s '{env}' | base64 --decode > \"$d/env.sh\"; \
         printf %s '{setenv}' | base64 --decode > \"$d/env-launchctl.sh\"; \
         printf %s '{unset}' | base64 --decode > \"$d/env-unset.sh\"; \
         /bin/sh \"$d/env-launchctl.sh\"; \
         for f in \"$HOME/.zshenv\" \"$HOME/.profile\"; do \
         grep -qs 'cua-vmm env' \"$f\" || printf '\\n%s\\n' {line} >> \"$f\"; done; \
         printf %s '{plist}' | base64 --decode > \"$HOME/Library/LaunchAgents/ai.cua.env.plist\"; \
         echo cua-env-ok",
        env = b(&env_sh),
        setenv = b(&setenv),
        unset = b(&unset),
        line = q(line),
        plist = b(plist),
    ))
}

/// Runs [`macos_env_script`] through `exec`, retrying (bounded by `budget`)
/// while the guest's sshd is not up yet. Fails loudly: an environment the
/// caller asked for is never dropped.
pub async fn apply_macos_env(
    exec: &dyn GuestExec,
    name: &str,
    env: &std::collections::BTreeMap<String, String>,
    budget: Duration,
    retry: Duration,
) -> Result<()> {
    let script = macos_env_script(env)?;
    let deadline = tokio::time::Instant::now() + budget;
    let max_attempts = (budget.as_millis() / retry.as_millis().max(1)).max(1) as usize + 1;
    let mut last = String::new();
    for _ in 0..max_attempts {
        let req = crate::exec::ExecRequest::sh(script.clone()).timeout(Duration::from_secs(60));
        match exec.exec(req).await {
            Ok(out) if out.success() && out.stdout_str().contains("cua-env-ok") => return Ok(()),
            Ok(out) => last = crate::exec::tail(&out.stderr_str(), 500),
            Err(e) => last = e.to_string(),
        }
        if tokio::time::Instant::now() + retry > deadline {
            break;
        }
        tokio::time::sleep(retry).await;
    }
    Err(VmmError::Timeout {
        name: name.to_string(),
        secs: budget.as_secs(),
        detail: format!("could not set the sandbox environment over `lume ssh`: {last}"),
    })
}

fn map_status(s: &str) -> Status {
    match s {
        "running" => Status::Running,
        "stopped" | "stop" => Status::Stopped,
        "pulling" | "provisioning" | "starting" => Status::Provisioning,
        "suspended" | "paused" => Status::Paused,
        other => Status::Unknown(other.into()),
    }
}

/// Run the official Lume installer (explicit opt-in only).
pub async fn install_lume() -> Result<()> {
    let curl = host::which("curl")
        .ok_or_else(|| VmmError::missing("curl", "needed to download the Lume installer"))?;
    let script = host::run(curl, &["-fsSL", INSTALL_URL]).await?;
    tracing::info!("running the Lume installer");
    let out = crate::exec::run_with_stdin(
        "/bin/bash",
        &["-s".into()],
        Some(script.as_bytes()),
        Some(Duration::from_secs(600)),
    )
    .await?;
    out.check("lume installer")?;
    // The installer's LaunchAgent logs to /tmp; record that cua installed
    // it, so `cua cache prune` caps those logs (a user's own install is
    // left alone).
    let marker = lume_installed_marker();
    if let Some(d) = marker.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&marker, host::now_secs().to_string());
    Ok(())
}

/// Marker file: this cua home installed Lume ([`install_lume`]).
pub fn lume_installed_marker() -> PathBuf {
    host::cua_home()
        .join("vmm")
        .join("lume")
        .join("installed-by-cua")
}

/// Logs the Lume installer's LaunchAgent writes (`lume serve` as a daemon).
pub const LUME_DAEMON_LOGS: [&str; 2] = ["/tmp/lume_daemon.log", "/tmp/lume_daemon.error.log"];

#[async_trait]
impl Runtime for LumeRuntime {
    fn kind(&self) -> BackendKind {
        BackendKind::Lume
    }

    async fn ensure_base(&self, image: &ImageSource, base_name: &str) -> Result<CheckpointInfo> {
        self.ensure_base_with(image, base_name, None).await
    }

    async fn start(&self, spec: &StartSpec) -> Result<Instance> {
        validate_name(&spec.name)?;
        crate::types::reject_sidecars(BackendKind::Lume, spec)?;
        crate::types::reject_gpu(
            BackendKind::Lume,
            spec,
            if spec.os == GuestOs::Macos {
                &[crate::gpu::PARAVIRTUAL]
            } else {
                &[]
            },
        )?;
        // Lume answers, so only the host (Apple silicon) can say no.
        if spec.gpu.is_some()
            && let Err(why) = crate::gpu::lume_support(true).pick(crate::gpu::PARAVIRTUAL)
        {
            return Err(VmmError::invalid(why));
        }
        crate::types::reject_restricted_network(BackendKind::Lume, spec)?;
        self.ensure_serving().await?;
        // Linux disk VMs always get SSH access: the caller's or a managed
        // per-instance key (seeded through cloud-init).
        let owned;
        let spec = if spec.os == GuestOs::Linux
            && spec.ssh.is_none()
            && spec.cloud_init_user_data.is_none()
            && (matches!(spec.image, ImageSource::Disk { .. })
                || self.linux_state(&spec.name).is_some())
        {
            let dir = self.cfg.root.join(&spec.name);
            std::fs::create_dir_all(&dir)?;
            let mut s = spec.clone();
            s.ssh = crate::cloudinit::managed_ssh(&dir).await;
            owned = s;
            &owned
        } else {
            spec
        };
        let existing = self.client.get(&spec.name).await?;
        let mut macos_booted = false;
        match (&existing, &spec.image) {
            (Some(vm), _) if vm.status == "running" => {}
            (Some(_), _) => {
                // Stopped VM (fork/clone or earlier run): boot it again.
                match self.linux_state(&spec.name) {
                    Some((disk, mut seed)) => {
                        // A new start spec may carry a new token: reseed
                        // (a Linux guest always gets a seed).
                        if let Some(s) = crate::cloudinit::seed_for_spec(&spec.name, spec)? {
                            let p = self.cfg.root.join(&spec.name).join("seed.iso");
                            s.write_iso(&p)?;
                            seed = Some(p);
                        }
                        self.run_linux_detached(&spec.name, &disk, seed.as_deref())
                            .await?
                    }
                    None => {
                        let run = self.macos_run(&spec.name, Some(spec))?;
                        self.before_macos_boot(&spec.name)?;
                        self.client.run(&spec.name, &run).await?;
                        macos_booted = true;
                    }
                }
            }
            (None, ImageSource::Existing) => return Err(VmmError::NotFound(spec.name.clone())),
            (None, ImageSource::Oci { reference }) => {
                let base = base_vm_name(reference);
                self.ensure_base_with(&spec.image, &base, spec.registry_auth.as_ref())
                    .await?;
                crate::disk::ensure_space(
                    &host::home_dir().join(".lume"),
                    crate::disk::VM_CREATE_ESTIMATE,
                    &format!("create VM {}", spec.name),
                )?;
                // APFS clonefile: copy-on-write, so the clone costs only what
                // the guest writes.
                crate::progress::report(crate::progress::Progress::phase(
                    crate::progress::Phase::Creating,
                ));
                // From here the VM exists in Lume (a clone Lume finishes even
                // when the request is dropped): a create cut off before it
                // booted deletes it again.
                let mut made = CloneGuard::new(self.client.clone(), &spec.name, &self.cfg.root);
                // An error is Lume's answer (no clone made); only a create
                // cut off mid-request leaves the guard to clean up.
                self.client
                    .clone_vm(&base, &spec.name)
                    .await
                    .inspect_err(|_| made.done())?;
                OwnedVms::default().mark(&spec.name, OwnedKind::Instance, Some(&base));
                OwnedVms::default().touch(&base);
                // A clone inherits the base's CPU, memory and disk: apply the
                // requested ones before the first boot.
                if let Err(e) = self.apply_clone_resources(spec).await {
                    let _ = self.client.delete(&spec.name).await;
                    OwnedVms::default().forget(&spec.name);
                    made.done();
                    return Err(e);
                }
                if spec.gpu.is_some() {
                    self.gpu().enable(&spec.name).inspect_err(|_| made.done())?;
                }
                let run = self.macos_run(&spec.name, Some(spec))?;
                crate::progress::report(crate::progress::Progress::phase(
                    crate::progress::Phase::Booting,
                ));
                self.before_macos_boot(&spec.name)
                    .inspect_err(|_| made.done())?;
                self.client
                    .run(&spec.name, &run)
                    .await
                    .inspect_err(|_| made.done())?;
                made.done();
                macos_booted = true;
            }
            (None, ImageSource::Disk { path }) => {
                if spec.os != GuestOs::Linux {
                    return Err(VmmError::invalid(
                        "lume boots disk images for Linux guests only",
                    ));
                }
                let (disk, seed) = self.prepare_linux(spec, path).await?;
                let size_gb = std::fs::metadata(&disk)?.len().div_ceil(1 << 30).max(1);
                self.client
                    .create(&CreateVm {
                        name: spec.name.clone(),
                        os: "linux".into(),
                        cpu: spec.cpus,
                        memory: format!("{}MB", spec.memory_mb),
                        disk_size: format!("{size_gb}GB"),
                        display: "1024x768".into(),
                        ipsw: None,
                        storage: None,
                    })
                    .await?;
                OwnedVms::default().mark(&spec.name, OwnedKind::Instance, None);
                // Creation is asynchronous; wait until the VM shell exists.
                let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
                loop {
                    match self.client.get(&spec.name).await? {
                        Some(vm) if vm.status != "provisioning" => break,
                        _ if tokio::time::Instant::now() > deadline => {
                            return Err(VmmError::Timeout {
                                name: spec.name.clone(),
                                secs: 120,
                                detail: "lume create did not finish".into(),
                            });
                        }
                        _ => tokio::time::sleep(Duration::from_millis(500)).await,
                    }
                }
                self.run_linux_detached(&spec.name, &disk, seed.as_deref())
                    .await?;
            }
        }
        crate::progress::report(crate::progress::Progress::phase(
            crate::progress::Phase::Booting,
        ));
        let vm = self.wait_for_ip(&spec.name).await?;
        if macos_booted {
            let bin = lume_bin();
            let exec = bin.map(|b| PrefixExec {
                program: b.display().to_string(),
                prefix: vec!["ssh".into(), spec.name.clone()],
            });
            self.sync_macos_env(
                &spec.name,
                &spec.env,
                exec.as_ref().map(|e| e as &dyn GuestExec),
            )
            .await?;
        }
        // Remember the published guest ports: a later process reattaching
        // by name (`endpoints`) must still see e.g. cua-spacesd's 3211.
        self.save_ports(&spec.name, &spec.ports);
        let inst = Self::instance(&vm, spec.ssh.as_ref(), &spec.ports);
        if !spec.probes.is_empty() {
            crate::progress::report(crate::progress::Progress::phase(
                crate::progress::Phase::WaitingForServices,
            ));
        }
        crate::probe::wait_all(
            &spec.name,
            &inst.endpoints,
            &spec.probes,
            spec.ready_timeout,
        )
        .await?;
        Ok(inst)
    }

    async fn stop(&self, name: &str) -> Result<()> {
        self.ensure_serving().await?;
        match self.client.get(name).await? {
            None => Err(VmmError::NotFound(name.into())),
            Some(vm) if vm.status == "stopped" => Ok(()),
            Some(_) => self.client.stop(name).await,
        }
    }

    /// Lume has no in-memory pause: suspending stops the VM (its disk and
    /// setup share stay) and [`Runtime::resume`] boots it again, so `cua sb
    /// suspend` / `resume` work for macOS Spaces as they do for the others.
    async fn suspend(&self, name: &str) -> Result<()> {
        self.stop(name).await
    }

    /// Boots a stopped VM again (Lume has no in-memory pause, so a stopped
    /// VM is what a `lume stop` or a host reboot leaves): the way `start`
    /// boots an existing VM, with its setup share (the env token the last
    /// start wrote) for a macOS guest and its disk and seed for a Linux disk
    /// VM. Returns once the guest has an IP. A running VM is returned as is.
    async fn resume(&self, name: &str) -> Result<Instance> {
        self.ensure_serving().await?;
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        if map_status(&vm.status) == Status::Stopped {
            match self.linux_state(name) {
                Some((disk, seed)) => {
                    self.run_linux_detached(name, &disk, seed.as_deref())
                        .await?
                }
                None => {
                    let run = self.macos_run(name, None)?;
                    self.before_macos_boot(name)?;
                    self.client.run(name, &run).await?
                }
            }
        }
        let vm = self.wait_for_ip(name).await?;
        Ok(Self::instance(&vm, None, &self.load_ports(name)))
    }

    async fn fork(&self, source: &str, new_name: &str) -> Result<()> {
        validate_name(new_name)?;
        let vm = self
            .client
            .get(source)
            .await?
            .ok_or_else(|| VmmError::NotFound(source.into()))?;
        if vm.status == "running" {
            return Err(VmmError::invalid(format!(
                "'{source}' is running; stop it before cloning"
            )));
        }
        if self.client.get(new_name).await?.is_some() {
            return Err(VmmError::AlreadyExists(new_name.into()));
        }
        self.client.clone_vm(source, new_name).await?;
        OwnedVms::default().mark(new_name, OwnedKind::Checkpoint, Some(source));
        // Linux disk VMs keep their disk outside lume: copy it (APFS clonefile
        // via `cp -c` keeps this instant and copy-on-write).
        if let Some((disk, seed)) = self.linux_state(source) {
            let dir = self.cfg.root.join(new_name);
            std::fs::create_dir_all(&dir)?;
            let (s, d) = (
                disk.display().to_string(),
                dir.join("disk.img").display().to_string(),
            );
            if host::run("cp", &["-c", &s, &d]).await.is_err() {
                std::fs::copy(&disk, dir.join("disk.img"))?;
            }
            if let Some(seed) = seed {
                std::fs::copy(seed, dir.join("seed.iso"))?;
            }
        }
        Ok(())
    }

    /// Stop → clone into `checkpoint` → restart (clean, stopped-state clone,
    /// the same contract as cloud snapshots).
    async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        let was_running = vm.status == "running";
        if was_running {
            self.client.stop(name).await?;
        }
        let res = self.fork(name, checkpoint).await;
        if was_running {
            match self.linux_state(name) {
                Some((disk, seed)) => {
                    self.run_linux_detached(name, &disk, seed.as_deref())
                        .await?
                }
                None => {
                    let run = self.macos_run(name, None)?;
                    self.before_macos_boot(name)?;
                    self.client.run(name, &run).await?
                }
            }
        }
        res?;
        Ok(CheckpointInfo::now(
            checkpoint,
            BackendKind::Lume,
            Some(name.into()),
        ))
    }

    async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
        self.delete(checkpoint).await
    }

    async fn list(&self) -> Result<Vec<InstanceSummary>> {
        self.ensure_serving().await?;
        Ok(self
            .client
            .list()
            .await?
            .into_iter()
            .map(|vm| InstanceSummary {
                status: map_status(&vm.status),
                name: vm.name,
                backend: BackendKind::Lume,
            })
            .collect())
    }

    async fn status(&self, name: &str) -> Result<Status> {
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        Ok(map_status(&vm.status))
    }

    async fn delete(&self, name: &str) -> Result<()> {
        // `lume serve` may have exited (a crash, an update, a reboot without
        // its login item): start it rather than leave a Space that cannot
        // be deleted.
        self.ensure_serving().await?;
        let dir = self.cfg.root.join(name);
        let Some(vm) = self.client.get(name).await? else {
            // A create cut off before Lume had the VM may have left cua's
            // own state for it.
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                self.forget_gpu(name);
            }
            return Err(VmmError::NotFound(name.into()));
        };
        if vm.status == "running" {
            let _ = self.client.stop_for_delete(name).await;
        }
        self.client.delete(name).await?;
        OwnedVms::default().forget(name);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        self.forget_gpu(name);
        Ok(())
    }

    async fn endpoints(&self, name: &str) -> Result<Endpoints> {
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        Ok(Self::endpoints_from(&vm, None, &self.load_ports(name)))
    }

    /// [`LumeSshExec`] (`lume ssh <vm>`) for macOS guests (password auth
    /// handled by lume; stdout and stderr kept apart, no stdin);
    /// Linux disk VMs use the SSH endpoint from their spec, so callers should
    /// build an [`crate::SshExec`] from `Instance::endpoints.ssh`.
    async fn guest_exec(&self, name: &str) -> Result<Option<Arc<dyn GuestExec>>> {
        let vm = self
            .client
            .get(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        if vm.os.eq_ignore_ascii_case("macos") {
            let bin =
                lume_bin().ok_or_else(|| VmmError::missing("lume", "needed for `lume ssh`"))?;
            return Ok(Some(Arc::new(LumeSshExec::new(
                bin.display().to_string(),
                name,
            ))));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_names_match_the_python_runtime() {
        // hashlib.sha256(b"ghcr.io/trycua/macos-sequoia-cua:latest").hexdigest()[:12]
        assert_eq!(
            lume_pull_ref(
                "ghcr.io/trycua/macos:26@sha256:b03ff1154dc80a7dadef5397eaca395c1e6cb64093a29bd53ca5916127816a8e"
            ),
            "ghcr.io/trycua/macos:26"
        );
        assert_eq!(
            lume_pull_ref("ghcr.io/trycua/macos:26"),
            "ghcr.io/trycua/macos:26"
        );
        // A bare digest ref is not a tag: left alone.
        assert_eq!(
            lume_pull_ref("ghcr.io/trycua/macos@sha256:ab"),
            "ghcr.io/trycua/macos@sha256:ab"
        );
        // A moved tag names a different base.
        assert_ne!(
            base_vm_name("ghcr.io/trycua/macos:26@sha256:aa"),
            base_vm_name("ghcr.io/trycua/macos:26@sha256:bb")
        );
        let n = base_vm_name("ghcr.io/trycua/macos-sequoia-cua:latest");
        assert_eq!(n, "cua-base-3de9994dac39");
        assert_ne!(n, base_vm_name("ghcr.io/trycua/macos-tahoe-cua:latest"));
    }

    #[test]
    fn statuses_map() {
        assert_eq!(map_status("running"), Status::Running);
        assert_eq!(map_status("stopped"), Status::Stopped);
        assert_eq!(map_status("pulling"), Status::Provisioning);
    }

    fn base_vm() -> VmDetails {
        VmDetails {
            name: "clone".into(),
            os: "macOS".into(),
            cpu_count: 4,
            memory_size: 8 << 30,
            disk_size: serde_json::json!({"allocated": 1u64 << 30, "total": 150u64 << 30}),
            ..Default::default()
        }
    }

    #[test]
    fn clone_resources_follow_the_spec() {
        let img = ImageSource::Oci {
            reference: "ghcr.io/trycua/macos:26".into(),
        };
        let spec = StartSpec::new("clone", img.clone()).cpus(2).memory_mb(4096);
        let set = clone_resources(&base_vm(), &spec);
        assert_eq!(
            set,
            SetVm {
                cpu: Some(2),
                memory: Some("4096MB".into()),
                ..Default::default()
            }
        );

        // Already matching: nothing to set.
        let same = StartSpec::new("clone", img.clone()).cpus(4).memory_mb(8192);
        assert!(clone_resources(&base_vm(), &same).is_empty());

        // Disk grows only; a smaller request keeps the base size.
        let mut grow = same.clone();
        grow.disk_size_gb = Some(200);
        let set = clone_resources(&base_vm(), &grow);
        assert_eq!(set.disk_size.as_deref(), Some("200GB"));
        assert_eq!(set.no_backup, Some(true));
        let mut shrink = same;
        shrink.disk_size_gb = Some(64);
        assert!(clone_resources(&base_vm(), &shrink).is_empty());
    }

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A `GuestExec` that fails `fail_first` times, then succeeds, recording
    /// every script.
    struct FakeExec {
        fail_first: Mutex<u32>,
        scripts: Mutex<Vec<String>>,
    }

    impl FakeExec {
        fn new(fail_first: u32) -> Self {
            Self {
                fail_first: Mutex::new(fail_first),
                scripts: Mutex::default(),
            }
        }
    }

    #[async_trait]
    impl GuestExec for FakeExec {
        async fn exec(&self, req: crate::exec::ExecRequest) -> Result<crate::exec::ExecOutput> {
            self.scripts.lock().unwrap().push(req.script);
            let mut f = self.fail_first.lock().unwrap();
            if *f > 0 {
                *f -= 1;
                return Ok(crate::exec::ExecOutput {
                    exit_code: 255,
                    stderr: b"Connection refused".to_vec(),
                    ..Default::default()
                });
            }
            Ok(crate::exec::ExecOutput {
                exit_code: 0,
                stdout: b"cua-env-ok\n".to_vec(),
                ..Default::default()
            })
        }
    }

    fn decoded_parts(script: &str) -> Vec<String> {
        use base64::Engine as _;
        script
            .split("printf %s '")
            .skip(1)
            .map(|p| {
                let b = &p[..p.find('\'').unwrap()];
                String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b).unwrap())
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn macos_env_script_writes_shell_launchctl_and_agent_files() {
        let env: BTreeMap<String, String> = [
            ("FOO".to_string(), "bar baz".to_string()),
            ("Q".to_string(), "it's".to_string()),
        ]
        .into();
        let script = macos_env_script(&env).unwrap();
        // Values never appear in the clear (base64 inside the script).
        assert!(!script.contains("bar baz"));
        let parts = decoded_parts(&script);
        assert_eq!(parts.len(), 4, "env.sh, launchctl, unset, plist");
        assert!(parts[0].contains("export FOO='bar baz'\n"));
        assert!(parts[0].contains("export Q='it'\\''s'\n"));
        assert!(parts[1].contains("launchctl setenv FOO 'bar baz'\n"));
        assert_eq!(parts[2], "launchctl unsetenv FOO\nlaunchctl unsetenv Q\n");
        assert!(parts[3].contains("<string>ai.cua.env</string>"));
        assert!(parts[3].contains("<key>RunAtLoad</key><true/>"));
        assert!(script.contains(".zshenv") && script.contains(".profile"));
        assert!(script.ends_with("echo cua-env-ok"));
        // Empty env: a clearing script (unsets the previous variables).
        let clear = decoded_parts(&macos_env_script(&BTreeMap::new()).unwrap());
        assert!(!clear[0].contains("export"));
        assert!(clear[1].is_empty());
        // Bad names and control characters are refused.
        let bad: BTreeMap<String, String> = [("1X".to_string(), "v".to_string())].into();
        assert!(macos_env_script(&bad).is_err());
        let ctl: BTreeMap<String, String> = [("X".to_string(), "a\nb".to_string())].into();
        assert!(macos_env_script(&ctl).is_err());
    }

    #[tokio::test]
    async fn macos_env_retries_until_ssh_answers_and_fails_loudly() {
        let env: BTreeMap<String, String> = [("FOO".to_string(), "1".to_string())].into();
        let exec = FakeExec::new(2);
        apply_macos_env(
            &exec,
            "m",
            &env,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .await
        .unwrap();
        assert_eq!(exec.scripts.lock().unwrap().len(), 3);
        let never = FakeExec::new(u32::MAX);
        let err = apply_macos_env(
            &never,
            "m",
            &env,
            Duration::from_millis(50),
            Duration::from_millis(10),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, VmmError::Timeout { .. }), "{err}");
        assert!(err.to_string().contains("Connection refused"));
        assert!(never.scripts.lock().unwrap().len() <= 7, "bounded retries");
    }

    /// A fake `lume serve`: answers GET /lume/vms/<vm> with `status` and
    /// records every request line.
    async fn fake_lume(status: &'static str) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let line = req.lines().next().unwrap_or("").to_string();
                    log.lock().unwrap().push(line.clone());
                    let body = if line.starts_with("GET ") {
                        format!(r#"{{"name":"m","os":"macOS","status":"{status}"}}"#)
                    } else {
                        "{}".to_string()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (url, seen)
    }

    /// `cua sb suspend` on a macOS Space: Lume cannot pause in memory, so a
    /// suspend stops the VM (resume boots it again) instead of failing with
    /// "lume does not support suspend".
    #[tokio::test]
    async fn suspend_stops_a_lume_vm_and_is_a_no_op_when_stopped() {
        let dir = tempfile::tempdir().unwrap();
        for (status, stops) in [("running", 1), ("stopped", 0)] {
            let (url, seen) = fake_lume(status).await;
            let rt = LumeRuntime::new(LumeConfig {
                url,
                spawn_serve: false,
                root: dir.path().to_path_buf(),
                ..LumeConfig::default()
            });
            Runtime::suspend(&rt, "m").await.unwrap();
            let posts = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("POST /lume/vms/m/stop"))
                .count();
            assert_eq!(posts, stops, "{status}: {:?}", seen.lock().unwrap());
            // It made sure `lume serve` answers first (and would start it).
            assert_eq!(
                seen.lock().unwrap().first().map(String::as_str),
                Some("GET /lume/host/status HTTP/1.1")
            );
        }
    }

    /// Deleting a macOS Space when `lume serve` is gone starts it (here,
    /// with starting it off, says how) instead of failing on a raw
    /// connection error and leaving the Space behind.
    #[tokio::test]
    async fn delete_makes_sure_lume_serve_answers_first() {
        let dir = tempfile::tempdir().unwrap();
        let (url, seen) = fake_lume("stopped").await;
        let rt = LumeRuntime::new(LumeConfig {
            url,
            spawn_serve: false,
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        Runtime::delete(&rt, "m").await.unwrap();
        let lines = seen.lock().unwrap().clone();
        assert_eq!(
            lines.first().map(String::as_str),
            Some("GET /lume/host/status HTTP/1.1")
        );
        assert!(
            lines.iter().any(|l| l.starts_with("DELETE /lume/vms/m")),
            "{lines:?}"
        );

        // (Elsewhere Lume is refused before: it runs on macOS only.)
        if !cfg!(target_os = "macos") {
            return;
        }
        let gone = LumeRuntime::new(LumeConfig {
            url: "http://127.0.0.1:9".into(),
            spawn_serve: false,
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        let e = Runtime::delete(&gone, "m").await.unwrap_err();
        assert!(e.to_string().contains("start it with `lume serve`"), "{e}");
    }

    #[test]
    fn published_ports_survive_a_new_process() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = || LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        };
        assert!(LumeRuntime::new(cfg()).load_ports("m").is_empty());
        LumeRuntime::new(cfg()).save_ports("m", &[3211, 8080]);
        // A fresh runtime (another CLI process) reads them back.
        assert_eq!(LumeRuntime::new(cfg()).load_ports("m"), vec![3211, 8080]);
    }

    #[tokio::test]
    async fn macos_env_sync_skips_clean_guests_and_clears_after_env() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        let exec = FakeExec::new(0);
        // Token-only env: nothing to do, no SSH round trip.
        let token: BTreeMap<String, String> =
            [(crate::cloudinit::ENV_TOKEN_VAR.to_string(), "t".to_string())].into();
        rt.sync_macos_env("m", &token, Some(&exec)).await.unwrap();
        assert!(exec.scripts.lock().unwrap().is_empty());
        // Workload env: applied (the token stays out of it), marker written.
        let mut env = token.clone();
        env.insert("FOO".into(), "1".into());
        rt.sync_macos_env("m", &env, Some(&exec)).await.unwrap();
        {
            let scripts = exec.scripts.lock().unwrap();
            assert_eq!(scripts.len(), 1);
            let parts = decoded_parts(&scripts[0]);
            assert!(parts[0].contains("export FOO='1'"));
            assert!(!parts[0].contains("CUA_ENV_TOKEN"));
        }
        assert!(dir.path().join("m").join(MACOS_ENV_MARKER).exists());
        // A later start without env clears it once.
        rt.sync_macos_env("m", &token, Some(&exec)).await.unwrap();
        assert_eq!(exec.scripts.lock().unwrap().len(), 2);
        assert!(!dir.path().join("m").join(MACOS_ENV_MARKER).exists());
        rt.sync_macos_env("m", &token, Some(&exec)).await.unwrap();
        assert_eq!(exec.scripts.lock().unwrap().len(), 2);
        // No lume binary: a clear error, never a silent drop.
        let err = rt.sync_macos_env("m", &env, None).await.unwrap_err();
        assert!(matches!(err, VmmError::Missing { .. }), "{err}");
    }

    #[test]
    fn cli_pull_args_split_like_the_api() {
        assert_eq!(
            cli_pull_args("ghcr.io/me/macos-private:26", "cua-base-x"),
            vec![
                "pull",
                "macos-private:26",
                "cua-base-x",
                "--registry",
                "ghcr.io",
                "--organization",
                "me"
            ]
        );
        assert_eq!(
            cli_pull_args("me/img:1", "b"),
            vec!["pull", "img:1", "b", "--organization", "me"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_pull_passes_credentials_in_the_environment_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let bin = dir.path().join("lume");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" \"$GITHUB_USERNAME\" \"$GITHUB_TOKEN\" > {}\n\
                 [ \"$2\" = fail:1 ] && {{ echo \"denied for $GITHUB_TOKEN\" >&2; exit 3; }}\nexit 0\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let creds = RegistryCredentials::new("me", "s3cret");
        cli_pull(
            &bin,
            "ghcr.io/me/img:1",
            "base",
            &creds,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        let seen = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = seen.lines().collect();
        assert_eq!(
            lines,
            vec![
                "pull img:1 base --registry ghcr.io --organization me",
                "me",
                "s3cret"
            ]
        );
        let err = cli_pull(
            &bin,
            "ghcr.io/me/fail:1",
            "base",
            &creds,
            Duration::from_secs(20),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("denied for ***"), "{err}");
        assert!(!err.contains("s3cret"));
    }

    #[test]
    fn macos_setup_share_carries_the_env_token_only_when_given() {
        let dir = tempfile::tempdir().unwrap();
        let rt = LumeRuntime::new(LumeConfig {
            root: dir.path().to_path_buf(),
            ..LumeConfig::default()
        });
        let mut spec = StartSpec::new("cua-e2e-mac", ImageSource::Existing);
        spec.os = GuestOs::Macos;
        // No token: nothing shared.
        assert!(
            rt.macos_run("cua-e2e-mac", Some(&spec))
                .unwrap()
                .shared_directories
                .is_empty()
        );

        spec.env
            .insert(crate::cloudinit::ENV_TOKEN_VAR.into(), "tok-1".into());
        let run = rt.macos_run("cua-e2e-mac", Some(&spec)).unwrap();
        assert_eq!(run.shared_directories.len(), 1);
        assert!(run.shared_directories[0].read_only);
        let share = std::path::PathBuf::from(&run.shared_directories[0].host_path);
        let token = share.join(MACOS_SETUP_TOKEN_FILE);
        assert_eq!(std::fs::read_to_string(&token).unwrap(), "tok-1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&token).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&share).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let body = serde_json::to_value(&run).unwrap();
        assert_eq!(body["sharedDirectories"][0]["readOnly"], true);

        // A restart without a spec reuses the share.
        assert_eq!(
            rt.macos_run("cua-e2e-mac", None)
                .unwrap()
                .shared_directories
                .len(),
            1
        );
        // A later start without a token never shares the stale one.
        spec.env.clear();
        assert!(
            rt.macos_run("cua-e2e-mac", Some(&spec))
                .unwrap()
                .shared_directories
                .is_empty()
        );
        assert!(!token.exists());
    }

    #[test]
    fn macos_endpoints_default_to_lume_credentials() {
        let vm = VmDetails {
            name: "m".into(),
            os: "macOS".into(),
            status: "running".into(),
            ip_address: Some("192.168.64.9".into()),
            vnc_url: Some("vnc://:pw@127.0.0.1:5905".into()),
            ..Default::default()
        };
        let ep = LumeRuntime::endpoints_from(&vm, None, &[8080]);
        assert_eq!(ep.host, "192.168.64.9");
        assert_eq!(ep.addr(8080).as_deref(), Some("192.168.64.9:8080"));
        assert_eq!(ep.ssh.as_ref().unwrap().user, "lume");
        assert_eq!(ep.vnc.as_ref().unwrap().port, 5905);
        assert_eq!(ep.vnc.as_ref().unwrap().password.as_deref(), Some("pw"));
    }
}
