//! In-process embedding API for cua-driver.
//!
//! This crate builds the same platform tool registry as the standalone
//! `cua-driver mcp` server, but leaves transport and process ownership to the
//! host application. On macOS that means Accessibility and Screen Recording
//! permissions are attributed to the embedding application, not a separate
//! `CuaDriver.app` helper or shell-spawned executable.
//!
//! The embedded API does not start the daemon proxy, read from stdio, or run the
//! cursor overlay AppKit loop. Hosts can call tools directly or pass JSON-RPC
//! MCP requests through an in-process transport.

use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::Arc,
};

pub use cua_driver_core::{
    protocol::{Content, Request, Response, ToolResult},
    tool::{ToolDef, ToolRegistry},
};
use serde_json::Value;

/// Options used when building an embedded driver registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmbeddedOptions {
    /// Register the Claude Code computer-use compatible screenshot variant.
    ///
    /// This mirrors `cua-driver mcp --claude-code-computer-use-compat`.
    pub claude_code_compat: bool,
}

/// In-process cua-driver runtime.
#[derive(Clone)]
pub struct EmbeddedDriver {
    registry: Arc<ToolRegistry>,
}

impl EmbeddedDriver {
    /// Build a driver using default options.
    pub fn new() -> Self {
        Self::with_options(EmbeddedOptions::default())
    }

    /// Build a driver using explicit options.
    pub fn with_options(options: EmbeddedOptions) -> Self {
        let registry = Arc::new(build_registry(options));
        registry.init_self_weak();
        Self { registry }
    }

    /// Return the underlying tool registry.
    ///
    /// This is useful for hosts that want to reuse cua-driver-core helpers or
    /// expose a custom transport around the same registered tool set.
    pub fn registry(&self) -> Arc<ToolRegistry> {
        self.registry.clone()
    }

    /// Return the MCP `tools/list` result payload.
    pub fn tools_list(&self) -> Value {
        self.registry.tools_list()
    }

    /// Invoke a tool directly without JSON-RPC framing.
    pub async fn call_tool(&self, name: impl AsRef<str>, arguments: Value) -> ToolResult {
        self.registry.invoke(name.as_ref(), arguments).await
    }

    /// Handle one parsed MCP JSON-RPC request.
    ///
    /// Returns `None` for notifications, exactly like the stdio MCP server.
    pub async fn handle_mcp_request(&self, request: Request) -> Option<Response> {
        cua_driver_core::server::handle_request(request, &self.registry).await
    }

    /// Handle one JSON value containing an MCP JSON-RPC request.
    ///
    /// Malformed request shapes return the same parse-error response as the
    /// stdio server currently emits for invalid input.
    pub async fn handle_mcp_request_value(&self, request: Value) -> Option<Value> {
        let response = match serde_json::from_value::<Request>(request) {
            Ok(req) => self.handle_mcp_request(req).await?,
            Err(_) => Response::parse_error(),
        };
        Some(response_to_value(response))
    }

    /// Handle one JSON-encoded MCP request line.
    ///
    /// Returns `None` for notifications. Malformed JSON is converted to a
    /// JSON-RPC parse-error response string.
    pub async fn handle_mcp_request_json(&self, request: &str) -> Option<String> {
        let response = match serde_json::from_str::<Request>(request) {
            Ok(req) => self.handle_mcp_request(req).await?,
            Err(_) => Response::parse_error(),
        };
        Some(response_to_string(response))
    }
}

impl Default for EmbeddedDriver {
    fn default() -> Self {
        Self::new()
    }
}

fn response_to_value(response: Response) -> Value {
    serde_json::to_value(response).unwrap_or_else(|e| {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {
                "code": -32603,
                "message": format!("serialize error: {e}"),
            }
        })
    })
}

fn response_to_string(response: Response) -> String {
    serde_json::to_string(&response).unwrap_or_else(|e| {
        format!(
            r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#
        )
    })
}

/// Opaque C ABI handle for non-Rust hosts.
///
/// Each handle owns a Tokio runtime so C, Swift, Objective-C, and other hosts
/// can use a blocking JSON-in/JSON-out MCP bridge without taking a Rust async
/// dependency at the language boundary.
pub struct CuaDriver {
    driver: EmbeddedDriver,
    runtime: tokio::runtime::Runtime,
}

/// Create an embedded driver for use through the C ABI.
///
/// Returns null if the Tokio runtime cannot be created.
#[no_mangle]
pub extern "C" fn cua_driver_embedded_new(claude_code_compat: bool) -> *mut CuaDriver {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .ok()?;
        let driver = EmbeddedDriver::with_options(EmbeddedOptions { claude_code_compat });
        Some(Box::new(CuaDriver { driver, runtime }))
    }));

    match result {
        Ok(Some(driver)) => Box::into_raw(driver),
        _ => ptr::null_mut(),
    }
}

/// Free an embedded C ABI driver.
#[no_mangle]
pub unsafe extern "C" fn cua_driver_embedded_free(driver: *mut CuaDriver) {
    if !driver.is_null() {
        drop(Box::from_raw(driver));
    }
}

/// Handle one JSON-RPC MCP request encoded as UTF-8 JSON.
///
/// GUI hosts should call this from a worker queue rather than the app main
/// thread, because some macOS tools interact with AppKit or system services.
///
/// Returns null for notifications. Non-null return values must be released
/// with `cua_driver_embedded_string_free`.
#[no_mangle]
pub unsafe extern "C" fn cua_driver_embedded_handle_mcp_json(
    driver: *mut CuaDriver,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if driver.is_null() {
            return Some(json_rpc_error_string(-32603, "driver handle is null"));
        }
        if request_json.is_null() {
            return Some(json_rpc_error_string(-32600, "request_json is null"));
        }

        let request = match CStr::from_ptr(request_json).to_str() {
            Ok(value) => value,
            Err(_) => return Some(json_rpc_error_string(-32700, "request_json is not UTF-8")),
        };

        let handle = &*driver;
        handle
            .runtime
            .block_on(handle.driver.handle_mcp_request_json(request))
    }));

    match result {
        Ok(Some(response)) => string_to_c(response),
        Ok(None) => ptr::null_mut(),
        Err(_) => string_to_c(json_rpc_error_string(
            -32603,
            "panic while handling embedded MCP request",
        )),
    }
}

/// Free a string returned by `cua_driver_embedded_handle_mcp_json`.
#[no_mangle]
pub unsafe extern "C" fn cua_driver_embedded_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}

fn string_to_c(value: String) -> *mut c_char {
    match CString::new(value) {
        Ok(value) => value.into_raw(),
        Err(_) => string_to_c(json_rpc_error_string(
            -32603,
            "response contained an interior nul byte",
        )),
    }
}

fn json_rpc_error_string(code: i64, message: &str) -> String {
    response_to_string(Response::error(serde_json::Value::Null, code, message))
}

#[cfg(target_os = "macos")]
fn build_registry(options: EmbeddedOptions) -> ToolRegistry {
    register_recording_hooks();
    platform_macos::register_tools_with_compat(options.claude_code_compat)
}

#[cfg(target_os = "windows")]
fn build_registry(options: EmbeddedOptions) -> ToolRegistry {
    register_recording_hooks();
    platform_windows::register_tools_with_cursor(
        cursor_overlay::CursorConfig {
            enabled: false,
            ..Default::default()
        },
        options.claude_code_compat,
    )
}

#[cfg(target_os = "linux")]
fn build_registry(options: EmbeddedOptions) -> ToolRegistry {
    register_recording_hooks();
    platform_linux::register_tools_with_cursor(
        cursor_overlay::CursorConfig {
            enabled: false,
            ..Default::default()
        },
        options.claude_code_compat,
    )
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn build_registry(options: EmbeddedOptions) -> ToolRegistry {
    let _ = options;
    ToolRegistry::new()
}

#[cfg(target_os = "macos")]
fn register_recording_hooks() {
    cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
        if let Some(wid) = window_id {
            platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
        } else if let Some(p) = pid {
            platform_macos::windows::resolve_main_window_id(p as i32)
                .ok()
                .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
        } else {
            platform_macos::capture::screenshot_display_bytes().ok()
        }
    });
    cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
        platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
    });
    cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
        platform_macos::recording_hooks::app_state_json_for(window_id, pid)
    });
    cua_driver_core::recording::set_element_bounds_fn(|wid, pid, idx| {
        platform_macos::recording_hooks::element_window_local_xy(wid, pid, idx)
    });
}

#[cfg(target_os = "windows")]
fn register_recording_hooks() {
    cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
        if let Some(hwnd) = window_id {
            platform_windows::capture::screenshot_window_bytes(hwnd).ok()
        } else if let Some(p) = pid {
            let wins = platform_windows::win32::list_windows(Some(p as u32));
            wins.first()
                .and_then(|w| platform_windows::capture::screenshot_window_bytes(w.hwnd).ok())
        } else {
            platform_windows::capture::screenshot_display_bytes().ok()
        }
    });
    cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
        platform_windows::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
    });
    cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
        platform_windows::recording_hooks::app_state_json_for(window_id, pid)
    });
    cua_driver_core::recording::set_element_bounds_fn(|wid, pid, idx| {
        platform_windows::recording_hooks::element_window_local_xy(wid, pid, idx)
    });
}

#[cfg(target_os = "linux")]
fn register_recording_hooks() {
    cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
        if let Some(xid) = window_id {
            platform_linux::capture::screenshot_window_bytes(xid).ok()
        } else if let Some(p) = pid {
            let wins = platform_linux::x11::list_windows(Some(p as u32));
            wins.first()
                .and_then(|w| platform_linux::capture::screenshot_window_bytes(w.xid).ok())
        } else {
            platform_linux::capture::screenshot_display_bytes().ok()
        }
    });
    cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
        platform_linux::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    use serde_json::json;

    #[tokio::test]
    async fn handles_initialize_request() {
        let driver = EmbeddedDriver::new();
        let response = driver
            .handle_mcp_request_value(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            }))
            .await
            .expect("request returns a response");

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["serverInfo"]["name"], "cua-driver");
    }

    #[tokio::test]
    async fn drops_notifications() {
        let driver = EmbeddedDriver::new();
        let response = driver
            .handle_mcp_request_value(json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .await;

        assert!(response.is_none());
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let driver = EmbeddedDriver::new();
        let response = driver
            .handle_mcp_request_json("{not-json")
            .await
            .expect("parse errors return a response");
        let response: Value = serde_json::from_str(&response).expect("valid response json");

        assert_eq!(response["error"]["code"], -32700);
    }

    #[test]
    fn ffi_handles_initialize_json() {
        let driver = cua_driver_embedded_new(false);
        assert!(!driver.is_null());

        let request = CString::new(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).unwrap();
        let response = unsafe { cua_driver_embedded_handle_mcp_json(driver, request.as_ptr()) };
        assert!(!response.is_null());

        let response_json = unsafe { CStr::from_ptr(response) }.to_str().unwrap();
        let response_value: Value = serde_json::from_str(response_json).unwrap();
        assert_eq!(response_value["result"]["serverInfo"]["name"], "cua-driver");

        unsafe {
            cua_driver_embedded_string_free(response);
            cua_driver_embedded_free(driver);
        }
    }

    #[test]
    fn ffi_returns_null_for_notifications() {
        let driver = cua_driver_embedded_new(false);
        assert!(!driver.is_null());

        let request =
            CString::new(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        let response = unsafe { cua_driver_embedded_handle_mcp_json(driver, request.as_ptr()) };
        assert!(response.is_null());

        unsafe {
            cua_driver_embedded_free(driver);
        }
    }
}
