// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Linux identity and endpoint evidence for the first-class browser tools.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::browser::managed_policy::policy_forces_extensions;
use cua_driver_core::browser::platform::{
    select_isolated_browser_executable, BrowserConsentOutcome, BrowserConsentRequest,
    BrowserPlatform, BrowserVisualAction, BrowserVisualActionKind, ExistingProfileSetupOutcome,
    ExistingProfileSetupRequest, PrepareAction, PrepareOutcome, PrepareRequest,
};
use cua_driver_core::browser::refusal::{BrowserRefusal, BrowserRefusalCode};
use cua_driver_core::browser::types::{
    BrowserClassification, BrowserEngineFamily, BrowserProcessRole, BrowserProduct,
    EndpointOwnershipMethod, EndpointOwnershipProof, EndpointTransport, NativeOwnershipMethod,
    NativeOwnershipProof, NativeWindowInfo, OwnedEndpoint, ProcessFingerprint, Rect,
};
use cua_driver_core::browser::{
    existing_profile_setup_descriptor, fetch_json_version_ws_url, is_electron_app_layout,
    is_firefox, loopback_websocket_port, parse_devtools_active_port, BrowserCursorTracker,
};

pub struct LinuxBrowserPlatform {
    cursor_registry: Arc<cursor_overlay::CursorRegistry>,
    browser_cursors: Mutex<BrowserCursorTracker>,
}

impl LinuxBrowserPlatform {
    pub fn new(cursor_registry: Arc<cursor_overlay::CursorRegistry>) -> Self {
        Self {
            cursor_registry,
            browser_cursors: Mutex::new(BrowserCursorTracker::default()),
        }
    }
}

impl Default for LinuxBrowserPlatform {
    fn default() -> Self {
        Self::new(Arc::new(cursor_overlay::CursorRegistry::new()))
    }
}

async fn x11_native_window(pid: i64, window_id: u64) -> Option<(NativeWindowInfo, bool)> {
    let pid_u32 = u32::try_from(pid).ok()?;
    tokio::task::spawn_blocking(move || {
        select_x11_window(pid, window_id, crate::x11::list_windows(Some(pid_u32)))
    })
    .await
    .ok()
    .flatten()
}

fn select_x11_window(
    pid: i64,
    window_id: u64,
    windows: Vec<crate::x11::WindowInfo>,
) -> Option<(NativeWindowInfo, bool)> {
    let only = windows.len() == 1;
    let window = windows
        .into_iter()
        .find(|w| w.xid == window_id && w.pid.map(i64::from) == Some(pid))?;
    Some((
        NativeWindowInfo {
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
        },
        only,
    ))
}

fn is_helium(name: &str) -> bool {
    std::path::Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("helium"))
}

fn hyprland_identity_matches(pid: u32, window_id: u64, owner_pid: u32, address: u64) -> bool {
    pid != 0 && window_id != 0 && owner_pid == pid && address == window_id
}

fn hyprland_is_only_owned_window(
    pid: u32,
    window_id: u64,
    identities: impl IntoIterator<Item = (u32, u64)>,
) -> bool {
    let mut owned = identities.into_iter().filter(|(owner, _)| *owner == pid);
    let Some((owner, address)) = owned.next() else {
        return false;
    };
    hyprland_identity_matches(pid, window_id, owner, address) && owned.next().is_none()
}

fn run_existing_profile_cleanup<T: Send + 'static>(
    cleanup: impl FnOnce() -> T + Send + 'static,
) -> Result<T, BrowserRefusal> {
    std::thread::Builder::new()
        .name("cua-browser-cleanup".into())
        .spawn(cleanup)
        .map_err(|error| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not start exact browser cleanup: {error}"),
            )
        })?
        .join()
        .map_err(|_| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "exact browser cleanup worker panicked",
            )
        })
}

pub(crate) fn is_chromium_identity(name: &str) -> bool {
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

fn is_chromium(name: &str) -> bool {
    is_chromium_identity(name)
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

/// Whether `path` and every directory above it are root-owned, not group- or
/// world-writable, and not symlinks, so only root can have put it there.
fn root_owned_path(path: &std::path::Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let mut current = Some(path);
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
    true
}

fn trusted_root_owned_installation(executable: &std::path::Path) -> bool {
    root_owned_path(executable)
        && std::fs::metadata(executable)
            .is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0)
}

/// The mandatory policy directories Chromium reads for `executable`'s
/// product on Linux. Chromium builds read `/etc/chromium`, except Ubuntu's
/// `chromium-browser` packaging, which reads only its own directory.
fn managed_policy_dirs(executable: &std::path::Path) -> &'static [&'static str] {
    match browser_product(&executable.to_string_lossy()) {
        BrowserProduct::GoogleChrome => &["/etc/opt/chrome/policies/managed"],
        BrowserProduct::Chromium
            if executable
                .file_name()
                .is_some_and(|name| name == "chromium-browser") =>
        {
            &["/etc/chromium-browser/policies/managed"]
        }
        BrowserProduct::Chromium => &["/etc/chromium/policies/managed"],
        BrowserProduct::MicrosoftEdge => &["/etc/opt/edge/policies/managed"],
        BrowserProduct::Brave => &["/etc/brave/policies/managed"],
        _ => &[],
    }
}

/// The first `*.json` policy file in `dirs` that `trusted` accepts and that
/// forces extensions. Unreadable or malformed files are skipped, as Chromium
/// skips them.
fn managed_extension_policy_file<'a>(
    dirs: impl IntoIterator<Item = &'a std::path::Path>,
    trusted: impl Fn(&std::path::Path) -> bool,
) -> Option<PathBuf> {
    dirs.into_iter().find_map(|dir| {
        let mut files = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        files.sort();
        files.into_iter().find(|file| {
            trusted(file)
                && std::fs::metadata(file).is_ok_and(|metadata| metadata.is_file())
                && std::fs::read(file)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .is_some_and(|policy| policy_forces_extensions(&policy))
        })
    })
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

/// Map the path of every filesystem-bound AF_UNIX stream socket that is
/// listening to its kernel inode, by scanning `/proc/net/unix`.
///
/// The columns are `Num RefCount Protocol Flags Type St Inode Path`. The
/// kernel sets `__SO_ACCEPTCON` in `Flags` only for a listening socket, and
/// Chromium's process-singleton socket is a `SOCK_STREAM` listener, so a bound
/// datagram socket or an unlistened stream socket never qualifies.
///
/// The kernel's `Inode` column is `%5lu`: right-justified in a minimum
/// five-character field, so a small inode number is left-padded with extra
/// spaces. Splitting on runs of whitespace (rather than a fixed column
/// offset) is what keeps that padding from being misread as additional
/// fields.
fn unix_listener_inodes() -> HashMap<PathBuf, u64> {
    const SO_ACCEPTCON: u32 = 1 << 16;
    const SOCK_STREAM: &str = "0001";
    let Ok(text) = std::fs::read_to_string("/proc/net/unix") else {
        return HashMap::new();
    };
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace().skip(3);
            let flags = u32::from_str_radix(fields.next()?, 16).ok()?;
            let kind = fields.next()?;
            let inode = fields.nth(1)?.parse::<u64>().ok()?;
            if kind != SOCK_STREAM || flags & SO_ACCEPTCON == 0 {
                return None;
            }
            // Only a filesystem-bound socket carries a path column; an
            // abstract-namespace socket has none and never matches an
            // absolute target. A path containing internal runs of more than
            // one space is an inherent ambiguity in this whitespace-delimited
            // encoding, not one this parser can resolve — Chromium's own
            // singleton-socket path is a single-token generated directory
            // name and never hits it.
            let rest: Vec<&str> = fields.collect();
            (!rest.is_empty()).then(|| (PathBuf::from(rest.join(" ")), inode))
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
        return Err(BrowserRefusal::new(
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

/// What every file descriptor held by `pid` or one of its descendants points
/// at, as the kernel reports it in `/proc/<pid>/fd`.
fn fd_targets_for_process_tree(pid: i64) -> Result<Vec<PathBuf>, BrowserRefusal> {
    let mut targets = Vec::new();
    for owner_pid in process_family_pids(pid)? {
        let Ok(directory) = std::fs::read_dir(format!("/proc/{owner_pid}/fd")) else {
            continue;
        };
        targets.extend(
            directory
                .flatten()
                .filter_map(|entry| std::fs::read_link(entry.path()).ok()),
        );
    }
    Ok(targets)
}

fn socket_inode(fd_target: &std::path::Path) -> Option<u64> {
    fd_target
        .to_str()?
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse::<u64>()
        .ok()
}

fn socket_inodes_for_process_tree(pid: i64) -> Result<HashSet<u64>, BrowserRefusal> {
    Ok(fd_targets_for_process_tree(pid)?
        .iter()
        .filter_map(|target| socket_inode(target))
        .collect())
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

/// Chromium resolves its default profile under the browser's own
/// `$XDG_CONFIG_HOME`, used verbatim even when relative, falling back to
/// `$HOME/.config`, so read both from the browser process's environment. The
/// driver runs as the same user, so its `$HOME` stands in when the browser's
/// environment cannot be read. A relative result is resolved by the caller.
fn default_user_data_dir(product: BrowserProduct, environ: &[u8]) -> Option<PathBuf> {
    let relative = match product {
        BrowserProduct::GoogleChrome => "google-chrome",
        BrowserProduct::MicrosoftEdge => "microsoft-edge",
        BrowserProduct::Chromium => "chromium",
        BrowserProduct::Helium => "net.imput.helium",
        BrowserProduct::Brave => "BraveSoftware/Brave-Browser",
        _ => return None,
    };
    let var = |name: &[u8]| {
        environ
            .split(|byte| *byte == 0)
            .find_map(|entry| entry.strip_prefix(name)?.strip_prefix(b"="))
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(std::ffi::OsStr::from_bytes(value)))
    };
    let config_home = match var(b"XDG_CONFIG_HOME") {
        Some(path) => path,
        None => var(b"HOME")
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?
            .join(".config"),
    };
    Some(config_home.join(relative))
}

/// Corroborate a candidate profile directory against the kernel's own view of
/// the process.
///
/// A Chromium profile is not merely named on the command line; the running
/// browser owns that profile's process-singleton listener. Chromium creates
/// `<profile>/SingletonSocket` as a symlink to a private AF_UNIX socket and
/// keeps it bound for the life of the browser, so the kernel's own listing of
/// live sockets — not the process's ambient working directory or its
/// unrelated open file descriptors — is what ties a directory to this exact
/// process. An attacker who controls only argument text can name any path,
/// including one the process's cwd or some incidental fd happens to fall
/// under, but cannot inject a socket file descriptor into another process's
/// fd table; only the kernel can put one there.
///
/// The `SingletonSocket` symlink itself is only text, though: any other
/// directory can carry a link to the same live socket. Chromium keeps its
/// profile databases open, so the real profile always lies on the path of a
/// file the browser holds. Among the directories on the paths of every file
/// the process tree holds, the candidate must be the only one linking to the
/// socket: a forged directory either holds no browser file, or is a second
/// claimant beside the real profile.
fn process_owns_profile_directory(pid: i64, directory: &std::path::Path) -> bool {
    let Ok(directory) = std::fs::canonicalize(directory) else {
        return false;
    };
    let Ok(fd_targets) = fd_targets_for_process_tree(pid) else {
        return false;
    };
    let listeners = unix_listener_inodes();
    let singleton_inode = |candidate: &std::path::Path| {
        let target = std::fs::read_link(candidate.join("SingletonSocket")).ok()?;
        listeners.get(&candidate.join(target)).copied()
    };
    let Some(inode) = singleton_inode(directory.as_path()) else {
        return false;
    };
    if !fd_targets
        .iter()
        .any(|target| socket_inode(target) == Some(inode))
    {
        return false;
    }
    let claimants = fd_targets
        .iter()
        .filter(|target| target.is_absolute())
        .flat_map(|file| file.ancestors().skip(1))
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|candidate| singleton_inode(candidate) == Some(inode))
        .collect::<Vec<_>>();
    claimants == [directory.as_path()]
}

fn user_data_dir_for_pid(pid: i64) -> Result<Option<PathBuf>, BrowserRefusal> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).map_err(|_| {
        BrowserRefusal::new(
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
    let path = match directories.as_slice() {
        [] => {
            let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
            let Some(path) = default_user_data_dir(product, &environ) else {
                return Ok(None);
            };
            path
        }
        [path] => path.clone(),
        _ => {
            return Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "browser process has multiple distinct --user-data-dir arguments",
            ))
        }
    };
    let resolved = if path.is_absolute() {
        path
    } else {
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).map_err(|_| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserBindingStale,
                format!("browser process {pid} working directory is unavailable"),
            )
        })?;
        cwd.join(path)
    };
    if candidate_is_unseparated && !process_owns_profile_directory(pid, &resolved) {
        return Err(BrowserRefusal::new(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "the browser's command line has no argument separators, so a --user-data-dir value read from it could not be distinguished from ordinary argument text, and the kernel does not show the process owning that directory as its profile",
        ));
    }
    Ok(Some(resolved))
}

fn active_port_endpoint(pid: i64) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
    let Some(user_data_dir) = user_data_dir_for_pid(pid)? else {
        return Ok(None);
    };
    let text = match std::fs::read_to_string(user_data_dir.join("DevToolsActivePort")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not read the browser's DevToolsActivePort file: {error}"),
            ))
        }
    };
    let Some((port, path)) = parse_devtools_active_port(&text) else {
        return Err(BrowserRefusal::new(
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
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} is no longer available"),
        )
    })?;
    let started = crate::proc_fs::process_start_time_from_stat(&stat).ok_or_else(|| {
        BrowserRefusal::new(
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
    let url = fetch_json_version_ws_url(port).await?;
    (loopback_websocket_port(&url) == Some(port)).then_some(url)
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
        Err(BrowserRefusal::new(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "no root-owned, non-writable system Chromium executable is available for isolated launch",
        ))
    }

    fn managed_extension_policy(&self, executable: &str) -> Option<String> {
        let dirs = managed_policy_dirs(std::path::Path::new(executable));
        managed_extension_policy_file(dirs.iter().map(std::path::Path::new), root_owned_path)
            .map(|file| file.display().to_string())
    }

    fn standalone_trusted_input_background_limitation(&self) -> Option<&'static str> {
        Some("Chromium's trusted CDP Input route activates its standalone browser window on Linux")
    }

    async fn visualize_browser_action(&self, action: BrowserVisualAction) {
        if action.session.is_empty()
            || action.cdp_target_id.is_empty()
            || cua_driver_core::session::is_session_ended(&action.session)
        {
            return;
        }

        let visibility_updates = self.browser_cursors.lock().unwrap().update(
            &action.session,
            action.window_id,
            &action.cdp_target_id,
            action.tab_is_active,
        );
        let cursor_enabled = self
            .cursor_registry
            .get_or_create(&action.session)
            .config
            .enabled;
        for (key, visible) in visibility_updates {
            let enabled = if key == action.session {
                visible && cursor_enabled
            } else {
                visible
                    && self
                        .cursor_registry
                        .get(&key)
                        .is_some_and(|state| state.config.enabled)
            };
            crate::overlay::send_command_for(
                key,
                cursor_overlay::OverlayCommand::SetEnabled(enabled),
            );
        }
        if !action.tab_is_active || !cursor_enabled {
            return;
        }
        let (Some(screen_x), Some(screen_y)) = (action.screen_x, action.screen_y) else {
            return;
        };
        if !screen_x.is_finite() || !screen_y.is_finite() {
            return;
        }

        crate::overlay::send_command_for(
            action.session.clone(),
            cursor_overlay::OverlayCommand::PinAbove(action.window_id),
        );
        crate::overlay::animate_cursor_to_for(action.session.clone(), screen_x, screen_y).await;
        self.cursor_registry
            .update_position(&action.session, screen_x, screen_y);

        if matches!(
            action.kind,
            BrowserVisualActionKind::Click
                | BrowserVisualActionKind::Type
                | BrowserVisualActionKind::RightClick
                | BrowserVisualActionKind::DoubleClick
                | BrowserVisualActionKind::Drag
        ) {
            crate::overlay::send_command_for(
                action.session,
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: screen_x,
                    y: screen_y,
                },
            );
        }
    }

    async fn classify_browser(&self, pid: i64) -> Result<BrowserClassification, BrowserRefusal> {
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Linux process-id range"),
            )
        })?;
        let (process, executable, electron_app) = tokio::task::spawn_blocking(move || {
            let process = crate::proc_fs::list_processes()
                .into_iter()
                .find(|process| process.pid == pid_u32);
            let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
            let electron_app = executable.as_deref().is_some_and(is_electron_app_layout);
            let executable = executable.map(|path| path.to_string_lossy().into_owned());
            (process, executable, electron_app)
        })
        .await
        .ok()
        .and_then(|(process, executable, electron_app)| {
            process.map(|process| (process, executable, electron_app))
        })
        .ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserBindingStale,
                format!("browser process {pid} is no longer available"),
            )
        })?;
        let identity = executable
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| process.name.clone());
        let chromium = electron_app || is_chromium(&identity);
        let gecko = is_firefox(&identity);
        let product_kind = if electron_app {
            BrowserProduct::Electron
        } else {
            browser_product(&identity)
        };
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
        if let Some((window, _)) = x11_native_window(pid, window_id).await {
            return Ok(window);
        }
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Linux process-id range"),
            )
        })?;
        if crate::desktop::desktop() != crate::desktop::Desktop::X11 {
            if crate::wayland::hyprland::is_session() {
                let (window, bounds) = tokio::task::spawn_blocking(move || {
                    crate::wayland::hyprland::browser_window(window_id)
                })
                .await
                .ok()
                .flatten()
                .ok_or_else(|| {
                    BrowserRefusal::new(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        "Hyprland could not attest the exact mapped browser window",
                    )
                })?;
                if !hyprland_identity_matches(pid_u32, window_id, window.pid, window.address) {
                    return Err(BrowserRefusal::new(
                        BrowserRefusalCode::BrowserWrongTargetRefused,
                        "Hyprland browser window does not match the requested pid and full address",
                    ));
                }
                // Without its output the rect stays in layout coordinates,
                // which Chromium does not share, so it binds read-only.
                let geometry_exact = bounds.is_some();
                let [x, y, width, height] = bounds.unwrap_or([
                    f64::from(window.x),
                    f64::from(window.y),
                    f64::from(window.width),
                    f64::from(window.height),
                ]);
                return Ok(NativeWindowInfo {
                    pid,
                    window_id,
                    title: window.title,
                    bounds: Rect::new(x, y, width, height),
                    geometry_exact,
                    ownership: NativeOwnershipProof {
                        method: NativeOwnershipMethod::WindowServerOwner,
                        owner_pid: pid,
                        detail: Some(
                            "authenticated Hyprland IPC pid, full address, and mapped window rect on its output"
                                .to_owned(),
                        ),
                    },
                });
            }
            if let Some(window) = crate::wayland::sway_ipc::window_for_id(window_id) {
                if window.pid != pid_u32 {
                    return Err(BrowserRefusal::new(
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
                BrowserRefusal::new(
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

        Err({
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserBindingStale,
                format!("X11 window {window_id} is not owned by pid {pid}"),
            )
        })
    }

    async fn is_only_exact_native_window(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<Option<bool>, BrowserRefusal> {
        if let Some((_, only)) = x11_native_window(pid, window_id).await {
            return Ok(Some(only));
        }
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Linux process-id range"),
            )
        })?;
        if crate::desktop::desktop() != crate::desktop::Desktop::X11 {
            if crate::wayland::hyprland::is_session() {
                let windows = tokio::task::spawn_blocking(crate::wayland::hyprland::list_windows)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .ok_or_else(|| {
                        BrowserRefusal::new(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            "Hyprland could not attest browser window cardinality",
                        )
                    })?;
                // Count every mapped client owned by the PID, including hidden
                // and off-workspace clients; visibility cannot prove uniqueness.
                return Ok(Some(hyprland_is_only_owned_window(
                    pid_u32,
                    window_id,
                    windows
                        .into_iter()
                        .map(|window| (window.pid, window.address)),
                )));
            }
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
                        BrowserRefusal::new(
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
        Ok(Some(false))
    }

    async fn discover_owned_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        let ports = tokio::task::spawn_blocking(move || loopback_ports_for_pid(pid))
            .await
            .map_err(|error| {
                BrowserRefusal::new(
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
                BrowserRefusal::new(
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
            _ => Err(BrowserRefusal::new(
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
            return Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                "the approved existing-profile endpoint is not loopback-only",
            ));
        };
        if let Some(endpoint) = active_port_endpoint(pid)? {
            if endpoint.ws_url != expected_ws_url {
                return Err(BrowserRefusal::new(
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
                BrowserRefusal::new(
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
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!(
                    "approved existing-profile setup is not implemented for {:?}",
                    request.browser
                ),
            )
        })?;
        let pid_u32 = u32::try_from(request.pid).map_err(|_| {
            BrowserRefusal::new(
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
                    BrowserRefusal::new(
                        BrowserRefusalCode::BrowserRouteUnavailable,
                        format!("listener inspection task failed: {error}"),
                    )
                })??;
        let handle = tokio::task::spawn_blocking(move || {
            crate::browser_setup_ui::enable(pid_u32, window_id, descriptor)
        })
        .await
        .map_err(|error| {
            BrowserRefusal::new(
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
                    break Err(BrowserRefusal::new(
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
                    break Err(BrowserRefusal::new(
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
                    break Err(BrowserRefusal::new(
                        BrowserRefusalCode::BrowserRequiresSetup,
                        format!(
                            "{} did not expose a uniquely PID-owned loopback endpoint after the exact setup action",
                            descriptor.product_name
                        ),
                    ))
                }
                _ => {
                    break Err(BrowserRefusal::new(
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
                        BrowserRefusal::new(
                            BrowserRefusalCode::BrowserRouteUnavailable,
                            format!("could not finish browser setup cleanup: {join_error}"),
                        )
                    })??;
                return Err(BrowserRefusal::new(
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
                        BrowserRefusal::new(
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
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser pid is outside the Linux process-id range",
            )
        })?;
        tokio::task::spawn_blocking(move || {
            crate::browser_setup_ui::commit_pending(pid, request.window_id)
        })
        .await
        .map_err(|error| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not commit exact browser setup cleanup: {error}"),
            )
        })?
    }

    fn cleanup_existing_profile_setup(
        &self,
        request: ExistingProfileSetupRequest,
    ) -> Result<bool, BrowserRefusal> {
        let descriptor = existing_profile_setup_descriptor(request.browser).ok_or_else(|| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!(
                    "existing-profile cleanup is not implemented for {:?}",
                    request.browser
                ),
            )
        })?;
        let pid = u32::try_from(request.pid).map_err(|_| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser pid is outside the Linux process-id range",
            )
        })?;
        let window_id = request.window_id;
        run_existing_profile_cleanup(move || {
            let dismissed_before = crate::browser_consent_ui::dismiss(pid, window_id)?;
            let closed_setup_page = crate::browser_setup_ui::disable(pid, window_id, descriptor)?;
            let dismissed_after = crate::browser_consent_ui::dismiss(pid, window_id)?;
            Ok(dismissed_before || closed_setup_page || dismissed_after)
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
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not roll back exact browser setup: {join_error}"),
            )
        })
    }

    async fn dismiss_abandoned_existing_profile_consent(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<bool, BrowserRefusal> {
        let Ok(pid) = u32::try_from(pid) else {
            return Ok(false);
        };
        tokio::task::spawn_blocking(move || crate::browser_consent_ui::dismiss(pid, window_id))
            .await
            .unwrap_or_else(|join_error| {
                Err(BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not close the abandoned browser consent prompt: {join_error}"),
                ))
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
                BrowserRefusal::new(
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
            return Err(BrowserRefusal::new(
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
        Err(BrowserRefusal::new(
            BrowserRefusalCode::BrowserRequiresSetup,
            "No owned endpoint is available. Acting setup is handled by shared core only for a verified driver-owned isolated profile.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_user_data_dir_follows_the_browser_process_config_home() {
        let brave = browser_product("/opt/brave.com/brave/brave");
        assert_eq!(
            default_user_data_dir(brave, b"HOME=/home/ana\0LANG=C\0"),
            Some(PathBuf::from(
                "/home/ana/.config/BraveSoftware/Brave-Browser"
            ))
        );
        assert_eq!(
            default_user_data_dir(brave, b"HOME=/home/ana\0XDG_CONFIG_HOME=/srv/cfg\0"),
            Some(PathBuf::from("/srv/cfg/BraveSoftware/Brave-Browser"))
        );
        assert_eq!(
            default_user_data_dir(
                BrowserProduct::GoogleChrome,
                b"XDG_CONFIG_HOME=\0HOME=/home/ana\0"
            ),
            Some(PathBuf::from("/home/ana/.config/google-chrome"))
        );
        assert_eq!(
            default_user_data_dir(brave, b"HOME=/home/ana\0XDG_CONFIG_HOME=cfg\0"),
            Some(PathBuf::from("cfg/BraveSoftware/Brave-Browser"))
        );
        assert_eq!(
            default_user_data_dir(BrowserProduct::Vivaldi, b"HOME=/home/ana\0"),
            None
        );
    }

    #[test]
    fn x11_identity_and_cardinality_precede_compositor_selection() {
        let window = |xid| crate::x11::WindowInfo {
            xid,
            pid: Some(42),
            app_name: String::new(),
            title: "browser".into(),
            is_on_screen: true,
            z_index: None,
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        };
        let (native, only) = select_x11_window(42, 7, vec![window(7)]).unwrap();
        assert!(native.geometry_exact && only);
        assert_eq!(native.window_id, 7);
        assert!(
            !select_x11_window(42, 7, vec![window(7), window(8)])
                .unwrap()
                .1
        );
        for id in [8, u64::from(u32::MAX) + 1] {
            let mut compositor_id = None;
            let result = select_x11_window(42, id, vec![window(7)]).or_else(|| {
                compositor_id = Some(id);
                None
            });
            assert!(result.is_none());
            assert_eq!(compositor_id, Some(id));
        }
        assert!(select_x11_window(43, 7, vec![window(7)]).is_none());
    }

    #[test]
    fn hyprland_browser_identity_requires_exact_pid_and_full_native_address() {
        let address = 0x1234_0000_0042;
        assert!(hyprland_identity_matches(42, address, 42, address));
        assert!(!hyprland_identity_matches(43, address, 42, address));
        assert!(!hyprland_identity_matches(42, 0x42, 42, address));
        assert!(!hyprland_identity_matches(42, address + 1, 42, address));
        assert!(!hyprland_identity_matches(0, address, 0, address));
        assert!(!hyprland_identity_matches(42, 0, 42, 0));
    }

    #[test]
    fn hyprland_browser_cardinality_requires_one_exact_owned_surface() {
        let address = 0x1234_0000_0042;
        assert!(hyprland_is_only_owned_window(42, address, [(42, address)]));
        assert!(hyprland_is_only_owned_window(
            42,
            address,
            [(7, address + 1), (42, address)],
        ));
        for identities in [
            vec![],
            vec![(7, address)],
            vec![(42, 0x42)],
            vec![(42, address + 1)],
            vec![(42, address), (42, address + 1)],
            vec![(42, address), (42, address)],
        ] {
            assert!(!hyprland_is_only_owned_window(42, address, identities));
        }
        assert!(!hyprland_is_only_owned_window(0, address, [(0, address)]));
        assert!(!hyprland_is_only_owned_window(42, 0, [(42, 0)]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn existing_profile_cleanup_can_drive_atspi_runtime_from_async_session_teardown() {
        let cleaned = run_existing_profile_cleanup(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("AT-SPI-style cleanup runtime")
                .block_on(async { true })
        })
        .expect("cleanup worker");

        assert!(cleaned);
    }

    #[tokio::test]
    async fn browser_visual_feedback_updates_the_declared_session_cursor() {
        let registry = Arc::new(cursor_overlay::CursorRegistry::new());
        let platform = LinuxBrowserPlatform::new(registry.clone());
        platform
            .visualize_browser_action(BrowserVisualAction {
                session: "browser-cursor-test".to_owned(),
                window_id: 77,
                cdp_target_id: "tab-A".to_owned(),
                tab_is_active: true,
                screen_x: Some(321.0),
                screen_y: Some(456.0),
                kind: BrowserVisualActionKind::Click,
            })
            .await;

        let state = registry
            .get("browser-cursor-test")
            .expect("browser action should materialize its session cursor");
        assert_eq!((state.x, state.y), (Some(321.0), Some(456.0)));
    }

    #[tokio::test]
    async fn inactive_tab_feedback_materializes_but_does_not_move_its_cursor() {
        let registry = Arc::new(cursor_overlay::CursorRegistry::new());
        let platform = LinuxBrowserPlatform::new(registry.clone());
        platform
            .visualize_browser_action(BrowserVisualAction {
                session: "browser-cursor-hidden".to_owned(),
                window_id: 77,
                cdp_target_id: "tab-hidden".to_owned(),
                tab_is_active: false,
                screen_x: Some(321.0),
                screen_y: Some(456.0),
                kind: BrowserVisualActionKind::Click,
            })
            .await;

        let state = registry
            .get("browser-cursor-hidden")
            .expect("browser action should establish its session-to-tab binding");
        assert!(
            state.x.is_none() && state.y.is_none(),
            "an inactive tab must not animate or move its visible cursor"
        );
    }

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
    fn managed_policy_dirs_follow_the_launched_product() {
        for (executable, dirs) in [
            (
                "/opt/google/chrome/google-chrome",
                &["/etc/opt/chrome/policies/managed"][..],
            ),
            (
                "/usr/lib/chromium/chromium",
                &["/etc/chromium/policies/managed"][..],
            ),
            (
                "/usr/lib/chromium-browser/chromium-browser",
                &["/etc/chromium-browser/policies/managed"][..],
            ),
            (
                "/opt/microsoft/msedge/msedge",
                &["/etc/opt/edge/policies/managed"][..],
            ),
            (
                "/opt/brave.com/brave/brave",
                &["/etc/brave/policies/managed"][..],
            ),
            ("/usr/lib/firefox/firefox", &[][..]),
        ] {
            assert_eq!(
                managed_policy_dirs(std::path::Path::new(executable)),
                dirs,
                "{executable}"
            );
        }
    }

    #[test]
    fn only_a_trusted_policy_file_that_forces_extensions_counts() {
        let root = tempfile::tempdir().expect("temporary policy directory");
        let dir = root.path();
        std::fs::write(
            dir.join("languages.json"),
            r#"{"ForcedLanguages": ["en-US"]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        std::fs::write(
            dir.join("extensions.txt"),
            r#"{"ExtensionInstallForcelist": ["ddkjiahejlhfcafbddmgiahcphecmpfh"]}"#,
        )
        .unwrap();
        assert_eq!(managed_extension_policy_file([dir], |_| true), None);

        let forced = dir.join("test.json");
        std::fs::write(
            &forced,
            r#"{"ExtensionInstallForcelist": ["ddkjiahejlhfcafbddmgiahcphecmpfh;https://clients2.google.com/service/update2/crx"]}"#,
        )
        .unwrap();
        assert_eq!(
            managed_extension_policy_file([dir], |_| true),
            Some(forced.clone())
        );
        // A file the user could have written is not admin policy.
        assert_eq!(managed_extension_policy_file([dir], root_owned_path), None);
        assert_eq!(
            managed_extension_policy_file([dir], |file| file != forced.as_path()),
            None
        );
        assert_eq!(
            managed_extension_policy_file([dir.join("missing").as_path(), dir], |_| true),
            Some(forced)
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
        assert_eq!(
            default_user_data_dir(BrowserProduct::Helium, b"HOME=/home/ana\0"),
            Some(PathBuf::from("/home/ana/.config/net.imput.helium"))
        );
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

    /// Serializes the `fork` inside `FakeHeliumProcess::spawn` against the
    /// window in which a test holds a listening socket open and measures its
    /// liveness.
    ///
    /// `Command::spawn` is fork plus exec, and the fork duplicates every
    /// descriptor its thread can see. A sibling fork landing inside that
    /// window lends this test's listener to a child for the moment between
    /// `fork` and the child's `exec`, so the kernel keeps the socket alive for
    /// several milliseconds past this test's own `drop`. That is a property of
    /// running these tests concurrently in one process, not of the ownership
    /// check under test, so the two are serialized rather than papered over
    /// with a sleep.
    static FORK_EXCLUSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn exclude_concurrent_forks() -> std::sync::MutexGuard<'static, ()> {
        FORK_EXCLUSION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn unix_listener_inodes_resolve_only_a_live_stream_listener() {
        // Held for the sockets' whole lifetime so no sibling test can fork
        // these descriptors into a child that outlives `drop(listener)`.
        let _fork_exclusion = exclude_concurrent_forks();
        let directory = tempfile::tempdir().expect("temporary socket directory");
        let sock_path = directory.path().join("probe.sock");
        let listener =
            std::os::unix::net::UnixListener::bind(&sock_path).expect("bind probe socket");
        assert!(unix_listener_inodes()
            .get(&sock_path)
            .is_some_and(|inode| *inode > 0));

        // Dropping the listener closes its only fd. The bind path remains a
        // socket special file on disk (Rust's UnixListener never unlinks on
        // drop), but the live kernel object -- and its /proc/net/unix line
        // -- is gone. A path that merely exists on disk must not resolve.
        drop(listener);
        assert!(sock_path.exists());
        assert!(!unix_listener_inodes().contains_key(&sock_path));

        // A bound datagram socket is live but never listens.
        let datagram_path = directory.path().join("datagram.sock");
        let _datagram =
            std::os::unix::net::UnixDatagram::bind(&datagram_path).expect("bind datagram socket");
        assert!(!unix_listener_inodes().contains_key(&datagram_path));

        assert!(!unix_listener_inodes().contains_key(&directory.path().join("never-bound.sock")));
    }

    #[test]
    fn process_owns_profile_directory_requires_the_sole_owned_singleton_claimant() {
        let pid = i64::from(std::process::id());

        // No SingletonSocket symlink at all.
        let bare = tempfile::tempdir().expect("bare candidate directory");
        assert!(!process_owns_profile_directory(pid, bare.path()));

        // A SingletonSocket symlink present, but its target names no live
        // kernel socket (nothing currently holds it open).
        let stale_root = tempfile::tempdir().expect("stale socket directory");
        let stale_sock = stale_root.path().join("SingletonSocket");
        drop(std::os::unix::net::UnixListener::bind(&stale_sock).expect("bind then drop"));
        let stale_candidate = tempfile::tempdir().expect("stale candidate directory");
        std::os::unix::fs::symlink(&stale_sock, stale_candidate.path().join("SingletonSocket"))
            .expect("symlink stale SingletonSocket");
        assert!(!process_owns_profile_directory(pid, stale_candidate.path()));

        // A SingletonSocket symlink resolving to a datagram socket this
        // process holds: live, owned, but not a listener.
        let datagram_root = tempfile::tempdir().expect("datagram socket directory");
        let datagram_sock = datagram_root.path().join("SingletonSocket");
        let _datagram =
            std::os::unix::net::UnixDatagram::bind(&datagram_sock).expect("bind datagram");
        let datagram_candidate = tempfile::tempdir().expect("datagram candidate directory");
        std::fs::create_dir(datagram_candidate.path().join("Default")).expect("create Default");
        let _datagram_held =
            std::fs::File::create(datagram_candidate.path().join("Default/History"))
                .expect("hold a datagram-candidate file open");
        std::os::unix::fs::symlink(
            &datagram_sock,
            datagram_candidate.path().join("SingletonSocket"),
        )
        .expect("symlink datagram SingletonSocket");
        assert!(!process_owns_profile_directory(
            pid,
            datagram_candidate.path()
        ));

        // The real profile: a SingletonSocket symlink resolving to a listener
        // this exact process holds, and a file held open inside it.
        let owned_root = tempfile::tempdir().expect("owned socket directory");
        let owned_sock = owned_root.path().join("SingletonSocket");
        let _listener =
            std::os::unix::net::UnixListener::bind(&owned_sock).expect("bind owned socket");
        let profile_root = tempfile::tempdir().expect("profile root");
        let profile = profile_root.path().join("profile");
        std::fs::create_dir_all(profile.join("Default")).expect("create profile");
        std::os::unix::fs::symlink(&owned_sock, profile.join("SingletonSocket"))
            .expect("symlink real SingletonSocket");

        // Without any file held open inside it, the link alone is not enough.
        assert!(!process_owns_profile_directory(pid, &profile));
        let _held = std::fs::File::create(profile.join("Default/History"))
            .expect("hold a profile file open");
        assert!(process_owns_profile_directory(pid, &profile));

        // A forged sibling links to the same live socket but holds no files.
        let forged = tempfile::tempdir().expect("forged sibling directory");
        std::os::unix::fs::symlink(&owned_sock, forged.path().join("SingletonSocket"))
            .expect("symlink aliased SingletonSocket");
        assert!(!process_owns_profile_directory(pid, forged.path()));

        // Holding an incidental file there, such as a download, does not
        // help: the real profile is still a second claimant.
        let incidental = std::fs::File::create(forged.path().join("download.bin"))
            .expect("hold an incidental file in the forged sibling");
        assert!(!process_owns_profile_directory(pid, forged.path()));
        drop(incidental);
        std::fs::remove_file(forged.path().join("SingletonSocket"))
            .expect("remove aliased SingletonSocket");
        assert!(process_owns_profile_directory(pid, &profile));

        // A forged descendant of the real profile shares its open files, but
        // the real profile above it links to the same socket.
        std::os::unix::fs::symlink(&owned_sock, profile.join("Default/SingletonSocket"))
            .expect("symlink descendant SingletonSocket");
        assert!(!process_owns_profile_directory(
            pid,
            &profile.join("Default")
        ));
        std::fs::remove_file(profile.join("Default/SingletonSocket"))
            .expect("remove descendant SingletonSocket");

        // Likewise a forged ancestor, with the real profile below it.
        std::os::unix::fs::symlink(&owned_sock, profile_root.path().join("SingletonSocket"))
            .expect("symlink ancestor SingletonSocket");
        assert!(!process_owns_profile_directory(pid, profile_root.path()));
        // The forged ancestor shares the socket, so the real profile is no
        // longer the sole claimant either: refusing is the safe outcome.
        assert!(!process_owns_profile_directory(pid, &profile));
    }

    /// A disposable stand-in for `/opt/helium-browser-bin/helium`: a real
    /// child process whose `/proc/<pid>/exe` basename is exactly `helium`
    /// (satisfying `browser_product`) and whose `cmdline` is exactly one
    /// packed record under the caller's control via `arg0`, mirroring the
    /// observed Helium shape without needing a real browser installed.
    ///
    /// The stand-in executable is a copy of `/bin/sh` invoked with zero
    /// arguments: `packed_helium_arguments` requires the *entire* recovered
    /// argv to be a single NUL-terminated record, so the process must reach
    /// `/proc/<pid>/cmdline` with nothing beyond the spoofed `arg0` in it.
    /// Any additional positional argument -- e.g. a duration passed to
    /// `sleep` -- would itself become a second record and silently defeat
    /// the very packed-parsing path under test. The shell blocks reading
    /// commands from its own unclosed stdin instead, needing no arguments to
    /// stay alive. A coreutils tool such as `cat` cannot stand in: uutils
    /// coreutils (Ubuntu 26.04) is one multi-call binary that picks the tool
    /// from `arg0` and exits on the spoofed one.
    struct FakeHeliumProcess {
        child: std::process::Child,
        // Keeping the child's stdin open is what keeps the shell blocked; if
        // this handle is dropped, stdin sees EOF and the process exits.
        _stdin: std::process::ChildStdin,
        _executable_directory: tempfile::TempDir,
    }

    impl FakeHeliumProcess {
        fn spawn(
            packed_cmdline: &str,
            current_dir: &std::path::Path,
            stdout: std::process::Stdio,
            stderr: std::process::Stdio,
        ) -> Self {
            use std::os::unix::fs::PermissionsExt;
            use std::os::unix::process::CommandExt;
            // No sibling test may be measuring socket liveness across this
            // fork; see `FORK_EXCLUSION`.
            let _fork_exclusion = exclude_concurrent_forks();
            let executable_directory =
                tempfile::tempdir().expect("temporary fake-helium executable directory");
            let executable = executable_directory.path().join("helium");
            std::fs::copy("/bin/sh", &executable)
                .expect("stage a `/bin/sh` stand-in as the fake Helium executable");
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
                .expect("make the fake Helium executable runnable");
            let mut child = std::process::Command::new(&executable)
                .arg0(packed_cmdline)
                .current_dir(current_dir)
                .stdin(std::process::Stdio::piped())
                .stdout(stdout)
                .stderr(stderr)
                .spawn()
                .expect("spawn fake Helium process");
            let stdin = child.stdin.take().expect("piped stdin handle");
            // Give the kernel a moment to publish /proc/<pid>/{exe,cmdline,fd}
            // before any assertion reads them.
            std::thread::sleep(Duration::from_millis(30));
            Self {
                child,
                _stdin: stdin,
                _executable_directory: executable_directory,
            }
        }

        fn pid(&self) -> i64 {
            i64::from(self.child.id())
        }
    }

    impl Drop for FakeHeliumProcess {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn user_data_dir_for_pid_refuses_ordinary_text_naming_the_processs_own_cwd() {
        // The exact counterexample from review: Helium starts inside a
        // directory reachable from ordinary argument text -- here a file
        // positional argument that happens to contain
        // " --user-data-dir=<that same directory>" -- with no real profile
        // option ever supplied. The old working-directory check accepted
        // this because the candidate merely had to prefix-match the
        // process's ambient cwd, which ordinary argument text can trivially
        // arrange without the process actually using that directory as its
        // profile.
        let attacker_dir = tempfile::tempdir().expect("attacker-reachable directory");
        let packed = format!(
            "/opt/helium-browser-bin/helium {}/report --user-data-dir={}",
            attacker_dir.path().display(),
            attacker_dir.path().display()
        );
        let process = FakeHeliumProcess::spawn(
            &packed,
            attacker_dir.path(),
            std::process::Stdio::null(),
            std::process::Stdio::null(),
        );

        // Precondition: the process's real cwd genuinely is the candidate
        // directory, which is exactly what made the old cwd-based check
        // wrongly corroborate it.
        let cwd = std::fs::read_link(format!("/proc/{}/cwd", process.pid()))
            .expect("fake Helium process cwd");
        assert_eq!(cwd, attacker_dir.path());
        // And no real SingletonSocket exists there.
        assert!(!attacker_dir.path().join("SingletonSocket").exists());

        let outcome = user_data_dir_for_pid(process.pid());
        assert!(
            matches!(
                &outcome,
                Err(refusal) if refusal.code == BrowserRefusalCode::BrowserBindingAmbiguous
            ),
            "expected a browser_binding_ambiguous refusal, got {outcome:?}"
        );
    }

    #[test]
    fn user_data_dir_for_pid_corroborates_a_real_owned_singleton_socket() {
        // The legitimate counterpart: a real `--user-data-dir` switch naming
        // a profile directory with a space in its path, an unrelated
        // working directory, and a SingletonSocket this exact process
        // genuinely owns (mirroring Chromium's real process-singleton
        // mechanism), plus a profile file it holds open. This must attach.
        let profile_root = tempfile::tempdir().expect("profile root");
        let profile = profile_root.path().join("profile with space");
        std::fs::create_dir_all(profile.join("Default")).expect("create spaced profile directory");
        let singleton_root = tempfile::tempdir().expect("singleton socket root");
        let singleton_sock = singleton_root.path().join("SingletonSocket");
        let listener =
            std::os::unix::net::UnixListener::bind(&singleton_sock).expect("bind singleton");
        std::os::unix::fs::symlink(&singleton_sock, profile.join("SingletonSocket"))
            .expect("symlink real SingletonSocket into the profile");
        let stdout = unsafe {
            use std::os::fd::{FromRawFd, IntoRawFd};
            std::process::Stdio::from_raw_fd(listener.into_raw_fd())
        };

        let held = std::fs::File::create(profile.join("Default/History"))
            .expect("create a profile file for the browser to hold");

        let unrelated_cwd = tempfile::tempdir().expect("unrelated working directory");
        let packed = format!(
            "/opt/helium-browser-bin/helium --user-data-dir={}",
            profile.display()
        );
        let process = FakeHeliumProcess::spawn(&packed, unrelated_cwd.path(), stdout, held.into());

        let cwd = std::fs::read_link(format!("/proc/{}/cwd", process.pid()))
            .expect("fake Helium process cwd");
        assert_ne!(cwd, profile, "the profile match must not depend on cwd");

        assert_eq!(
            user_data_dir_for_pid(process.pid()).expect("resolve the owned profile directory"),
            Some(profile)
        );
    }

    #[test]
    fn user_data_dir_for_pid_refuses_a_forged_directory_aliasing_the_real_singleton_socket() {
        // Helium runs on its real profile, which links to a singleton listener
        // the process holds and contains a file it holds open. Ordinary
        // argument text names a different directory whose SingletonSocket
        // symlink points at that same live socket. The link is only text, so
        // it must not make the forged directory the profile.
        let profile_root = tempfile::tempdir().expect("profile root");
        let profile = profile_root.path().join("profile");
        std::fs::create_dir_all(profile.join("Default")).expect("create profile directory");
        let singleton_root = tempfile::tempdir().expect("singleton socket root");
        let singleton_sock = singleton_root.path().join("SingletonSocket");
        let listener =
            std::os::unix::net::UnixListener::bind(&singleton_sock).expect("bind singleton");
        std::os::unix::fs::symlink(&singleton_sock, profile.join("SingletonSocket"))
            .expect("symlink real SingletonSocket into the profile");
        let forged = tempfile::tempdir().expect("forged profile directory");
        std::os::unix::fs::symlink(&singleton_sock, forged.path().join("SingletonSocket"))
            .expect("symlink aliased SingletonSocket into the forged directory");
        let stdout = unsafe {
            use std::os::fd::{FromRawFd, IntoRawFd};
            std::process::Stdio::from_raw_fd(listener.into_raw_fd())
        };
        let held = std::fs::File::create(profile.join("Default/History"))
            .expect("create a profile file for the browser to hold");

        let packed = format!(
            "/opt/helium-browser-bin/helium {}/report --user-data-dir={}",
            forged.path().display(),
            forged.path().display()
        );
        let process = FakeHeliumProcess::spawn(&packed, profile_root.path(), stdout, held.into());

        let outcome = user_data_dir_for_pid(process.pid());
        assert!(
            matches!(
                &outcome,
                Err(refusal) if refusal.code == BrowserRefusalCode::BrowserBindingAmbiguous
            ),
            "expected a browser_binding_ambiguous refusal, got {outcome:?}"
        );
    }

    #[test]
    fn user_data_dir_for_pid_refuses_a_singleton_socket_owned_by_a_different_process() {
        // A directory can carry a real, live SingletonSocket -- just not one
        // the candidate process holds. Planting someone else's socket must
        // not be enough, mirroring the same ownership discipline the file
        // already applies to a DevToolsActivePort's TCP listener.
        let profile_root = tempfile::tempdir().expect("profile root");
        let profile = profile_root.path().join("profile");
        std::fs::create_dir_all(&profile).expect("create profile directory");
        let singleton_root = tempfile::tempdir().expect("singleton socket root");
        let singleton_sock = singleton_root.path().join("SingletonSocket");
        // Bound and held by THIS test process (the harness), not by the
        // fake Helium child spawned below.
        let _listener_owned_by_harness =
            std::os::unix::net::UnixListener::bind(&singleton_sock).expect("bind singleton");
        std::os::unix::fs::symlink(&singleton_sock, profile.join("SingletonSocket"))
            .expect("symlink real but foreign-owned SingletonSocket");

        let packed = format!(
            "/opt/helium-browser-bin/helium --user-data-dir={}",
            profile.display()
        );
        let process = FakeHeliumProcess::spawn(
            &packed,
            profile_root.path(),
            std::process::Stdio::null(),
            std::process::Stdio::null(),
        );

        let outcome = user_data_dir_for_pid(process.pid());
        assert!(
            matches!(
                &outcome,
                Err(refusal) if refusal.code == BrowserRefusalCode::BrowserBindingAmbiguous
            ),
            "expected a browser_binding_ambiguous refusal, got {outcome:?}"
        );
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
}
