//! Windows identity and endpoint evidence for the first-class browser tools.

use std::process::Stdio;
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
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, RECT};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

#[derive(Debug, Default)]
pub struct WindowsBrowserPlatform;

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

fn parse_netstat_loopback_ports(text: &str, pid: u32) -> Vec<u16> {
    let mut ports = text
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 5
                || !fields[0].eq_ignore_ascii_case("TCP")
                || !fields[3].eq_ignore_ascii_case("LISTENING")
                || fields[4].parse::<u32>().ok() != Some(pid)
            {
                return None;
            }
            let local = fields[1];
            let (host, port) = local.rsplit_once(':')?;
            let host = host.trim_matches(['[', ']']);
            matches!(host, "127.0.0.1" | "::1" | "localhost")
                .then(|| port.parse::<u16>().ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    ports.sort_unstable();
    ports.dedup();
    ports
}

async fn loopback_ports_for_pid(pid: u32) -> Result<Vec<u16>, BrowserRefusal> {
    let output = tokio::process::Command::new("netstat.exe")
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
    Ok(parse_netstat_loopback_ports(
        &String::from_utf8_lossy(&output.stdout),
        pid,
    ))
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
impl BrowserPlatform for WindowsBrowserPlatform {
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
        Ok(BrowserClassification {
            is_browser: chromium || gecko,
            engine: if chromium {
                BrowserEngineFamily::Chromium
            } else if gecko {
                BrowserEngineFamily::Gecko
            } else {
                BrowserEngineFamily::Unknown
            },
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
            crate::win32::list_windows(Some(pid_u32))
                .into_iter()
                .find(|window| window.hwnd == window_id)
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
            crate::win32::list_windows(Some(pid_u32))
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
        for port in loopback_ports_for_pid(pid_u32).await? {
            if let Some(ws_url) = browser_websocket_url(port).await {
                return Ok(Some(OwnedEndpoint {
                    ws_url,
                    http_port: Some(port),
                    ownership: EndpointOwnershipProof {
                        method: EndpointOwnershipMethod::ListeningSocketPid,
                        owner_pid: pid,
                        detail: Some("netstat listener owner pid".to_owned()),
                    },
                }));
            }
        }
        Ok(None)
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
    fn netstat_parser_requires_loopback_listening_and_exact_pid() {
        let input = "\
  TCP    127.0.0.1:9222       0.0.0.0:0       LISTENING       42\n\
  TCP    0.0.0.0:9333         0.0.0.0:0       LISTENING       42\n\
  TCP    [::1]:9444           [::]:0          LISTENING       42\n\
  TCP    127.0.0.1:9555       0.0.0.0:0       LISTENING       7\n";
        assert_eq!(parse_netstat_loopback_ports(input, 42), vec![9222, 9444]);
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
    fn firefox_classifier_uses_product_tokens() {
        assert!(is_firefox("firefox.exe"));
        assert!(is_firefox("Mozilla Firefox.exe"));
        assert!(!is_firefox("FirefoxHelper.exe"));
        assert!(!is_firefox("waterfox.exe"));
    }

    #[test]
    fn websocket_url_must_keep_the_attested_listener_port() {
        assert_eq!(
            loopback_websocket_port("ws://[::1]:9222/devtools/browser/id"),
            Some(9222)
        );
        assert_ne!(
            loopback_websocket_port("ws://127.0.0.1:9333/devtools/browser/foreign"),
            Some(9222)
        );
        assert_eq!(
            loopback_websocket_port("wss://127.0.0.1:9222/devtools"),
            None
        );
    }
}
