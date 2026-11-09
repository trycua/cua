//! The built-in Linux runtime on Windows: a WSL2 distribution cua owns,
//! running Docker, so Linux Spaces run on a PC with nothing installed first
//! (no Docker Desktop), like the built-in runtime on a Mac ([`super`]).
//!
//! # How it works
//!
//! * **A distribution of cua's own.** `cua-runtime-<tag>`, imported
//!   (`wsl --import`) from a pinned, SHA-256 checked Ubuntu base rootfs
//!   ([`rootfs_pin`], about 30 MB) into `<cua home>/runtimes/linux/wsl`. `<tag>` is
//!   a digest of the cua home, so each home (a released Cua Spaces, Cua Spaces
//!   Local, a test home) has its own distribution, pipe and disk, and none can
//!   see another's. An existing distribution (the user's Ubuntu,
//!   `docker-desktop`) is never read, started, stopped or changed: cua only
//!   ever names its own.
//! * **Docker inside it.** One-time setup installs Docker Engine, `socat` and
//!   `iptables` from Ubuntu's package archive ([`PACKAGES`]) and a
//!   `wsl.conf` that turns off the distribution's access to the user's
//!   Windows drives and Windows interop: a container engine running as root
//!   in it cannot reach the user's files. Its bridge networks use
//!   [`BRIDGE_CIDR`] and [`ADDRESS_POOL`], away from the subnets WSL and
//!   Hyper-V pick.
//! * **Started on demand.** A hidden `wsl.exe -d <distro> --exec dockerd`
//!   keeps the engine (and the distribution, which WSL stops when nothing runs
//!   in it) alive. It outlives the process that started it, so a restarted
//!   daemon finds the engine running. The VM WSL runs all distributions in
//!   is shared with whatever else the user runs; its memory and processor
//!   limits are the user's `.wslconfig`.
//! * **A pipe, not a TCP port.** Windows reaches the engine at the named pipe
//!   `\\.\pipe\cua-engine-<tag>` ([`pipe_name`]), served by the cua daemon:
//!   each connection runs `socat` in the distribution against Docker's
//!   socket. Only this user (the pipe's default security) and no remote
//!   client can connect; Docker is never published on a TCP port.
//!   [`crate::container::ContainerRuntime`] connects to it unchanged.
//! * **When it cannot.** WSL2 missing, off or without virtualization is
//!   reported with what to turn on ([`classify_failure`]), never "install
//!   Docker".
//!
//! Differences from Docker Desktop: no GUI, no `docker` CLI on the PATH, no
//! Windows drive mounts in Spaces, and no Kubernetes; from the macOS
//! runtime: Spaces run under `runc` (no gVisor yet) and the engine's VM is
//! WSL's, shared with the user's other distributions.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::Pin;
use crate::error::{Result, VmmError};
use crate::host;

/// The Ubuntu release of the pinned rootfs. Ubuntu, not Alpine: cua's engine
/// helper (checkpoints, egress rules) runs in a privileged container that
/// bind-mounts the engine's `/usr` and starts `/usr/bin/sleep`, and calls
/// `/usr/sbin/chroot`, so the engine's Linux must have Ubuntu's merged-`/usr`
/// layout and GNU userland, as the Mac's runtime (Colima's Ubuntu) does.
pub const BASE_VERSION: &str = "24.04.5";

/// What the one-time setup installs, from Ubuntu's archive.
pub const PACKAGES: &str = "docker.io iptables nftables iproute2 socat ca-certificates";

/// Docker's default bridge, away from WSL's own `172.x` subnets.
pub const BRIDGE_CIDR: &str = "10.230.0.1/24";
/// Where user-defined networks get their subnets.
pub const ADDRESS_POOL: &str = "10.231.0.0/16";

/// Docker's socket inside the distribution.
pub const GUEST_SOCKET: &str = "/run/cua/docker.sock";

/// Bumped when the one-time setup changes: an older distribution is set up again.
pub const SETUP_VERSION: u32 = 3;

/// The pinned Ubuntu base rootfs for this PC's architecture.
pub fn rootfs_pin() -> Pin {
    if cfg!(target_arch = "aarch64") {
        Pin {
            url: "https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/ubuntu-base-24.04.5-base-arm64.tar.gz",
            sha256: "a91d5a93010193712d346d761372b7c9db6dfcf093893161c64ca107f05914f2",
            size: 29_936_675,
        }
    } else {
        Pin {
            url: "https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/ubuntu-base-24.04.5-base-amd64.tar.gz",
            sha256: "e77b6f10c2590cef872b33ee9f635a0e3fd1f57fb074c0e52b5c7f56147a0c86",
            size: 30_028_293,
        }
    }
}

/// What setting up writes under the cua home: the rootfs, the distribution's
/// disk (a sparse VHDX that grows with images and Spaces) and Docker's
/// packages. The guard keeps `CUA_DISK_MIN_FREE` (5 GiB) free on top.
pub const SETUP_DISK_BYTES: u64 = 2 << 30;

/// A warning for the drive holding the runtime's disk (a sparse VHDX that
/// grows with the images and Spaces in it): `None` while it has more than the
/// warning threshold free (`CUA_DISK_WARN_FREE`, default 10 GiB), else how much
/// is left and what to do. Nothing is refused here; a create that would leave
/// less than `CUA_DISK_MIN_FREE` is refused by the setup and image-pull guards.
pub fn low_space_note(volume: &std::path::Path) -> Option<String> {
    let cfg = crate::disk::CacheConfig::load();
    let avail = crate::disk::space(volume).ok()?.available;
    low_space_message(avail, cfg.min_free, cfg.warn_free, volume)
}

fn low_space_message(
    avail: u64,
    min_free: u64,
    warn_free: u64,
    volume: &std::path::Path,
) -> Option<String> {
    use crate::disk::format_size;
    if avail >= warn_free.max(min_free) {
        return None;
    }
    let level = if avail < min_free {
        "below what cua keeps free: new Linux Spaces and image pulls are refused"
    } else {
        "getting low"
    };
    Some(format!(
        "low disk space: {} free on {} ({level}); remove Spaces you do not use or run `cua cache prune`",
        format_size(avail),
        volume.display()
    ))
}

/// What the first Linux Space downloads: the rootfs and about 120 MB of packages.
pub fn first_use_download() -> u64 {
    rootfs_pin().size + (120 << 20)
}

/// Eight hex digits of the cua home's digest: what tells homes apart.
fn tag_of(home: &str) -> String {
    let d = Sha256::digest(home.trim_end_matches(['\\', '/']).to_lowercase().as_bytes());
    d.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// This home's tag.
pub fn tag() -> String {
    tag_of(&host::cua_home().to_string_lossy())
}

/// Every distribution cua creates starts with this.
pub const DISTRO_PREFIX: &str = "cua-runtime-";

/// This home's distribution.
pub fn distro() -> String {
    format!("{DISTRO_PREFIX}{}", tag())
}

/// The named pipe the engine is served at.
pub fn pipe_name() -> String {
    format!(r"\\.\pipe\cua-engine-{}", tag())
}

/// The pipe as an engine endpoint.
pub fn endpoint() -> String {
    format!("npipe:////./pipe/cua-engine-{}", tag())
}

/// Where the distribution's disk lives.
pub fn install_dir() -> PathBuf {
    super::root().join("wsl")
}

/// Written once the distribution is set up (its [`SETUP_VERSION`]).
pub fn marker() -> PathBuf {
    marker_in(&install_dir())
}

fn marker_in(dir: &std::path::Path) -> PathBuf {
    dir.join("provisioned")
}

/// Whether setup finished for this home (the distribution may be stopped).
pub fn provisioned() -> bool {
    std::fs::read_to_string(marker())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(|v| v == SETUP_VERSION)
}

// ---------------------------------------------------------------------------
// Talking to wsl.exe
// ---------------------------------------------------------------------------

/// What a `wsl.exe` call printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WslOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl WslOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// Everything it printed, trimmed.
    pub fn text(&self) -> String {
        format!("{}\n{}", self.stdout.trim(), self.stderr.trim())
            .trim()
            .to_string()
    }
}

/// Runs `wsl.exe` (a fake in tests).
#[async_trait]
pub trait Wsl: Send + Sync {
    /// `wsl.exe <args>`, killed after `secs`.
    async fn run(&self, args: &[String], secs: u64) -> std::io::Result<WslOutput>;
}

/// The real `wsl.exe`.
pub struct WslExe;

/// `wsl.exe` prints its own messages in UTF-16LE (or UTF-8 with
/// `WSL_UTF8=1`); what a Linux program prints is UTF-8.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = bytes.len() >= 2 && (bytes[1] == 0 || bytes[..2] == [0xff, 0xfe]);
    let s = if utf16 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    s.trim_start_matches('\u{feff}')
        .replace(['\r', '\0'], "")
        .to_string()
}

#[async_trait]
impl Wsl for WslExe {
    async fn run(&self, args: &[String], secs: u64) -> std::io::Result<WslOutput> {
        let mut c = tokio::process::Command::new("wsl.exe");
        c.args(args)
            .env("WSL_UTF8", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        c.creation_flags(CREATE_NO_WINDOW);
        let child = c.spawn()?;
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("wsl.exe {} took longer than {secs} s", args.join(" ")),
            )
        })??;
        Ok(WslOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: decode(&out.stdout),
            stderr: decode(&out.stderr),
        })
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Runs `program` as root in `distro` (`wsl.exe -d <distro> -u root --exec`).
pub async fn run_in(
    wsl: &dyn Wsl,
    distro: &str,
    program: &[&str],
    secs: u64,
) -> std::io::Result<WslOutput> {
    wsl.run(&exec_args(distro, program), secs).await
}

/// `wsl.exe -d <distro> -u root --exec <program> <args...>`.
fn exec_args(distro: &str, program: &[&str]) -> Vec<String> {
    let mut v = args(&["-d", distro, "-u", "root", "--exec"]);
    v.extend(program.iter().map(|s| s.to_string()));
    v
}

// ---------------------------------------------------------------------------
// What is there
// ---------------------------------------------------------------------------

/// What `doctor` and `ensure` find out about WSL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// WSL is not installed or not usable: what to do about it.
    Unavailable(String),
    /// WSL answers.
    Available {
        /// This home's distribution is registered.
        distro: bool,
    },
}

/// Why `wsl.exe` is not usable, in the user's words.
pub const NOT_INSTALLED: &str = "Windows Subsystem for Linux (WSL 2) is not installed. \
Cua Spaces runs Linux Spaces in its own small WSL 2 distribution, which needs it: open \
PowerShell as an administrator and run `wsl --install --no-distribution`, restart, and \
try again";

/// Looks at WSL and this home's distribution (starts nothing).
pub async fn probe(wsl: &dyn Wsl) -> Probe {
    match wsl.run(&args(&["--version"]), 20).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Probe::Unavailable(NOT_INSTALLED.into());
        }
        Err(e) => return Probe::Unavailable(format!("wsl.exe did not answer: {e}")),
        Ok(o) if !o.ok() => return Probe::Unavailable(NOT_INSTALLED.into()),
        Ok(_) => {}
    }
    match list(wsl, false).await {
        Ok(names) => Probe::Available {
            distro: names.iter().any(|n| n == &distro()),
        },
        Err(e) => Probe::Unavailable(e),
    }
}

/// Registered (or, with `running`, running) distribution names. A PC with
/// none registered says so with a failing exit code: that is an empty list.
pub async fn list(wsl: &dyn Wsl, running: bool) -> std::result::Result<Vec<String>, String> {
    let mut a = args(&["-l", "-q"]);
    if running {
        a.insert(1, "--running".into());
    }
    let o = wsl
        .run(&a, 30)
        .await
        .map_err(|e| format!("wsl.exe did not answer: {e}"))?;
    Ok(o.stdout
        .lines()
        .map(|l| l.trim().trim_end_matches("(Default)").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// What the failure of a `wsl.exe` call means for the person, when it is one
/// of the known ways a PC cannot run WSL 2; `None` for anything else.
pub fn classify_failure(output: &str) -> Option<String> {
    let o = output.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| o.contains(w));
    if has(&[
        "0x80370102",
        "virtualization is not enabled",
        "virtualization must be enabled",
        "enable virtualization",
        "bios",
    ]) {
        Some(
            "Virtualization is turned off on this PC. Restart into the BIOS/UEFI setup, turn on \
             Intel Virtualization Technology (VT-x) or AMD SVM Mode, start Windows, and try \
             again. (If this is a virtual machine, turn on nested virtualization for it.)"
                .into(),
        )
    } else if has(&["0x80370114", "virtual machine platform", "0x80370102"]) {
        Some(
            "The Windows feature Virtual Machine Platform is off. In PowerShell as an \
             administrator run `wsl --install --no-distribution` (or `dism.exe /online \
             /enable-feature /featurename:VirtualMachinePlatform /all /norestart`), restart, \
             and try again."
                .into(),
        )
    } else if has(&[
        "0x8007019e",
        "optional component is not enabled",
        "subsystem for linux optional",
    ]) {
        Some(
            "The Windows feature Windows Subsystem for Linux is off. In PowerShell as an \
             administrator run `wsl --install --no-distribution`, restart, and try again."
                .into(),
        )
    } else if has(&["kernel update", "wsl --update", "0x800701bc"]) {
        Some(
            "WSL 2 needs its kernel updated: run `wsl --update` in PowerShell, then try again."
                .into(),
        )
    } else if has(&["0x80070003", "0x80070002", "0x8007023e"]) {
        Some(format!(
            "Windows could not create the runtime's disk under {}: check that the drive exists \
             and has room, then try again.",
            install_dir().display()
        ))
    } else {
        None
    }
}

/// An error for a failed `wsl.exe` step: the known cause, else its output.
fn failed(step: &str, out: &WslOutput) -> VmmError {
    let text = out.text();
    match classify_failure(&text) {
        Some(why) => VmmError::other(format!(
            "the built-in Linux runtime cannot be set up: {why}"
        )),
        None => VmmError::other(format!(
            "the built-in Linux runtime: {step} failed (exit {}): {}",
            out.code,
            last_lines(&text, 6)
        )),
    }
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

// ---------------------------------------------------------------------------
// The one-time setup
// ---------------------------------------------------------------------------

/// `/etc/wsl.conf`: no Windows drives, no Windows interop, no systemd.
pub const WSL_CONF: &str = "[automount]\nenabled=false\nmountFsTab=false\n\n[interop]\nenabled=false\nappendWindowsPath=false\n\n[boot]\nsystemd=false\n\n[user]\ndefault=root\n";

/// Docker's `daemon.json`.
pub fn daemon_json() -> String {
    format!(
        "{{\"bip\":\"{BRIDGE_CIDR}\",\"default-address-pools\":[{{\"base\":\"{ADDRESS_POOL}\",\"size\":24}}],\"log-driver\":\"local\",\"log-opts\":{{\"max-size\":\"20m\",\"max-file\":\"3\"}}}}"
    )
}

/// Run in the new distribution, as root: its configuration.
pub fn configure_script() -> String {
    format!(
        "set -e\nprintf '%s' '{}' > /etc/wsl.conf\nmkdir -p /etc/docker\nprintf '%s' '{}' > /etc/docker/daemon.json\n",
        WSL_CONF,
        daemon_json()
    )
}

/// Run in the distribution, as root, after it restarted with its configuration.
pub fn install_script() -> String {
    format!(
        "set -e\n\
         printf '#!/bin/sh\\nexit 101\\n' > /usr/sbin/policy-rc.d\n\
         chmod +x /usr/sbin/policy-rc.d\n\
         export DEBIAN_FRONTEND=noninteractive\n\
         apt-get update\n\
         apt-get install -y --no-install-recommends {PACKAGES}\n\
         {HELPER_CHECK}\n\
         docker --version >/dev/null\n\
         printf 'cua-runtime setup {SETUP_VERSION}\\n' > /etc/cua-runtime\n"
    )
}

/// What the engine helper needs of the engine's Linux (`container::snapshot`
/// bind-mounts `/usr`, starts `/usr/bin/sleep` and runs `/usr/sbin/chroot`;
/// `container::egress` runs `ip`, `nsenter` and `nft` in it):
/// setup fails with a clear message instead of every later create doing so.
pub const HELPER_CHECK: &str = "test -x /usr/bin/sleep && test -x /usr/sbin/chroot && command -v nft >/dev/null && command -v nsenter >/dev/null && command -v ip >/dev/null || { echo 'the runtime image lacks /usr/bin/sleep, /usr/sbin/chroot, nft, nsenter or ip (merged /usr layout)' >&2; exit 1; }";

/// The program that is the engine: Docker's daemon on [`GUEST_SOCKET`].
pub const DOCKERD_SCRIPT: &str = "mkdir -p /run/cua && exec dockerd --host=unix:///run/cua/docker.sock --pidfile=/run/cua/dockerd.pid >>/var/log/dockerd.log 2>&1";

/// Docker answers in the distribution.
pub async fn engine_answers(wsl: &dyn Wsl, distro: &str) -> bool {
    let a = exec_args(
        distro,
        &[
            "docker",
            "-H",
            "unix:///run/cua/docker.sock",
            "version",
            "--format",
            "{{.Server.Version}}",
        ],
    );
    matches!(wsl.run(&a, 20).await, Ok(o) if o.ok() && !o.stdout.trim().is_empty())
}

/// Imports the distribution from the rootfs, configures and sets it up. Any
/// failure removes what this call created.
#[cfg(feature = "lume")]
pub async fn provision(
    wsl: &dyn Wsl,
    name: &str,
    dir: &std::path::Path,
    rootfs: &std::path::Path,
    report: &(dyn Fn(&str, f64) + Sync),
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let disk = dir.join("disk");
    // A distribution of this name is registered already: its disk is the one
    // in use (a failed import must never delete it).
    if list(wsl, false)
        .await
        .is_ok_and(|names| names.iter().any(|n| n == name))
    {
        return Err(VmmError::other(format!(
            "the Linux runtime {name} is already registered with WSL (its disk is kept);              remove it with `wsl --unregister {name}` to set it up again"
        )));
    }
    let _ = std::fs::remove_dir_all(&disk);
    std::fs::create_dir_all(&disk)?;
    report("Importing the Linux runtime", 0.05);
    let import = wsl
        .run(
            &args(&[
                "--import",
                name,
                &disk.to_string_lossy(),
                &rootfs.to_string_lossy(),
                "--version",
                "2",
            ]),
            300,
        )
        .await
        .map_err(|e| VmmError::other(format!("wsl.exe --import: {e}")))?;
    if !import.ok() {
        let _ = std::fs::remove_dir_all(&disk);
        return Err(failed("wsl --import", &import));
    }
    let built = async {
        report("Configuring the Linux runtime", 0.25);
        let cfg = wsl
            .run(&exec_args(name, &["sh", "-c", &configure_script()]), 120)
            .await
            .map_err(|e| VmmError::other(format!("configuring the runtime: {e}")))?;
        if !cfg.ok() {
            return Err(failed("configuring the distribution", &cfg));
        }
        // wsl.conf is read when the distribution starts.
        terminate(wsl, name).await;
        report("Installing Docker in the Linux runtime", 0.4);
        let inst = wsl
            .run(&exec_args(name, &["sh", "-c", &install_script()]), 900)
            .await
            .map_err(|e| VmmError::other(format!("installing Docker: {e}")))?;
        if !inst.ok() {
            return Err(failed("installing Docker (is the network up?)", &inst));
        }
        std::fs::write(marker_in(dir), SETUP_VERSION.to_string())?;
        Ok(())
    }
    .await;
    if built.is_err() {
        // Only the distribution this call imported.
        unregister(wsl, name).await;
        let _ = std::fs::remove_dir_all(&disk);
    }
    built
}

/// Stops this home's distribution (nothing else).
pub async fn terminate(wsl: &dyn Wsl, name: &str) {
    if name.starts_with(DISTRO_PREFIX) {
        let _ = wsl.run(&args(&["--terminate", name]), 60).await;
    }
}

/// Removes this home's distribution and its disk (nothing else).
pub async fn unregister(wsl: &dyn Wsl, name: &str) {
    if name.starts_with(DISTRO_PREFIX) {
        let _ = wsl.run(&args(&["--unregister", name]), 120).await;
    }
}

/// Starts Docker in the distribution when it does not answer, and waits for it.
pub async fn start_engine(wsl: &dyn Wsl, name: &str) -> Result<()> {
    if engine_answers(wsl, name).await {
        return Ok(());
    }
    spawn_engine(name)?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        if engine_answers(wsl, name).await {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            let log = wsl
                .run(
                    &exec_args(name, &["tail", "-n", "5", "/var/log/dockerd.log"]),
                    20,
                )
                .await
                .map(|o| last_lines(&o.text(), 5))
                .unwrap_or_default();
            return Err(VmmError::other(format!(
                "the built-in Linux runtime started but Docker did not answer within 90 s: {log}"
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// A hidden `wsl.exe` that is Docker. It outlives this process.
fn spawn_engine(name: &str) -> Result<()> {
    let mut c = std::process::Command::new("wsl.exe");
    c.args(exec_args(name, &["sh", "-c", DOCKERD_SCRIPT]))
        .env("WSL_UTF8", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let base = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
        // Out of the parent's job (Electron's, a terminal's), when it allows it.
        if c.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        c.creation_flags(base);
    }
    c.spawn()
        .map(drop)
        .map_err(|e| VmmError::other(format!("starting wsl.exe: {e}")))
}

// ---------------------------------------------------------------------------
// The pipe
// ---------------------------------------------------------------------------

/// Whether the engine's pipe is there (served by this or another process).
pub fn pipe_exists() -> bool {
    #[cfg(windows)]
    {
        match std::fs::metadata(pipe_name()) {
            Ok(_) => true,
            // Busy: its one listening instance is taken this instant.
            Err(e) => e.raw_os_error() == Some(231),
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Stops serving the pipe from this process (the engine is stopping).
pub fn stop_serving() {
    #[cfg(windows)]
    {
        if let Some(h) = SERVING.lock().unwrap_or_else(|e| e.into_inner()).take() {
            h.abort();
        }
    }
}

#[cfg(windows)]
static SERVING: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>> = std::sync::Mutex::new(None);

/// A daemon that restarted finds this home's engine still running (its
/// `wsl.exe` outlives it) and serves the pipe again. Never blocks the caller
/// (a wedged WSL service must not wedge the daemon): the look runs in the
/// background, one at a time, at most every 15 s, and is given 10 s.
pub fn reattach() {
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        static BUSY: AtomicBool = AtomicBool::new(false);
        static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
        if !provisioned() {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        {
            let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(15)) {
                return;
            }
            if BUSY.swap(true, SeqCst) {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        rt.spawn(async {
            let look =
                tokio::time::timeout(std::time::Duration::from_secs(10), list(&WslExe, true)).await;
            if let Ok(Ok(names)) = look
                && names.iter().any(|n| *n == distro())
            {
                let _ = serve_pipe();
            }
            BUSY.store(false, SeqCst);
        });
    }
}

/// Serves the engine's pipe from this process until it ends: every
/// connection is a `socat` in the distribution against Docker's socket.
/// Another process already serving it is fine.
pub fn serve_pipe() -> Result<()> {
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ServerOptions;
        let mut slot = SERVING.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return Ok(());
        }
        let name = pipe_name();
        let first = match ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&name)
        {
            Ok(s) => s,
            // Access denied or busy: another process serves it.
            Err(e) if matches!(e.raw_os_error(), Some(5) | Some(231)) => return Ok(()),
            Err(e) => {
                return Err(VmmError::other(format!(
                    "the built-in Linux runtime cannot open its pipe {name}: {e}"
                )));
            }
        };
        let distro = distro();
        *slot = Some(tokio::spawn(async move {
            let mut server = first;
            loop {
                if server.connect().await.is_err() {
                    break;
                }
                let client = server;
                server = match ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&name)
                {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("the built-in Linux runtime's pipe stopped: {e}");
                        break;
                    }
                };
                tokio::spawn(bridge(client, distro.clone()));
            }
        }));
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(VmmError::other(
            "the Windows runtime's pipe only exists on Windows",
        ))
    }
}

/// How many clients of the pipe may be bridged at once: each is a `wsl.exe`
/// (and a `socat`) in the distribution, and a few hundred of them exhausted
/// the WSL service's sockets.
#[cfg(windows)]
const MAX_BRIDGES: usize = 96;

/// How long a bridge lets Docker finish its reply after the client went away (a
/// named pipe has no half-close, so a client that left wants nothing more; the
/// daemon opens ten of these a second, and each lives this long).
#[cfg(windows)]
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// One client of the pipe to Docker's socket.
#[cfg(windows)]
async fn bridge(client: tokio::net::windows::named_pipe::NamedPipeServer, distro: String) {
    static LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_BRIDGES);
    let Ok(Ok(_slot)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), LIMIT.acquire()).await
    else {
        tracing::warn!("the Linux runtime's pipe has {MAX_BRIDGES} connections open; refusing one");
        return;
    };
    let mut c = tokio::process::Command::new("wsl.exe");
    c.args(exec_args(
        &distro,
        &["socat", "-", &format!("UNIX-CONNECT:{GUEST_SOCKET}")],
    ))
    .env("WSL_UTF8", "1")
    .creation_flags(CREATE_NO_WINDOW);
    bridge_with(client, c).await;
}

/// Runs `command` (it speaks to Docker's socket on its stdin and stdout) for
/// `client` until either side is done, then ends it. `wsl.exe` does not pass
/// the end of its stdin on to the program in the distribution, so a client that
/// went away must not leave it running: that leaked one `wsl.exe` per
/// connection.
#[cfg(windows)]
async fn bridge_with<C>(client: C, mut command: tokio::process::Command)
where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin,
{
    use tokio::io::{AsyncWriteExt as _, split};
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let Ok(mut child) = command.spawn() else {
        return;
    };
    let (Some(mut to_guest), Some(mut from_guest)) = (child.stdin.take(), child.stdout.take())
    else {
        return;
    };
    let (mut from_client, mut to_client) = split(client);
    let up = async {
        let _ = tokio::io::copy(&mut from_client, &mut to_guest).await;
        let _ = to_guest.shutdown().await;
    };
    let down = async {
        let _ = tokio::io::copy(&mut from_guest, &mut to_client).await;
        let _ = to_client.shutdown().await;
    };
    tokio::pin!(up);
    tokio::pin!(down);
    tokio::select! {
        // Docker closed its side (or the program ended): done.
        _ = &mut down => {}
        // The client is gone: a moment for the reply, then the program ends.
        _ = &mut up => {
            let _ = tokio::time::timeout(CLOSE_GRACE, &mut down).await;
        }
    }
    let _ = child.kill().await;
}

/// Alias for callers that hold the process-wide `WslExe`.
pub fn real() -> Arc<WslExe> {
    Arc::new(WslExe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A `wsl.exe` that answers from a script and records what it was asked.
    struct Fake {
        calls: Mutex<Vec<String>>,
        answers: Vec<(&'static str, WslOutput)>,
    }

    impl Fake {
        fn new(answers: Vec<(&'static str, WslOutput)>) -> Self {
            Self {
                calls: Mutex::new(vec![]),
                answers,
            }
        }
    }

    fn out(code: i32, stdout: &str, stderr: &str) -> WslOutput {
        WslOutput {
            code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[async_trait]
    impl Wsl for Fake {
        async fn run(&self, args: &[String], _secs: u64) -> std::io::Result<WslOutput> {
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            Ok(self
                .answers
                .iter()
                .find(|(k, _)| line.contains(k))
                .map(|(_, o)| o.clone())
                .unwrap_or_else(|| out(0, "", "")))
        }
    }

    #[test]
    fn utf16_output_of_wsl_exe_is_decoded() {
        let bytes: Vec<u8> = "\u{feff}Ubuntu\r\ndocker-desktop\r\n"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode(&bytes), "Ubuntu\ndocker-desktop\n");
        assert_eq!(decode(b"plain\r\n"), "plain\n");
    }

    #[test]
    fn homes_get_their_own_names_and_pipes() {
        assert_eq!(tag_of(r"C:\Users\a\.cua"), tag_of(r"c:\users\a\.cua\"));
        assert_ne!(tag_of(r"C:\Users\a\.cua"), tag_of(r"C:\Users\a\.cua-local"));
        assert_eq!(tag_of("x").len(), 8);
        assert!(distro().starts_with(DISTRO_PREFIX));
        assert!(pipe_name().starts_with(r"\\.\pipe\cua-engine-"));
        assert!(endpoint().starts_with("npipe:////./pipe/cua-engine-"));
        // Never one of Docker Desktop's.
        assert!(!distro().contains("docker-desktop"));
        assert!(!endpoint().contains("docker_engine"));
    }

    #[test]
    fn the_pins_are_checked_downloads() {
        let p = rootfs_pin();
        assert_eq!(p.sha256.len(), 64);
        assert!(p.url.starts_with("https://"));
        assert!(p.url.contains(BASE_VERSION));
    }

    #[test]
    fn known_wsl_failures_say_what_to_turn_on() {
        let v = classify_failure("WslRegisterDistribution failed with error: 0x80370102").unwrap();
        assert!(v.contains("BIOS"), "{v}");
        let f =
            classify_failure("Please enable the Virtual Machine Platform Windows feature").unwrap();
        assert!(f.contains("Virtual Machine Platform"), "{f}");
        let w = classify_failure("Error: 0x8007019e").unwrap();
        assert!(w.contains("wsl --install"), "{w}");
        assert!(classify_failure("some other failure").is_none());
        // Never tells the user to install Docker.
        for m in [&v, &f, &w] {
            assert!(!m.to_lowercase().contains("install docker"));
        }
    }

    #[tokio::test]
    async fn probe_tells_missing_wsl_from_a_missing_distro() {
        let none = Fake::new(vec![("--version", out(1, "", "invalid option"))]);
        assert_eq!(probe(&none).await, Probe::Unavailable(NOT_INSTALLED.into()));
        let other = Fake::new(vec![("-l -q", out(0, "Ubuntu\ndocker-desktop\n", ""))]);
        assert_eq!(probe(&other).await, Probe::Available { distro: false });
        let ours = Fake::new(vec![(
            "-l -q",
            out(0, &format!("Ubuntu (Default)\n{}\n", distro()), ""),
        )]);
        assert_eq!(probe(&ours).await, Probe::Available { distro: true });
    }

    #[tokio::test]
    async fn only_our_distribution_is_ever_stopped_or_removed() {
        let w = Fake::new(vec![]);
        terminate(&w, "Ubuntu").await;
        unregister(&w, "docker-desktop").await;
        assert!(w.calls.lock().unwrap().is_empty());
        terminate(&w, &distro()).await;
        unregister(&w, &distro()).await;
        let calls = w.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|c| c.contains(DISTRO_PREFIX)));
    }

    #[tokio::test]
    async fn a_failed_import_names_the_cause_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Fake::new(vec![(
            "--import",
            out(
                1,
                "",
                "WslRegisterDistribution failed with error: 0x80370102",
            ),
        )]);
        let rootfs = tmp.path().join("r.tar.gz");
        let e = provision(&w, &distro(), tmp.path(), &rootfs, &|_, _| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("BIOS"), "{e}");
        assert!(!tmp.path().join("disk").exists());
    }

    #[tokio::test]
    async fn a_failed_install_unregisters_only_what_it_imported() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Fake::new(vec![(
            "apt-get install",
            out(1, "", "fetch failed: network unreachable"),
        )]);
        let rootfs = tmp.path().join("r.tar.gz");
        let e = provision(&w, &distro(), tmp.path(), &rootfs, &|_, _| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("network"), "{e}");
        let calls = w.calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with(&format!("--unregister {}", distro())))
        );
        assert!(!marker_in(tmp.path()).exists());
    }

    #[tokio::test]
    async fn setup_configures_before_it_installs_and_marks_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Fake::new(vec![]);
        provision(
            &w,
            &distro(),
            tmp.path(),
            &tmp.path().join("r.tar.gz"),
            &|_, _| {},
        )
        .await
        .unwrap();
        let calls = w.calls.lock().unwrap().clone();
        let at = |needle: &str| calls.iter().position(|c| c.contains(needle)).unwrap();
        assert!(at("--import") < at("wsl.conf"));
        assert!(at("wsl.conf") < at("--terminate"));
        assert!(at("--terminate") < at("apt-get install"));
        assert!(marker_in(tmp.path()).exists());
    }

    /// The engine helper (`container::snapshot`) bind-mounts the engine's `/usr` and
    /// starts `/usr/bin/sleep`: an Alpine engine had no such file and every
    /// Space that needed the helper failed with "stat /usr/bin/sleep".
    #[test]
    fn the_engine_linux_has_what_the_helper_runs() {
        let s = install_script();
        assert!(s.contains(HELPER_CHECK), "{s}");
        assert!(
            HELPER_CHECK.contains("/usr/bin/sleep")
                && HELPER_CHECK.contains("/usr/sbin/chroot")
                && HELPER_CHECK.contains("nft")
                && HELPER_CHECK.contains("nsenter")
                && HELPER_CHECK.contains("command -v ip ")
        );
        // Debian's package scripts must not try to start services in a distribution without init.
        assert!(s.contains("policy-rc.d") && s.contains("exit 101"));
        assert!(s.contains("apt-get install -y --no-install-recommends docker.io"));
        assert!(
            rootfs_pin().url.contains("ubuntu-base"),
            "an Ubuntu rootfs, like the Mac's runtime"
        );
        let helper = include_str!("container/snapshot.rs");
        assert!(
            helper.contains("/usr/bin/sleep"),
            "the helper's entrypoint this check is for"
        );
    }

    /// A client that closes its end must not leave the bridged program (a
    /// `wsl.exe` that ignores the end of its stdin) running: 600 leaked ones
    /// exhausted the WSL service.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_client_that_goes_away_ends_the_bridged_program() {
        use tokio::io::AsyncWriteExt as _;
        let (mut client, server) = tokio::io::duplex(1024);
        let mut sleeper = tokio::process::Command::new("powershell.exe");
        sleeper.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 120"]);
        let task = tokio::spawn(bridge_with(server, sleeper));
        client
            .write_all(
                b"GET /_ping HTTP/1.1

",
            )
            .await
            .unwrap();
        drop(client);
        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        assert!(ended.is_ok(), "the bridge stayed up after its client left");
    }

    #[test]
    fn a_filling_drive_is_reported_before_it_is_refused() {
        let v = std::path::Path::new("D:/cua");
        let gib = 1u64 << 30;
        assert_eq!(low_space_message(50 * gib, 5 * gib, 10 * gib, v), None);
        let warn = low_space_message(8 * gib, 5 * gib, 10 * gib, v).unwrap();
        assert!(
            warn.contains("8.0 GiB free") && warn.contains("getting low"),
            "{warn}"
        );
        let refuse = low_space_message(2 * gib, 5 * gib, 10 * gib, v).unwrap();
        assert!(
            refuse.contains("refused") && refuse.contains("cua cache prune"),
            "{refuse}"
        );
    }

    #[test]
    fn the_guest_configuration_isolates_the_distribution() {
        let s = configure_script();
        assert!(s.contains("enabled=false"));
        assert!(s.contains("appendWindowsPath=false"));
        let j: serde_json::Value = serde_json::from_str(&daemon_json()).unwrap();
        assert_eq!(j["bip"], BRIDGE_CIDR);
        assert!(!DOCKERD_SCRIPT.contains("tcp://"), "no TCP listener");
    }
}
