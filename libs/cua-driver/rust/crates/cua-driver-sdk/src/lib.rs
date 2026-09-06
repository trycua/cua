//! Canonical typed Cua Driver SDK and UniFFI export boundary.
//!
//! [`CuaDriver::create`] owns the platform runtime in the importing process.
//! [`CuaDriver::connect`] remains a temporary compatibility constructor for the
//! released daemon-client topology. Both paths expose the same typed operations;
//! MCP and daemon transports are downstream adapters rather than peer contracts.

use cua_driver_contract::{
    ActionResult, ClickInput, ClipboardReadInput, ClipboardWriteInput, DragInput, EndSessionInput,
    EndSessionOutput, EscalateSessionInput, GetAgentCursorStateInput, GetCursorPositionInput,
    GetDesktopStateInput, GetScreenSizeInput, GetSessionInput, GetSessionStateInput, HotkeyInput,
    InvokeMenuInput, ListSessionsInput, ListSessionsOutput, MoveCursorInput, PressKeyInput,
    ScrollInput, SessionOutput, SessionStateOutput, SetAgentCursorEnabledInput,
    SetAgentCursorMotionInput, SetAgentCursorThemeInput, SetWindowFrameInput, StartSessionInput,
    StartSessionOutput, ToolInput, TypeTextInput, VerifyStateInput, VerifyStateOutput,
};
use cua_driver_core::daemon::{
    is_daemon_listening, request_daemon_metadata, send_request, socket_path_for_namespace,
    DaemonClientKind, DaemonMetadata, DaemonRequest, ToolObservationOrigin,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;

mod abi;
mod activity_observer;
mod authorization_host;
mod embedded;
pub mod remote;
mod runtime;
mod service_session;
#[doc(hidden)]
pub mod worker;
use abi::{NativeAbiDriver, NativeAbiSession};
pub use activity_observer::{DriverActivityEvent, DriverActivityKind, DriverActivityObserver};
pub use authorization_host::{
    DriverAuthorizationAction, DriverAuthorizationDecision, DriverAuthorizationHost,
    DriverAuthorizationHostError, DriverAuthorizationRequest,
};
pub use embedded::*;
use remote::{DriverEnvelopeChannel, RemoteBoundSession, RemoteDriverClient};
use runtime::RuntimeOptions;
use service_session::ServiceSessionClient;
use worker::{ActionCompletion, PrivateWorkerClient};

fn host_sessions_json_for_prefix(runtime_prefix: &str) -> Value {
    let sessions = cua_driver_core::session::list_session_snapshots_with_prefix(
        runtime_prefix,
        cua_driver_core::session::DEFAULT_SESSION_IDLE_TTL,
    )
    .into_iter()
    .map(|session| {
        let transport = match session.transport {
            cua_driver_core::session::SessionTransport::Cli => "cli",
            cua_driver_core::session::SessionTransport::Daemon => "daemon",
            cua_driver_core::session::SessionTransport::McpStdio => "mcp_stdio",
            cua_driver_core::session::SessionTransport::McpHttp => "mcp_http",
        };
        let client_kind = match session.client_kind {
            cua_driver_core::session::SessionClientKind::Cli => "cli",
            cua_driver_core::session::SessionClientKind::Direct => "direct",
            cua_driver_core::session::SessionClientKind::Mcp => "mcp",
            cua_driver_core::session::SessionClientKind::PythonSdk => "python_sdk",
            cua_driver_core::session::SessionClientKind::TypescriptSdk => "typescript_sdk",
        };
        serde_json::json!({
            "session": session.public_label.as_deref().and_then(cursor_overlay::sanitize_session_label),
            "implicit": session.implicit,
            "state": if session.ending { "ending" } else { "active" },
            "client_kind": client_kind,
            "transport": transport,
            "cursor_visible": cua_driver_core::session::cursor_visible(&session.runtime_id),
            "recording_active": cua_driver_core::session::recording_active(&session.runtime_id),
            "started_seconds_ago": session.started_for.as_secs(),
            "idle_seconds": session.idle.as_secs(),
            "expires_in_seconds": session.expires_in.as_secs(),
        })
    })
    .collect::<Vec<_>>();
    serde_json::json!({"count": sessions.len(), "sessions": sessions})
}

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
    pub action: Option<ActionResult>,
    pub verification: Option<VerifyStateOutput>,
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
    Daemon(Arc<DaemonBackend>),
    PrivateWorker(Arc<PrivateWorkerClient>),
    Remote(Arc<RemoteDriverClient>),
}

impl CuaDriver {
    /// Trusted Rust-host access to the daemon-owned local history controller.
    /// This is intentionally absent from UniFFI and public agent protocols.
    #[doc(hidden)]
    pub fn local_history_manager(&self) -> Option<Arc<cua_driver_core::history::HistoryManager>> {
        match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.history(),
            DriverBackend::Daemon(_)
            | DriverBackend::PrivateWorker(_)
            | DriverBackend::Remote(_) => None,
        }
    }
}

struct DaemonBackend {
    socket_path: String,
    transport_session: String,
}

impl DaemonBackend {
    fn new(socket_path: String) -> Arc<Self> {
        Arc::new(Self {
            socket_path,
            transport_session: format!("sdk-{}", uuid::Uuid::new_v4()),
        })
    }

    async fn compatible_metadata(&self) -> Result<DaemonMetadata, DriverError> {
        // Local socket paths can be rebound by a replacement daemon between
        // calls. Re-negotiate every action instead of caching a successful
        // result from an earlier process generation.
        let socket_path = self.socket_path.clone();
        let request_path = socket_path.clone();
        let metadata = tokio::task::spawn_blocking(move || request_daemon_metadata(&request_path))
            .await
            .map_err(|error| DriverError::Protocol {
                reason: format!("daemon compatibility task failed: {error}"),
            })?
            .map_err(|error| DriverError::Transport {
                socket_path,
                reason: error.to_string(),
            })?;
        validate_daemon_metadata(&metadata)?;
        Ok(metadata)
    }
}

impl Drop for DaemonBackend {
    fn drop(&mut self) {
        let request = DaemonRequest {
            method: "session_end".into(),
            name: None,
            args: None,
            session_id: Some(self.transport_session.clone()),
            observation_origin: Some(ToolObservationOrigin::Direct),
            client_kind: Some(DaemonClientKind::Unknown),
        };
        let _ = send_request(&self.socket_path, &request);
    }
}

fn validate_daemon_metadata(metadata: &DaemonMetadata) -> Result<(), DriverError> {
    let mismatch = if metadata.contract_version != cua_driver_contract::CONTRACT_VERSION {
        Some(format!(
            "contract version {} does not match SDK {}",
            metadata.contract_version,
            cua_driver_contract::CONTRACT_VERSION
        ))
    } else if metadata.tools_list_schema_version != cua_driver_contract::TOOLS_LIST_SCHEMA_VERSION {
        Some(format!(
            "tools-list schema version {} does not match SDK {}",
            metadata.tools_list_schema_version,
            cua_driver_contract::TOOLS_LIST_SCHEMA_VERSION
        ))
    } else if metadata.capability_version != cua_driver_contract::CAPABILITY_VERSION {
        Some(format!(
            "capability version {} does not match SDK {}",
            metadata.capability_version,
            cua_driver_contract::CAPABILITY_VERSION
        ))
    } else if metadata.mcp_protocol_version != cua_driver_contract::MCP_PROTOCOL_VERSION {
        Some(format!(
            "MCP protocol version {} does not match SDK {}",
            metadata.mcp_protocol_version,
            cua_driver_contract::MCP_PROTOCOL_VERSION
        ))
    } else {
        None
    };
    mismatch.map_or(Ok(()), |reason| {
        Err(DriverError::Protocol {
            reason: format!("incompatible daemon: {reason}"),
        })
    })
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
    /// Optional in standard and unrestricted; required when compatibility mode is bounded.
    #[uniffi(default = None)]
    pub compatibility_capability_manifest_path: Option<String>,
    /// Deprecated alias for `compatibility_capability_manifest_path`.
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

fn configured_driver_options_json(options: &ConfiguredDriverOptions) -> Value {
    let allowed_modes = options
        .authorization
        .allowed_modes
        .iter()
        .map(|mode| mode.as_str())
        .collect::<Vec<_>>();
    serde_json::json!({
        "claude_code_compatibility": options.claude_code_compatibility,
        "authorization": {
            "allowed_modes": allowed_modes,
            "compatibility_mode": options.authorization.compatibility_mode.as_str(),
            "compatibility_capability_manifest_path": options.authorization.compatibility_capability_manifest_path.clone(),
            "compatibility_bounded_manifest_path": options.authorization.compatibility_bounded_manifest_path.clone(),
            "unrestricted_acknowledged": options.authorization.unrestricted_acknowledged,
            "max_session_ttl_seconds": options.authorization.max_session_ttl_seconds,
            "max_idle_ttl_seconds": options.authorization.max_idle_ttl_seconds,
        }
    })
}

/// Trusted host request for one immutable, connection-bound action surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TrustedSessionOptions {
    pub public_session: String,
    pub mode: SessionPermissionMode,
    pub ttl_seconds: u64,
    pub idle_ttl_seconds: u64,
    #[uniffi(default = None)]
    pub capability_manifest_path: Option<String>,
    /// Deprecated alias for `capability_manifest_path`.
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
    /// The runtime owner is the embedding/direct host, so macOS permission
    /// checks are status-only and the host owns all request/restart UX.
    pub host_owns_permission_ux: bool,
    /// Advisory host identity shown in macOS permission diagnostics. It never
    /// grants authority and is carried as immutable runtime configuration so
    /// private workers do not mutate process-global environment state.
    pub host_bundle_id: Option<String>,
    pub claude_code_compatibility: bool,
    pub prepare_desktop_environment: bool,
    /// Temporary compatibility hook for daemon-only administrative tools.
    /// Desktop operations must live behind the typed SDK contract instead.
    pub register_host_tools: Option<fn(&mut cua_driver_core::tool::ToolRegistry)>,
    /// Constructor-only authorization surface supplied by a trusted Rust
    /// host. It is never exposed through UniFFI, MCP, CLI arguments, or
    /// transport metadata.
    pub authorization_host: Option<Arc<dyn DriverAuthorizationHost>>,
    /// Optional content-free activity callback supplied by a trusted host.
    pub activity_observer: Option<Arc<dyn DriverActivityObserver>>,
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
            verify_state: VerifyStateInput,
            move_cursor: MoveCursorInput,
            set_window_frame: SetWindowFrameInput,
            invoke_menu: InvokeMenuInput,
            click: ClickInput,
            drag: DragInput,
            scroll: ScrollInput,
            clipboard_read: ClipboardReadInput,
            clipboard_write: ClipboardWriteInput,
            type_text: TypeTextInput,
            press_key: PressKeyInput,
            hotkey: HotkeyInput,
            set_agent_cursor_enabled: SetAgentCursorEnabledInput,
            set_agent_cursor_motion: SetAgentCursorMotionInput,
            set_agent_cursor_theme: SetAgentCursorThemeInput,
            get_agent_cursor_state: GetAgentCursorStateInput,
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
            <GetSessionInput as ToolInput>::TOOL_NAME,
            <ListSessionsInput as ToolInput>::TOOL_NAME,
            <GetSessionStateInput as ToolInput>::TOOL_NAME,
            <EndSessionInput as ToolInput>::TOOL_NAME,
            $(<$input as ToolInput>::TOOL_NAME,)*
        ];
    };
}

desktop_tool_methods!(define_exported_tool_names);

#[uniffi::export]
impl CuaDriver {
    #[doc(hidden)]
    pub fn runtime_scope_prefix(&self) -> Option<String> {
        match &self.backend {
            DriverBackend::Embedded(native) => {
                Some(format!("__cua_runtime_{}:", native.runtime_scope_key()))
            }
            DriverBackend::Daemon(_)
            | DriverBackend::PrivateWorker(_)
            | DriverBackend::Remote(_) => None,
        }
    }

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
        let native_options = configured_driver_options_json(&options);
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(NativeAbiDriver::create_configured(
                native_options,
            )?)),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Create a configured same-process runtime with an optional residual
    /// authorization callback supplied by trusted embedding-host code.
    ///
    /// The callback object is immutable runtime configuration. Applications
    /// must not expose it to an agent or implement it using ordinary MCP
    /// elicitation, model-visible stdio, or an auto-accepting callback.
    #[uniffi::constructor]
    pub fn create_configured_with_authorization_host(
        options: ConfiguredDriverOptions,
        host: Arc<dyn DriverAuthorizationHost>,
    ) -> Result<Arc<Self>, DriverError> {
        let provider = authorization_host::SdkDriverAuthorizationHost::new(host);
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(
                NativeAbiDriver::create_configured_with_authorization_provider(
                    configured_driver_options_json(&options),
                    provider,
                    None,
                )?,
            )),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Create a configured same-process runtime with a content-free activity
    /// observer supplied by trusted embedding-host code.
    #[uniffi::constructor]
    pub fn create_configured_with_activity_observer(
        options: ConfiguredDriverOptions,
        observer: Arc<dyn DriverActivityObserver>,
    ) -> Result<Arc<Self>, DriverError> {
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(
                NativeAbiDriver::create_configured_with_activity_observer(
                    configured_driver_options_json(&options),
                    observer,
                )?,
            )),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Create a configured same-process runtime with both residual
    /// authorization and content-free activity callbacks.
    #[uniffi::constructor]
    pub fn create_configured_with_host_integrations(
        options: ConfiguredDriverOptions,
        host: Arc<dyn DriverAuthorizationHost>,
        observer: Arc<dyn DriverActivityObserver>,
    ) -> Result<Arc<Self>, DriverError> {
        let provider = authorization_host::SdkDriverAuthorizationHost::new(host);
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(Arc::new(
                NativeAbiDriver::create_configured_with_authorization_provider(
                    configured_driver_options_json(&options),
                    provider,
                    Some(observer),
                )?,
            )),
            client_kind: DaemonClientKind::Unknown,
        }))
    }

    /// Language-package entry point for a configured protected-host runtime.
    #[uniffi::constructor]
    pub fn create_configured_with_authorization_host_and_client_kind(
        options: ConfiguredDriverOptions,
        host: Arc<dyn DriverAuthorizationHost>,
        client_kind: SdkClientKind,
    ) -> Result<Arc<Self>, DriverError> {
        let driver = Self::create_configured_with_authorization_host(options, host)?;
        let DriverBackend::Embedded(runtime) = &driver.backend else {
            unreachable!("protected-host constructor always returns an embedded runtime")
        };
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(runtime.clone()),
            client_kind: client_kind.into(),
        }))
    }

    /// Language-package entry point for a configured activity-observer runtime.
    #[uniffi::constructor]
    pub fn create_configured_with_activity_observer_and_client_kind(
        options: ConfiguredDriverOptions,
        observer: Arc<dyn DriverActivityObserver>,
        client_kind: SdkClientKind,
    ) -> Result<Arc<Self>, DriverError> {
        let driver = Self::create_configured_with_activity_observer(options, observer)?;
        let DriverBackend::Embedded(runtime) = &driver.backend else {
            unreachable!("activity-observer constructor always returns an embedded runtime")
        };
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(runtime.clone()),
            client_kind: client_kind.into(),
        }))
    }

    /// Language-package entry point for both trusted host callbacks.
    #[uniffi::constructor]
    pub fn create_configured_with_host_integrations_and_client_kind(
        options: ConfiguredDriverOptions,
        host: Arc<dyn DriverAuthorizationHost>,
        observer: Arc<dyn DriverActivityObserver>,
        client_kind: SdkClientKind,
    ) -> Result<Arc<Self>, DriverError> {
        let driver = Self::create_configured_with_host_integrations(options, host, observer)?;
        let DriverBackend::Embedded(runtime) = &driver.backend else {
            unreachable!("host-integration constructor always returns an embedded runtime")
        };
        Ok(Arc::new(Self {
            backend: DriverBackend::Embedded(runtime.clone()),
            client_kind: client_kind.into(),
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
            backend: DriverBackend::Daemon(DaemonBackend::new(socket_path)),
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
        let DriverBackend::Daemon(daemon) = &driver.backend else {
            unreachable!("connect always returns a daemon client")
        };
        Ok(Arc::new(Self {
            backend: DriverBackend::Daemon(daemon.clone()),
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
            DriverBackend::Daemon(_) => DriverExecutionMode::Daemon,
            DriverBackend::PrivateWorker(_) => DriverExecutionMode::PrivateWorker,
            DriverBackend::Remote(_) => DriverExecutionMode::Remote,
        }
    }

    /// Compatibility accessor. Embedded runtimes have no socket and return an
    /// empty string; new code should branch on [`Self::execution_mode`].
    pub fn socket_path(&self) -> String {
        match &self.backend {
            DriverBackend::Embedded(_) => String::new(),
            DriverBackend::Daemon(daemon) => daemon.socket_path.clone(),
            DriverBackend::PrivateWorker(_) => String::new(),
            DriverBackend::Remote(_) => String::new(),
        }
    }

    pub fn is_available(&self) -> bool {
        match &self.backend {
            DriverBackend::Embedded(runtime) => runtime.is_available(),
            DriverBackend::Daemon(daemon) => is_daemon_listening(&daemon.socket_path),
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
    /// Return the canonical platform tool inventory without creating an
    /// action-capable runtime.
    ///
    /// Rust transport adapters use this only for finite metadata commands such
    /// as `list-tools`, `describe`, and `dump-docs`. The returned definitions
    /// cannot dispatch actions, own sessions, or bypass interactive-desktop
    /// admission.
    pub fn inspect_host_tools(options: DriverHostOptions) -> Value {
        runtime::tool_inventory(RuntimeOptions {
            cursor: options.cursor,
            host_owns_permission_ux: options.host_owns_permission_ux,
            host_bundle_id: options.host_bundle_id,
            compatibility_mode: options.claude_code_compatibility,
            prepare_desktop_environment: options.prepare_desktop_environment,
            register_host_tools: options.register_host_tools,
            authorization_ceiling: None,
            compatibility_authorization: None,
            authorization_host: authorization_host::SdkDriverAuthorizationHost::adapt(
                options.authorization_host,
            ),
            activity_observer: options.activity_observer,
        })
    }

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
                    "capability_manifest_path": options.capability_manifest_path,
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
            DriverBackend::Daemon(daemon) => {
                let client = ServiceSessionClient::connect_and_bind(
                    daemon.socket_path.clone(),
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

    /// Daemon-host bridge for reconnecting one trusted host lease without
    /// exposing its transport identity through generated bindings.
    #[doc(hidden)]
    pub fn create_trusted_session_for_transport(
        &self,
        options: TrustedSessionOptions,
        transport_session: &str,
    ) -> Result<Arc<CuaDriverSession>, DriverError> {
        let DriverBackend::Embedded(runtime) = &self.backend else {
            return Err(DriverError::Configuration {
                reason:
                    "stable trusted-session transport binding requires an embedded host runtime"
                        .into(),
            });
        };
        if transport_session.trim().is_empty() {
            return Err(DriverError::Configuration {
                reason: "trusted-session transport identity must not be empty".into(),
            });
        }
        let native_options = serde_json::json!({
            "public_session": options.public_session,
            "mode": options.mode.as_str(),
            "ttl_seconds": options.ttl_seconds,
            "idle_ttl_seconds": options.idle_ttl_seconds,
            "capability_manifest_path": options.capability_manifest_path,
            "bounded_manifest_path": options.bounded_manifest_path,
            "transport_session": transport_session,
        });
        Ok(Arc::new(CuaDriverSession {
            backend: SessionBackend::Embedded(Arc::new(runtime.create_session(native_options)?)),
        }))
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
                    host_owns_permission_ux: options.host_owns_permission_ux,
                    host_bundle_id: options.host_bundle_id,
                    compatibility_mode: options.claude_code_compatibility,
                    prepare_desktop_environment: options.prepare_desktop_environment,
                    register_host_tools: options.register_host_tools,
                    authorization_ceiling: None,
                    compatibility_authorization: None,
                    authorization_host: authorization_host::SdkDriverAuthorizationHost::adapt(
                        options.authorization_host,
                    ),
                    activity_observer: options.activity_observer,
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
        let manifest = cua_driver_core::session_manifest::configured_capability_manifest()
            .map_err(|error| DriverError::Configuration { reason: error })?
            .cloned()
            .map(Arc::new);
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
                    host_owns_permission_ux: options.host_owns_permission_ux,
                    host_bundle_id: options.host_bundle_id,
                    compatibility_mode: options.claude_code_compatibility,
                    prepare_desktop_environment: options.prepare_desktop_environment,
                    register_host_tools: options.register_host_tools,
                    authorization_ceiling: Some(ceiling),
                    compatibility_authorization: Some((mode, manifest)),
                    authorization_host: authorization_host::SdkDriverAuthorizationHost::adapt(
                        options.authorization_host,
                    ),
                    activity_observer: options.activity_observer,
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
                "compatibility_capability_manifest_path": options.authorization.compatibility_capability_manifest_path,
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
                    host.host_owns_permission_ux,
                    host.host_bundle_id,
                    host.prepare_desktop_environment,
                    host.register_host_tools,
                    authorization_host::SdkDriverAuthorizationHost::adapt(host.authorization_host),
                    host.activity_observer,
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
            DriverBackend::Daemon(daemon) => {
                let metadata = daemon.compatible_metadata().await?;
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
            DriverBackend::Daemon(daemon) => {
                daemon.compatible_metadata().await?;
                let socket_path = daemon.socket_path.clone();
                let request_path = socket_path.clone();
                let request = DaemonRequest {
                    method: "list".into(),
                    name: None,
                    args: None,
                    session_id: Some(daemon.transport_session.clone()),
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

    /// Content-free lifecycle summaries for the trusted host's runtime or
    /// host-lease namespace. This is deliberately separate from the
    /// agent-facing `list_sessions` tool, whose view is transport scoped.
    pub async fn list_host_sessions_json(&self) -> Result<String, DriverError> {
        let result = match &self.backend {
            DriverBackend::Embedded(runtime) => {
                let prefix = format!("__cua_runtime_{}:", runtime.runtime_scope_key());
                host_sessions_json_for_prefix(&prefix)
            }
            DriverBackend::PrivateWorker(worker) => worker.list_host_sessions().await?,
            DriverBackend::Remote(remote) => remote.list_host_sessions().await?,
            DriverBackend::Daemon(daemon) => {
                daemon.compatible_metadata().await?;
                let socket_path = daemon.socket_path.clone();
                let request_path = socket_path.clone();
                let request = DaemonRequest {
                    method: "sessions_list".into(),
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
                            reason: format!("host session listing task failed: {error}"),
                        })?
                        .map_err(|error| DriverError::Transport {
                            socket_path,
                            reason: error.to_string(),
                        })?;
                if !response.ok {
                    return Err(DriverError::Protocol {
                        reason: response
                            .error
                            .unwrap_or_else(|| "host session listing failed".into()),
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

    pub async fn get_session(&self, input: GetSessionInput) -> Result<SessionOutput, DriverError> {
        self.invoke_typed(GetSessionInput::TOOL_NAME, input)
            .await?
            .typed_success(GetSessionInput::TOOL_NAME)
    }

    pub async fn list_sessions(
        &self,
        input: ListSessionsInput,
    ) -> Result<ListSessionsOutput, DriverError> {
        self.invoke_typed(ListSessionsInput::TOOL_NAME, input)
            .await?
            .typed_success(ListSessionsInput::TOOL_NAME)
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
            DriverBackend::Daemon(_) => {}
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

    pub async fn get_session(&self, input: GetSessionInput) -> Result<SessionOutput, DriverError> {
        self.invoke_typed(GetSessionInput::TOOL_NAME, input)
            .await?
            .typed_success(GetSessionInput::TOOL_NAME)
    }

    pub async fn list_sessions(
        &self,
        input: ListSessionsInput,
    ) -> Result<ListSessionsOutput, DriverError> {
        self.invoke_typed(ListSessionsInput::TOOL_NAME, input)
            .await?
            .typed_success(ListSessionsInput::TOOL_NAME)
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
        normalize_result(name, raw)
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
        mut arguments: Value,
    ) -> Result<ToolResult, DriverError> {
        if !arguments.is_object() {
            return Err(DriverError::InvalidArguments {
                tool: name.into(),
                reason: "arguments must be a JSON object".into(),
            });
        }
        if let DriverBackend::Embedded(runtime) = &self.backend {
            let raw = runtime.invoke_from_trusted_adapter(name, arguments).await?;
            return normalize_result(name, raw);
        }

        // Trust evidence is local to the adapter/runtime boundary. A private
        // worker re-establishes it in the child; remote and daemon transports
        // derive their own evidence from their authenticated connection and
        // must never receive underscore-prefixed claims from this process.
        if !matches!(&self.backend, DriverBackend::PrivateWorker(_)) {
            cua_driver_core::tool_args::sanitize_reserved_args(&mut arguments);
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
            DriverBackend::Daemon(daemon) => {
                daemon.compatible_metadata().await?;
                let socket_path = daemon.socket_path.clone();
                let request_path = socket_path.clone();
                let request = DaemonRequest {
                    method: "call".into(),
                    name: Some(name.into()),
                    args: Some(arguments),
                    session_id: Some(daemon.transport_session.clone()),
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
        normalize_result(name, raw)
    }
}

impl ToolResult {
    /// Typed action facts when this result came from an action tool.
    pub fn action(&self) -> Option<&ActionResult> {
        self.action.as_ref()
    }

    /// Typed tri-state postcondition result when this result came from
    /// `verify_state`.
    pub fn verification(&self) -> Option<&VerifyStateOutput> {
        self.verification.as_ref()
    }

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

fn normalize_result(tool: &str, raw: Value) -> Result<ToolResult, DriverError> {
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
    let is_error = object
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let action = if !is_error && cua_driver_core::action_record::is_action_tool(tool) {
        let structured = structured.ok_or_else(|| DriverError::Protocol {
            reason: format!("{tool} response omitted ActionResult"),
        })?;
        let action =
            serde_json::from_value::<ActionResult>(structured.clone()).map_err(|error| {
                DriverError::Protocol {
                    reason: format!("{tool} returned an invalid ActionResult: {error}"),
                }
            })?;
        action
            .validate_invariants()
            .map_err(|error| DriverError::Protocol {
                reason: format!("{tool} returned an invalid ActionResult: {error}"),
            })?;
        Some(action)
    } else {
        None
    };
    let verification = if !is_error && tool == VerifyStateInput::TOOL_NAME {
        let structured = structured.ok_or_else(|| DriverError::Protocol {
            reason: "verify_state response omitted VerifyStateOutput".into(),
        })?;
        Some(
            serde_json::from_value::<VerifyStateOutput>(structured.clone()).map_err(|error| {
                DriverError::Protocol {
                    reason: format!("verify_state returned an invalid VerifyStateOutput: {error}"),
                }
            })?,
        )
    } else {
        None
    };
    let degraded = structured
        .and_then(|value| value.get("degraded"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(ToolResult {
        text: text_parts.join("\n"),
        images,
        structured_json: structured.map(Value::to_string),
        is_error,
        error_code,
        action,
        verification,
        degraded,
        raw_json: raw.to_string(),
    })
}

uniffi::setup_scaffolding!("cua_driver_sdk");

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::io::{BufRead, BufReader, Write};
    #[cfg(unix)]
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

    #[cfg(unix)]
    fn serve_once(response: Value) -> (tempfile::TempDir, String, std::thread::JoinHandle<Value>) {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("driver.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = std::thread::spawn(move || {
            let (metadata_stream, _) = listener.accept().unwrap();
            let mut metadata_line = String::new();
            BufReader::new(metadata_stream.try_clone().unwrap())
                .read_line(&mut metadata_line)
                .unwrap();
            let metadata_request: Value = serde_json::from_str(&metadata_line).unwrap();
            assert_eq!(metadata_request["method"], "metadata");
            let metadata_response = serde_json::json!({
                "ok": true,
                "result": cua_driver_core::daemon::current_daemon_metadata()
            });
            let mut metadata_writer = metadata_stream;
            writeln!(metadata_writer, "{}", metadata_response).unwrap();

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
                compatibility_capability_manifest_path: None,
                compatibility_bounded_manifest_path: None,
                unrestricted_acknowledged: false,
                max_session_ttl_seconds: 60,
                max_idle_ttl_seconds: 30,
            },
        })
        .unwrap()
    }

    #[derive(Default)]
    struct RecordingActivityObserver {
        events: std::sync::Mutex<Vec<DriverActivityEvent>>,
    }

    impl DriverActivityObserver for RecordingActivityObserver {
        fn on_activity(&self, event: DriverActivityEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn activity_observer_receives_only_content_free_authorization_metadata() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let observer = Arc::new(RecordingActivityObserver::default());
        let driver = CuaDriver::create_configured_with_activity_observer(
            ConfiguredDriverOptions {
                claude_code_compatibility: false,
                authorization: RuntimeAuthorizationOptions {
                    allowed_modes: vec![SessionPermissionMode::Standard],
                    compatibility_mode: SessionPermissionMode::Standard,
                    compatibility_capability_manifest_path: None,
                    compatibility_bounded_manifest_path: None,
                    unrestricted_acknowledged: false,
                    max_session_ttl_seconds: 60,
                    max_idle_ttl_seconds: 30,
                },
            },
            observer.clone(),
        )
        .unwrap();

        driver
            .call_tool("health_report".into(), "{}".into())
            .await
            .unwrap();

        let events = observer.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, DriverActivityKind::AuthorizedAction);
        assert_eq!(events[0].tool_name, "health_report");
        assert_eq!(events[0].risk_class, "r0");
        assert!(events[0].public_session.is_none());
        assert!(events[0].refusal_code.is_none());
    }

    #[cfg(target_os = "macos")]
    fn configured_unrestricted_driver() -> Arc<CuaDriver> {
        CuaDriver::create_configured(ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![SessionPermissionMode::Unrestricted],
                compatibility_mode: SessionPermissionMode::Unrestricted,
                compatibility_capability_manifest_path: None,
                compatibility_bounded_manifest_path: None,
                unrestricted_acknowledged: true,
                max_session_ttl_seconds: 60,
                max_idle_ttl_seconds: 30,
            },
        })
        .unwrap()
    }

    struct SlowHostTool;

    static SLOW_HOST_TOOL_DEF: std::sync::OnceLock<cua_driver_core::tool::ToolDef> =
        std::sync::OnceLock::new();
    static SLOW_HOST_TOOL_STARTED: std::sync::OnceLock<tokio::sync::Notify> =
        std::sync::OnceLock::new();
    static SLOW_HOST_TOOL_RELEASE: std::sync::OnceLock<tokio::sync::Notify> =
        std::sync::OnceLock::new();

    #[async_trait::async_trait]
    impl cua_driver_core::tool::Tool for SlowHostTool {
        fn def(&self) -> &cua_driver_core::tool::ToolDef {
            SLOW_HOST_TOOL_DEF.get_or_init(|| cua_driver_core::tool::ToolDef {
                // Replace a reviewed R0 operation so the test exercises
                // lifecycle draining rather than the unknown-tool fail-closed
                // authorization path.
                name: "health_report".into(),
                description: "test-only admitted-call drain probe".into(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            })
        }

        async fn invoke(&self, _args: Value) -> cua_driver_core::protocol::ToolResult {
            SLOW_HOST_TOOL_STARTED
                .get_or_init(tokio::sync::Notify::new)
                .notify_one();
            SLOW_HOST_TOOL_RELEASE
                .get_or_init(tokio::sync::Notify::new)
                .notified()
                .await;
            cua_driver_core::protocol::ToolResult::text("drained")
        }
    }

    fn register_slow_host_tool(registry: &mut cua_driver_core::tool::ToolRegistry) {
        registry.register(Box::new(SlowHostTool));
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn host_tool_inspection_does_not_acquire_runtime_ownership() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let inventory = CuaDriver::inspect_host_tools(DriverHostOptions {
            cursor: cursor_overlay::CursorConfig {
                enabled: false,
                ..cursor_overlay::CursorConfig::default()
            },
            host_owns_permission_ux: false,
            host_bundle_id: None,
            claude_code_compatibility: false,
            prepare_desktop_environment: false,
            register_host_tools: None,
            authorization_host: None,
            activity_observer: None,
        });
        assert!(inventory["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "click")));

        let driver = CuaDriver::create(None).unwrap();
        driver.shutdown().await.unwrap();
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
    async fn trusted_host_listing_spans_its_runtime_without_exposing_owner_keys() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let driver = CuaDriver::create(None).unwrap();
        let public = format!("host-listing-{}", uuid::Uuid::new_v4());
        driver
            .start_session(StartSessionInput {
                session: Some(public.clone()),
                capture_scope: None,
                cursor_theme: None,
            })
            .await
            .unwrap();

        let listed: Value =
            serde_json::from_str(&driver.list_host_sessions_json().await.unwrap()).unwrap();
        let session = listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|session| {
                session["session"]
                    .as_str()
                    .is_some_and(|label| label.starts_with("host-listing-"))
            })
            .unwrap_or_else(|| {
                panic!("host-scoped listing omitted its live named session: {listed}")
            });
        assert_eq!(session["implicit"], false);
        assert!(session.get("owner_transport").is_none());
        assert!(session.get("runtime_id").is_none());

        driver
            .end_session(EndSessionInput {
                session: Some(public),
            })
            .await
            .unwrap();
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_drains_an_already_admitted_call() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let driver = CuaDriver::try_create_for_host(DriverHostOptions {
            cursor: cursor_overlay::CursorConfig {
                enabled: false,
                ..cursor_overlay::CursorConfig::default()
            },
            host_owns_permission_ux: false,
            host_bundle_id: None,
            claude_code_compatibility: false,
            prepare_desktop_environment: false,
            register_host_tools: Some(register_slow_host_tool),
            authorization_host: None,
            activity_observer: None,
        })
        .unwrap();
        let action_driver = driver.clone();
        let action = tokio::spawn(async move {
            action_driver
                .call_tool("health_report".into(), "{}".into())
                .await
        });
        SLOW_HOST_TOOL_STARTED
            .get_or_init(tokio::sync::Notify::new)
            .notified()
            .await;
        let shutdown_driver = driver.clone();
        let shutdown = tokio::spawn(async move { shutdown_driver.shutdown().await });
        tokio::task::yield_now().await;
        assert!(
            !shutdown.is_finished(),
            "shutdown returned before the admitted call completed"
        );
        SLOW_HOST_TOOL_RELEASE
            .get_or_init(tokio::sync::Notify::new)
            .notify_one();
        assert_eq!(action.await.unwrap().unwrap().text, "drained");
        shutdown.await.unwrap().unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn session_zero_refuses_runtime_creation_before_platform_dispatch() {
        if platform_windows::diagnostics::current_session_id() != Some(0) {
            return;
        }
        let error = match CuaDriver::create(None) {
            Ok(_) => panic!("Session 0 unexpectedly created a desktop runtime"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("Session 0"));
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
    async fn direct_runtimes_have_independent_sessions_and_shutdown() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let hook_baseline = cua_driver_core::session::session_end_hook_count();
        let first = configured_standard_driver();
        let second = CuaDriver::create_configured(ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![
                    SessionPermissionMode::Standard,
                    SessionPermissionMode::Unrestricted,
                ],
                compatibility_mode: SessionPermissionMode::Standard,
                compatibility_capability_manifest_path: None,
                compatibility_bounded_manifest_path: None,
                unrestricted_acknowledged: true,
                max_session_ttl_seconds: 60,
                max_idle_ttl_seconds: 30,
            },
        })
        .unwrap();
        let first_session = first
            .create_trusted_session(TrustedSessionOptions {
                public_session: "same-public-label".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                capability_manifest_path: None,
                bounded_manifest_path: None,
            })
            .unwrap();
        let second_session = second
            .create_trusted_session(TrustedSessionOptions {
                public_session: "same-public-label".into(),
                mode: SessionPermissionMode::Unrestricted,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                capability_manifest_path: None,
                bounded_manifest_path: None,
            })
            .unwrap();

        let first_started = first_session
            .call_tool(
                "start_session".into(),
                serde_json::json!({
                    "session": "same-public-label",
                    "capture_scope": "auto"
                })
                .to_string(),
            )
            .await
            .unwrap();
        let second_started = second_session
            .call_tool(
                "start_session".into(),
                serde_json::json!({
                    "session": "same-public-label",
                    "capture_scope": "window"
                })
                .to_string(),
            )
            .await
            .unwrap();
        assert!(first_started.text.contains("'same-public-label'"));
        assert!(second_started.text.contains("'same-public-label'"));
        assert!(!first_started.text.contains("__cua_runtime_"));
        assert!(!second_started.text.contains("__cua_runtime_"));
        assert!(
            !cua_driver_core::session::has_session_activity("same-public-label"),
            "transport-visible labels must never become process-global activity keys"
        );

        if std::env::var("CUA_REQUIRE_GUI").as_deref() == Ok("1") {
            let standard_windows = first_session
                .call_tool("list_windows".into(), "{}".into())
                .await
                .unwrap();
            assert!(
                !standard_windows.is_error,
                "promptless standard mode could not inspect the GUI: code={:?} text={}",
                standard_windows.error_code, standard_windows.text
            );

            let unrestricted_windows = second_session
                .call_tool("list_windows".into(), "{}".into())
                .await
                .unwrap();
            assert!(
                !unrestricted_windows.is_error,
                "explicitly acknowledged unrestricted runtime could not inspect the GUI: {}",
                unrestricted_windows.text
            );
        }

        let second_recording_dir = tempfile::tempdir().unwrap();
        let recording_started = second_session
            .call_tool(
                "start_recording".into(),
                serde_json::json!({
                    "session": "same-public-label",
                    "output_dir": second_recording_dir.path(),
                    "record_video": false
                })
                .to_string(),
            )
            .await
            .unwrap();
        assert!(!recording_started.is_error);

        first_session
            .call_tool(
                "end_session".into(),
                serde_json::json!({"session": "same-public-label"}).to_string(),
            )
            .await
            .unwrap();
        first.shutdown().await.unwrap();
        drop(first);

        let second_state = second_session
            .call_tool(
                "get_session_state".into(),
                serde_json::json!({"session": "same-public-label"}).to_string(),
            )
            .await
            .unwrap();
        assert!(!second_state.is_error);
        let structured: Value =
            serde_json::from_str(second_state.structured_json.as_deref().unwrap()).unwrap();
        assert_eq!(structured["session"], "same-public-label");
        assert_eq!(structured["capture_scope"], "window");
        let recording_state = second_session
            .call_tool(
                "get_recording_state".into(),
                serde_json::json!({"session": "same-public-label"}).to_string(),
            )
            .await
            .unwrap();
        let recording: Value =
            serde_json::from_str(recording_state.structured_json.as_deref().unwrap()).unwrap();
        assert_eq!(recording["enabled"], true);
        if std::env::var("CUA_REQUIRE_GUI").as_deref() == Ok("1") {
            let windows_after_other_shutdown = second_session
                .call_tool("list_windows".into(), "{}".into())
                .await
                .unwrap();
            assert!(
                !windows_after_other_shutdown.is_error,
                "runtime B lost GUI access after runtime A shut down: {}",
                windows_after_other_shutdown.text
            );
        }

        second_session
            .call_tool(
                "stop_recording".into(),
                serde_json::json!({"session": "same-public-label"}).to_string(),
            )
            .await
            .unwrap();

        second_session.close();
        second.shutdown().await.unwrap();
        drop(first_session);
        drop(second_session);
        drop(second);
        assert_eq!(
            cua_driver_core::session::session_end_hook_count(),
            hook_baseline
        );
    }

    #[tokio::test]
    async fn trusted_session_idle_ttl_reaches_the_lifecycle_contract() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let driver = configured_standard_driver();
        let session = driver
            .create_trusted_session(TrustedSessionOptions {
                public_session: "trusted-lifecycle-ttl".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 2,
                capability_manifest_path: None,
                bounded_manifest_path: None,
            })
            .unwrap();

        let started = session
            .call_tool("start_session".into(), "{}".into())
            .await
            .unwrap();
        assert!(!started.is_error, "{}", started.text);

        let inspected = session
            .call_tool("get_session".into(), "{}".into())
            .await
            .unwrap();
        assert!(!inspected.is_error, "{}", inspected.text);
        let lifecycle: Value =
            serde_json::from_str(inspected.structured_json.as_deref().unwrap()).unwrap();
        let expires_in = lifecycle["expires_in_seconds"].as_u64().unwrap();
        assert!(
            expires_in <= 2,
            "trusted lifecycle TTL was not applied: {lifecycle}"
        );
        assert_eq!(lifecycle["session"], "trusted-lifecycle-ttl");

        session.close();
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn authorization_expiry_in_one_runtime_does_not_affect_another() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let expiring = CuaDriver::create_configured(ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![SessionPermissionMode::Standard],
                compatibility_mode: SessionPermissionMode::Standard,
                compatibility_capability_manifest_path: None,
                compatibility_bounded_manifest_path: None,
                unrestricted_acknowledged: false,
                max_session_ttl_seconds: 2,
                max_idle_ttl_seconds: 2,
            },
        })
        .unwrap();
        let stable = configured_standard_driver();
        let expiring_session = expiring
            .create_trusted_session(TrustedSessionOptions {
                public_session: "expiry-isolation".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 1,
                idle_ttl_seconds: 1,
                capability_manifest_path: None,
                bounded_manifest_path: None,
            })
            .unwrap();
        let stable_session = stable
            .create_trusted_session(TrustedSessionOptions {
                public_session: "expiry-isolation".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                capability_manifest_path: None,
                bounded_manifest_path: None,
            })
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        let expired = expiring_session
            .call_tool("health_report".into(), "{}".into())
            .await
            .unwrap();
        assert_eq!(expired.error_code.as_deref(), Some("permission_denied"));
        let live = stable_session
            .call_tool("health_report".into(), "{}".into())
            .await
            .unwrap();
        assert_ne!(live.error_code.as_deref(), Some("permission_denied"));

        stable_session.close();
        expiring_session.close();
        stable.shutdown().await.unwrap();
        expiring.shutdown().await.unwrap();
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
                    capability_manifest_path: None,
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
                compatibility_capability_manifest_path: None,
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
                capability_manifest_path: None,
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
                capability_manifest_path: None,
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
                capability_manifest_path: None,
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
    #[cfg(unix)]
    async fn typed_desktop_call_serializes_contract_and_normalizes_result() {
        let response = serde_json::json!({
            "ok": true,
            "result": {
                "content": [
                    {"type": "text", "text": "captured"},
                    {"type": "image", "mimeType": "image/png", "data": "cG5n"}
                ],
                "structuredContent": {"screenshot_width": 2},
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
        assert!(result.action().is_none());
        assert!(result.verification().is_none());

        let request = server.join().unwrap();
        assert_eq!(request["method"], "call");
        assert_eq!(request["name"], "get_desktop_state");
        assert_eq!(request["args"], serde_json::json!({"session": "run-1"}));
        assert_eq!(request["observation_origin"], "direct");
        assert_eq!(request["client_kind"], "python_sdk");
    }

    #[test]
    fn result_normalization_exposes_typed_action_and_verification_views() {
        let action = normalize_result(
            "type_text",
            serde_json::json!({
                "content": [{"type": "text", "text": "Action outcome"}],
                "structuredContent": {
                    "effect": "confirmed",
                    "route": "accessibility",
                    "delivery": {"mode": "background"},
                    "evidence": [{"kind": "value_readback"}]
                },
                "isError": false
            }),
        )
        .unwrap();
        assert_eq!(
            action.action().map(|value| value.effect),
            Some(cua_driver_contract::ActionEffect::Confirmed)
        );
        assert!(action.verification().is_none());

        let verification = normalize_result(
            "verify_state",
            serde_json::json!({
                "content": [{"type": "text", "text": "satisfied"}],
                "structuredContent": {
                    "status": "satisfied",
                    "stable": true,
                    "elapsed_ms": 42,
                    "samples": 2,
                    "predicates": []
                },
                "isError": false
            }),
        )
        .unwrap();
        assert_eq!(
            verification.verification().map(|value| value.status),
            Some(cua_driver_contract::VerificationStatus::Satisfied)
        );
        assert!(verification.action().is_none());
    }

    #[test]
    fn result_normalization_rejects_an_unsubstantiated_confirmation() {
        let error = normalize_result(
            "browser_click",
            serde_json::json!({
                "content": [{"type": "text", "text": "claimed success"}],
                "structuredContent": {
                    "effect": "confirmed",
                    "route": "trusted_input"
                },
                "isError": false
            }),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DriverError::Protocol { reason }
                if reason.contains("confirmed effect requires evidence")
        ));
    }

    #[test]
    fn result_normalization_fails_closed_on_legacy_or_missing_action_payloads() {
        let legacy = normalize_result(
            "click",
            serde_json::json!({
                "content": [{"type": "text", "text": "legacy click"}],
                "structuredContent": {
                    "path": "ax",
                    "verified": true
                },
                "isError": false
            }),
        )
        .unwrap_err();
        assert!(matches!(
            legacy,
            DriverError::Protocol { reason }
                if reason.contains("invalid ActionResult")
        ));

        let missing = normalize_result(
            "click",
            serde_json::json!({
                "content": [{"type": "text", "text": "missing payload"}],
                "isError": false
            }),
        )
        .unwrap_err();
        assert!(matches!(
            missing,
            DriverError::Protocol { reason }
                if reason.contains("omitted ActionResult")
        ));
    }

    #[tokio::test]
    #[cfg(unix)]
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
    #[cfg(unix)]
    async fn daemon_version_mismatch_is_refused_before_action_dispatch() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("incompatible.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let action_dispatched = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_action = action_dispatched.clone();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "metadata");
            let mut metadata = cua_driver_core::daemon::current_daemon_metadata();
            metadata.contract_version = "incompatible-test-contract".into();
            let mut writer = stream;
            writeln!(
                writer,
                "{}",
                serde_json::json!({"ok": true, "result": metadata})
            )
            .unwrap();

            listener.set_nonblocking(true).unwrap();
            for _ in 0..20 {
                match listener.accept() {
                    Ok(_) => {
                        observed_action.store(true, std::sync::atomic::Ordering::Release);
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept after incompatible metadata: {error}"),
                }
            }
        });
        let driver = CuaDriver::connect(Some(socket.to_string_lossy().into_owned())).unwrap();
        let error = driver
            .call_tool("health_report".into(), "{}".into())
            .await
            .unwrap_err();
        assert!(matches!(error, DriverError::Protocol { .. }));
        assert!(error.to_string().contains("incompatible daemon"));
        server.join().unwrap();
        assert!(
            !action_dispatched.load(std::sync::atomic::Ordering::Acquire),
            "tool request crossed the compatibility gate"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn session_method_returns_the_canonical_typed_output() {
        let response = serde_json::json!({
            "ok": true,
            "result": {
                "content": [{"type": "text", "text": "started"}],
                "structuredContent": {
                    "session": "run-2",
                    "capture_scope": "auto",
                    "effective_scope": "window",
                    "desktop_capture_authorized": false,
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
                session: Some("run-2".into()),
                capture_scope: Some(cua_driver_contract::CaptureScope::Auto),
                cursor_theme: None,
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
        async fn negotiate(&self) -> Result<remote::DriverChannelCapabilities, String> {
            Ok(remote::DriverChannelCapabilities {
                minimum_envelope_version: remote::DRIVER_ENVELOPE_VERSION,
                maximum_envelope_version: remote::DRIVER_ENVELOPE_VERSION,
                supports_cancellation: true,
            })
        }

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

        async fn cancel(&self, _request_id: &str) -> Result<(), String> {
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
                capability_manifest_path: None,
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
        async fn negotiate(&self) -> Result<remote::DriverChannelCapabilities, String> {
            Ok(remote::DriverChannelCapabilities {
                minimum_envelope_version: remote::DRIVER_ENVELOPE_VERSION,
                maximum_envelope_version: remote::DRIVER_ENVELOPE_VERSION,
                supports_cancellation: true,
            })
        }

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

        async fn cancel(&self, _request_id: &str) -> Result<(), String> {
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

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn direct_macos_runtime_reports_cursor_overlay_facility_unavailable() {
        let _runtime_test = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        // This test is about the unavailable host-owned cursor facility, not
        // protected input admission. Use an explicitly acknowledged
        // unrestricted runtime so the call reaches that platform invariant.
        let driver = configured_unrestricted_driver();
        for (tool, arguments) in [
            (
                "set_agent_cursor_enabled",
                serde_json::json!({"enabled": true, "session": "direct-test"}),
            ),
            (
                "get_agent_cursor_state",
                serde_json::json!({"session": "direct-test"}),
            ),
            (
                "move_cursor",
                serde_json::json!({
                    "x": 10,
                    "y": 20,
                    "scope": "window",
                    "session": "direct-test"
                }),
            ),
        ] {
            let result = driver
                .call_tool(tool.into(), arguments.to_string())
                .await
                .unwrap();
            assert!(result.is_error, "{tool} reported false success");
            assert_eq!(
                result.error_code.as_deref(),
                Some("facility_unavailable"),
                "{tool} returned the wrong structured refusal"
            );
        }
        let permissions = driver
            .call_tool(
                "check_permissions".into(),
                serde_json::json!({"prompt": true}).to_string(),
            )
            .await
            .unwrap();
        let structured: Value =
            serde_json::from_str(permissions.structured_json.as_deref().unwrap()).unwrap();
        assert_eq!(structured["direct_capture_status"], "not_checked");
        assert_eq!(structured["screen_recording_capturable"], Value::Null);
        assert_eq!(structured["source"]["attribution"], "host");
        assert_eq!(structured["source"]["direct_runtime"], true);
        driver.shutdown().await.unwrap();
    }

    struct IncompatibleRemoteChannel;

    #[async_trait::async_trait]
    impl remote::DriverEnvelopeChannel for IncompatibleRemoteChannel {
        async fn negotiate(&self) -> Result<remote::DriverChannelCapabilities, String> {
            Ok(remote::DriverChannelCapabilities {
                minimum_envelope_version: remote::DRIVER_ENVELOPE_VERSION + 1,
                maximum_envelope_version: remote::DRIVER_ENVELOPE_VERSION + 1,
                supports_cancellation: true,
            })
        }

        async fn exchange(
            &self,
            _request: remote::DriverRequestEnvelope,
        ) -> Result<remote::DriverResponseEnvelope, String> {
            panic!("incompatible carrier must be refused before envelope dispatch")
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

        async fn cancel(&self, _request_id: &str) -> Result<(), String> {
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
    async fn remote_carrier_negotiates_before_dispatch() {
        let driver = CuaDriver::connect_remote(Arc::new(IncompatibleRemoteChannel)).unwrap();
        let error = driver.metadata().await.unwrap_err();
        assert!(matches!(error, DriverError::Protocol { .. }));
        assert!(error.to_string().contains("outside carrier range"));
        let error = match driver
            .create_remote_trusted_session(TrustedSessionOptions {
                public_session: "must-not-bind".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                capability_manifest_path: None,
                bounded_manifest_path: None,
            })
            .await
        {
            Ok(_) => panic!("incompatible carrier unexpectedly bound a session"),
            Err(error) => error,
        };
        assert!(matches!(error, DriverError::Protocol { .. }));
    }

    struct LegacyRemoteChannel;

    #[async_trait::async_trait]
    impl remote::DriverEnvelopeChannel for LegacyRemoteChannel {
        async fn exchange(
            &self,
            _request: remote::DriverRequestEnvelope,
        ) -> Result<remote::DriverResponseEnvelope, String> {
            panic!("legacy carrier must be refused before envelope dispatch")
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
            "legacy-principal"
        }

        fn connection_generation(&self) -> &str {
            "legacy-generation"
        }
    }

    #[tokio::test]
    async fn legacy_remote_trait_implementors_compile_but_fail_closed_before_dispatch() {
        let driver = CuaDriver::connect_remote(Arc::new(LegacyRemoteChannel)).unwrap();
        let error = driver.metadata().await.unwrap_err();
        assert!(matches!(error, DriverError::Protocol { .. }));
        assert!(error
            .to_string()
            .contains("does not support request cancellation"));
    }

    struct CancellableRemoteChannel {
        started: Arc<tokio::sync::Notify>,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl remote::DriverEnvelopeChannel for CancellableRemoteChannel {
        async fn negotiate(&self) -> Result<remote::DriverChannelCapabilities, String> {
            Ok(remote::DriverChannelCapabilities {
                minimum_envelope_version: remote::DRIVER_ENVELOPE_VERSION,
                maximum_envelope_version: remote::DRIVER_ENVELOPE_VERSION,
                supports_cancellation: true,
            })
        }

        async fn exchange(
            &self,
            _request: remote::DriverRequestEnvelope,
        ) -> Result<remote::DriverResponseEnvelope, String> {
            self.started.notify_one();
            std::future::pending().await
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

        async fn cancel(&self, _request_id: &str) -> Result<(), String> {
            self.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
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
    async fn dropping_remote_action_future_cancels_its_request_identity() {
        let started = Arc::new(tokio::sync::Notify::new());
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let driver = CuaDriver::connect_remote(Arc::new(CancellableRemoteChannel {
            started: started.clone(),
            cancelled: cancelled.clone(),
        }))
        .unwrap();
        let action_driver = driver.clone();
        let action =
            tokio::spawn(async move { action_driver.call_tool("click".into(), "{}".into()).await });
        started.notified().await;
        action.abort();
        let _ = action.await;
        for _ in 0..50 {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("remote request cancellation was not forwarded");
    }
}
