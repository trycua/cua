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
