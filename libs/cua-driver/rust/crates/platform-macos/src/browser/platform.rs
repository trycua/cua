//! macOS identity and endpoint evidence for the first-class browser tools.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cua_driver_core::browser::existing_profile_setup_descriptor;
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
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CONSENT_CLEANUP_PASSES: usize = 20;

#[derive(Clone, Debug, Serialize)]
struct ConsentSurfaceEvidence {
    window_id: u32,
    pid: i32,
    bounds: [f64; 4],
}

fn consent_surface_evidence(
    windows: impl IntoIterator<Item = crate::windows::WindowInfo>,
    pid: i32,
) -> Vec<ConsentSurfaceEvidence> {
    let mut surfaces = windows
        .into_iter()
        .filter(|window| {
            window.pid == pid
                && window.title == "Allow remote debugging?"
                && window.is_on_screen
                && window.bounds.width > 0.0
                && window.bounds.height > 0.0
        })
        .map(|window| ConsentSurfaceEvidence {
            window_id: window.window_id,
            pid: window.pid,
            bounds: [
                window.bounds.x,
                window.bounds.y,
                window.bounds.width,
                window.bounds.height,
            ],
        })
        .collect::<Vec<_>>();
    surfaces.sort_by_key(|surface| surface.window_id);
    surfaces
}

fn visible_consent_surfaces(pid: i32) -> Vec<ConsentSurfaceEvidence> {
    consent_surface_evidence(crate::windows::all_windows(), pid)
}

fn add_consent_cleanup_detail(
    mut error: BrowserRefusal,
    cleanup: serde_json::Value,
) -> BrowserRefusal {
    let prior = error.detail.take();
    error.detail = Some(match prior {
        Some(serde_json::Value::Object(mut detail)) => {
            detail.insert("consent_cleanup".to_owned(), cleanup);
            serde_json::Value::Object(detail)
        }
        Some(cause) => serde_json::json!({
            "cause": cause,
            "consent_cleanup": cleanup,
        }),
        None => serde_json::json!({ "consent_cleanup": cleanup }),
    });
    error
}

fn run_failed_prepare_consent_cleanup<D, R, O, P>(
    error: BrowserRefusal,
    max_passes: usize,
    mut dismiss: D,
    rollback: R,
    mut observe: O,
    mut pause: P,
) -> BrowserRefusal
where
    D: FnMut() -> Result<bool, BrowserRefusal>,
    R: FnOnce(BrowserRefusal) -> BrowserRefusal,
    O: FnMut() -> Vec<ConsentSurfaceEvidence>,
    P: FnMut(),
{
    let mut dismissed_via_ax = false;
    let mut dismiss_error = None;
    match dismiss() {
        Ok(dismissed) => dismissed_via_ax |= dismissed,
        Err(error) => dismiss_error = Some(error),
    }

    // The retained setup handle is the exact authorization to undo the
    // setting change. Always run it even when consent UI is AX-ambiguous.
    let error = rollback(error);
    let mut retained_surfaces = Vec::new();
    for pass in 0..max_passes {
        match dismiss() {
            Ok(dismissed) => {
                dismissed_via_ax |= dismissed;
                dismiss_error = None;
            }
            Err(error) => dismiss_error = Some(error),
        }
        retained_surfaces = observe();
        if pass + 1 < max_passes {
            pause();
        }
    }

    let status = if retained_surfaces.is_empty() {
        "dismissed"
    } else {
        "retained"
    };
    add_consent_cleanup_detail(
        error,
        serde_json::json!({
            "status": status,
            "dismissed_via_ax": dismissed_via_ax,
            "retained_surfaces": retained_surfaces,
            "dismiss_error": dismiss_error,
        }),
    )
}

fn run_committed_consent_cleanup<D, C, O, P>(
    max_passes: usize,
    mut dismiss: D,
    cleanup_setup: C,
    mut observe: O,
    mut pause: P,
) -> Result<bool, BrowserRefusal>
where
    D: FnMut() -> Result<bool, BrowserRefusal>,
    C: FnOnce() -> Result<bool, BrowserRefusal>,
    O: FnMut() -> Vec<ConsentSurfaceEvidence>,
    P: FnMut(),
{
    let mut changed = false;
    let mut dismiss_error = None;
    match dismiss() {
        Ok(dismissed) => changed |= dismissed,
        Err(error) => dismiss_error = Some(error),
    }
    let cleanup_result = cleanup_setup();
    let cleanup_error = cleanup_result.as_ref().err().cloned();
    if let Ok(cleaned) = cleanup_result.as_ref() {
        changed |= *cleaned;
    }

    let mut retained_surfaces = Vec::new();
    for pass in 0..max_passes {
        match dismiss() {
            Ok(dismissed) => {
                changed |= dismissed;
                dismiss_error = None;
            }
            Err(error) => dismiss_error = Some(error),
        }
        retained_surfaces = observe();
        if pass + 1 < max_passes {
            pause();
        }
    }

    if !retained_surfaces.is_empty() {
        let mut error = cleanup_error.unwrap_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "remote-debugging cleanup retained browser consent UI",
            )
        });
        error.detail = Some(serde_json::json!({
            "status": "retained",
            "retained_surfaces": retained_surfaces,
            "dismiss_error": dismiss_error,
        }));
        return Err(error);
    }

    cleanup_result.map(|_| changed)
}

#[derive(Clone)]
pub struct MacOsBrowserPlatform {
    cursor_registry: Arc<crate::cursor::CursorRegistry>,
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
    /// Record the session-to-tab binding and return the exact overlay
    /// visibility changes needed for this action. An active tab owns the sole
    /// visible browser cursor in its native window. An inactive tab hides only
    /// its own cursor and never disturbs the cursor for the selected tab.
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

impl MacOsBrowserPlatform {
    pub fn new(cursor_registry: Arc<crate::cursor::CursorRegistry>) -> Self {
        Self {
            cursor_registry,
            browser_cursors: Arc::new(Mutex::new(BrowserCursorTracker::default())),
        }
    }
}

impl Default for MacOsBrowserPlatform {
    fn default() -> Self {
        Self::new(Arc::new(crate::cursor::CursorRegistry::new()))
    }
}

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

fn browser_product(name: &str, bundle_id: &str) -> BrowserProduct {
    let name = name.to_ascii_lowercase();
    let bundle_id = bundle_id.to_ascii_lowercase();
    if bundle_id.starts_with("com.google.chrome") || name == "google chrome" {
        BrowserProduct::GoogleChrome
    } else if bundle_id.starts_with("com.microsoft.edgemac") || name == "microsoft edge" {
        BrowserProduct::MicrosoftEdge
    } else if bundle_id.starts_with("org.chromium.chromium") || name == "chromium" {
        BrowserProduct::Chromium
    } else if bundle_id.starts_with("com.brave.browser") || name == "brave browser" {
        BrowserProduct::Brave
    } else if bundle_id.starts_with("com.vivaldi.vivaldi") || name == "vivaldi" {
        BrowserProduct::Vivaldi
    } else if bundle_id.starts_with("com.operasoftware.opera") || name == "opera" {
        BrowserProduct::Opera
    } else if bundle_id.starts_with("company.thebrowser.browser") || name == "arc" {
        BrowserProduct::Arc
    } else if bundle_id == "com.apple.safari" || name == "safari" {
        BrowserProduct::Safari
    } else if bundle_id.contains("firefox") || name == "firefox" {
        BrowserProduct::Firefox
    } else if bundle_id.contains("electron") || name == "electron" {
        BrowserProduct::Electron
    } else {
        BrowserProduct::Other
    }
}

fn isolated_browser_candidates() -> Vec<(PathBuf, &'static str, &'static str)> {
    // pid-free launch deliberately supports only vendor-signed system-wide
    // installations. Chromium builds have no stable vendor team identity, so
    // callers can still select one by supplying its already-running pid.
    vec![
        (
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            "com.google.Chrome",
            "EQHXZ8M8AV",
        ),
        (
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            "com.microsoft.edgemac",
            "UBF8T346G9",
        ),
    ]
}

fn codesign_identity_matches(details: &str, identifier: &str, team_identifier: &str) -> bool {
    let mut observed_identifier = None;
    let mut observed_team = None;
    for line in details.lines() {
        if let Some(value) = line.strip_prefix("Identifier=") {
            observed_identifier = Some(value.trim());
        } else if let Some(value) = line.strip_prefix("TeamIdentifier=") {
            observed_team = Some(value.trim());
        }
    }
    observed_identifier == Some(identifier) && observed_team == Some(team_identifier)
}

fn has_trusted_codesign_identity(
    executable: &std::path::Path,
    identifier: &str,
    team_identifier: &str,
) -> bool {
    let requirement = format!(
        "=anchor apple generic and certificate leaf[subject.OU] = \"{team_identifier}\" and identifier \"{identifier}\""
    );
    let verified = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--test-requirement", &requirement])
        .arg(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !verified {
        return false;
    }
    let Ok(details) = std::process::Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(executable)
        .stdin(Stdio::null())
        .output()
    else {
        return false;
    };
    details.status.success()
        && codesign_identity_matches(
            &String::from_utf8_lossy(&details.stderr),
            identifier,
            team_identifier,
        )
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

fn default_user_data_dir(product: BrowserProduct) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let relative = match product {
        BrowserProduct::GoogleChrome => "Library/Application Support/Google/Chrome",
        BrowserProduct::MicrosoftEdge => "Library/Application Support/Microsoft Edge",
        BrowserProduct::Chromium => "Library/Application Support/Chromium",
        _ => return None,
    };
    Some(home.join(relative))
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

fn process_arguments(pid: i64) -> Result<Vec<Vec<u8>>, BrowserRefusal> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| {
        refusal(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "browser pid is outside the macOS process-id range",
        )
    })?;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size = 0usize;
    let size_result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if size_result != 0 || size < std::mem::size_of::<libc::c_int>() {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} arguments are unavailable"),
        ));
    }
    let mut bytes = vec![0u8; size];
    let read_result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read_result != 0 {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} arguments changed during inspection"),
        ));
    }
    bytes.truncate(size);
    let argc = libc::c_int::from_ne_bytes(
        bytes[..std::mem::size_of::<libc::c_int>()]
            .try_into()
            .expect("c_int byte width"),
    );
    if argc <= 0 {
        return Ok(Vec::new());
    }
    let mut cursor = std::mem::size_of::<libc::c_int>();
    while cursor < bytes.len() && bytes[cursor] != 0 {
        cursor += 1;
    }
    while cursor < bytes.len() && bytes[cursor] == 0 {
        cursor += 1;
    }
    let mut args = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        if cursor >= bytes.len() {
            break;
        }
        let end = bytes[cursor..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|offset| cursor + offset)
            .unwrap_or(bytes.len());
        args.push(bytes[cursor..end].to_vec());
        cursor = end.saturating_add(1);
    }
    Ok(args)
}

fn user_data_dir_from_arguments(
    args: &[Vec<u8>],
    default: Option<PathBuf>,
) -> Result<Option<PathBuf>, BrowserRefusal> {
    use std::os::unix::ffi::OsStringExt;

    let mut directories = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if let Some(path) = arg.strip_prefix(b"--user-data-dir=") {
            if !path.is_empty() {
                directories.push(PathBuf::from(std::ffi::OsString::from_vec(path.to_vec())));
            }
        } else if arg == b"--user-data-dir" {
            if let Some(path) = args.get(index + 1).filter(|path| !path.is_empty()) {
                directories.push(PathBuf::from(std::ffi::OsString::from_vec(path.clone())));
            }
        }
    }
    directories.sort();
    directories.dedup();
    match directories.as_slice() {
        [] => Ok(default),
        [path] if path.is_absolute() => Ok(Some(path.clone())),
        [_] => Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "the browser uses a relative --user-data-dir that cannot be attested without its launch working directory",
        )),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "browser process has multiple distinct --user-data-dir arguments",
        )),
    }
}

async fn user_data_dir_for_pid(
    pid: i64,
    product: BrowserProduct,
) -> Result<Option<PathBuf>, BrowserRefusal> {
    tokio::task::spawn_blocking(move || {
        let args = process_arguments(pid)?;
        user_data_dir_from_arguments(&args, default_user_data_dir(product))
    })
    .await
    .map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("browser argument inspection task failed: {error}"),
        )
    })?
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

async fn active_port_endpoint(
    pid: i64,
    product: BrowserProduct,
) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
    // The Chrome 144+ existing-profile bridge deliberately returns 404 for
    // legacy /json discovery. Its exact browser WebSocket path is instead
    // published in the exact user-data root's DevToolsActivePort file. argv
    // comes from KERN_PROCARGS2 rather than lossy `ps` text.
    let Some(user_data_dir) = user_data_dir_for_pid(pid, product).await? else {
        return Ok(None);
    };
    let text = match tokio::fs::read_to_string(user_data_dir.join("DevToolsActivePort")).await {
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
    if !loopback_ports_for_pid(pid).await?.contains(&port) {
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
                "exact default-profile DevToolsActivePort path plus lsof loopback listener owner"
                    .to_owned(),
            ),
        },
    }))
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
    fn isolated_browser_executable(&self) -> Result<String, BrowserRefusal> {
        for (candidate, identifier, team_identifier) in isolated_browser_candidates() {
            let Ok(executable) = select_isolated_browser_executable([candidate]) else {
                continue;
            };
            if has_trusted_codesign_identity(
                std::path::Path::new(&executable),
                identifier,
                team_identifier,
            ) {
                return Ok(executable);
            }
        }
        Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "no vendor-signed system Chromium executable is available for isolated launch",
        ))
    }

    fn standalone_trusted_input_background_limitation(&self) -> Option<&'static str> {
        Some("Chromium's trusted CDP Input route activates its standalone browser window on macOS")
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
            crate::cursor::overlay::send_command(
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

        crate::cursor::overlay::send_command(
            action.session.clone(),
            cursor_overlay::OverlayCommand::PinAbove(action.window_id),
        );
        crate::cursor::overlay::animate_cursor_to(action.session.clone(), screen_x, screen_y).await;
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
            crate::cursor::overlay::send_command(
                action.session,
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: screen_x,
                    y: screen_y,
                },
            );
        }
    }

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
        let product_kind = browser_product(name, bundle_id);
        let helper = name.to_ascii_lowercase().contains("helper")
            || bundle_id.to_ascii_lowercase().contains(".helper")
            || process_arguments(pid).ok().is_some_and(|args| {
                args.iter().any(|arg| {
                    arg.starts_with(b"--type=") && arg.get(7..).is_some_and(|kind| !kind.is_empty())
                })
            });
        let process_role = if helper {
            BrowserProcessRole::Helper
        } else if product_kind == BrowserProduct::Electron {
            BrowserProcessRole::EmbeddedApplication
        } else if matches!(
            product_kind,
            BrowserProduct::GoogleChrome
                | BrowserProduct::Chromium
                | BrowserProduct::MicrosoftEdge
                | BrowserProduct::Brave
                | BrowserProduct::Vivaldi
                | BrowserProduct::Opera
                | BrowserProduct::Arc
        ) {
            BrowserProcessRole::StandaloneConsumer
        } else {
            BrowserProcessRole::Unknown
        };
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
            product_kind,
            product: (!name.is_empty()).then(|| name.to_owned()),
            channel: (process_role == BrowserProcessRole::StandaloneConsumer).then(|| {
                if bundle_id.to_ascii_lowercase().contains("canary") {
                    "canary".to_owned()
                } else if bundle_id.to_ascii_lowercase().contains("beta") {
                    "beta".to_owned()
                } else if bundle_id.to_ascii_lowercase().contains("dev") {
                    "dev".to_owned()
                } else {
                    "stable".to_owned()
                }
            }),
            process_role,
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
                    transport: EndpointTransport::LegacyJsonVersion,
                    ownership: EndpointOwnershipProof {
                        method: EndpointOwnershipMethod::ListeningSocketPid,
                        owner_pid: pid,
                        listener_pid: None,
                        detail: Some("lsof loopback listener owner".to_owned()),
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
        let classification = self.classify_browser(pid).await?;
        if let Some(endpoint) = active_port_endpoint(pid, classification.product_kind).await? {
            return Ok(Some(endpoint));
        }
        let ports = loopback_ports_for_pid(pid).await?;
        let mut discovered = Vec::new();
        for port in &ports {
            if let Some(ws_url) = browser_websocket_url(*port).await {
                discovered.push((*port, ws_url));
            }
        }
        if discovered.len() == 1 {
            let (port, ws_url) = discovered.pop().expect("one discovered endpoint");
            return Ok(Some(OwnedEndpoint {
                ws_url,
                http_port: Some(port),
                transport: EndpointTransport::LegacyJsonVersion,
                ownership: EndpointOwnershipProof {
                    method: EndpointOwnershipMethod::ListeningSocketPid,
                    owner_pid: pid,
                    listener_pid: None,
                    detail: Some("lsof loopback listener owner plus /json/version".to_owned()),
                },
            }));
        }
        if discovered.len() > 1 {
            return Err(refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "multiple browser-level DevTools endpoints are owned by the approved process",
            ));
        }
        // A bare listener is not enough to identify DevTools before approval:
        // Chromium can own unrelated loopback services. The setup path below
        // correlates a listener with the exact checkbox transition and the
        // pooled WebSocket claim proves the protocol before attachment.
        Ok(None)
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
        if !loopback_ports_for_pid(pid).await?.contains(&port) {
            return Ok(None);
        }
        let classification = self.classify_browser(pid).await?;
        if let Some(endpoint) = active_port_endpoint(pid, classification.product_kind).await? {
            if endpoint.ws_url != expected_ws_url {
                return Err(refusal(
                    BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                    "the browser's exact DevTools endpoint changed after approval",
                ));
            }
            return Ok(Some(endpoint));
        }
        Ok(Some(OwnedEndpoint {
            ws_url: expected_ws_url.to_owned(),
            http_port: Some(port),
            transport: EndpointTransport::LegacyJsonVersion,
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
                listener_pid: None,
                detail: Some("lsof owner of exact approved endpoint".to_owned()),
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
        let pid = i32::try_from(request.pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser pid is outside the macOS process-id range",
            )
        })?;
        let window_id = u32::try_from(request.window_id).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser window is outside the macOS window-id range",
            )
        })?;
        let listeners_before = loopback_ports_for_pid(request.pid).await?;
        let handle = crate::focus_guard::with_focus_suppressed_now(
            Some(pid),
            "browser_prepare.remote_debugging",
            || async move {
                tokio::task::spawn_blocking(move || {
                    super::setup_ui::enable(pid, window_id, descriptor)
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
                })?
            },
        )
        .await?;
        let opened_setup_page = handle.opened_setup_page;
        let enabled_remote_debugging = handle.enabled_remote_debugging;
        let used_bounded_pixel_fallback = handle.used_bounded_pixel_fallback;
        let focused_setup_address_field = handle.focused_setup_address_field;
        let foregrounded_window = handle.foregrounded_window;
        let injected_global_input = handle.injected_global_input;

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let endpoint_result = loop {
            match active_port_endpoint(request.pid, request.browser).await {
                Ok(Some(endpoint)) => break Ok(endpoint),
                Ok(None) => {}
                Err(error) => break Err(error),
            }
            let ports = match loopback_ports_for_pid(request.pid).await {
                Ok(ports) => ports,
                Err(error) => break Err(error),
            };
            let mut endpoints = Vec::new();
            for port in &ports {
                if let Some(ws_url) = browser_websocket_url(*port).await {
                    endpoints.push((*port, ws_url, "lsof owner plus /json/version"));
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
            Err(error) => {
                let error =
                    tokio::task::spawn_blocking(move || handle.abort(pid, window_id, error))
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
        super::setup_ui::retain_pending(pid, window_id, handle)?;

        Ok(ExistingProfileSetupOutcome {
            opened_setup_page,
            closed_setup_page: false,
            enabled_remote_debugging,
            used_bounded_pixel_fallback,
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
        let pid = i32::try_from(request.pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser pid is outside the macOS process-id range",
            )
        })?;
        let window_id = u32::try_from(request.window_id).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser window is outside the macOS window-id range",
            )
        })?;
        tokio::task::spawn_blocking(move || super::setup_ui::commit_pending(pid, window_id))
            .await
            .map_err(|error| {
                refusal(
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
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!(
                    "existing-profile cleanup is not implemented for {:?}",
                    request.browser
                ),
            )
        })?;
        let pid = i32::try_from(request.pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser pid is outside the macOS process-id range",
            )
        })?;
        let window_id = u32::try_from(request.window_id).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser window is outside the macOS window-id range",
            )
        })?;
        run_committed_consent_cleanup(
            CONSENT_CLEANUP_PASSES,
            || super::consent_ui::dismiss(pid, window_id),
            || super::setup_ui::disable(pid, window_id, descriptor),
            || visible_consent_surfaces(pid),
            || std::thread::sleep(Duration::from_millis(100)),
        )
    }

    async fn abort_existing_profile_setup(
        &self,
        request: ExistingProfileSetupRequest,
        error: BrowserRefusal,
    ) -> BrowserRefusal {
        let Ok(pid) = i32::try_from(request.pid) else {
            return error;
        };
        let Ok(window_id) = u32::try_from(request.window_id) else {
            return error;
        };
        tokio::task::spawn_blocking(move || {
            run_failed_prepare_consent_cleanup(
                error,
                CONSENT_CLEANUP_PASSES,
                || super::consent_ui::dismiss(pid, window_id),
                |error| super::setup_ui::abort_pending(pid, window_id, error),
                || visible_consent_surfaces(pid),
                || std::thread::sleep(Duration::from_millis(100)),
            )
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
        super::consent_ui::handle(request).await
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

    fn consent_surface(window_id: u32) -> ConsentSurfaceEvidence {
        ConsentSurfaceEvidence {
            window_id,
            pid: 42,
            bounds: [120.0, 120.0, 520.0, 260.0],
        }
    }

    #[test]
    fn consent_cleanup_evidence_requires_exact_pid_title_visibility_and_bounds() {
        let exact = window(90, 42, "Allow remote debugging?");
        let mut foreign = window(91, 7, "Allow remote debugging?");
        foreign.app_name = "Google Chrome".to_owned();
        let mut hidden = window(92, 42, "Allow remote debugging?");
        hidden.is_on_screen = false;
        let mut empty = window(93, 42, "Allow remote debugging?");
        empty.bounds.width = 0.0;

        let surfaces = consent_surface_evidence(
            [exact, foreign, hidden, empty, window(94, 42, "Unrelated")],
            42,
        );
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].window_id, 90);
        assert_eq!(surfaces[0].pid, 42);
    }

    #[test]
    fn failed_prepare_cleanup_drains_stacked_and_late_consent_surfaces() {
        let events = std::cell::RefCell::new(Vec::new());
        let mut observations = vec![
            vec![consent_surface(90), consent_surface(91)],
            vec![consent_surface(92)],
            Vec::new(),
            Vec::new(),
        ]
        .into_iter();
        let error = run_failed_prepare_consent_cleanup(
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "prepare failed",
            ),
            4,
            || {
                events.borrow_mut().push("dismiss");
                Ok(false)
            },
            |error| {
                events.borrow_mut().push("rollback");
                error
            },
            || observations.next().unwrap_or_default(),
            || {},
        );

        assert_eq!(
            *events.borrow(),
            ["dismiss", "rollback", "dismiss", "dismiss", "dismiss", "dismiss"]
        );
        assert_eq!(
            error.detail.as_ref().unwrap()["consent_cleanup"]["status"],
            "dismissed"
        );
        assert_eq!(
            error.detail.as_ref().unwrap()["consent_cleanup"]["retained_surfaces"],
            serde_json::json!([])
        );
    }

    #[test]
    fn failed_prepare_cleanup_returns_bounded_evidence_for_retained_surfaces() {
        let mut rollback_calls = 0;
        let error = run_failed_prepare_consent_cleanup(
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "prepare failed",
            ),
            3,
            || {
                Err(refusal(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "ambiguous cancel actions",
                ))
            },
            |error| {
                rollback_calls += 1;
                error
            },
            || vec![consent_surface(90), consent_surface(91)],
            || {},
        );

        assert_eq!(
            rollback_calls, 1,
            "ambiguity must not skip exact setup rollback"
        );
        assert_eq!(
            error.detail.as_ref().unwrap()["consent_cleanup"]["status"],
            "retained"
        );
        assert_eq!(
            error.detail.as_ref().unwrap()["consent_cleanup"]["retained_surfaces"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            error.detail.as_ref().unwrap()["consent_cleanup"]["dismiss_error"]["message"],
            "ambiguous cancel actions"
        );
    }

    #[test]
    fn committed_cleanup_refuses_while_an_unresolved_consent_surface_is_retained() {
        let result = run_committed_consent_cleanup(
            3,
            || Ok(false),
            || Ok(true),
            || vec![consent_surface(90)],
            || {},
        );

        let error = result.expect_err("retained consent UI must retain the core cleanup request");
        assert_eq!(error.code, BrowserRefusalCode::BrowserWrongTargetRefused);
        assert_eq!(error.detail.as_ref().unwrap()["status"], "retained");
        assert_eq!(
            error.detail.as_ref().unwrap()["retained_surfaces"][0]["window_id"],
            90
        );
    }

    #[test]
    fn committed_cleanup_observes_the_full_window_for_late_surfaces() {
        let mut observations = vec![
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![consent_surface(90)],
            Vec::new(),
            Vec::new(),
        ]
        .into_iter();
        let result = run_committed_consent_cleanup(
            6,
            || Ok(false),
            || Ok(true),
            || observations.next().unwrap_or_default(),
            || {},
        );

        assert_eq!(result.unwrap(), true);
    }

    #[test]
    fn isolated_browser_candidates_are_vendor_attested_system_installs() {
        let candidates = isolated_browser_candidates();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].1, "com.google.Chrome");
        assert_eq!(candidates[0].2, "EQHXZ8M8AV");
        assert!(candidates[0]
            .0
            .ends_with("Google Chrome.app/Contents/MacOS/Google Chrome"));
        assert_eq!(candidates[1].1, "com.microsoft.edgemac");
        assert_eq!(candidates[1].2, "UBF8T346G9");
        assert!(candidates[1]
            .0
            .ends_with("Microsoft Edge.app/Contents/MacOS/Microsoft Edge"));
        assert!(candidates
            .iter()
            .all(|(candidate, _, _)| candidate.is_absolute()));
    }

    #[test]
    fn codesign_identity_requires_exact_identifier_and_team() {
        let details = "Identifier=com.google.Chrome\nTeamIdentifier=EQHXZ8M8AV\n";
        assert!(codesign_identity_matches(
            details,
            "com.google.Chrome",
            "EQHXZ8M8AV"
        ));
        assert!(!codesign_identity_matches(
            details,
            "com.google.Chrome",
            "ATTACKER00"
        ));
        assert!(!codesign_identity_matches(
            "Identifier=com.google.Chrome\nTeamIdentifier=\n",
            "com.google.Chrome",
            "EQHXZ8M8AV"
        ));
    }

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
            current_space_id: None,
            on_current_space: Some(true),
            space_ids: None,
        }
    }

    #[tokio::test]
    async fn browser_visual_feedback_updates_the_declared_session_cursor() {
        let registry = Arc::new(crate::cursor::CursorRegistry::new());
        let platform = MacOsBrowserPlatform::new(registry.clone());
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
        let position = state.position.expect("browser cursor position");
        assert_eq!((position.x, position.y), (321.0, 456.0));
    }

    #[tokio::test]
    async fn inactive_tab_feedback_materializes_but_does_not_move_its_cursor() {
        let registry = Arc::new(crate::cursor::CursorRegistry::new());
        let platform = MacOsBrowserPlatform::new(registry.clone());
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
            state.position.is_none(),
            "an inactive tab must not animate or move its visible cursor"
        );
    }

    #[test]
    fn browser_cursor_tracker_shows_only_the_active_tabs_session_per_window() {
        let mut tracker = BrowserCursorTracker::default();
        assert_eq!(
            tracker.update("session-red", 77, "tab-A", false),
            vec![("session-red".to_owned(), false)]
        );

        let first_active = tracker.update("session-red", 77, "tab-A", true);
        assert_eq!(first_active, vec![("session-red".to_owned(), true)]);

        let second_active = tracker
            .update("session-blue", 77, "tab-B", true)
            .into_iter()
            .collect::<HashMap<_, _>>();
        assert_eq!(second_active.get("session-red"), Some(&false));
        assert_eq!(second_active.get("session-blue"), Some(&true));

        let other_window = tracker.update("session-green", 88, "tab-C", true);
        assert_eq!(
            other_window,
            vec![("session-green".to_owned(), true)],
            "an active tab in another native window must not hide this window"
        );
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
    fn active_port_parser_requires_one_exact_browser_path() {
        assert_eq!(
            parse_devtools_active_port(
                "9222\n/devtools/browser/f1d991b4-2694-4b28-b63a-1f2a8da3a435\n"
            ),
            Some((
                9222,
                "/devtools/browser/f1d991b4-2694-4b28-b63a-1f2a8da3a435"
            ))
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/page/id\n"),
            None
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/id\nextra\n"),
            None
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/../page\n"),
            None
        );
    }

    #[test]
    fn user_data_dir_parser_preserves_exact_custom_argv_path() {
        let custom = std::env::temp_dir().join("cua browser profile with spaces");
        let arg = format!("--user-data-dir={}", custom.display()).into_bytes();
        assert_eq!(
            user_data_dir_from_arguments(&[b"/Applications/Chrome".to_vec(), arg], None)
                .expect("one absolute custom profile"),
            Some(custom)
        );
    }

    #[test]
    fn user_data_dir_parser_does_not_treat_a_path_as_authority() {
        let default = PathBuf::from("/tmp/default-browser-data");
        assert_eq!(
            user_data_dir_from_arguments(
                &[b"/Applications/Chrome".to_vec()],
                Some(default.clone())
            )
            .expect("default path"),
            Some(default)
        );
        assert!(user_data_dir_from_arguments(
            &[
                b"/Applications/Chrome".to_vec(),
                b"--user-data-dir=/tmp/one".to_vec(),
                b"--user-data-dir=/tmp/two".to_vec(),
            ],
            None,
        )
        .is_err());
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
