//! Canonical typed Cua Driver SDK and UniFFI export boundary.
//!
//! [`CuaDriver::create`] owns the platform runtime in the importing process.
//! [`CuaDriver::connect`] remains a temporary compatibility constructor for the
//! released daemon-client topology. Both paths expose the same typed operations;
//! MCP and daemon transports are downstream adapters rather than peer contracts.

use cua_driver_contract::{
    ClickInput, DragInput, EndSessionInput, EndSessionOutput, EscalateSessionInput,
    GetCursorPositionInput, GetDesktopStateInput, GetScreenSizeInput, GetSessionStateInput,
    HotkeyInput, MoveCursorInput, PressKeyInput, ScrollInput, SessionStateOutput,
    StartSessionInput, StartSessionOutput, ToolInput, TypeTextInput,
};
use cua_driver_core::daemon::{
    is_daemon_listening, request_daemon_metadata, send_request, socket_path_for_namespace,
    DaemonClientKind, DaemonRequest, ToolObservationOrigin,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;

mod abi;
mod embedded;
mod runtime;
use abi::NativeAbiDriver;
pub use embedded::*;
use runtime::RuntimeOptions;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ImageContent {
    pub mime_type: String,
    pub data_base64: String,
}

/// Transport-neutral result envelope used for open-ended tool calls and
/// desktop tools whose platform extensions are intentionally preserved as JSON.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ToolResult {
    pub text: String,
    pub images: Vec<ImageContent>,
    pub structured_json: Option<String>,
    pub is_error: bool,
    pub error_code: Option<String>,
    pub verified: Option<bool>,
    pub degraded: bool,
    pub raw_json: String,
}

/// Transport-independent daemon identity used to prove that standalone and
/// embedded SDK/MCP routes reached a compatible Rust implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct DriverMetadata {
    pub driver_version: String,
    pub contract_version: String,
    pub tools_list_schema_version: String,
    pub capability_version: String,
    pub mcp_protocol_version: String,
    pub pid: u32,
    pub embedded: bool,
    pub host_bundle_id: Option<String>,
}

/// The TCC grants required by a macOS host application before it starts an
/// embedded driver. These probes execute in the importing SDK process so the
/// operating system attributes the request to the host application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MacOsPermissionStatus {
    pub accessibility: bool,
    pub screen_recording: bool,
}

#[uniffi::export]
pub fn current_mac_os_permission_status() -> MacOsPermissionStatus {
    #[cfg(target_os = "macos")]
    {
        MacOsPermissionStatus {
            accessibility: mac_os_accessibility_granted(),
            screen_recording: mac_os_screen_recording_granted(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        MacOsPermissionStatus {
            accessibility: true,
            screen_recording: true,
        }
    }
}

#[uniffi::export]
pub fn request_mac_os_permissions() -> MacOsPermissionStatus {
    #[cfg(target_os = "macos")]
    {
        MacOsPermissionStatus {
            accessibility: request_mac_os_accessibility(),
            screen_recording: request_mac_os_screen_recording(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        MacOsPermissionStatus {
            accessibility: true,
            screen_recording: true,
        }
    }
}

#[uniffi::export]
pub fn open_mac_os_screen_recording_settings() -> Result<(), DriverError> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
            .status()
            .map_err(|error| DriverError::Protocol {
                reason: format!("open macOS Screen Recording settings: {error}"),
            })?;
        if !status.success() {
            return Err(DriverError::Protocol {
                reason: format!(
                    "open macOS Screen Recording settings exited with {:?}",
                    status.code()
                ),
            });
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn mac_os_accessibility_granted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[cfg(target_os = "macos")]
fn request_mac_os_accessibility() -> bool {
    use core_foundation::base::TCFType as _;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::CFString;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    }
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

#[cfg(target_os = "macos")]
fn mac_os_screen_recording_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[cfg(target_os = "macos")]
fn request_mac_os_screen_recording() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}

#[derive(Debug, Error, uniffi::Error)]
pub enum DriverError {
    #[error("invalid SDK configuration: {reason}")]
    Configuration { reason: String },
    #[error("invalid arguments for {tool}: {reason}")]
    InvalidArguments { tool: String, reason: String },
    #[error("daemon transport error on {socket_path}: {reason}")]
    Transport { socket_path: String, reason: String },
    #[error("invalid daemon response: {reason}")]
    Protocol { reason: String },
    #[error("{tool} failed: {message}")]
    Tool {
        tool: String,
        message: String,
        error_code: String,
    },
    #[error("the Cua Driver SDK has been shut down")]
    Shutdown,
}

#[derive(uniffi::Object)]
pub struct CuaDriver {
    backend: DriverBackend,
    client_kind: DaemonClientKind,
}

enum DriverBackend {
    Embedded(Arc<NativeAbiDriver>),
    Daemon { socket_path: String },
}

/// Process topology used by this SDK object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DriverExecutionMode {
    Embedded,
    Daemon,
}

/// Options for a same-process Cua Driver SDK runtime.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct DriverOptions {
    /// Preserve the temporary reduced screenshot surface used by older Claude
    /// Code integrations. New applications should leave this false.
    pub claude_code_compatibility: bool,
}

/// Rust-only host configuration used by the standalone daemon. Language
/// bindings intentionally receive the smaller [`DriverOptions`] record.
pub struct DriverHostOptions {
    pub cursor: cursor_overlay::CursorConfig,
    pub claude_code_compatibility: bool,
    pub prepare_desktop_environment: bool,
    /// Temporary compatibility hook for daemon-only administrative tools.
    /// Desktop operations must live behind the typed SDK contract instead.
    pub register_host_tools: Option<fn(&mut cua_driver_core::tool::ToolRegistry)>,
}

/// Runtime that imported the shared UniFFI SDK library. The language package
/// selects this automatically at its root entry point; callers do not need to
/// supply telemetry metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SdkClientKind {
    Python,
    Typescript,
}

impl From<SdkClientKind> for DaemonClientKind {
    fn from(value: SdkClientKind) -> Self {
        match value {
            SdkClientKind::Python => Self::PythonSdk,
            SdkClientKind::Typescript => Self::TypescriptSdk,
        }
    }
}

macro_rules! desktop_tool_methods {
    ($callback:ident) => {
        $callback! {
            get_desktop_state: GetDesktopStateInput,
            get_screen_size: GetScreenSizeInput,
            get_cursor_position: GetCursorPositionInput,
            move_cursor: MoveCursorInput,
            click: ClickInput,
            drag: DragInput,
            scroll: ScrollInput,
            type_text: TypeTextInput,
            press_key: PressKeyInput,
            hotkey: HotkeyInput,
        }
    };
}

macro_rules! define_desktop_tool_methods {
    ($($method:ident: $input:ty,)*) => {
        #[uniffi::export(async_runtime = "tokio")]
        impl CuaDriver {
            $(
                pub async fn $method(&self, input: $input) -> Result<ToolResult, DriverError> {
                    self.invoke_typed(<$input as ToolInput>::TOOL_NAME, input).await
                }
            )*
        }
    };
}

macro_rules! define_exported_tool_names {
    ($($method:ident: $input:ty,)*) => {
        #[cfg(test)]
        const EXPORTED_TOOL_NAMES: &[&str] = &[
            <StartSessionInput as ToolInput>::TOOL_NAME,
            <EscalateSessionInput as ToolInput>::TOOL_NAME,
            <GetSessionStateInput as ToolInput>::TOOL_NAME,
            <EndSessionInput as ToolInput>::TOOL_NAME,
            $(<$input as ToolInput>::TOOL_NAME,)*
        ];
    };
}

desktop_tool_methods!(define_exported_tool_names);

#[uniffi::export]
impl CuaDriver {
    /// Create a same-process driver runtime. This constructor never launches
    /// `cua-driver` and never opens daemon IPC.
    #[uniffi::constructor]
    pub fn create(options: Option<DriverOptions>) -> Result<Arc<Self>, DriverError> {
        let options = options.unwrap_or_default();
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(NativeAbiDriver::create(
                options.claude_code_compatibility,
            )?)),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Language-package entry point for the same-process runtime. The wrapper
    /// at each package root selects the client kind automatically.
    #[uniffi::constructor]
    pub fn create_with_client_kind(
        options: Option<DriverOptions>,
        client_kind: SdkClientKind,
    ) -> Result<Arc<Self>, DriverError> {
        let driver = Self::create(options)?;
        let DriverBackend::Embedded(runtime) = &driver.backend else {
            unreachable!("create always returns an embedded runtime")
        };
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(runtime.clone()),
            client_kind: client_kind.into(),
        }))
    }

    /// Connect to the default installed daemon or an explicitly selected socket.
    /// This is the temporary compatibility path for released socket clients.
    #[uniffi::constructor]
    pub fn connect(socket_path: Option<String>) -> Result<Arc<Self>, DriverError> {
        let socket_path = socket_path.unwrap_or_else(|| socket_path_for_namespace("cua-driver"));
        if socket_path.trim().is_empty() {
            return Err(DriverError::Configuration {
                reason: "socket_path must not be empty".into(),
            });
        }
        Ok(Arc::new(Self {
            backend: DriverBackend::Daemon { socket_path },
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Language-package entry point that preserves the public `connect` API
    /// while attaching a closed runtime category to daemon requests.
    #[uniffi::constructor]
    pub fn connect_with_client_kind(
        socket_path: Option<String>,
        client_kind: SdkClientKind,
    ) -> Result<Arc<Self>, DriverError> {
        let driver = Self::connect(socket_path)?;
        let DriverBackend::Daemon { socket_path } = &driver.backend else {
            unreachable!("connect always returns a daemon client")
        };
        Ok(Arc::new(Self {
            backend: DriverBackend::Daemon {
                socket_path: socket_path.clone(),
            },
            client_kind: client_kind.into(),
        }))
    }

    pub fn execution_mode(&self) -> DriverExecutionMode {
        match &self.backend {
            DriverBackend::Embedded(_) => DriverExecutionMode::Embedded,
            DriverBackend::Daemon { .. } => DriverExecutionMode::Daemon,
        }
    }

    /// Compatibility accessor. Embedded runtimes have no socket and return an
    /// empty string; new code should branch on [`Self::execution_mode`].
    pub fn socket_path(&self) -> String {
        match &self.backend {
            DriverBackend::Embedded(_) => String::new(),
            DriverBackend::Daemon { socket_path } => socket_path.clone(),
        }
    }

    pub fn is_available(&self) -> bool {
        match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.is_available(),
            DriverBackend::Daemon { socket_path } => is_daemon_listening(socket_path),
        }
    }
}

impl CuaDriver {
    /// Construct the same SDK-owned runtime for the standalone daemon host.
    /// This Rust-only entry point keeps CLI presentation options out of the
    /// generated language contract.
    pub fn create_for_host(options: DriverHostOptions) -> Arc<Self> {
        Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(NativeAbiDriver::create_for_host(
                RuntimeOptions {
                    cursor: options.cursor,
                    compatibility_mode: options.claude_code_compatibility,
                    prepare_desktop_environment: options.prepare_desktop_environment,
                    register_host_tools: options.register_host_tools,
                },
            ))),
            client_kind: DaemonClientKind::Unknown,
        })
    }

    /// Transitional adapter for the released private daemon protocol and the
    /// current MCP host. It is intentionally not exported through UniFFI and
    /// will disappear when the official MCP adapter owns generic dispatch.
    #[doc(hidden)]
    pub fn compatibility_registry(&self) -> Option<Arc<cua_driver_core::tool::ToolRegistry>> {
        match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.registry(),
            DriverBackend::Daemon { .. } => None,
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl CuaDriver {
    pub async fn metadata(&self) -> Result<DriverMetadata, DriverError> {
        match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.metadata(),
            DriverBackend::Daemon { socket_path } => {
                let socket_path = socket_path.clone();
                let request_path = socket_path.clone();
                let metadata =
                    tokio::task::spawn_blocking(move || request_daemon_metadata(&request_path))
                        .await
                        .map_err(|error| DriverError::Protocol {
                            reason: format!("metadata task failed: {error}"),
                        })?
                        .map_err(|error| DriverError::Transport {
                            socket_path,
                            reason: error.to_string(),
                        })?;
                Ok(DriverMetadata {
                    driver_version: metadata.driver_version,
                    contract_version: metadata.contract_version,
                    tools_list_schema_version: metadata.tools_list_schema_version,
                    capability_version: metadata.capability_version,
                    mcp_protocol_version: metadata.mcp_protocol_version,
                    pid: metadata.pid,
                    embedded: metadata.embedded,
                    host_bundle_id: metadata.host_bundle_id,
                })
            }
        }
    }

    /// Temporary compatibility escape hatch. New application code should use
    /// the typed methods; MCP servers own generic discovery and invocation.
    pub async fn call_tool(
        &self,
        name: String,
        arguments_json: String,
    ) -> Result<ToolResult, DriverError> {
        let arguments = parse_arguments(&name, &arguments_json)?;
        self.invoke(&name, arguments).await
    }

    /// Temporary compatibility discovery surface for adapters migrating to
    /// the typed SDK contract.
    pub async fn list_tools_json(&self) -> Result<String, DriverError> {
        let result = match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.tools_list()?,
            DriverBackend::Daemon { socket_path } => {
                let socket_path = socket_path.clone();
                let request_path = socket_path.clone();
                let request = DaemonRequest {
                    method: "list".into(),
                    name: None,
                    args: None,
                    session_id: None,
                    observation_origin: Some(ToolObservationOrigin::Direct),
                    client_kind: Some(self.client_kind),
                };
                let response =
                    tokio::task::spawn_blocking(move || send_request(&request_path, &request))
                        .await
                        .map_err(|error| DriverError::Protocol {
                            reason: format!("list task failed: {error}"),
                        })?
                        .map_err(|error| DriverError::Transport {
                            socket_path,
                            reason: error.to_string(),
                        })?;
                if !response.ok {
                    return Err(DriverError::Protocol {
                        reason: response
                            .error
                            .unwrap_or_else(|| "list request failed".into()),
                    });
                }
                response.result.unwrap_or(Value::Null)
            }
        };
        serde_json::to_string(&result).map_err(|error| DriverError::Protocol {
            reason: error.to_string(),
        })
    }

    pub async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<StartSessionOutput, DriverError> {
        self.invoke_typed(StartSessionInput::TOOL_NAME, input)
            .await?
            .typed_success(StartSessionInput::TOOL_NAME)
    }

    pub async fn escalate_session(
        &self,
        input: EscalateSessionInput,
    ) -> Result<SessionStateOutput, DriverError> {
        self.invoke_typed(EscalateSessionInput::TOOL_NAME, input)
            .await?
            .typed_success(EscalateSessionInput::TOOL_NAME)
    }

    pub async fn get_session_state(
        &self,
        input: GetSessionStateInput,
    ) -> Result<SessionStateOutput, DriverError> {
        self.invoke_typed(GetSessionStateInput::TOOL_NAME, input)
            .await?
            .typed_success(GetSessionStateInput::TOOL_NAME)
    }

    pub async fn end_session(
        &self,
        input: EndSessionInput,
    ) -> Result<EndSessionOutput, DriverError> {
        self.invoke_typed(EndSessionInput::TOOL_NAME, input)
            .await?
            .typed_success(EndSessionInput::TOOL_NAME)
    }

    /// Stop accepting new embedded operations. Repeated calls are harmless;
    /// daemon compatibility clients do not own the daemon and therefore no-op.
    pub async fn shutdown(&self) -> Result<(), DriverError> {
        if let DriverBackend::Embedded(runtime) = &self.backend {
            runtime.shutdown().await?;
        }
        Ok(())
    }
}

desktop_tool_methods!(define_desktop_tool_methods);

impl CuaDriver {
    async fn invoke_typed<T: Serialize>(
        &self,
        name: &str,
        input: T,
    ) -> Result<ToolResult, DriverError> {
        let arguments =
            serde_json::to_value(input).map_err(|error| DriverError::InvalidArguments {
                tool: name.into(),
                reason: error.to_string(),
            })?;
        self.invoke(name, arguments).await
    }

    async fn invoke(&self, name: &str, arguments: Value) -> Result<ToolResult, DriverError> {
        if !arguments.is_object() {
            return Err(DriverError::InvalidArguments {
                tool: name.into(),
                reason: "arguments must be a JSON object".into(),
            });
        }
        let raw = match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.invoke(name, arguments).await?,
            DriverBackend::Daemon { socket_path } => {
                let socket_path = socket_path.clone();
                let request_path = socket_path.clone();
                let request = DaemonRequest {
                    method: "call".into(),
                    name: Some(name.into()),
                    args: Some(arguments),
                    session_id: None,
                    observation_origin: Some(ToolObservationOrigin::Direct),
                    client_kind: Some(self.client_kind),
                };
                let response =
                    tokio::task::spawn_blocking(move || send_request(&request_path, &request))
                        .await
                        .map_err(|error| DriverError::Protocol {
                            reason: format!("{name} transport task failed: {error}"),
                        })?
                        .map_err(|error| DriverError::Transport {
                            socket_path,
                            reason: error.to_string(),
                        })?;
                if !response.ok {
                    return Err(DriverError::Tool {
                        tool: name.into(),
                        message: response
                            .error
                            .unwrap_or_else(|| "daemon rejected the request".into()),
                        error_code: response
                            .exit_code
                            .map(|code| code.to_string())
                            .unwrap_or_default(),
                    });
                }
                response.result.ok_or_else(|| DriverError::Protocol {
                    reason: format!("{name} response omitted result"),
                })?
            }
        };
        normalize_result(raw)
    }
}

impl ToolResult {
    fn typed_success<T: serde::de::DeserializeOwned>(self, tool: &str) -> Result<T, DriverError> {
        if self.is_error {
            return Err(DriverError::Tool {
                tool: tool.into(),
                message: self.text,
                error_code: self.error_code.unwrap_or_default(),
            });
        }
        let structured = self.structured_json.ok_or_else(|| DriverError::Protocol {
            reason: format!("{tool} response omitted structuredContent"),
        })?;
        serde_json::from_str(&structured).map_err(|error| DriverError::Protocol {
            reason: format!("{tool} returned an invalid typed result: {error}"),
        })
    }
}

fn parse_arguments(tool: &str, arguments_json: &str) -> Result<Value, DriverError> {
    let value: Value =
        serde_json::from_str(arguments_json).map_err(|error| DriverError::InvalidArguments {
            tool: tool.into(),
            reason: error.to_string(),
        })?;
    if !value.is_object() {
        return Err(DriverError::InvalidArguments {
            tool: tool.into(),
            reason: "arguments must be a JSON object".into(),
        });
    }
    Ok(value)
}

fn normalize_result(raw: Value) -> Result<ToolResult, DriverError> {
    let object = raw.as_object().ok_or_else(|| DriverError::Protocol {
        reason: "tool result must be a JSON object".into(),
    })?;
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    if let Some(content) = object.get("content").and_then(Value::as_array) {
        for item in content.iter().filter_map(Value::as_object) {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        text_parts.push(text.to_owned());
                    }
                }
                Some("image") => {
                    if let (Some(mime_type), Some(data_base64)) = (
                        item.get("mimeType").and_then(Value::as_str),
                        item.get("data").and_then(Value::as_str),
                    ) {
                        images.push(ImageContent {
                            mime_type: mime_type.into(),
                            data_base64: data_base64.into(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    let structured = object
        .get("structuredContent")
        .filter(|value| value.is_object());
    let error_code = structured.and_then(|value| {
        value
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| value.get("refusal")?.get("code")?.as_str())
            .map(str::to_owned)
    });
    let verified = structured
        .and_then(|value| value.get("verified"))
        .and_then(Value::as_bool);
    let degraded = structured
        .and_then(|value| value.get("degraded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(ToolResult {
        text: text_parts.join("\n"),
        images,
        structured_json: structured.map(Value::to_string),
        is_error: object
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        error_code,
        verified,
        degraded,
        raw_json: raw.to_string(),
    })
}

uniffi::setup_scaffolding!("cua_driver_sdk");

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn exported_typed_methods_cover_every_published_contract() {
        let mut expected = cua_driver_contract::manifest()
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        expected.sort();
        let mut exported = EXPORTED_TOOL_NAMES.to_vec();
        exported.sort_unstable();
        assert_eq!(exported, expected);
    }

    fn serve_once(response: Value) -> (tempfile::TempDir, String, std::thread::JoinHandle<Value>) {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("driver.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let mut writer = stream;
            writeln!(writer, "{}", response).unwrap();
            request
        });
        (directory, socket.to_string_lossy().into_owned(), handle)
    }

    #[tokio::test]
    async fn embedded_runtime_owns_tools_without_daemon_ipc_and_shuts_down_idempotently() {
        let driver = CuaDriver::create(None).unwrap();
        assert_eq!(driver.execution_mode(), DriverExecutionMode::Embedded);
        assert!(driver.socket_path().is_empty());
        assert!(driver.is_available());

        let metadata = driver.metadata().await.unwrap();
        assert!(metadata.embedded);
        assert_eq!(metadata.pid, std::process::id());
        let listed: Value = serde_json::from_str(&driver.list_tools_json().await.unwrap()).unwrap();
        let names = listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for published in EXPORTED_TOOL_NAMES {
            assert!(names.contains(published), "missing {published}");
        }

        driver.shutdown().await.unwrap();
        driver.shutdown().await.unwrap();
        assert!(!driver.is_available());
        assert!(matches!(
            driver.list_tools_json().await,
            Err(DriverError::Shutdown)
        ));
    }

    #[tokio::test]
    async fn typed_desktop_call_serializes_contract_and_normalizes_result() {
        let response = serde_json::json!({
            "ok": true,
            "result": {
                "content": [
                    {"type": "text", "text": "captured"},
                    {"type": "image", "mimeType": "image/png", "data": "cG5n"}
                ],
                "structuredContent": {"screenshot_width": 2, "verified": true},
                "isError": false
            }
        });
        let (_directory, socket, server) = serve_once(response);
        let driver =
            CuaDriver::connect_with_client_kind(Some(socket), SdkClientKind::Python).unwrap();
        let result = driver
            .get_desktop_state(GetDesktopStateInput {
                session: Some("run-1".into()),
                screenshot_out_file: None,
            })
            .await
            .unwrap();
        assert_eq!(result.text, "captured");
        assert_eq!(result.images[0].mime_type, "image/png");
        assert_eq!(result.verified, Some(true));

        let request = server.join().unwrap();
        assert_eq!(request["method"], "call");
        assert_eq!(request["name"], "get_desktop_state");
        assert_eq!(request["args"], serde_json::json!({"session": "run-1"}));
        assert_eq!(request["observation_origin"], "direct");
        assert_eq!(request["client_kind"], "python_sdk");
    }

    #[tokio::test]
    async fn tool_discovery_uses_the_shared_direct_daemon_protocol() {
        let response = serde_json::json!({
            "ok": true,
            "result": [{"name": "get_desktop_state"}]
        });
        let (_directory, socket, server) = serve_once(response);
        let driver = CuaDriver::connect(Some(socket)).unwrap();
        let tools: Value = serde_json::from_str(&driver.list_tools_json().await.unwrap()).unwrap();
        assert_eq!(tools[0]["name"], "get_desktop_state");

        let request = server.join().unwrap();
        assert_eq!(request["method"], "list");
        assert_eq!(request["observation_origin"], "direct");
        assert_eq!(request["client_kind"], "unknown");
    }

    #[tokio::test]
    async fn session_method_returns_the_canonical_typed_output() {
        let response = serde_json::json!({
            "ok": true,
            "result": {
                "content": [{"type": "text", "text": "started"}],
                "structuredContent": {
                    "session": "run-2",
                    "capture_scope": "auto",
                    "effective_scope": "window",
                    "desktop_unlocked": false,
                    "escalation_reason": null,
                    "escalation_detail": null,
                    "active": true,
                    "revived": false
                },
                "isError": false
            }
        });
        let (_directory, socket, server) = serve_once(response);
        let driver = CuaDriver::connect(Some(socket)).unwrap();
        let output = driver
            .start_session(StartSessionInput {
                session: "run-2".into(),
                capture_scope: Some(cua_driver_contract::CaptureScope::Auto),
            })
            .await
            .unwrap();
        assert!(output.active);
        assert_eq!(output.state.session, "run-2");
        server.join().unwrap();
    }
}
