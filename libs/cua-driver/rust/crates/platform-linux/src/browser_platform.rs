//! Linux identity and endpoint evidence for the first-class browser tools.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::browser::existing_profile_setup_descriptor;
use cua_driver_core::browser::platform::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserPlatform, ExistingProfileSetupOutcome,
    ExistingProfileSetupRequest, PrepareAction, PrepareOutcome, PrepareRequest,
};
use cua_driver_core::browser::refusal::{BrowserRefusal, BrowserRefusalCode};
use cua_driver_core::browser::types::{
    BrowserClassification, BrowserEngineFamily, BrowserProduct, EndpointOwnershipMethod,
    EndpointOwnershipProof, NativeOwnershipMethod, NativeOwnershipProof, NativeWindowInfo,
    OwnedEndpoint, ProcessFingerprint, Rect,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Default)]
pub struct LinuxBrowserPlatform;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn is_chromium(name: &str) -> bool {
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

fn socket_inodes_for_pid(pid: i64) -> Result<HashSet<u64>, BrowserRefusal> {
    let directory = std::fs::read_dir(format!("/proc/{pid}/fd")).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} is no longer available"),
        )
    })?;
    Ok(directory
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter_map(|target| {
            let target = target.to_string_lossy();
            target
                .strip_prefix("socket:[")
                .and_then(|value| value.strip_suffix(']'))
                .and_then(|value| value.parse::<u64>().ok())
        })
        .collect())
}

fn loopback_ports_for_pid(pid: i64) -> Result<Vec<u16>, BrowserRefusal> {
    let owned = socket_inodes_for_pid(pid)?;
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
        _ => return None,
    };
    Some(home.join(relative))
}

fn user_data_dir_for_pid(pid: i64) -> Result<Option<PathBuf>, BrowserRefusal> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} is no longer available"),
        )
    })?;
    let args = bytes
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect::<Vec<_>>();
    let mut directories = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if let Some(path) = arg.strip_prefix("--user-data-dir=") {
            if !path.is_empty() {
                directories.push(PathBuf::from(path));
            }
        } else if arg == "--user-data-dir" {
            if let Some(path) = args.get(index + 1).filter(|path| !path.is_empty()) {
                directories.push(PathBuf::from(path));
            }
        }
    }
    directories.sort();
    directories.dedup();
    match directories.as_slice() {
        [] => {
            let Some(executable) = std::fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
            else {
                return Ok(None);
            };
            Ok(default_user_data_dir(browser_product(&executable)))
        }
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
    }
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
        ownership: EndpointOwnershipProof {
            method: EndpointOwnershipMethod::DevtoolsActivePortsFile,
            owner_pid: pid,
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
        if let Some(endpoint) = active_port_endpoint(pid)? {
            return Ok(Some(endpoint));
        }
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
                    ownership: EndpointOwnershipProof {
                        method: EndpointOwnershipMethod::ListeningSocketPid,
                        owner_pid: pid,
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
                ownership: EndpointOwnershipProof {
                    method: EndpointOwnershipMethod::ListeningSocketPid,
                    owner_pid: pid,
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
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
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
        if self
            .is_only_exact_native_window(request.pid, request.window_id)
            .await?
            != Some(true)
        {
            return Err(refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "existing-profile setup on Linux requires exactly one PID-owned native window; generic Wayland and multi-window processes are refused",
            ));
        }
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
                if correlated.len() > 1 {
                    break Err(refusal(
                        BrowserRefusalCode::BrowserBindingAmbiguous,
                        format!(
                            "{} exposed multiple newly correlated PID-owned listeners",
                            descriptor.product_name
                        ),
                    ));
                }
                // A newly correlated listener proves process ownership, but it
                // does not yet reveal Chrome's UUID-bearing browser WebSocket
                // path. Keep polling `/json/version` instead of fabricating
                // `/devtools/browser`, which is not a connectable endpoint.
            }
            match endpoints.as_slice() {
                [(port, ws_url, detail)] => {
                    break Ok(OwnedEndpoint {
                        ws_url: ws_url.clone(),
                        http_port: Some(*port),
                        ownership: EndpointOwnershipProof {
                            method: EndpointOwnershipMethod::ListeningSocketPid,
                            owner_pid: request.pid,
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
    fn classifier_covers_embedded_and_standalone_chromium() {
        assert!(is_chromium("CuaTestHarness.Electron"));
        assert!(is_chromium("chromium-browser"));
        assert!(is_chromium("msedge --remote-debugging-port=9222"));
        assert!(is_chromium("/opt/microsoft/msedge/msedge"));
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
