//! Host introspection and small process/network helpers shared by backends.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use cua_home::channel::CuaChannel;
use serde::{Deserialize, Serialize};

use crate::error::{Result, VmmError};
use crate::types::Arch;

/// Host operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOs {
    Macos,
    Linux,
    Windows,
}

impl HostOs {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            HostOs::Macos
        } else if cfg!(target_os = "windows") {
            HostOs::Windows
        } else {
            HostOs::Linux
        }
    }
}

/// `$CUA_HOME`, else this process's channel's cua home: `~/.cua`
/// (Stable) or `~/.cua-local` (Local, so a standalone `cua-local` never
/// shares Stable's state, runtimes or config when `CUA_HOME` is unset).
pub fn cua_home() -> PathBuf {
    cua_home_with(
        CuaChannel::current(),
        std::env::var_os("CUA_HOME").as_deref(),
        &home_dir(),
    )
}

/// [`cua_home`] for `channel`, with `$CUA_HOME` and the user's home given.
/// Local's home is `~/.cua-local` itself, as the app pins it, not below
/// `envs/<env>`.
pub fn cua_home_with(
    channel: CuaChannel,
    cua_home_env: Option<&std::ffi::OsStr>,
    home: &Path,
) -> PathBuf {
    if let Some(h) = cua_home_env.filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    match channel.home_dir().file_name() {
        Some(name) => home.join(name),
        None => home.join(".cua"),
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Locate an executable on `PATH`, then in well-known install prefixes that
/// are often missing from GUI/ssh PATHs (Homebrew, MacPorts, ~/.local/bin).
pub fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/opt/local/bin",
        "/usr/bin",
        "/usr/sbin",
    ] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.push(home_dir().join(".local/bin"));
    dirs.push(cua_home().join("bin"));
    dirs.extend(windows_install_dirs());
    dirs.into_iter()
        .map(|d| d.join(&exe))
        .find(|p| is_executable(p))
}

/// Where Windows installers put tools without adding them to `PATH`. The
/// QEMU installer is one: it installs to `C:\Program Files\qemu` and
/// leaves `PATH` alone, so a PC that has QEMU would still look as if it
/// did not.
fn windows_install_dirs() -> Vec<PathBuf> {
    if !cfg!(windows) {
        return Vec::new();
    }
    ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|d| PathBuf::from(d).join("qemu"))
        .collect()
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// A free loopback TCP port chosen by the OS.
/// Writes `data` to `path` readable by the owner only (`0600` on Unix):
/// written to a sibling temp file created with that mode, then renamed, so
/// the contents are never readable by others, even briefly.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

pub fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Ports handed out to a start that has not finished binding them yet.
///
/// A probe (`bind` then drop) only proves a port was free at that instant:
/// two starts racing in one process would both see it free, and QEMU then
/// loses the bind. Every allocation is therefore recorded here until its
/// [`PortLease`] drops (after the VM is up and holds the ports itself), so
/// concurrent allocations in one process can never return the same port.
fn leased() -> &'static std::sync::Mutex<std::collections::HashSet<u16>> {
    static LEASED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u16>>> =
        std::sync::OnceLock::new();
    LEASED.get_or_init(Default::default)
}

/// Host ports reserved for one VM start; released on drop.
#[derive(Debug, Default)]
pub struct PortLease {
    ports: Vec<u16>,
}

impl Drop for PortLease {
    fn drop(&mut self) {
        let mut set = leased().lock().unwrap_or_else(|e| e.into_inner());
        for p in &self.ports {
            set.remove(p);
        }
    }
}

impl PortLease {
    /// `n` distinct free loopback ports that no live lease holds.
    pub fn ports(&mut self, n: usize) -> Result<Vec<u16>> {
        let mut out = Vec::with_capacity(n);
        let mut tries = 0;
        while out.len() < n {
            tries += 1;
            if tries > 64 * n.max(1) {
                return Err(VmmError::other("no free loopback ports"));
            }
            // Hold the probes together so one call's ports are distinct.
            let probes: Vec<TcpListener> = (0..n - out.len())
                .map(|_| TcpListener::bind("127.0.0.1:0"))
                .collect::<std::io::Result<_>>()?;
            let mut set = leased().lock().unwrap_or_else(|e| e.into_inner());
            for l in &probes {
                let p = l.local_addr()?.port();
                if set.insert(p) {
                    self.ports.push(p);
                    out.push(p);
                }
            }
        }
        Ok(out)
    }

    /// A VNC display `N >= start` whose port `5900+N` is free on loopback
    /// and held by no live lease. QEMU exits with "Failed to find an
    /// available port" if the display is taken.
    pub fn vnc_display(&mut self, start: u16) -> Option<u16> {
        let mut set = leased().lock().unwrap_or_else(|e| e.into_inner());
        (start..start.saturating_add(1000)).find(|d| {
            let port = 5900 + d;
            if set.contains(&port) || TcpListener::bind(("127.0.0.1", port)).is_err() {
                return false;
            }
            set.insert(port);
            self.ports.push(port);
            true
        })
    }
}

/// `n` distinct free ports (held open while picking so they don't collide).
/// Prefer [`PortLease::ports`] when the ports are bound later by another
/// process: this variant does not reserve them past the call.
pub fn free_ports(n: usize) -> Result<Vec<u16>> {
    // Listeners stay open until every port is picked, so one call never
    // repeats a port; ports handed out recently are skipped, so concurrent
    // launches (a wave of forks) do not get the same port before the first
    // one binds it.
    let mut listeners: Vec<TcpListener> = Vec::with_capacity(n);
    let mut ports = Vec::with_capacity(n);
    let mut tries = 0;
    while ports.len() < n {
        let l = TcpListener::bind("127.0.0.1:0")?;
        let port = l.local_addr()?.port();
        tries += 1;
        if reserve(PortKind::Tcp, port) || tries > n * 50 {
            ports.push(port);
        }
        listeners.push(l);
    }
    Ok(ports)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PortKind {
    Tcp,
}

/// How long a handed-out port is kept from other callers of this process:
/// long enough for the launch that asked for it to bind it.
const PORT_HOLD: std::time::Duration = std::time::Duration::from_secs(60);

/// Records `port` as handed out; false when it was within [`PORT_HOLD`].
fn reserve(kind: PortKind, port: u16) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Instant;
    static HELD: Mutex<Option<HashMap<(PortKind, u16), Instant>>> = Mutex::new(None);
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let held = held.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    held.retain(|_, at| now.duration_since(*at) < PORT_HOLD);
    if held.contains_key(&(kind, port)) {
        return false;
    }
    held.insert((kind, port), now);
    true
}

/// Run a host command to completion and return stdout; errors carry stderr.
pub async fn run(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Result<String> {
    let program = program.as_ref();
    let out = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VmmError::missing(program.to_string_lossy(), "not found on PATH")
            } else {
                e.into()
            }
        })?;
    if !out.status.success() {
        return Err(VmmError::Command {
            cmd: format!("{} {}", program.to_string_lossy(), args.join(" ")),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

static SUDO_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn sudo_available() -> bool {
    *SUDO_AVAILABLE.get_or_init(|| {
        which("sudo").is_some_and(|sudo| {
            std::process::Command::new(sudo)
                .args(["-n", "true"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    })
}

/// Whether hardware virtualisation is usable for a guest of `guest` arch, and
/// which QEMU accelerator to use.
pub fn qemu_accel(os: HostOs, guest: Arch, kvm: &std::io::Result<()>) -> Result<&'static str> {
    qemu_accel_with(os, guest, kvm, sudo_available, |message| {
        tracing::warn!("{message}")
    })
}

pub(crate) fn qemu_accel_with(
    os: HostOs,
    guest: Arch,
    kvm: &std::io::Result<()>,
    sudo: impl Fn() -> bool,
    mut warn: impl FnMut(&str),
) -> Result<&'static str> {
    let host = Arch::host();
    match (os, kvm) {
        // HVF only virtualises the host's own architecture.
        (HostOs::Macos, _) if host == guest => Ok("hvf"),
        (HostOs::Linux, Ok(())) if host == guest => Ok("kvm"),
        (HostOs::Linux, Err(e)) if host == guest && e.kind() != std::io::ErrorKind::NotFound => {
            if e.kind() == std::io::ErrorKind::PermissionDenied && !sudo() {
                warn("KVM access denied and sudo unavailable; using slow TCG emulation");
                Ok("tcg")
            } else {
                Err(VmmError::missing("KVM", kvm_hint(e)))
            }
        }
        (HostOs::Windows, _) if host == guest => Ok("whpx"),
        _ => Ok("tcg"),
    }
}

/// Opens `/dev/kvm` read/write.
pub fn kvm_usable() -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .map(drop)
}

/// Why `/dev/kvm` could not be opened, and how to fix it.
pub fn kvm_hint(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "/dev/kvm is missing, so same-architecture guests run \
            under slow TCG emulation; enable virtualization on the host (nested virtualization \
            if the host is a VM)"
            .into(),
        std::io::ErrorKind::PermissionDenied => "permission denied opening /dev/kvm; add your \
            user to its group (for the kvm group: `sudo usermod -aG kvm \"$USER\"`), then log \
            out and back in"
            .into(),
        _ => format!("cannot open /dev/kvm: {e}"),
    }
}

/// The macOS version (`26.6.2`), when this is a Mac.
pub fn macos_version() -> Option<String> {
    if HostOs::current() != HostOs::Macos {
        return None;
    }
    let plist = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
    product_version(&plist)
}

/// `ProductVersion` from a `SystemVersion.plist`.
fn product_version(plist: &str) -> Option<String> {
    let after = plist.split("<key>ProductVersion</key>").nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let len = after[start..].find("</string>")?;
    Some(after[start..start + len].trim().to_string()).filter(|v| !v.is_empty())
}

/// The major macOS version (`26`), when this is a Mac.
pub fn macos_major() -> Option<u32> {
    macos_version()?.split('.').next()?.parse().ok()
}

/// Unix seconds.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether a process with this pid is alive.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) via the `kill` utility keeps us free of libc bindings.
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        windows_pid_alive(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Opens the pid with query rights and checks its exit code. A pid that no
/// longer exists fails to open with `ERROR_INVALID_PARAMETER`; one we may not
/// query (`ERROR_ACCESS_DENIED`, e.g. another user's or a protected process)
/// exists, so it counts as alive rather than as exited.
#[cfg(windows)]
fn windows_pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return false;
    }
    // SAFETY: OpenProcess takes a numeric pid and returns a handle or null;
    // only query rights are requested.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // SAFETY: reads this thread's last error, set by OpenProcess.
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    let mut code = 0u32;
    // SAFETY: `handle` is a valid process handle opened above and `code` is a
    // live u32; the handle is closed exactly once.
    unsafe {
        let queried = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        // A query failure on an open handle cannot confirm exit. (A process
        // that exits with code 259, STILL_ACTIVE, reads as alive.)
        !queried || code == STILL_ACTIVE as u32
    }
}

/// Send a signal (`TERM`, `KILL`, ...) to a pid.
pub fn signal(pid: u32, sig: &str) {
    let _ = std::process::Command::new("kill")
        .args([&format!("-{sig}"), &pid.to_string()])
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_handed_out_concurrently_never_repeat() {
        let all: Vec<u16> = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..16)
                .map(|_| sc.spawn(|| super::free_ports(4).unwrap()))
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        let set: std::collections::BTreeSet<u16> = all.iter().copied().collect();
        assert_eq!(set.len(), all.len(), "a port was handed out twice: {all:?}");
    }

    /// Displays handed out at once (VMs starting together) never collide,
    /// although nothing listens on them yet.
    #[test]
    fn vnc_displays_handed_out_at_once_are_distinct() {
        let mut lease = PortLease::default();
        let ds: Vec<u16> = (0..8).filter_map(|_| lease.vnc_display(40)).collect();
        assert_eq!(ds.len(), 8);
        let mut uniq = ds.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 8, "{ds:?}");
    }

    #[test]
    fn the_cua_home_follows_the_channel_and_cua_home_overrides_it() {
        use std::ffi::OsStr;
        let home = Path::new("/Users/a");
        // Stable's default is unchanged.
        assert_eq!(
            cua_home_with(CuaChannel::Stable, None, home),
            PathBuf::from("/Users/a/.cua")
        );
        assert_eq!(
            cua_home_with(CuaChannel::Local, None, home),
            PathBuf::from("/Users/a/.cua-local")
        );
        for c in CuaChannel::ALL {
            assert_eq!(
                cua_home_with(c, Some(OsStr::new("/tmp/h")), home),
                PathBuf::from("/tmp/h")
            );
            // Blank says nothing.
            assert_eq!(
                cua_home_with(c, Some(OsStr::new("")), home),
                cua_home_with(c, None, home)
            );
        }
    }

    /// A standalone `cua-local` (no app, so no `CUA_HOME` pinned) is Local
    /// by its name and keeps everything under `~/.cua-local`.
    #[test]
    fn a_standalone_cua_local_without_cua_home_uses_the_local_home() {
        let home = Path::new("/Users/a");
        let exe = PathBuf::from("/Users/a/.local/bin/cua-local");
        let channel = CuaChannel::resolve(None, None, &[exe]).unwrap();
        assert_eq!(channel, CuaChannel::Local);
        let cua = cua_home_with(channel, None, home);
        assert_eq!(cua, PathBuf::from("/Users/a/.cua-local"));
        #[cfg(feature = "lume")]
        {
            assert_eq!(
                crate::lume::root_in(&cua),
                PathBuf::from("/Users/a/.cua-local/vmm/lume")
            );
            assert_eq!(
                crate::lume::builtin::root_in(&cua),
                PathBuf::from("/Users/a/.cua-local/runtimes/lume")
            );
        }
        // ... and the same name for Stable is still `~/.cua`.
        let stable =
            CuaChannel::resolve(None, None, &[PathBuf::from("/Users/a/.local/bin/cua")]).unwrap();
        assert_eq!(
            cua_home_with(stable, None, home),
            PathBuf::from("/Users/a/.cua")
        );
    }

    /// Binds `port`, waiting out another thread's momentary probe of it (a
    /// probe is dropped at once; this is the window `launch` retries over).
    fn bind_soon(port: u16) -> TcpListener {
        for _ in 0..200 {
            if let Ok(l) = TcpListener::bind(("127.0.0.1", port)) {
                return l;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("leased port {port} never became bindable");
    }

    #[test]
    fn leases_never_hand_out_the_same_port_concurrently() {
        // Many threads allocate ports and VNC displays and hold the lease
        // (as a starting VM does); no port may appear twice.
        let all = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u16>::new()));
        let held = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let (all, held) = (all.clone(), held.clone());
                std::thread::spawn(move || {
                    for _ in 0..8 {
                        let mut lease = PortLease::default();
                        let mut got = lease.ports(3).unwrap();
                        // Bind what we were given, as QEMU would.
                        let mut socks: Vec<_> = got.iter().map(|p| bind_soon(*p)).collect();
                        if let Some(d) = lease.vnc_display(0) {
                            got.push(5900 + d);
                            socks.push(bind_soon(5900 + d));
                        }
                        all.lock().unwrap().extend(got);
                        held.lock().unwrap().push((lease, socks));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let mut v = all.lock().unwrap().clone();
        let n = v.len();
        v.sort_unstable();
        v.dedup();
        assert_eq!(v.len(), n, "a port was handed out twice");
    }

    #[test]
    fn dropping_a_lease_releases_its_ports() {
        let mut a = PortLease::default();
        let d = a.vnc_display(0).expect("a display");
        let port = 5900 + d;
        assert!(leased().lock().unwrap().contains(&port), "held by a");
        let mut b = PortLease::default();
        assert_ne!(b.vnc_display(d), Some(d), "b must not get a's display");
        drop(a);
        assert!(
            !leased().lock().unwrap().contains(&port),
            "released on drop"
        );
    }

    #[test]
    fn concurrent_vnc_displays_are_distinct() {
        // Nothing binds them in between, as when several QEMUs start at once.
        let mut lease = PortLease::default();
        let a = lease.vnc_display(40).unwrap();
        let b = lease.vnc_display(40).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn free_ports_are_distinct() {
        let p = free_ports(4).unwrap();
        let mut d = p.clone();
        d.dedup();
        assert_eq!(p.len(), 4);
        assert!(p.iter().all(|x| *x > 0));
    }

    #[test]
    fn accel_is_tcg_for_foreign_arch() {
        let foreign = match Arch::host() {
            Arch::X86_64 => Arch::Aarch64,
            Arch::Aarch64 => Arch::X86_64,
        };
        let denied = Err(std::io::ErrorKind::PermissionDenied.into());
        for os in [HostOs::Linux, HostOs::Macos, HostOs::Windows] {
            assert_eq!(qemu_accel(os, foreign, &denied).unwrap(), "tcg");
        }
        assert_eq!(
            qemu_accel(HostOs::Macos, Arch::host(), &denied).unwrap(),
            "hvf"
        );
        assert_eq!(
            qemu_accel(HostOs::Windows, Arch::host(), &denied).unwrap(),
            "whpx"
        );
    }

    #[test]
    fn native_linux_accel_tells_missing_from_unopenable_kvm() {
        let native = |kvm| qemu_accel_with(HostOs::Linux, Arch::host(), &kvm, || true, |_| {});
        assert_eq!(native(Ok(())).unwrap(), "kvm");
        assert_eq!(
            native(Err(std::io::ErrorKind::NotFound.into())).unwrap(),
            "tcg"
        );
        let missing = kvm_hint(&std::io::ErrorKind::NotFound.into());
        assert!(missing.contains("nested virtualization"), "{missing}");
        let denied = native(Err(std::io::ErrorKind::PermissionDenied.into()));
        let denied = denied.unwrap_err().to_string();
        assert!(denied.contains("permission denied"), "{denied}");
        assert!(
            denied.contains("sudo usermod -aG kvm \"$USER\""),
            "{denied}"
        );
        let mut warning = String::new();
        assert_eq!(
            qemu_accel_with(
                HostOs::Linux,
                Arch::host(),
                &Err(std::io::ErrorKind::PermissionDenied.into()),
                || false,
                |message| warning.push_str(message)
            )
            .unwrap(),
            "tcg"
        );
        assert_eq!(
            warning,
            "KVM access denied and sudo unavailable; using slow TCG emulation"
        );
        let other = native(Err(std::io::Error::other("device busy")))
            .unwrap_err()
            .to_string();
        assert!(other.contains("device busy"), "{other}");
        assert!(!other.contains("usermod"), "{other}");
    }

    /// Regression for #4756: Windows reported every pid as dead.
    #[test]
    fn pid_alive_tracks_a_real_process() {
        assert!(pid_alive(std::process::id()));
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // Reaped (and, on Windows, its last handle closed): gone.
        drop(child);
        assert!(!pid_alive(pid));
    }

    #[test]
    fn product_version_from_the_plist() {
        let plist = "<dict>\n\t<key>ProductName</key>\n\t<string>macOS</string>\n\t\
                     <key>ProductVersion</key>\n\t<string>26.6.2</string>\n</dict>";
        assert_eq!(product_version(plist).as_deref(), Some("26.6.2"));
        assert_eq!(product_version("<dict></dict>"), None);
        #[cfg(target_os = "macos")]
        assert!(macos_major().is_some_and(|m| m >= 11));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(macos_major(), None);
    }

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-binary-cua").is_none());
    }
}
