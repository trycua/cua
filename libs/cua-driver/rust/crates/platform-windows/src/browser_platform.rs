//! Windows identity and endpoint evidence for the first-class browser tools.

use std::future::Future;
use std::process::Stdio;
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

const ENDPOINT_DISCOVERY_ATTEMPTS: usize = 4;
const ENDPOINT_DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(100);

async fn browser_endpoints_once(pid: u32) -> Result<Vec<(u16, String)>, BrowserRefusal> {
    let mut endpoints = Vec::new();
    for port in loopback_ports_for_pid(pid).await? {
        if let Some(ws_url) = browser_websocket_url(port).await {
            endpoints.push((port, ws_url));
        }
    }
    Ok(endpoints)
}

async fn retry_empty_endpoint_discovery<F, Fut>(
    attempts: usize,
    delay: Duration,
    mut probe: F,
) -> Result<Vec<(u16, String)>, BrowserRefusal>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Vec<(u16, String)>, BrowserRefusal>>,
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

async fn browser_endpoints_for_pid(pid: u32) -> Result<Vec<(u16, String)>, BrowserRefusal> {
    retry_empty_endpoint_discovery(
        ENDPOINT_DISCOVERY_ATTEMPTS,
        ENDPOINT_DISCOVERY_RETRY_DELAY,
        || browser_endpoints_once(pid),
    )
    .await
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
        Ok(browser_endpoints_for_pid(pid_u32)
            .await?
            .into_iter()
            .next()
            .map(|(port, ws_url)| OwnedEndpoint {
                ws_url,
                http_port: Some(port),
                ownership: EndpointOwnershipProof {
                    method: EndpointOwnershipMethod::ListeningSocketPid,
                    owner_pid: pid,
                    detail: Some("netstat listener owner pid".to_owned()),
                },
            }))
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
        let discovered = browser_endpoints_for_pid(pid_u32).await?;
        match discovered.as_slice() {
            [] => Ok(None),
            [(port, ws_url)] => Ok(Some(OwnedEndpoint {
                ws_url: ws_url.clone(),
                http_port: Some(*port),
                ownership: EndpointOwnershipProof {
                    method: EndpointOwnershipMethod::ListeningSocketPid,
                    owner_pid: pid,
                    detail: Some("Windows listener owner plus /json/version".to_owned()),
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
        let pid_u32 = u32::try_from(pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("pid {pid} is outside the Windows process-id range"),
            )
        })?;
        let Some(port) = loopback_websocket_port(expected_ws_url) else {
            return Err(refusal(
                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                "the approved existing-profile endpoint is not loopback-only",
            ));
        };
        let endpoint_is_owned = browser_endpoints_for_pid(pid_u32).await?.iter().any(
            |(candidate_port, candidate_ws_url)| {
                *candidate_port == port && candidate_ws_url == expected_ws_url
            },
        );
        if !endpoint_is_owned {
            return Ok(None);
        }
        Ok(Some(OwnedEndpoint {
            ws_url: expected_ws_url.to_owned(),
            http_port: Some(port),
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
                detail: Some("Windows owner of exact approved endpoint".to_owned()),
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
        let listeners_before = loopback_ports_for_pid(pid_u32).await?;
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
            let ports = match loopback_ports_for_pid(pid_u32).await {
                Ok(ports) => ports,
                Err(error) => break Err(error),
            };
            let mut endpoints = Vec::new();
            for port in &ports {
                if let Some(ws_url) = browser_websocket_url(*port).await {
                    endpoints.push((*port, ws_url, "Windows owner plus /json/version"));
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
                        "new PID-owned listener correlated with exact approved setup",
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
        crate::browser_setup_ui::retain_pending(hwnd, handle)?;

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
            )]
        );
    }
}
