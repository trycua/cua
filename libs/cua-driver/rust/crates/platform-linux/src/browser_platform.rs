//! Linux identity and endpoint evidence for the first-class browser tools.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::browser::existing_profile_setup_descriptor;
use cua_driver_core::browser::platform::{
    select_isolated_browser_executable, BrowserConsentOutcome, BrowserConsentRequest,
    BrowserPlatform, ExistingProfileSetupOutcome, ExistingProfileSetupRequest, PrepareAction,
    PrepareOutcome, PrepareRequest,
};
use cua_driver_core::browser::refusal::{BrowserRefusal, BrowserRefusalCode};
use cua_driver_core::browser::types::{
    BrowserClassification, BrowserEngineFamily, BrowserProcessRole, BrowserProduct,
    EndpointOwnershipMethod, EndpointOwnershipProof, EndpointTransport, NativeOwnershipMethod,
    NativeOwnershipProof, NativeWindowInfo, OwnedEndpoint, ProcessFingerprint, Rect,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Default)]
pub struct LinuxBrowserPlatform;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn is_helium(name: &str) -> bool {
    std::path::Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("helium"))
}

fn is_chromium(name: &str) -> bool {
    if is_helium(name) {
        return true;
    }
    let name = name.to_ascii_lowercase();
    let products = [
        "chrome", "chromium", "electron", "brave", "edge", "msedge", "vivaldi", "opera", "arc",
        "thorium", "iridium", "yandex",
    ];
    name.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| products.contains(&token))
}

fn is_firefox(name: &str) -> bool {
    name.to_ascii_lowercase().split_whitespace().any(|word| {
        word.rsplit(['/', '\\'])
            .next()
            .unwrap_or(word)
            .trim_end_matches(".exe")
            == "firefox"
    })
}

fn browser_product(identity: &str) -> BrowserProduct {
    if is_helium(identity) {
        return BrowserProduct::Helium;
    }
    let tokens = identity
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let has = |values: &[&str]| tokens.iter().any(|token| values.contains(&token.as_str()));
    if has(&["google"]) && has(&["chrome"]) {
        BrowserProduct::GoogleChrome
    } else if has(&["msedge"]) || (has(&["microsoft"]) && has(&["edge"])) {
        BrowserProduct::MicrosoftEdge
    } else if has(&["chromium"]) {
        BrowserProduct::Chromium
    } else if has(&["brave"]) {
        BrowserProduct::Brave
    } else if has(&["vivaldi"]) {
        BrowserProduct::Vivaldi
    } else if has(&["opera"]) {
        BrowserProduct::Opera
    } else if has(&["electron"]) {
        BrowserProduct::Electron
    } else if has(&["firefox"]) {
        BrowserProduct::Firefox
    } else {
        BrowserProduct::Other
    }
}

fn isolated_browser_candidates() -> Vec<PathBuf> {
    // These are the root-managed payload locations of supported Linux
    // packages, not PATH shims. Core additionally rejects symlinked paths, so
    // an isolated-launch grant cannot be redirected to a user-controlled file.
    [
        "/opt/google/chrome/google-chrome",
        "/usr/lib/chromium/chromium",
        "/usr/lib/chromium-browser/chromium-browser",
        "/opt/microsoft/msedge/msedge",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn trusted_root_owned_installation(executable: &std::path::Path) -> bool {
    if !executable.is_absolute() {
        return false;
    }
    let mut current = Some(executable);
    while let Some(path) = current {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if metadata.file_type().is_symlink() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0
        {
            return false;
        }
        current = path.parent();
    }
    std::fs::metadata(executable)
        .is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0)
}

fn command_line_arguments(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect()
}

fn has_process_type(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| {
        argument
            .strip_prefix("--type=")
            .is_some_and(|kind| !kind.is_empty())
    })
}

/// Recover argv from the single space-joined `/proc/<pid>/cmdline` record
/// observed for Helium, which does not use the usual NUL delimiter.
///
/// This is deliberately narrow. It applies only to a one-record command line,
/// and every caller gates it on the product already resolved from
/// `/proc/<pid>/exe`, so no non-Helium process reaches it. Splitting happens
/// only at a ` --` switch boundary, so a value containing spaces
/// (`--user-data-dir=/tmp/helium profile`) survives intact, and leading
/// positional arguments (`helium https://example.test --flag`) do not defeat
/// recovery. Every other process keeps ordinary NUL semantics.
///
/// The packed encoding is inherently ambiguous: it carries no quoting or
/// length information, so a value that itself contains ` --` is
/// indistinguishable from a following switch. Where that ambiguity bites,
/// this parser prefers the reading that refuses rather than the one that
/// grants access.
fn packed_helium_arguments(bytes: &[u8]) -> Option<Vec<String>> {
    let arguments = command_line_arguments(bytes);
    let [packed] = arguments.as_slice() else {
        return None;
    };
    let (_leading, switches) = packed.split_once(" --")?;
    let mut recovered = Vec::new();
    for switch in switches.split(" --") {
        // Inside one switch record the boundary is already known, so a
        // `--flag value` pair splits at its first space and everything after
        // it is the value. `--flag=value` stays a single argument.
        match switch.split_once(' ') {
            Some((flag, value)) if !flag.contains('=') => {
                recovered.push(format!("--{flag}"));
                recovered.push(value.to_owned());
            }
            _ => recovered.push(format!("--{switch}")),
        }
    }
    Some(recovered)
}

fn packed_helium_process_type(bytes: &[u8]) -> bool {
    // Any recovered `--type=` marks this as a helper, not just a leading one.
    // Chromium emits it first in every helper observed here, but relying on
    // that position would let an unexpected ordering promote a renderer, GPU,
    // or utility process to the main-browser role. Reading a spurious
    // `--type=` out of an exotic value only costs a refusal, so this
    // deliberately errs toward Helper.
    packed_helium_arguments(bytes).is_some_and(|arguments| has_process_type(&arguments))
}

fn user_data_dir_arguments(arguments: &[String]) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        if let Some(path) = argument.strip_prefix("--user-data-dir=") {
            if !path.is_empty() {
                directories.push(PathBuf::from(path));
            }
        } else if argument == "--user-data-dir" {
            if let Some(path) = arguments.get(index + 1).filter(|path| !path.is_empty()) {
                directories.push(PathBuf::from(path));
            }
        }
    }
    directories
}

fn process_role_for_pid(pid: i64, product: BrowserProduct) -> BrowserProcessRole {
    let helper = std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .is_some_and(|bytes| {
            has_process_type(&command_line_arguments(&bytes))
                || (product == BrowserProduct::Helium && packed_helium_process_type(&bytes))
        });
    if helper {
        BrowserProcessRole::Helper
    } else if product == BrowserProduct::Electron {
        BrowserProcessRole::EmbeddedApplication
    } else if matches!(
        product,
        BrowserProduct::GoogleChrome
            | BrowserProduct::Chromium
            | BrowserProduct::Helium
            | BrowserProduct::MicrosoftEdge
            | BrowserProduct::Brave
            | BrowserProduct::Vivaldi
            | BrowserProduct::Opera
            | BrowserProduct::Arc
    ) {
        BrowserProcessRole::StandaloneConsumer
    } else {
        BrowserProcessRole::Unknown
    }
}

fn loopback_websocket_port(url: &str) -> Option<u16> {
    ["ws://127.0.0.1:", "ws://localhost:", "ws://[::1]:"]
        .iter()
        .find_map(|prefix| {
            url.strip_prefix(prefix)?
                .split('/')
                .next()?
                .parse::<u16>()
                .ok()
        })
}

fn parse_proc_net_loopback_listeners(text: &str) -> Vec<(u16, u64)> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 10 || fields[3] != "0A" {
                return None;
            }
            let (address, port) = fields[1].split_once(':')?;
            let loopback = address == "0100007F" || address == "00000000000000000000000001000000";
            if !loopback {
                return None;
            }
            Some((
                u16::from_str_radix(port, 16).ok()?,
                fields[9].parse::<u64>().ok()?,
            ))
        })
        .collect()
}

fn descendant_pids(root: i64, relationships: impl IntoIterator<Item = (i64, i64)>) -> HashSet<i64> {
    let mut children = HashMap::<i64, Vec<i64>>::new();
    for (pid, parent) in relationships {
        children.entry(parent).or_default().push(pid);
    }
    let mut family = HashSet::from([root]);
    let mut pending = VecDeque::from([root]);
    while let Some(parent) = pending.pop_front() {
        for child in children.get(&parent).into_iter().flatten() {
            if family.insert(*child) {
                pending.push_back(*child);
            }
        }
    }
    family
}

fn process_family_pids(root: i64) -> Result<HashSet<i64>, BrowserRefusal> {
    if !std::path::Path::new(&format!("/proc/{root}")).exists() {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {root} is no longer available"),
        ));
    }
    let relationships = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_string_lossy().parse::<i64>().ok()?;
            let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
            let parent = status
                .lines()
                .find_map(|line| line.strip_prefix("PPid:"))?
                .trim()
                .parse::<i64>()
                .ok()?;
            Some((pid, parent))
        });
    Ok(descendant_pids(root, relationships))
}

fn socket_inodes_for_process_tree(pid: i64) -> Result<HashSet<u64>, BrowserRefusal> {
    let process_family = process_family_pids(pid)?;
    let mut inodes = HashSet::new();
    for owner_pid in process_family {
        let Ok(directory) = std::fs::read_dir(format!("/proc/{owner_pid}/fd")) else {
            continue;
        };
        inodes.extend(
            directory
                .flatten()
                .filter_map(|entry| std::fs::read_link(entry.path()).ok())
                .filter_map(|target| {
                    let target = target.to_string_lossy();
                    target
                        .strip_prefix("socket:[")
                        .and_then(|value| value.strip_suffix(']'))
                        .and_then(|value| value.parse::<u64>().ok())
                }),
        );
    }
    Ok(inodes)
}

fn loopback_ports_for_pid(pid: i64) -> Result<Vec<u16>, BrowserRefusal> {
    // Chromium may delegate its DevTools listener to a utility child. Core's
    // ownership contract explicitly accepts the approved browser PID or one
    // of its children, so inspect the bounded descendant tree as well as the
    // root process while still attributing the result to the approved root.
    let owned = socket_inodes_for_process_tree(pid)?;
    let mut listeners = Vec::new();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(path) {
            listeners.extend(
                parse_proc_net_loopback_listeners(&text)
                    .into_iter()
                    .filter(|(_, inode)| owned.contains(inode))
                    .map(|(port, _)| port),
            );
        }
    }
    listeners.sort_unstable();
    listeners.dedup();
    Ok(listeners)
}

fn parse_devtools_active_port(text: &str) -> Option<(u16, &str)> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let port = lines.next()?.parse::<u16>().ok()?;
    let path = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    let instance = path.strip_prefix("/devtools/browser/")?;
    (!instance.is_empty()
        && instance
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then_some((port, path))
}

fn default_user_data_dir(product: BrowserProduct) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let relative = match product {
        BrowserProduct::GoogleChrome => ".config/google-chrome",
        BrowserProduct::MicrosoftEdge => ".config/microsoft-edge",
        BrowserProduct::Chromium => ".config/chromium",
        BrowserProduct::Helium => ".config/net.imput.helium",
        _ => return None,
    };
    Some(home.join(relative))
}

/// Corroborate a candidate profile directory against the kernel's own view of
/// the process.
///
/// A Chromium profile is not merely named on the command line; the running
/// browser keeps files inside it open. Those descriptors, and the process
/// working directory, are recorded by the kernel and cannot be forged by the
/// contents of an argument value, which makes them independent evidence that a
/// directory really is this process's profile.
fn process_uses_directory(pid: i64, directory: &std::path::Path) -> bool {
    if std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd.starts_with(directory)) {
        return true;
    }
    std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            std::fs::read_link(entry.path()).is_ok_and(|target| target.starts_with(directory))
        })
    })
}

fn user_data_dir_for_pid(pid: i64) -> Result<Option<PathBuf>, BrowserRefusal> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} is no longer available"),
        )
    })?;
    let executable = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    let product = executable
        .as_deref()
        .map(browser_product)
        .unwrap_or(BrowserProduct::Other);
    let mut directories = user_data_dir_arguments(&command_line_arguments(&bytes));
    // A NUL-delimited command line carries the kernel's own argument
    // separators, so its switches are authoritative. A packed record has lost
    // them: ordinary text inside a file name or URL is then indistinguishable
    // from a real switch, so anything recovered from it is a candidate to be
    // corroborated rather than a fact to be trusted.
    let mut candidate_is_unseparated = false;
    if directories.is_empty() && product == BrowserProduct::Helium {
        if let Some(packed) = packed_helium_arguments(&bytes) {
            directories = user_data_dir_arguments(&packed);
            candidate_is_unseparated = !directories.is_empty();
        }
    }
    directories.sort();
    directories.dedup();
    let resolved = match directories.as_slice() {
        [] => Ok(default_user_data_dir(product)),
        [path] if path.is_absolute() => Ok(Some(path.clone())),
        [path] => {
            let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).map_err(|_| {
                refusal(
                    BrowserRefusalCode::BrowserBindingStale,
                    format!("browser process {pid} working directory is unavailable"),
                )
            })?;
            Ok(Some(cwd.join(path)))
        }
        _ => Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "browser process has multiple distinct --user-data-dir arguments",
        )),
    }?;
    if candidate_is_unseparated {
        let Some(directory) = resolved.as_deref() else {
            return Ok(None);
        };
        if !process_uses_directory(pid, directory) {
            return Err(refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "the browser's command line has no argument separators, so a --user-data-dir value read from it could not be distinguished from ordinary argument text and the process does not hold that directory open",
            ));
        }
    }
    Ok(resolved)
}

fn active_port_endpoint(pid: i64) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
    let Some(user_data_dir) = user_data_dir_for_pid(pid)? else {
        return Ok(None);
    };
    let text = match std::fs::read_to_string(user_data_dir.join("DevToolsActivePort")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not read the browser's DevToolsActivePort file: {error}"),
            ))
        }
    };
    let Some((port, path)) = parse_devtools_active_port(&text) else {
        return Err(refusal(
            BrowserRefusalCode::BrowserEndpointOwnerMismatch,
            "the browser's DevToolsActivePort file did not contain one exact browser endpoint",
        ));
    };
    if !loopback_ports_for_pid(pid)?.contains(&port) {
        return Ok(None);
    }
    Ok(Some(OwnedEndpoint {
        ws_url: format!("ws://127.0.0.1:{port}{path}"),
        http_port: Some(port),
        transport: EndpointTransport::DevToolsActivePort,
        ownership: EndpointOwnershipProof {
            method: EndpointOwnershipMethod::DevtoolsActivePortsFile,
            owner_pid: pid,
            listener_pid: None,
            detail: Some(
                "exact /proc argv profile port file plus loopback socket inode owner".to_owned(),
            ),
        },
    }))
}

fn process_identity(pid: i64) -> Result<(u64, Option<String>), BrowserRefusal> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} is no longer available"),
        )
    })?;
    let tail = stat
        .rsplit_once(')')
        .map(|(_, tail)| tail.trim())
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not parse process identity for pid {pid}"),
            )
        })?;
    let started = tail
        .split_whitespace()
        .nth(19)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not parse process start time for pid {pid}"),
            )
        })?;
    let executable = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    Ok((started, executable))
}

async fn browser_websocket_url(port: u16) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(2), async move {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .ok()?;
        let request = format!(
            "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read = stream.read(&mut chunk).await.ok()?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            if bytes.len() > 256 * 1024 {
                return None;
            }
            if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                if let Some(length) = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                }) {
                    if bytes.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
        }
        let body_start = bytes.windows(4).position(|part| part == b"\r\n\r\n")? + 4;
        let value: serde_json::Value = serde_json::from_slice(&bytes[body_start..]).ok()?;
        let url = value.get("webSocketDebuggerUrl")?.as_str()?.to_owned();
        (loopback_websocket_port(&url) == Some(port)).then_some(url)
    })
    .await
    .ok()
    .flatten()
}

#[async_trait]
impl BrowserPlatform for LinuxBrowserPlatform {
    fn isolated_browser_executable(&self) -> Result<String, BrowserRefusal> {
        for candidate in isolated_browser_candidates() {
            let Ok(executable) = select_isolated_browser_executable([candidate]) else {
                continue;
            };
            if trusted_root_owned_installation(std::path::Path::new(&executable)) {
                return Ok(executable);
            }
        }
        Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "no root-owned, non-writable system Chromium executable is available for isolated launch",
        ))
    }

    fn standalone_trusted_input_background_limitation(&self) -> Option<&'static str> {
        Some("Chromium's trusted CDP Input route activates its standalone browser window on Linux")
    }

    async fn classify_browser(&self, pid: i64) -> Result<BrowserClassification, BrowserRefusal> {
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Linux process-id range"),
            )
        })?;
        let (process, executable) = tokio::task::spawn_blocking(move || {
            let process = crate::proc_fs::list_processes()
                .into_iter()
                .find(|process| process.pid == pid_u32);
            let executable = std::fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .map(|path| path.to_string_lossy().into_owned());
            (process, executable)
        })
        .await
        .ok()
        .and_then(|(process, executable)| process.map(|process| (process, executable)))
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                format!("browser process {pid} is no longer available"),
            )
        })?;
        let identity = executable
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| process.name.clone());
        let chromium = is_chromium(&identity);
        let gecko = is_firefox(&identity);
        let product_kind = browser_product(&identity);
        Ok(BrowserClassification {
            is_browser: chromium || gecko,
            engine: if chromium {
                BrowserEngineFamily::Chromium
            } else if gecko {
                BrowserEngineFamily::Gecko
            } else {
                BrowserEngineFamily::Unknown
            },
            product_kind,
            product: Some(process.name),
            channel: None,
            process_role: process_role_for_pid(pid, product_kind),
            supports_cdp: chromium,
        })
    }

    async fn native_window(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<NativeWindowInfo, BrowserRefusal> {
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Linux process-id range"),
            )
        })?;
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            if let Some(window) = crate::wayland::sway_ipc::window_for_id(window_id) {
                if window.pid != pid_u32 {
                    return Err(refusal(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        format!("Sway window {window_id} is not owned by pid {pid}"),
                    ));
                }
                return Ok(NativeWindowInfo {
                    pid,
                    window_id,
                    title: window.title,
                    bounds: Rect::new(
                        f64::from(window.x),
                        f64::from(window.y),
                        f64::from(window.width),
                        f64::from(window.height),
                    ),
                    geometry_exact: true,
                    ownership: NativeOwnershipProof {
                        method: NativeOwnershipMethod::WindowServerOwner,
                        owner_pid: pid,
                        detail: Some("Sway IPC pid and rect".to_owned()),
                    },
                });
            }
            if let Some(window) =
                crate::wayland::shell_helper::trusted_window_for_id(pid_u32, window_id)
            {
                return Ok(NativeWindowInfo {
                    pid,
                    window_id,
                    title: window.title,
                    bounds: Rect::new(
                        f64::from(window.x),
                        f64::from(window.y),
                        f64::from(window.width),
                        f64::from(window.height),
                    ),
                    geometry_exact: true,
                    ownership: NativeOwnershipProof {
                        method: NativeOwnershipMethod::PlatformAttested,
                        owner_pid: pid,
                        detail: Some(
                            "verified GNOME Shell helper owner, stable window id, pid, and frame rect"
                                .to_owned(),
                        ),
                    },
                });
            }
            let window = tokio::task::spawn_blocking(move || {
                crate::wayland::list_windows_dispatch(Some(pid_u32))
                    .into_iter()
                    .find(|window| window.xid == window_id)
            })
            .await
            .ok()
            .flatten()
            .ok_or_else(|| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    "the active Wayland compositor cannot prove this window's pid and geometry",
                )
            })?;
            return Ok(NativeWindowInfo {
                pid,
                window_id,
                title: window.title,
                bounds: Rect::new(
                    f64::from(window.x),
                    f64::from(window.y),
                    f64::from(window.width),
                    f64::from(window.height),
                ),
                geometry_exact: false,
                ownership: NativeOwnershipProof {
                    method: NativeOwnershipMethod::PlatformAttested,
                    owner_pid: pid,
                    detail: Some(
                        "Wayland shell/AT-SPI metadata; geometry may be unavailable, so core permits read-only heuristic binding only"
                            .to_owned(),
                    ),
                },
            });
        }

        let window = tokio::task::spawn_blocking(move || {
            crate::x11::list_windows(Some(pid_u32))
                .into_iter()
                .find(|window| window.xid == window_id)
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                format!("X11 window {window_id} is not owned by pid {pid}"),
            )
        })?;
        Ok(NativeWindowInfo {
            pid,
            window_id,
            title: window.title,
            bounds: Rect::new(
                f64::from(window.x),
                f64::from(window.y),
                f64::from(window.width),
                f64::from(window.height),
            ),
            geometry_exact: true,
            ownership: NativeOwnershipProof {
                method: NativeOwnershipMethod::PlatformAttested,
                owner_pid: pid,
                detail: Some(
                    "X11 _NET_WM_PID corroborated independently by the owned endpoint pid"
                        .to_owned(),
                ),
            },
        })
    }

    async fn is_only_exact_native_window(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<Option<bool>, BrowserRefusal> {
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Linux process-id range"),
            )
        })?;
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            let Some(windows) = crate::wayland::sway_ipc::list_windows() else {
                if let Some(owned) =
                    crate::wayland::shell_helper::trusted_window_ids_for_pid(pid_u32)
                {
                    return Ok(Some(owned.len() == 1 && owned[0] == window_id));
                }
                if crate::wayland::is_inject_mode() {
                    // The private cua-compositor route owns both the native
                    // toplevel enumeration and its PID correlation. Unlike a
                    // generic AT-SPI-only Wayland session, that is sufficient
                    // to attest singleton native-window cardinality for an
                    // embedded Chromium endpoint.
                    let owned = tokio::task::spawn_blocking(move || {
                        crate::wayland::list_windows_dispatch(Some(pid_u32))
                            .into_iter()
                            .filter(|window| window.pid == Some(pid_u32))
                            .map(|window| window.xid)
                            .collect::<Vec<_>>()
                    })
                    .await
                    .map_err(|error| {
                        refusal(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            format!("could not enumerate cua-compositor browser windows: {error}"),
                        )
                    })?;
                    return Ok(Some(owned.len() == 1 && owned[0] == window_id));
                }
                return Ok(None);
            };
            let owned = windows
                .into_iter()
                .filter(|window| window.pid == pid_u32)
                .map(|window| window.id)
                .collect::<Vec<_>>();
            return Ok(Some(owned.len() == 1 && owned[0] == window_id));
        }
        let owned = tokio::task::spawn_blocking(move || {
            crate::x11::list_windows(Some(pid_u32))
                .into_iter()
                .map(|window| u64::from(window.xid))
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not enumerate X11 browser windows: {error}"),
            )
        })?;
        Ok(Some(owned.len() == 1 && owned[0] == window_id))
    }

    async fn discover_owned_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        let ports = tokio::task::spawn_blocking(move || loopback_ports_for_pid(pid))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("listener inspection task failed: {error}"),
                )
            })??;
        for port in ports {
            if let Some(ws_url) = browser_websocket_url(port).await {
                return Ok(Some(OwnedEndpoint {
                    ws_url,
                    http_port: Some(port),
                    transport: EndpointTransport::LegacyJsonVersion,
                    ownership: EndpointOwnershipProof {
                        method: EndpointOwnershipMethod::ListeningSocketPid,
                        owner_pid: pid,
                        listener_pid: None,
                        detail: Some("/proc socket inode owner".to_owned()),
                    },
                }));
            }
        }
        Ok(None)
    }

    async fn discover_existing_profile_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        if let Some(endpoint) = active_port_endpoint(pid)? {
            return Ok(Some(endpoint));
        }
        // Helium's documented Linux boundary is its own PID-owned
        // DevToolsActivePort file. Without it, refuse instead of accepting an
        // arbitrary PID-owned listener through generic /json/version
        // discovery. Classification is deliberately deferred to here so the
        // ordinary Chrome/Edge path does not pay a full /proc enumeration.
        if self.classify_browser(pid).await?.product_kind == BrowserProduct::Helium {
            return Ok(None);
        }
        let ports = tokio::task::spawn_blocking(move || loopback_ports_for_pid(pid))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("listener inspection task failed: {error}"),
                )
            })??;
        let mut discovered = Vec::new();
        for port in ports {
            if let Some(ws_url) = browser_websocket_url(port).await {
                discovered.push((port, ws_url));
            }
        }
        match discovered.as_slice() {
            [] => Ok(None),
            [(port, ws_url)] => Ok(Some(OwnedEndpoint {
                ws_url: ws_url.clone(),
                http_port: Some(*port),
                transport: EndpointTransport::LegacyJsonVersion,
                ownership: EndpointOwnershipProof {
                    method: EndpointOwnershipMethod::ListeningSocketPid,
                    owner_pid: pid,
                    listener_pid: None,
                    detail: Some("/proc owner plus /json/version".to_owned()),
                },
            })),
            _ => Err(refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "multiple browser-level DevTools endpoints are owned by the approved process",
            )),
        }
    }

    async fn reprove_existing_profile_endpoint(
        &self,
        pid: i64,
        expected_ws_url: &str,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        let Some(port) = loopback_websocket_port(expected_ws_url) else {
            return Err(refusal(
                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                "the approved existing-profile endpoint is not loopback-only",
            ));
        };
        if let Some(endpoint) = active_port_endpoint(pid)? {
            if endpoint.ws_url != expected_ws_url {
                return Err(refusal(
                    BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                    "the browser's exact DevTools endpoint changed after approval",
                ));
            }
            return Ok(Some(endpoint));
        }
        if self.classify_browser(pid).await?.product_kind == BrowserProduct::Helium {
            return Ok(None);
        }
        let ports = tokio::task::spawn_blocking(move || loopback_ports_for_pid(pid))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("listener inspection task failed: {error}"),
                )
            })??;
        if !ports.contains(&port) {
            return Ok(None);
        }
        Ok(Some(OwnedEndpoint {
            ws_url: expected_ws_url.to_owned(),
            http_port: Some(port),
            transport: EndpointTransport::LegacyJsonVersion,
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
                listener_pid: None,
                detail: Some("/proc owner of exact approved endpoint".to_owned()),
            },
        }))
    }

    async fn setup_existing_profile_endpoint(
        &self,
        request: ExistingProfileSetupRequest,
    ) -> Result<ExistingProfileSetupOutcome, BrowserRefusal> {
        let descriptor = existing_profile_setup_descriptor(request.browser).ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!(
                    "approved existing-profile setup is not implemented for {:?}",
                    request.browser
                ),
            )
        })?;
        let pid_u32 = u32::try_from(request.pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser pid is outside the Linux process-id range",
            )
        })?;
        // Window cardinality is deliberately NOT a precondition here. Setup acts
        // inside one named window, and `browser_setup_ui` proves that window's
        // identity directly by resolving the native window to exactly one
        // accessibility top-level before it matches or actuates any control. A
        // process owning several windows is the ordinary case for a browser and
        // says nothing about whether the driver can act precisely; refusing on
        // it made the whole existing-profile route unreachable for most real
        // sessions. Where identity genuinely cannot be established — a generic
        // Wayland session with no compositor window list — that same proof fails
        // and setup refuses with a reason that names the cause.
        let window_id = request.window_id;
        let listeners_before =
            tokio::task::spawn_blocking(move || loopback_ports_for_pid(request.pid))
                .await
                .map_err(|error| {
                    refusal(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        format!("listener inspection task failed: {error}"),
                    )
                })??;
        let handle = tokio::task::spawn_blocking(move || {
            crate::browser_setup_ui::enable(pid_u32, window_id, descriptor)
        })
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!(
                    "could not inspect {}'s remote-debugging setup UI: {error}",
                    descriptor.product_name
                ),
            )
        })??;
        let opened_setup_page = handle.opened_setup_page;
        let enabled_remote_debugging = handle.enabled_remote_debugging;
        let focused_setup_address_field = handle.focused_setup_address_field;
        let foregrounded_window = handle.foregrounded_window;
        let injected_global_input = handle.injected_global_input;

        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        let endpoint_result = loop {
            match active_port_endpoint(request.pid) {
                Ok(Some(endpoint)) => break Ok(endpoint),
                Ok(None) => {}
                Err(error) => break Err(error),
            }
            let ports = match tokio::task::spawn_blocking(move || {
                loopback_ports_for_pid(request.pid)
            })
            .await
            {
                Ok(Ok(ports)) => ports,
                Ok(Err(error)) => break Err(error),
                Err(error) => {
                    break Err(refusal(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        format!("listener inspection task failed: {error}"),
                    ))
                }
            };
            let mut endpoints = Vec::new();
            for port in &ports {
                if let Some(ws_url) = browser_websocket_url(*port).await {
                    endpoints.push((*port, ws_url, "/proc owner plus /json/version"));
                }
            }
            if endpoints.is_empty() {
                let correlated = ports
                    .iter()
                    .copied()
                    .filter(|port| !listeners_before.contains(port))
                    .collect::<Vec<_>>();
                if let [port] = correlated.as_slice() {
                    // Chromium's consent-gated server intentionally disables
                    // `/json/*` discovery and accepts the stable browser route
                    // without a UUID. The exact setup action plus the newly
                    // PID-owned listener proves which approval server this is.
                    endpoints.push((
                        *port,
                        format!("ws://127.0.0.1:{port}/devtools/browser"),
                        "new PID-owned approval listener correlated with exact setup",
                    ));
                } else if correlated.len() > 1 {
                    break Err(refusal(
                        BrowserRefusalCode::BrowserBindingAmbiguous,
                        format!(
                            "{} exposed multiple newly correlated PID-owned listeners",
                            descriptor.product_name
                        ),
                    ));
                }
            }
            match endpoints.as_slice() {
                [(port, ws_url, detail)] => {
                    break Ok(OwnedEndpoint {
                        ws_url: ws_url.clone(),
                        http_port: Some(*port),
                        transport: EndpointTransport::LegacyJsonVersion,
                        ownership: EndpointOwnershipProof {
                            method: EndpointOwnershipMethod::ListeningSocketPid,
                            owner_pid: request.pid,
                            listener_pid: None,
                            detail: Some((*detail).to_owned()),
                        },
                    })
                }
                [] if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                [] => {
                    break Err(refusal(
                        BrowserRefusalCode::BrowserRequiresSetup,
                        format!(
                            "{} did not expose a uniquely PID-owned loopback endpoint after the exact setup action",
                            descriptor.product_name
                        ),
                    ))
                }
                _ => {
                    break Err(refusal(
                        BrowserRefusalCode::BrowserBindingAmbiguous,
                        format!(
                            "{} exposed multiple PID-owned endpoint candidates after the exact setup action",
                            descriptor.product_name
                        ),
                    ))
                }
            }
        };
        let endpoint = match endpoint_result {
            Ok(endpoint) => endpoint,
            // The toggle took, but no endpoint appeared. That is Chromium's
            // documented-by-behaviour semantics rather than a failure: "Allow
            // remote debugging for this browser instance" persists a preference
            // that the browser acts on at its NEXT launch — verified on Chrome
            // 151 by enabling it and observing no listener for 25s, then a
            // listener on 127.0.0.1 plus a DevToolsActivePort file immediately
            // after a restart with no --remote-debugging-port flag.
            //
            // Rolling back here erased the setting moments before the restart
            // that would activate it, so this path could never succeed. Keep the
            // preference, close the temporary tab, and tell the caller the one
            // thing it needs to do. The profile is armed, not broken.
            Err(error)
                if enabled_remote_debugging
                    && error.code == BrowserRefusalCode::BrowserRequiresSetup =>
            {
                let closed = tokio::task::spawn_blocking(move || handle.close_for_success())
                    .await
                    .map_err(|join_error| {
                        refusal(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            format!("could not finish browser setup cleanup: {join_error}"),
                        )
                    })??;
                return Err(refusal(
                    BrowserRefusalCode::BrowserRequiresSetup,
                    format!(
                        "remote debugging is now enabled for this {} profile, but {} only opens \
                         its debugging endpoint at startup. Restart the browser, then call \
                         browser_prepare again — the setting persists and does not need to be \
                         applied twice.",
                        descriptor.product_name, descriptor.product_name
                    ),
                )
                .with_detail(serde_json::json!({
                    "restart_required": true,
                    "remote_debugging_enabled": true,
                    "setup_side_effects": {
                        "opened_setup_page": opened_setup_page,
                        "closed_setup_page": closed.unwrap_or(false),
                        "enabled_remote_debugging": true,
                        "restored_remote_debugging": false,
                        "focused_setup_address_field": focused_setup_address_field,
                        "foregrounded_window": foregrounded_window,
                        "injected_global_input": injected_global_input,
                    },
                })));
            }
            Err(error) => {
                let error = tokio::task::spawn_blocking(move || handle.abort(error))
                    .await
                    .map_err(|join_error| {
                        refusal(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            format!("could not roll back browser setup: {join_error}"),
                        )
                    })?;
                return Err(error);
            }
        };
        crate::browser_setup_ui::retain_pending(pid_u32, window_id, handle)?;

        Ok(ExistingProfileSetupOutcome {
            opened_setup_page,
            closed_setup_page: false,
            enabled_remote_debugging,
            used_bounded_pixel_fallback: false,
            focused_setup_address_field,
            foregrounded_window,
            injected_global_input,
            endpoint: Some(endpoint),
        })
    }

    async fn commit_existing_profile_setup(
        &self,
        request: ExistingProfileSetupRequest,
    ) -> Result<bool, BrowserRefusal> {
        let pid = u32::try_from(request.pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser pid is outside the Linux process-id range",
            )
        })?;
        tokio::task::spawn_blocking(move || {
            crate::browser_setup_ui::commit_pending(pid, request.window_id)
        })
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not commit exact browser setup cleanup: {error}"),
            )
        })?
    }

    async fn abort_existing_profile_setup(
        &self,
        request: ExistingProfileSetupRequest,
        error: BrowserRefusal,
    ) -> BrowserRefusal {
        let Ok(pid) = u32::try_from(request.pid) else {
            return error;
        };
        tokio::task::spawn_blocking(move || {
            crate::browser_setup_ui::abort_pending(pid, request.window_id, error)
        })
        .await
        .unwrap_or_else(|join_error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not roll back exact browser setup: {join_error}"),
            )
        })
    }

    async fn handle_existing_profile_consent(
        &self,
        request: BrowserConsentRequest,
    ) -> Result<BrowserConsentOutcome, BrowserRefusal> {
        crate::browser_consent_ui::handle(request).await
    }

    async fn process_fingerprint(&self, pid: i64) -> Result<ProcessFingerprint, BrowserRefusal> {
        let (start_time, executable) = tokio::task::spawn_blocking(move || process_identity(pid))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("process fingerprint task failed: {error}"),
                )
            })??;
        Ok(ProcessFingerprint {
            pid,
            start_time: Some(start_time),
            executable,
        })
    }

    async fn prepare_endpoint(
        &self,
        request: PrepareRequest,
    ) -> Result<PrepareOutcome, BrowserRefusal> {
        let Some(pid) = request.pid else {
            return Err(refusal(
                BrowserRefusalCode::BrowserRequiresSetup,
                "pid-free isolated launch is handled by shared core",
            ));
        };
        if let Some(endpoint) = self.discover_owned_endpoint(pid).await? {
            return Ok(PrepareOutcome {
                action: PrepareAction::AlreadyPrepared,
                prepared_pid: Some(endpoint.ownership.owner_pid),
                endpoint: Some(endpoint),
                message: "An owned loopback DevTools endpoint is already available.".to_owned(),
                side_effects: Default::default(),
                attachment: None,
            });
        }
        Err(refusal(
            BrowserRefusalCode::BrowserRequiresSetup,
            "No owned endpoint is available. Acting setup is handled by shared core only for a verified driver-owned isolated profile.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_browser_candidates_use_only_root_managed_payloads() {
        let candidates = isolated_browser_candidates();
        assert!(candidates.iter().all(|candidate| candidate.is_absolute()));
        assert_eq!(
            candidates,
            [
                "/opt/google/chrome/google-chrome",
                "/usr/lib/chromium/chromium",
                "/usr/lib/chromium-browser/chromium-browser",
                "/opt/microsoft/msedge/msedge",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn user_owned_writable_candidate_is_not_a_trusted_installation() {
        let root = tempfile::tempdir().expect("temporary untrusted installation");
        let executable = root.path().join("google-chrome");
        std::fs::write(&executable, b"not a browser").expect("untrusted executable fixture");
        assert!(!trusted_root_owned_installation(&executable));
    }

    #[test]
    fn proc_net_parser_returns_only_loopback_listeners() {
        let input = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:2406 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000 0 12345\n\
   1: 00000000:2475 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000 0 67890\n";
        assert_eq!(
            parse_proc_net_loopback_listeners(input),
            vec![(9222, 12345)]
        );
    }

    #[test]
    fn process_family_contains_only_transitive_descendants() {
        assert_eq!(
            descendant_pids(10, [(11, 10), (12, 11), (20, 1), (21, 20)]),
            HashSet::from([10, 11, 12])
        );
    }

    #[test]
    fn classifier_covers_embedded_and_standalone_chromium() {
        assert!(is_chromium("CuaTestHarness.Electron"));
        assert!(is_chromium("chromium-browser"));
        assert!(is_chromium("msedge --remote-debugging-port=9222"));
        assert!(is_chromium("/opt/microsoft/msedge/msedge"));
        assert!(is_chromium("helium"));
        assert!(is_chromium("/opt/helium/helium"));
        assert!(is_chromium("/opt/helium-browser-bin/helium"));
        assert!(!is_chromium("/opt/helium/not-a-browser"));
        assert!(!is_chromium("helium-helper"));
        assert!(!is_chromium(r"/opt/fake\helium"));
        assert!(!is_chromium("firefox"));
        assert!(!is_chromium("search-worker"));
        assert!(!is_chromium("ledger-service"));
    }

    #[test]
    fn product_classification_uses_executable_identity_not_page_arguments() {
        assert_eq!(
            browser_product("/usr/lib/firefox/firefox"),
            BrowserProduct::Firefox
        );
        assert_eq!(
            browser_product("/snap/chromium/current/usr/lib/chromium-browser/chrome"),
            BrowserProduct::Chromium
        );
        assert_eq!(
            browser_product("/opt/google/chrome/chrome"),
            BrowserProduct::GoogleChrome
        );
        assert_eq!(
            browser_product("/opt/helium-browser-bin/helium"),
            BrowserProduct::Helium
        );
        assert_eq!(
            browser_product("/opt/helium/helium"),
            BrowserProduct::Helium
        );
        assert_eq!(
            browser_product("/opt/chromium/helium"),
            BrowserProduct::Helium
        );
        assert_eq!(
            browser_product("/opt/helium/not-a-browser"),
            BrowserProduct::Other
        );
        assert_eq!(browser_product("helium-helper"), BrowserProduct::Other);
    }

    #[test]
    fn helium_uses_its_branded_default_profile_directory() {
        assert!(default_user_data_dir(BrowserProduct::Helium)
            .expect("HOME-backed Helium profile directory")
            .ends_with(".config/net.imput.helium"));
    }

    #[test]
    fn process_type_detection_handles_nul_and_packed_command_lines() {
        assert!(has_process_type(&command_line_arguments(
            b"helium\0--type=renderer\0"
        )));
        assert!(packed_helium_process_type(
            b"/opt/helium-browser-bin/helium --type=gpu-process --utility-sub-type=test\0"
        ));
        // Packed records are never split on whitespace by the NUL parser.
        assert!(!has_process_type(&command_line_arguments(
            b"/opt/helium-browser-bin/helium --type=gpu-process --utility-sub-type=test\0"
        )));
        // A helper is a helper wherever `--type=` appears. Position is not a
        // load-bearing assumption: an unexpected ordering must not promote a
        // renderer, GPU, or utility process to the main-browser role.
        assert!(packed_helium_process_type(
            b"/opt/helium-browser-bin/helium --enable-logging --type=renderer\0"
        ));
        assert!(packed_helium_process_type(
            b"/opt/helium-browser-bin/helium --load-extension=/tmp/example --type=renderer\0"
        ));
        // A real main process carries no `--type=` at all.
        assert!(!packed_helium_process_type(
            b"/opt/helium-browser-bin/helium --user-data-dir=/tmp/helium profile --no-first-run\0"
        ));
        assert!(!packed_helium_process_type(b"helium --type=\0"));
        assert!(!packed_helium_process_type(
            b"/opt/helium-browser-bin/helium\0--type=renderer\0"
        ));
    }

    #[test]
    fn packed_helium_command_line_preserves_profile_arguments() {
        assert_eq!(
            command_line_arguments(
                b"/opt/helium-browser-bin/helium --user-data-dir=/tmp/helium profile\0"
            ),
            ["/opt/helium-browser-bin/helium --user-data-dir=/tmp/helium profile"]
        );
        assert_eq!(
            command_line_arguments(
                b"/opt/helium-browser-bin/helium\0--user-data-dir\0/tmp/helium-profile\0"
            ),
            [
                "/opt/helium-browser-bin/helium",
                "--user-data-dir",
                "/tmp/helium-profile"
            ]
        );
    }

    #[test]
    fn packed_helium_recovery_keeps_switch_boundaries_and_spaced_values() {
        let packed = packed_helium_arguments(
            b"/opt/helium-browser-bin/helium --user-data-dir=/tmp/helium profile --remote-debugging-port=0\0",
        )
        .expect("packed Helium argv");
        assert_eq!(
            packed,
            [
                "--user-data-dir=/tmp/helium profile",
                "--remote-debugging-port=0"
            ]
        );
        assert_eq!(
            user_data_dir_arguments(&packed),
            [PathBuf::from("/tmp/helium profile")]
        );
        // The separated form resolves through the same shared extractor.
        assert_eq!(
            user_data_dir_arguments(
                &packed_helium_arguments(
                    b"/opt/helium-browser-bin/helium --user-data-dir /tmp/helium profile --no-first-run\0"
                )
                .expect("packed Helium argv")
            ),
            [PathBuf::from("/tmp/helium profile")]
        );
        // NUL-delimited records are handled by the ordinary parser instead.
        assert!(
            packed_helium_arguments(b"/opt/helium-browser-bin/helium\0--no-first-run\0").is_none()
        );
        assert!(packed_helium_arguments(b"/opt/helium-browser-bin/helium\0").is_none());
    }

    #[test]
    fn ordinary_argument_text_cannot_select_the_profile_directory() {
        // Helium replaces the kernel's argument separators with spaces, so an
        // ordinary file name or URL containing " --user-data-dir=" is textually
        // identical to a real switch. This process supplied no profile option
        // at all, and does not hold the forged directory open, so the value
        // must be refused rather than accepted as the profile.
        let attack = b"/opt/helium-browser-bin/helium /tmp/report --user-data-dir=/tmp/forged\0";
        let recovered =
            user_data_dir_arguments(&packed_helium_arguments(attack).expect("packed argv"));
        assert_eq!(recovered, [PathBuf::from("/tmp/forged")]);

        // The raw parse is deliberately credulous; the corroboration step is
        // what makes it safe. This process holds no descriptor under the
        // forged directory, so it is rejected.
        let pid = i64::from(std::process::id());
        assert!(!process_uses_directory(
            pid,
            std::path::Path::new("/tmp/forged")
        ));
        assert!(!process_uses_directory(
            pid,
            std::path::Path::new("/definitely/not/open")
        ));

        // A directory the process genuinely uses is corroborated. The test
        // process holds its own working directory, which is evidence of
        // exactly the kind user_data_dir_for_pid relies on.
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).expect("test process cwd");
        assert!(process_uses_directory(pid, &cwd));
    }

    #[test]
    fn packed_recovery_survives_leading_positional_arguments() {
        // Opening a URL or file puts a positional argument before the first
        // switch. Recovery must still find the profile rather than silently
        // falling back to the default directory.
        assert_eq!(
            user_data_dir_arguments(
                &packed_helium_arguments(
                    b"/opt/helium-browser-bin/helium https://example.test --user-data-dir=/tmp/p\0"
                )
                .expect("packed Helium argv with a leading positional")
            ),
            [PathBuf::from("/tmp/p")]
        );
        assert_eq!(
            user_data_dir_arguments(
                &packed_helium_arguments(
                    b"/opt/helium-browser-bin/helium /home/u/a.html --user-data-dir=/tmp/spaced dir\0"
                )
                .expect("packed Helium argv with a leading file positional")
            ),
            [PathBuf::from("/tmp/spaced dir")]
        );
    }

    #[test]
    fn user_data_dir_extraction_is_shared_by_both_argument_forms() {
        let joined = ["--user-data-dir=/tmp/a".to_owned()];
        let separated = ["--user-data-dir".to_owned(), "/tmp/b".to_owned()];
        assert_eq!(user_data_dir_arguments(&joined), [PathBuf::from("/tmp/a")]);
        assert_eq!(
            user_data_dir_arguments(&separated),
            [PathBuf::from("/tmp/b")]
        );
        assert!(user_data_dir_arguments(&["--user-data-dir=".to_owned()]).is_empty());
        assert!(user_data_dir_arguments(&["--user-data-dir".to_owned()]).is_empty());
        assert_eq!(
            user_data_dir_arguments(&[
                "--user-data-dir=/tmp/a".to_owned(),
                "--user-data-dir=/tmp/b".to_owned()
            ])
            .len(),
            2
        );
    }

    #[test]
    fn helium_is_a_standalone_consumer_browser() {
        assert_eq!(
            process_role_for_pid(i64::from(std::process::id()), BrowserProduct::Helium),
            BrowserProcessRole::StandaloneConsumer
        );
    }

    #[test]
    fn firefox_classifier_uses_product_tokens() {
        assert!(is_firefox("firefox --new-instance"));
        assert!(is_firefox("Mozilla Firefox"));
        assert!(!is_firefox("firefox-helper"));
        assert!(!is_firefox("waterfox"));
    }

    #[test]
    fn websocket_url_must_keep_the_attested_listener_port() {
        assert_eq!(
            loopback_websocket_port("ws://localhost:9222/devtools/browser/id"),
            Some(9222)
        );
        assert_ne!(
            loopback_websocket_port("ws://[::1]:9333/devtools/browser/foreign"),
            Some(9222)
        );
        assert_eq!(loopback_websocket_port("ws://0.0.0.0:9222/devtools"), None);
    }

    #[test]
    fn active_port_parser_requires_one_exact_browser_path() {
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/abc-123\n"),
            Some((9222, "/devtools/browser/abc-123"))
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser\n"),
            None
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/page/abc\n"),
            None
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/../page\n"),
            None
        );
    }
}
