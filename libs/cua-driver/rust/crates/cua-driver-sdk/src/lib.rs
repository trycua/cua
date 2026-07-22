//! Canonical Rust implementation for imported Cua Driver SDKs.
//!
//! This library intentionally talks to the existing daemon rather than
//! embedding the platform automation engine. Python and Node bindings therefore
//! share one implementation while retaining the daemon's permission ownership,
//! session state, policy enforcement, and platform event-loop constraints.

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
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;

mod embedded;
pub use embedded::*;

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
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
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
}

#[derive(uniffi::Object)]
pub struct CuaDriver {
    socket_path: String,
    client_kind: DaemonClientKind,
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
        #[uniffi::export]
        impl CuaDriver {
            $(
                pub fn $method(&self, input: $input) -> Result<ToolResult, DriverError> {
                    self.invoke_typed(<$input as ToolInput>::TOOL_NAME, input)
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
    /// Connect to the default installed daemon or an explicitly selected socket.
    /// Construction does not launch a process or perform I/O.
    #[uniffi::constructor]
    pub fn connect(socket_path: Option<String>) -> Result<Arc<Self>, DriverError> {
        let socket_path = socket_path.unwrap_or_else(|| socket_path_for_namespace("cua-driver"));
        if socket_path.trim().is_empty() {
            return Err(DriverError::Configuration {
                reason: "socket_path must not be empty".into(),
            });
        }
        Ok(Arc::new(Self {
            socket_path,
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
        Ok(Arc::new(Self {
            socket_path: driver.socket_path.clone(),
            client_kind: client_kind.into(),
        }))
    }

    pub fn socket_path(&self) -> String {
        self.socket_path.clone()
    }

    pub fn is_available(&self) -> bool {
        is_daemon_listening(&self.socket_path)
    }

    pub fn metadata(&self) -> Result<DriverMetadata, DriverError> {
        let metadata =
            request_daemon_metadata(&self.socket_path).map_err(|error| DriverError::Transport {
                socket_path: self.socket_path.clone(),
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

    /// Escape hatch for forward-compatible tools and application-owned server
    /// adapters. Typed methods below serialize the same canonical Rust inputs.
    pub fn call_tool(
        &self,
        name: String,
        arguments_json: String,
    ) -> Result<ToolResult, DriverError> {
        let arguments = parse_arguments(&name, &arguments_json)?;
        self.invoke(&name, arguments)
    }

    pub fn list_tools_json(&self) -> Result<String, DriverError> {
        let request = DaemonRequest {
            method: "list".into(),
            name: None,
            args: None,
            session_id: None,
            observation_origin: Some(ToolObservationOrigin::Direct),
            client_kind: Some(self.client_kind),
        };
        let response =
            send_request(&self.socket_path, &request).map_err(|error| DriverError::Transport {
                socket_path: self.socket_path.clone(),
                reason: error.to_string(),
            })?;
        if !response.ok {
            return Err(DriverError::Protocol {
                reason: response
                    .error
                    .unwrap_or_else(|| "list request failed".into()),
            });
        }
        serde_json::to_string(&response.result.unwrap_or(Value::Null)).map_err(|error| {
            DriverError::Protocol {
                reason: error.to_string(),
            }
        })
    }

    pub fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<StartSessionOutput, DriverError> {
        self.invoke_typed(StartSessionInput::TOOL_NAME, input)
            .and_then(|result| result.typed_success(StartSessionInput::TOOL_NAME))
    }

    pub fn escalate_session(
        &self,
        input: EscalateSessionInput,
    ) -> Result<SessionStateOutput, DriverError> {
        self.invoke_typed(EscalateSessionInput::TOOL_NAME, input)
            .and_then(|result| result.typed_success(EscalateSessionInput::TOOL_NAME))
    }

    pub fn get_session_state(
        &self,
        input: GetSessionStateInput,
    ) -> Result<SessionStateOutput, DriverError> {
        self.invoke_typed(GetSessionStateInput::TOOL_NAME, input)
            .and_then(|result| result.typed_success(GetSessionStateInput::TOOL_NAME))
    }

    pub fn end_session(&self, input: EndSessionInput) -> Result<EndSessionOutput, DriverError> {
        self.invoke_typed(EndSessionInput::TOOL_NAME, input)
            .and_then(|result| result.typed_success(EndSessionInput::TOOL_NAME))
    }
}

desktop_tool_methods!(define_desktop_tool_methods);

impl CuaDriver {
    fn invoke_typed<T: Serialize>(&self, name: &str, input: T) -> Result<ToolResult, DriverError> {
        let arguments =
            serde_json::to_value(input).map_err(|error| DriverError::InvalidArguments {
                tool: name.into(),
                reason: error.to_string(),
            })?;
        self.invoke(name, arguments)
    }

    fn invoke(&self, name: &str, arguments: Value) -> Result<ToolResult, DriverError> {
        if !arguments.is_object() {
            return Err(DriverError::InvalidArguments {
                tool: name.into(),
                reason: "arguments must be a JSON object".into(),
            });
        }
        let request = DaemonRequest {
            method: "call".into(),
            name: Some(name.into()),
            args: Some(arguments),
            session_id: None,
            observation_origin: Some(ToolObservationOrigin::Direct),
            client_kind: Some(self.client_kind),
        };
        let response =
            send_request(&self.socket_path, &request).map_err(|error| DriverError::Transport {
                socket_path: self.socket_path.clone(),
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
        normalize_result(response.result.ok_or_else(|| DriverError::Protocol {
            reason: format!("{name} response omitted result"),
        })?)
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

    #[test]
    fn typed_desktop_call_serializes_contract_and_normalizes_result() {
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

    #[test]
    fn tool_discovery_uses_the_shared_direct_daemon_protocol() {
        let response = serde_json::json!({
            "ok": true,
            "result": [{"name": "get_desktop_state"}]
        });
        let (_directory, socket, server) = serve_once(response);
        let driver = CuaDriver::connect(Some(socket)).unwrap();
        let tools: Value = serde_json::from_str(&driver.list_tools_json().unwrap()).unwrap();
        assert_eq!(tools[0]["name"], "get_desktop_state");

        let request = server.join().unwrap();
        assert_eq!(request["method"], "list");
        assert_eq!(request["observation_origin"], "direct");
        assert_eq!(request["client_kind"], "unknown");
    }

    #[test]
    fn session_method_returns_the_canonical_typed_output() {
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
            .unwrap();
        assert!(output.active);
        assert_eq!(output.state.session, "run-2");
        server.join().unwrap();
    }
}
