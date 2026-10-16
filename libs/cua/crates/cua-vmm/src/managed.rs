//! The built-in Linux runtime: a small VM cua sets up and boots itself, so
//! Linux Spaces run on a Mac with no Docker, under gVisor, with no setup.
//!
//! # How it works
//!
//! * **Colima, pinned.** A pinned [Colima](https://github.com/abiosoft/colima)
//!   ([`COLIMA_VERSION`]) and [Lima](https://lima-vm.io) ([`LIMA_VERSION`]),
//!   SHA-256 checked, run the VM on Apple's Virtualization.framework (`vz`).
//!   They live in cua's own directory ([`root`]) and run with their own
//!   `HOME` and `COLIMA_HOME` there, in a profile named [`PROFILE`], so a
//!   Colima, Lima or Docker of the user's is never read or touched. Colima
//!   needs a `docker` CLI only to manage Docker contexts; cua gives it a
//!   no-op one, so no Docker context is ever created.
//! * **The disk.** `ghcr.io/trycua/runtime-gvisor` (`libs/images/runtime-gvisor`):
//!   Colima's own Ubuntu Docker disk with gVisor's `runsc` baked in, pinned
//!   by its layer digest ([`DISK_SHA256_ARM64`]), so the first boot
//!   downloads no packages. The profile registers `runsc` as Docker's
//!   default runtime.
//! * **The socket.** Lima forwards the guest's Docker socket to
//!   `<root>/vm/cua/docker.sock` ([`socket`]) over its SSH connection, mode
//!   0600: only this user, and only the SDK knows it is there (it is not a
//!   Docker context and not a well-known socket path).
//!   [`crate::container::ContainerRuntime`] connects to it unchanged:
//!   containers, exec, checkpoints and published ports work as on any
//!   engine. Ports published on the VM's 127.0.0.1 are forwarded to the
//!   Mac's 127.0.0.1 by Lima.
//! * **No host files.** The VM mounts no host directory.
//! * **Lifecycle.** Set up and booted on the first Linux Space that needs
//!   it ([`ensure`]), with the downloads and the boot reported as the
//!   create's progress. One VM for every Linux Space; [`ManagedContainers`]
//!   stops it when no Linux Space runs any more and boots it again on the
//!   next start. [`remove`] (Remove host setup) deletes it and everything
//!   it downloaded.
//! * **Which engine.** [`LinuxSource`] (`cua config set runtime.linux`, the
//!   apps' Settings): `auto` uses a working Docker engine on this Mac and
//!   the built-in runtime otherwise (and keeps using the built-in one once
//!   it exists, so its Spaces stay reachable); `builtin` always the built-in
//!   one; `system` only this Mac's own engine, never downloading anything.

use std::path::{Path, PathBuf};

use crate::error::{Result, VmmError};
use crate::host;

/// Colima's profile (its Lima instance is `colima-cua`).
pub const PROFILE: &str = "cua";
/// Fixed instance name of the runtime VM, as Lima names it.
pub const INSTANCE_NAME: &str = "colima-cua";

/// The pinned Colima release.
pub const COLIMA_VERSION: &str = "0.10.3";
/// The pinned Lima release.
pub const LIMA_VERSION: &str = "2.2.0";
/// The runtime disk's repository and pinned tag.
pub const DISK_REPOSITORY: &str = "trycua/runtime-gvisor";
/// The pinned tag (for people; the digests below are what is checked).
pub const DISK_TAG: &str = "0.1.0-rc1";
/// gVisor release baked into the disk.
pub const GVISOR_VERSION: &str = "20260921.0";

/// The arm64 disk layer's digest (sha256 of the `.raw.gz`) and size.
pub const DISK_SHA256_ARM64: &str =
    "0fa448f22f97dc54416cff056ae82d3bb194ec8ec58dd0983440eaecb2921fa0";
pub const DISK_SIZE_ARM64: u64 = 424_641_499;
/// The amd64 disk layer's digest and size.
pub const DISK_SHA256_AMD64: &str =
    "3f2b0446e7694417a0956c786c7de76a083fe9e3ad0794879a99d3b69044bd4a";
pub const DISK_SIZE_AMD64: u64 = 457_760_272;

/// One pinned download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// Colima for this Mac's architecture.
pub fn colima_pin() -> Pin {
    if cfg!(target_arch = "x86_64") {
        Pin {
            url: "https://github.com/abiosoft/colima/releases/download/v0.10.3/colima-Darwin-x86_64",
            sha256: "3082737fe8a98afda11cba7d9a20b6e56fe80c6153464beda04bec630758770b",
            size: 16_954_240,
        }
    } else {
        Pin {
            url: "https://github.com/abiosoft/colima/releases/download/v0.10.3/colima-Darwin-arm64",
            sha256: "980ad8bf61a4ca370243f4cb41401a61276dcd2c2502bee7b9b86f9250169f34",
            size: 15_656_320,
        }
    }
}

/// Lima for this Mac's architecture.
pub fn lima_pin() -> Pin {
    if cfg!(target_arch = "x86_64") {
        Pin {
            url: "https://github.com/lima-vm/lima/releases/download/v2.2.0/lima-2.2.0-Darwin-x86_64.tar.gz",
            sha256: "0d6f99c19f6e4bc3c92730c4c29d929e6927f0cb0a0ba1a84383367135a8ff31",
            size: 24_415_554,
        }
    } else {
        Pin {
            url: "https://github.com/lima-vm/lima/releases/download/v2.2.0/lima-2.2.0-Darwin-arm64.tar.gz",
            sha256: "bbdef91774885a0d05f7b048c4eb89ae2bcf3a0c252ae7ca7934e63df76d93c3",
            size: 37_586_365,
        }
    }
}

/// The runtime disk's layer for this Mac's architecture: its digest is
/// checked after the download.
pub fn disk_pin() -> Pin {
    if cfg!(target_arch = "x86_64") {
        Pin {
            url: "",
            sha256: DISK_SHA256_AMD64,
            size: DISK_SIZE_AMD64,
        }
    } else {
        Pin {
            url: "",
            sha256: DISK_SHA256_ARM64,
            size: DISK_SIZE_ARM64,
        }
    }
}

/// Everything the first Linux Space downloads, in bytes.
pub fn first_use_download() -> u64 {
    colima_pin().size + lima_pin().size + disk_pin().size
}

/// One-line status for `doctor`.
pub const STATUS: &str =
    "built-in Linux runtime (a small VM with Docker and gVisor), set up on first use";

/// The setting that picks the Linux engine: `runtime.linux` in
/// `$CUA_HOME/config.toml`, overridden by `CUA_RUNTIME_LINUX`.
pub const SETTING_ENV: &str = "CUA_RUNTIME_LINUX";

/// Which engine local Linux Spaces run on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinuxSource {
    /// A working Docker engine on this Mac, else the built-in runtime.
    #[default]
    Auto,
    /// Always the built-in runtime.
    Builtin,
    /// Only this Mac's own engine (nothing is downloaded).
    System,
}

impl LinuxSource {
    /// `auto`, `builtin` or `system`.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Self::Auto),
            "builtin" | "built-in" => Some(Self::Builtin),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    /// The setting's word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Builtin => "builtin",
            Self::System => "system",
        }
    }

    /// The setting now: the environment, else the config file, else auto.
    /// Read on every use, so a change in Settings applies at once.
    pub fn current() -> Self {
        if let Some(s) = std::env::var(SETTING_ENV)
            .ok()
            .and_then(|v| Self::parse(&v))
        {
            return s;
        }
        Self::from_config(&host::cua_home().join("config.toml"))
    }

    /// `[runtime] linux = "..."` in `path` (auto when absent or unreadable).
    pub fn from_config(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
            .and_then(|d| {
                d.get("runtime")?
                    .get("linux")?
                    .as_str()
                    .and_then(Self::parse)
            })
            .unwrap_or_default()
    }

    /// Whether the built-in runtime may be used (and downloaded).
    pub fn allows_builtin(self) -> bool {
        self != Self::System && supported()
    }
}

/// The built-in runtime runs on macOS 13+ (Virtualization.framework).
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// Local Linux Spaces run on the built-in runtime here, as [`choose`]
/// decides without probing an engine: `builtin`, or `auto` once the
/// built-in runtime exists.
pub fn in_use() -> bool {
    supported()
        && match LinuxSource::current() {
            LinuxSource::Builtin => true,
            LinuxSource::Auto => exists(),
            LinuxSource::System => false,
        }
}

/// `$CUA_HOME/runtimes/linux`: everything the built-in runtime has.
pub fn root() -> PathBuf {
    host::cua_home().join("runtimes").join("linux")
}

/// The pinned Colima and Lima.
fn tools_dir() -> PathBuf {
    root().join(format!("colima-{COLIMA_VERSION}-lima-{LIMA_VERSION}"))
}

/// Colima's `COLIMA_HOME` (Lima's home is `<vm>/_lima`).
fn vm_dir() -> PathBuf {
    root().join("vm")
}

/// The `HOME` Colima and Lima run with (their caches, a Docker config
/// nothing reads).
fn home() -> PathBuf {
    root().join("home")
}

/// The Docker API socket of the runtime VM, while it runs.
pub fn socket() -> PathBuf {
    vm_dir().join(PROFILE).join("docker.sock")
}

/// The socket as an engine endpoint.
pub fn endpoint() -> String {
    format!("unix://{}", socket().display())
}

/// Whether an engine endpoint is the built-in runtime's.
pub fn is_endpoint(uri: &str) -> bool {
    Path::new(uri.trim_start_matches("unix://")).starts_with(root())
}

/// Lima's directory for the VM (it exists once the VM was created).
fn instance_dir() -> PathBuf {
    vm_dir().join("_lima").join(INSTANCE_NAME)
}

/// The VM was created (it may be stopped).
pub fn exists() -> bool {
    instance_dir().join("lima.yaml").is_file()
}

/// The VM runs and its Docker engine answers on [`socket`].
pub fn running() -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(socket()).is_ok()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Lima's sockets live in the VM's directory and must fit a Unix socket
/// path (104 bytes on macOS, with the suffix Lima adds).
fn check_path_length() -> Result<()> {
    let longest = instance_dir().join("ssh.sock.1234567890123456");
    let len = longest.as_os_str().len();
    if len >= 104 {
        return Err(VmmError::other(format!(
            "the built-in Linux runtime cannot run from {} (its path is {len} bytes; Unix \
             sockets allow 103): set CUA_HOME to a shorter directory",
            root().display()
        )));
    }
    Ok(())
}

/// Container names the engine had when the VM last stopped: their Spaces
/// are stopped, not gone, while the VM is down.
fn stopped_names_file() -> PathBuf {
    root().join("stopped-spaces.json")
}

fn stopped_names() -> Vec<String> {
    std::fs::read(stopped_names_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// The pinned Colima and Lima, once installed and checked.
#[derive(Clone, Debug)]
struct Tools {
    colima: PathBuf,
    bin: PathBuf,
    lima_bin: PathBuf,
}

/// Written last, once every tool passed its checks.
const VERIFIED: &str = ".verified";

fn installed_tools() -> Option<Tools> {
    let d = tools_dir();
    let t = Tools {
        colima: d.join("bin/colima"),
        bin: d.join("bin"),
        lima_bin: d.join("lima/bin"),
    };
    (t.colima.is_file() && t.lima_bin.join("limactl").is_file() && d.join(VERIFIED).is_file())
        .then_some(t)
}

/// The no-op `docker` Colima's docker runtime asks for (it only manages
/// Docker contexts with it, which cua never wants).
const DOCKER_SHIM: &str = "#!/bin/sh\n# Cua's built-in Linux runtime: Colima manages Docker contexts with this\n# command; cua uses none, so it does nothing.\nexit 0\n";

/// The profile Colima starts the VM from. (The disk is a `start` flag:
/// Colima does not read `diskImage` from its profile.)
pub fn profile_yaml(arch: &str, cpus: u32, memory_gib: u32) -> String {
    format!(
        "# Written by cua (cua_vmm::managed) before every start.\n\
         cpu: {cpus}\n\
         memory: {memory_gib}\n\
         disk: 60\n\
         rootDisk: 20\n\
         arch: {arch}\n\
         runtime: docker\n\
         autoActivate: false\n\
         hostname: cua-runtime\n\
         vmType: vz\n\
         rosetta: false\n\
         binfmt: false\n\
         nestedVirtualization: false\n\
         sshConfig: false\n\
         forwardAgent: false\n\
         portForwarder: ssh\n\
         network:\n  address: false\n\
         kubernetes:\n  enabled: false\n\
         forceDiskImage: true\n\
         docker:\n  default-runtime: runsc\n  runtimes:\n    runsc:\n      path: /usr/local/bin/runsc\n      \
         runtimeArgs:\n        - --allow-suid\n"
    )
}

/// CPUs of the runtime VM: up to 4 (Spaces share them; a CPU limit is a
/// ceiling, so sharing them is safe).
fn vm_cpus() -> u32 {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    cpus.clamp(1, 4)
}

// ------------------------------------------------------------ memory
//
// Memory is not a ceiling the way CPUs are: every Space's container may use
// its whole memory limit, and when the Spaces' limits add up to more than
// the VM has, the guest kernel kills one of them (a gVisor sandbox) and the
// Space stops on its own. A container's /dev/shm is tmpfs inside its own
// memory limit, so the limit already counts it.
//
// So the VM boots at a large ceiling ([`ceiling_gib`]: 75% of this Mac's
// memory, at most 32 GiB) instead of a size picked for the Spaces it runs,
// and a Space starts as long as the running Spaces' limits and its own fit
// under that ceiling: no refusal and no VM restart before then. That costs
// little: Virtualization.framework backs guest memory only as the guest
// touches it (a 48 GiB VM idles at about 1.9 GB on the host, against 1.05 GB
// at 4 GiB; the difference is the guest's page tables for the memory map,
// about 1.8% of the ceiling), macOS compresses and swaps idle VM pages
// under pressure, and every page goes back to the Mac when the VM stops,
// which it does once no Linux Space runs. What it does not do is return
// pages while Spaces run (its balloon has no free page reporting): memory a
// stopped Space used stays with the VM until the VM stops.

/// The VM's memory besides the Spaces': the kernel, dockerd, containerd.
pub const OVERHEAD_MIB: u64 = 1024;
/// The smallest VM (the size it had before it was sized from the Mac).
pub const MIN_GIB: u32 = 4;
/// The largest VM.
pub const MAX_GIB: u32 = 32;

/// This Mac's memory in MiB.
pub fn host_memory_mib() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut bytes: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: `hw.memsize` is a u64; `bytes` and `len` describe it.
        let rc = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&mut bytes as *mut u64).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && bytes > 0).then_some(bytes >> 20)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The VM's memory: 75% of this Mac's memory, at most [`MAX_GIB`] and at
/// least [`MIN_GIB`] ([`MIN_GIB`] when it is unknown).
pub fn ceiling_gib(host_mib: Option<u64>) -> u32 {
    host_mib
        .map(|m| (m * 3 / 4 / 1024).min(u64::from(MAX_GIB)) as u32)
        .unwrap_or(MIN_GIB)
        .max(MIN_GIB)
}

/// Whether a Space asking for `new` MiB fits the VM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fit {
    /// It fits the running VM.
    Fits,
    /// Nothing runs in the VM: restart it with this many GiB.
    Restart(u32),
    /// It would overcommit the VM: the message says what to do.
    Refuse(String),
}

/// `mib` as GB for people: `2`, `1.5`.
fn gb(mib: u64) -> String {
    let tenths = (mib * 10).div_ceil(1024);
    if tenths.is_multiple_of(10) {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}

/// What Spaces may reserve in a VM of `gib`.
pub fn room(gib: u32) -> u64 {
    (u64::from(gib) << 10).saturating_sub(OVERHEAD_MIB)
}

/// Whether a Space asking for `new` MiB fits a VM of `vm_gib` where the
/// running (or paused) Spaces reserve `running` MiB, the VM's size being
/// `ceiling` GiB once booted by this version. A VM still at an older,
/// smaller size (once, after an update) is restarted at the ceiling when
/// nothing runs in it (nothing is lost), and never under running Spaces.
pub fn fit(new: u64, running: u64, vm_gib: u32, ceiling: u32) -> Fit {
    let most = room(ceiling);
    if new > most {
        return Fit::Refuse(format!(
            "This Linux Space asks for {} GB of memory, but Linux Spaces can use at most {} GB \
             on this Mac. Give it less memory.",
            gb(new),
            gb(most)
        ));
    }
    if running + new > most {
        return Fit::Refuse(format!(
            "Not enough memory for this Linux Space ({} GB): Linux Spaces can use at most {} GB \
             on this Mac and your running Linux Spaces reserve {} GB. Stop a Linux Space and try \
             again, or give this one less memory.",
            gb(new),
            gb(most),
            gb(running)
        ));
    }
    if vm_gib >= ceiling {
        return Fit::Fits;
    }
    if running == 0 {
        return Fit::Restart(ceiling);
    }
    if running + new <= room(vm_gib) {
        return Fit::Fits;
    }
    Fit::Refuse(format!(
        "Not enough memory for this Linux Space ({} GB): the Linux runtime still has its old \
         size of {} GB. Stop your running Linux Spaces once and it restarts with up to {} GB.",
        gb(new),
        vm_gib,
        gb(most)
    ))
}

/// The VM's memory in GiB, as the profile it was booted from says.
fn configured_gib() -> Option<u32> {
    memory_gib_of(&std::fs::read_to_string(vm_dir().join(PROFILE).join("colima.yaml")).ok()?)
}

/// `memory:` of a [`profile_yaml`].
fn memory_gib_of(profile: &str) -> Option<u32> {
    profile
        .lines()
        .find_map(|l| l.strip_prefix("memory:"))
        .and_then(|v| v.trim().parse().ok())
}

/// Spaces admitted in this process and starting now (MiB by name), not
/// running in the engine yet: creates that arrive together all count.
static PENDING: std::sync::Mutex<std::collections::BTreeMap<String, u64>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Engine work (an image pull, a commit) that a VM restart would cut short.
static BUSY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts as [`BUSY`] while it lives.
#[cfg(feature = "lume")]
struct Busy;

#[cfg(feature = "lume")]
impl Busy {
    fn new() -> Self {
        BUSY.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}

#[cfg(feature = "lume")]
impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// One fit check (and the restart it may need) at a time.
#[cfg(feature = "lume")]
static SIZING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A Space in [`PENDING`] until it is dropped.
#[cfg(feature = "lume")]
struct Pending(String);

#[cfg(feature = "lume")]
impl Pending {
    fn new(name: &str, mib: u64) -> Self {
        PENDING
            .lock()
            .expect("pending")
            .insert(name.to_string(), mib);
        Self(name.to_string())
    }
}

#[cfg(feature = "lume")]
impl Drop for Pending {
    fn drop(&mut self) {
        PENDING.lock().expect("pending").remove(&self.0);
    }
}

/// A `colima` command that sees only cua's own Colima, Lima and VM.
fn colima(tools: &Tools) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(&tools.colima);
    c.env_clear()
        .env("HOME", home())
        .env("COLIMA_HOME", vm_dir())
        .env("LIMA_HOME", vm_dir().join("_lima"))
        .env("DOCKER_CONFIG", home().join(".docker"))
        .env(
            "PATH",
            format!(
                "{}:{}:/usr/bin:/bin:/usr/sbin:/sbin",
                tools.bin.display(),
                tools.lima_bin.display()
            ),
        )
        .env("TMPDIR", std::env::temp_dir());
    for k in ["USER", "LOGNAME", "LANG"] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    c.stdin(std::process::Stdio::null());
    c
}

async fn run_colima(tools: &Tools, args: &[&str], secs: u64) -> Result<String> {
    let mut c = colima(tools);
    c.args(["--profile", PROFILE]).args(args);
    c.kill_on_drop(true);
    let out = tokio::time::timeout(std::time::Duration::from_secs(secs), c.output())
        .await
        .map_err(|_| {
            VmmError::other(format!("colima {} timed out after {secs}s", args.join(" ")))
        })??;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(VmmError::other(format!(
            "colima {}: {}",
            args.join(" "),
            last_error(&String::from_utf8_lossy(&out.stderr))
        )))
    }
}

/// Colima's last `fatal`/`error` message, else its last line.
fn last_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let pick = lines
        .iter()
        .rev()
        .find(|l| l.contains("level=fatal") || l.contains("level=error"))
        .or(lines.last())
        .copied()
        .unwrap_or("failed");
    match pick.find("msg=") {
        Some(i) => pick[i + 4..].trim().trim_matches('"').to_string(),
        None => pick.trim().to_string(),
    }
}

/// How far a boot is, from Colima's log line (it prints no progress).
pub fn boot_fraction(line: &str) -> Option<f64> {
    const STEPS: &[(&str, f64)] = &[
        ("starting colima", 0.05),
        ("creating and starting", 0.1),
        ("Starting VZ", 0.3),
        ("SSH Local Port", 0.5),
        ("Guest agent is running", 0.65),
        ("final requirement", 0.75),
        ("READY", 0.85),
        ("provisioning", 0.9),
        ("starting ...", 0.95),
        ("msg=done", 1.0),
    ];
    STEPS
        .iter()
        .rev()
        .find(|(needle, _)| line.contains(needle))
        .map(|(_, f)| *f)
}

/// One setup at a time in this process (a second create waits for it).
static SETUP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Words on the create while the runtime is being set up.
pub const SETTING_UP: &str = "Setting up the Linux runtime";

/// Sets up and boots the built-in runtime when it is not running; its
/// engine endpoint. The downloads and the boot are the running create's
/// progress.
#[cfg(feature = "lume")]
pub async fn ensure() -> Result<String> {
    if running() {
        return Ok(endpoint());
    }
    let _one = SETUP.lock().await;
    if running() {
        return Ok(endpoint());
    }
    if !supported() {
        return Err(VmmError::missing(
            "a container engine",
            "the built-in Linux runtime runs on macOS; install Docker",
        ));
    }
    check_path_length()?;
    let started = std::time::Instant::now();
    let fresh = !exists();
    let mut dl = Downloads::new(
        if installed_tools().is_none() {
            colima_pin().size + lima_pin().size
        } else {
            0
        } + if fresh { disk_pin().size } else { 0 },
    );
    let tools = ensure_tools(&mut dl).await?;
    let disk = if fresh {
        Some(ensure_disk(&mut dl).await?)
    } else {
        None
    };
    let (cpus, mem) = (vm_cpus(), ceiling_gib(host_memory_mib()));
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "aarch64"
    };
    let profile_dir = vm_dir().join(PROFILE);
    std::fs::create_dir_all(&profile_dir)?;
    std::fs::create_dir_all(home())?;
    std::fs::write(
        profile_dir.join("colima.yaml"),
        profile_yaml(arch, cpus, mem),
    )?;
    boot(&tools, disk.as_deref()).await?;
    wait_engine().await?;
    // The VM runs cua's disk (gVisor baked in), not one Colima chose.
    if let Err(e) = run_colima(
        &tools,
        &["ssh", "--", "test", "-x", "/usr/local/bin/runsc"],
        60,
    )
    .await
    {
        return Err(VmmError::other(format!(
            "the built-in Linux runtime booted without gVisor ({e}); remove it (Remove host \
             setup) and try again"
        )));
    }
    if let Some(d) = disk {
        // Lima unpacked it into the VM's own disk: the download and Lima's
        // copy of it are not needed again.
        let _ = std::fs::remove_file(d);
        let _ = std::fs::remove_dir_all(home().join("Library/Caches"));
    }
    let _ = std::fs::remove_file(stopped_names_file());
    tracing::info!(
        fresh,
        cpus,
        memory_gib = mem,
        secs = started.elapsed().as_secs_f64(),
        "built-in Linux runtime is up"
    );
    Ok(endpoint())
}

/// `colima start`, its log turned into boot progress. A VM left half-up
/// (a crash, a sleep) is stopped and started once more.
#[cfg(feature = "lume")]
async fn boot(tools: &Tools, disk: Option<&Path>) -> Result<()> {
    match start_once(tools, disk).await {
        Ok(()) => Ok(()),
        Err(first) if exists() => {
            tracing::warn!("built-in Linux runtime did not start ({first}); restarting it");
            let _ = run_colima(tools, &["stop", "--force"], 120).await;
            start_once(tools, disk).await
        }
        Err(e) => Err(e),
    }
}

#[cfg(feature = "lume")]
async fn start_once(tools: &Tools, disk: Option<&Path>) -> Result<()> {
    use crate::progress::{Phase, Progress, report};
    use tokio::io::AsyncBufReadExt as _;
    report(
        Progress::phase(Phase::Preparing)
            .detail(SETTING_UP)
            .fraction(0.0),
    );
    let mut c = colima(tools);
    c.args(["--profile", PROFILE, "start"]);
    if let Some(d) = disk {
        // The VM is created from cua's disk (not Colima's own download).
        c.arg("--disk-image").arg(d).arg("--force-disk-image");
    }
    c.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = c.spawn()?;
    let stderr = child.stderr.take().expect("piped");
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    let mut tail: Vec<String> = Vec::new();
    let mut at = 0.0;
    let read = async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(f) = boot_fraction(&line).filter(|f| *f > at) {
                at = f;
                report(
                    Progress::phase(Phase::Preparing)
                        .detail(SETTING_UP)
                        .fraction(f),
                );
            }
            tail.push(line);
            if tail.len() > 40 {
                tail.remove(0);
            }
        }
    };
    let status = tokio::time::timeout(std::time::Duration::from_secs(600), async {
        read.await;
        child.wait().await
    })
    .await
    .map_err(|_| VmmError::other("the built-in Linux runtime did not start within 10 minutes"))??;
    if status.success() {
        Ok(())
    } else {
        Err(VmmError::other(format!(
            "the built-in Linux runtime did not start: {}",
            last_error(&tail.join("\n"))
        )))
    }
}

/// Docker in the VM answers on the forwarded socket.
#[cfg(feature = "lume")]
async fn wait_engine() -> Result<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        if running() {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            return Err(VmmError::other(
                "the built-in Linux runtime started but its Docker engine does not answer",
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Stops the VM (its Spaces stop with it). Nothing to do when it is down.
pub async fn stop(names: &[String]) -> Result<()> {
    let Some(tools) = installed_tools() else {
        return Ok(());
    };
    if !exists() {
        return Ok(());
    }
    let _one = SETUP.lock().await;
    std::fs::write(stopped_names_file(), serde_json::to_vec(names)?)?;
    run_colima(&tools, &["stop"], 180).await.map(drop)
}

/// Deletes the built-in runtime: the VM, its disks and every download.
/// A Colima, Lima or Docker of the user's is elsewhere and stays.
pub async fn remove() -> Result<()> {
    let _one = SETUP.lock().await;
    if let Some(tools) = installed_tools()
        && exists()
        && let Err(e) = run_colima(&tools, &["delete", "--force", "--data"], 180).await
    {
        tracing::warn!("removing the built-in Linux runtime VM: {e}");
    }
    match std::fs::remove_dir_all(root()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Bytes of every download of one setup, as one transfer.
struct Downloads {
    total: u64,
    done: u64,
    meter: crate::progress::Meter,
}

impl Downloads {
    fn new(total: u64) -> Self {
        Self {
            total,
            done: 0,
            meter: crate::progress::Meter::new(),
        }
    }

    fn add(&mut self, n: u64) {
        use crate::progress::{Phase, Progress, report};
        self.done += n;
        if let Some(t) = self
            .meter
            .sample_at(std::time::Instant::now(), self.done, self.total)
        {
            report(
                Progress::phase(Phase::Preparing)
                    .detail(SETTING_UP)
                    .bytes(t)
                    .fraction(self.done as f64 / self.total.max(1) as f64),
            );
        }
    }
}

/// Installs the pinned Colima and Lima when they are not there yet.
#[cfg(feature = "lume")]
async fn ensure_tools(dl: &mut Downloads) -> Result<Tools> {
    if let Some(t) = installed_tools() {
        return Ok(t);
    }
    std::fs::create_dir_all(root())?;
    crate::disk::ensure_space(&root(), 200 << 20, "set up the built-in Linux runtime")?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(root())?;
    let s = staging.path().join("tools");
    let s = s.as_path();
    std::fs::create_dir_all(s)?;
    let colima_pin = colima_pin();
    let lima_pin = lima_pin();
    std::fs::create_dir_all(s.join("bin"))?;
    download(colima_pin, None, &s.join("bin/colima"), dl).await?;
    let lima_tgz = s.join("lima.tar.gz");
    download(lima_pin, None, &lima_tgz, dl).await?;
    std::fs::create_dir_all(s.join("lima"))?;
    host::run(
        "/usr/bin/tar",
        &[
            "-xzf",
            &lima_tgz.to_string_lossy(),
            "-C",
            &s.join("lima").to_string_lossy(),
            "bin/limactl",
            "bin/lima",
            "share/lima",
            "libexec/lima",
        ],
    )
    .await?;
    std::fs::remove_file(&lima_tgz)?;
    std::fs::write(s.join("bin/docker"), DOCKER_SHIM)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for f in ["bin/colima", "bin/docker"] {
            std::fs::set_permissions(s.join(f), std::fs::Permissions::from_mode(0o755))?;
        }
    }
    std::fs::write(
        s.join(VERIFIED),
        format!("colima {COLIMA_VERSION}\nlima {LIMA_VERSION}\n"),
    )?;
    let target = tools_dir();
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(s, &target)?;
    tracing::info!(dir = %target.display(), "installed the built-in Linux runtime's Colima and Lima");
    installed_tools().ok_or_else(|| VmmError::other("the built-in Linux runtime did not install"))
}

/// The runtime disk, downloaded from ghcr.io and checked against its pin.
#[cfg(feature = "lume")]
async fn ensure_disk(dl: &mut Downloads) -> Result<PathBuf> {
    let pin = disk_pin();
    let dir = root().join("disk");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("runtime-gvisor-{}.raw.gz", &pin.sha256[..12]));
    if path.is_file() && sha256_file(&path)? == pin.sha256 {
        dl.add(pin.size);
        return Ok(path);
    }
    // The disk unpacks to about 3.5 GB and grows with the images Spaces use.
    crate::disk::ensure_space(
        &dir,
        pin.size + (6 << 30),
        "set up the built-in Linux runtime",
    )?;
    let url = format!(
        "https://ghcr.io/v2/{DISK_REPOSITORY}/blobs/sha256:{}",
        pin.sha256
    );
    let token = ghcr_token().await?;
    download(Pin { url: "", ..pin }, Some((&url, &token)), &path, dl).await?;
    Ok(path)
}

/// A pull token for [`DISK_REPOSITORY`]: anonymous, or with the
/// `CUA_REGISTRY_USERNAME`/`PASSWORD` credentials container pulls use.
#[cfg(feature = "lume")]
async fn ghcr_token() -> Result<String> {
    let url =
        format!("https://ghcr.io/token?scope=repository:{DISK_REPOSITORY}:pull&service=ghcr.io");
    let mut req = http()?.get(url);
    if let (Ok(u), Ok(p)) = (
        std::env::var("CUA_REGISTRY_USERNAME"),
        std::env::var("CUA_REGISTRY_PASSWORD"),
    ) {
        req = req.basic_auth(u, Some(p));
    }
    let res = req
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| VmmError::other(format!("download the built-in Linux runtime: {e}")))?;
    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| VmmError::other(format!("download the built-in Linux runtime: {e}")))?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| VmmError::other("ghcr.io gave no pull token for the built-in Linux runtime"))
}

#[cfg(feature = "lume")]
fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| VmmError::other(format!("download the built-in Linux runtime: {e}")))
}

/// Streams a pinned file to `to` (via `<to>.partial`), checking its size
/// and SHA-256 before it is renamed into place.
#[cfg(feature = "lume")]
async fn download(
    pin: Pin,
    bearer: Option<(&str, &str)>,
    to: &Path,
    dl: &mut Downloads,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt as _;
    let (url, token) = match bearer {
        Some((u, t)) => (u, Some(t)),
        None => (pin.url, None),
    };
    let mut req = http()?.get(url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let mut res = req
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| VmmError::other(format!("download the built-in Linux runtime: {e}")))?;
    let partial = to.with_extension("partial");
    let mut file = tokio::fs::File::create(&partial).await?;
    let mut hash = Sha256::new();
    let mut got = 0u64;
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| VmmError::other(format!("download the built-in Linux runtime: {e}")))?
    {
        got += chunk.len() as u64;
        if got > pin.size {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(VmmError::other(
                "a built-in Linux runtime download is larger than its pin",
            ));
        }
        hash.update(&chunk);
        file.write_all(&chunk).await?;
        dl.add(chunk.len() as u64);
    }
    file.flush().await?;
    drop(file);
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != pin.sha256 || got != pin.size {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(VmmError::other(format!(
            "a built-in Linux runtime download did not match its checksum (got {digest}); try again"
        )));
    }
    tokio::fs::rename(&partial, to).await?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(all(feature = "container", feature = "lume"))]
pub use engine_impl::{Choice, ManagedContainers, choose, connect_if_up};

#[cfg(all(feature = "container", feature = "lume"))]
mod engine_impl {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::container::{ContainerConfig, ContainerRuntime};
    use crate::exec::GuestExec;
    use crate::runtime::Runtime;
    use crate::types::{
        BackendKind, CheckpointInfo, Endpoints, ImageSource, Instance, InstanceSummary, StartSpec,
        Status,
    };

    /// The engine Linux Spaces use here.
    pub enum Choice {
        /// This Mac's own engine (Docker Desktop, Colima, OrbStack, ...).
        System(Box<ContainerRuntime>),
        /// The built-in runtime (booted on demand).
        Builtin(Arc<ManagedContainers>),
    }

    /// Picks the engine for `source` without booting anything.
    pub async fn choose(cfg: ContainerConfig, source: LinuxSource) -> Result<Choice> {
        if cfg.endpoint.is_some() {
            return ContainerRuntime::connect(cfg)
                .await
                .map(|rt| Choice::System(Box::new(rt)));
        }
        let builtin = |cfg| Ok(Choice::Builtin(Arc::new(ManagedContainers::new(cfg))));
        match source {
            LinuxSource::Builtin if supported() => builtin(cfg),
            LinuxSource::Builtin => Err(VmmError::missing(
                "a container engine",
                "the built-in Linux runtime runs on macOS; set runtime.linux to auto or system",
            )),
            LinuxSource::System => ContainerRuntime::connect(cfg)
                .await
                .map(|rt| Choice::System(Box::new(rt))),
            // Once the built-in runtime exists it keeps its Spaces.
            LinuxSource::Auto if supported() && exists() => builtin(cfg),
            LinuxSource::Auto => match ContainerRuntime::connect(cfg.clone()).await {
                Ok(rt) => Ok(Choice::System(Box::new(rt))),
                Err(_) if supported() => builtin(cfg),
                Err(e) => Err(e),
            },
        }
    }

    /// The engine Linux Spaces use, when it answers now (the built-in
    /// runtime only while its VM runs). Never boots or downloads anything:
    /// for views of what is there (disk use, downloaded images).
    pub async fn connect_if_up(cfg: ContainerConfig) -> Option<ContainerRuntime> {
        choose(cfg, LinuxSource::current())
            .await
            .ok()?
            .connect_if_up()
            .await
    }

    impl Choice {
        pub async fn connect_if_up(self) -> Option<ContainerRuntime> {
            match self {
                Self::System(rt) => Some(*rt),
                Self::Builtin(rt) if running() => ContainerRuntime::connect(ContainerConfig {
                    endpoint: Some(endpoint()),
                    ..rt.cfg.clone()
                })
                .await
                .ok(),
                Self::Builtin(_) => None,
            }
        }
    }

    /// The built-in runtime as a container backend: boots the VM for
    /// anything that needs the engine, answers status and list from what
    /// it knew while the VM is down, and stops the VM once no Linux Space
    /// runs any more.
    pub struct ManagedContainers {
        cfg: ContainerConfig,
        engine: tokio::sync::Mutex<Option<Arc<ContainerRuntime>>>,
    }

    impl ManagedContainers {
        pub fn new(cfg: ContainerConfig) -> Self {
            Self {
                cfg,
                engine: tokio::sync::Mutex::new(None),
            }
        }

        /// The engine, booting the VM (and setting it up the first time).
        pub async fn engine(&self) -> Result<Arc<ContainerRuntime>> {
            let uri = ensure().await?;
            self.connect(uri).await
        }

        /// The engine when the VM is up; never boots it.
        pub async fn engine_if_running(&self) -> Option<Arc<ContainerRuntime>> {
            if !running() {
                return None;
            }
            self.connect(endpoint()).await.ok()
        }

        async fn connect(&self, uri: String) -> Result<Arc<ContainerRuntime>> {
            let mut slot = self.engine.lock().await;
            if let Some(rt) = slot.as_ref() {
                return Ok(rt.clone());
            }
            let rt = Arc::new(
                ContainerRuntime::connect(ContainerConfig {
                    endpoint: Some(uri),
                    ..self.cfg.clone()
                })
                .await?,
            );
            *slot = Some(rt.clone());
            Ok(rt)
        }

        /// Makes room for the Space `name` before it starts, asking for
        /// `new` MiB (`None`: what its container already has): boots the VM
        /// sized for every Space it holds, restarts an idle VM that is too
        /// small, or refuses a Space that would overcommit it (the guest
        /// kernel would kill a Space later). The returned guard counts the
        /// Space for other fit checks until its start is over.
        async fn make_room(
            &self,
            name: &str,
            new: Option<u64>,
        ) -> Result<(Arc<ContainerRuntime>, Pending)> {
            let _one = SIZING.lock().await;
            loop {
                let rt = self.engine().await?;
                let rows = rt.memory_reservations().await?;
                let me = rows.iter().find(|(n, _, _)| n == name);
                let new = new.or(me.map(|(_, _, m)| *m)).unwrap_or(2048);
                // Running or paused: it holds its memory already.
                if me.is_some_and(|(_, s, _)| *s != Status::Stopped) {
                    return Ok((rt, Pending::new(name, new)));
                }
                let up = |n: &str| rows.iter().any(|(r, s, _)| r == n && *s != Status::Stopped);
                // Paused containers keep their memory in the VM: they count
                // as running. So do Spaces admitted here and still starting.
                let starting: u64 = PENDING
                    .lock()
                    .expect("pending")
                    .iter()
                    .filter(|(n, _)| n.as_str() != name && !up(n))
                    .map(|(_, m)| *m)
                    .sum();
                let running: u64 = rows
                    .iter()
                    .filter(|(n, s, _)| n != name && *s != Status::Stopped)
                    .map(|(_, _, m)| *m)
                    .sum::<u64>()
                    + starting;
                let vm = configured_gib().unwrap_or(MIN_GIB);
                match fit(new, running, vm, ceiling_gib(host_memory_mib())) {
                    Fit::Fits => return Ok((rt, Pending::new(name, new))),
                    Fit::Refuse(why) => {
                        tracing::warn!(space = %name, vm_gib = vm, running_mib = running, "{why}");
                        return Err(VmmError::other(why));
                    }
                    // An image pull or a commit is under way: restart once it
                    // is over.
                    Fit::Restart(_) if BUSY.load(std::sync::atomic::Ordering::SeqCst) > 0 => {
                        drop(rt);
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                    Fit::Restart(gib) => {
                        // A VM an older version booted at its fixed size, and
                        // nothing runs in it, so nothing is lost: boot it again
                        // at the ceiling (once per Mac).
                        tracing::info!(
                            space = %name,
                            from_gib = vm,
                            to_gib = gib,
                            "restarting the built-in Linux runtime with more memory"
                        );
                        // Its Spaces (not their sidecars) read as stopped
                        // while it is down.
                        let names: Vec<String> =
                            rt.list().await?.into_iter().map(|s| s.name).collect();
                        drop(rt);
                        stop(&names).await?;
                        *self.engine.lock().await = None;
                    }
                }
            }
        }

        /// Stops the VM when none of its containers runs (or is paused).
        pub async fn stop_if_idle(&self) {
            let Some(rt) = self.engine_if_running().await else {
                return;
            };
            let Ok(list) = rt.list().await else {
                return;
            };
            if list.iter().any(|s| s.status != Status::Stopped) {
                return;
            }
            let names: Vec<String> = list.into_iter().map(|s| s.name).collect();
            match stop(&names).await {
                Ok(()) => tracing::info!("stopped the built-in Linux runtime: no Linux Space runs"),
                Err(e) => tracing::warn!("stopping the idle built-in Linux runtime: {e}"),
            }
        }
    }

    #[async_trait]
    impl Runtime for ManagedContainers {
        fn kind(&self) -> BackendKind {
            BackendKind::Container
        }

        async fn ensure_base(
            &self,
            image: &ImageSource,
            base_name: &str,
        ) -> Result<CheckpointInfo> {
            let _busy = Busy::new();
            self.engine().await?.ensure_base(image, base_name).await
        }

        async fn start(&self, spec: &StartSpec) -> Result<Instance> {
            let need = spec.memory_mb
                + spec.sidecars.len() as u64
                    * (crate::container::SIDECAR_MEMORY_BYTES as u64 >> 20);
            let (rt, _room) = self.make_room(&spec.name, Some(need)).await?;
            rt.start(spec).await
        }

        async fn stop(&self, name: &str) -> Result<()> {
            let Some(rt) = self.engine_if_running().await else {
                return if stopped_names().iter().any(|n| n == name) {
                    Ok(())
                } else {
                    Err(VmmError::NotFound(name.into()))
                };
            };
            rt.stop(name).await?;
            self.stop_if_idle().await;
            Ok(())
        }

        async fn suspend(&self, name: &str) -> Result<()> {
            self.engine().await?.suspend(name).await
        }

        async fn resume(&self, name: &str) -> Result<Instance> {
            let (rt, _room) = self.make_room(name, None).await?;
            rt.resume(name).await
        }

        async fn fork(&self, source: &str, new_name: &str) -> Result<()> {
            let _busy = Busy::new();
            self.engine().await?.fork(source, new_name).await
        }

        async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
            let _busy = Busy::new();
            self.engine().await?.checkpoint(name, checkpoint).await
        }

        async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
            self.engine().await?.delete_checkpoint(checkpoint).await
        }

        async fn list(&self) -> Result<Vec<InstanceSummary>> {
            match self.engine_if_running().await {
                Some(rt) => rt.list().await,
                None => Ok(stopped_names()
                    .into_iter()
                    .map(|name| InstanceSummary {
                        name,
                        backend: BackendKind::Container,
                        status: Status::Stopped,
                    })
                    .collect()),
            }
        }

        async fn status(&self, name: &str) -> Result<Status> {
            match self.engine_if_running().await {
                Some(rt) => rt.status(name).await,
                None if stopped_names().iter().any(|n| n == name) => Ok(Status::Stopped),
                None => Err(VmmError::NotFound(name.into())),
            }
        }

        async fn delete(&self, name: &str) -> Result<()> {
            if !running() && !stopped_names().iter().any(|n| n == name) {
                return Err(VmmError::NotFound(name.into()));
            }
            self.engine().await?.delete(name).await?;
            self.stop_if_idle().await;
            Ok(())
        }

        async fn endpoints(&self, name: &str) -> Result<Endpoints> {
            match self.engine_if_running().await {
                Some(rt) => rt.endpoints(name).await,
                None => Err(VmmError::other(format!(
                    "{name} is stopped (the built-in Linux runtime is not running)"
                ))),
            }
        }

        async fn guest_tcp_listening(&self, name: &str, guest_port: u16) -> Result<Option<bool>> {
            match self.engine_if_running().await {
                Some(rt) => rt.guest_tcp_listening(name, guest_port).await,
                None => Ok(Some(false)),
            }
        }

        async fn guest_exec(&self, name: &str) -> Result<Option<Arc<dyn GuestExec>>> {
            self.engine().await?.guest_exec(name).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_parses_and_defaults_to_auto() {
        assert_eq!(LinuxSource::parse("Built-in"), Some(LinuxSource::Builtin));
        assert_eq!(LinuxSource::parse("system"), Some(LinuxSource::System));
        assert_eq!(LinuxSource::parse("docker"), None);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        assert_eq!(LinuxSource::from_config(&p), LinuxSource::Auto);
        std::fs::write(&p, "[runtime]\nlume = \"system\"\nlinux = \"builtin\"\n").unwrap();
        assert_eq!(LinuxSource::from_config(&p), LinuxSource::Builtin);
        assert!(!LinuxSource::System.allows_builtin());
        assert_eq!(
            LinuxSource::Auto.allows_builtin(),
            cfg!(target_os = "macos")
        );
    }

    /// The pins here and the image's versions.env name the same releases
    /// (the image is built from the file; the SDK downloads by these).
    #[test]
    fn the_pins_match_the_image_build() {
        let env = include_str!("../../../../images/runtime-gvisor/versions.env");
        let get = |k: &str| {
            env.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")))
                .unwrap_or_else(|| panic!("{k} in versions.env"))
                .to_string()
        };
        assert_eq!(get("COLIMA_VERSION"), COLIMA_VERSION);
        assert_eq!(get("LIMA_VERSION"), LIMA_VERSION);
        assert_eq!(get("GVISOR_VERSION"), GVISOR_VERSION);
        assert!(DISK_TAG.starts_with(&get("RUNTIME_VERSION")));
        for p in [colima_pin(), lima_pin(), disk_pin()] {
            assert_eq!(p.sha256.len(), 64);
            assert!(p.size > 1 << 20);
        }
        assert!(colima_pin().url.contains(&format!("v{COLIMA_VERSION}/")));
        assert!(
            lima_pin()
                .url
                .contains(&format!("v{LIMA_VERSION}/lima-{LIMA_VERSION}-Darwin"))
        );
        // A few hundred MB in all.
        assert!(first_use_download() < 600 << 20);
    }

    #[test]
    fn the_profile_runs_gvisor_by_default_and_shares_nothing() {
        let y = profile_yaml("aarch64", 4, 4);
        assert!(y.contains("default-runtime: runsc"));
        assert!(y.contains("path: /usr/local/bin/runsc"));
        // Setuid (`sudo`, `fusermount3`) works for the Space user (#4667).
        assert!(
            y.contains("      runtimeArgs:\n        - --allow-suid\n"),
            "{y}"
        );
        assert!(y.contains("vmType: vz"));
        assert!(y.contains("autoActivate: false"));
        assert!(y.contains("forceDiskImage: true"));
        // No `mounts:` key: Colima then mounts nothing (an empty list would
        // mount the home directory).
        assert!(!y.contains("mounts"));
    }

    /// The VM boots at 75% of the Mac, at most 32 GiB, at least 4 GiB:
    /// the host backs it only as the guest touches it.
    #[test]
    fn the_vm_boots_at_three_quarters_of_the_mac_up_to_32_gib() {
        assert_eq!(ceiling_gib(Some(8 << 10)), 6);
        assert_eq!(ceiling_gib(Some(16 << 10)), 12);
        assert_eq!(ceiling_gib(Some(32 << 10)), 24);
        assert_eq!(ceiling_gib(Some(96 << 10)), MAX_GIB);
        assert_eq!(ceiling_gib(Some(192 << 10)), MAX_GIB);
        assert_eq!(ceiling_gib(Some(4 << 10)), MIN_GIB);
        assert_eq!(ceiling_gib(None), MIN_GIB);
        // The running VM's size is read back from the profile it booted from.
        assert_eq!(memory_gib_of(&profile_yaml("aarch64", 4, 12)), Some(12));
        #[cfg(target_os = "macos")]
        assert!(host_memory_mib().is_some_and(|m| m >= 4096));
    }

    #[test]
    fn spaces_start_until_the_mac_is_full_and_never_overcommit() {
        // 16 GB Mac: a 12 GiB VM, room for 11 GB of Spaces. Three, four
        // and five 2 GB Spaces start with no restart and no refusal.
        assert_eq!(fit(2048, 2 * 2048, 12, 12), Fit::Fits);
        assert_eq!(fit(2048, 4 * 2048, 12, 12), Fit::Fits);
        // A sixth would overcommit it: refused, with what to do.
        let Fit::Refuse(why) = fit(2048, 5 * 2048, 12, 12) else {
            panic!("a sixth 2 GB Space must be refused on a 16 GB Mac");
        };
        assert_eq!(
            why,
            "Not enough memory for this Linux Space (2 GB): Linux Spaces can use at most 11 GB \
             on this Mac and your running Linux Spaces reserve 10 GB. Stop a Linux Space and try \
             again, or give this one less memory."
        );
        // 96 GB Mac: 32 GiB, fifteen 2 GB Spaces.
        assert_eq!(fit(2048, 14 * 2048, 32, 32), Fit::Fits);
        assert!(matches!(fit(2048, 15 * 2048, 32, 32), Fit::Refuse(_)));
        // One Space bigger than the Mac allows.
        let Fit::Refuse(why) = fit(16 << 10, 0, 12, 12) else {
            panic!("a 16 GB Space cannot run on a 16 GB Mac");
        };
        assert_eq!(
            why,
            "This Linux Space asks for 16 GB of memory, but Linux Spaces can use at most 11 GB \
             on this Mac. Give it less memory."
        );
        assert_eq!(gb(1536), "1.5");
        assert_eq!(gb(3 * 1024), "3");
    }

    /// A VM an older version booted at its fixed 4 GiB: restarted at the
    /// ceiling once nothing runs, never under running Spaces.
    #[test]
    fn an_old_4_gib_vm_grows_once_nothing_runs_in_it() {
        assert_eq!(fit(2048, 0, 4, 12), Fit::Restart(12));
        // Spaces run: one that fits the old VM starts...
        assert_eq!(fit(1024, 2048, 4, 12), Fit::Fits);
        // ...one that does not is refused (it used to start and be killed).
        let Fit::Refuse(why) = fit(2048, 2 * 2048, 4, 12) else {
            panic!("never restart the VM under running Spaces");
        };
        assert_eq!(
            why,
            "Not enough memory for this Linux Space (2 GB): the Linux runtime still has its old \
             size of 4 GB. Stop your running Linux Spaces once and it restarts with up to 11 GB."
        );
        // At the ceiling, nothing restarts.
        assert_eq!(fit(2048, 0, 12, 12), Fit::Fits);
    }

    #[test]
    fn boot_progress_follows_colima_s_log() {
        assert_eq!(
            boot_fraction(r#"level=info msg="[hostagent] Starting VZ (hint: ...)""#),
            Some(0.3)
        );
        assert_eq!(boot_fraction(r#"level=info msg=done"#), Some(1.0));
        assert_eq!(boot_fraction("unrelated"), None);
        assert_eq!(
            last_error("a\ntime=x level=fatal msg=\"error starting vm: boom\"\n"),
            "error starting vm: boom"
        );
    }

    #[test]
    fn its_socket_is_recognised_and_not_a_user_engine() {
        let ep = endpoint();
        assert!(is_endpoint(&ep));
        assert!(!is_endpoint("unix:///Users/me/.colima/default/docker.sock"));
        // engine::classify must not mistake it for the user's Colima.
        #[cfg(feature = "container")]
        assert!(!matches!(
            crate::container::engine::classify(&ep),
            crate::container::engine::EngineKind::Colima { .. }
        ));
    }

    /// Opt-in (network, boots a VM, about 480 MB): sets up the built-in
    /// runtime in a temporary `CUA_HOME`, runs a container under runsc,
    /// stops and removes it.
    /// `CUA_HOME=<tmp> CUA_RUNTIME_LINUX=builtin cargo test -p cua-vmm --lib managed -- --ignored`
    #[cfg(all(feature = "container", feature = "lume", target_os = "macos"))]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "downloads and boots the built-in Linux runtime (network, a VM)"]
    async fn sets_up_boots_and_removes_the_builtin_runtime() {
        let home = std::env::var("CUA_HOME").expect("set CUA_HOME to a temporary directory");
        assert!(
            !home.is_empty() && !home.ends_with("/.cua"),
            "never the real cua home"
        );
        let uri = ensure().await.unwrap();
        assert!(running());
        let rt = crate::container::ContainerRuntime::connect(crate::container::ContainerConfig {
            endpoint: Some(uri),
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(rt.runtimes().await.unwrap().iter().any(|r| r == "runsc"));
        let info = rt.docker().info().await.unwrap();
        assert_eq!(info.default_runtime.as_deref(), Some("runsc"));
        stop(&[]).await.unwrap();
        assert!(!running());
        remove().await.unwrap();
        assert!(!root().exists());
    }
}
