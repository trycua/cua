//! Windows identity and endpoint evidence for the first-class browser tools.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::browser::existing_profile_setup_descriptor;
use cua_driver_core::browser::platform::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserPlatform, BrowserVisualAction,
    BrowserVisualActionKind, ExistingProfileSetupOutcome, ExistingProfileSetupRequest,
    PrepareAction, PrepareOutcome, PrepareRequest,
};
use cua_driver_core::browser::refusal::{BrowserRefusal, BrowserRefusalCode};
use cua_driver_core::browser::types::{
    BrowserClassification, BrowserEngineFamily, BrowserProduct, EndpointOwnershipMethod,
    EndpointOwnershipProof, NativeOwnershipMethod, NativeOwnershipProof, NativeWindowInfo,
    OwnedEndpoint, ProcessFingerprint, Rect,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, RECT};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetWindowRect, GA_ROOT};

#[derive(Clone)]
pub struct WindowsBrowserPlatform {
    cursor_registry: Arc<cursor_overlay::CursorRegistry>,
    browser_cursors: Arc<Mutex<BrowserCursorTracker>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserCursorBinding {
    window_id: u64,
    cdp_target_id: String,
}

#[derive(Debug, Default)]
struct BrowserCursorTracker {
    bindings: HashMap<String, BrowserCursorBinding>,
}

impl BrowserCursorTracker {
    fn update(
        &mut self,
        session: &str,
        window_id: u64,
        cdp_target_id: &str,
        tab_is_active: bool,
    ) -> Vec<(String, bool)> {
        self.bindings.insert(
            session.to_owned(),
            BrowserCursorBinding {
                window_id,
                cdp_target_id: cdp_target_id.to_owned(),
            },
        );

        if !tab_is_active {
            return vec![(session.to_owned(), false)];
        }

        self.bindings
            .iter()
            .filter(|(_, binding)| binding.window_id == window_id)
            .map(|(key, binding)| {
                (
                    key.clone(),
                    key == session && binding.cdp_target_id == cdp_target_id,
                )
            })
            .collect()
    }
}

impl WindowsBrowserPlatform {
    pub fn new(cursor_registry: Arc<cursor_overlay::CursorRegistry>) -> Self {
        Self {
            cursor_registry,
            browser_cursors: Arc::new(Mutex::new(BrowserCursorTracker::default())),
        }
    }
}

impl Default for WindowsBrowserPlatform {
    fn default() -> Self {
        Self::new(Arc::new(cursor_overlay::CursorRegistry::new()))
    }
}

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn is_chromium(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let products = [
        "chrome", "chromium", "electron", "msedge", "brave", "vivaldi", "opera", "arc", "thorium",
        "iridium", "yandex",
    ];
    name.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| products.contains(&token))
}

fn is_firefox(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token == "firefox")
}

fn browser_product(name: &str) -> BrowserProduct {
    let executable = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    match executable.trim_end_matches(".exe") {
        "chrome" => BrowserProduct::GoogleChrome,
        "chromium" => BrowserProduct::Chromium,
        "msedge" => BrowserProduct::MicrosoftEdge,
        "brave" => BrowserProduct::Brave,
        "vivaldi" => BrowserProduct::Vivaldi,
        "opera" => BrowserProduct::Opera,
        "arc" => BrowserProduct::Arc,
        "electron" => BrowserProduct::Electron,
        "firefox" => BrowserProduct::Firefox,
        _ => BrowserProduct::Other,
    }
}

fn allows_embedded_descendant_endpoint(executable_path: &str) -> bool {
    let executable = executable_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(executable_path);
    browser_product(executable) == BrowserProduct::Other && !is_chromium(executable)
}

fn is_embedded_webview_runtime(executable_path: &str) -> bool {
    executable_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(executable_path)
        .eq_ignore_ascii_case("msedgewebview2.exe")
}

fn listener_process_belongs_to_root_lifetime(root_started: u64, listener_started: u64) -> bool {
    listener_started >= root_started
}

#[derive(Debug)]
struct LifetimeScopedProcessTree {
    pids: Vec<u32>,
    started_at: HashMap<u32, u64>,
}

fn lifetime_scoped_descendants_from_processes(
    root_pid: u32,
    processes: &[crate::win32::ProcessInfo],
    mut started_at: impl FnMut(u32) -> Option<u64>,
) -> Option<LifetimeScopedProcessTree> {
    let root_started = started_at(root_pid)?;
    let mut pids = vec![root_pid];
    let mut starts = HashMap::from([(root_pid, root_started)]);
    let mut frontier = vec![root_pid];
    while let Some(parent_pid) = frontier.pop() {
        let parent_started = starts[&parent_pid];
        for process in processes {
            if process.parent_pid != parent_pid || starts.contains_key(&process.pid) {
                continue;
            }
            let Some(child_started) = started_at(process.pid) else {
                continue;
            };
            // Toolhelp parent ids are not lifetime-scoped. Validate every
            // parent -> child edge so pid reuse anywhere in the transitive
            // tree cannot graft an older unrelated process onto this root.
            if !listener_process_belongs_to_root_lifetime(parent_started, child_started) {
                continue;
            }
            pids.push(process.pid);
            starts.insert(process.pid, child_started);
            frontier.push(process.pid);
        }
    }
    Some(LifetimeScopedProcessTree {
        pids,
        started_at: starts,
    })
}

fn retain_identity_matched_listeners(
    observed: Vec<(u16, u32)>,
    expected_starts: &HashMap<u32, u64>,
    mut current_started_at: impl FnMut(u32) -> Option<u64>,
) -> Vec<(u16, u32)> {
    observed
        .into_iter()
        .filter(|(_port, owner_pid)| {
            expected_starts
                .get(owner_pid)
                .zip(current_started_at(*owner_pid))
                .is_some_and(|(expected, current)| *expected == current)
        })
        .collect()
}

fn websocket_port_and_suffix<'a>(url: &'a str, prefix: &str) -> Option<(u16, &'a str)> {
    let remainder = url.strip_prefix(prefix)?;
    let path_start = remainder.find('/')?;
    let port = remainder[..path_start].parse::<u16>().ok()?;
    Some((port, &remainder[path_start..]))
}

fn literal_loopback_websocket_port(url: &str) -> Option<u16> {
    ["ws://127.0.0.1:", "ws://[::1]:"]
        .iter()
        .find_map(|prefix| websocket_port_and_suffix(url, prefix).map(|(port, _)| port))
}

fn canonical_discovered_websocket_url(url: &str, expected_port: u16) -> Option<String> {
    ["ws://127.0.0.1:", "ws://[::1]:", "ws://localhost:"]
        .iter()
        .find_map(|prefix| websocket_port_and_suffix(url, prefix))
        .and_then(|(port, suffix)| {
            (port == expected_port).then(|| format!("ws://127.0.0.1:{expected_port}{suffix}"))
        })
}

fn process_identity(pid: u32) -> Result<(u64, Option<String>), BrowserRefusal> {
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                format!("browser process {pid} is no longer available"),
            )
        })?;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let times =
        unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    let mut path_buf = [0u16; 1024];
    let mut path_len = path_buf.len() as u32;
    let path = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(path_buf.as_mut_ptr()),
            &mut path_len,
        )
    }
    .ok()
    .filter(|_| path_len > 0)
    .map(|_| String::from_utf16_lossy(&path_buf[..path_len as usize]));
    let _ = unsafe { CloseHandle(handle) };
    times.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not fingerprint browser process {pid}: {error}"),
        )
    })?;
    let started = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    Ok((started, path))
}

fn cdp_comparable_window_bounds(window_id: u64) -> Result<Rect, BrowserRefusal> {
    let hwnd = HWND(window_id as *mut _);
    let mut outer = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut outer) }.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("could not read Windows outer bounds for window {window_id}: {error}"),
        )
    })?;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let scale = if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 };
    Ok(Rect::new(
        f64::from(outer.left) / scale,
        f64::from(outer.top) / scale,
        f64::from(outer.right - outer.left) / scale,
        f64::from(outer.bottom - outer.top) / scale,
    ))
}

fn overlay_window_and_scale(window_id: u64) -> Option<(u64, f64)> {
    let hwnd = HWND(window_id as *mut _);
    if hwnd.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let overlay_window = if root.0.is_null() {
        window_id
    } else {
        root.0 as u64
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let scale = if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 };
    Some((overlay_window, scale))
}

fn parse_netstat_loopback_listeners(text: &str, allowed_pids: &[u32]) -> Vec<(u16, u32)> {
    let mut listeners = text
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 5
                || !fields[0].eq_ignore_ascii_case("TCP")
                || !fields[3].eq_ignore_ascii_case("LISTENING")
            {
                return None;
            }
            let owner_pid = fields[4].parse::<u32>().ok()?;
            if !allowed_pids.contains(&owner_pid) {
                return None;
            }
            let local = fields[1];
            let (host, port) = local.rsplit_once(':')?;
            let host = host.trim_matches(['[', ']']);
            matches!(host, "127.0.0.1" | "::1" | "localhost")
                .then(|| port.parse::<u16>().ok().map(|port| (port, owner_pid)))
                .flatten()
        })
        .collect::<Vec<_>>();
    listeners.sort_unstable();
    listeners.dedup();
    listeners
}

#[cfg(test)]
fn parse_netstat_loopback_ports(text: &str, allowed_pids: &[u32]) -> Vec<u16> {
    let mut ports = parse_netstat_loopback_listeners(text, allowed_pids)
        .into_iter()
        .map(|(port, _owner_pid)| port)
        .collect::<Vec<_>>();
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn system_netstat_path() -> Result<PathBuf, BrowserRefusal> {
    let mut buffer = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "could not resolve the trusted Windows system directory",
        ));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("netstat.exe"))
}

async fn netstat_loopback_listeners(
    allowed_pids: &[u32],
) -> Result<Vec<(u16, u32)>, BrowserRefusal> {
    let netstat = system_netstat_path()?;
    let output = tokio::process::Command::new(netstat)
        .args(["-ano", "-p", "tcp"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not inspect browser listeners: {error}"),
            )
        })?;
    Ok(parse_netstat_loopback_listeners(
        &String::from_utf8_lossy(&output.stdout),
        allowed_pids,
    ))
}

async fn raw_loopback_listeners_for_process_tree(
    root_pid: u32,
) -> Result<Vec<(u16, u32)>, BrowserRefusal> {
    let allowed_pids =
        tokio::task::spawn_blocking(move || crate::win32::list_descendants(root_pid))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not inspect browser process tree: {error}"),
                )
            })?;
    let observed = netstat_loopback_listeners(&allowed_pids).await?;
    tokio::task::spawn_blocking(move || {
        observed
            .into_iter()
            .filter(|(_port, owner_pid)| process_identity(*owner_pid).is_ok())
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not inspect spawned browser listener identities: {error}"),
        )
    })
}

async fn loopback_listeners_for_process_tree(
    root_pid: u32,
) -> Result<Vec<(u16, u32)>, BrowserRefusal> {
    let tree = tokio::task::spawn_blocking(move || {
        let processes = crate::win32::list_processes();
        lifetime_scoped_descendants_from_processes(root_pid, &processes, |pid| {
            process_identity(pid).ok().map(|identity| identity.0)
        })
    })
    .await
    .map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not inspect browser process lifetimes: {error}"),
        )
    })?
    .ok_or_else(|| {
        refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {root_pid} is no longer available"),
        )
    })?;

    let observed = netstat_loopback_listeners(&tree.pids).await?;
    let expected_starts = tree.started_at;
    tokio::task::spawn_blocking(move || {
        retain_identity_matched_listeners(observed, &expected_starts, |pid| {
            process_identity(pid).ok().map(|identity| identity.0)
        })
    })
    .await
    .map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not reprove browser listener identities: {error}"),
        )
    })
}

async fn loopback_listeners_for_exact_pid(pid: u32) -> Result<Vec<(u16, u32)>, BrowserRefusal> {
    let expected_started =
        tokio::task::spawn_blocking(move || process_identity(pid).map(|identity| identity.0))
            .await
            .map_err(|error| {
                refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    format!("could not inspect browser process identity: {error}"),
                )
            })??;
    let observed = netstat_loopback_listeners(&[pid]).await?;
    tokio::task::spawn_blocking(move || {
        retain_identity_matched_listeners(
            observed,
            &HashMap::from([(pid, expected_started)]),
            |candidate_pid| {
                process_identity(candidate_pid)
                    .ok()
                    .map(|identity| identity.0)
            },
        )
    })
    .await
    .map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not reprove browser process identity: {error}"),
        )
    })
}

async fn loopback_ports_for_exact_pid(pid: u32) -> Result<Vec<u16>, BrowserRefusal> {
    let mut ports = loopback_listeners_for_exact_pid(pid)
        .await?
        .into_iter()
        .map(|(port, _owner_pid)| port)
        .collect::<Vec<_>>();
    ports.sort_unstable();
    ports.dedup();
    Ok(ports)
}

async fn unfiltered_loopback_ports_for_exact_pid(pid: u32) -> Result<Vec<u16>, BrowserRefusal> {
    let mut ports = netstat_loopback_listeners(&[pid])
        .await?
        .into_iter()
        .map(|(port, _owner_pid)| port)
        .collect::<Vec<_>>();
    ports.sort_unstable();
    ports.dedup();
    Ok(ports)
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
        let url = value.get("webSocketDebuggerUrl")?.as_str()?;
        canonical_discovered_websocket_url(url, port)
    })
    .await
    .ok()
    .flatten()
}

const ENDPOINT_DISCOVERY_ATTEMPTS: usize = 4;
const ENDPOINT_DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(100);

async fn browser_endpoints_once(pid: u32) -> Result<Vec<(u16, String, u32)>, BrowserRefusal> {
    let mut endpoints = Vec::new();
    for (port, owner_pid) in loopback_listeners_for_exact_pid(pid).await? {
        if let Some(ws_url) = browser_websocket_url(port).await {
            // Re-read both the socket owner and process identity after the
            // HTTP probe so pid recycling during discovery cannot become
            // exact ownership evidence.
            let reproved = loopback_listeners_for_exact_pid(pid).await?;
            if reproved.contains(&(port, owner_pid)) {
                endpoints.push((port, ws_url, owner_pid));
            } else {
                tracing::debug!(
                    browser_pid = pid,
                    listener_pid = owner_pid,
                    port,
                    "discarding browser endpoint whose listener ownership changed during discovery"
                );
            }
        }
    }
    Ok(endpoints)
}

async fn retry_empty_endpoint_discovery<T, F, Fut>(
    attempts: usize,
    delay: Duration,
    mut probe: F,
) -> Result<Vec<T>, BrowserRefusal>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Vec<T>, BrowserRefusal>>,
{
    let attempts = attempts.max(1);
    for attempt in 0..attempts {
        let endpoints = probe().await?;
        if !endpoints.is_empty() || attempt + 1 == attempts {
            return Ok(endpoints);
        }
        tokio::time::sleep(delay).await;
    }
    unreachable!("the bounded endpoint-discovery loop always returns")
}

async fn exact_browser_endpoints_for_pid(
    pid: u32,
) -> Result<Vec<(u16, String, u32)>, BrowserRefusal> {
    retry_empty_endpoint_discovery(
        ENDPOINT_DISCOVERY_ATTEMPTS,
        ENDPOINT_DISCOVERY_RETRY_DELAY,
        || browser_endpoints_once(pid),
    )
    .await
}

async fn root_can_use_embedded_descendant_endpoint(pid: u32) -> Result<bool, BrowserRefusal> {
    tokio::task::spawn_blocking(move || {
        let (_started, executable) = process_identity(pid)?;
        Ok(executable.is_some_and(|path| allows_embedded_descendant_endpoint(&path)))
    })
    .await
    .map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not classify browser process identity: {error}"),
        )
    })?
}

async fn embedded_browser_endpoints_once(
    pid: u32,
) -> Result<Vec<(u16, String, u32)>, BrowserRefusal> {
    let mut endpoints = Vec::new();
    for (port, listener_pid) in loopback_listeners_for_process_tree(pid).await? {
        let is_webview_runtime = tokio::task::spawn_blocking(move || {
            process_identity(listener_pid)
                .ok()
                .and_then(|identity| identity.1)
                .is_some_and(|path| is_embedded_webview_runtime(&path))
        })
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not classify embedded browser listener: {error}"),
            )
        })?;
        if !is_webview_runtime {
            continue;
        }
        if let Some(ws_url) = browser_websocket_url(port).await {
            let reproved = loopback_listeners_for_process_tree(pid).await?;
            if reproved.contains(&(port, listener_pid)) {
                endpoints.push((port, ws_url, listener_pid));
            }
        }
    }
    Ok(endpoints)
}

async fn browser_endpoints_for_pid(pid: u32) -> Result<Vec<(u16, String, u32)>, BrowserRefusal> {
    let exact = exact_browser_endpoints_for_pid(pid).await?;
    if !exact.is_empty() || !root_can_use_embedded_descendant_endpoint(pid).await? {
        return Ok(exact);
    }
    // Native embedded hosts such as Tauri/WPF own the window while a
    // WebView2 child owns DevTools. Standalone Chromium/Electron executables
    // never enter this fallback: their endpoint must be owned by the exact
    // approved pid.
    retry_empty_endpoint_discovery(
        ENDPOINT_DISCOVERY_ATTEMPTS,
        ENDPOINT_DISCOVERY_RETRY_DELAY,
        || embedded_browser_endpoints_once(pid),
    )
    .await
}

async fn loopback_listeners_for_spawned_tree(
    root_pid: u32,
) -> Result<Vec<(u16, u32)>, BrowserRefusal> {
    match loopback_listeners_for_process_tree(root_pid).await {
        Ok(listeners) => Ok(listeners),
        // Edge on Windows ARM can transfer the browser role to a descendant
        // and let its launcher exit. This fallback is used only while core
        // attests the exact private-profile DevTools URL it just read from
        // DevToolsActivePort; ordinary and existing-profile discovery never
        // accept descendant-owned endpoints.
        Err(error) if error.code == BrowserRefusalCode::BrowserBindingStale => {
            raw_loopback_listeners_for_process_tree(root_pid).await
        }
        Err(error) => Err(error),
    }
}

async fn spawned_browser_endpoints_once(
    root_pid: u32,
    expected_ws_url: &str,
) -> Result<Vec<(u16, String, u32)>, BrowserRefusal> {
    let Some(expected_port) = literal_loopback_websocket_port(expected_ws_url) else {
        return Err(refusal(
            BrowserRefusalCode::BrowserEndpointOwnerMismatch,
            "the driver-spawned browser endpoint is not loopback-only",
        ));
    };
    let mut endpoints = Vec::new();
    for (port, listener_pid) in loopback_listeners_for_spawned_tree(root_pid)
        .await?
        .into_iter()
        .filter(|(port, _listener_pid)| *port == expected_port)
    {
        if browser_websocket_url(port).await.as_deref() != Some(expected_ws_url) {
            continue;
        }
        let reproved = loopback_listeners_for_spawned_tree(root_pid).await?;
        if reproved.contains(&(port, listener_pid)) {
            endpoints.push((port, expected_ws_url.to_owned(), listener_pid));
        }
    }
    Ok(endpoints)
}

async fn spawned_browser_endpoints_for_pid(
    root_pid: u32,
    expected_ws_url: &str,
) -> Result<Vec<(u16, String, u32)>, BrowserRefusal> {
    retry_empty_endpoint_discovery(
        ENDPOINT_DISCOVERY_ATTEMPTS,
        ENDPOINT_DISCOVERY_RETRY_DELAY,
        || spawned_browser_endpoints_once(root_pid, expected_ws_url),
    )
    .await
}

fn owned_endpoint_from_listener(
    root_pid: i64,
    port: u16,
    ws_url: String,
    listener_pid: u32,
    context: &str,
) -> OwnedEndpoint {
    OwnedEndpoint {
        ws_url,
        http_port: Some(port),
        ownership: EndpointOwnershipProof {
            method: EndpointOwnershipMethod::ListeningSocketPid,
            // The discovery route proved listener_pid under its documented
            // ownership scope. Core authorizes and fingerprints root_pid;
            // retain the exact socket owner separately for audit evidence and
            // the narrow Windows launcher-handoff promotion path.
            owner_pid: root_pid,
            listener_pid: Some(i64::from(listener_pid)),
            detail: Some(format!(
                "{context}; exact loopback listener pid {listener_pid}"
            )),
        },
    }
}

fn select_unique_owned_endpoint(
    root_pid: i64,
    discovered: Vec<(u16, String, u32)>,
    context: &str,
) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
    match discovered.as_slice() {
        [] => Ok(None),
        [(port, ws_url, listener_pid)] => Ok(Some(owned_endpoint_from_listener(
            root_pid,
            *port,
            ws_url.clone(),
            *listener_pid,
            context,
        ))),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "multiple browser-level DevTools endpoints satisfy the approved ownership scope",
        )
        .with_detail(serde_json::json!({
            "candidates": discovered
                .iter()
                .map(|(port, _ws_url, listener_pid)| serde_json::json!({
                    "port": port,
                    "listener_pid": listener_pid,
                }))
                .collect::<Vec<_>>(),
        }))),
    }
}

async fn loopback_port_is_owned_with_retry(
    pid: u32,
    expected_port: u16,
) -> Result<bool, BrowserRefusal> {
    retry_port_ownership(
        ENDPOINT_DISCOVERY_ATTEMPTS,
        ENDPOINT_DISCOVERY_RETRY_DELAY,
        expected_port,
        || loopback_ports_for_exact_pid(pid),
    )
    .await
}

async fn retry_port_ownership<F, Fut>(
    attempts: usize,
    delay: Duration,
    expected_port: u16,
    mut probe: F,
) -> Result<bool, BrowserRefusal>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Vec<u16>, BrowserRefusal>>,
{
    let attempts = attempts.max(1);
    for attempt in 0..attempts {
        if probe().await?.contains(&expected_port) {
            return Ok(true);
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(delay).await;
        }
    }
    Ok(false)
}

#[async_trait]
impl BrowserPlatform for WindowsBrowserPlatform {
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
            crate::overlay::send_command(key, cursor_overlay::OverlayCommand::SetEnabled(enabled));
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
        let Some((overlay_window, scale)) = overlay_window_and_scale(action.window_id) else {
            return;
        };
        let screen_x = screen_x * scale;
        let screen_y = screen_y * scale;

        crate::overlay::send_command(
            action.session.clone(),
            cursor_overlay::OverlayCommand::PinAbove(overlay_window),
        );
        crate::overlay::animate_cursor_to(action.session.clone(), screen_x, screen_y).await;
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
            crate::overlay::send_command(
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
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        let name = tokio::task::spawn_blocking(move || {
            crate::win32::list_processes()
                .into_iter()
                .find(|process| process.pid == pid_u32)
                .map(|process| process.name)
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                format!("browser process {pid} is no longer available"),
            )
        })?;
        let chromium = is_chromium(&name);
        let gecko = is_firefox(&name);
        let product_kind = browser_product(&name);
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
            product: Some(name),
            channel: None,
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
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        let window = tokio::task::spawn_blocking(move || {
            crate::win32::find_window_by_pid_and_handle(pid_u32, window_id)
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                format!("Windows window {window_id} is not owned by pid {pid}"),
            )
        })?;
        // CDP Browser.getWindowBounds reports Chromium's outer Win32 rect,
        // including the invisible resize border. General window enumeration
        // intentionally uses DWM's visible frame instead, so obtain the
        // correlation geometry directly from GetWindowRect here.
        let bounds = cdp_comparable_window_bounds(window_id)?;
        Ok(NativeWindowInfo {
            pid,
            window_id,
            title: window.title,
            bounds,
            geometry_exact: true,
            ownership: NativeOwnershipProof {
                method: NativeOwnershipMethod::WindowServerOwner,
                owner_pid: pid,
                detail: Some("GetWindowThreadProcessId".to_owned()),
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
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        let windows = tokio::task::spawn_blocking(move || {
            crate::win32::list_windows_via_win32(Some(pid_u32))
                .into_iter()
                .map(|window| window.hwnd)
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not enumerate Windows browser windows: {error}"),
            )
        })?;
        Ok(Some(windows.len() == 1 && windows[0] == window_id))
    }

    async fn discover_owned_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        select_unique_owned_endpoint(
            pid,
            browser_endpoints_for_pid(pid_u32).await?,
            "listener owned by the exact approved browser pid or its classified embedded webview tree",
        )
    }

    async fn discover_spawned_endpoint(
        &self,
        pid: i64,
        expected_ws_url: &str,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        select_unique_owned_endpoint(
            pid,
            spawned_browser_endpoints_for_pid(pid_u32, expected_ws_url).await?,
            "exact private-profile endpoint owned by the driver-spawned browser tree",
        )
    }

    async fn discover_existing_profile_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        select_unique_owned_endpoint(
            pid,
            exact_browser_endpoints_for_pid(pid_u32).await?,
            "Windows exact browser-pid listener plus /json/version",
        )
    }

    async fn reprove_existing_profile_endpoint(
        &self,
        pid: i64,
        expected_ws_url: &str,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        let Some(port) = literal_loopback_websocket_port(expected_ws_url) else {
            return Err(refusal(
                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                "the approved existing-profile endpoint is not loopback-only",
            ));
        };
        // Setup may approve the exact PID-owned port before Chromium publishes
        // its final browser WebSocket id. Reprove that stable port ownership
        // here; the connection layer still uses the approved WebSocket path.
        if !loopback_port_is_owned_with_retry(pid_u32, port).await? {
            return Ok(None);
        }
        Ok(Some(OwnedEndpoint {
            ws_url: expected_ws_url.to_owned(),
            http_port: Some(port),
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
                listener_pid: None,
                detail: Some("Windows exact browser-pid owner of approved endpoint".to_owned()),
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
                "the approved browser pid is outside the Windows process-id range",
            )
        })?;
        let hwnd = request.window_id;
        // The subtraction baseline must remain a conservative superset: a
        // transient identity-reproof failure must not make an old exact-pid
        // listener appear newly created after the approved setup action.
        let listeners_before = unfiltered_loopback_ports_for_exact_pid(pid_u32).await?;
        let handle =
            tokio::task::spawn_blocking(move || crate::browser_setup_ui::enable(hwnd, descriptor))
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
            let ports = match loopback_ports_for_exact_pid(pid_u32).await {
                Ok(ports) => ports,
                Err(error) => break Err(error),
            };
            let mut endpoints = Vec::new();
            for port in &ports {
                if let Some(ws_url) = browser_websocket_url(*port).await {
                    endpoints.push((
                        *port,
                        ws_url,
                        "Windows exact browser-pid owner plus /json/version",
                    ));
                }
            }
            if endpoints.is_empty() {
                let correlated = ports
                    .iter()
                    .copied()
                    .filter(|port| !listeners_before.contains(port))
                    .collect::<Vec<_>>();
                if let [port] = correlated.as_slice() {
                    endpoints.push((
                        *port,
                        format!("ws://127.0.0.1:{port}/devtools/browser"),
                        "new exact browser-pid listener correlated with approved setup",
                    ));
                } else if correlated.len() > 1 {
                    break Err(refusal(
                        BrowserRefusalCode::BrowserBindingAmbiguous,
                        format!(
                            "{} exposed multiple newly correlated exact-pid listeners",
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
                            "{} did not expose a uniquely exact-pid-owned loopback endpoint after the exact setup action",
                            descriptor.product_name
                        ),
                    ))
                }
                _ => {
                    break Err(refusal(
                        BrowserRefusalCode::BrowserBindingAmbiguous,
                        format!(
                            "{} exposed multiple exact-pid-owned endpoint candidates after the exact setup action",
                            descriptor.product_name
                        ),
                    ))
                }
            }
        };
        let endpoint = match endpoint_result {
            Ok(endpoint) => endpoint,
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
        crate::browser_setup_ui::retain_pending(hwnd, handle)?;

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
        tokio::task::spawn_blocking(move || {
            crate::browser_setup_ui::commit_pending(request.window_id)
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
        tokio::task::spawn_blocking(move || {
            crate::browser_setup_ui::abort_pending(request.window_id, error)
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
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        let (start_time, executable) =
            tokio::task::spawn_blocking(move || process_identity(pid_u32))
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
        if let Some(endpoint) = self.discover_owned_endpoint(request.pid).await? {
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn browser_cursor_tracker_shows_only_the_active_tabs_session_per_window() {
        let mut tracker = BrowserCursorTracker::default();
        assert_eq!(
            tracker.update("tab-a", 101, "target-a", false),
            vec![("tab-a".to_owned(), false)]
        );
        assert_eq!(
            tracker.update("tab-b", 101, "target-b", false),
            vec![("tab-b".to_owned(), false)]
        );

        let mut updates = tracker.update("tab-a", 101, "target-a", true);
        updates.sort();
        assert_eq!(
            updates,
            vec![("tab-a".to_owned(), true), ("tab-b".to_owned(), false)]
        );

        let mut updates = tracker.update("tab-b", 101, "target-b", true);
        updates.sort();
        assert_eq!(
            updates,
            vec![("tab-a".to_owned(), false), ("tab-b".to_owned(), true)]
        );
    }

    #[test]
    fn netstat_parser_requires_loopback_listening_and_browser_process_tree() {
        let input = "\
  TCP    127.0.0.1:9222       0.0.0.0:0       LISTENING       42\n\
  TCP    0.0.0.0:9333         0.0.0.0:0       LISTENING       43\n\
  TCP    [::1]:9444           [::]:0          LISTENING       43\n\
  TCP    127.0.0.1:9555       0.0.0.0:0       LISTENING       7\n";
        assert_eq!(
            parse_netstat_loopback_ports(input, &[42, 43]),
            vec![9222, 9444]
        );
    }

    #[test]
    fn netstat_parser_rejects_unrelated_process_trees() {
        let input = "\
  TCP    127.0.0.1:9222       0.0.0.0:0       LISTENING       42\n\
  TCP    127.0.0.1:9555       0.0.0.0:0       LISTENING       99\n";
        assert_eq!(parse_netstat_loopback_ports(input, &[42, 43]), vec![9222]);
    }

    #[test]
    fn netstat_parser_preserves_the_exact_listener_owner() {
        let input = "\
  TCP    127.0.0.1:9222       0.0.0.0:0       LISTENING       43\n\
  TCP    127.0.0.1:9555       0.0.0.0:0       LISTENING       99\n";
        assert_eq!(
            parse_netstat_loopback_listeners(input, &[42, 43]),
            vec![(9222, 43)]
        );
    }

    #[test]
    fn process_tree_endpoint_uses_the_authorized_root_and_retains_listener_evidence() {
        let endpoint = owned_endpoint_from_listener(
            42,
            9222,
            "ws://127.0.0.1:9222/devtools/browser/id".to_owned(),
            43,
            "verified process tree",
        );

        assert_eq!(endpoint.ownership.owner_pid, 42);
        assert_eq!(endpoint.ownership.listener_pid, Some(43));
        assert_eq!(endpoint.http_port, Some(9222));
        assert!(endpoint
            .ownership
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("exact loopback listener pid 43")));
    }

    #[test]
    fn owned_endpoint_selection_requires_exactly_one_lifetime_matched_listener() {
        assert!(
            select_unique_owned_endpoint(42, Vec::new(), "verified process tree")
                .expect("empty discovery is not an error")
                .is_none()
        );

        let selected = select_unique_owned_endpoint(
            42,
            vec![(
                9222,
                "ws://127.0.0.1:9222/devtools/browser/edge".to_owned(),
                43,
            )],
            "verified process tree",
        )
        .expect("one lifetime-matched listener")
        .expect("selected endpoint");
        assert_eq!(selected.http_port, Some(9222));
        assert_eq!(selected.ownership.listener_pid, Some(43));

        let ambiguous = select_unique_owned_endpoint(
            42,
            vec![
                (
                    9222,
                    "ws://127.0.0.1:9222/devtools/browser/a".to_owned(),
                    43,
                ),
                (
                    9333,
                    "ws://127.0.0.1:9333/devtools/browser/b".to_owned(),
                    44,
                ),
            ],
            "verified process tree",
        )
        .expect_err("multiple lifetime-matched listeners must be refused");
        assert_eq!(ambiguous.code, BrowserRefusalCode::BrowserBindingAmbiguous);
        let candidates = ambiguous
            .detail
            .as_ref()
            .and_then(|detail| detail.get("candidates"))
            .and_then(serde_json::Value::as_array)
            .expect("candidate detail");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0]["port"], 9222);
        assert_eq!(candidates[0]["listener_pid"], 43);
        assert_eq!(candidates[1]["port"], 9333);
        assert_eq!(candidates[1]["listener_pid"], 44);
    }

    #[test]
    fn classifier_covers_embedded_and_standalone_chromium() {
        assert!(is_chromium("CuaTestHarness.Electron.exe"));
        assert!(is_chromium("msedge.exe"));
        assert!(!is_chromium("firefox.exe"));
        assert!(!is_chromium("Operator.exe"));
        assert!(!is_chromium("Knowledge.exe"));
    }

    #[test]
    fn only_native_embedded_hosts_may_use_descendant_owned_devtools() {
        assert!(allows_embedded_descendant_endpoint(
            r"D:\fixtures\CuaTestHarness.Tauri.exe"
        ));
        assert!(allows_embedded_descendant_endpoint(
            r"D:\fixtures\CuaTestHarness.WebView.exe"
        ));
        assert!(!allows_embedded_descendant_endpoint(
            r"C:\Program Files\Google\Chrome\Application\chrome.exe"
        ));
        assert!(!allows_embedded_descendant_endpoint(
            r"D:\fixtures\CuaTestHarness.Electron.exe"
        ));
        assert!(is_embedded_webview_runtime(
            r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application\msedgewebview2.exe"
        ));
        assert!(!is_embedded_webview_runtime(
            r"D:\fixtures\CuaTestHarness.Electron.exe"
        ));
    }

    #[test]
    fn endpoint_listener_cannot_predate_the_authorized_browser_root() {
        assert!(listener_process_belongs_to_root_lifetime(100, 100));
        assert!(listener_process_belongs_to_root_lifetime(100, 101));
        assert!(!listener_process_belongs_to_root_lifetime(100, 99));
    }

    #[test]
    fn lifetime_scoped_tree_validates_every_parent_child_edge() {
        let processes = vec![
            crate::win32::ProcessInfo {
                pid: 42,
                parent_pid: 1,
                name: "msedge.exe".to_owned(),
            },
            crate::win32::ProcessInfo {
                pid: 43,
                parent_pid: 42,
                name: "msedge.exe".to_owned(),
            },
            crate::win32::ProcessInfo {
                pid: 44,
                parent_pid: 43,
                name: "CuaTestHarness.Electron.exe".to_owned(),
            },
            crate::win32::ProcessInfo {
                pid: 45,
                parent_pid: 42,
                name: "msedge.exe".to_owned(),
            },
        ];
        let starts = HashMap::from([
            (42, 100),
            // This pid was reused by a real child of the new browser root.
            (43, 300),
            // This unrelated process names pid 43 as its parent but predates
            // that incarnation, so validating only against root 42 is unsafe.
            (44, 200),
            (45, 400),
        ]);

        let tree = lifetime_scoped_descendants_from_processes(42, &processes, |pid| {
            starts.get(&pid).copied()
        })
        .expect("live root");
        assert_eq!(tree.pids, vec![42, 43, 45]);
        assert!(!tree.pids.contains(&44));
    }

    #[test]
    fn lifetime_scoped_tree_drops_unidentifiable_processes_and_their_children() {
        let processes = vec![
            crate::win32::ProcessInfo {
                pid: 42,
                parent_pid: 1,
                name: "chrome.exe".to_owned(),
            },
            crate::win32::ProcessInfo {
                pid: 43,
                parent_pid: 42,
                name: "chrome.exe".to_owned(),
            },
            crate::win32::ProcessInfo {
                pid: 44,
                parent_pid: 43,
                name: "chrome.exe".to_owned(),
            },
        ];
        let starts = HashMap::from([(42, 100), (44, 300)]);

        let tree = lifetime_scoped_descendants_from_processes(42, &processes, |pid| {
            starts.get(&pid).copied()
        })
        .expect("live root");
        assert_eq!(tree.pids, vec![42]);
    }

    #[test]
    fn listener_identity_reproof_drops_recycled_and_vanished_pids() {
        let observed = vec![(9222, 42), (9333, 43), (9444, 44)];
        let expected = HashMap::from([(42, 100), (43, 200), (44, 300)]);
        let current = HashMap::from([
            (42, 100),
            // pid 43 was recycled between the socket snapshot and reproof.
            (43, 201),
            // pid 44 vanished and is intentionally absent.
        ]);

        assert_eq!(
            retain_identity_matched_listeners(observed, &expected, |pid| {
                current.get(&pid).copied()
            }),
            vec![(9222, 42)]
        );
    }

    #[test]
    fn firefox_classifier_uses_product_tokens() {
        assert!(is_firefox("firefox.exe"));
        assert!(is_firefox("Mozilla Firefox.exe"));
        assert!(!is_firefox("FirefoxHelper.exe"));
        assert!(!is_firefox("waterfox.exe"));
    }

    #[test]
    fn discovered_websocket_url_is_canonical_and_keeps_the_attested_port() {
        assert_eq!(
            canonical_discovered_websocket_url("ws://localhost:9222/devtools/browser/id", 9222),
            Some("ws://127.0.0.1:9222/devtools/browser/id".to_owned())
        );
        assert_eq!(
            canonical_discovered_websocket_url("ws://[::1]:9222/devtools/browser/id", 9222),
            Some("ws://127.0.0.1:9222/devtools/browser/id".to_owned())
        );
        assert_eq!(
            canonical_discovered_websocket_url(
                "ws://127.0.0.1:9333/devtools/browser/foreign",
                9222
            ),
            None
        );
        assert_eq!(
            canonical_discovered_websocket_url("wss://127.0.0.1:9222/devtools/browser/id", 9222),
            None
        );
        assert_eq!(
            canonical_discovered_websocket_url(
                "ws://127.0.0.1.evil.test:9222/devtools/browser/id",
                9222
            ),
            None
        );
        assert_eq!(
            canonical_discovered_websocket_url(
                "ws://user@127.0.0.1:9222/devtools/browser/id",
                9222
            ),
            None
        );
    }

    #[test]
    fn endpoint_reproof_requires_a_literal_loopback_url() {
        assert_eq!(
            literal_loopback_websocket_port("ws://127.0.0.1:9222/devtools/browser/id"),
            Some(9222)
        );
        assert_eq!(
            literal_loopback_websocket_port("ws://[::1]:9222/devtools/browser/id"),
            Some(9222)
        );
        assert_eq!(
            literal_loopback_websocket_port("ws://localhost:9222/devtools/browser/id"),
            None
        );
        assert_eq!(literal_loopback_websocket_port("ws://127.0.0.1:9222"), None);
    }

    #[tokio::test]
    async fn endpoint_discovery_retries_only_empty_socket_snapshots() {
        let calls = Arc::new(AtomicUsize::new(0));
        let probe_calls = Arc::clone(&calls);
        let endpoints = retry_empty_endpoint_discovery(4, Duration::ZERO, move || {
            let call = probe_calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if call < 2 {
                    Ok(Vec::new())
                } else {
                    Ok(vec![(
                        9222,
                        "ws://127.0.0.1:9222/devtools/browser/proven".to_owned(),
                        43,
                    )])
                }
            }
        })
        .await
        .expect("retry transient empty socket snapshots");

        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            endpoints,
            vec![(
                9222,
                "ws://127.0.0.1:9222/devtools/browser/proven".to_owned(),
                43,
            )]
        );
    }

    #[tokio::test]
    async fn endpoint_ownership_retries_transient_socket_snapshots() {
        let calls = Arc::new(AtomicUsize::new(0));
        let probe_calls = Arc::clone(&calls);
        let owned = retry_port_ownership(4, Duration::ZERO, 9222, move || {
            let call = probe_calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if call < 2 {
                    Ok(Vec::new())
                } else {
                    Ok(vec![9222])
                }
            }
        })
        .await
        .expect("retry transient socket snapshots");

        assert!(owned);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn endpoint_ownership_exhausts_wrong_ports_and_fails_fast_on_errors() {
        let wrong_calls = Arc::new(AtomicUsize::new(0));
        let probe_calls = Arc::clone(&wrong_calls);
        let owned = retry_port_ownership(3, Duration::ZERO, 9222, move || {
            probe_calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(vec![9333]) }
        })
        .await
        .expect("wrong ports are a clean ownership miss");
        assert!(!owned);
        assert_eq!(wrong_calls.load(Ordering::SeqCst), 3);

        let error_calls = Arc::new(AtomicUsize::new(0));
        let probe_calls = Arc::clone(&error_calls);
        let result = retry_port_ownership(3, Duration::ZERO, 9222, move || {
            probe_calls.fetch_add(1, Ordering::SeqCst);
            async {
                Err(refusal(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    "socket inventory failed",
                ))
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(error_calls.load(Ordering::SeqCst), 1);
    }
}
