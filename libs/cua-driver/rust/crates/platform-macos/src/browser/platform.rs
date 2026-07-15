//! macOS identity and endpoint evidence for the first-class browser tools.

use std::hash::{Hash, Hasher};
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

#[derive(Debug, Default)]
pub struct MacOsBrowserPlatform;

fn refusal(code: BrowserRefusalCode, message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(code, message)
}

fn is_chromium(name: &str, bundle_id: &str) -> bool {
    let value = format!("{name} {bundle_id}").to_ascii_lowercase();
    let products = [
        "chrome", "chromium", "electron", "brave", "edge", "vivaldi", "opera", "arc", "thorium",
        "iridium", "yandex",
    ];
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| products.contains(&token))
}

fn is_firefox(name: &str, bundle_id: &str) -> bool {
    format!("{name} {bundle_id}")
        .to_ascii_lowercase()
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

fn stable_hash(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn exact_browser_surface_ids(
    windows: impl IntoIterator<Item = crate::windows::WindowInfo>,
    pid: i64,
) -> Vec<u64> {
    windows
        .into_iter()
        .filter(|window| {
            i64::from(window.pid) == pid
                && !window.title.trim().is_empty()
                && window.bounds.width > 0.0
                && window.bounds.height > 0.0
        })
        .map(|window| u64::from(window.window_id))
        .collect()
}

async fn process_details(pid: i64) -> Result<(String, String), BrowserRefusal> {
    let output = tokio::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart=", "-o", "comm="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not inspect browser process {pid}: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} is no longer available"),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} has no process identity"),
        ));
    }
    let split = text
        .char_indices()
        .nth(24)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let (started, executable) = text.split_at(split);
    Ok((started.trim().to_owned(), executable.trim().to_owned()))
}

fn parse_loopback_lsof_ports(text: &str) -> Vec<u16> {
    let mut ports = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix('n'))
        .filter_map(|address| {
            let (host, port) = address.rsplit_once(':')?;
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

async fn loopback_ports_for_pid(pid: i64) -> Result<Vec<u16>, BrowserRefusal> {
    let output = tokio::process::Command::new("lsof")
        .args([
            "-a",
            "-p",
            &pid.to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fn",
            "-P",
        ])
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
    Ok(parse_loopback_lsof_ports(&String::from_utf8_lossy(
        &output.stdout,
    )))
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
impl BrowserPlatform for MacOsBrowserPlatform {
    async fn classify_browser(&self, pid: i64) -> Result<BrowserClassification, BrowserRefusal> {
        let (app, fallback_name, fallback_bundle_id) = tokio::task::spawn_blocking(move || {
            let app = crate::apps::list_running_apps()
                .into_iter()
                .find(|app| i64::from(app.pid) == pid);
            let fallback_name = crate::apps::get_app_name_for_pid(pid as i32);
            let fallback_bundle_id = crate::apps::bundle_id_for_pid(pid as i32);
            (app, fallback_name, fallback_bundle_id)
        })
        .await
        .unwrap_or((None, None, None));
        let name = app
            .as_ref()
            .map(|app| app.name.as_str())
            .or(fallback_name.as_deref())
            .unwrap_or("");
        let bundle_id = app
            .as_ref()
            .and_then(|app| app.bundle_id.as_deref())
            .or(fallback_bundle_id.as_deref())
            .unwrap_or("");
        let chromium = is_chromium(name, bundle_id);
        let webkit = bundle_id == "com.apple.Safari" || name.eq_ignore_ascii_case("Safari");
        let gecko = is_firefox(name, bundle_id);
        Ok(BrowserClassification {
            is_browser: chromium || webkit || gecko,
            engine: if chromium {
                BrowserEngineFamily::Chromium
            } else if webkit {
                BrowserEngineFamily::Webkit
            } else if gecko {
                BrowserEngineFamily::Gecko
            } else {
                BrowserEngineFamily::Unknown
            },
            product: (!name.is_empty()).then(|| name.to_owned()),
            channel: None,
            supports_cdp: chromium,
        })
    }

    async fn native_window(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<NativeWindowInfo, BrowserRefusal> {
        let window_id_u32 = u32::try_from(window_id).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("window_id {window_id} is outside the macOS window-id range"),
            )
        })?;
        let window = tokio::task::spawn_blocking(move || {
            crate::windows::all_windows()
                .into_iter()
                .find(|window| window.window_id == window_id_u32)
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserBindingStale,
                format!("macOS window {window_id} is no longer available"),
            )
        })?;
        if i64::from(window.pid) != pid {
            return Err(refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                format!("window {window_id} is not owned by pid {pid}"),
            ));
        }
        Ok(NativeWindowInfo {
            pid,
            window_id,
            title: window.title,
            bounds: Rect::new(
                window.bounds.x,
                window.bounds.y,
                window.bounds.width,
                window.bounds.height,
            ),
            geometry_exact: true,
            ownership: NativeOwnershipProof {
                method: NativeOwnershipMethod::WindowServerOwner,
                owner_pid: pid,
                detail: Some("CGWindow owner pid".to_owned()),
            },
        })
    }

    async fn is_only_exact_native_window(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<Option<bool>, BrowserRefusal> {
        let windows = tokio::task::spawn_blocking(move || {
            exact_browser_surface_ids(crate::windows::all_windows(), pid)
        })
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not enumerate macOS browser windows: {error}"),
            )
        })?;
        Ok(Some(windows.len() == 1 && windows[0] == window_id))
    }

    async fn discover_owned_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        for port in loopback_ports_for_pid(pid).await? {
            if let Some(ws_url) = browser_websocket_url(port).await {
                return Ok(Some(OwnedEndpoint {
                    ws_url,
                    http_port: Some(port),
                    ownership: EndpointOwnershipProof {
                        method: EndpointOwnershipMethod::ListeningSocketPid,
                        owner_pid: pid,
                        detail: Some("lsof loopback listener owner".to_owned()),
                    },
                }));
            }
        }
        Ok(None)
    }

    async fn process_fingerprint(&self, pid: i64) -> Result<ProcessFingerprint, BrowserRefusal> {
        let (started, executable) = process_details(pid).await?;
        Ok(ProcessFingerprint {
            pid,
            start_time: Some(stable_hash(&started)),
            executable: (!executable.is_empty()).then_some(executable),
        })
    }

    async fn prepare_endpoint(
        &self,
        request: PrepareRequest,
    ) -> Result<PrepareOutcome, BrowserRefusal> {
        if let Some(endpoint) = self.discover_owned_endpoint(request.pid).await? {
            return Ok(PrepareOutcome {
                action: PrepareAction::AlreadyPrepared,
                endpoint: Some(endpoint),
                message: "An owned loopback DevTools endpoint is already available.".to_owned(),
            });
        }
        if !request.consent_granted {
            return Err(refusal(
                BrowserRefusalCode::BrowserConsentRequired,
                "Preparing this browser may require a debug-enabled relaunch; set consent_granted=true after confirming with the user.",
            ));
        }
        Err(refusal(
            BrowserRefusalCode::BrowserRequiresSetup,
            if request.allow_restart {
                "No owned endpoint is available. Relaunch the browser explicitly with launch_app.cdp_debugging_port; browser_prepare does not infer an application profile."
            } else {
                "No owned endpoint is available. Enable a DevTools endpoint explicitly or allow a deliberate browser relaunch."
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(window_id: u32, pid: i32, title: &str) -> crate::windows::WindowInfo {
        crate::windows::WindowInfo {
            window_id,
            pid,
            app_name: "Electron".to_owned(),
            title: title.to_owned(),
            bounds: crate::windows::WindowBounds {
                x: 120.0,
                y: 120.0,
                width: 940.0,
                height: 780.0,
            },
            layer: 0,
            z_index: 0,
            is_on_screen: true,
            on_current_space: Some(true),
            space_ids: None,
        }
    }

    #[test]
    fn lsof_parser_accepts_only_loopback_listeners() {
        let input = "n127.0.0.1:9222\nn*:9333\nn[::1]:9444\nn0.0.0.0:9555\n";
        assert_eq!(parse_loopback_lsof_ports(input), vec![9222, 9444]);
    }

    #[test]
    fn browser_classifier_covers_embedded_and_standalone_chromium() {
        assert!(is_chromium("Electron", "com.example.fixture"));
        assert!(is_chromium("Google Chrome", "com.google.Chrome"));
        assert!(!is_chromium("Safari", "com.apple.Safari"));
        assert!(!is_chromium("Search", "com.example.Search"));
        assert!(!is_chromium("Operator", "com.example.Operator"));
    }

    #[test]
    fn firefox_classifier_uses_product_tokens() {
        assert!(is_firefox("Firefox", "org.mozilla.firefox"));
        assert!(is_firefox("Mozilla Firefox", "org.mozilla.firefox"));
        assert!(!is_firefox("FirefoxHelper", "com.example.FirefoxHelper"));
        assert!(!is_firefox("Waterfox", "net.waterfox.current"));
    }

    #[test]
    fn websocket_url_must_keep_the_attested_listener_port() {
        assert_eq!(
            loopback_websocket_port("ws://127.0.0.1:9222/devtools/browser/id"),
            Some(9222)
        );
        assert_ne!(
            loopback_websocket_port("ws://localhost:9333/devtools/browser/foreign"),
            Some(9222)
        );
        assert_eq!(
            loopback_websocket_port("ws://192.0.2.1:9222/devtools"),
            None
        );
    }

    #[test]
    fn embedded_window_proof_ignores_untitled_window_server_helpers() {
        let mut helper = window(8, 42, "");
        helper.is_on_screen = false;
        let ids = exact_browser_surface_ids([window(7, 42, "CuaTestHarness Electron"), helper], 42);
        assert_eq!(ids, vec![7]);
    }

    #[test]
    fn embedded_window_proof_retains_multiple_titled_browser_surfaces() {
        let ids = exact_browser_surface_ids(
            [
                window(7, 42, "Primary browser window"),
                window(8, 42, "Secondary browser window"),
            ],
            42,
        );
        assert_eq!(ids, vec![7, 8]);
    }
}
