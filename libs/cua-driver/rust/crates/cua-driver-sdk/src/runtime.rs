//! Private platform-runtime composition behind the public SDK boundary.
//!
//! This is deliberately not a second public interface. Both imported SDK
//! objects and the daemon host construct the same runtime here; transport
//! adapters are downstream consumers of `CuaDriver`.

use cua_driver_core::{protocol::ToolResult as CoreToolResult, tool::ToolRegistry};
use cursor_overlay::CursorConfig;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

const RECORDING_IDLE_TTL_SECS_DEFAULT: u64 = 300;
const SESSION_IDLE_TTL_SECS_DEFAULT: u64 = 300;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeOptions {
    pub cursor: CursorConfig,
    pub compatibility_mode: bool,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub prepare_desktop_environment: bool,
    pub register_host_tools: Option<fn(&mut ToolRegistry)>,
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
        }
    }
}

pub(crate) struct DriverRuntime {
    registry: Arc<ToolRegistry>,
    shutdown: AtomicBool,
    last_activity: AtomicU64,
    /// Calls hold a read guard; shutdown takes the write guard after closing
    /// admission. Therefore shutdown is idempotent and does not return while a
    /// previously admitted operation is still executing.
    lifecycle: tokio::sync::RwLock<()>,
}

impl DriverRuntime {
    pub(crate) fn create(options: RuntimeOptions) -> Arc<Self> {
        let registry = Arc::new(build_registry(&options));
        registry.init_self_weak();
        register_recording_session_end_hook(&registry);
        let runtime = Arc::new(Self {
            registry,
            shutdown: AtomicBool::new(false),
            last_activity: AtomicU64::new(now_unix_secs()),
            lifecycle: tokio::sync::RwLock::new(()),
        });
        spawn_lifecycle_maintenance(&runtime);
        runtime
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::Acquire)
    }

    pub(crate) async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        let _drained = self.lifecycle.write().await;
        let recording = self.registry.recording.clone();
        let _ = tokio::task::spawn_blocking(move || recording.stop_owner(None)).await;
    }

    pub(crate) fn tools_list(&self) -> Option<Value> {
        self.is_running().then(|| self.registry.tools_list())
    }

    pub(crate) async fn invoke(&self, name: &str, args: Value) -> Option<CoreToolResult> {
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
        let result = self.registry.invoke(name, args).await;
        if let Some(session) = ending_session {
            // `end_session` is a lifecycle boundary: do not report completion
            // until any recording owned by the session has finalized.
            let recording = self.registry.recording.clone();
            let _ = tokio::task::spawn_blocking(move || recording.stop_owner(Some(&session))).await;
        }
        Some(result)
    }
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
