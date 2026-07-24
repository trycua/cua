//! Private platform-runtime composition behind the public SDK boundary.
//!
//! This is deliberately not a second public interface. Both imported SDK
//! objects and the daemon host construct the same runtime here; transport
//! adapters are downstream consumers of `CuaDriver`.

use cua_driver_core::{
    authorization::PermissionMode,
    protocol::ToolResult as CoreToolResult,
    session_authorization::{
        AuthenticatedActionConnection, DelegatedSessionRequest, EffectiveAuthorizationContext,
        SessionAuthorizationError, SessionAuthorizationRegistry, SessionModeCeiling,
        TrustedHostLease,
    },
    session_manifest::SessionManifest,
    tool::ToolRegistry,
};
use cursor_overlay::CursorConfig;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

const RECORDING_IDLE_TTL_SECS_DEFAULT: u64 = 300;
const SESSION_IDLE_TTL_SECS_DEFAULT: u64 = 300;
static DIRECT_RUNTIME_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
pub(crate) static TEST_RUNTIME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RuntimeCreateError {
    #[error(
        "runtime_already_exists: one direct Cua Driver runtime is already active in this process"
    )]
    AlreadyExists,
    #[error("invalid runtime authorization configuration: {0}")]
    Authorization(String),
}

struct RuntimeOwnershipGuard {
    released: AtomicBool,
}

impl RuntimeOwnershipGuard {
    fn acquire() -> Result<Self, RuntimeCreateError> {
        DIRECT_RUNTIME_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| RuntimeCreateError::AlreadyExists)?;
        Ok(Self {
            released: AtomicBool::new(false),
        })
    }

    fn release(&self) {
        if !self.released.swap(true, Ordering::AcqRel) {
            DIRECT_RUNTIME_ACTIVE.store(false, Ordering::Release);
        }
    }
}

impl Drop for RuntimeOwnershipGuard {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeOptions {
    pub cursor: CursorConfig,
    pub compatibility_mode: bool,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub prepare_desktop_environment: bool,
    pub register_host_tools: Option<fn(&mut ToolRegistry)>,
    pub authorization_ceiling: Option<SessionModeCeiling>,
    pub compatibility_authorization: Option<(PermissionMode, Option<Arc<SessionManifest>>)>,
}

impl RuntimeOptions {
    pub(crate) fn embedded(compatibility_mode: bool) -> Self {
        Self {
            cursor: CursorConfig {
                enabled: false,
                ..CursorConfig::default()
            },
            compatibility_mode,
            prepare_desktop_environment: true,
            register_host_tools: None,
            authorization_ceiling: None,
            compatibility_authorization: None,
        }
    }

    pub(crate) fn embedded_with_ceiling(
        compatibility_mode: bool,
        authorization_ceiling: SessionModeCeiling,
        compatibility_permission_mode: PermissionMode,
        compatibility_manifest: Option<Arc<SessionManifest>>,
    ) -> Self {
        Self {
            authorization_ceiling: Some(authorization_ceiling),
            compatibility_authorization: Some((
                compatibility_permission_mode,
                compatibility_manifest,
            )),
            ..Self::embedded(compatibility_mode)
        }
    }
}

pub(crate) struct RuntimeSession {
    runtime: Arc<DriverRuntime>,
    authorization_registry: Arc<SessionAuthorizationRegistry>,
    host: TrustedHostLease,
    connection: AuthenticatedActionConnection,
    context: Arc<EffectiveAuthorizationContext>,
    public_session: String,
    transport_session: String,
}

impl RuntimeSession {
    pub(crate) async fn invoke(&self, name: &str, mut args: Value) -> Option<CoreToolResult> {
        cua_driver_core::tool_args::sanitize_reserved_args(&mut args);
        let Some(arguments) = args.as_object_mut() else {
            return Some(permission_denied_result(
                "session-bound actions require an object argument".to_owned(),
            ));
        };
        if arguments
            .get("session")
            .and_then(Value::as_str)
            .is_some_and(|session| session != self.public_session)
        {
            return Some(permission_denied_result(
                "public session substitution does not match the bound authorization context"
                    .to_owned(),
            ));
        }
        arguments.insert(
            "session".to_owned(),
            Value::String(self.public_session.clone()),
        );
        arguments.insert(
            "_session_id".to_owned(),
            Value::String(self.public_session.clone()),
        );
        arguments.insert(
            "_transport_session_id".to_owned(),
            Value::String(self.transport_session.clone()),
        );
        let result = self
            .runtime
            .invoke_with_context(name, args, self.context.clone())
            .await;
        if name == "end_session" && result.is_some() {
            self.authorization_registry
                .revoke_connection(&self.connection);
        }
        result
    }
}

impl Drop for RuntimeSession {
    fn drop(&mut self) {
        self.authorization_registry
            .revoke_connection(&self.connection);
        self.authorization_registry.revoke_host(&self.host);
    }
}

pub(crate) struct DriverRuntime {
    registry: Arc<ToolRegistry>,
    authorization_registry: Arc<SessionAuthorizationRegistry>,
    compatibility_context: Arc<EffectiveAuthorizationContext>,
    ownership: RuntimeOwnershipGuard,
    shutdown: AtomicBool,
    last_activity: AtomicU64,
    /// Calls hold a read guard; shutdown takes the write guard after closing
    /// admission. Therefore shutdown is idempotent and does not return while a
    /// previously admitted operation is still executing.
    lifecycle: tokio::sync::RwLock<()>,
}

impl DriverRuntime {
    pub(crate) fn create(options: RuntimeOptions) -> Result<Arc<Self>, RuntimeCreateError> {
        let authorization_registry = Arc::new(match options.authorization_ceiling.clone() {
            Some(ceiling) => SessionAuthorizationRegistry::with_ceiling(ceiling),
            None => SessionAuthorizationRegistry::process()
                .map_err(RuntimeCreateError::Authorization)?,
        });
        let compatibility_context = match options.compatibility_authorization.clone() {
            Some((mode, manifest)) => authorization_registry
                .compatibility_context(mode, manifest)
                .map_err(RuntimeCreateError::Authorization)?,
            None => authorization_registry
                .legacy_context()
                .map_err(RuntimeCreateError::Authorization)?,
        };
        let ownership = RuntimeOwnershipGuard::acquire()?;
        let registry = Arc::new(build_registry(&options));
        registry.init_self_weak();
        register_recording_session_end_hook(&registry);
        let runtime = Arc::new(Self {
            registry,
            authorization_registry,
            compatibility_context,
            ownership,
            shutdown: AtomicBool::new(false),
            last_activity: AtomicU64::new(now_unix_secs()),
            lifecycle: tokio::sync::RwLock::new(()),
        });
        spawn_lifecycle_maintenance(&runtime);
        Ok(runtime)
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::Acquire)
    }

    pub(crate) async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        let _drained = self.lifecycle.write().await;
        self.authorization_registry.revoke_all();
        cua_driver_core::session::revoke_all_sessions();
        let recording = self.registry.recording.clone();
        let _ = tokio::task::spawn_blocking(move || recording.stop_owner(None)).await;
        self.ownership.release();
    }

    pub(crate) fn tools_list(&self) -> Option<Value> {
        self.is_running().then(|| self.registry.tools_list())
    }

    pub(crate) async fn invoke(&self, name: &str, args: Value) -> Option<CoreToolResult> {
        self.invoke_with_context(name, args, self.compatibility_context.clone())
            .await
    }

    async fn invoke_with_context(
        &self,
        name: &str,
        args: Value,
        context: Arc<EffectiveAuthorizationContext>,
    ) -> Option<CoreToolResult> {
        if !self.is_running() {
            return None;
        }
        self.last_activity.store(now_unix_secs(), Ordering::Relaxed);
        let _operation = self.lifecycle.read().await;
        if !self.is_running() {
            return None;
        }
        let ending_session = (name == "end_session")
            .then(|| {
                args.get("session")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten();
        let result = self.registry.invoke_with_context(name, args, context).await;
        if let Some(session) = ending_session {
            // `end_session` is a lifecycle boundary: do not report completion
            // until any recording owned by the session has finalized.
            let recording = self.registry.recording.clone();
            let _ = tokio::task::spawn_blocking(move || recording.stop_owner(Some(&session))).await;
        }
        Some(result)
    }

    pub(crate) fn create_trusted_session(
        self: &Arc<Self>,
        request: DelegatedSessionRequest,
    ) -> Result<Arc<RuntimeSession>, SessionAuthorizationError> {
        if !self.is_running() {
            return Err(SessionAuthorizationError::RuntimeUnavailable);
        }
        let public_session = request.public_session.clone();
        let transport_session = request.transport_session.clone();
        let (host, connection) = self.authorization_registry.trusted_in_process_binding();
        self.authorization_registry
            .bind_delegated_session(&host, &connection, request)?;
        let context = self.authorization_registry.resolve_delegated(
            &connection,
            &public_session,
            &transport_session,
        )?;
        Ok(Arc::new(RuntimeSession {
            runtime: self.clone(),
            authorization_registry: self.authorization_registry.clone(),
            host,
            connection,
            context,
            public_session,
            transport_session,
        }))
    }
}

impl Drop for DriverRuntime {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.authorization_registry.revoke_all();
        cua_driver_core::session::revoke_all_sessions();
        let _ = self.registry.recording.stop_owner(None);
        self.ownership.release();
    }
}

fn permission_denied_result(message: String) -> CoreToolResult {
    CoreToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": {
            "code": "permission_denied",
            "message": message,
        }
    }))
}

fn configured_ttl(name: &str, fallback: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn spawn_lifecycle_maintenance(runtime: &Arc<DriverRuntime>) {
    let runtime = Arc::downgrade(runtime);
    let recording_ttl = configured_ttl(
        "CUA_DRIVER_RS_RECORDING_IDLE_TTL_SECS",
        RECORDING_IDLE_TTL_SECS_DEFAULT,
    );
    let session_ttl = std::time::Duration::from_secs(configured_ttl(
        "CUA_DRIVER_RS_SESSION_IDLE_TTL_SECS",
        SESSION_IDLE_TTL_SECS_DEFAULT,
    ));
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(30));
        let Some(runtime) = runtime.upgrade() else {
            break;
        };
        if !runtime.is_running() {
            break;
        }
        let ended = cua_driver_core::session::evict_idle(session_ttl);
        if !ended.is_empty() {
            tracing::info!(
                count = ended.len(),
                "idle-TTL reclaimed sessions: {ended:?}"
            );
        }
        let idle = now_unix_secs().saturating_sub(runtime.last_activity.load(Ordering::Relaxed));
        if idle >= recording_ttl && runtime.registry.recording.current_state().enabled {
            tracing::warn!("recording idle {idle}s ≥ {recording_ttl}s TTL; auto-stopping");
            let _ = runtime.registry.recording.stop_owner(None);
        }
    });
}

fn register_recording_session_end_hook(registry: &Arc<ToolRegistry>) {
    let recording = Arc::downgrade(&registry.recording);
    cua_driver_core::session::register_session_end_hook(move |session| {
        let Some(recording) = recording.upgrade() else {
            return;
        };
        let session = session.to_owned();
        std::thread::spawn(move || {
            let _ = recording.stop_owner(Some(&session));
        });
    });
}

fn build_registry(options: &RuntimeOptions) -> ToolRegistry {
    #[cfg(target_os = "macos")]
    let mut registry = {
        configure_macos_runtime();
        platform_macos::register_tools_with_cursor(
            options.cursor.clone(),
            options.compatibility_mode,
        )
    };

    #[cfg(target_os = "windows")]
    let mut registry = {
        configure_windows_runtime();
        platform_windows::register_tools_with_cursor(
            options.cursor.clone(),
            options.compatibility_mode,
        )
    };

    #[cfg(target_os = "linux")]
    let mut registry = {
        configure_linux_runtime(options.prepare_desktop_environment);
        platform_linux::register_tools_with_cursor(
            options.cursor.clone(),
            options.compatibility_mode,
        )
    };

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let mut registry = {
        let _ = options;
        ToolRegistry::new()
    };

    if let Some(register_host_tools) = options.register_host_tools {
        register_host_tools(&mut registry);
    }
    registry
}

#[cfg(target_os = "macos")]
fn configure_macos_runtime() {
    cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
        if let Some(window_id) = window_id {
            platform_macos::capture::screenshot_window_bytes(window_id as u32).ok()
        } else if let Some(pid) = pid {
            platform_macos::windows::resolve_main_window_id(pid as i32)
                .ok()
                .and_then(|window_id| {
                    platform_macos::capture::screenshot_window_bytes(window_id).ok()
                })
        } else {
            platform_macos::capture::screenshot_display_bytes().ok()
        }
    });
    cua_driver_core::recording::set_click_marker_fn(|png_bytes, x, y| {
        platform_macos::capture::crosshair_png_bytes(png_bytes, x, y).ok()
    });
    cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
        platform_macos::recording_hooks::app_state_json_for(window_id, pid)
    });
    cua_driver_core::recording::set_element_bounds_fn(|window_id, pid, index| {
        platform_macos::recording_hooks::element_window_local_xy(window_id, pid, index)
    });
    cua_driver_core::video::set_video_backend_factory(Box::new(
        platform_macos::video_sckit::SckitVideoBackendFactory,
    ));
}

#[cfg(target_os = "windows")]
fn configure_windows_runtime() {
    cua_driver_core::recording::set_classified_screenshot_fn(|window_id, pid| {
        platform_windows::recording_hooks::screenshot_for_recording(window_id, pid)
    });
    cua_driver_core::recording::set_click_marker_fn(|png_bytes, x, y| {
        platform_windows::capture::crosshair_png_bytes(png_bytes, x, y).ok()
    });
    cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
        platform_windows::recording_hooks::app_state_json_for(window_id, pid)
    });
    cua_driver_core::recording::set_element_bounds_fn(|window_id, pid, index| {
        platform_windows::recording_hooks::element_window_local_xy(window_id, pid, index)
    });
    cua_driver_core::video::set_video_backend_factory(Box::new(
        cua_driver_core::video_ffmpeg::FfmpegVideoBackendFactory,
    ));
}

#[cfg(target_os = "linux")]
fn configure_linux_runtime(prepare_desktop_environment: bool) {
    if prepare_desktop_environment {
        platform_linux::xauth::ensure_xauthority_discovered();
        platform_linux::session_bus::ensure_session_bus_discovered();
        platform_linux::a11y::ensure_chromium_accessibility_enabled();
        if let Err(error) = platform_linux::atspi::ensure_listener_active() {
            tracing::warn!("could not activate the persistent AT-SPI listener: {error}");
        }
    }
    cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
        platform_linux::recording_hooks::screenshot_for_recording(window_id, pid)
    });
    cua_driver_core::recording::set_click_marker_fn(|png_bytes, x, y| {
        platform_linux::capture::crosshair_png_bytes(png_bytes, x, y).ok()
    });
    cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
        platform_linux::recording_hooks::app_state_json_for(window_id, pid)
    });
    cua_driver_core::recording::set_element_bounds_fn(|window_id, pid, index| {
        platform_linux::recording_hooks::element_window_local_xy(window_id, pid, index)
    });
    if platform_linux::wayland::is_wayland() {
        cua_driver_core::video::set_video_backend_factory(Box::new(
            platform_linux::video_wayland::WfRecorderVideoBackendFactory,
        ));
    } else {
        cua_driver_core::video::set_video_backend_factory(Box::new(
            cua_driver_core::video_ffmpeg::FfmpegVideoBackendFactory,
        ));
    }
}
