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
pub mod remote;
mod runtime;
mod service_session;
#[doc(hidden)]
pub mod worker;
use abi::{NativeAbiDriver, NativeAbiSession};
pub use embedded::*;
use remote::{DriverEnvelopeChannel, RemoteBoundSession, RemoteDriverClient};
use runtime::RuntimeOptions;
use service_session::ServiceSessionClient;
use worker::{ActionCompletion, PrivateWorkerClient};

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
    #[error(
        "runtime_already_exists: one direct Cua Driver runtime is already active in this process"
    )]
    RuntimeAlreadyExists,
    #[error("private worker error: {reason}")]
    Worker { reason: String },
    #[error("remote Driver error: {reason}")]
    Remote { reason: String },
    #[error("driver action was interrupted ({completion:?}): {reason}")]
    ActionInterrupted {
        completion: ActionCompletion,
        reason: String,
    },
}

#[derive(uniffi::Object)]
pub struct CuaDriver {
    backend: DriverBackend,
    client_kind: DaemonClientKind,
}

enum DriverBackend {
    Embedded(Arc<NativeAbiDriver>),
    Daemon { socket_path: String },
    PrivateWorker(Arc<PrivateWorkerClient>),
    Remote(Arc<RemoteDriverClient>),
}

/// Process topology used by this SDK object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DriverExecutionMode {
    Embedded,
    Daemon,
    PrivateWorker,
    Remote,
}

/// Options for a same-process Cua Driver SDK runtime.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct DriverOptions {
    /// Preserve the temporary reduced screenshot surface used by older Claude
    /// Code integrations. New applications should leave this false.
    pub claude_code_compatibility: bool,
}

/// Permission mode chosen by trusted host code for a runtime or session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum SessionPermissionMode {
    Standard,
    Bounded,
    Unrestricted,
}

impl SessionPermissionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Bounded => "bounded",
            Self::Unrestricted => "unrestricted",
        }
    }
}

/// Immutable authorization ceiling supplied before a runtime accepts actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct RuntimeAuthorizationOptions {
    pub allowed_modes: Vec<SessionPermissionMode>,
    /// Mode inherited by calls made through the released `CuaDriver` object
    /// rather than a trusted session-bound action surface.
    pub compatibility_mode: SessionPermissionMode,
    /// Required only when `compatibility_mode` is bounded.
    pub compatibility_bounded_manifest_path: Option<String>,
    pub unrestricted_acknowledged: bool,
    pub max_session_ttl_seconds: u64,
    pub max_idle_ttl_seconds: u64,
}

/// Additive configured-runtime constructor options. Existing callers continue
/// to use [`DriverOptions`] and inherit the compatibility session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ConfiguredDriverOptions {
    pub claude_code_compatibility: bool,
    pub authorization: RuntimeAuthorizationOptions,
}

/// Trusted host request for one immutable, connection-bound action surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TrustedSessionOptions {
    pub public_session: String,
    pub mode: SessionPermissionMode,
    pub ttl_seconds: u64,
    pub idle_ttl_seconds: u64,
    pub bounded_manifest_path: Option<String>,
}

#[derive(uniffi::Object)]
pub struct CuaDriverSession {
    backend: SessionBackend,
}

enum SessionBackend {
    Embedded(Arc<NativeAbiSession>),
    PrivateWorker {
        client: Arc<PrivateWorkerClient>,
        session_handle: String,
        closed: std::sync::atomic::AtomicBool,
    },
    Service {
        client: Arc<ServiceSessionClient>,
        closed: std::sync::atomic::AtomicBool,
    },
    Remote(Arc<RemoteBoundSession>),
}

impl SessionBackend {
    async fn close_async(&self) {
        match self {
            Self::Embedded(session) => session.close(),
            Self::PrivateWorker {
                client,
                session_handle,
                closed,
            } => {
                if !closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    let client = client.clone();
                    let session_handle = session_handle.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        client.close_session(&session_handle);
                    })
                    .await;
                }
            }
            Self::Service { client, closed } => {
                if !closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    let client = client.clone();
                    let _ = tokio::task::spawn_blocking(move || client.close()).await;
                }
            }
            Self::Remote(session) => session.close(),
        }
    }

    fn close(&self) {
        match self {
            Self::Embedded(session) => session.close(),
            Self::PrivateWorker {
                client,
                session_handle,
                closed,
            } => {
                if !closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    client.close_session(session_handle);
                }
            }
            Self::Service { client, closed } => {
                if !closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    client.close();
                }
            }
            Self::Remote(session) => session.close(),
        }
    }

    fn abandon(&self) {
        match self {
            Self::Embedded(session) => session.close(),
            Self::PrivateWorker {
                client,
                session_handle,
                closed,
            } => {
                if !closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    let client = client.clone();
                    let session_handle = session_handle.clone();
                    std::thread::spawn(move || client.close_session(&session_handle));
                }
            }
            Self::Service { client, closed } => {
                if !closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    client.abandon();
                }
            }
            Self::Remote(session) => session.close(),
        }
    }
}

impl Drop for CuaDriverSession {
    fn drop(&mut self) {
        // Language finalizers may run on arbitrary threads. They must revoke
        // local authority without waiting on a child, socket, or network peer.
        self.backend.abandon();
    }
}

/// Options for a directly supervised process-isolated runtime.
///
/// The child receives configuration and actions only over inherited stdio. It
/// exposes no socket and cannot be reconnected after the host channel closes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PrivateWorkerOptions {
    pub binary_path: String,
    pub host_bundle_id: String,
    pub startup_timeout_ms: Option<u64>,
    pub shutdown_timeout_ms: Option<u64>,
    pub configured_driver: ConfiguredDriverOptions,
    pub environment: Vec<EmbeddedEnvironmentVariable>,
    pub inherit_stderr: bool,
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

macro_rules! define_session_desktop_tool_methods {
    ($($method:ident: $input:ty,)*) => {
        #[uniffi::export(async_runtime = "tokio")]
        impl CuaDriverSession {
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
        cua_driver_core::authorization::validate_startup_authorization().map_err(|error| {
            DriverError::Configuration {
                reason: format!("authorization configuration is invalid: {error}"),
            }
        })?;
        let options = options.unwrap_or_default();
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(NativeAbiDriver::create(
                options.claude_code_compatibility,
            )?)),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Create a same-process runtime with an explicit immutable authorization
    /// ceiling. This is a trusted host constructor and is not exposed as an
    /// agent tool. Existing `create()` callers remain unchanged.
    #[uniffi::constructor]
    pub fn create_configured(options: ConfiguredDriverOptions) -> Result<Arc<Self>, DriverError> {
        let allowed_modes = options
            .authorization
            .allowed_modes
            .iter()
            .map(|mode| mode.as_str())
            .collect::<Vec<_>>();
        let native_options = serde_json::json!({
            "claude_code_compatibility": options.claude_code_compatibility,
            "authorization": {
                "allowed_modes": allowed_modes,
                "compatibility_mode": options.authorization.compatibility_mode.as_str(),
                "compatibility_bounded_manifest_path": options.authorization.compatibility_bounded_manifest_path,
                "unrestricted_acknowledged": options.authorization.unrestricted_acknowledged,
                "max_session_ttl_seconds": options.authorization.max_session_ttl_seconds,
                "max_idle_ttl_seconds": options.authorization.max_idle_ttl_seconds,
            }
        });
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(NativeAbiDriver::create_configured(
                native_options,
            )?)),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Language-package entry point for an explicitly configured runtime.
    #[uniffi::constructor]
    pub fn create_configured_with_client_kind(
        options: ConfiguredDriverOptions,
        client_kind: SdkClientKind,
    ) -> Result<Arc<Self>, DriverError> {
        let driver = Self::create_configured(options)?;
        let DriverBackend::Embedded(runtime) = &driver.backend else {
            unreachable!("create_configured always returns an embedded runtime")
        };
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(runtime.clone()),
            client_kind: client_kind.into(),
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

    /// Create a process-isolated runtime owned by this SDK object.
    ///
    /// This constructor directly spawns the supplied Cua Driver binary and
    /// communicates only over inherited stdio. No daemon or reusable endpoint
    /// is created.
    #[uniffi::constructor]
    pub fn create_private_worker(options: PrivateWorkerOptions) -> Result<Arc<Self>, DriverError> {
        create_private_worker_for_client(options, DaemonClientKind::Unknown)
    }

    /// Language-package entry point that preserves the worker constructor
    /// while attaching the importing SDK runtime category.
    #[uniffi::constructor]
    pub fn create_private_worker_with_client_kind(
        options: PrivateWorkerOptions,
        client_kind: SdkClientKind,
    ) -> Result<Arc<Self>, DriverError> {
        create_private_worker_for_client(options, client_kind.into())
    }

    pub fn execution_mode(&self) -> DriverExecutionMode {
        match &self.backend {
            DriverBackend::Embedded(_) => DriverExecutionMode::Embedded,
            DriverBackend::Daemon { .. } => DriverExecutionMode::Daemon,
            DriverBackend::PrivateWorker(_) => DriverExecutionMode::PrivateWorker,
            DriverBackend::Remote(_) => DriverExecutionMode::Remote,
        }
    }

    /// Compatibility accessor. Embedded runtimes have no socket and return an
    /// empty string; new code should branch on [`Self::execution_mode`].
    pub fn socket_path(&self) -> String {
        match &self.backend {
            DriverBackend::Embedded(_) => String::new(),
            DriverBackend::Daemon { socket_path } => socket_path.clone(),
            DriverBackend::PrivateWorker(_) => String::new(),
            DriverBackend::Remote(_) => String::new(),
        }
    }

    pub fn is_available(&self) -> bool {
        match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.is_available(),
            DriverBackend::Daemon { socket_path } => is_daemon_listening(socket_path),
            DriverBackend::PrivateWorker(worker) => worker.is_available(),
            DriverBackend::Remote(remote) => remote.is_available(),
        }
    }
}

fn create_private_worker_for_client(
    options: PrivateWorkerOptions,
    client_kind: DaemonClientKind,
) -> Result<Arc<CuaDriver>, DriverError> {
    let validated = worker::validate_worker_options(
        options.binary_path,
        options.host_bundle_id,
        options.startup_timeout_ms,
        options.shutdown_timeout_ms,
        options.configured_driver,
        options.environment,
        options.inherit_stderr,
    )?;
    Ok(Arc::new(CuaDriver {
        backend: DriverBackend::PrivateWorker(PrivateWorkerClient::spawn(validated)?),
        client_kind,
    }))
}

impl CuaDriver {
    /// Rust-only constructor for a transport adapter that exchanges generated
    /// Driver envelopes over an authenticated asynchronous channel.
    pub fn connect_remote(
        channel: Arc<dyn DriverEnvelopeChannel>,
    ) -> Result<Arc<Self>, DriverError> {
        Ok(Arc::new(Self {
            backend: DriverBackend::Remote(RemoteDriverClient::connect(channel)?),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Rust host API for an action surface already bound to one trusted
    /// immutable authorization context.
    pub fn create_trusted_session(
        &self,
        options: TrustedSessionOptions,
    ) -> Result<Arc<CuaDriverSession>, DriverError> {
        match &self.backend {
            DriverBackend::Embedded(runtime) => {
                let native_options = serde_json::json!({
                    "public_session": options.public_session,
                    "mode": options.mode.as_str(),
                    "ttl_seconds": options.ttl_seconds,
                    "idle_ttl_seconds": options.idle_ttl_seconds,
                    "bounded_manifest_path": options.bounded_manifest_path,
                });
                Ok(Arc::new(CuaDriverSession {
                    backend: SessionBackend::Embedded(Arc::new(
                        runtime.create_session(native_options)?,
                    )),
                }))
            }
            DriverBackend::PrivateWorker(worker) => {
                let session_handle = worker.bind_session(options)?;
                Ok(Arc::new(CuaDriverSession {
                    backend: SessionBackend::PrivateWorker {
                        client: worker.clone(),
                        session_handle,
                        closed: std::sync::atomic::AtomicBool::new(false),
                    },
                }))
            }
            DriverBackend::Daemon { socket_path } => {
                let client = ServiceSessionClient::connect_and_bind(
                    socket_path.clone(),
                    options,
                    self.client_kind,
                )?;
                Ok(Arc::new(CuaDriverSession {
                    backend: SessionBackend::Service {
                        client,
                        closed: std::sync::atomic::AtomicBool::new(false),
                    },
                }))
            }
            DriverBackend::Remote(_) => Err(DriverError::Configuration {
                reason:
                    "remote trusted sessions require create_remote_trusted_session so the authenticated carrier can bind a new logical channel"
                        .into(),
            }),
        }
    }

    /// Bind a trusted session through an authenticated remote carrier without
    /// exposing serialized authority to the caller.
    pub async fn create_remote_trusted_session(
        &self,
        options: TrustedSessionOptions,
    ) -> Result<Arc<CuaDriverSession>, DriverError> {
        let DriverBackend::Remote(remote) = &self.backend else {
            return Err(DriverError::Configuration {
                reason: "create_remote_trusted_session requires a remote Driver backend".into(),
            });
        };
        Ok(Arc::new(CuaDriverSession {
            backend: SessionBackend::Remote(remote.bind_session(options).await?),
        }))
    }

    /// Construct the same SDK-owned runtime for the standalone daemon host.
    /// This Rust-only entry point keeps CLI presentation options out of the
    /// generated language contract.
    pub fn create_for_host(options: DriverHostOptions) -> Arc<Self> {
        Self::try_create_for_host(options).expect(
            "standalone host attempted to create a second Cua Driver runtime in one process",
        )
    }

    /// Fallible host constructor used by adapters that must surface lifecycle
    /// conflicts instead of panicking. The compatibility wrapper above is
    /// retained for existing Rust callers.
    pub fn try_create_for_host(options: DriverHostOptions) -> Result<Arc<Self>, DriverError> {
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(NativeAbiDriver::create_for_host(
                RuntimeOptions {
                    cursor: options.cursor,
                    compatibility_mode: options.claude_code_compatibility,
                    prepare_desktop_environment: options.prepare_desktop_environment,
                    register_host_tools: options.register_host_tools,
                    authorization_ceiling: None,
                    compatibility_authorization: None,
                },
            )?)),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Construct a runtime for a service adapter that can bind trusted
    /// sessions after authenticating an accepted connection.
    ///
    /// The service's existing process mode remains the compatibility mode and
    /// the only delegated mode. Mixed ceilings require the explicit configured
    /// constructor and are never inferred from a caller request.
    pub fn try_create_service_for_host(
        options: DriverHostOptions,
    ) -> Result<Arc<Self>, DriverError> {
        let mode =
            cua_driver_core::authorization::configured_permission_mode().map_err(|error| {
                DriverError::Configuration {
                    reason: format!("authorization configuration is invalid: {error}"),
                }
            })?;
        let manifest = if mode == cua_driver_core::authorization::PermissionMode::Bounded {
            cua_driver_core::session_manifest::configured_session_manifest()
                .map_err(|error| DriverError::Configuration { reason: error })?
                .cloned()
                .map(Arc::new)
        } else {
            None
        };
        let ceiling =
            cua_driver_core::session_authorization::SessionModeCeiling::for_trusted_sessions(
                [mode],
                mode == cua_driver_core::authorization::PermissionMode::Unrestricted,
                std::time::Duration::from_secs(24 * 60 * 60),
                std::time::Duration::from_secs(60 * 60),
            )
            .map_err(|reason| DriverError::Configuration { reason })?;
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(NativeAbiDriver::create_for_host(
                RuntimeOptions {
                    cursor: options.cursor,
                    compatibility_mode: options.claude_code_compatibility,
                    prepare_desktop_environment: options.prepare_desktop_environment,
                    register_host_tools: options.register_host_tools,
                    authorization_ceiling: Some(ceiling),
                    compatibility_authorization: Some((mode, manifest)),
                },
            )?)),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Construct an explicitly authorized runtime with native host facilities
    /// such as the cursor overlay. Directly supervised private workers use
    /// this Rust-only entry point; generated-language callers cannot inject
    /// host callbacks.
    pub fn try_create_configured_for_host(
        options: ConfiguredDriverOptions,
        host: DriverHostOptions,
    ) -> Result<Arc<Self>, DriverError> {
        let allowed_modes = options
            .authorization
            .allowed_modes
            .iter()
            .map(|mode| mode.as_str())
            .collect::<Vec<_>>();
        let native_options = serde_json::json!({
            "claude_code_compatibility": options.claude_code_compatibility,
            "authorization": {
                "allowed_modes": allowed_modes,
                "compatibility_mode": options.authorization.compatibility_mode.as_str(),
                "compatibility_bounded_manifest_path": options.authorization.compatibility_bounded_manifest_path,
                "unrestricted_acknowledged": options.authorization.unrestricted_acknowledged,
                "max_session_ttl_seconds": options.authorization.max_session_ttl_seconds,
                "max_idle_ttl_seconds": options.authorization.max_idle_ttl_seconds,
            }
        });
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(
                NativeAbiDriver::create_configured_for_host(
                    native_options,
                    host.cursor,
                    host.prepare_desktop_environment,
                    host.register_host_tools,
                )?,
            )),
            client_kind: DaemonClientKind::Unknown,
        }))
    }
}

/// Generated-language host factory for a session-bound action surface.
///
/// This remains a separate top-level capability instead of adding a required
/// method to the released `CuaDriverProtocol` / `CuaDriverLike` structural
/// interfaces.
#[uniffi::export]
pub fn create_trusted_session(
    driver: Arc<CuaDriver>,
    options: TrustedSessionOptions,
) -> Result<Arc<CuaDriverSession>, DriverError> {
    driver.create_trusted_session(options)
}

#[uniffi::export(async_runtime = "tokio")]
impl CuaDriver {
    pub async fn metadata(&self) -> Result<DriverMetadata, DriverError> {
        match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.metadata(),
            DriverBackend::PrivateWorker(worker) => worker.metadata().await,
            DriverBackend::Remote(remote) => remote.metadata().await,
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

    /// Generic protocol-adapter surface. Ordinary applications should prefer
    /// typed methods; MCP and other open-ended adapters use this method so they
    /// remain downstream of the same public SDK runtime.
    pub async fn call_tool(
        &self,
        name: String,
        arguments_json: String,
    ) -> Result<ToolResult, DriverError> {
        let mut arguments = parse_arguments(&name, &arguments_json)?;
        cua_driver_core::tool_args::sanitize_reserved_args(&mut arguments);
        self.invoke(&name, arguments).await
    }

    /// Canonical tool inventory for MCP and other protocol adapters.
    pub async fn list_tools_json(&self) -> Result<String, DriverError> {
        let result = match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.tools_list()?,
            DriverBackend::PrivateWorker(worker) => worker.list_tools().await?,
            DriverBackend::Remote(remote) => remote.list_tools().await?,
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
        match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.shutdown().await?,
            DriverBackend::PrivateWorker(worker) => worker.shutdown().await?,
            DriverBackend::Remote(remote) => remote.shutdown().await?,
            DriverBackend::Daemon { .. } => {}
        }
        Ok(())
    }
}

desktop_tool_methods!(define_desktop_tool_methods);
desktop_tool_methods!(define_session_desktop_tool_methods);

#[uniffi::export(async_runtime = "tokio")]
impl CuaDriverSession {
    pub async fn call_tool(
        &self,
        name: String,
        arguments_json: String,
    ) -> Result<ToolResult, DriverError> {
        let arguments = parse_arguments(&name, &arguments_json)?;
        self.invoke(&name, arguments).await
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
        let result = self
            .invoke_typed(EndSessionInput::TOOL_NAME, input)
            .await?
            .typed_success(EndSessionInput::TOOL_NAME);
        self.backend.close_async().await;
        result
    }
}

#[uniffi::export]
impl CuaDriverSession {
    /// Revoke the session-bound authority and release its native handle.
    ///
    /// This operation is idempotent. Dropping the object performs the same
    /// cleanup, but trusted hosts should call it at their lifecycle boundary.
    pub fn close(&self) {
        self.backend.close();
    }
}

impl CuaDriverSession {
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
            SessionBackend::Embedded(runtime) => runtime.invoke(name, arguments).await?,
            SessionBackend::PrivateWorker {
                client,
                session_handle,
                closed,
            } => {
                if closed.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(DriverError::Shutdown);
                }
                client
                    .invoke(name, arguments, Some(session_handle.clone()))
                    .await?
            }
            SessionBackend::Service { client, closed } => {
                if closed.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(DriverError::Shutdown);
                }
                client.invoke(name, arguments).await?
            }
            SessionBackend::Remote(session) => session.invoke(name, arguments).await?,
        };
        normalize_result(raw)
    }
}

impl CuaDriver {
    /// Rust-only ingress for a trusted protocol adapter that already removed
    /// caller-supplied reserved arguments before adding its own transport
    /// evidence. Public SDK callers must use [`Self::call_tool`].
    #[doc(hidden)]
    pub async fn call_tool_from_trusted_adapter(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, DriverError> {
        if !arguments.is_object() {
            return Err(DriverError::InvalidArguments {
                tool: name.into(),
                reason: "arguments must be a JSON object".into(),
            });
        }
        self.invoke(name, arguments).await
    }

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
            DriverBackend::PrivateWorker(worker) => worker.invoke(name, arguments, None).await?,
            DriverBackend::Remote(remote) => remote.invoke(name, arguments).await?,
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

    fn configured_standard_driver() -> Arc<CuaDriver> {
        CuaDriver::create_configured(ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![SessionPermissionMode::Standard],
                compatibility_mode: SessionPermissionMode::Standard,
                compatibility_bounded_manifest_path: None,
                unrestricted_acknowledged: false,
                max_session_ttl_seconds: 60,
                max_idle_ttl_seconds: 30,
            },
        })
        .unwrap()
    }

    #[tokio::test]
    async fn embedded_runtime_owns_tools_without_daemon_ipc_and_shuts_down_idempotently() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
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
    async fn embedded_runtime_enforces_authorization_before_platform_dispatch() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let driver = CuaDriver::create(None).unwrap();
        let result = driver
            .call_tool(
                "click".into(),
                serde_json::json!({
                    "pid": std::process::id(),
                    "x": 1,
                    "y": 1
                })
                .to_string(),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert_eq!(result.error_code.as_deref(), Some("permission_denied"));
        assert!(result.text.contains("authorization process"));
        let structured: Value =
            serde_json::from_str(result.structured_json.as_deref().unwrap()).unwrap();
        assert_eq!(
            structured.pointer("/refusal/code"),
            Some(&Value::String("permission_denied".into()))
        );
    }

    #[tokio::test]
    async fn direct_runtime_is_process_exclusive_and_reusable_after_shutdown() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let first = CuaDriver::create(None).unwrap();
        assert!(matches!(
            CuaDriver::create(None),
            Err(DriverError::RuntimeAlreadyExists)
        ));
        first.shutdown().await.unwrap();
        let replacement = configured_standard_driver();
        let replacement_session = replacement
            .create_trusted_session(TrustedSessionOptions {
                public_session: "replacement-after-stale-runtime".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            })
            .unwrap();

        // Dropping the already-shut-down generation must not revoke sessions
        // owned by the replacement runtime.
        drop(first);
        let result = replacement_session
            .call_tool("health_report".into(), "{}".into())
            .await
            .unwrap();
        assert_ne!(result.error_code.as_deref(), Some("permission_denied"));

        replacement_session.close();
        replacement.shutdown().await.unwrap();
        let after_drop = CuaDriver::create(None).unwrap();
        after_drop.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_session_close_can_race_with_invoke_without_invalidating_the_handle() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let driver = configured_standard_driver();

        for iteration in 0..64 {
            let session = driver
                .create_trusted_session(TrustedSessionOptions {
                    public_session: format!("close-race-{iteration}"),
                    mode: SessionPermissionMode::Standard,
                    ttl_seconds: 60,
                    idle_ttl_seconds: 30,
                    bounded_manifest_path: None,
                })
                .unwrap();
            let barrier = Arc::new(tokio::sync::Barrier::new(3));
            let invoke_session = session.clone();
            let invoke_barrier = barrier.clone();
            let invoke = tokio::spawn(async move {
                invoke_barrier.wait().await;
                invoke_session
                    .call_tool("health_report".into(), "{}".into())
                    .await
            });
            let close_session = session.clone();
            let close_barrier = barrier.clone();
            let close = tokio::spawn(async move {
                close_barrier.wait().await;
                close_session.close();
            });

            barrier.wait().await;
            close.await.unwrap();
            match invoke.await.unwrap() {
                Ok(result) => {
                    assert_ne!(result.error_code.as_deref(), Some("permission_denied"));
                }
                Err(DriverError::Shutdown) => {}
                Err(error) => panic!("unexpected close-race error: {error}"),
            }
        }

        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn trusted_session_is_bound_in_memory_and_rejects_public_id_substitution() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let driver = CuaDriver::create_configured(ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![
                    SessionPermissionMode::Standard,
                    SessionPermissionMode::Unrestricted,
                ],
                compatibility_mode: SessionPermissionMode::Standard,
                compatibility_bounded_manifest_path: None,
                unrestricted_acknowledged: true,
                max_session_ttl_seconds: 60,
                max_idle_ttl_seconds: 30,
            },
        })
        .unwrap();
        let session = driver
            .create_trusted_session(TrustedSessionOptions {
                public_session: "trusted-a".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            })
            .unwrap();

        let result = session
            .call_tool("health_report".into(), "{}".into())
            .await
            .unwrap();
        assert_ne!(result.error_code.as_deref(), Some("permission_denied"));

        let substituted = session
            .call_tool(
                "health_report".into(),
                serde_json::json!({"session": "other"}).to_string(),
            )
            .await
            .unwrap();
        assert_eq!(substituted.error_code.as_deref(), Some("permission_denied"));
        assert!(substituted.text.contains("substitution"));

        let reserved_substitution = session
            .call_tool(
                "health_report".into(),
                serde_json::json!({
                    "_session_id": "other",
                    "_transport_session_id": "other"
                })
                .to_string(),
            )
            .await
            .unwrap();
        assert_ne!(
            reserved_substitution.error_code.as_deref(),
            Some("permission_denied")
        );

        assert!(matches!(
            driver.create_trusted_session(TrustedSessionOptions {
                public_session: "trusted-a".into(),
                mode: SessionPermissionMode::Unrestricted,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            }),
            Err(DriverError::Configuration { .. })
        ));
        let unrestricted = driver
            .create_trusted_session(TrustedSessionOptions {
                public_session: "trusted-b".into(),
                mode: SessionPermissionMode::Unrestricted,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            })
            .unwrap();
        let unrestricted_result = unrestricted
            .call_tool("health_report".into(), "{}".into())
            .await
            .unwrap();
        assert_ne!(
            unrestricted_result.error_code.as_deref(),
            Some("permission_denied")
        );
        driver.shutdown().await.unwrap();
        assert!(matches!(
            session.call_tool("health_report".into(), "{}".into()).await,
            Err(DriverError::Shutdown)
        ));
        session.close();
        session.close();
        unrestricted.close();
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

    struct FakeRemoteChannel {
        bound: bool,
    }

    #[async_trait::async_trait]
    impl remote::DriverEnvelopeChannel for FakeRemoteChannel {
        async fn exchange(
            &self,
            request: remote::DriverRequestEnvelope,
        ) -> Result<remote::DriverResponseEnvelope, String> {
            let result = match request.operation.as_str() {
                "metadata" => serde_json::to_value(DriverMetadata {
                    driver_version: env!("CARGO_PKG_VERSION").into(),
                    contract_version: cua_driver_contract::CONTRACT_VERSION.into(),
                    tools_list_schema_version: cua_driver_contract::TOOLS_LIST_SCHEMA_VERSION
                        .into(),
                    capability_version: cua_driver_contract::CAPABILITY_VERSION.into(),
                    mcp_protocol_version: cua_driver_contract::MCP_PROTOCOL_VERSION.into(),
                    pid: 42,
                    embedded: false,
                    host_bundle_id: None,
                })
                .unwrap(),
                "list" => serde_json::json!({"tools": []}),
                "call" => serde_json::json!({
                    "content": [{"type": "text", "text": if self.bound { "bound" } else { "compatibility" }}],
                    "structuredContent": {"ok": true},
                    "isError": false,
                }),
                other => return Err(format!("unexpected operation {other}")),
            };
            Ok(remote::DriverResponseEnvelope {
                envelope_version: remote::DRIVER_ENVELOPE_VERSION,
                request_id: request.request_id,
                ok: true,
                result: Some(result),
                error: None,
                error_code: None,
                completion_known: true,
            })
        }

        async fn bind_session(
            &self,
            _options: TrustedSessionOptions,
        ) -> Result<Arc<dyn remote::DriverEnvelopeChannel>, String> {
            Ok(Arc::new(Self { bound: true }))
        }

        async fn close(&self) -> Result<(), String> {
            Ok(())
        }

        fn authenticated_principal(&self) -> &str {
            "test-principal"
        }

        fn connection_generation(&self) -> &str {
            "test-generation"
        }
    }

    #[tokio::test]
    async fn remote_carrier_uses_generated_envelopes_behind_the_typed_sdk() {
        let driver = CuaDriver::connect_remote(Arc::new(FakeRemoteChannel { bound: false }))
            .expect("authenticated carrier");
        assert_eq!(driver.execution_mode(), DriverExecutionMode::Remote);
        assert_eq!(driver.metadata().await.unwrap().pid, 42);
        assert_eq!(
            driver
                .call_tool("health_report".into(), "{}".into())
                .await
                .unwrap()
                .text,
            "compatibility"
        );

        let session = driver
            .create_remote_trusted_session(TrustedSessionOptions {
                public_session: "remote-bound".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            })
            .await
            .unwrap();
        assert_eq!(
            session
                .call_tool("health_report".into(), "{}".into())
                .await
                .unwrap()
                .text,
            "bound"
        );
        session.close();
        driver.shutdown().await.unwrap();
    }

    struct UncertainRemoteChannel {
        transport_error: bool,
    }

    #[async_trait::async_trait]
    impl remote::DriverEnvelopeChannel for UncertainRemoteChannel {
        async fn exchange(
            &self,
            request: remote::DriverRequestEnvelope,
        ) -> Result<remote::DriverResponseEnvelope, String> {
            if self.transport_error {
                return Err("carrier lost the action response".into());
            }
            Ok(remote::DriverResponseEnvelope {
                envelope_version: remote::DRIVER_ENVELOPE_VERSION,
                request_id: request.request_id,
                ok: true,
                result: Some(serde_json::json!({
                    "content": [{"type": "text", "text": "untrusted success"}],
                    "isError": false,
                })),
                error: None,
                error_code: None,
                completion_known: false,
            })
        }

        async fn bind_session(
            &self,
            _options: TrustedSessionOptions,
        ) -> Result<Arc<dyn remote::DriverEnvelopeChannel>, String> {
            Err("not used".into())
        }

        async fn close(&self) -> Result<(), String> {
            Ok(())
        }

        fn authenticated_principal(&self) -> &str {
            "test-principal"
        }

        fn connection_generation(&self) -> &str {
            "test-generation"
        }
    }

    #[tokio::test]
    async fn remote_action_transport_failures_never_look_safe_to_retry() {
        for transport_error in [true, false] {
            let driver =
                CuaDriver::connect_remote(Arc::new(UncertainRemoteChannel { transport_error }))
                    .unwrap();
            assert!(matches!(
                driver
                    .call_tool("click".into(), serde_json::json!({}).to_string())
                    .await,
                Err(DriverError::ActionInterrupted {
                    completion: ActionCompletion::Unknown,
                    ..
                })
            ));
        }
    }
}
