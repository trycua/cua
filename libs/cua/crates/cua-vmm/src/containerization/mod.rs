//! microVM backend on macOS: Apple's Containerization framework, through
//! its `container` CLI (Apache-2.0, <https://github.com/apple/container>).
//!
//! Every Space is its own lightweight VM (Virtualization.framework, Apple's
//! Linux kernel) running one OCI image, so a Linux Space gets a kernel of
//! its own with no shared Linux VM and no gVisor syscall tax.
//!
//! cua runs its own copy of `container` ([`install`]): the signed release
//! pinned by version and SHA-256, expanded under
//! `~/.cua/vmm/containerization/install/<version>` without root, with its
//! images and containers under `~/.cua/vmm/containerization/data`. Its
//! services are per user (launchd, one fixed service name), so when another
//! install of `container` runs them the backend refuses to start rather
//! than putting Spaces into, or stopping, a user's own setup.
//!
//! **Images** are pulled the way every runtime pulls them: through the
//! shared puller ([`ContainerizationConfig::resolver`], cua-image), which
//! holds the host's registry credentials (the daemon's
//! `CUA_REGISTRY_USERNAME` and `CUA_REGISTRY_PASSWORD`, `GITHUB_TOKEN` for
//! ghcr.io, `docker login`, a Space's own `registry_auth`), mirrors and the
//! layer cache the other runtimes share. The layers are packed unchanged
//! into an OCI archive and handed to `container image load`, unless the
//! store already holds the variant the registry serves. `container image
//! pull` cannot do this: it can only use a login in the user's keychain
//! (which cua never writes to), it refuses a private package's anonymous
//! token exchange, and it pulled the 0.5 GB slim image in 178 s where the
//! shared puller needed about 125 s on the same link (5.9 MB/s). Without a
//! resolver `container` pulls by itself.
//!
//! **First start.** Apple's package (about 20 s) and, on the services' first
//! start, its guest kernel (about 140 s) download before an image could
//! begin, so the layers come down meanwhile (a prefetch through the same
//! puller, joined before the container is made): a cold start of the slim
//! image took 245 s instead of 253 to 307 s on a link that is the limit,
//! and every later start is under 3 s.
//!
//! **Guest DNS.** Guests get the host's own resolvers (then public ones)
//! with `--dns` ([`cli::guest_dns`]). The resolver `container` configures,
//! the vmnet gateway, answers every AAAA query with the truncated bit set
//! and serves no TCP, so `getaddrinfo` failed for every program that asks
//! for both address families (curl, node, git, pip: a guest could not
//! download its coding app) while `getent hosts` worked.
//!
//! **Network policy.** None can be enforced: the guests sit on vmnet,
//! which only pf can filter and pf needs root, and Apple's runtime helper
//! attaches no other network device. A firewall inside the guest would
//! not be Cua's to promise (the Space's own root can change it), so under
//! the secure default only No protection starts a Space here, and
//! [`NO_PROTECTION_WHY`] says why and what to use instead (the `vz`
//! backend, which keeps every connection behind `cua-netgw`).
//!
//! The environment (the Spaces token among it) goes in through an env file,
//! never the command line; the host reaches the guest's ports at its own
//! address on the vmnet network (as for a Lume VM: `container`'s loopback
//! forwarder runs in a launchd helper that Local Network privacy keeps from
//! the guest); commands run with `container exec`. `container` has no pause and no
//! snapshots: suspending a Space stops it (its disk is kept), and
//! checkpoints and forks are not supported.
//!
//! **Warm pool.** [`Runtime::claim`] binds a running pool member to a Space
//! under the Space's name. `container` cannot rename a container, so the
//! name is an alias (`<root>/aliases.json`) every call resolves. A member is
//! a cold-booted microVM (its own kernel, entropy, clock and DHCP lease), so
//! the claim only renews what the caller's spacesd `Init{new_machine}` does
//! (token, machine-seal keys) plus the hostname.
//!
//! **Why there are no memory snapshots.** Virtualization.framework itself
//! can save and restore a Linux VM of this shape (macOS 14+
//! `saveMachineStateTo`/`restoreMachineStateFrom`; a generic-platform VM
//! with a direct-boot kernel, virtio-blk, entropy and a vsock device passes
//! `validateSaveRestoreSupport()` on macOS 26, and a test program
//! restored such a guest to a listening cua-spacesd in about 2 s). What
//! stops it here is Apple's stack above VZ, as pinned ([`install::VERSION`]
//! of apple/container, and apple/containerization as of 2026-10):
//! * the `container` CLI and its XPC API have no checkpoint, save or
//!   restore operation; `container export` writes the root filesystem only
//!   (a frozen disk copy, `snapshotDisk`), never memory;
//! * the `VZVirtualMachine` is created and owned by Apple's signed
//!   `container-runtime-linux` helper, one launchd service per container.
//!   VZ saves or restores a machine only from the process that owns it,
//!   and nothing in that helper's routes exposes either;
//! * Containerization's `VZVirtualMachineInstance` keeps the machine
//!   private (its `VirtualMachineInstance` protocol offers start, stop,
//!   pause, resume and agent dialing only), creates a fresh
//!   `VZGenericMachineIdentifier` per start (a restore must use the saved
//!   one), and holds vminitd's gRPC-over-vsock connections, which VZ does
//!   not carry across a save and restore;
//! * the vmnet network the guest is attached to is created by
//!   `container-network-vmnet`, another process, and a VM may only attach to
//!   a network its own process created (`VZVmnetNetworkDeviceAttachment`).
//!
//! Snapshots would need cua's own build of the runtime helper (save and
//! restore routes, a persisted machine identifier, re-dialing the agent),
//! signed by cua and installed in place of Apple's: a fork of
//! apple/container to maintain, not something this backend can do with the
//! signed release it pins. Warm pools ([`Runtime::claim`]) are what this
//! backend offers instead. A Mac's microVMs with checkpoints and forks run
//! under cua's own monitor instead ([`crate::microvm::vz`], backend `vz`),
//! which a Mac picks for `--kind microvm` when it is installed.

pub mod cli;
pub mod install;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::error::{Result, VmmError};
use crate::exec::{ExecChunk, ExecOutput, ExecRequest, GuestExec};
use crate::host;
use crate::runtime::{RootfsResolver, Runtime};
use crate::types::*;

pub use cli::{ContainerInfo, Services};
pub use install::ContainerizationReport;

/// Configuration for [`ContainerizationRuntime`].
#[derive(Clone)]
pub struct ContainerizationConfig {
    /// State root (default `~/.cua/vmm/containerization`).
    pub root: PathBuf,
    /// The `container` binary to run instead of cua's own install
    /// (`$CUA_CONTAINERIZATION_BIN`; tests point it at a fake). The services
    /// still run with this backend's application root.
    pub bin: Option<PathBuf>,
    /// Install the pinned `container` on first use when it is missing.
    pub auto_install: bool,
    /// How long `stop` lets the guest shut down before it is killed.
    pub graceful_stop: Duration,
    /// Pulls OCI images the way every other runtime does (implemented by
    /// `cua-image`: the host's registry credentials, mirrors and the shared
    /// layer cache), and hands them to `container` as an archive to load.
    /// Without one `container` pulls by itself, anonymously.
    pub resolver: Option<Arc<dyn RootfsResolver>>,
}

impl std::fmt::Debug for ContainerizationConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerizationConfig")
            .field("root", &self.root)
            .field("bin", &self.bin)
            .field("auto_install", &self.auto_install)
            .field("graceful_stop", &self.graceful_stop)
            .field("resolver", &self.resolver.as_ref().map(|_| "set"))
            .finish()
    }
}

impl Default for ContainerizationConfig {
    fn default() -> Self {
        Self {
            root: host::cua_home().join("vmm").join("containerization"),
            bin: std::env::var_os("CUA_CONTAINERIZATION_BIN")
                .filter(|b| !b.is_empty())
                .map(PathBuf::from),
            auto_install: true,
            graceful_stop: Duration::from_secs(5),
            resolver: None,
        }
    }
}

/// Timeout of a `container` call that does not pull or boot.
const QUICK: Duration = Duration::from_secs(60);

/// The microVM backend on macOS.
pub struct ContainerizationRuntime {
    cfg: ContainerizationConfig,
    /// Serializes service checks and installs.
    setup: Mutex<()>,
    /// cua's own services were seen running.
    ready: std::sync::atomic::AtomicBool,
    locks: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ContainerizationRuntime {
    pub fn new(cfg: ContainerizationConfig) -> Self {
        Self {
            cfg,
            setup: Mutex::new(()),
            ready: std::sync::atomic::AtomicBool::new(false),
            locks: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(ContainerizationConfig::default())
    }

    pub fn config(&self) -> &ContainerizationConfig {
        &self.cfg
    }

    fn app_root(&self) -> PathBuf {
        install::app_root(&self.cfg.root)
    }

    fn install_root(&self) -> PathBuf {
        install::install_root(&self.cfg.root)
    }

    /// The `container` binary, if there is one.
    ///
    /// With `runtime.microvm = system`, Apple's `container` already on this
    /// Mac (`runtime.microvm_path`, else the one on `PATH`), never cua's
    /// own copy.
    pub fn bin(&self) -> Option<PathBuf> {
        match &self.cfg.bin {
            Some(b) => Some(b.clone()),
            None if Self::uses_installed() => {
                crate::kinds::system_bin(crate::kinds::RuntimeKind::Microvm)
            }
            None => install::installed_bin(&self.cfg.root),
        }
    }

    /// MicroVMs are set to what is already installed (nothing is downloaded).
    fn uses_installed() -> bool {
        crate::kinds::source(crate::kinds::RuntimeKind::Microvm) == crate::kinds::KindSource::System
    }

    fn lock_for(&self, name: &str) -> Arc<Mutex<()>> {
        let mut m = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Runs `container <args>`; stdout on success.
    async fn cli(&self, args: &[String], timeout: Duration) -> Result<String> {
        let bin = self.bin().ok_or_else(|| {
            VmmError::missing(
                "Apple container",
                "run `cua runtime setup containerization` to install it",
            )
        })?;
        let out = self.cli_output(&bin, args, timeout).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("XPC") || stderr.contains("apiserver") {
                // The services went away: check (and start) them again.
                self.ready.store(false, Ordering::SeqCst);
            }
            return Err(VmmError::Command {
                cmd: format!(
                    "container {}",
                    args.first().map(String::as_str).unwrap_or("")
                ),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    async fn cli_output(
        &self,
        bin: &Path,
        args: &[String],
        timeout: Duration,
    ) -> Result<std::process::Output> {
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(args)
            .env("CONTAINER_APP_ROOT", self.app_root())
            .env("CONTAINER_INSTALL_ROOT", self.install_root())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(r) => Ok(r?),
            Err(_) => Err(VmmError::other(format!(
                "`container {}` did not finish in {}s",
                args.first().map(String::as_str).unwrap_or(""),
                timeout.as_secs()
            ))),
        }
    }

    /// The services' state, as `container system status` reports it.
    pub async fn services(&self) -> Result<Services> {
        let bin = self.bin().ok_or_else(|| {
            VmmError::missing(
                "Apple container",
                "run `cua runtime setup containerization` to install it",
            )
        })?;
        let args = ["system", "status", "--format", "json"].map(String::from);
        let out = self.cli_output(&bin, &args, QUICK).await?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        cli::parse_services(&stdout).ok_or_else(|| VmmError::Command {
            cmd: "container system status".into(),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }

    /// Installs `container` (when allowed) and starts its services with this
    /// backend's roots, once; refuses when another install runs them.
    pub async fn ensure_ready(&self) -> Result<()> {
        if self.ready.load(Ordering::SeqCst) {
            return Ok(());
        }
        let _g = self.setup.lock().await;
        if self.ready.load(Ordering::SeqCst) {
            return Ok(());
        }
        if self.cfg.bin.is_none() {
            if let Some(why) = install::unsupported_reason() {
                return Err(VmmError::other(why));
            }
            if Self::uses_installed() {
                if self.bin().is_none() {
                    return Err(VmmError::missing(
                        "Apple container",
                        "MicroVMs are set to use what is already installed, and Apple's container \
                         is not installed on this Mac (Settings, Advanced, or \
                         `cua config set runtime.microvm auto`)",
                    ));
                }
            } else if install::installed_bin(&self.cfg.root).is_none() {
                if !self.cfg.auto_install {
                    return Err(VmmError::missing(
                        "Apple container",
                        "run `cua runtime setup containerization` to install it",
                    ));
                }
                let t = std::time::Instant::now();
                install::install(&self.cfg.root).await?;
                tracing::info!(
                    ms = t.elapsed().as_millis() as u64,
                    "Apple container downloaded and expanded"
                );
            }
        }
        let ours = |app_root: &str| cli::same_dir(app_root, &self.app_root().to_string_lossy());
        match self.services().await? {
            Services::Running { app_root, .. } if ours(&app_root) => {}
            Services::Running { app_root, .. } => return Err(other_install(&app_root)),
            Services::Stopped => {
                std::fs::create_dir_all(self.app_root())?;
                tracing::info!(app_root = %self.app_root().display(), "starting Apple container services");
                let args: Vec<String> = vec![
                    "system".into(),
                    "start".into(),
                    "--install-root".into(),
                    self.install_root().display().to_string(),
                    "--app-root".into(),
                    self.app_root().display().to_string(),
                    // The first start downloads Apple's pinned guest kernel.
                    "--enable-kernel-install".into(),
                    "--timeout".into(),
                    "60".into(),
                ];
                let t = std::time::Instant::now();
                self.cli(&args, Duration::from_secs(600)).await?;
                tracing::info!(
                    ms = t.elapsed().as_millis() as u64,
                    "Apple container services started (the first start downloads its guest kernel)"
                );
                // `system start` reports success when launchd refuses its
                // service because another install holds the name: check.
                match self.services().await? {
                    Services::Running { app_root, .. } if ours(&app_root) => {}
                    Services::Running { app_root, .. } => return Err(other_install(&app_root)),
                    Services::Stopped => {
                        return Err(VmmError::other(
                            "Apple container services did not start (`container system logs` \
                             has details)",
                        ));
                    }
                }
            }
        }
        self.ready.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Every container in this backend's application root.
    pub async fn containers(&self) -> Result<Vec<ContainerInfo>> {
        let args = ["list", "--all", "--format", "json"].map(String::from);
        let out = self.cli(&args, QUICK).await?;
        Ok(cli::parse_containers(&out)?)
    }

    /// `<root>/aliases.json`: Space name → container id, for warm-pool
    /// members claimed under a Space's name (`container` cannot rename).
    fn aliases_path(&self) -> PathBuf {
        self.cfg.root.join("aliases.json")
    }

    fn aliases(&self) -> BTreeMap<String, String> {
        std::fs::read(self.aliases_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_aliases(&self, aliases: &BTreeMap<String, String>) -> Result<()> {
        std::fs::create_dir_all(&self.cfg.root)?;
        let tmp = self.cfg.root.join("aliases.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(aliases).unwrap_or_default())?;
        std::fs::rename(tmp, self.aliases_path())?;
        Ok(())
    }

    /// The container behind Space `name`: a claimed pool member's, else the
    /// container of that name.
    pub fn id_of(&self, name: &str) -> String {
        self.aliases()
            .remove(name)
            .unwrap_or_else(|| name.to_string())
    }

    /// The directory holding Space `name`'s container under this backend's
    /// application root (`<root>/data/containers/<id>`: its own root file
    /// system and state; the image it came from is stored apart).
    pub fn container_dir(&self, name: &str) -> PathBuf {
        self.app_root().join("containers").join(self.id_of(name))
    }

    /// The Space name container `id` goes by.
    fn name_of(&self, id: &str, aliases: &BTreeMap<String, String>) -> String {
        aliases
            .iter()
            .find(|(_, v)| v.as_str() == id)
            .map_or_else(|| id.to_string(), |(k, _)| k.clone())
    }

    /// One container (by Space name), or [`VmmError::NotFound`].
    pub async fn inspect(&self, name: &str) -> Result<ContainerInfo> {
        let id = self.id_of(name);
        let args = vec!["inspect".to_string(), id.clone()];
        match self.cli(&args, QUICK).await {
            Ok(out) => cli::parse_containers(&out)?
                .into_iter()
                .find(|c| c.id == id)
                .ok_or_else(|| VmmError::NotFound(name.into())),
            Err(VmmError::Command { stderr, .. }) if cli::not_found(&stderr) => {
                Err(VmmError::NotFound(name.into()))
            }
            Err(e) => Err(e),
        }
    }

    /// The guest at its own address (on the host's vmnet network), every
    /// port it serves at the same number, as for a Lume VM. A stopped guest
    /// has no address.
    fn endpoints_of(c: &ContainerInfo) -> Endpoints {
        let mut ports = c.ports.clone();
        if c.ip.is_some() {
            ports.extend(c.served.iter().map(|p| (*p, *p)));
        }
        Endpoints {
            host: c.ip.clone().unwrap_or_default(),
            ports,
            container_id: Some(c.id.clone()),
            ..Default::default()
        }
    }

    fn instance(c: &ContainerInfo, name: &str) -> Instance {
        Instance {
            name: name.to_string(),
            backend: BackendKind::Containerization,
            status: c.status(),
            endpoints: Self::endpoints_of(c),
            isolation: Isolation::Vm { accel: "vz".into() },
            arch: Some(Arch::Aarch64),
        }
    }

    /// Waits for `name` to run and for every probe to pass (dialed at the
    /// guest's own address: nothing on the host accepts for it).
    async fn wait_ready(
        &self,
        name: &str,
        probes: &[Probe],
        timeout: Duration,
    ) -> Result<Instance> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut backoff = crate::probe::Backoff::default();
        let c = loop {
            let c = self.inspect(name).await?;
            match c.status() {
                Status::Running => break c,
                Status::Stopped if c.state == "stopped" => {
                    let logs = self
                        .cli(
                            &["logs".into(), "-n".into(), "20".into(), self.id_of(name)],
                            QUICK,
                        )
                        .await
                        .unwrap_or_default();
                    return Err(VmmError::other(format!(
                        "{name} stopped while starting{}",
                        if logs.trim().is_empty() {
                            String::new()
                        } else {
                            format!(":\n{}", crate::exec::tail(logs.trim(), 2000))
                        }
                    )));
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(VmmError::Timeout {
                    name: name.into(),
                    secs: timeout.as_secs(),
                    detail: format!("still {}", c.state),
                });
            }
            tokio::time::sleep(backoff.step()).await;
        };
        let ep = Self::endpoints_of(&c);
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        crate::probe::wait_all(name, &ep, probes, left).await?;
        Ok(Self::instance(&c, name))
    }

    /// Whether the guest listens on `port` (`/proc/net/tcp*`), `None` when
    /// it cannot say.
    async fn guest_listening(&self, name: &str, port: u16) -> Option<bool> {
        let id = self.id_of(name);
        let args: Vec<String> = ["exec", &id, "cat", "/proc/net/tcp", "/proc/net/tcp6"]
            .map(String::from)
            .to_vec();
        let bin = self.bin()?;
        let out = self.cli_output(&bin, &args, QUICK).await.ok()?;
        // `cat` exits 1 when one table is missing (no IPv6); the other is
        // still printed.
        crate::probe::proc_net_listens(&String::from_utf8_lossy(&out.stdout), port)
    }

    /// The directory the injected cua-spacesd is bound from, with the
    /// binary in it as `cua-spacesd` (cached by the binary's content), and
    /// `spec` with the supervisor as its command.
    async fn inject(
        &self,
        spec: &StartSpec,
        binary: &Path,
        image: &str,
    ) -> Result<(PathBuf, StartSpec)> {
        let id = {
            let b = binary.to_path_buf();
            tokio::task::spawn_blocking(move || crate::inject::content_id(&b))
                .await
                .map_err(|e| VmmError::other(e.to_string()))??
        };
        let dir = self.cfg.root.join("spacesd").join(&id);
        let staged = dir.join("cua-spacesd");
        if !staged.is_file() {
            std::fs::create_dir_all(&dir)?;
            let tmp = dir.join("cua-spacesd.partial");
            std::fs::copy(binary, &tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o555))?;
            }
            std::fs::rename(&tmp, &staged)?;
        }
        // The image's own command, user and working directory: pulled, then
        // read back from the image store.
        self.ensure_image(spec, image).await?;
        let out = self
            .cli(&["image".into(), "inspect".into(), image.into()], QUICK)
            .await?;
        let arch = spec.effective_arch().oci();
        let run = cli::parse_run_config(&out, arch).ok_or_else(|| {
            VmmError::other(format!(
                "could not read the run config of {image} from `container`"
            ))
        })?;
        let mut wrapped = spec.clone();
        wrapped.command = Some(crate::inject::wrapper_command(
            &run,
            spec.command.as_deref(),
        ));
        Ok((dir, wrapped))
    }

    /// Makes `image` present in `container`'s image store, pulled the way
    /// every runtime pulls: through the resolver ([`ContainerizationConfig::resolver`]),
    /// which holds the host's registry credentials, mirrors and the shared
    /// layer cache, then loaded as an archive. `container pull` itself can
    /// only use a login kept in the user's keychain (which cua never writes
    /// to) and refuses a private package's anonymous token exchange, and it
    /// pulled about half as fast as the shared puller on the same link.
    /// Without a resolver (tests) or when it cannot serve the image,
    /// `container` pulls it itself, anonymously.
    async fn ensure_image(&self, spec: &StartSpec, image: &str) -> Result<()> {
        let budget = spec.ready_timeout.max(Duration::from_secs(600));
        let mut through_cua: Option<VmmError> = None;
        if let Some(resolver) = &self.cfg.resolver {
            let lock = self.lock_for(&format!("image:{image}"));
            let _g = lock.lock().await;
            match self
                .load_image(resolver.as_ref(), spec, image, budget)
                .await
            {
                Ok(()) => return Ok(()),
                // `container` cannot use a credential it was handed, so
                // another pull would only hide the reason.
                Err(e) if spec.registry_auth.is_some() => return Err(e),
                Err(e) => {
                    tracing::warn!(image, error = %e, "could not pull through cua; `container` pulls it");
                    through_cua = Some(e);
                }
            }
        }
        let mut pull: Vec<String> = ["image", "pull", "--progress", "none"]
            .map(String::from)
            .into();
        if let Some(arch) = spec.arch {
            pull.extend(["--arch".into(), arch.oci().into()]);
        }
        pull.push(image.into());
        match self.cli(&pull, budget).await {
            Ok(_) => Ok(()),
            Err(VmmError::Command { stderr, .. }) if cli::auth_refused(&stderr) => {
                Err(sign_in_hint(image, through_cua))
            }
            Err(e) => Err(e),
        }
    }

    /// Pulls `image` through `resolver` and loads it into `container`'s
    /// store, unless the variant the registry serves now is there already.
    async fn load_image(
        &self,
        resolver: &dyn RootfsResolver,
        spec: &StartSpec,
        image: &str,
        budget: Duration,
    ) -> Result<()> {
        let arch = spec.effective_arch();
        let creds = spec.registry_auth.as_ref();
        // A digest names one image for good: if the store has it, the
        // registry need not be asked (three requests, 0.7 s on every start).
        if image.contains("@sha256:") && self.image_present(image).await {
            return Ok(());
        }
        // The layers go to the shared cache; the digest says which variant
        // the registry serves now.
        let t = std::time::Instant::now();
        let layers = resolver.resolve_rootfs(image, arch, creds).await?;
        let pulled = t.elapsed();
        if self.image_has(image, &layers.digest).await {
            return Ok(());
        }
        let dir = self.cfg.root.join("run");
        std::fs::create_dir_all(&dir)?;
        let archive = dir.join(format!("image-{}.tar", cli::short_hash(image)));
        let t = std::time::Instant::now();
        let loaded: Result<String> = async {
            let digest = resolver
                .pull_oci_archive(image, arch, creds, &archive)
                .await?;
            let args: Vec<String> = vec![
                "image".into(),
                "load".into(),
                "--input".into(),
                archive.display().to_string(),
            ];
            let packed = t.elapsed();
            let t = std::time::Instant::now();
            self.cli(&args, budget).await?;
            tracing::info!(
                image,
                pull_ms = pulled.as_millis() as u64,
                pack_ms = packed.as_millis() as u64,
                load_ms = t.elapsed().as_millis() as u64,
                "image pulled through cua and loaded into Apple's store"
            );
            Ok(digest)
        }
        .await;
        let _ = std::fs::remove_file(&archive);
        let digest = loaded?;
        if !self.image_has(image, &digest).await {
            return Err(VmmError::other(format!(
                "`container` loaded {image} but does not list it"
            )));
        }
        Ok(())
    }

    /// Whether the store holds `image` at all.
    async fn image_present(&self, image: &str) -> bool {
        let args = ["image".to_string(), "inspect".into(), image.into()];
        self.cli(&args, QUICK).await.is_ok()
    }

    /// Whether the store holds `image` with a variant of manifest `digest`.
    async fn image_has(&self, image: &str, digest: &str) -> bool {
        let args = ["image".to_string(), "inspect".into(), image.into()];
        self.cli(&args, QUICK)
            .await
            .is_ok_and(|out| cli::image_has_variant(&out, digest))
    }

    /// The nameservers for a guest ([`cli::guest_dns`]).
    fn guest_dns(&self) -> Vec<String> {
        let conf = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        cli::guest_dns(std::env::var("CUA_GUEST_DNS").ok().as_deref(), &conf)
    }

    /// Creates and starts the container for `spec`.
    async fn create(&self, spec: &StartSpec, image: &str) -> Result<()> {
        let injected;
        let (spec, inject_dir) = match &spec.inject_spacesd {
            Some(binary) => {
                let (dir, wrapped) = self.inject(spec, binary, image).await?;
                injected = wrapped;
                (&injected, Some(dir))
            }
            None => {
                // `container run` would pull a missing image anonymously;
                // the shared puller holds the credentials.
                if self.cfg.resolver.is_some() {
                    self.ensure_image(spec, image).await?;
                }
                (spec, None)
            }
        };
        let mut labels = BTreeMap::from([
            (
                cli::LABEL_BACKEND.to_string(),
                "containerization".to_string(),
            ),
            (cli::LABEL_NAME.to_string(), spec.name.clone()),
            ("ai.cua.managed".to_string(), "true".to_string()),
            ("ai.cua.kind".to_string(), "sandbox".to_string()),
            ("ai.cua.created".to_string(), host::now_secs().to_string()),
        ]);
        let mut served: Vec<u16> = spec.ports.clone();
        served.extend(spec.probes.iter().filter_map(Probe::port));
        served.sort_unstable();
        served.dedup();
        if !served.is_empty() {
            let list: Vec<String> = served.iter().map(u16::to_string).collect();
            labels.insert(cli::LABEL_PORTS.to_string(), list.join(","));
        }
        labels.extend(spec.labels.clone());
        // The environment carries the Spaces token: an owner-only file,
        // removed once `container` has read it.
        let env_file = if spec.env.is_empty() {
            None
        } else {
            let body = cli::env_file(&spec.env).map_err(VmmError::invalid)?;
            let dir = self.cfg.root.join("run");
            std::fs::create_dir_all(&dir)?;
            let p = dir.join(format!("{}.env", spec.name));
            host::write_private(&p, body.as_bytes())?;
            Some(p)
        };
        let args = cli::run_args(
            spec,
            image,
            env_file.as_deref().and_then(Path::to_str),
            &labels,
            inject_dir.as_deref().and_then(Path::to_str),
            &self.guest_dns(),
        );
        let budget = spec.ready_timeout.max(Duration::from_secs(600));
        let r = self.cli(&args, budget).await;
        if let Some(p) = &env_file {
            let _ = std::fs::remove_file(p);
        }
        if let Err(e) = r {
            let e = match e {
                VmmError::Command { stderr, .. } if cli::auth_refused(&stderr) => {
                    sign_in_hint(image, None)
                }
                other => other,
            };
            // A half-made container would hold the name.
            let _ = self
                .cli(
                    &["delete".into(), "--force".into(), spec.name.clone()],
                    QUICK,
                )
                .await;
            return Err(e);
        }
        Ok(())
    }
}

/// Why a Space on Apple's Containerization starts only with No protection
/// (the support table's words, [`crate::network::WHY_CONTAINERIZATION`]).
pub const NO_PROTECTION_WHY: &str = crate::network::WHY_CONTAINERIZATION;

/// What a person can do when `image` needs a registry sign-in: the reason
/// the shared puller gave when it was asked first (the registry's own words),
/// else how to give Cua the credential.
fn sign_in_hint(image: &str, through_cua: Option<VmmError>) -> VmmError {
    VmmError::other(match through_cua {
        Some(e) => e.to_string(),
        None => format!(
            "{image} is private or needs a registry sign-in. Cua pulls with the credentials \
             it holds (CUA_REGISTRY_USERNAME and CUA_REGISTRY_PASSWORD in the daemon's \
             environment, `docker login`, or GITHUB_TOKEN for ghcr.io)"
        ),
    })
}

fn other_install(app_root: &str) -> VmmError {
    VmmError::other(format!(
        "Apple's container services are running from another install of `container` \
         (data in {app_root}). They are per user, so cua cannot run its own beside them: \
         stop them with `container system stop`, then try again"
    ))
}

#[async_trait]
impl Runtime for ContainerizationRuntime {
    fn kind(&self) -> BackendKind {
        BackendKind::Containerization
    }

    async fn ensure_base(&self, _image: &ImageSource, _base_name: &str) -> Result<CheckpointInfo> {
        Err(VmmError::Unsupported {
            backend: "containerization",
            op: "base instances (forks are not supported)",
        })
    }

    async fn start(&self, spec: &StartSpec) -> Result<Instance> {
        reject_sidecars(BackendKind::Containerization, spec)?;
        crate::types::reject_mounts(BackendKind::Containerization, spec)?;
        // Apple's Containerization can't protect a Space's network: under the
        // secure default only No protection starts it (anything else is
        // refused in plain words); as before under the old one.
        match crate::network::gate(
            BackendKind::Containerization,
            &spec.name,
            &crate::network::spec_settings(spec),
        )? {
            Some(crate::network::NetworkPlan::Open { .. }) => {}
            Some(_) => {
                return Err(VmmError::Other(NO_PROTECTION_WHY.into()));
            }
            None => {
                reject_restricted_network(BackendKind::Containerization, spec)?;
                crate::egress::check(BackendKind::Containerization, &spec.egress)?;
            }
        }
        reject_gpu(BackendKind::Containerization, spec, &[])?;
        if spec.os != GuestOs::Linux {
            return Err(VmmError::Unsupported {
                backend: "containerization",
                op: "guests other than Linux (macOS guests run on Lume)",
            });
        }
        if spec.disk_size_gb.is_some() {
            return Err(VmmError::Unsupported {
                backend: "containerization",
                op: "a disk size (the image's filesystem grows as needed)",
            });
        }
        if !cli::valid_name(&spec.name) {
            return Err(VmmError::invalid(format!(
                "{:?} is not a valid microVM name (letters, digits, '_', '.', '-', at most 63)",
                spec.name
            )));
        }
        // The first start downloads and starts Apple's container (about
        // 20 s for its package, then about 140 s for the guest kernel its
        // services fetch on their first start) before the image could
        // begin: the image's layers come down meanwhile through the shared
        // puller, which needs none of that. Joined before the container is
        // made, so nothing downloads twice.
        let prefetch = match (&spec.image, &self.cfg.resolver) {
            (ImageSource::Oci { reference }, Some(resolver))
                if !self.ready.load(Ordering::SeqCst) =>
            {
                let (resolver, reference) = (resolver.clone(), reference.clone());
                let (arch, creds) = (spec.effective_arch(), spec.registry_auth.clone());
                // One download of an image at a time (the same lock as
                // [`Self::ensure_image`]): two starts of the same image must
                // not write one blob file together.
                let lock = self.lock_for(&format!("image:{reference}"));
                Some(tokio::spawn(async move {
                    let _g = lock.lock().await;
                    let _ = resolver
                        .resolve_rootfs(&reference, arch, creds.as_ref())
                        .await;
                }))
            }
            _ => None,
        };
        self.ensure_ready().await?;
        if let Some(prefetch) = prefetch {
            let _ = prefetch.await;
        }
        let lock = self.lock_for(&spec.name);
        let _g = lock.lock().await;
        match self.inspect(&spec.name).await {
            Ok(c) if c.status() == Status::Running => return Ok(Self::instance(&c, &spec.name)),
            Ok(c) => {
                self.cli(&["start".into(), c.id], QUICK).await?;
            }
            Err(VmmError::NotFound(_)) => match &spec.image {
                ImageSource::Oci { reference } => self.create(spec, reference).await?,
                ImageSource::Existing => return Err(VmmError::NotFound(spec.name.clone())),
                ImageSource::Disk { .. } => {
                    return Err(VmmError::Unsupported {
                        backend: "containerization",
                        op: "disk images (a microVM boots an OCI image)",
                    });
                }
            },
            Err(e) => return Err(e),
        }
        self.wait_ready(&spec.name, &spec.probes, spec.ready_timeout)
            .await
    }

    async fn stop(&self, name: &str) -> Result<()> {
        self.ensure_ready().await?;
        let c = self.inspect(name).await?;
        if c.status() != Status::Running {
            return Ok(());
        }
        let secs = self.cfg.graceful_stop.as_secs().max(1).to_string();
        self.cli(
            &["stop".into(), "--time".into(), secs, c.id],
            QUICK + self.cfg.graceful_stop,
        )
        .await
        .map(drop)
    }

    /// `container` has no pause: a suspended Space is stopped (its disk
    /// kept) and boots again on resume, as a Lume VM does.
    async fn suspend(&self, name: &str) -> Result<()> {
        self.stop(name).await
    }

    async fn resume(&self, name: &str) -> Result<Instance> {
        self.ensure_ready().await?;
        let c = self.inspect(name).await?;
        if c.status() != Status::Running {
            self.cli(&["start".into(), c.id], QUICK).await?;
        }
        self.wait_ready(name, &[], QUICK).await
    }

    /// Binds the running pool member `member` to a Space: `container`
    /// cannot rename, so the Space name becomes an alias of the member's
    /// container ([`Self::id_of`]) that every later call resolves, and the
    /// guest's hostname is set to `identity.hostname` (default `new_name`)
    /// in the background. A member is a cold-booted microVM with a kernel,
    /// entropy pool, clock and DHCP lease of its own (nothing is restored
    /// from a snapshot), so only the Spaces token and machine-seal keys
    /// need renewing, which the caller's `Init{new_machine}` does.
    async fn claim(
        &self,
        member: &str,
        new_name: &str,
        identity: &ClaimIdentity,
    ) -> Result<Instance> {
        if !cli::valid_name(new_name) {
            return Err(VmmError::invalid(format!(
                "{new_name:?} is not a valid microVM name (letters, digits, '_', '.', '-', at most 63)"
            )));
        }
        self.ensure_ready().await?;
        let lock = self.lock_for(member);
        let _g = lock.lock().await;
        let c = self.inspect(member).await?;
        if c.status() != Status::Running {
            return Err(VmmError::invalid(format!(
                "pool member '{member}' is not running ({})",
                c.state
            )));
        }
        match self.inspect(new_name).await {
            Err(VmmError::NotFound(_)) => {}
            Ok(_) => return Err(VmmError::AlreadyExists(new_name.into())),
            Err(e) => return Err(e),
        }
        let mut aliases = self.aliases();
        aliases.remove(member);
        aliases.insert(new_name.to_string(), c.id.clone());
        self.save_aliases(&aliases)?;
        if let Some(bin) = self.bin() {
            let exec = ContainerizationExec {
                bin,
                name: c.id.clone(),
            };
            let host = cli::hostname(identity.hostname.as_deref().unwrap_or(new_name));
            let name = new_name.to_string();
            crate::cleanup::spawn(async move {
                let req = ExecRequest::sh(format!(
                    "hostname {h} && echo {h} > /etc/hostname",
                    h = crate::exec::shell_quote(&host)
                ))
                .timeout(Duration::from_secs(20));
                match exec.exec(req).await {
                    Ok(out) if out.exit_code == 0 => {}
                    Ok(out) => {
                        tracing::warn!(vm = %name, code = out.exit_code, "could not set the claimed guest's hostname")
                    }
                    Err(e) => {
                        tracing::warn!(vm = %name, error = %e, "could not set the claimed guest's hostname")
                    }
                }
            });
        }
        Ok(Self::instance(&c, new_name))
    }

    async fn fork(&self, _source: &str, _new_name: &str) -> Result<()> {
        Err(VmmError::Unsupported {
            backend: "containerization",
            op: "forks (Apple's container has no VM snapshots)",
        })
    }

    async fn checkpoint(&self, _name: &str, _checkpoint: &str) -> Result<CheckpointInfo> {
        Err(VmmError::Unsupported {
            backend: "containerization",
            op: "checkpoints (Apple's container has no VM snapshots)",
        })
    }

    async fn list(&self) -> Result<Vec<InstanceSummary>> {
        // Listing never installs or starts anything.
        if !self.services_ours().await {
            return Ok(vec![]);
        }
        let aliases = self.aliases();
        Ok(self
            .containers()
            .await?
            .iter()
            .filter(|c| c.ours())
            .map(|c| InstanceSummary {
                name: self.name_of(&c.id, &aliases),
                backend: BackendKind::Containerization,
                status: c.status(),
            })
            .collect())
    }

    async fn status(&self, name: &str) -> Result<Status> {
        // Never install or start anything to answer "is it here?".
        if !self.services_ours().await {
            return Err(VmmError::NotFound(name.into()));
        }
        Ok(self.inspect(name).await?.status())
    }

    async fn delete(&self, name: &str) -> Result<()> {
        self.ensure_ready().await?;
        let lock = self.lock_for(name);
        let _g = lock.lock().await;
        let c = self.inspect(name).await?;
        self.cli(&["delete".into(), "--force".into(), c.id], QUICK)
            .await?;
        let mut aliases = self.aliases();
        if aliases.remove(name).is_some() {
            self.save_aliases(&aliases)?;
        }
        Ok(())
    }

    async fn endpoints(&self, name: &str) -> Result<Endpoints> {
        Ok(Self::endpoints_of(&self.inspect(name).await?))
    }

    async fn guest_tcp_listening(&self, name: &str, guest_port: u16) -> Result<Option<bool>> {
        if self.inspect(name).await?.status() != Status::Running {
            return Ok(Some(false));
        }
        Ok(self.guest_listening(name, guest_port).await)
    }

    async fn guest_exec(&self, name: &str) -> Result<Option<Arc<dyn GuestExec>>> {
        let bin = self.bin().ok_or_else(|| VmmError::NotFound(name.into()))?;
        Ok(Some(Arc::new(ContainerizationExec {
            bin,
            name: self.id_of(name),
        })))
    }
}

impl ContainerizationRuntime {
    /// Whether cua's own services run (read-only; for lookups). Remembered
    /// once seen.
    async fn services_ours(&self) -> bool {
        if self.ready.load(Ordering::SeqCst) {
            return true;
        }
        if self.bin().is_none() {
            return false;
        }
        let ours = match self.services().await {
            Ok(Services::Running { app_root, .. }) => {
                cli::same_dir(&app_root, &self.app_root().to_string_lossy())
            }
            _ => false,
        };
        if ours {
            self.ready.store(true, Ordering::SeqCst);
        }
        ours
    }

    /// `cua runtime doctor`'s report (read-only).
    pub async fn report(&self) -> ContainerizationReport {
        let mut r = ContainerizationReport {
            supported_host: install::supported_host(),
            macos_version: host::macos_version(),
            binary: self.bin(),
            version: install::VERSION.into(),
            services: "stopped".into(),
            other_app_root: None,
        };
        if r.binary.is_some()
            && let Ok(Services::Running { app_root, .. }) = self.services().await
        {
            if cli::same_dir(&app_root, &self.app_root().to_string_lossy()) {
                r.services = "running".into();
            } else {
                r.services = "other".into();
                r.other_app_root = Some(app_root);
            }
        }
        r
    }
}

/// `container exec` as a [`GuestExec`].
#[derive(Clone, Debug)]
pub struct ContainerizationExec {
    pub bin: PathBuf,
    pub name: String,
}

impl ContainerizationExec {
    /// `exec [-i] [--user u] <name> /bin/sh -c <script>`.
    pub fn argv(&self, req: &ExecRequest) -> Vec<String> {
        let mut a = vec!["exec".to_string()];
        if req.stdin.is_some() {
            a.push("--interactive".into());
        }
        if let Some(u) = &req.user {
            a.push("--user".into());
            a.push(u.clone());
        }
        a.push(self.name.clone());
        a.push("/bin/sh".into());
        a.push("-c".into());
        a.push(crate::exec::remote_script(req));
        a
    }

    fn program(&self) -> String {
        self.bin.display().to_string()
    }
}

#[async_trait]
impl GuestExec for ContainerizationExec {
    async fn exec(&self, req: ExecRequest) -> Result<ExecOutput> {
        crate::exec::run_with_stdin(
            &self.program(),
            &self.argv(&req),
            req.stdin.as_deref(),
            req.timeout,
        )
        .await
    }

    async fn exec_streaming(
        &self,
        req: ExecRequest,
        sink: tokio::sync::mpsc::Sender<ExecChunk>,
    ) -> Result<i64> {
        crate::exec::run_streaming(
            &self.program(),
            &self.argv(&req),
            req.stdin.as_deref(),
            req.timeout,
            sink,
        )
        .await
    }
}
