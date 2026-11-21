// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Shared Cua Driver daemon wire protocol and synchronous client.
//!
//! The daemon, CLI, MCP proxy, and imported SDK all use these exact request,
//! response, probing, timeout, and framing rules. Keeping them outside the
//! binary crate prevents language bindings from growing an independent
//! implementation of the native transport.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Versioned identity returned by the daemon before an imported SDK or an MCP
/// proxy is allowed to treat an endpoint as ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonMetadata {
    pub driver_version: String,
    pub contract_version: String,
    pub tools_list_schema_version: String,
    pub capability_version: String,
    pub mcp_protocol_version: String,
    pub pid: u32,
    pub embedded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_bundle_id: Option<String>,
    /// The daemon answers, but refuses every tool call with
    /// `permissions_pending` until the macOS grants arrive. Older daemons
    /// omit it.
    #[serde(default)]
    pub permissions_pending: bool,
}

pub fn current_daemon_metadata() -> DaemonMetadata {
    DaemonMetadata {
        driver_version: env!("CARGO_PKG_VERSION").to_owned(),
        contract_version: cua_driver_contract::CONTRACT_VERSION.to_owned(),
        tools_list_schema_version: cua_driver_contract::TOOLS_LIST_SCHEMA_VERSION.to_owned(),
        capability_version: cua_driver_contract::CAPABILITY_VERSION.to_owned(),
        mcp_protocol_version: cua_driver_contract::MCP_PROTOCOL_VERSION.to_owned(),
        pid: std::process::id(),
        embedded: crate::embedded_mode(),
        host_bundle_id: std::env::var(crate::HOST_BUNDLE_ID_ENV)
            .ok()
            .filter(|value| !value.is_empty()),
        permissions_pending: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolObservationOrigin {
    McpProxy,
    Direct,
}

/// Closed, content-free identity for a direct daemon client. This is carried
/// separately from `observation_origin` so older daemons safely ignore the
/// additive field instead of rejecting a newer enum variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonClientKind {
    Cli,
    PythonSdk,
    TypescriptSdk,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRequest {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    /// Transport-owned connection identity. Public session identity remains
    /// in `args.session`; callers must not use this field to select policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Bounded internal routing metadata for exactly-once completion
    /// observation. Older peers ignore this additive field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_origin: Option<ToolObservationOrigin>,
    /// Privacy-bounded direct-client identity used only for aggregate product
    /// telemetry. Raw package names, application names, and host identifiers
    /// are never accepted here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_kind: Option<DaemonClientKind>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl DaemonResponse {
    pub fn ok(result: serde_json::Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
            exit_code: None,
        }
    }

    pub fn err(message: impl Into<String>, exit_code: i32) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(message.into()),
            exit_code: Some(exit_code),
        }
    }
}

/// Perform the daemon identity/version handshake used by embedded startup and
/// imported SDK diagnostics. Unlike a connect-only probe, this proves the
/// endpoint speaks the expected Cua wire protocol.
pub fn request_daemon_metadata(socket_path: &str) -> anyhow::Result<DaemonMetadata> {
    let response = send_request(
        socket_path,
        &DaemonRequest {
            method: "metadata".into(),
            name: None,
            args: None,
            session_id: None,
            observation_origin: None,
            client_kind: None,
        },
    )?;
    if !response.ok {
        anyhow::bail!(
            "daemon metadata request failed: {}",
            response.error.unwrap_or_else(|| "unknown error".into())
        );
    }
    serde_json::from_value(
        response
            .result
            .ok_or_else(|| anyhow::anyhow!("daemon metadata response omitted result"))?,
    )
    .map_err(Into::into)
}

/// Return the platform socket or pipe path for an installed-product namespace.
/// The executable chooses its release/local namespace; imported SDKs use the
/// release namespace unless the caller supplies an explicit path.
pub fn socket_path_for_namespace(namespace: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/Library/Caches/{namespace}/{namespace}.sock")
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.cache/{namespace}/{namespace}.sock")
    }
    #[cfg(target_os = "windows")]
    {
        format!(r"\\.\pipe\{namespace}")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        format!("/tmp/{namespace}.sock")
    }
}

/// What one read-only `list` request finds on a daemon endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointProbe {
    /// A daemon answered `list` with `ok`.
    Answers,
    /// Something holds the endpoint but gave no `ok` answer in time.
    Unanswered,
    /// Nothing listens on the endpoint.
    Closed,
}

/// Connect to `socket_path` and send the protocol's read-only `list`
/// request, the same way on every OS, waiting up to `reply_within` for the
/// answer. The connection is closed before this returns, answered or not.
pub fn probe_endpoint(socket_path: &str, reply_within: Duration) -> EndpointProbe {
    match probe_exchange(socket_path, &read_only_request("list"), reply_within) {
        Ok(response) if response.ok => EndpointProbe::Answers,
        Ok(_) | Err(ProbeFailure::Unanswered) => EndpointProbe::Unanswered,
        Err(ProbeFailure::Closed) => EndpointProbe::Closed,
    }
}

/// Whether a daemon answers on `socket_path` within
/// [`PRESENCE_REPLY_DEADLINE`], by [`probe_endpoint`].
pub fn is_daemon_listening(socket_path: &str) -> bool {
    probe_endpoint(socket_path, PRESENCE_REPLY_DEADLINE) == EndpointProbe::Answers
}

fn read_only_request(method: &str) -> DaemonRequest {
    DaemonRequest {
        method: method.into(),
        name: None,
        args: None,
        session_id: None,
        observation_origin: None,
        client_kind: None,
    }
}

enum ProbeFailure {
    Closed,
    Unanswered,
}

/// Send one request with a single connect attempt and wait up to
/// `reply_within` for the reply.
#[cfg(unix)]
fn probe_exchange(
    socket_path: &str,
    request: &DaemonRequest,
    reply_within: Duration,
) -> Result<DaemonResponse, ProbeFailure> {
    let stream =
        std::os::unix::net::UnixStream::connect(socket_path).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                    ProbeFailure::Closed
                }
                _ => ProbeFailure::Unanswered,
            }
        })?;
    exchange_over(stream, request, reply_within).map_err(|_| ProbeFailure::Unanswered)
}

/// Send one request and wait up to `reply_within` for the reply. Only a pipe
/// name with no server instance reads as closed; a busy or inaccessible pipe
/// still has a server. The pipe is opened for overlapped I/O so a late reply
/// is cancelled and the instance handed back when the wait ends.
#[cfg(windows)]
fn probe_exchange(
    socket_path: &str,
    request: &DaemonRequest,
    reply_within: Duration,
) -> Result<DaemonResponse, ProbeFailure> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use std::time::Instant;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::CreateEventW;

    const ERROR_PIPE_BUSY: i32 = 231;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

    let deadline = Instant::now() + reply_within;
    let pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(socket_path)
        {
            Ok(pipe) => break pipe,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProbeFailure::Closed);
            }
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return Err(ProbeFailure::Unanswered),
        }
    };
    let line = serde_json::to_string(request).map_err(|_| ProbeFailure::Unanswered)? + "\n";
    let handle = HANDLE(pipe.as_raw_handle());
    // SAFETY: an unnamed manual-reset event, closed below.
    let event =
        unsafe { CreateEventW(None, true, false, None) }.map_err(|_| ProbeFailure::Unanswered)?;
    let reply = exchange_overlapped(handle, event, line.as_bytes(), deadline);
    // SAFETY: no transfer is pending on `event` once the exchange returns.
    unsafe {
        let _ = CloseHandle(event);
    }
    drop(pipe);
    reply.ok_or(ProbeFailure::Unanswered)
}

/// Write `line` and read one reply line on the overlapped pipe `pipe`.
#[cfg(windows)]
fn exchange_overlapped(
    pipe: windows::Win32::Foundation::HANDLE,
    event: windows::Win32::Foundation::HANDLE,
    line: &[u8],
    deadline: std::time::Instant,
) -> Option<DaemonResponse> {
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};

    let mut written = 0;
    while written < line.len() {
        let pending = &line[written..];
        // SAFETY: `pending` and the OVERLAPPED outlive the transfer, which
        // `overlapped_transfer` finishes or cancels before returning.
        let sent = overlapped_transfer(pipe, event, deadline, |overlapped| unsafe {
            WriteFile(pipe, Some(pending), None, Some(overlapped))
        })?;
        if sent == 0 {
            return None;
        }
        written += sent as usize;
    }

    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(newline) = buffer.iter().position(|&byte| byte == b'\n') {
            return serde_json::from_slice(&buffer[..newline]).ok();
        }
        // SAFETY: as for the write above.
        let read = overlapped_transfer(pipe, event, deadline, |overlapped| unsafe {
            ReadFile(pipe, Some(&mut chunk), None, Some(overlapped))
        })?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read as usize]);
    }
}

/// Start one overlapped transfer and wait for it until `deadline`. A transfer
/// still pending then is cancelled, and it has always finished by the time
/// this returns, so its buffer may be reused or freed.
#[cfg(windows)]
fn overlapped_transfer(
    pipe: windows::Win32::Foundation::HANDLE,
    event: windows::Win32::Foundation::HANDLE,
    deadline: std::time::Instant,
    start: impl FnOnce(*mut windows::Win32::System::IO::OVERLAPPED) -> windows::core::Result<()>,
) -> Option<u32> {
    use windows::Win32::Foundation::{ERROR_IO_PENDING, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::WaitForSingleObject;
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

    let mut overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    if let Err(error) = start(&mut overlapped) {
        if error.code() != ERROR_IO_PENDING.to_hresult() {
            return None;
        }
    }
    let wait_ms = deadline
        .saturating_duration_since(std::time::Instant::now())
        .as_millis()
        .min(u128::from(u32::MAX - 1)) as u32;
    let mut transferred = 0;
    // SAFETY: `overlapped` stays in place until GetOverlappedResult has
    // waited for the transfer to finish, cancelled or not.
    unsafe {
        if WaitForSingleObject(event, wait_ms) != WAIT_OBJECT_0 {
            let _ = CancelIoEx(pipe, Some(&overlapped));
        }
        GetOverlappedResult(pipe, &overlapped, &mut transferred, true).ok()?;
    }
    Some(transferred)
}

#[cfg(not(any(unix, windows)))]
fn probe_exchange(
    _socket_path: &str,
    _request: &DaemonRequest,
    _reply_within: Duration,
) -> Result<DaemonResponse, ProbeFailure> {
    Err(ProbeFailure::Closed)
}

/// Whether a serving daemon runs tool calls yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGate {
    Open,
    /// Every tool call returns `permissions_pending` (macOS, before the
    /// Accessibility and Screen Recording grants arrive).
    PermissionsPending,
}

/// Who holds a daemon socket path, read by [`probe_presence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonPresence {
    /// Nothing answers, nothing holds the lock, and no socket file is left.
    Absent,
    /// Nothing answers or holds the lock, but a dead daemon left its socket
    /// file. The next launch clears it. A path that is not a socket is never
    /// stale.
    Stale { socket_file: std::path::PathBuf },
    /// A process holds the daemon lock without answering: a launch that has
    /// not started serving yet, or a daemon finishing its shutdown.
    Starting { lock_held: std::path::PathBuf },
    /// A daemon answers on the socket.
    Serving { pid: u32, gate: ToolGate },
}

/// How long [`probe_presence`] waits for an answer. A daemon that has bound
/// its socket but not yet started accepting reads as `Starting`.
const PRESENCE_REPLY_DEADLINE: Duration = Duration::from_secs(2);

/// The lock a standalone daemon holds on `socket_path` from launch until it
/// exits. The kernel drops it when the daemon dies.
pub fn daemon_lock_path(socket_path: &str) -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    {
        // `%LOCALAPPDATA%` stays the same across a user's sessions where
        // `%TEMP%` may not. The file name is a fixed-length digest of the
        // uppercased name, because Windows compares pipe names
        // case-insensitively and a name can outgrow a path component.
        use sha2::Digest as _;
        let base = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let digest = sha2::Sha256::digest(socket_path.to_uppercase().as_bytes());
        base.join("cua-driver-pipe-locks")
            .join(format!("{digest:x}.lock"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::path::PathBuf::from(format!("{socket_path}.lock"))
    }
}

/// Read the daemon's presence on `socket_path` from its answer, its lock and
/// its socket file together. The lock is tried and released at once, never
/// held, so a launch waiting on it is delayed by an instant at most.
pub fn probe_presence(socket_path: &str) -> DaemonPresence {
    if let Some(metadata) = probe_metadata(socket_path) {
        return DaemonPresence::Serving {
            pid: metadata.pid,
            gate: if metadata.permissions_pending {
                ToolGate::PermissionsPending
            } else {
                ToolGate::Open
            },
        };
    }
    let lock_path = daemon_lock_path(socket_path);
    if lock_is_held(&lock_path) {
        return DaemonPresence::Starting {
            lock_held: lock_path,
        };
    }
    // A named pipe vanishes with its last server, so only Unix leaves a file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt as _;
        if std::fs::symlink_metadata(socket_path)
            .is_ok_and(|metadata| metadata.file_type().is_socket())
        {
            return DaemonPresence::Stale {
                socket_file: socket_path.into(),
            };
        }
    }
    DaemonPresence::Absent
}

fn probe_metadata(socket_path: &str) -> Option<DaemonMetadata> {
    let response = probe_exchange(
        socket_path,
        &read_only_request("metadata"),
        PRESENCE_REPLY_DEADLINE,
    )
    .ok()?;
    if !response.ok {
        return None;
    }
    serde_json::from_value(response.result?).ok()
}

fn lock_is_held(lock_path: &std::path::Path) -> bool {
    let Ok(lock) = std::fs::File::open(lock_path) else {
        return false;
    };
    matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// Room left for the daemon to accept the connection, parse the request, run
/// a tool for its full budget and serialize the reply.
const DAEMON_OVERHEAD_ALLOWANCE: Duration = Duration::from_secs(30);

/// Longest a client waits for the daemon's reply to one request. The largest
/// budget a caller can ask any tool for ([`crate::tool_schema::TIMEOUT_MS_MAX`])
/// fits under it with [`DAEMON_OVERHEAD_ALLOWANCE`] to spare, so a tool that
/// legitimately uses its whole budget still gets its result delivered.
pub const REPLY_DEADLINE: Duration = Duration::from_millis(crate::tool_schema::TIMEOUT_MS_MAX)
    .saturating_add(DAEMON_OVERHEAD_ALLOWANCE);

/// Send one newline-delimited request and read one response.
///
/// Unix reads and writes retry transient timeouts until [`REPLY_DEADLINE`] so
/// slow accessibility walks and large screenshots are supported.
#[cfg(unix)]
pub fn send_request(socket_path: &str, request: &DaemonRequest) -> anyhow::Result<DaemonResponse> {
    let stream = std::os::unix::net::UnixStream::connect(socket_path)
        .map_err(|error| anyhow::anyhow!("connect to {socket_path}: {error}"))?;
    exchange_over(stream, request, REPLY_DEADLINE)
}

#[cfg(unix)]
fn exchange_over(
    mut stream: std::os::unix::net::UnixStream,
    request: &DaemonRequest,
    reply_deadline: Duration,
) -> anyhow::Result<DaemonResponse> {
    use std::io::Read;
    use std::time::Instant;

    const READ_POLL: Duration = Duration::from_secs(10);

    // macOS rejects setsockopt with EINVAL once the peer has closed its end,
    // even when the peer's reply is still buffered for us to read. Treat that
    // as "peer already gone": the read below still drains buffered data and
    // reports a real close, instead of failing with a bare "Invalid argument".
    set_timeout_unless_peer_closed(stream.set_write_timeout(Some(Duration::from_secs(5))))?;

    let mut writer = stream.try_clone()?;
    let line = serde_json::to_string(request)? + "\n";
    let write_deadline = Instant::now() + reply_deadline;
    crate::socket_io::write_all_with_retry(&mut writer, line.as_bytes(), write_deadline)?;

    let reply_by = Instant::now() + reply_deadline;
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut chunk = [0_u8; 64 * 1024];
    let response_line = loop {
        if let Some(newline) = buffer.iter().position(|&byte| byte == b'\n') {
            break String::from_utf8_lossy(&buffer[..newline]).into_owned();
        }
        let remaining = reply_by.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!(
                "timed out after {reply_deadline:?} waiting for daemon response (received {} bytes so far); \
                 the daemon may still be running the request",
                buffer.len()
            );
        }
        set_timeout_unless_peer_closed(stream.set_read_timeout(Some(remaining.min(READ_POLL))))?;
        match stream.read(&mut chunk) {
            Ok(0) if buffer.is_empty() => {
                anyhow::bail!("daemon closed connection without response");
            }
            Ok(0) => break String::from_utf8_lossy(&buffer).into_owned(),
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }
    };

    Ok(serde_json::from_str(&response_line)?)
}

#[cfg(unix)]
fn set_timeout_unless_peer_closed(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
        other => other,
    }
}

#[cfg(not(unix))]
pub fn send_request(socket_path: &str, request: &DaemonRequest) -> anyhow::Result<DaemonResponse> {
    #[cfg(target_os = "windows")]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::time::Duration;

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let pipe = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(socket_path)
            {
                Ok(pipe) => break pipe,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => anyhow::bail!("connect to named pipe {socket_path}: {error}"),
            }
        };

        let mut writer = pipe.try_clone()?;
        writer.write_all((serde_json::to_string(request)? + "\n").as_bytes())?;
        writer.flush()?;

        let response_line = BufReader::new(pipe)
            .lines()
            .next()
            .ok_or_else(|| anyhow::anyhow!("daemon closed connection without response"))??;
        Ok(serde_json::from_str(&response_line)?)
    }
    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("daemon client not supported on this platform (socket path: {socket_path})");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        current_daemon_metadata, socket_path_for_namespace, DaemonClientKind, DaemonRequest,
        DaemonResponse, ToolObservationOrigin, DAEMON_OVERHEAD_ALLOWANCE, REPLY_DEADLINE,
    };
    use std::time::Duration;

    #[test]
    fn the_reply_deadline_outlasts_every_tool_budget() {
        #[cfg_attr(not(feature = "script"), allow(unused_mut))]
        let mut budgets_ms = vec![crate::tool_schema::TIMEOUT_MS_MAX];
        #[cfg(feature = "script")]
        budgets_ms.push(crate::script_tool::MAX_TIMEOUT_MS);
        for budget_ms in budgets_ms {
            assert!(
                REPLY_DEADLINE >= Duration::from_millis(budget_ms) + DAEMON_OVERHEAD_ALLOWANCE,
                "a tool may use {budget_ms} ms and the client gives up after {REPLY_DEADLINE:?}"
            );
        }
    }

    #[test]
    fn socket_path_keeps_the_selected_namespace() {
        let path = socket_path_for_namespace("cua-driver-test");
        assert!(path.contains("cua-driver-test"));
    }

    #[test]
    fn legacy_request_without_additive_fields_deserializes() {
        let request: DaemonRequest = serde_json::from_value(serde_json::json!({
            "method": "call",
            "name": "get_screen_size",
            "args": {}
        }))
        .unwrap();
        assert_eq!(request.session_id, None);
        assert_eq!(request.observation_origin, None);
        assert_eq!(request.client_kind, None);
    }

    #[test]
    fn direct_client_kind_is_bounded_and_snake_case() {
        let request = DaemonRequest {
            method: "call".into(),
            name: Some("start_session".into()),
            args: None,
            session_id: None,
            observation_origin: Some(ToolObservationOrigin::Direct),
            client_kind: Some(DaemonClientKind::TypescriptSdk),
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["client_kind"], "typescript_sdk");
        assert!(value.get("application_name").is_none());
        assert!(value.get("package_name").is_none());
    }

    #[test]
    fn response_helpers_preserve_wire_shape() {
        let ok = DaemonResponse::ok(serde_json::json!({"value": 1}));
        assert!(ok.ok);
        assert_eq!(ok.result.unwrap()["value"], 1);

        let error = DaemonResponse::err("nope", 64);
        assert!(!error.ok);
        assert_eq!(error.error.as_deref(), Some("nope"));
        assert_eq!(error.exit_code, Some(64));
    }

    #[test]
    fn observation_origin_is_snake_case() {
        assert_eq!(
            serde_json::to_value(ToolObservationOrigin::McpProxy).unwrap(),
            "mcp_proxy"
        );
    }

    #[test]
    fn metadata_is_versioned_and_process_bound() {
        let metadata = current_daemon_metadata();
        assert_eq!(metadata.pid, std::process::id());
        assert_eq!(
            metadata.contract_version,
            cua_driver_contract::CONTRACT_VERSION
        );
        assert_eq!(
            metadata.mcp_protocol_version,
            cua_driver_contract::MCP_PROTOCOL_VERSION
        );
    }

    #[test]
    fn metadata_from_a_daemon_without_a_gate_reads_as_open() {
        let mut older = serde_json::to_value(current_daemon_metadata()).unwrap();
        older.as_object_mut().unwrap().remove("permissions_pending");
        let metadata: super::DaemonMetadata = serde_json::from_value(older).unwrap();
        assert!(!metadata.permissions_pending);
    }
}

#[cfg(all(test, any(unix, windows)))]
mod probe_endpoint_tests {
    use super::{
        probe_endpoint, DaemonRequest, DaemonResponse, EndpointProbe, PRESENCE_REPLY_DEADLINE,
    };
    use std::time::Duration;
    use tokio::io::{
        AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
    };

    async fn probe(endpoint: String) -> EndpointProbe {
        tokio::task::spawn_blocking(move || probe_endpoint(&endpoint, PRESENCE_REPLY_DEADLINE))
            .await
            .unwrap()
    }

    /// Read one request and answer it, recording the method it asked for.
    async fn answer(
        connection: impl AsyncRead + AsyncWrite + Unpin,
        response: DaemonResponse,
    ) -> String {
        let mut connection = BufReader::new(connection);
        let mut line = String::new();
        connection.read_line(&mut line).await.unwrap();
        let request: DaemonRequest = serde_json::from_str(&line).unwrap();
        let reply = serde_json::to_string(&response).unwrap() + "\n";
        connection.write_all(reply.as_bytes()).await.unwrap();
        request.method
    }

    #[cfg(unix)]
    mod endpoint {
        use tokio::net::{UnixListener, UnixStream};

        pub struct Endpoint {
            _dir: tempfile::TempDir,
            pub path: String,
        }

        pub fn unused() -> Endpoint {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("d.sock").to_str().unwrap().to_owned();
            Endpoint { _dir: dir, path }
        }

        /// A listener that accepts at the kernel and never reads.
        pub fn listen(endpoint: &Endpoint) -> UnixListener {
            UnixListener::bind(&endpoint.path).unwrap()
        }

        pub async fn accept(listener: UnixListener) -> UnixStream {
            listener.accept().await.unwrap().0
        }
    }

    #[cfg(windows)]
    mod endpoint {
        use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

        pub struct Endpoint {
            pub path: String,
        }

        pub fn unused() -> Endpoint {
            Endpoint {
                path: format!(
                    r"\\.\pipe\cua-driver-probe-{}",
                    uuid::Uuid::new_v4().simple()
                ),
            }
        }

        /// A pipe instance that accepts a client and never reads.
        pub fn listen(endpoint: &Endpoint) -> NamedPipeServer {
            ServerOptions::new()
                .first_pipe_instance(true)
                .create(&endpoint.path)
                .unwrap()
        }

        pub async fn accept(server: NamedPipeServer) -> NamedPipeServer {
            server.connect().await.unwrap();
            server
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_endpoint_nothing_listens_on_reads_as_closed() {
        let endpoint = endpoint::unused();
        assert_eq!(probe(endpoint.path.clone()).await, EndpointProbe::Closed);

        // A dead daemon's socket file is still closed.
        drop(endpoint::listen(&endpoint));
        assert_eq!(probe(endpoint.path.clone()).await, EndpointProbe::Closed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_endpoint_that_never_answers_reads_as_unanswered() {
        let endpoint = endpoint::unused();
        let _listener = endpoint::listen(&endpoint);
        assert_eq!(
            probe(endpoint.path.clone()).await,
            EndpointProbe::Unanswered
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unanswered_probe_closes_its_connection() {
        let endpoint = endpoint::unused();
        let listener = endpoint::listen(&endpoint);
        let probed = tokio::spawn(probe(endpoint.path.clone()));
        let mut connection = BufReader::new(endpoint::accept(listener).await);
        let mut request = String::new();
        connection.read_line(&mut request).await.unwrap();
        assert_eq!(probed.await.unwrap(), EndpointProbe::Unanswered);

        // The probe gave the connection back instead of leaving a read
        // pending on it, so the server sees it close.
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), connection.read_to_end(&mut rest))
            .await
            .expect("the probe kept its connection open after giving up")
            .ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_an_ok_answer_to_list_reads_as_answers() {
        for (response, expected) in [
            (
                DaemonResponse::ok(serde_json::json!([])),
                EndpointProbe::Answers,
            ),
            (DaemonResponse::err("refused", 1), EndpointProbe::Unanswered),
        ] {
            let endpoint = endpoint::unused();
            let listener = endpoint::listen(&endpoint);
            let probed = tokio::spawn(probe(endpoint.path.clone()));
            let method = answer(endpoint::accept(listener).await, response).await;
            assert_eq!(method, "list");
            assert_eq!(probed.await.unwrap(), expected);
        }
    }
}
