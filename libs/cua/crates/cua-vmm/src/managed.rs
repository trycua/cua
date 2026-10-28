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
//! * **Host files: job members' only.** The VM shares (virtiofs) the
//!   directories cua's job runner keeps a job member's files in
//!   ([`JOB_SHARES`] under the cua home: its work directory, its run's
//!   data, the caches and `uv`), at the same paths, so a job member's
//!   container binds them like on any Linux host. Nothing else of the Mac
//!   is in the VM. A VM that booted before it shared them (an older cua)
//!   gets them at its next start: it is restarted at once when no Linux
//!   Space runs, else a job member is refused until it stops by itself.
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

/// The directories under the cua home the VM shares: where cua's job
/// runner keeps job members' files (`cua_cluster::jobdir`: attempt and work
/// directories, runs' data, the caches, `uv` for the guest).
pub const JOB_SHARES: [&str; 4] = ["jobs", "job-data", "job-cache", "job-tools"];

/// The host directories the VM shares ([`JOB_SHARES`] under the cua home).
pub fn job_shares() -> Vec<PathBuf> {
    JOB_SHARES
        .iter()
        .map(|n| host::cua_home().join(n))
        .collect()
}

/// Whether the host directory `path` is in the VM, at the same path.
pub fn is_shared(path: &Path) -> bool {
    job_shares().iter().any(|s| path.starts_with(s))
}

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
        Self::of(crate::kinds::source(crate::kinds::RuntimeKind::Container))
    }

    /// `[runtime] container` (or the older `linux`) in `path` (auto when
    /// absent or unreadable).
    pub fn from_config(path: &Path) -> Self {
        Self::of(crate::kinds::source_in(
            crate::kinds::RuntimeKind::Container,
            path,
        ))
    }

    fn of(s: crate::kinds::KindSource) -> Self {
        match s {
            crate::kinds::KindSource::Auto => Self::Auto,
            crate::kinds::KindSource::Builtin => Self::Builtin,
            crate::kinds::KindSource::System => Self::System,
        }
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

/// Whether the VM as it runs now shares every job directory (Lima's
/// configuration of the running instance names each).
pub fn shares_job_dirs() -> bool {
    let Ok(y) = std::fs::read_to_string(instance_dir().join("lima.yaml")) else {
        return false;
    };
    job_shares()
        .iter()
        .all(|s| y.contains(&s.display().to_string()))
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

/// What the idle check finds in the runtime VM ([`idle_verdict`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleVerdict {
    /// Nothing needs the VM: it may stop.
    Stop,
    /// A warm-pool shape the host keeps warm on purpose holds it up.
    KeptWarm,
    /// This many Linux Spaces (or jobs) run in it.
    InUse(usize),
}

/// Whether the runtime VM may stop. `spaces` are its containers as
/// `(name, active)` (running or paused), `members` its warm-pool members as
/// `(name, keep, running)`. Warm-pool members are not Spaces in use (an
/// idle host stops the VM anyway; the pool refills on the next create)
/// unless their shape is kept warm on purpose (`pool.warm`).
pub fn idle_verdict(spaces: &[(&str, bool)], members: &[(&str, bool, bool)]) -> IdleVerdict {
    if members.iter().any(|(_, keep, running)| *keep && *running) {
        return IdleVerdict::KeptWarm;
    }
    let in_use = spaces
        .iter()
        .filter(|(name, active)| *active && !members.iter().any(|(m, _, _)| m == name))
        .count();
    if in_use > 0 {
        IdleVerdict::InUse(in_use)
    } else {
        IdleVerdict::Stop
    }
}

/// What a request to stop the runtime VM now did ([`ManagedContainers::stop_now`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeStop {
    /// It was not running.
    NotRunning,
    /// It stopped.
    Stopped,
    /// It keeps running for this many Linux Spaces.
    KeptForSpaces(usize),
    /// It keeps running for warm Spaces the host keeps ready.
    KeptWarm,
    /// An operation on the engine is in flight; nothing was stopped.
    Busy,
    /// Stopping it failed; it may still run.
    Failed(String),
}

impl RuntimeStop {
    /// What `cua daemon stop` tells the user when the VM keeps running:
    /// one sentence saying why and when it stops. `None` when it stopped
    /// or was not running (or a stop failed: that is logged, not a state
    /// the user acts on).
    pub fn keeps_running_sentence(&self) -> Option<String> {
        match self {
            Self::KeptForSpaces(n) if *n > 0 => Some(format!(
                "the Linux runtime keeps running for {n} Space{}; it stops when {}",
                if *n == 1 { "" } else { "s" },
                if *n == 1 { "it does" } else { "they do" }
            )),
            Self::KeptWarm => Some(
                "the Linux runtime keeps running for the Spaces this host keeps warm; it stops \
                 when that setting is removed"
                    .into(),
            ),
            _ => None,
        }
    }
}

const SHUTDOWN_NOTE: &str = "shutdown-note";

/// Leaves what a stopping daemon did to the runtime VM for the command that
/// asked it to stop ([`take_shutdown_note`]): the sentence when the VM keeps
/// running, nothing otherwise.
pub fn record_shutdown(stop: &RuntimeStop) {
    record_shutdown_in(&root(), stop);
}

fn record_shutdown_in(dir: &Path, stop: &RuntimeStop) {
    let file = dir.join(SHUTDOWN_NOTE);
    match stop.keeps_running_sentence() {
        Some(s) => {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(file, s);
        }
        None => {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// The note [`record_shutdown`] left, removed as it is read.
pub fn take_shutdown_note() -> Option<String> {
    take_shutdown_note_in(&root())
}

fn take_shutdown_note_in(dir: &Path) -> Option<String> {
    let file = dir.join(SHUTDOWN_NOTE);
    let s = std::fs::read_to_string(&file).ok()?;
    let _ = std::fs::remove_file(file);
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
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
    profile_yaml_sharing(arch, cpus, memory_gib, &[])
}

/// [`profile_yaml`] sharing `shares` (host directories, writable, over
/// virtiofs, at the same paths in the VM). None: no `mounts:` key at all
/// (an empty list would make Colima mount the home directory).
pub fn profile_yaml_sharing(arch: &str, cpus: u32, memory_gib: u32, shares: &[PathBuf]) -> String {
    let mut y = profile_base(arch, cpus, memory_gib);
    if !shares.is_empty() {
        y.push_str("mountType: virtiofs\nmounts:\n");
        for s in shares {
            // Lima's YAML takes a JSON string (paths with spaces, quotes).
            let loc = serde_json::to_string(&s.display().to_string()).unwrap_or_default();
            y.push_str(&format!("  - location: {loc}\n    writable: true\n"));
        }
    }
    y
}

fn profile_base(arch: &str, cpus: u32, memory_gib: u32) -> String {
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
         runtimeArgs:\n        - --allow-suid\n    \
         cua-runsc:\n      path: /usr/local/lib/cua/cua-runsc\n      runtimeArgs:\n        - --allow-suid\n"
    )
}

/// Environment overrides of the runtime VM's size (CPUs, GiB of memory).
pub const CPUS_ENV: &str = "CUA_RUNTIME_LINUX_CPUS";
pub const MEMORY_ENV: &str = "CUA_RUNTIME_LINUX_MEMORY_GIB";

/// The default CPUs: up to 4 (Spaces share them; a CPU limit is a ceiling,
/// so sharing them is safe).
const DEFAULT_CPUS: u32 = 4;
/// The memory when the Mac's is unknown.
const DEFAULT_MEMORY_GIB: u32 = MIN_GIB;
/// The smallest VM (the size it had before it was sized from the Mac).
pub const MIN_GIB: u32 = 4;
/// The largest VM a default sizing gives (a setting may give more).
pub const MAX_GIB: u32 = 32;

/// The default memory: 75% of this Mac's, at most [`MAX_GIB`] and at
/// least [`MIN_GIB`] ([`MIN_GIB`] when it is unknown). Memory is a large
/// ceiling rather than a size picked for the Spaces: every Space's
/// container may use its whole limit, and Virtualization.framework backs
/// guest memory only as the guest touches it (a 48 GiB VM idles at about
/// 1.9 GB on the host), so a large VM costs little and the guest kernel
/// does not kill a Space when the Spaces' limits add up to more than 4 GiB.
pub fn ceiling_gib(host_gib: Option<u32>) -> u32 {
    host_gib
        .map(|m| (m * 3 / 4).min(MAX_GIB))
        .unwrap_or(MIN_GIB)
        .max(MIN_GIB)
}
/// Memory the Mac always keeps for itself, whatever the setting says.
const HOST_RESERVE_GIB: u32 = 4;

/// What the runtime VM gets: the environment (`CUA_RUNTIME_LINUX_CPUS`,
/// `CUA_RUNTIME_LINUX_MEMORY_GIB`), else `[runtime] linux_cpus` and
/// `linux_memory_gib` in `~/.cua/config.toml`, else up to 4 CPUs and
/// [`ceiling_gib`] of memory. A machine that provides Spaces to others sets these to most of
/// itself (`cua host setup --profile spare`); every Space then shares that
/// VM. Read before every boot: a change applies when the VM next starts.
fn vm_size() -> (u32, u32) {
    let host_cpus = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(DEFAULT_CPUS);
    let config = host::cua_home().join("config.toml");
    let (cfg_cpus, cfg_mem) = size_from_config(&config);
    let cpus = env_u32(CPUS_ENV).or(cfg_cpus);
    let mem = env_u32(MEMORY_ENV).or(cfg_mem);
    clamp_size(cpus, mem, host_cpus, host_memory_gib())
}

/// The VM size for the requested `cpus` and `mem` (GiB), defaults filled
/// in and kept within the host: at most every CPU, and memory leaving
/// [`HOST_RESERVE_GIB`] to the host.
fn clamp_size(
    cpus: Option<u32>,
    mem: Option<u32>,
    host_cpus: u32,
    host_mem: Option<u32>,
) -> (u32, u32) {
    let host_cpus = host_cpus.max(1);
    let cpus = cpus
        .unwrap_or(DEFAULT_CPUS.min(host_cpus))
        .clamp(1, host_cpus);
    let max_mem = host_mem
        .map(|m| m.saturating_sub(HOST_RESERVE_GIB).max(2))
        .unwrap_or(u32::MAX);
    let mem = mem
        .unwrap_or_else(|| ceiling_gib(host_mem))
        .clamp(2, max_mem.max(2));
    (cpus, mem)
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|v| *v > 0)
}

/// `[runtime] linux_cpus` and `linux_memory_gib` in `path`.
fn size_from_config(path: &Path) -> (Option<u32>, Option<u32>) {
    let Some(doc) = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
    else {
        return (None, None);
    };
    let get = |k: &str| {
        let item = doc.get("runtime")?.get(k)?;
        item.as_integer()
            .and_then(|v| u32::try_from(v).ok())
            .or_else(|| item.as_str()?.trim().parse().ok())
            .filter(|v| *v > 0)
    };
    (get("linux_cpus"), get("linux_memory_gib"))
}

/// The size a machine dedicated to Spaces gives the runtime VM: every CPU
/// and three quarters of its memory (the rest stays with macOS).
fn dedicated_size(host_cpus: u32, host_mem: Option<u32>) -> (u32, u32) {
    let mem = host_mem.map(|m| m * 3 / 4).unwrap_or(DEFAULT_MEMORY_GIB);
    clamp_size(Some(host_cpus), Some(mem), host_cpus, host_mem)
}

/// Gives the runtime VM most of this machine, for a spare machine that
/// provides Spaces (`cua host setup --profile spare`): writes
/// `[runtime] linux_cpus` and `linux_memory_gib` to `config` unless the
/// owner already set either. Returns the size written, if any. It applies
/// when the VM next starts.
pub fn dedicate_to_spaces(config: &Path) -> Result<Option<(u32, u32)>> {
    if size_from_config(config) != (None, None) {
        return Ok(None);
    }
    let host_cpus = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(DEFAULT_CPUS);
    let (cpus, mem) = dedicated_size(host_cpus, host_memory_gib());
    let mut doc = std::fs::read_to_string(config)
        .ok()
        .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
        .unwrap_or_default();
    if !doc.contains_table("runtime") {
        doc["runtime"] = toml_edit::table();
    }
    doc["runtime"]["linux_cpus"] = toml_edit::value(i64::from(cpus));
    doc["runtime"]["linux_memory_gib"] = toml_edit::value(i64::from(mem));
    if let Some(dir) = config.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(config, doc.to_string())?;
    Ok(Some((cpus, mem)))
}

/// This Mac's memory in GiB (`hw.memsize`).
fn host_memory_gib() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let mut size: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let name = c"hw.memsize";
        // SAFETY: sysctlbyname writes at most `len` bytes into `size`.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut size as *mut u64).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 {
            return u32::try_from(size >> 30).ok();
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
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
        trim_running_vm_once().await;
        return Ok(endpoint());
    }
    let _one = SETUP.lock().await;
    if running() {
        trim_running_vm_once().await;
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
    let (cpus, mem) = vm_size();
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "aarch64"
    };
    let profile_dir = vm_dir().join(PROFILE);
    std::fs::create_dir_all(&profile_dir)?;
    std::fs::create_dir_all(home())?;
    // Job members' directories exist before the VM shares them.
    let shares = job_shares();
    for d in &shares {
        if !d.is_dir() {
            std::fs::create_dir_all(d)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
            }
        }
    }
    std::fs::write(
        profile_dir.join("colima.yaml"),
        profile_yaml_sharing(arch, cpus, mem, &shares),
    )?;
    boot(&tools, disk.as_deref()).await?;
    wait_engine().await?;
    // The VM runs cua's disk (gVisor baked in), not one Colima chose.
    // The same call keeps pointer authentication off in its kernel from
    // the next boot on ([`AFTER_BOOT`]).
    if let Err(e) = run_colima(&tools, &["ssh", "--", "sudo", "sh", "-c", AFTER_BOOT], 60).await {
        return Err(VmmError::other(format!(
            "the built-in Linux runtime booted without gVisor ({e}); remove it (Remove host \
             setup) and try again"
        )));
    }
    install_vfs_trim(&tools).await;
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

/// Run in the VM after every boot (as root): fails without gVisor, and on
/// arm64 boots the VM's kernel with `arm64.nopauth` from the next boot on.
///
/// gVisor does not carry a process's pointer-authentication keys across a
/// checkpoint restore yet (gvisor#13542, fix pending upstream), so with PAC
/// on, every PAC-built program (all of Ubuntu's) in a restored Space dies
/// with SIGILL. With it off, PAC's instructions are no-ops for every
/// program in this VM: the Spaces' (as on x86, which has no PAC) and the
/// VM's own services (Docker, containerd and gVisor's sentry are Go, which
/// does not use PAC). gVisor remains the isolation boundary. It applies at
/// the VM's next boot: the VM stops when no Linux Space runs and boots
/// again on the next one; until then Spaces start cold and checkpoints are
/// filesystem-only.
pub const AFTER_BOOT: &str = r#"test -x /usr/local/bin/runsc || exit 3
if [ "$(uname -m)" = aarch64 ] && [ ! -e /etc/default/grub.d/99-cua-nopauth.cfg ]; then
  echo 'GRUB_CMDLINE_LINUX_DEFAULT="$GRUB_CMDLINE_LINUX_DEFAULT arm64.nopauth"' > /etc/default/grub.d/99-cua-nopauth.cfg
  update-grub >/dev/null 2>&1 || rm -f /etc/default/grub.d/99-cua-nopauth.cfg
fi
exit 0
"#;

/// Environment variable: the most inodes the runtime VM's guest keeps
/// cached before it drops the unused ones ([`vfs_trim_setup`]).
pub const MAX_CACHED_INODES_ENV: &str = "CUA_RUNTIME_LINUX_MAX_CACHED_INODES";

/// The default: about 10,000 host descriptors (the guest caches roughly two
/// inodes per descriptor the VM holds on the Mac).
pub const DEFAULT_MAX_CACHED_INODES: u64 = 20_000;

/// The inode limit from the environment value (default
/// [`DEFAULT_MAX_CACHED_INODES`]; at least 1,000 so the guest does not
/// thrash).
pub fn max_cached_inodes_from(value: Option<&str>) -> u64 {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(DEFAULT_MAX_CACHED_INODES, |n| n.max(1_000))
}

fn max_cached_inodes() -> u64 {
    max_cached_inodes_from(std::env::var(MAX_CACHED_INODES_ENV).ok().as_deref())
}

/// Inodes the guest keeps cached while it is busy before a trim drops them
/// at once, and the level below which a quiet guest is left alone.
pub const TRIM_QUIET_FLOOR: u64 = 2_000;

/// Run in the VM after every boot (as root): installs and starts a service
/// that keeps the guest's inode cache bounded. Every two seconds it drops
/// the guest's unused dentries and inodes (`drop_caches=2`) when the cache
/// holds more than `limit` inodes, or when it holds more than
/// [`TRIM_QUIET_FLOOR`] and has stopped growing (a job finished).
///
/// Why. Apple's Virtualization framework serves the shared folders
/// (virtiofs) from the Mac's processes and keeps one host file descriptor
/// open per inode the guest has cached. A guest with gigabytes of page
/// cache never evicts them, so installing a Python or Node environment
/// grows the count to tens of thousands and a few weeks of jobs to
/// hundreds of thousands, against the roughly 262,000 the whole Mac
/// shares (`kern.maxfiles`); then `git`, `ssh` and the guest's own `uv`
/// fail with ENFILE. Dropping the unused cache releases the host
/// descriptors at once (measured: 9,179 to 91 in under a second) and loses
/// nothing: only cache, never data, and files a job holds open stay.
///
/// `vm.vfs_cache_pressure` alone does not bound it (measured: the same
/// 9,242 descriptors at 10000 as at the default 100).
pub fn vfs_trim_setup(limit: u64) -> String {
    format!(
        r#"mkdir -p /usr/local/lib/cua
cat > /usr/local/lib/cua/cua-vfs-trim <<'EOS'
#!/bin/sh
# Written by cua: keeps the host file descriptors of the shared folders bounded.
# (A proc file is read whole, by cat: a shell's byte-wise read sees pieces.)
prev=0
while :; do
  set -- $(cat /proc/sys/fs/inode-nr)
  inodes=$1
  delta=$((inodes > prev ? inodes - prev : prev - inodes))
  if [ "$inodes" -gt {limit} ] || {{ [ "$inodes" -gt {floor} ] && [ "$delta" -lt 100 ]; }}; then
    echo 2 > /proc/sys/vm/drop_caches
  fi
  prev=$inodes
  sleep 2
done
EOS
chmod 755 /usr/local/lib/cua/cua-vfs-trim
systemctl stop cua-vfs-trim 2>/dev/null
systemctl reset-failed cua-vfs-trim 2>/dev/null
systemd-run --quiet --unit=cua-vfs-trim --property=Restart=always /usr/local/lib/cua/cua-vfs-trim
"#,
        floor = TRIM_QUIET_FLOOR
    )
}

/// Installs the guest's cache trimmer ([`vfs_trim_setup`]): best effort,
/// the jobs run either way (the host file table is what is at stake).
#[cfg(feature = "lume")]
async fn install_vfs_trim(tools: &Tools) {
    let script = vfs_trim_setup(max_cached_inodes());
    if let Err(e) = run_colima(tools, &["ssh", "--", "sudo", "sh", "-c", &script], 60).await {
        tracing::warn!("the built-in Linux runtime could not start its cache trimmer: {e}");
    }
}

#[cfg(not(feature = "lume"))]
async fn install_vfs_trim(_: &Tools) {}

/// A VM that booted before it had the trimmer (an older cua's) gets it
/// once per process, when this process first finds it running.
async fn trim_running_vm_once() {
    static DONE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    DONE.get_or_init(|| async {
        if let Some(tools) = installed_tools() {
            install_vfs_trim(&tools).await;
        }
    })
    .await;
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
        BackendKind, CheckpointInfo, ClaimIdentity, Endpoints, ImageSource, Instance,
        InstanceSummary, StartSpec, Status,
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
        engine: Arc<tokio::sync::Mutex<Option<Arc<ContainerRuntime>>>>,
        idle: Arc<IdleState>,
    }

    /// What the idle stop checks: operations on the engine still running,
    /// and a counter that moves on every operation (so a stop scheduled
    /// before one is called off).
    #[derive(Default)]
    struct IdleState {
        busy: std::sync::atomic::AtomicUsize,
        activity: std::sync::atomic::AtomicU64,
    }

    /// Marks an engine operation in flight until dropped.
    struct Busy(Arc<IdleState>);

    impl Drop for Busy {
        fn drop(&mut self) {
            use std::sync::atomic::Ordering::SeqCst;
            self.0.activity.fetch_add(1, SeqCst);
            self.0.busy.fetch_sub(1, SeqCst);
        }
    }

    /// How long the VM stays up after its last Linux Space went away
    /// (`CUA_RUNTIME_LINUX_IDLE_SECS`, default 120): creates that follow a
    /// delete, as a busy host's do, find it running.
    fn idle_grace() -> std::time::Duration {
        std::time::Duration::from_secs(
            std::env::var("CUA_RUNTIME_LINUX_IDLE_SECS")
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(120),
        )
    }

    impl ManagedContainers {
        pub fn new(cfg: ContainerConfig) -> Self {
            Self {
                cfg,
                engine: Arc::new(tokio::sync::Mutex::new(None)),
                idle: Arc::new(IdleState::default()),
            }
        }

        fn busy(&self) -> Busy {
            use std::sync::atomic::Ordering::SeqCst;
            self.idle.busy.fetch_add(1, SeqCst);
            self.idle.activity.fetch_add(1, SeqCst);
            Busy(self.idle.clone())
        }

        /// The engine, booting the VM (and setting it up the first time).
        pub async fn engine(&self) -> Result<Arc<ContainerRuntime>> {
            let uri = ensure().await?;
            self.connect(uri).await
        }

        /// Mounts of a job member's directories: the VM shares them at the
        /// same paths (once it booted sharing them; one that did not is
        /// restarted when no Linux Space runs); any other host directory
        /// is not in the VM.
        async fn ready_for_mounts(&self, spec: &StartSpec) -> Result<()> {
            crate::types::check_mounts(crate::BackendKind::Container, &spec.mounts)?;
            if let Some(m) = spec.mounts.iter().find(|m| !is_shared(&m.host)) {
                return Err(VmmError::invalid(format!(
                    "the built-in Linux runtime shares only cua's job directories, not {}",
                    m.host.display()
                )));
            }
            if !running() || shares_job_dirs() {
                return Ok(());
            }
            // Booted before it shared them: restart it now if nothing runs
            // there (the next boot writes the profile that shares them).
            stop_now_if_idle(&self.engine, &self.cfg).await;
            if !running() {
                return Ok(());
            }
            Err(VmmError::other(
                "the built-in Linux runtime started before it shared job directories, and Linux \
                 Spaces run in it now: it shares them from its next start (it stops once no \
                 Linux Space runs)",
            ))
        }

        /// The engine when the VM is up; never boots it.
        pub async fn engine_if_running(&self) -> Option<Arc<ContainerRuntime>> {
            if !running() {
                return None;
            }
            self.connect(endpoint()).await.ok()
        }

        async fn connect(&self, uri: String) -> Result<Arc<ContainerRuntime>> {
            connect_slot(&self.engine, &self.cfg, uri).await
        }

        /// Stops the VM now, without the idle grace, when no Linux Space
        /// or kept-warm member runs in it and no engine operation is in
        /// flight: what a stopping daemon does, so the VM does not outlive
        /// it. Says what it did.
        pub async fn stop_now(&self) -> RuntimeStop {
            if self.idle.busy.load(std::sync::atomic::Ordering::SeqCst) != 0 {
                return RuntimeStop::Busy;
            }
            stop_now_if_idle(&self.engine, &self.cfg).await
        }

        /// Refuses with [`VmmError::HostCapacity`] while the host's file
        /// table is short of room for `what`, after giving back what this
        /// home can: its idle runtime VM (a VM sharing host directories
        /// holds a host descriptor per file its guest cached).
        async fn admit_files(&self, what: &str) -> Result<()> {
            let min = crate::filetable::min_free();
            let Some(table) = crate::filetable::read() else {
                return Ok(());
            };
            if crate::filetable::check(table, min).is_ok() {
                return Ok(());
            }
            if self.stop_now().await == RuntimeStop::Stopped
                && let Some(now) = crate::filetable::read()
                && crate::filetable::check(now, min).is_ok()
            {
                return Ok(());
            }
            crate::filetable::admit(what)
        }

        /// Stops the VM when none of its containers runs (or is paused),
        /// no engine operation is in flight, and nothing happened for
        /// [`idle_grace`]: checked again after the grace, in the background.
        pub async fn stop_if_idle(&self) {
            use std::sync::atomic::Ordering::SeqCst;
            let seen = self.idle.activity.load(SeqCst);
            let idle = self.idle.clone();
            let engine = self.engine.clone();
            let cfg = self.cfg.clone();
            tokio::spawn(async move {
                tokio::time::sleep(idle_grace()).await;
                if idle.busy.load(SeqCst) != 0 || idle.activity.load(SeqCst) != seen {
                    return;
                }
                stop_now_if_idle(&engine, &cfg).await;
            });
        }
    }

    async fn connect_slot(
        engine: &tokio::sync::Mutex<Option<Arc<ContainerRuntime>>>,
        cfg: &ContainerConfig,
        uri: String,
    ) -> Result<Arc<ContainerRuntime>> {
        {
            let mut slot = engine.lock().await;
            if let Some(rt) = slot.as_ref() {
                return Ok(rt.clone());
            }
            let rt = Arc::new(
                ContainerRuntime::connect(ContainerConfig {
                    endpoint: Some(uri),
                    ..cfg.clone()
                })
                .await?,
            );
            *slot = Some(rt.clone());
            Ok(rt)
        }
    }

    /// Stops the VM now when none of its containers runs (or is paused).
    async fn stop_now_if_idle(
        engine: &tokio::sync::Mutex<Option<Arc<ContainerRuntime>>>,
        cfg: &ContainerConfig,
    ) -> RuntimeStop {
        if !running() {
            return RuntimeStop::NotRunning;
        }
        let failed = |what: &str, e: VmmError| RuntimeStop::Failed(format!("{what}: {e}"));
        let rt = match connect_slot(engine, cfg, endpoint()).await {
            Ok(rt) => rt,
            Err(e) => return failed("connecting to the Linux runtime", e),
        };
        let list = match rt.list().await {
            Ok(l) => l,
            Err(e) => return failed("listing the Linux runtime's Spaces", e),
        };
        let members = match rt.pool_members().await {
            Ok(m) => m,
            Err(e) => return failed("listing the warm pool", e),
        };
        let spaces: Vec<(&str, bool)> = list
            .iter()
            .map(|s| (s.name.as_str(), s.status != Status::Stopped))
            .collect();
        let member_states: Vec<(&str, bool, bool)> = members
            .iter()
            .map(|m| (m.name.as_str(), m.keep, m.running))
            .collect();
        match idle_verdict(&spaces, &member_states) {
            IdleVerdict::KeptWarm => return RuntimeStop::KeptWarm,
            IdleVerdict::InUse(n) => return RuntimeStop::KeptForSpaces(n),
            IdleVerdict::Stop => {}
        }
        let member = |n: &str| members.iter().any(|m| m.name == n);
        for m in &members {
            if let Err(e) = rt.delete(&m.name).await {
                tracing::warn!(member = %m.name, "removing a warm-pool member before the idle stop: {e}");
                return failed("removing a warm-pool member", e);
            }
        }
        let names: Vec<String> = list
            .into_iter()
            .map(|s| s.name)
            .filter(|n| !member(n))
            .collect();
        match stop(&names).await {
            Ok(()) => {
                // The next boot is a new engine (its kernel settings, what
                // it supports): connect afresh then.
                *engine.lock().await = None;
                tracing::info!("stopped the built-in Linux runtime: no Linux Space runs");
                RuntimeStop::Stopped
            }
            Err(e) => {
                tracing::warn!("stopping the idle built-in Linux runtime: {e}");
                RuntimeStop::Failed(e.to_string())
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
            let _busy = self.busy();
            self.engine().await?.ensure_base(image, base_name).await
        }

        async fn start(&self, spec: &StartSpec) -> Result<Instance> {
            self.admit_files("a Linux Space").await?;
            if !spec.mounts.is_empty() {
                self.ready_for_mounts(spec).await?;
            }
            let started = {
                let _busy = self.busy();
                self.engine().await?.start(spec).await
            };
            // A warm-pool member is not a Space in use: once it is up, the
            // idle stop is checked again (a refill must not keep an idle
            // host's VM running).
            if spec.pool_member {
                self.stop_if_idle().await;
            }
            started
        }

        async fn claim(
            &self,
            member: &str,
            new_name: &str,
            identity: &ClaimIdentity,
        ) -> Result<Instance> {
            let _busy = self.busy();
            self.engine().await?.claim(member, new_name, identity).await
        }

        async fn stop(&self, name: &str) -> Result<()> {
            {
                let _busy = self.busy();
                let Some(rt) = self.engine_if_running().await else {
                    return if stopped_names().iter().any(|n| n == name) {
                        Ok(())
                    } else {
                        Err(VmmError::NotFound(name.into()))
                    };
                };
                rt.stop(name).await?;
            }
            // After the operation is done, so its own end does not call
            // the idle stop off.
            self.stop_if_idle().await;
            Ok(())
        }

        async fn suspend(&self, name: &str) -> Result<()> {
            self.engine().await?.suspend(name).await
        }

        async fn resume(&self, name: &str) -> Result<Instance> {
            let _busy = self.busy();
            self.engine().await?.resume(name).await
        }

        async fn fork(&self, source: &str, new_name: &str) -> Result<()> {
            let _busy = self.busy();
            self.engine().await?.fork(source, new_name).await
        }

        async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointInfo> {
            let _busy = self.busy();
            self.engine().await?.checkpoint(name, checkpoint).await
        }

        async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
            self.engine().await?.delete_checkpoint(checkpoint).await
        }

        async fn set_egress(
            &self,
            name: &str,
            policy: &crate::egress::EgressPolicy,
        ) -> Result<crate::egress::Enforcement> {
            let _busy = self.busy();
            self.engine().await?.set_egress(name, policy).await
        }

        async fn egress(&self, name: &str) -> Result<crate::egress::EgressPolicy> {
            Ok(crate::container::egress::kept(name)
                .map(|k| k.policy)
                .unwrap_or_default())
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
            {
                let _busy = self.busy();
                if !running() && !stopped_names().iter().any(|n| n == name) {
                    return Err(VmmError::NotFound(name.into()));
                }
                self.engine().await?.delete(name).await?;
            }
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

    #[cfg(test)]
    mod idle_tests {
        use super::*;

        #[tokio::test]
        async fn an_operation_in_flight_holds_off_the_idle_stop() {
            use std::sync::atomic::Ordering::SeqCst;
            let m = ManagedContainers::new(crate::container::ContainerConfig::default());
            let before = m.idle.activity.load(SeqCst);
            let guard = m.busy();
            assert_eq!(m.idle.busy.load(SeqCst), 1);
            let during = m.idle.activity.load(SeqCst);
            assert_ne!(before, during);
            drop(guard);
            assert_eq!(m.idle.busy.load(SeqCst), 0);
            // Its end moves the counter again: a stop scheduled while it ran
            // is called off.
            assert_ne!(m.idle.activity.load(SeqCst), during);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vm_size_defaults_to_4_cpus_and_the_memory_ceiling_within_the_host() {
        assert_eq!(clamp_size(None, None, 12, Some(96)), (4, 32));
        assert_eq!(clamp_size(None, None, 2, Some(8)), (2, 4));
        // A spare machine gives the VM most of itself, never all of it.
        assert_eq!(clamp_size(Some(12), Some(80), 12, Some(96)), (12, 80));
        assert_eq!(clamp_size(Some(64), Some(200), 12, Some(96)), (12, 92));
        assert_eq!(clamp_size(Some(0), Some(1), 12, Some(96)), (1, 2));
        assert_eq!(clamp_size(Some(8), Some(16), 8, None), (8, 16));
    }

    #[test]
    fn a_spare_machine_gives_the_vm_most_of_itself_once() {
        assert_eq!(dedicated_size(12, Some(96)), (12, 72));
        assert_eq!(dedicated_size(8, Some(16)), (8, 12));
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[default]\non = \"local\"\n").unwrap();
        let wrote = dedicate_to_spaces(&p).unwrap().expect("written");
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("on = \"local\""), "{text}");
        assert_eq!(size_from_config(&p), (Some(wrote.0), Some(wrote.1)));
        // The owner's own choice is kept.
        std::fs::write(&p, "[runtime]\nlinux_memory_gib = 8\n").unwrap();
        assert_eq!(dedicate_to_spaces(&p).unwrap(), None);
        assert_eq!(size_from_config(&p), (None, Some(8)));
    }

    #[test]
    fn the_idle_verdict_keeps_the_vm_for_spaces_and_kept_warm_members_only() {
        // Nothing runs: stop.
        assert_eq!(idle_verdict(&[], &[]), IdleVerdict::Stop);
        // A stopped Space does not hold it; a running or paused one does.
        assert_eq!(idle_verdict(&[("a", false)], &[]), IdleVerdict::Stop);
        assert_eq!(
            idle_verdict(&[("a", true), ("b", true), ("c", false)], &[]),
            IdleVerdict::InUse(2)
        );
        // Warm-pool members are not Spaces in use...
        assert_eq!(
            idle_verdict(&[("w1", true)], &[("w1", false, true)]),
            IdleVerdict::Stop
        );
        // ...unless the host keeps their shape warm on purpose.
        assert_eq!(
            idle_verdict(&[("w1", true)], &[("w1", true, true)]),
            IdleVerdict::KeptWarm
        );
        // A kept member that is not running holds nothing up.
        assert_eq!(
            idle_verdict(&[("w1", false)], &[("w1", true, false)]),
            IdleVerdict::Stop
        );
        // A Space next to members still counts.
        assert_eq!(
            idle_verdict(&[("w1", true), ("job", true)], &[("w1", false, true)]),
            IdleVerdict::InUse(1)
        );
    }

    #[test]
    fn the_daemon_stop_says_why_the_runtime_keeps_running() {
        assert_eq!(
            RuntimeStop::KeptForSpaces(2)
                .keeps_running_sentence()
                .unwrap(),
            "the Linux runtime keeps running for 2 Spaces; it stops when they do"
        );
        assert_eq!(
            RuntimeStop::KeptForSpaces(1)
                .keeps_running_sentence()
                .unwrap(),
            "the Linux runtime keeps running for 1 Space; it stops when it does"
        );
        for quiet in [
            RuntimeStop::Stopped,
            RuntimeStop::NotRunning,
            RuntimeStop::Busy,
            RuntimeStop::KeptForSpaces(0),
            RuntimeStop::Failed("x".into()),
        ] {
            assert_eq!(quiet.keeps_running_sentence(), None, "{quiet:?}");
        }
        assert!(
            RuntimeStop::KeptWarm
                .keeps_running_sentence()
                .unwrap()
                .contains("keeps running")
        );
    }

    #[test]
    fn the_inode_limit_defaults_and_never_thrashes() {
        assert_eq!(max_cached_inodes_from(None), DEFAULT_MAX_CACHED_INODES);
        assert_eq!(max_cached_inodes_from(Some(" 50000 ")), 50_000);
        assert_eq!(
            max_cached_inodes_from(Some("junk")),
            DEFAULT_MAX_CACHED_INODES
        );
        assert_eq!(max_cached_inodes_from(Some("5")), 1_000);
    }

    #[test]
    fn the_guest_trimmer_is_a_valid_shell_script_with_its_limits() {
        let setup = vfs_trim_setup(30_000);
        assert!(setup.contains("-gt 30000 "), "{setup}");
        assert!(
            setup.contains(&format!("-gt {TRIM_QUIET_FLOOR} ")),
            "{setup}"
        );
        assert!(setup.contains("echo 2 > /proc/sys/vm/drop_caches"));
        // The script the service runs is the heredoc: it must parse.
        let body = setup
            .split("<<'EOS'\n")
            .nth(1)
            .and_then(|r| r.split("\nEOS\n").next())
            .expect("heredoc");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("trim.sh");
        std::fs::write(&file, body).unwrap();
        let out = std::process::Command::new("sh")
            .arg("-n")
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // The setup itself (the sh -c argument) parses too.
        let setup_file = dir.path().join("setup.sh");
        std::fs::write(&setup_file, &setup).unwrap();
        let out = std::process::Command::new("sh")
            .arg("-n")
            .arg(&setup_file)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn the_shutdown_note_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        record_shutdown_in(d, &RuntimeStop::KeptForSpaces(3));
        assert_eq!(
            take_shutdown_note_in(d).unwrap(),
            "the Linux runtime keeps running for 3 Spaces; it stops when they do"
        );
        assert_eq!(take_shutdown_note_in(d), None);
        // A later stop that stopped everything leaves no stale note.
        record_shutdown_in(d, &RuntimeStop::KeptForSpaces(1));
        record_shutdown_in(d, &RuntimeStop::Stopped);
        assert_eq!(take_shutdown_note_in(d), None);
    }

    #[test]
    fn the_vm_size_reads_the_runtime_table() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "[runtime]\nlinux = \"builtin\"\nlinux_cpus = 10\nlinux_memory_gib = 72\n",
        )
        .unwrap();
        assert_eq!(size_from_config(&p), (Some(10), Some(72)));
        std::fs::write(
            &p,
            "[runtime]\nlinux_cpus = \"many\"\nlinux_memory_gib = \"64\"\n",
        )
        .unwrap();
        assert_eq!(size_from_config(&p), (None, Some(64)));
        assert_eq!(
            size_from_config(&dir.path().join("missing.toml")),
            (None, None)
        );
    }

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
        // The runsc wrapper of memory checkpoints is registered too.
        assert!(y.contains(
            "    cua-runsc:\n      path: /usr/local/lib/cua/cua-runsc\n      runtimeArgs:\n        \
             - --allow-suid\n"
        ));
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

    /// By default the VM boots at 75% of the Mac, at most 32 GiB, at least
    /// 4 GiB: the host backs it only as the guest touches it.
    #[test]
    fn the_vm_boots_at_three_quarters_of_the_mac_up_to_32_gib() {
        assert_eq!(ceiling_gib(Some(8)), 6);
        assert_eq!(ceiling_gib(Some(16)), 12);
        assert_eq!(ceiling_gib(Some(32)), 24);
        assert_eq!(ceiling_gib(Some(96)), MAX_GIB);
        assert_eq!(ceiling_gib(Some(192)), MAX_GIB);
        assert_eq!(ceiling_gib(Some(4)), MIN_GIB);
        assert_eq!(ceiling_gib(None), MIN_GIB);
        // No setting: the ceiling, within the host's reserve.
        assert_eq!(clamp_size(None, None, 8, Some(16)), (4, 12));
        // A setting wins.
        assert_eq!(clamp_size(None, Some(6), 8, Some(16)), (4, 6));
    }

    #[test]
    fn the_profile_shares_only_the_job_directories_it_is_given() {
        let shares = vec![
            PathBuf::from("/Users/ada/.cua/jobs"),
            PathBuf::from("/Users/ada/My Cua/job-data"),
        ];
        let y = profile_yaml_sharing("aarch64", 4, 4, &shares);
        assert!(y.contains("mountType: virtiofs\nmounts:\n"), "{y}");
        assert!(
            y.contains("  - location: \"/Users/ada/.cua/jobs\"\n    writable: true\n"),
            "{y}"
        );
        assert!(
            y.contains("  - location: \"/Users/ada/My Cua/job-data\"\n"),
            "{y}"
        );
        assert_eq!(y.matches("location:").count(), 2);
        // Everything else is the same profile.
        assert!(y.starts_with(&profile_yaml("aarch64", 4, 4)));
        assert_eq!(
            JOB_SHARES,
            ["jobs", "job-data", "job-cache", "job-tools"],
            "cua_cluster::jobdir keeps job members' files there"
        );
        assert!(is_shared(&host::cua_home().join("jobs/cj-1-1/job")));
        assert!(!is_shared(&host::cua_home().join("host")));
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
