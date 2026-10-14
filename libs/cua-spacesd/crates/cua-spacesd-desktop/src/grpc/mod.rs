// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Desktop services of cua-spacesd: `ComputerService`, `WindowsService`,
//! `AccessibilityService`, `StreamService`, `PresenceService`, and the
//! ticket-authenticated `/media` WebSocket (rcdp wire v2).
//!
//! [`DesktopServiceProvider`] plugs into the server core through the
//! `cua_spacesd_server::ServiceProvider` extension point. Platform work goes through a
//! [`backend::DesktopBackend`]; the media plane is `cua_spacesd_session::media`.

mod accessibility;
pub mod backend;
mod computer;
pub mod cursor_shape;
pub mod keys;
pub mod media_quic;
mod media_ws;
pub mod presence;
pub mod presence_quic;
mod status;
mod stream;
pub mod tool_backend;
mod windows;

#[cfg(target_os = "linux")]
pub mod linux_backend;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cua_proto::env::v1::{
    accessibility_service_server::AccessibilityServiceServer,
    computer_service_server::ComputerServiceServer, presence_service_server::PresenceServiceServer,
    stream_service_server::StreamServiceServer, windows_service_server::WindowsServiceServer,
    ComponentHealth, Display, DisplayServer, Feature, HealthStatus, PixelSize, Rect,
};
use cua_spacesd_session::media::leases::InputLeases;
use cua_spacesd_session::media::MediaRuntime;

use cua_spacesd_server::{ServerContext, ServiceProvider};
use presence::PresenceHub;
use prost::Message as _;

pub use backend::*;

/// Fully-qualified names of the services this provider registers.
pub const SERVICES: &[&str] = &[
    "cua.env.v1.ComputerService",
    "cua.env.v1.WindowsService",
    "cua.env.v1.AccessibilityService",
    "cua.env.v1.StreamService",
    "cua.env.v1.PresenceService",
];

pub(crate) struct ScreenshotRecord {
    pub id: String,
    pub logical_bounds: (f64, f64, f64, f64),
    pub image_size: (u32, u32),
    pub source: ScreenshotSource,
}

#[derive(Clone, PartialEq)]
pub(crate) enum ScreenshotSource {
    Display(String),
    Window(String),
}

pub(crate) struct A11ySnapshotRecord {
    pub window: WindowRecord,
    pub backend_snapshot: u64,
    pub created: Instant,
}

/// State shared by every desktop service.
pub(crate) struct DesktopState {
    pub backend: Arc<dyn DesktopBackend>,
    pub media: Arc<MediaRuntime>,
    pub presence: Arc<PresenceHub>,
    pub ctx: ServerContext,
    pub leases: Arc<InputLeases>,
    pub screenshots: Mutex<VecDeque<ScreenshotRecord>>,
    pub a11y: Mutex<HashMap<String, A11ySnapshotRecord>>,
    pub quic: Mutex<Option<media_quic::QuicMedia>>,
    /// Who drives the pointer (presence ownership and probe idle gating).
    pub activity: Arc<cua_spacesd_session::input_activity::InputActivity>,
    /// The presence cursor-shape prober, when a backend is installed.
    pub prober: Mutex<Option<Arc<cursor_shape::ShapeProber>>>,
}

/// Keep at least this many screenshot ids (computer.proto).
pub(crate) const SCREENSHOT_HISTORY: usize = 16;
/// Accessibility snapshots kept for `Act`.
pub(crate) const A11Y_HISTORY: usize = 64;

impl DesktopState {
    pub(crate) fn window(
        &self,
        reference: &cua_proto::env::v1::WindowRef,
    ) -> Result<WindowRecord, tonic::Status> {
        let windows = self.backend.windows().map_err(status::provider)?;
        let window = windows
            .into_iter()
            .find(|window| window.handle.0 == reference.id)
            .ok_or_else(|| {
                status::status(
                    tonic::Code::NotFound,
                    cua_proto::env::v1::ErrorReason::TargetUnavailable,
                    "window is gone",
                )
            })?;
        if reference.epoch != 0 && window.epoch.0 != reference.epoch {
            return Err(status::status(
                tonic::Code::FailedPrecondition,
                cua_proto::env::v1::ErrorReason::StaleHandle,
                format!("window epoch is stale (current {})", window.epoch.0),
            ));
        }
        Ok(window)
    }
}

/// The desktop services as one server extension.
#[derive(Clone)]
pub struct DesktopServiceProvider {
    state: Arc<DesktopState>,
}

impl DesktopServiceProvider {
    /// Build over an explicit backend (tests use a fake).
    pub fn new(backend: Arc<dyn DesktopBackend>, ctx: ServerContext) -> Self {
        let leases = Arc::new(InputLeases::default());
        let media = MediaRuntime::new(backend.media_providers(), leases.clone());
        let presence = PresenceHub::new();
        presence.spawn_flusher();
        // Root token rotated by SystemService.Init: close media sockets 4401.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let mut rotations = ctx.token_rotations();
            let media = media.clone();
            handle.spawn(async move {
                while rotations.changed().await.is_ok() {
                    media.close_all(cua_media_protocol::v2::close_code::TICKET_INVALID);
                }
            });
        }
        Self {
            state: Arc::new(DesktopState {
                backend,
                media,
                presence,
                ctx,
                leases,
                screenshots: Mutex::new(VecDeque::new()),
                a11y: Mutex::new(HashMap::new()),
                quic: Mutex::new(None),
                activity: cua_spacesd_session::input_activity::global().clone(),
                prober: Mutex::new(None),
            }),
        }
    }

    /// Compute presence cursor shapes with `backend` (the platform's on a
    /// real desktop, a fake in tests). Idempotent per provider.
    pub fn with_pointer_shapes(
        &self,
        backend: Arc<dyn cua_driver_core::pointer_shape::PointerShapeBackend>,
    ) -> &Self {
        let mut prober = self.state.prober.lock().unwrap();
        if prober.is_none() {
            let ctx = self.state.ctx.clone();
            *prober = Some(cursor_shape::ShapeProber::start(
                &self.state.presence,
                backend,
                Arc::new(DesktopMapper::new(Arc::downgrade(&self.state))),
                self.state.activity.clone(),
                Arc::new(move || ctx.cursor_probe()),
                cursor_shape::ProberConfig::default(),
            ));
        }
        drop(prober);
        self
    }

    /// Install this platform's pointer-shape backend and start the prober
    /// with it (real desktops only: it hit-tests and may move the pointer).
    fn install_pointer_shapes(&self) {
        if let Some(backend) = cursor_shape::install_platform_backend() {
            self.with_pointer_shapes(backend);
        }
    }

    /// Build over this machine's desktop: the X11 backend on Linux, the
    /// driver-tool backend over the rcdp media providers on macOS/Windows.
    pub fn for_host(ctx: ServerContext) -> Result<Self, cua_spacesd_provider_api::ProviderError> {
        #[cfg(target_os = "linux")]
        {
            if crate::wayland_session() {
                return Self::for_wayland(
                    ctx,
                    crate::codec_audio::CodecAudioProvider::new().map(|audio| {
                        Arc::new(audio) as Arc<dyn cua_spacesd_session::media::audio::AudioProvider>
                    }),
                );
            }
            let backend = Arc::new(linux_backend::LinuxBackend::new()?);
            let provider = Self::new(backend, ctx);
            provider.install_agent_cursor_hook();
            provider.install_pointer_shapes();
            Ok(provider)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let audio = crate::codec_audio::CodecAudioProvider::new().map(|audio| {
                Arc::new(audio) as Arc<dyn cua_spacesd_session::media::audio::AudioProvider>
            });
            let platform = if cfg!(target_os = "macos") {
                "macOS"
            } else {
                "Windows"
            };
            let backend = Arc::new(tool_backend::ToolBackend::for_bundle(
                crate::CuaProviderBundle::new()?,
                audio,
                platform,
            ));
            let provider = Self::new(backend, ctx);
            provider.install_agent_cursor_hook();
            provider.install_pointer_shapes();
            Ok(provider)
        }
    }

    /// Like [`Self::for_host`] with an explicit audio provider (Linux).
    #[cfg(target_os = "linux")]
    pub fn for_host_with_audio(
        ctx: ServerContext,
        audio: Option<Arc<dyn cua_spacesd_session::media::audio::AudioProvider>>,
    ) -> Result<Self, cua_spacesd_provider_api::ProviderError> {
        if crate::wayland_session() {
            return Self::for_wayland(ctx, audio);
        }
        let backend = Arc::new(linux_backend::LinuxBackend::with_audio(audio)?);
        let provider = Self::new(backend, ctx);
        provider.install_agent_cursor_hook();
        provider.install_pointer_shapes();
        Ok(provider)
    }

    /// A Hyprland (Wayland) session: the X11 backend would see only XWayland,
    /// so displays, windows and capture come from the Hyprland providers
    /// (`hyprctl`, `grim`) and every action from cua-driver's tools, which
    /// route Wayland input (virtual pointer and keyboard, the
    /// cua-hyprland-plugin seats) and accessibility themselves.
    #[cfg(target_os = "linux")]
    fn for_wayland(
        ctx: ServerContext,
        audio: Option<Arc<dyn cua_spacesd_session::media::audio::AudioProvider>>,
    ) -> Result<Self, cua_spacesd_provider_api::ProviderError> {
        let backend = Arc::new(
            tool_backend::ToolBackend::for_bundle(
                crate::CuaProviderBundle::new()?,
                audio,
                "Linux Wayland",
            )
            .with_text_clipboard(crate::wayland_clipboard())
            .with_window_manager(crate::wayland_window_manager()),
        );
        let provider = Self::new(backend, ctx);
        provider.install_agent_cursor_hook();
        provider.install_pointer_shapes();
        Ok(provider)
    }

    /// Forward cua-driver cursor moves (the agent cursor overlay hook) into
    /// presence as agent cursors. Moves of a human-origin driver session
    /// (a viewer's media input) are not agent cursors and are skipped.
    pub fn install_agent_cursor_hook(&self) {
        // A driver session (an agent run keyed by X-Cua-Agent-Session, or an
        // MCP session) that ends takes its agent cursor with it.
        let ended = Arc::downgrade(&self.state);
        cua_driver_core::session::register_session_end_hook(move |session_id| {
            if let Some(state) = ended.upgrade() {
                state
                    .presence
                    .agent_ended(&format!("cua-driver:{session_id}"));
            }
        });
        let state = Arc::downgrade(&self.state);
        cua_driver_core::cursor_hook::set_cursor_hook_fn(move |event| {
            if let Some(state) = state.upgrade() {
                publish_driver_cursor(&state, &event);
            }
        });
    }

    /// Close every media socket with 4401 and drop tickets (root token
    /// rotated). The server core calls this from `SystemService.Init`.
    pub fn revoke_media(&self) {
        // Tickets are already invalid (their key derives from the token).
        self.state
            .media
            .close_all(cua_media_protocol::v2::close_code::TICKET_INVALID);
    }

    /// Start the direct QUIC media listener (normally UDP 3212). OpenMedia
    /// advertises it (`quic`) when a client sets `prefer_quic`.
    pub fn start_quic(
        &self,
        address: std::net::SocketAddr,
    ) -> Result<media_quic::QuicMedia, String> {
        let media = media_quic::start(self.state.clone(), address)?;
        *self.state.quic.lock().unwrap() = Some(media.clone());
        Ok(media)
    }

    /// [`Self::start_quic`] on an already-bound UDP socket.
    pub fn start_quic_on(
        &self,
        socket: std::net::UdpSocket,
    ) -> Result<media_quic::QuicMedia, String> {
        let media = media_quic::start_on(self.state.clone(), socket)?;
        *self.state.quic.lock().unwrap() = Some(media.clone());
        Ok(media)
    }

    pub fn media_runtime(&self) -> &Arc<MediaRuntime> {
        &self.state.media
    }

    pub fn presence(&self) -> &Arc<PresenceHub> {
        &self.state.presence
    }

    /// An axum router serving every desktop service (gRPC) plus `/media`.
    /// Convenience for tests and for binaries without the server core.
    pub fn standalone_router(&self) -> axum::Router {
        let routes = self.register(tonic::service::Routes::default(), &self.state.ctx);
        let mut router = routes.into_axum_router();
        if let Some(http) = self.http_routes() {
            router = router.merge(http);
        }
        router
    }
}

impl FeatureStatus {
    fn into_feature(self) -> Feature {
        feature(self)
    }
}

fn feature(status: FeatureStatus) -> Feature {
    Feature {
        name: status.name.to_owned(),
        supported: status.supported,
        limitation: status.limitation,
        attributes: status.attributes.into_iter().collect(),
    }
}

impl ServiceProvider for DesktopServiceProvider {
    fn capabilities(&self) -> Vec<Feature> {
        let mut features: Vec<Feature> = self
            .state
            .backend
            .features()
            .into_iter()
            .map(feature)
            .collect();
        features.push(feature(FeatureStatus::yes("presence")));
        let prober = self.state.prober.lock().unwrap().clone();
        features.push(match prober {
            Some(prober) if prober.names().hit_test.is_empty() && prober.names().system.is_empty() => {
                feature(FeatureStatus::no(
                    "presence.cursor_shape",
                    prober.limitation().unwrap_or_else(|| {
                        "this desktop exposes no cursor shape or accessibility hit-test".into()
                    }),
                ))
            }
            Some(prober) => Feature {
                name: "presence.cursor_shape".into(),
                supported: true,
                limitation: prober.limitation().unwrap_or_default(),
                attributes: cursor_shape::capability_attributes(
                    prober.names(),
                    prober.probe_enabled(),
                )
                .into_iter()
                .collect(),
            },
            None => feature(FeatureStatus::no(
                "presence.cursor_shape",
                "this cua-spacesd has no pointer-shape backend for its desktop; cursors are drawn as arrows",
            )),
        });
        let quic = self.state.quic.lock().unwrap().clone();
        features.push(match quic {
            Some(quic) => FeatureStatus {
                attributes: vec![("port".into(), quic.port.to_string())],
                ..FeatureStatus::yes("quic_media")
            },
            None => FeatureStatus::no("quic_media", "the QUIC media listener is not running (--quic-port 0); use the /media WebSocket"),
        }.into_feature());
        features
    }

    fn register(
        &self,
        routes: tonic::service::Routes,
        _ctx: &ServerContext,
    ) -> tonic::service::Routes {
        let limit = cua_spacesd_server::config::MAX_MESSAGE_BYTES as usize;
        routes
            .add_service(
                ComputerServiceServer::new(computer::Computer(self.state.clone()))
                    .max_decoding_message_size(limit),
            )
            .add_service(
                WindowsServiceServer::new(windows::Windows(self.state.clone()))
                    .max_decoding_message_size(limit),
            )
            .add_service(
                AccessibilityServiceServer::new(accessibility::Accessibility(self.state.clone()))
                    .max_decoding_message_size(limit),
            )
            .add_service(
                StreamServiceServer::new(stream::Stream(self.state.clone()))
                    .max_decoding_message_size(limit),
            )
            .add_service(
                PresenceServiceServer::new(presence::Presence(self.state.clone()))
                    .max_decoding_message_size(limit),
            )
    }

    fn http_routes(&self) -> Option<axum::Router> {
        Some(media_ws::router(self.state.clone()))
    }

    fn services(&self) -> Vec<&'static str> {
        SERVICES.to_vec()
    }

    fn http_paths(&self) -> Vec<&'static str> {
        vec![cua_proto::metadata::MEDIA_WS_PATH]
    }

    fn displays(&self) -> Vec<Display> {
        self.state
            .backend
            .displays()
            .unwrap_or_default()
            .into_iter()
            .map(proto_display)
            .collect()
    }

    fn health(&self) -> Vec<ComponentHealth> {
        let state = self.state.clone();
        // Bounded: an X server that stops answering must not hang Health.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(state.backend.desktop_ready());
        });
        let ready = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap_or_else(|_| Err("the display did not answer within 2 s".into()));
        vec![desktop_component(ready)]
    }

    fn tool_provider(&self) -> Option<Arc<dyn cua_driver_core::server::ToolProvider>> {
        // Share the provider bundle's registry so the Driver service, /mcp
        // and the desktop services drive one cua-driver (one cursor overlay).
        self.state.backend.tool_registry().map(|registry| {
            Arc::new(InputTools {
                inner: registry as Arc<dyn cua_driver_core::server::ToolProvider>,
                activity: self.state.activity.clone(),
            }) as Arc<dyn cua_driver_core::server::ToolProvider>
        })
    }

    fn display_server(&self) -> Option<DisplayServer> {
        #[cfg(target_os = "linux")]
        return Some(
            if std::env::var_os("WAYLAND_DISPLAY").is_some()
                && std::env::var_os("DISPLAY").is_none()
            {
                DisplayServer::Wayland
            } else {
                DisplayServer::X11
            },
        );
        #[cfg(target_os = "macos")]
        return Some(DisplayServer::Quartz);
        #[cfg(target_os = "windows")]
        return Some(DisplayServer::Win32);
        #[allow(unreachable_code)]
        None
    }
}

/// The driver tool registry as `/mcp` and the Driver service see it: input
/// tools announce themselves to presence (pointer ownership, probe idle
/// gating) and hold the pointer lock while they run.
struct InputTools {
    inner: Arc<dyn cua_driver_core::server::ToolProvider>,
    activity: Arc<cua_spacesd_session::input_activity::InputActivity>,
}

/// Publish one cua-driver cursor move into presence as an agent cursor.
///
/// A human viewer's media input relayed through the driver's tools is
/// skipped: the viewer's client draws and publishes the viewer's own cursor,
/// and the media path already records the viewer as the pointer's owner.
/// Publishing it here would add a second cursor, named as an agent.
pub(crate) fn publish_driver_cursor(
    state: &DesktopState,
    event: &cua_driver_core::cursor_hook::CursorHookEvent,
) {
    if !agent_cursor_event(event) {
        return;
    }
    state.activity.note(
        &format!("cua-driver:{}", event.cursor_id),
        Some((event.x, event.y)),
    );
    let Ok(displays) = state.backend.displays() else {
        return;
    };
    let Some(display) = displays
        .iter()
        .find(|display| display.primary)
        .or(displays.first())
    else {
        return;
    };
    let (x, y, width, height) = display.bounds;
    state.presence.agent_cursor(
        &format!("cua-driver:{}", event.cursor_id),
        "CUA agent",
        &display.id,
        None,
        (
            (event.x - x) / width.max(1.0),
            (event.y - y) / height.max(1.0),
        ),
    );
}

/// Whether a cua-driver cursor event is an agent's: not a human viewer's
/// media input relayed through the driver (see
/// `cua_driver_core::agent_cursor::InputOrigin`).
pub(crate) fn agent_cursor_event(event: &cua_driver_core::cursor_hook::CursorHookEvent) -> bool {
    cua_driver_core::agent_cursor::input_origin(&event.cursor_id)
        == cua_driver_core::agent_cursor::InputOrigin::Agent
}

/// Whether a driver tool injects input (so it must not overlap a probe).
pub(crate) fn injects_input(tool: &str) -> bool {
    const INPUT: &[&str] = &[
        "click", "type", "press", "key", "scroll", "drag", "move", "mouse", "hotkey",
    ];
    let tool = tool.to_ascii_lowercase();
    INPUT.iter().any(|word| tool.contains(word))
}

#[tonic::async_trait]
impl cua_driver_core::server::ToolProvider for InputTools {
    fn tools_list(&self) -> serde_json::Value {
        self.inner.tools_list()
    }

    async fn invoke_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if !injects_input(name) {
            return self.inner.invoke_tool(name, arguments).await;
        }
        let session = arguments["_session_id"].as_str().unwrap_or("default");
        let guard = self
            .activity
            .announce(&format!("cua-driver:{session}"))
            .acquire()
            .await;
        let result = self.inner.invoke_tool(name, arguments).await;
        drop(guard);
        result
    }
}

/// Presence cursor positions to global screen points, from the backend's
/// displays and windows (window list cached briefly: the prober asks often).
struct DesktopMapper {
    state: std::sync::Weak<DesktopState>,
    windows: Mutex<Option<(Instant, Vec<WindowRecord>)>>,
}

impl DesktopMapper {
    fn new(state: std::sync::Weak<DesktopState>) -> Self {
        Self {
            state,
            windows: Mutex::new(None),
        }
    }
}

impl cursor_shape::ScreenMapper for DesktopMapper {
    fn to_screen(&self, cursor: &cua_proto::env::v1::CursorPosition) -> Option<(f64, f64, String)> {
        let state = self.state.upgrade()?;
        let point = cursor.position.as_ref()?;
        if let Some(window) = &cursor.window {
            let mut cache = self.windows.lock().unwrap();
            let fresh = cache
                .as_ref()
                .is_some_and(|(at, _)| at.elapsed() < std::time::Duration::from_millis(250));
            if !fresh {
                *cache = Some((Instant::now(), state.backend.windows().ok()?));
            }
            let record = cache
                .as_ref()?
                .1
                .iter()
                .find(|record| record.handle.0 == window.id)?
                .clone();
            let (x, y, width, height) = record.bounds;
            return Some((
                x + point.x * width,
                y + point.y * height,
                format!("window:{}", window.id),
            ));
        }
        let displays = state.backend.displays().ok()?;
        let display = displays
            .iter()
            .find(|display| display.id == cursor.display_id)
            .or_else(|| displays.iter().find(|display| display.primary))
            .or(displays.first())?;
        let (x, y, width, height) = display.bounds;
        Some((
            x + point.x * width,
            y + point.y * height,
            format!("display:{}", display.id),
        ))
    }
}

/// The `desktop` component of `Health` (the name clients look for:
/// `cua_spacesd_client::DESKTOP_COMPONENT`).
pub(crate) fn desktop_component(ready: Result<(), String>) -> ComponentHealth {
    match ready {
        Ok(()) => ComponentHealth {
            name: "desktop".into(),
            status: HealthStatus::Serving as i32,
            detail: String::new(),
        },
        Err(detail) => ComponentHealth {
            name: "desktop".into(),
            status: HealthStatus::NotServing as i32,
            detail,
        },
    }
}

pub(crate) fn proto_display(display: cua_spacesd_provider_api::ProviderDisplay) -> Display {
    let (x, y, width, height) = display.bounds;
    Display {
        id: display.id,
        name: display.name,
        primary: display.primary,
        bounds: Some(Rect {
            x,
            y,
            width,
            height,
        }),
        native_size: Some(PixelSize {
            width: display.native_width_px,
            height: display.native_height_px,
        }),
        scale_factor: display.scale_factor,
        refresh_rate_hz: display.refresh_rate_hz,
        rotation_degrees: 0,
    }
}

/// The caller's principal: the server core's authenticated caller when the
/// request passed its auth layer, else the `x-cua-principal-bin` metadata
/// (standalone routers in tests).
pub(crate) fn caller_principal<T>(
    request: &tonic::Request<T>,
) -> Option<cua_proto::env::v1::Principal> {
    cua_spacesd_server::caller(request).principal.or_else(|| {
        let value = request
            .metadata()
            .get_bin(cua_proto::metadata::PRINCIPAL_BIN)?;
        let bytes = value.to_bytes().ok()?;
        cua_proto::env::v1::Principal::decode(bytes.as_ref()).ok()
    })
}

pub(crate) fn timestamp(time: std::time::SystemTime) -> cua_proto::wkt::Timestamp {
    let since = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    cua_proto::wkt::Timestamp {
        seconds: since.as_secs() as i64,
        nanos: since.subsec_nanos() as i32,
    }
}

pub(crate) fn random_id(prefix: &str) -> String {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).expect("system randomness");
    format!(
        "{prefix}-{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests;
