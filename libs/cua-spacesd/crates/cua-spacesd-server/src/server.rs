// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Assembles every service and route on one port and serves HTTP/1.1 and
//! h2c (native gRPC, gRPC-Web, WebSockets, plain HTTP) with graceful
//! shutdown.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::{any, get};
use axum::Router;
use cua_driver_core::server::ToolProvider;
use cua_spacesd_teleport::Receiver;
use http::header::HeaderName;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tower::{Layer, ServiceExt};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use crate::auth::AuthLayer;
use crate::context::ServerContext;
use crate::filesystem::{FilesystemServiceImpl, FsState};
use crate::http::mcp::McpState;
use crate::process::{ProcessManager, ProcessServiceImpl};
use crate::provider::{feature, register_stub, ServiceProvider, DESKTOP_SERVICES};
use crate::services::diagnose::{DiagnoseState, Diagnoser};
use crate::services::driver::{DriverServiceImpl, NO_REGISTRY};
use crate::services::system::SystemServiceImpl;
use crate::services::teleport::TeleportServiceImpl;
use crate::services::tunnel::{hotspot_ws, tunnel_ws, TunnelServiceImpl, TunnelState};
use crate::services::volume::{volume_ws, VolumeServiceImpl, VolumeShared};

/// gRPC services implemented by this crate.
/// The feature name of `SystemService.AttachRelay` / `DetachRelay`.
pub const RELAY_ATTACH_FEATURE: &str = "relay_attach";

pub const CORE_SERVICES: &[&str] = &[
    "cua.env.v1.SystemService",
    "cua.env.v1.ProcessService",
    "cua.env.v1.FilesystemService",
    "cua.env.v1.DriverService",
    "cua.env.v1.TeleportService",
    "cua.env.v1.TunnelService",
    "cua.env.v1.VolumeService",
];

/// Reflection services (not part of the contract).
pub const REFLECTION_SERVICES: &[&str] = &[
    "grpc.reflection.v1.ServerReflection",
    "grpc.reflection.v1alpha.ServerReflection",
];

/// What was registered, for the route/spec test and `--print-config`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RouteManifest {
    /// Fully-qualified gRPC service names with a real implementation.
    pub grpc_services: BTreeSet<String>,
    /// Desktop services answered by the `FAILED_PRECONDITION` stub.
    pub stubbed_services: BTreeSet<String>,
    /// Plain-HTTP paths.
    pub http_paths: BTreeSet<String>,
}

/// Builds a [`Server`].
pub struct ServerBuilder {
    ctx: ServerContext,
    providers: Vec<Arc<dyn ServiceProvider>>,
    tools: Option<Arc<dyn ToolProvider>>,
    teleport_receiver: Option<Arc<Receiver>>,
    diagnoser: Option<Arc<dyn Diagnoser>>,
}

impl ServerBuilder {
    /// Starts a builder.
    pub fn new(ctx: ServerContext) -> Self {
        Self {
            ctx,
            providers: Vec::new(),
            tools: None,
            teleport_receiver: None,
            diagnoser: None,
        }
    }

    /// Adds an extension provider (desktop services).
    pub fn provider(mut self, provider: Arc<dyn ServiceProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Sets the cua-driver tool registry (Driver service and `/mcp`). When
    /// unset, the first provider's [`ServiceProvider::tool_provider`] is used.
    pub fn tools(mut self, tools: Arc<dyn ToolProvider>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Links the doctor (`SystemService.Diagnose`). Without one, Diagnose
    /// answers FAILED_PRECONDITION.
    pub fn diagnoser(mut self, diagnoser: Arc<dyn Diagnoser>) -> Self {
        self.diagnoser = Some(diagnoser);
        self
    }

    /// Overrides the teleport receiver (tests pass a fake host and home).
    pub fn teleport_receiver(mut self, receiver: Arc<Receiver>) -> Self {
        self.teleport_receiver = Some(receiver);
        self
    }

    /// Wires every service and route.
    pub fn build(self) -> Server {
        let ctx = self.ctx;
        let providers = Arc::new(self.providers);
        let tools = self
            .tools
            .or_else(|| providers.iter().find_map(|p| p.tool_provider()));
        let receiver = self.teleport_receiver.unwrap_or_else(|| {
            let home = ctx
                .config()
                .teleport_home
                .clone()
                .or_else(crate::config::home_dir)
                .unwrap_or_else(|| std::path::PathBuf::from("/"));
            let host = cua_spacesd_teleport::default_host();
            Arc::new(Receiver::with_host(home, host))
        });
        let mut manifest = RouteManifest::default();

        // Core services.
        let processes = ProcessManager::new(ctx.clone());
        let fs = FsState::new(ctx.clone());
        let tunnel = TunnelState::new(ctx.clone());
        let volume = VolumeShared::new(ctx.clone());
        let volume_state = volume.clone();
        let teleport = TeleportServiceImpl::new(ctx.clone(), receiver);
        let mut core_features = vec![
            // Unix openpty; Windows ConPTY.
            if cfg!(any(unix, windows)) {
                feature("pty", true, "")
            } else {
                feature(
                    "pty",
                    false,
                    "PTY processes are not supported on this platform yet",
                )
            },
            feature("fs_watch", true, ""),
            feature("tunnel.forward", true, ""),
            feature("hotspot", true, ""),
            match crate::services::volume::backend() {
                Ok(backend) => {
                    let mut f = feature(crate::services::volume::FEATURE, true, "");
                    f.attributes.insert("backend".into(), backend.into());
                    f
                }
                Err(reason) => feature(crate::services::volume::FEATURE, false, &reason),
            },
            match &tools {
                Some(_) => feature("driver", true, ""),
                None => feature("driver", false, NO_REGISTRY),
            },
        ];
        for app in teleport.supported_apps() {
            core_features.push(feature(&format!("teleport.{app}"), true, ""));
        }
        // `TeleportService.WipeImport`, import ledgers and
        // `ImportOptions.expires_at_ms`.
        core_features.push(feature(crate::services::teleport::WIPE_FEATURE, true, ""));
        // `SystemService.AttachRelay` / `DetachRelay`: share a running Space
        // through a relay.
        core_features.push(feature(RELAY_ATTACH_FEATURE, true, ""));
        let system = SystemServiceImpl::new(ctx.clone(), providers.clone(), core_features)
            .with_diagnose(DiagnoseState::new(ctx.clone(), self.diagnoser));

        let reflection_v1 = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(cua_proto::FILE_DESCRIPTOR_SET)
            .build_v1()
            .expect("valid descriptor set");
        let reflection_v1alpha = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(cua_proto::FILE_DESCRIPTOR_SET)
            .build_v1alpha()
            .expect("valid descriptor set");

        let mut routes = tonic::service::Routes::new(system.into_server())
            .add_service(ProcessServiceImpl::new(processes).into_server())
            .add_service(FilesystemServiceImpl::new(fs).into_server())
            .add_service(DriverServiceImpl::new(tools.clone()).into_server())
            .add_service(teleport.into_server())
            .add_service(TunnelServiceImpl::new(tunnel.clone()).into_server())
            .add_service(VolumeServiceImpl::new(volume.clone()).into_server())
            .add_service(reflection_v1)
            .add_service(reflection_v1alpha);
        manifest.grpc_services.extend(
            CORE_SERVICES
                .iter()
                .chain(REFLECTION_SERVICES)
                .map(|s| s.to_string()),
        );

        // Extension providers, then stubs for unclaimed desktop services.
        for provider in providers.iter() {
            routes = provider.register(routes, &ctx);
            for service in provider.services() {
                assert!(
                    manifest.grpc_services.insert(service.to_owned()),
                    "gRPC service {service} registered twice"
                );
            }
        }
        for service in DESKTOP_SERVICES {
            if !manifest.grpc_services.contains(*service) {
                register_stub(&mut routes, service);
                manifest.stubbed_services.insert((*service).to_owned());
            }
        }
        // Only a host (`join` with a host policy) serves HostSpacesService.
        const HOST_SPACES: &str = "cua.env.v1.HostSpacesService";
        if !manifest.grpc_services.contains(HOST_SPACES) {
            crate::provider::register_stub_with(
                &mut routes,
                HOST_SPACES,
                crate::host_spaces::HOST_SPACES_FEATURE,
                crate::provider::NOT_A_SPACES_HOST,
            );
            manifest.stubbed_services.insert(HOST_SPACES.to_owned());
        }

        let grpc = routes.prepare().into_axum_router();
        let grpc = tonic_web::GrpcWebLayer::new()
            .layer(AuthLayer::new(ctx.auth().clone(), ctx.access_mode()).layer(grpc));

        // Plain-HTTP routes.
        let mut http = Router::new()
            .route(cua_proto::metadata::HEALTH_PATH, get(crate::http::health))
            .route(
                cua_proto::metadata::FILES_PATH,
                any(crate::http::files::handle),
            )
            .with_state(ctx.clone());
        manifest
            .http_paths
            .insert(cua_proto::metadata::HEALTH_PATH.into());
        manifest
            .http_paths
            .insert(cua_proto::metadata::FILES_PATH.into());
        // The HTML5 viewer: a public static page that authenticates with the
        // viewer ticket in its URL fragment.
        http = http.merge(cua_spacesd_html5::router());
        manifest
            .http_paths
            .insert(cua_proto::metadata::VIEWER_PATH.into());
        http = http.merge(
            Router::new()
                .route(cua_proto::metadata::TUNNEL_WS_PATH, get(tunnel_ws))
                .route(cua_proto::metadata::HOTSPOT_WS_PATH, get(hotspot_ws))
                .with_state(tunnel),
        );
        manifest
            .http_paths
            .insert(cua_proto::metadata::TUNNEL_WS_PATH.into());
        manifest
            .http_paths
            .insert(cua_proto::metadata::HOTSPOT_WS_PATH.into());
        http = http.merge(
            Router::new()
                .route(cua_proto::metadata::VOLUME_WS_PATH, get(volume_ws))
                .with_state(volume),
        );
        manifest
            .http_paths
            .insert(cua_proto::metadata::VOLUME_WS_PATH.into());
        if let Some(tools) = tools.filter(|_| ctx.config().enable_mcp) {
            http = http.merge(
                Router::new()
                    .route(cua_proto::metadata::MCP_PATH, any(crate::http::mcp::handle))
                    .with_state(McpState {
                        ctx: ctx.clone(),
                        envelopes: crate::http::mcp_envelope::enabled()
                            .then(|| crate::http::mcp_envelope::Envelopes::new(tools.clone())),
                        tools,
                    }),
            );
            manifest
                .http_paths
                .insert(cua_proto::metadata::MCP_PATH.into());
        } else {
            http = http.route(
                cua_proto::metadata::MCP_PATH,
                any(|| async {
                    (
                        http::StatusCode::NOT_IMPLEMENTED,
                        "MCP is disabled or no cua-driver tool registry is linked",
                    )
                }),
            );
            manifest
                .http_paths
                .insert(cua_proto::metadata::MCP_PATH.into());
        }
        for provider in providers.iter() {
            if let Some(router) = provider.http_routes() {
                http = http.merge(router);
            }
            for path in provider.http_paths() {
                manifest.http_paths.insert(path.to_owned());
            }
        }
        if !manifest
            .http_paths
            .contains(cua_proto::metadata::MEDIA_WS_PATH)
        {
            http = http.route(
                cua_proto::metadata::MEDIA_WS_PATH,
                any(|| async {
                    (
                        http::StatusCode::NOT_IMPLEMENTED,
                        crate::provider::NO_DESKTOP_PROVIDER,
                    )
                }),
            );
            manifest
                .http_paths
                .insert(cua_proto::metadata::MEDIA_WS_PATH.into());
        }

        // Await-token-file mode: apply the file now (a token already present
        // is live on the first request), then watch it.
        if let Some(path) = ctx.config().await_token_file.clone() {
            let watcher = crate::token_file::TokenFileWatcher {
                path,
                interval: ctx.config().token_poll_interval,
                allow_world_readable: ctx.config().token_file_allow_world_readable,
            };
            watcher.apply_now(&ctx);
            watcher.spawn(ctx.clone());
        }

        // Everything that is not a plain-HTTP route is gRPC (native or web).
        // Outermost: a token-less loopback server refuses browser requests
        // from other origins or host names (before CORS answers a preflight).
        let app = http.fallback_service(grpc).layer(cors_layer()).layer(
            axum::middleware::from_fn_with_state(ctx.clone(), crate::browser_guard::guard),
        );
        Server {
            ctx,
            app,
            manifest,
            volume: volume_state,
        }
    }
}

/// CORS for browser clients (gRPC-Web, `/files`, WebSockets). Credentials
/// are bearer tokens and tickets, never cookies, so any origin is allowed;
/// a token-less loopback server is additionally behind
/// [`crate::browser_guard`].
pub fn cors_layer() -> CorsLayer {
    let allow = [
        "authorization",
        cua_proto::metadata::ENV_AUTHORIZATION,
        cua_proto::metadata::PRINCIPAL_BIN,
        crate::process::KEEPALIVE_METADATA,
        "content-type",
        "x-grpc-web",
        "x-user-agent",
        "grpc-timeout",
        "grpc-accept-encoding",
        "grpc-encoding",
        "range",
        "mcp-session-id",
        crate::http::mcp::AGENT_SESSION_HEADER,
    ];
    let expose = [
        "grpc-status",
        "grpc-message",
        "grpc-status-details-bin",
        "content-range",
        "content-length",
        "accept-ranges",
        "mcp-session-id",
    ];
    CorsLayer::new()
        .allow_origin(AllowOrigin::any())
        .allow_methods(Any)
        .allow_headers(allow.map(HeaderName::from_static))
        .expose_headers(expose.map(HeaderName::from_static))
        .max_age(Duration::from_secs(24 * 3600))
}

/// A built server.
pub struct Server {
    ctx: ServerContext,
    app: Router,
    manifest: RouteManifest,
    volume: VolumeShared,
}

impl Server {
    /// The route manifest.
    pub fn manifest(&self) -> &RouteManifest {
        &self.manifest
    }

    /// The context.
    pub fn context(&self) -> &ServerContext {
        &self.ctx
    }

    /// The complete router (for in-process tests).
    pub fn router(&self) -> Router {
        self.app.clone()
    }

    /// Serves on `listener` until the context's shutdown token is cancelled,
    /// then drains in-flight connections for the configured grace period.
    pub async fn serve(self, listener: TcpListener) -> std::io::Result<()> {
        if let Ok(addr) = listener.local_addr() {
            self.ctx.set_local_addr(addr);
        }
        // Aggregate health counts, only when the host enabled telemetry for
        // this sandbox (`CUA_SPACESD_TELEMETRY=1`).
        if crate::telemetry::spawn_reporter(&self.ctx.config().data_dir) {
            tracing::info!("aggregate telemetry on (CUA_SPACESD_TELEMETRY)");
        }
        // A mount left by a previous daemon that died without unmounting
        // answers nothing; clear it before anything touches the path.
        self.volume.clear_stale_mount().await;
        // The `cua` keychain (Chrome's fallback home for its Safe Storage
        // key) must never be locked: whatever touches it while it is
        // raises a password dialog nobody can answer.
        #[cfg(target_os = "macos")]
        spawn_keychain_keeper(self.ctx.shutdown_token());
        let shutdown = self.ctx.shutdown_token();
        let grace = self.ctx.config().shutdown_grace;
        let keepalive = self.ctx.config().keepalive_interval;
        let mut builder = auto::Builder::new(TokioExecutor::new());
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(Duration::from_secs(30))
            .keep_alive(true);
        builder
            .http2()
            .timer(TokioTimer::new())
            .keep_alive_interval(Some(keepalive))
            .keep_alive_timeout(keepalive)
            .max_concurrent_streams(4096)
            .initial_stream_window_size(4 * 1024 * 1024)
            .initial_connection_window_size(16 * 1024 * 1024)
            .max_frame_size(1024 * 1024)
            .max_send_buf_size(4 * 1024 * 1024)
            .enable_connect_protocol();
        let graceful = hyper_util::server::graceful::GracefulShutdown::new();
        loop {
            let (stream, peer) = tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok(pair) => pair,
                    Err(error) => {
                        tracing::warn!(%error, "accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                },
                _ = shutdown.cancelled() => break,
            };
            let _ = stream.set_nodelay(true);
            let socket = socket2::SockRef::from(&stream);
            let _ = socket.set_tcp_keepalive(
                &socket2::TcpKeepalive::new()
                    .with_time(keepalive)
                    .with_interval(keepalive),
            );
            // Every request carries its TCP peer (services that take calls
            // only from some addresses read it: `HostSpacesService` on a
            // direct host).
            let service = TowerToHyperService::new(self.app.clone().map_request(
                move |req: http::Request<hyper::body::Incoming>| {
                    let mut req = req.map(axum::body::Body::new);
                    req.extensions_mut().insert(crate::peer::PeerAddr(peer));
                    req
                },
            ));
            let connection = builder
                .serve_connection_with_upgrades(TokioIo::new(stream), service)
                .into_owned();
            let connection = graceful.watch(connection);
            // Token-file rotation/revocation drops every open connection, so
            // no stream outlives the token that authorized it.
            let mut revoked = self.ctx.session_revocations();
            tokio::spawn(async move {
                tokio::select! {
                    result = connection => {
                        if let Err(error) = result {
                            tracing::debug!(%peer, %error, "connection ended with an error");
                        }
                    }
                    _ = revoked.changed() => {
                        tracing::debug!(%peer, "connection dropped: sessions revoked");
                    }
                }
            });
        }
        tracing::info!("shutting down: draining connections");
        drop(listener);
        if tokio::time::timeout(grace, graceful.shutdown())
            .await
            .is_err()
        {
            tracing::warn!(?grace, "grace period elapsed with connections still open");
        }
        // Unmount the volume before the process exits: a daemon that leaves
        // its NFS mount behind leaves a dead mount that hangs `ls`.
        self.volume.finish().await;
        Ok(())
    }
}

/// Binds `ctx.config().listen` after enforcing the bind policy.
pub async fn bind(ctx: &ServerContext) -> std::io::Result<TcpListener> {
    let config = ctx.config();
    crate::config::check_bind_policy(
        config.listen,
        ctx.auth().has_token(),
        config.insecure_bootstrap || config.await_token_file.is_some(),
    )
    .map_err(|m| std::io::Error::new(std::io::ErrorKind::PermissionDenied, m))?;
    TcpListener::bind(config.listen).await
}

/// Convenience for tests and embedders: binds an ephemeral loopback port and
/// serves in the background. Returns the bound address.
pub async fn spawn_local(server: Server) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(error) = server.serve(listener).await {
            tracing::error!(%error, "server failed");
        }
    });
    Ok(addr)
}

/// Unlocks the `cua` keychain now and keeps it unlocked (after a reboot, a
/// wake, a timeout): see `cua_spacesd_teleport::keychain::keep_cua_keychain_unlocked`.
#[cfg(target_os = "macos")]
fn spawn_keychain_keeper(shutdown: tokio_util::sync::CancellationToken) {
    // Nothing in an unattended Space can answer a Keychain dialog: have any
    // Security API call made in this process fail instead of showing one.
    cua_spacesd_teleport::keychain::forbid_keychain_prompts();
    tokio::spawn(async move {
        let host = cua_spacesd_teleport::default_host();
        loop {
            let h = host.clone();
            let _ = tokio::task::spawn_blocking(move || {
                cua_spacesd_teleport::keychain::keep_cua_keychain_unlocked(&*h)
            })
            .await;
            tokio::select! {
                _ = tokio::time::sleep(cua_spacesd_teleport::keychain::KEEPER_INTERVAL) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    });
}
