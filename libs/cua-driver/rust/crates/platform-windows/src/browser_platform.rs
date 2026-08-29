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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, FILETIME, HWND, RECT};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_APPEND_DATA, FILE_DELETE_CHILD,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, OPEN_EXISTING,
    WRITE_DAC, WRITE_OWNER,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{
    FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
};
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
    let executable = executable.trim_end_matches(".exe");
    match executable {
        "chrome" => BrowserProduct::GoogleChrome,
        "chromium" => BrowserProduct::Chromium,
        "msedge" => BrowserProduct::MicrosoftEdge,
        "brave" => BrowserProduct::Brave,
        "vivaldi" => BrowserProduct::Vivaldi,
        "opera" => BrowserProduct::Opera,
        "arc" => BrowserProduct::Arc,
        "electron" => BrowserProduct::Electron,
        "firefox" => BrowserProduct::Firefox,
        _ if executable
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|token| token == "electron") =>
        {
            BrowserProduct::Electron
        }
        _ => BrowserProduct::Other,
    }
}

fn isolated_browser_candidates_from_roots(
    program_files: &std::path::Path,
    program_files_x86: &std::path::Path,
) -> Vec<(PathBuf, PathBuf, &'static str, &'static str)> {
    vec![
        (
            program_files.join(r"Google\Chrome\Application\chrome.exe"),
            program_files.to_owned(),
            "CN=Google LLC",
            "O=Google LLC",
        ),
        (
            program_files_x86.join(r"Google\Chrome\Application\chrome.exe"),
            program_files_x86.to_owned(),
            "CN=Google LLC",
            "O=Google LLC",
        ),
        (
            program_files.join(r"Microsoft\Edge\Application\msedge.exe"),
            program_files.to_owned(),
            "CN=Microsoft Corporation",
            "O=Microsoft Corporation",
        ),
        (
            program_files_x86.join(r"Microsoft\Edge\Application\msedge.exe"),
            program_files_x86.to_owned(),
            "CN=Microsoft Corporation",
            "O=Microsoft Corporation",
        ),
    ]
}

fn known_folder_path(id: &windows::core::GUID) -> Result<PathBuf, BrowserRefusal> {
    let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("could not resolve trusted Windows installation root: {error}"),
        )
    })?;
    let decoded = unsafe { raw.to_string() };
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    decoded.map(PathBuf::from).map_err(|error| {
        refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("trusted Windows installation root was not valid Unicode: {error}"),
        )
    })
}

fn isolated_browser_candidates(
) -> Result<Vec<(PathBuf, PathBuf, &'static str, &'static str)>, BrowserRefusal> {
    let program_files = known_folder_path(&FOLDERID_ProgramFiles)?;
    let program_files_x86 = known_folder_path(&FOLDERID_ProgramFilesX86)?;
    Ok(isolated_browser_candidates_from_roots(
        &program_files,
        &program_files_x86,
    ))
}

fn authenticode_identity_matches(details: &str, expected_cn: &str, expected_org: &str) -> bool {
    let mut lines = details.lines();
    if lines.next().map(str::trim) != Some("Valid") {
        return false;
    }
    let Some(subject) = lines.next() else {
        return false;
    };
    let fields = subject.split(',').map(str::trim).collect::<Vec<_>>();
    fields.contains(&expected_cn) && fields.contains(&expected_org)
}

fn has_trusted_authenticode_identity(
    executable: &std::path::Path,
    expected_cn: &str,
    expected_org: &str,
) -> bool {
    let Ok(output) = authenticode_output(executable) else {
        return false;
    };
    output.status.success()
        && authenticode_identity_matches(
            &String::from_utf8_lossy(&output.stdout),
            expected_cn,
            expected_org,
        )
}

fn authenticode_output(executable: &std::path::Path) -> std::io::Result<std::process::Output> {
    let Ok(system32) = system_directory_path() else {
        return Err(std::io::Error::other(
            "could not resolve the Windows system directory",
        ));
    };
    let powershell = system32.join(r"WindowsPowerShell\v1.0\powershell.exe");
    std::process::Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"$ErrorActionPreference = 'Stop'; $utf8 = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = $utf8; $module = Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1'; Import-Module -Name $module -Force -ErrorAction Stop; $path = [Environment]::GetEnvironmentVariable('CUA_BROWSER_ATTEST_PATH'); $signature = Microsoft.PowerShell.Security\Get-AuthenticodeSignature -LiteralPath $path; Write-Output $signature.Status; Write-Output $signature.SignerCertificate.Subject"#,
        ])
        // The static command imports Security beside the trusted System32
        // PowerShell, ignoring an incompatible inherited PSModulePath. Pass the
        // browser path as data so PowerShell never parses it as command text.
        .env("CUA_BROWSER_ATTEST_PATH", executable)
        .stdin(Stdio::null())
        .output()
}

fn current_token_write_denial_reason(path: &std::path::Path, directory: bool) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;

    // This launch boundary protects the current agent token from executing a
    // browser tree that it can modify. Other principals that can replace an
    // installed browser are outside this runtime authorization boundary.
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let rights: &[(&str, u32)] = if directory {
        &[
            ("add_file", FILE_ADD_FILE.0),
            ("add_subdirectory", FILE_ADD_SUBDIRECTORY.0),
            ("delete_child", FILE_DELETE_CHILD.0),
            ("write_ea", FILE_WRITE_EA.0),
            ("write_attributes", FILE_WRITE_ATTRIBUTES.0),
            ("delete", DELETE.0),
            ("write_dac", WRITE_DAC.0),
            ("write_owner", WRITE_OWNER.0),
        ]
    } else {
        &[
            ("write_data", FILE_WRITE_DATA.0),
            ("append_data", FILE_APPEND_DATA.0),
            ("write_ea", FILE_WRITE_EA.0),
            ("write_attributes", FILE_WRITE_ATTRIBUTES.0),
            ("delete", DELETE.0),
            ("write_dac", WRITE_DAC.0),
            ("write_owner", WRITE_OWNER.0),
        ]
    };
    let flags = if directory {
        FILE_FLAG_BACKUP_SEMANTICS
    } else {
        FILE_FLAGS_AND_ATTRIBUTES(0)
    };
    for &(name, right) in rights {
        match unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                right,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                flags,
                None,
            )
        } {
            Ok(handle) => {
                let _ = unsafe { CloseHandle(handle) };
                return Some(format!("current token was granted {name}"));
            }
            Err(error) if error.code() == E_ACCESSDENIED => {}
            Err(error) => return Some(format!("{name} probe failed closed: {error}")),
        }
    }
    None
}

fn current_token_cannot_write(path: &std::path::Path, directory: bool) -> bool {
    current_token_write_denial_reason(path, directory).is_none()
}

fn trusted_windows_installation(
    executable: &std::path::Path,
    trusted_root: &std::path::Path,
) -> bool {
    trusted_windows_installation_with_probe(executable, trusted_root, current_token_cannot_write)
}

fn trusted_windows_installation_with_probe(
    executable: &std::path::Path,
    trusted_root: &std::path::Path,
    mut cannot_write: impl FnMut(&std::path::Path, bool) -> bool,
) -> bool {
    if !executable.starts_with(trusted_root) || !cannot_write(executable, false) {
        return false;
    }
    let mut current = executable.parent();
    while let Some(directory) = current {
        if !cannot_write(directory, true) {
            return false;
        }
        if directory == trusted_root {
            return true;
        }
        current = directory.parent();
    }
    false
}

fn default_user_data_dir(product: BrowserProduct) -> Option<PathBuf> {
    let root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let relative = match product {
        BrowserProduct::GoogleChrome => ["Google", "Chrome", "User Data"].as_slice(),
        BrowserProduct::MicrosoftEdge => ["Microsoft", "Edge", "User Data"].as_slice(),
        BrowserProduct::Chromium => ["Chromium", "User Data", ""].as_slice(),
        BrowserProduct::Brave => ["BraveSoftware", "Brave-Browser", "User Data"].as_slice(),
        BrowserProduct::Vivaldi => ["Vivaldi", "User Data", ""].as_slice(),
        _ => return None,
    };
    Some(
        relative
            .iter()
            .filter(|part| !part.is_empty())
            .fold(root, |path, part| path.join(part)),
    )
}

fn parse_windows_command_line(command_line: &str) -> Vec<String> {
    let chars = command_line.chars().collect::<Vec<_>>();
    let mut args = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if index == chars.len() {
            break;
        }
        let mut arg = String::new();
        let mut quoted = false;
        while index < chars.len() {
            if !quoted && chars[index].is_whitespace() {
                break;
            }
            let mut slashes = 0;
            while index < chars.len() && chars[index] == '\\' {
                slashes += 1;
                index += 1;
            }
            if index < chars.len() && chars[index] == '"' {
                arg.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 0 {
                    quoted = !quoted;
                } else {
                    arg.push('"');
                }
                index += 1;
            } else {
                arg.extend(std::iter::repeat_n('\\', slashes));
                if index < chars.len() && (quoted || !chars[index].is_whitespace()) {
                    arg.push(chars[index]);
                    index += 1;
                }
            }
        }
        args.push(arg);
    }
    args
}

fn is_windows_absolute_path(path: &std::path::Path) -> bool {
    let value = path.as_os_str().to_string_lossy();
    let bytes = value.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || value.starts_with(r"\\")
}

fn user_data_dir_from_command_line(
    command_line: &str,
    default: Option<PathBuf>,
) -> Result<Option<PathBuf>, BrowserRefusal> {
    let args = parse_windows_command_line(command_line);
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
        [] => Ok(default),
        [path] if is_windows_absolute_path(path) => Ok(Some(path.clone())),
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

fn system_powershell_path() -> Result<PathBuf, BrowserRefusal> {
    let system = system_netstat_path()?
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "could not resolve the trusted Windows system directory",
            )
        })?;
    Ok(system.join("WindowsPowerShell\\v1.0\\powershell.exe"))
}

async fn browser_command_line(pid: u32) -> Result<String, BrowserRefusal> {
    let powershell = system_powershell_path()?;
    let script = format!(
        "$OutputEncoding=[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); (Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}' -ErrorAction Stop).CommandLine"
    );
    let output = tokio::process::Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|error| {
            refusal(
                BrowserRefusalCode::BrowserRouteUnavailable,
                format!("could not inspect browser arguments: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} arguments are unavailable"),
        ));
    }
    let command_line = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if command_line.is_empty() {
        return Err(refusal(
            BrowserRefusalCode::BrowserBindingStale,
            format!("browser process {pid} has no command line"),
        ));
    }
    Ok(command_line)
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

async fn active_port_endpoint(
    pid: u32,
    product: BrowserProduct,
) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
    let default = default_user_data_dir(product);
    let user_data_dir = match browser_command_line(pid).await {
        Ok(command_line) => user_data_dir_from_command_line(&command_line, default.clone())?,
        Err(_) => default,
    };
    let Some(user_data_dir) = user_data_dir else {
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
    if !loopback_ports_for_exact_pid(pid).await?.contains(&port) {
        return Ok(None);
    }
    Ok(Some(OwnedEndpoint {
        ws_url: format!("ws://127.0.0.1:{port}{path}"),
        http_port: Some(port),
        transport: EndpointTransport::DevToolsActivePort,
        ownership: EndpointOwnershipProof {
            method: EndpointOwnershipMethod::DevtoolsActivePortsFile,
            owner_pid: i64::from(pid),
            listener_pid: Some(i64::from(pid)),
            detail: Some(
                "exact OS-reported argv profile port file plus loopback listener owner".to_owned(),
            ),
        },
    }))
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
    .map(|_| String::from_utf16_lossy(&path_buf[..path_len as usize]))
    .map(canonical_process_executable);
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

fn canonical_process_executable(path: String) -> String {
    // Manifest executable grants use `canonicalize` too. Normalize the Windows
    // process evidence at collection time so shared authorization stays exact.
    std::fs::canonicalize(&path)
        .map(|canonical| canonical.to_string_lossy().into_owned())
        .unwrap_or(path)
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

fn system_directory_path() -> Result<PathBuf, BrowserRefusal> {
    let mut buffer = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "could not resolve the trusted Windows system directory",
        ));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}

fn system_netstat_path() -> Result<PathBuf, BrowserRefusal> {
    Ok(system_directory_path()?.join("netstat.exe"))
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

fn select_provisional_setup_port(
    ports: &[u16],
    listeners_before: &[u16],
    setup_was_already_enabled: bool,
) -> Result<Option<u16>, BrowserRefusal> {
    let correlated = ports
        .iter()
        .copied()
        .filter(|port| !listeners_before.contains(port))
        .collect::<Vec<_>>();
    match correlated.as_slice() {
        [port] => Ok(Some(*port)),
        [] if setup_was_already_enabled => match ports {
            [] => Ok(None),
            [port] => Ok(Some(*port)),
            _ => Err(refusal(
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "the pre-enabled browser setup exposed multiple existing exact-pid loopback listeners",
            )),
        },
        [] => Ok(None),
        _ => Err(refusal(
            BrowserRefusalCode::BrowserBindingAmbiguous,
            "the approved setup action exposed multiple newly correlated exact-pid listeners",
        )),
    }
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
    transport: EndpointTransport,
) -> OwnedEndpoint {
    OwnedEndpoint {
        ws_url,
        http_port: Some(port),
        transport,
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
    transport: EndpointTransport,
) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
    match discovered.as_slice() {
        [] => Ok(None),
        [(port, ws_url, listener_pid)] => Ok(Some(owned_endpoint_from_listener(
            root_pid,
            *port,
            ws_url.clone(),
            *listener_pid,
            context,
            transport,
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

fn legacy_setup_endpoint_is_stable(
    observation: &mut Option<(String, std::time::Instant)>,
    ws_url: &str,
    now: std::time::Instant,
    preference_window: Duration,
) -> bool {
    if let Some((observed_url, observed_at)) = observation.as_ref() {
        if observed_url == ws_url {
            return now.saturating_duration_since(*observed_at) >= preference_window;
        }
    }
    *observation = Some((ws_url.to_owned(), now));
    false
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
    fn isolated_browser_executable(&self) -> Result<String, BrowserRefusal> {
        for (candidate, trusted_root, expected_cn, expected_org) in isolated_browser_candidates()? {
            let Ok(executable) = select_isolated_browser_executable([candidate.clone()]) else {
                continue;
            };
            if trusted_windows_installation(&candidate, &trusted_root)
                && has_trusted_authenticode_identity(
                    std::path::Path::new(&executable),
                    expected_cn,
                    expected_org,
                )
            {
                return Ok(executable);
            }
        }
        Err(refusal(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "no vendor-signed protected Chromium executable is available for isolated launch",
        ))
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
        let command_line = if chromium {
            browser_command_line(pid_u32).await.ok()
        } else {
            None
        };
        let helper = command_line.as_deref().is_some_and(|line| {
            parse_windows_command_line(line)
                .iter()
                .any(|arg| arg.starts_with("--type=") && arg.len() > "--type=".len())
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
            channel: if process_role == BrowserProcessRole::StandaloneConsumer {
                command_line.as_deref().map(|line| {
                    let lower = line.to_ascii_lowercase();
                    if lower.contains("chrome sxs") || lower.contains("canary") {
                        "canary".to_owned()
                    } else if lower.contains(" beta") || lower.contains("\\beta\\") {
                        "beta".to_owned()
                    } else if lower.contains(" dev") || lower.contains("\\dev\\") {
                        "dev".to_owned()
                    } else {
                        "stable".to_owned()
                    }
                })
            } else {
                None
            },
            process_role,
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
            EndpointTransport::LegacyJsonVersion,
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
            EndpointTransport::SpawnedExact,
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
        let classification = self.classify_browser(pid).await?;
        if let Some(endpoint) = active_port_endpoint(pid_u32, classification.product_kind).await? {
            return Ok(Some(endpoint));
        }
        select_unique_owned_endpoint(
            pid,
            exact_browser_endpoints_for_pid(pid_u32).await?,
            "Windows exact browser-pid listener plus /json/version",
            EndpointTransport::LegacyJsonVersion,
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
        let classification = self.classify_browser(pid).await?;
        if let Some(endpoint) = active_port_endpoint(pid_u32, classification.product_kind).await? {
            if endpoint.ws_url != expected_ws_url {
                return Err(refusal(
                    BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                    "the browser's exact DevTools endpoint changed after approval",
                ));
            }
            return Ok(Some(endpoint));
        }
        // Setup may approve the exact PID-owned port before Chromium publishes
        // its final browser WebSocket id. Reprove that stable port ownership
        // here; the connection layer still uses the approved WebSocket path.
        if !loopback_port_is_owned_with_retry(pid_u32, port).await? {
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
        let setup_was_already_enabled = !enabled_remote_debugging;
        let focused_setup_address_field = handle.focused_setup_address_field;
        let foregrounded_window = handle.foregrounded_window;
        let injected_global_input = handle.injected_global_input;

        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        // Chrome and Edge can expose /json/version a few milliseconds before
        // their profile-scoped DevToolsActivePort file becomes visible on
        // Windows. Prefer the stronger file + exact-pid listener proof during
        // that bounded publication window. Accepting the HTTP result
        // immediately can mint a grant for a transient browser WebSocket id
        // that the just-published file then (correctly) disproves.
        let mut legacy_endpoint_observation: Option<(String, std::time::Instant)> = None;
        let endpoint_result = loop {
            match active_port_endpoint(pid_u32, request.browser).await {
                Ok(Some(endpoint)) => break Ok(endpoint),
                Ok(None) => {}
                Err(error) => break Err(error),
            }
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
                match select_provisional_setup_port(
                    &ports,
                    &listeners_before,
                    setup_was_already_enabled,
                ) {
                    Ok(Some(port)) => endpoints.push((
                        port,
                        format!("ws://127.0.0.1:{port}/devtools/browser"),
                        if listeners_before.contains(&port) {
                            "unique existing exact browser-pid listener bound to a pre-enabled exact setup page"
                        } else {
                            "new exact browser-pid listener correlated with approved setup"
                        },
                    )),
                    Ok(None) => {}
                    Err(error) => break Err(error),
                }
            }
            match endpoints.as_slice() {
                [(port, ws_url, detail)] => {
                    let now = std::time::Instant::now();
                    if !legacy_setup_endpoint_is_stable(
                        &mut legacy_endpoint_observation,
                        ws_url,
                        now,
                        Duration::from_secs(1),
                    ) {
                        if now >= deadline {
                            break Err(refusal(
                                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                                format!(
                                    "{} did not keep one exact browser endpoint stable after the approved setup action",
                                    descriptor.product_name
                                ),
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
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
        let pid = u32::try_from(request.pid).map_err(|_| {
            refusal(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the approved browser pid is outside the Windows process-id range",
            )
        })?;
        let dismissed_before = crate::browser_consent_ui::dismiss(pid, request.window_id)?;
        let closed_setup_page = crate::browser_setup_ui::disable(request.window_id, descriptor)?;
        let dismissed_after = crate::browser_consent_ui::dismiss(pid, request.window_id)?;
        Ok(dismissed_before || closed_setup_page || dismissed_after)
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
    fn process_fingerprint_uses_manifest_canonical_executable_path() {
        let (_started, executable) =
            process_identity(std::process::id()).expect("current process fingerprint");
        let expected = std::fs::canonicalize(std::env::current_exe().expect("current executable"))
            .expect("canonical current executable")
            .to_string_lossy()
            .into_owned();

        assert_eq!(executable.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn isolated_browser_candidates_are_vendor_attested_protected_installs() {
        let candidates = isolated_browser_candidates_from_roots(
            std::path::Path::new(r"D:\Apps"),
            std::path::Path::new(r"E:\Apps32"),
        );
        assert_eq!(candidates.len(), 4);
        assert_eq!(
            candidates[0].0,
            PathBuf::from(r"D:\Apps\Google\Chrome\Application\chrome.exe")
        );
        assert_eq!(
            candidates[1].0,
            PathBuf::from(r"E:\Apps32\Google\Chrome\Application\chrome.exe")
        );
        assert_eq!(
            candidates[2].0,
            PathBuf::from(r"D:\Apps\Microsoft\Edge\Application\msedge.exe")
        );
    }

    #[test]
    fn authenticode_identity_requires_valid_exact_publisher_fields() {
        let details = "Valid\r\nCN=Google LLC, O=Google LLC, L=Mountain View\r\n";
        assert!(authenticode_identity_matches(
            details,
            "CN=Google LLC",
            "O=Google LLC"
        ));
        assert!(!authenticode_identity_matches(
            "NotSigned\r\nCN=Google LLC, O=Google LLC\r\n",
            "CN=Google LLC",
            "O=Google LLC"
        ));
        assert!(!authenticode_identity_matches(
            "Valid\r\nCN=Google LLC, O=Google LLC Evil\r\n",
            "CN=Google LLC",
            "O=Google LLC"
        ));
    }

    #[test]
    fn writable_installation_tree_is_rejected() {
        let root = tempfile::tempdir().expect("writable product root");
        let product = root.path().join(r"Google\Chrome\Application");
        std::fs::create_dir_all(&product).expect("writable product directories");
        let executable = product.join("chrome.exe");
        std::fs::write(&executable, b"not a browser").expect("writable executable");

        assert!(!trusted_windows_installation(&executable, root.path()));
    }

    #[test]
    fn installation_trust_walk_requires_every_probe_to_deny_write() {
        let root = std::path::Path::new(r"C:\Program Files");
        let executable = root.join(r"Google\Chrome\Application\chrome.exe");

        assert!(trusted_windows_installation_with_probe(
            &executable,
            root,
            |_, _| true,
        ));
        assert!(!trusted_windows_installation_with_probe(
            &executable,
            root,
            |path, directory| !(directory && path.ends_with(r"Google\Chrome")),
        ));
        assert!(!trusted_windows_installation_with_probe(
            &executable,
            root,
            |path, directory| !(directory && path == root),
        ));
        assert!(!trusted_windows_installation_with_probe(
            &std::path::PathBuf::from(r"D:\UserControlled\chrome.exe"),
            root,
            |_, _| true,
        ));
    }

    #[test]
    fn installed_vendor_browser_tree_is_accepted_only_for_a_nonwritable_token() {
        let candidates = isolated_browser_candidates().expect("trusted Known Folder roots");
        let installed = candidates
            .iter()
            .find(|(candidate, _, expected_cn, expected_org)| {
                candidate.is_file()
                    && has_trusted_authenticode_identity(candidate, expected_cn, expected_org)
            });
        let Some(installed) = installed else {
            let diagnostics = candidates
                .iter()
                .map(|(candidate, _, _, _)| {
                    let name = candidate
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("unknown.exe");
                    if !candidate.is_file() {
                        return format!("{name}: missing");
                    }
                    match authenticode_output(candidate) {
                        Ok(output) => format!(
                            "{name}: exit={:?}, stdout_utf8={:?}, stdout_bytes={:?}, stderr_utf8={:?}",
                            output.status.code(),
                            String::from_utf8_lossy(&output.stdout),
                            output.stdout,
                            String::from_utf8_lossy(&output.stderr),
                        ),
                        Err(error) => format!("{name}: invocation_error={error}"),
                    }
                })
                .collect::<Vec<_>>();
            panic!(
                "Windows CI image must provide signed Chrome or Edge; diagnostics: {diagnostics:?}"
            );
        };

        if !trusted_windows_installation(&installed.0, &installed.1) {
            let mut diagnostics = vec![(
                "executable".to_string(),
                current_token_write_denial_reason(&installed.0, false),
            )];
            let mut current = installed.0.parent();
            let mut index = 0;
            while let Some(directory) = current {
                diagnostics.push((
                    format!("ancestor_{index}"),
                    current_token_write_denial_reason(directory, true),
                ));
                if directory == installed.1 {
                    break;
                }
                current = directory.parent();
                index += 1;
            }
            for (location, reason) in &diagnostics {
                if let Some(reason) = reason {
                    assert!(
                        reason.starts_with("current token was granted "),
                        "{location} refusal must prove write-capable token posture, not an unexpected probe failure: {reason}"
                    );
                }
            }
            assert!(
                diagnostics.iter().any(|(_, reason)| reason.is_some()),
                "a refused installation must identify an explicitly granted write right: {diagnostics:?}"
            );
            assert!(
                std::env::var_os("CUA_TEST_REQUIRE_PROTECTED_WINDOWS_BROWSER").is_none(),
                "the standalone-browser runner must provide a vendor-signed browser tree protected from its current token: {diagnostics:?}"
            );
            eprintln!(
                "installed browser correctly refused for this write-capable runner token: {diagnostics:?}"
            );
            return;
        }
    }
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
            EndpointTransport::EmbeddedDescendant,
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
        assert!(select_unique_owned_endpoint(
            42,
            Vec::new(),
            "verified process tree",
            EndpointTransport::LegacyJsonVersion,
        )
        .expect("empty discovery is not an error")
        .is_none());

        let selected = select_unique_owned_endpoint(
            42,
            vec![(
                9222,
                "ws://127.0.0.1:9222/devtools/browser/edge".to_owned(),
                43,
            )],
            "verified process tree",
            EndpointTransport::LegacyJsonVersion,
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
            EndpointTransport::LegacyJsonVersion,
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
        assert_eq!(
            browser_product("CuaTestHarness.Electron.exe"),
            BrowserProduct::Electron
        );
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

    #[test]
    fn pre_enabled_setup_reuses_one_exact_pid_port_before_consent() {
        assert_eq!(
            select_provisional_setup_port(&[9222], &[9222], true).unwrap(),
            Some(9222)
        );
        assert_eq!(
            select_provisional_setup_port(&[9222], &[9222], false).unwrap(),
            None
        );
    }

    #[test]
    fn windows_command_line_parser_preserves_quoted_profile_paths() {
        let args = parse_windows_command_line(
            r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --flag "--user-data-dir=C:\Profiles\Personal Browser""#,
        );
        assert_eq!(
            args,
            vec![
                r#"C:\Program Files\Google\Chrome\Application\chrome.exe"#,
                "--flag",
                r#"--user-data-dir=C:\Profiles\Personal Browser"#,
            ]
        );
        assert_eq!(
            user_data_dir_from_command_line(
                r#"chrome.exe "--user-data-dir=C:\Profiles\Personal Browser""#,
                None,
            )
            .expect("one absolute custom profile"),
            Some(PathBuf::from(r#"C:\Profiles\Personal Browser"#))
        );
    }

    #[test]
    fn active_port_parser_rejects_non_browser_and_ambiguous_paths() {
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/exact-id\n"),
            Some((9222, "/devtools/browser/exact-id"))
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/page/id\n"),
            None
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/id\nextra\n"),
            None
        );
    }

    #[test]
    fn pre_enabled_setup_refuses_multiple_existing_exact_pid_ports() {
        assert_eq!(
            select_provisional_setup_port(&[9222, 9333], &[9222, 9333], true)
                .unwrap_err()
                .code,
            BrowserRefusalCode::BrowserBindingAmbiguous
        );
    }

    #[test]
    fn legacy_setup_endpoint_must_remain_exact_during_active_port_preference_window() {
        let start = std::time::Instant::now();
        let window = Duration::from_secs(1);
        let mut observation = None;

        assert!(!legacy_setup_endpoint_is_stable(
            &mut observation,
            "ws://127.0.0.1:9222/devtools/browser/first",
            start,
            window,
        ));
        assert!(!legacy_setup_endpoint_is_stable(
            &mut observation,
            "ws://127.0.0.1:9222/devtools/browser/first",
            start + Duration::from_millis(999),
            window,
        ));
        assert!(legacy_setup_endpoint_is_stable(
            &mut observation,
            "ws://127.0.0.1:9222/devtools/browser/first",
            start + window,
            window,
        ));
        assert!(!legacy_setup_endpoint_is_stable(
            &mut observation,
            "ws://127.0.0.1:9222/devtools/browser/second",
            start + window,
            window,
        ));
        assert!(legacy_setup_endpoint_is_stable(
            &mut observation,
            "ws://127.0.0.1:9222/devtools/browser/second",
            start + window + window,
            window,
        ));
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
