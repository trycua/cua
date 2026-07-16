//! Linux identity and endpoint evidence for the first-class browser tools.

use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::browser::platform::{
    BrowserPlatform, PrepareAction, PrepareOutcome, PrepareRequest,
};
use cua_driver_core::browser::refusal::{BrowserRefusal, BrowserRefusalCode};
use cua_driver_core::browser::types::{
    BrowserClassification, BrowserEngineFamily, EndpointOwnershipMethod, EndpointOwnershipProof,
    NativeOwnershipMethod, NativeOwnershipProof, NativeWindowInfo, OwnedEndpoint,
    ProcessFingerprint, Rect,
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
        "chrome", "chromium", "electron", "brave", "edge", "vivaldi", "opera", "arc", "thorium",
        "iridium", "yandex",
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
        let process = tokio::task::spawn_blocking(move || {
            crate::proc_fs::list_processes()
                .into_iter()
                .find(|process| process.pid == pid_u32)
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
        let identity = format!("{} {}", process.name, process.cmdline);
        let chromium = is_chromium(&identity);
        let gecko = is_firefox(&identity);
        Ok(BrowserClassification {
            is_browser: chromium || gecko,
            engine: if chromium {
                BrowserEngineFamily::Chromium
            } else if gecko {
                BrowserEngineFamily::Gecko
            } else {
                BrowserEngineFamily::Unknown
            },
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
        assert!(!is_chromium("firefox"));
        assert!(!is_chromium("search-worker"));
        assert!(!is_chromium("ledger-service"));
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
}
