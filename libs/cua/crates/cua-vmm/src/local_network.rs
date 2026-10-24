//! Whether *this process* can reach this Mac's own macOS VMs, and which app
//! macOS holds responsible for it.
//!
//! A macOS VM lives on Apple's shared network (192.168.64.x). macOS Local
//! Network privacy blocks a process from it until the app macOS holds
//! responsible for that process is allowed; a blocked connect fails at once
//! with "No route to host" ([`crate::probe::is_unreachable`]). The grant
//! belongs to the responsible app, not to the binary: a `cua` run from
//! Terminal is Terminal's, the daemon the Spaces app starts is the app's. So
//! the check has to run in the process that talks to the VMs (the cua
//! daemon), and it says which app that is ([`ResponsibleApp`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Listing the VMs (`lume serve` is local).
pub const LIST_TIMEOUT: Duration = Duration::from_secs(2);
/// One connect to the VM. A blocked network fails at once; a VM that is
/// there answers (accepts or refuses) well within this.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Any port works: a refusal comes back over the same network.
pub const PROBE_PORT: u16 = 22;

/// What one probe found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Probe {
    /// `lume serve` did not list its VMs in time (or answered an error).
    NoList { why: String },
    /// No macOS VM is running with an address.
    NoRunningVm,
    /// The VM answered (accepted or refused the connection).
    Reached { vm: String, ip: String },
    /// No route to the VM: Local Network access is off for the responsible
    /// app.
    Blocked { vm: String, ip: String },
    /// No answer in time, or another error: inconclusive.
    Unclear { vm: String, ip: String, why: String },
}

/// The app macOS holds responsible for a process (whose Local Network
/// grant applies to it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponsibleApp {
    /// What System Settings lists: the bundle's name without `.app`, else
    /// the executable's file name.
    pub name: String,
    /// The `.app` bundle, else the executable.
    pub path: PathBuf,
    /// Whether `path` is a `.app` bundle (System Settings lists apps; a
    /// bare executable shows up under the app that started it, if at all).
    pub bundle: bool,
}

/// The daemon-side probe as the daemon reports it (`cua doctor` reads it
/// from the daemon's runtime doctor report under `local_network`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// What the probe found.
    pub probe: Probe,
    /// The app macOS holds responsible for the probing process, when known.
    #[serde(default)]
    pub app: Option<ResponsibleApp>,
    /// The probing process.
    #[serde(default)]
    pub pid: u32,
}

/// What one connect says about the network (pure).
pub fn classify(vm: &str, ip: &str, result: Result<std::io::Result<()>, ()>) -> Probe {
    let (vm, ip) = (vm.to_owned(), ip.to_owned());
    match result {
        Ok(Ok(())) => Probe::Reached { vm, ip },
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            Probe::Reached { vm, ip }
        }
        Ok(Err(e)) if crate::probe::is_unreachable(&e) => Probe::Blocked { vm, ip },
        Ok(Err(e)) => Probe::Unclear {
            vm,
            ip,
            why: e.to_string(),
        },
        Err(()) => Probe::Unclear {
            vm,
            ip,
            why: format!("no answer in {} ms", CONNECT_TIMEOUT.as_millis()),
        },
    }
}

/// Lists the VMs `lume serve` runs and connects once, from this process,
/// to the first running macOS one with an address. Bounded: about 3.5 s.
#[cfg(feature = "lume")]
pub async fn probe(lume_base: &str) -> Probe {
    let client = crate::lume::LumeClient::new(lume_base);
    let vms = match tokio::time::timeout(LIST_TIMEOUT, client.list()).await {
        Ok(Ok(vms)) => vms,
        Ok(Err(e)) => return Probe::NoList { why: e.to_string() },
        Err(_) => {
            return Probe::NoList {
                why: format!("no answer in {} s", LIST_TIMEOUT.as_secs()),
            };
        }
    };
    let Some((vm, ip)) = vms.iter().find_map(|v| {
        (v.status == "running" && v.os.eq_ignore_ascii_case("macos"))
            .then(|| v.ip().map(|ip| (v.name.clone(), ip.to_owned())))
            .flatten()
    }) else {
        return Probe::NoRunningVm;
    };
    let addr = format!("{ip}:{PROBE_PORT}");
    let result = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(&addr))
        .await
        .map(|r| r.map(drop))
        .map_err(drop);
    classify(&vm, &ip, result)
}

/// [`probe`] plus who probed: what the daemon reports.
#[cfg(feature = "lume")]
pub async fn report(lume_base: &str) -> Report {
    Report {
        probe: probe(lume_base).await,
        app: ResponsibleApp::of_this_process(),
        pid: std::process::id(),
    }
}

/// Why this process cannot reach the macOS VM at `ip` (a connect to it
/// failed with "No route to host"): a VPN or firewall
/// ([`IssueKind::LocalNetworkVpn`]: the route to it goes through a VPN's
/// tunnel, or this process reaches the rest of the local network) or
/// missing Local Network access ([`IssueKind::LocalNetwork`]: the local
/// network is blocked for this process too). `None` when neither can be
/// told (no local network to compare with). Bounded: about 2 s. Off macOS,
/// always `None`.
///
/// [`IssueKind::LocalNetworkVpn`]: cua_create_failure::preflight::IssueKind::LocalNetworkVpn
/// [`IssueKind::LocalNetwork`]: cua_create_failure::preflight::IssueKind::LocalNetwork
pub async fn diagnose_unreachable(ip: &str) -> Option<cua_create_failure::preflight::IssueKind> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let route = tokio::time::timeout(
        Duration::from_secs(2),
        crate::host::run("/sbin/route", &["-n", "get", ip]),
    )
    .await
    .ok()
    .and_then(Result::ok);
    let iface = route
        .as_deref()
        .and_then(cua_create_failure::local_network::route_interface);
    let lan = match lan_host() {
        Some(host) => lan_access(&host).await,
        None => cua_create_failure::local_network::Access::Unknown,
    };
    let cause = cua_create_failure::local_network::unreachable_cause(iface, lan);
    tracing::info!(
        ip,
        ?iface,
        ?lan,
        ?cause,
        "macOS VM unreachable: local network check"
    );
    cause
}

/// A connect from this process to `host` on the local network: macOS
/// blocks it at once ("No route to host") when Local Network access is off
/// for the app responsible for this process; anything that went out
/// (accepted, refused) means it is on. Port 9 (discard): nothing has to
/// listen.
async fn lan_access(host: &str) -> cua_create_failure::local_network::Access {
    use cua_create_failure::local_network::Access;
    let started = std::time::Instant::now();
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((host, 9u16)),
    )
    .await
    {
        Ok(Ok(_)) => Access::Allowed,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => Access::Allowed,
        // The block is immediate; a slow "no route" is a host that is not
        // there, which says nothing about the permission.
        Ok(Err(e))
            if crate::probe::is_unreachable(&e)
                && started.elapsed() < Duration::from_millis(500) =>
        {
            Access::Blocked
        }
        _ => Access::Unknown,
    }
}

/// The first host of this Mac's first active local IPv4 network (usually
/// its router): not loopback, not link-local, not a VPN's tunnel, not the
/// VMs' own bridge. `None` without one.
#[cfg(unix)]
fn lan_host() -> Option<String> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list` with a linked list we free below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 || list.is_null() {
        return None;
    }
    let mut out = None;
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: `cursor` is a node of the list getifaddrs returned.
        let ifa = unsafe { &*cursor };
        cursor = ifa.ifa_next;
        let flags = ifa.ifa_flags as libc::c_int;
        if flags & libc::IFF_UP == 0
            || flags & libc::IFF_RUNNING == 0
            || flags & libc::IFF_LOOPBACK != 0
            || ifa.ifa_addr.is_null()
            || ifa.ifa_netmask.is_null()
        {
            continue;
        }
        // SAFETY: ifa_addr is non-null and points at a sockaddr.
        if i32::from(unsafe { (*ifa.ifa_addr).sa_family }) != libc::AF_INET {
            continue;
        }
        // SAFETY: ifa_name is a NUL-terminated C string.
        let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }.to_string_lossy();
        if name.starts_with("bridge") || cua_create_failure::local_network::is_tunnel(&name) {
            continue;
        }
        // SAFETY: both are AF_INET sockaddrs (checked above for the address;
        // the mask of an AF_INET address is one too).
        let (ip, mask) = unsafe {
            let a = &*(ifa.ifa_addr as *const libc::sockaddr_in);
            let m = &*(ifa.ifa_netmask as *const libc::sockaddr_in);
            (
                u32::from_be(a.sin_addr.s_addr),
                u32::from_be(m.sin_addr.s_addr),
            )
        };
        if let Some(host) = subnet_host(ip, mask) {
            out = Some(host);
            break;
        }
    }
    // SAFETY: `list` came from getifaddrs.
    unsafe { libc::freeifaddrs(list) };
    out
}

#[cfg(not(unix))]
fn lan_host() -> Option<String> {
    None
}

/// The subnet's first host (`192.168.1.23/24` → `192.168.1.1`, or `.2`
/// when that is this Mac); `None` for link-local or a subnet with no room
/// for another host.
#[cfg_attr(not(unix), allow(dead_code))]
fn subnet_host(ip: u32, mask: u32) -> Option<String> {
    if ip >> 16 == 0xA9FE || mask == 0 || !mask < 2 {
        return None;
    }
    let mut target = (ip & mask) + 1;
    if target == ip {
        target += 1;
    }
    Some(std::net::Ipv4Addr::from(target).to_string())
}

/// The `lume serve` this host's runtimes talk to.
pub fn lume_base() -> String {
    std::env::var("LUME_API").unwrap_or_else(|_| "http://127.0.0.1:7777".into())
}

impl ResponsibleApp {
    /// The app macOS holds responsible for this process: the responsible
    /// process's `.app` (for a daemon an app started, the app; for a CLI,
    /// the terminal), else this executable's own `.app`, else the
    /// executable. `None` off macOS.
    pub fn of_this_process() -> Option<Self> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let exe = std::env::current_exe().ok();
        let responsible = responsible_exe();
        Self::pick(responsible.as_deref(), exe.as_deref())
    }

    /// The responsible executable's bundle, else this one's bundle, else
    /// the responsible executable, else this one (pure).
    fn pick(responsible: Option<&Path>, exe: Option<&Path>) -> Option<Self> {
        responsible
            .and_then(Self::bundle_of)
            .or_else(|| exe.and_then(Self::bundle_of))
            .or_else(|| responsible.or(exe).map(Self::bare))
    }

    /// The `.app` bundle `exe` runs from.
    pub fn bundle_of(exe: &Path) -> Option<Self> {
        exe.ancestors().find_map(|dir| {
            let name = dir.file_name()?.to_str()?;
            let stem = name.strip_suffix(".app")?;
            Some(Self {
                name: stem.to_string(),
                path: dir.to_path_buf(),
                bundle: true,
            })
        })
    }

    fn bare(exe: &Path) -> Self {
        Self {
            name: exe
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| exe.display().to_string()),
            path: exe.to_path_buf(),
            bundle: false,
        }
    }
}

/// The executable of the process macOS holds responsible for this one
/// (`responsibility_get_pid_responsible_for_pid`, which TCC and Local
/// Network privacy use), when it is another process.
#[cfg(target_os = "macos")]
fn responsible_exe() -> Option<PathBuf> {
    unsafe extern "C" {
        // libquarantine, part of libSystem. Not in the SDK headers, but
        // stable since macOS 10.14 and what `launchctl procinfo` shows.
        fn responsibility_get_pid_responsible_for_pid(pid: libc::pid_t) -> libc::pid_t;
    }
    // SAFETY: takes and returns a pid; no memory is shared.
    let pid = unsafe { responsibility_get_pid_responsible_for_pid(libc::getpid()) };
    if pid <= 0 {
        return None;
    }
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is valid for `buf.len()` bytes; proc_pidpath writes at
    // most that many and returns the length written.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok().map(PathBuf::from)
}

#[cfg(not(target_os = "macos"))]
fn responsible_exe() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn connect_results_classify() {
        let blocked = |k| classify("m", "1.2.3.4", Ok(Err(Error::from(k))));
        assert!(matches!(
            blocked(ErrorKind::HostUnreachable),
            Probe::Blocked { .. }
        ));
        assert!(matches!(
            blocked(ErrorKind::NetworkUnreachable),
            Probe::Blocked { .. }
        ));
        // A refusal comes back over the network: reachable.
        assert!(matches!(
            blocked(ErrorKind::ConnectionRefused),
            Probe::Reached { .. }
        ));
        assert!(matches!(
            classify("m", "1.2.3.4", Ok(Ok(()))),
            Probe::Reached { .. }
        ));
        assert!(matches!(
            classify("m", "1.2.3.4", Err(())),
            Probe::Unclear { .. }
        ));
    }

    #[test]
    fn the_lan_probe_aims_at_the_subnets_first_host() {
        let ip = |s: &str| u32::from(s.parse::<std::net::Ipv4Addr>().unwrap());
        assert_eq!(
            subnet_host(ip("192.168.1.23"), ip("255.255.255.0")).as_deref(),
            Some("192.168.1.1")
        );
        assert_eq!(
            subnet_host(ip("10.0.0.1"), ip("255.0.0.0")).as_deref(),
            Some("10.0.0.2")
        );
        assert_eq!(subnet_host(ip("169.254.3.4"), ip("255.255.0.0")), None);
        assert_eq!(subnet_host(ip("10.8.0.6"), ip("255.255.255.255")), None);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn diagnosing_is_bounded_and_never_panics() {
        // Whatever this Mac's network is: an answer (or none) within the
        // route lookup and one connect.
        let started = std::time::Instant::now();
        let _ = diagnose_unreachable("192.168.64.250").await;
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_responsible_apps_bundle_wins_then_this_ones() {
        let app = Path::new("/Applications/Cua Spaces.app/Contents/MacOS/CuaSpacesMac");
        let cli = Path::new("/Applications/Cua Spaces.app/Contents/MacOS/cua");
        let bare = Path::new("/usr/local/bin/cua");
        let terminal =
            Path::new("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal");
        // The daemon the app started: the app.
        let a = ResponsibleApp::pick(Some(app), Some(cli)).unwrap();
        assert_eq!(a.name, "Cua Spaces");
        assert!(a.bundle);
        // A bare binary run from Terminal: Terminal.
        let a = ResponsibleApp::pick(Some(terminal), Some(bare)).unwrap();
        assert_eq!(a.name, "Terminal");
        // Unknown responsible process, but this binary sits in an app.
        let a = ResponsibleApp::pick(None, Some(cli)).unwrap();
        assert_eq!(a.name, "Cua Spaces");
        // Nothing in a bundle: the responsible executable, not a bundle.
        let a = ResponsibleApp::pick(Some(bare), Some(bare)).unwrap();
        assert_eq!((a.name.as_str(), a.bundle), ("cua", false));
        assert!(ResponsibleApp::pick(None, None).is_none());
    }

    #[test]
    fn the_report_round_trips_as_json() {
        let r = Report {
            probe: Probe::Blocked {
                vm: "m1".into(),
                ip: "192.168.64.5".into(),
            },
            app: ResponsibleApp::bundle_of(Path::new("/A/Cua Spaces.app/Contents/MacOS/cua")),
            pid: 7,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["probe"]["result"], "blocked");
        assert_eq!(json["app"]["name"], "Cua Spaces");
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back, r);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_process_has_a_responsible_app() {
        // Whatever ran the tests (a terminal, an IDE, launchd for CI): an
        // executable that exists.
        let a = ResponsibleApp::of_this_process().expect("on macOS");
        assert!(a.path.exists(), "{a:?}");
    }
}
