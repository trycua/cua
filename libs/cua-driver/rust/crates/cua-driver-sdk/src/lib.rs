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
    is_daemon_listening, send_request, socket_path_for_namespace, DaemonRequest,
    ToolObservationOrigin,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;

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
        Ok(Arc::new(Self { socket_path }))
    }

    pub fn socket_path(&self) -> String {
        self.socket_path.clone()
    }

    pub fn is_available(&self) -> bool {
        is_daemon_listening(&self.socket_path)
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
        let driver = CuaDriver::connect(Some(socket)).unwrap();
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
