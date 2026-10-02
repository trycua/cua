// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Extension point for services implemented outside this crate (the desktop
//! services: Computer, Windows, Accessibility, Stream, Presence), and the
//! stubs registered while no provider implements them.

use std::convert::Infallible;
use std::sync::Arc;

use cua_driver_core::server::ToolProvider;
use cua_proto::env::v1::{ComponentHealth, Display, DisplayServer, Feature};

use crate::context::ServerContext;

/// Fully-qualified names of the desktop services an extension provider is
/// expected to implement.
pub const DESKTOP_SERVICES: &[&str] = &[
    "cua.env.v1.ComputerService",
    "cua.env.v1.WindowsService",
    "cua.env.v1.AccessibilityService",
    "cua.env.v1.StreamService",
    "cua.env.v1.PresenceService",
];

/// Canonical feature names owned by the desktop services, reported as
/// unsupported unless a provider reports them.
pub const DESKTOP_FEATURES: &[&str] = &[
    "a11y",
    "background_input",
    "desktop_stream",
    "window_stream",
    "h264_hw",
    "h264_sw",
    "quic_media",
    "clipboard.text",
    "clipboard.files",
    "clipboard.image",
    "presence",
    "windows",
    "launch_app",
    "audio.desktop",
    "audio.per_app",
    "audio.uplink",
    "audio.opus",
];

/// A bundle of extra gRPC services and HTTP routes served on the driver port.
///
/// Lifecycle: the binary builds a [`ServerContext`], constructs providers
/// (keeping a clone of the context if they need tickets or the principal),
/// and hands them to [`crate::ServerBuilder::provider`]. At build time the
/// server calls, in order, [`ServiceProvider::register`] (once) and
/// [`ServiceProvider::http_routes`] (once), and calls
/// [`ServiceProvider::capabilities`], [`ServiceProvider::displays`] and
/// [`ServiceProvider::health`] on every `GetCapabilities` / `Health`.
///
/// Rules:
/// - Every gRPC service added in `register` must be listed in
///   [`ServiceProvider::services`]; the server registers a
///   `FAILED_PRECONDITION` stub for every desktop service no provider
///   claims, and registering the same service twice panics at build time.
/// - gRPC services are authenticated by the server (root token) before they
///   run; read the caller with [`crate::auth::caller`].
/// - HTTP routes are **not** authenticated by the server: they are
///   side channels that must validate a ticket
///   ([`ServerContext::validate_request_ticket`]) or the bearer token
///   ([`ServerContext::check_bearer`]) themselves. Every path must be listed
///   in [`ServiceProvider::http_paths`] and must be one of the paths in
///   `cua_proto::metadata` (the route/spec test enforces this).
pub trait ServiceProvider: Send + Sync + 'static {
    /// Features this provider implements (supported or not, with
    /// limitations). Entries override the server's defaults by name.
    fn capabilities(&self) -> Vec<Feature>;

    /// Adds this provider's tonic services to `routes`.
    fn register(
        &self,
        routes: tonic::service::Routes,
        ctx: &ServerContext,
    ) -> tonic::service::Routes;

    /// HTTP routes (for example the `/media` WebSocket), if any.
    fn http_routes(&self) -> Option<axum::Router>;

    /// Fully-qualified names (`cua.env.v1.StreamService`) of the gRPC
    /// services `register` adds.
    fn services(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Paths served by [`ServiceProvider::http_routes`].
    fn http_paths(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Displays for `GetCapabilities.displays`.
    fn displays(&self) -> Vec<Display> {
        Vec::new()
    }

    /// Graphical session type for `GetCapabilities.display_server`.
    fn display_server(&self) -> Option<DisplayServer> {
        None
    }

    /// Per-component health for `Health` (and `/health`).
    fn health(&self) -> Vec<ComponentHealth> {
        Vec::new()
    }

    /// The cua-driver tool registry this provider already built, so the
    /// Driver service and `/mcp` share it instead of building a second one.
    fn tool_provider(&self) -> Option<Arc<dyn ToolProvider>> {
        None
    }
}

/// Feature helper.
pub fn feature(name: &str, supported: bool, limitation: impl Into<String>) -> Feature {
    Feature {
        name: name.to_owned(),
        supported,
        limitation: limitation.into(),
        attributes: Default::default(),
    }
}

/// Limitation reported for desktop features with no provider.
pub const NO_DESKTOP_PROVIDER: &str =
    "this cua-spacesd build has no desktop provider (Computer, Windows, Accessibility, Stream and Presence are unavailable)";

/// Feature named in the stub error for each desktop service.
fn stub_feature(service: &str) -> &'static str {
    match service {
        "cua.env.v1.ComputerService" => "background_input",
        "cua.env.v1.WindowsService" => "windows",
        "cua.env.v1.AccessibilityService" => "a11y",
        "cua.env.v1.StreamService" => "desktop_stream",
        "cua.env.v1.PresenceService" => "presence",
        _ => "desktop",
    }
}

/// Registers a stub answering every method of `service` with
/// `FAILED_PRECONDITION` + `ErrorInfo{FEATURE_UNSUPPORTED}`.
pub(crate) fn register_stub(routes: &mut tonic::service::Routes, service: &'static str) {
    register_stub_with(routes, service, stub_feature(service), NO_DESKTOP_PROVIDER);
}

/// Limitation reported by a driver that is not a host providing Spaces.
pub const NOT_A_SPACES_HOST: &str =
    "this machine does not provide Spaces (a host set up with `cua host setup --provide-spaces` does)";

/// [`register_stub`] with an explicit feature and limitation.
pub(crate) fn register_stub_with(
    routes: &mut tonic::service::Routes,
    service: &'static str,
    feature_name: &'static str,
    limitation: &'static str,
) {
    let stub = tower::service_fn(move |_req: http::Request<axum::body::Body>| async move {
        let status = crate::error::unsupported(feature_name, format!("{service}: {limitation}"));
        Ok::<_, Infallible>(status.into_http::<axum::body::Body>())
    });
    let router = std::mem::take(routes.axum_router_mut());
    *routes.axum_router_mut() = router.route_service(&format!("/{service}/{{*rest}}"), stub);
}
