//! Child Spaces: a Space makes, lists and deletes Spaces of its own, on a
//! narrow channel only it can reach.
//!
//! - [`capability`]: the per-Space spawn capability this host mints.
//! - [`tree`]: who is whose child; every answer stays inside the caller's
//!   subtree.
//! - [`api`]: `SpaceService` and `DaemonService` answered for one Space's
//!   subtree (so `cua` and the SDKs work in a Space unchanged).
//! - [`bridge`]: keeps the channel of each running Space with children on:
//!   the guest's cua-spacesd listens on its own loopback and relays each
//!   connection over a WebSocket this host opened (`TunnelService.
//!   StartSpawn`), so the channel's Space is known by the connection, never
//!   by what the guest says. It is never the daemon's socket or loopback
//!   listener, and a guest never holds the daemon's token.

pub mod api;
pub mod bridge;
pub mod budget;
pub mod capability;
#[cfg(test)]
mod tests;
pub mod tree;

use std::path::Path;
use std::sync::Arc;

use axum::body::Body as AxumBody;
use axum::extract::{Path as AxPath, State};
use axum::response::{IntoResponse, Response};
use http::Request;

use crate::server::Shared;

/// The feature a daemon reached on a spawn channel reports.
pub const CHILDREN_FEATURE: &str = "children";

/// The capability a request presented (for the passthrough's bearer).
#[derive(Clone, Debug)]
pub struct Presented(pub String);

/// The channel's address as the caller dialed it (`127.0.0.1:<port>` in
/// the guest), for URLs handed back to it.
#[derive(Clone, Debug)]
pub struct Origin(pub String);

fn origin<B>(req: &Request<B>) -> String {
    req.uri()
        .authority()
        .map(|a| a.to_string())
        .or_else(|| {
            req.headers()
                .get(http::header::HOST)
                .and_then(|h| h.to_str().ok())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// The daemon's child-Space state.
pub struct Children {
    /// Mints and checks capabilities.
    pub authority: capability::Authority,
    /// Trees' budgets and the reservations of creates in flight.
    pub budget: budget::Ledger,
}

impl Children {
    /// Opens the state in `home`.
    pub fn open(home: &Path) -> std::io::Result<Self> {
        Ok(Self {
            authority: capability::Authority::open(home)?,
            budget: budget::Ledger::open(home),
        })
    }
}

/// A reservation in a tree's budget, released when dropped (the create is
/// over: its child now counts as used, or it failed).
pub(crate) struct Held {
    shared: Arc<Shared>,
    ticket: Option<budget::Ticket>,
}

impl Drop for Held {
    fn drop(&mut self) {
        if let (Some(t), Some(c)) = (self.ticket.take(), self.shared.children.as_deref()) {
            c.budget.release(t);
        }
    }
}

/// What the children of `root`'s tree hold now: each counts as a Space;
/// the ones not stopped hold their vCPUs and memory.
pub(crate) fn tree_usage(shared: &Shared, root: &str) -> budget::Usage {
    let spaces = shared.runtime.spaces();
    let links = api::links_of(spaces);
    let mut u = budget::Usage::default();
    for id in tree::subtree(&links, root) {
        u.spaces += 1;
        let (power, _) = spaces.power_of(&id);
        if power == "stopped" {
            continue;
        }
        let (cpus, mem) = local_size(shared, &id);
        u.vcpus += cpus;
        u.memory_mb += mem;
    }
    u
}

/// A local Space's vCPUs and memory (MiB) as recorded, else the create
/// defaults (2, 4096).
pub(crate) fn local_size(shared: &Shared, id: &str) -> (u32, u64) {
    let recorded = id.strip_prefix("local:").and_then(|name| {
        let cua_sandbox_core::SandboxState::Local(l) =
            shared.runtime.sandboxes().state().load(name)?;
        Some((l.cpu_count, l.memory_mb))
    });
    let (cpus, mem) = recorded.unwrap_or((None, None));
    (cpus.unwrap_or(2), mem.unwrap_or(4096))
}

/// Reserves `ask` in `root`'s tree, or says plainly why it does not fit.
pub(crate) fn admit(
    shared: &Arc<Shared>,
    root: &str,
    ask: budget::Ask,
) -> Result<Held, tonic::Status> {
    let Some(c) = shared.children.as_deref() else {
        return Ok(Held {
            shared: shared.clone(),
            ticket: None,
        });
    };
    let limits = c.budget.budget(root).effective();
    let used = tree_usage(shared, root);
    let ticket = c
        .budget
        .reserve(root, &limits, &used, ask)
        .map_err(|r| tonic::Status::resource_exhausted(r.0))?;
    Ok(Held {
        shared: shared.clone(),
        ticket: Some(ticket),
    })
}

impl Shared {
    /// The capability authority, when child Spaces are available.
    pub(crate) fn spawn_authority(&self) -> Option<&capability::Authority> {
        self.children.as_deref().map(|c| &c.authority)
    }
}

/// The bearer of a request (`authorization: Bearer <capability>`).
fn bearer<B>(req: &Request<B>) -> Option<String> {
    req.headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim().to_owned())
}

fn unauthenticated(grpc: bool, message: &str) -> Response {
    if grpc {
        let status = tonic::Status::unauthenticated(message);
        return status.into_http::<AxumBody>();
    }
    (http::StatusCode::UNAUTHORIZED, message.to_owned()).into_response()
}

/// The app one Space's channel serves.
pub(crate) fn channel_app(shared: Arc<Shared>, space: String) -> axum::Router {
    use cua_proto::daemon::v1::{
        daemon_service_server::DaemonServiceServer, space_service_server::SpaceServiceServer,
    };
    let scoped = api::Scoped {
        shared: shared.clone(),
        space: space.clone(),
    };
    let limit = cua_spacesd_client::MAX_MESSAGE_BYTES;
    let grpc = tonic::service::Routes::new(
        SpaceServiceServer::new(scoped.clone())
            .max_decoding_message_size(limit)
            .max_encoding_message_size(limit),
    )
    .add_service(DaemonServiceServer::new(scoped.clone()))
    .into_axum_router();
    let routes = axum::Router::new()
        .route(
            "/v1/spaces/{key}/env/{*rest}",
            axum::routing::any(child_env_passthrough),
        )
        .with_state(scoped)
        .merge(grpc);
    let check = move |mut req: Request<AxumBody>, next: axum::middleware::Next| {
        let shared = shared.clone();
        let space = space.clone();
        async move {
            let grpc = crate::passthrough::is_grpc(&req);
            // A caller header is never identity here: the channel is.
            crate::server::strip_forwarded_caller(req.headers_mut());
            let Some(authority) = shared.spawn_authority() else {
                return unauthenticated(grpc, "child Spaces are off on this machine");
            };
            let Some(token) = bearer(&req) else {
                return unauthenticated(grpc, capability::Refusal::Malformed.message());
            };
            match authority.verify(&token, &space) {
                Ok(claims) => {
                    let at = Origin(origin(&req));
                    req.extensions_mut().insert(claims);
                    req.extensions_mut().insert(Presented(token));
                    req.extensions_mut().insert(at);
                    next.run(req).await
                }
                Err(why) => {
                    tracing::warn!(space = %space, ?why, "spawn capability refused");
                    unauthenticated(grpc, why.message())
                }
            }
        }
    };
    routes.layer(axum::middleware::from_fn(check))
}

/// gRPC methods of a child's cua-spacesd its parent may not call: the
/// child's own spawn channel belongs to the host alone.
const CHILD_ENV_DENIED: &[&str] = &[
    "/cua.env.v1.TunnelService/StartSpawn",
    "/cua.env.v1.TunnelService/StopSpawn",
];

/// `/v1/spaces/<key>/env/<rest>` on a channel: the daemon's env
/// passthrough, for the caller's descendants only.
async fn child_env_passthrough(
    State(scoped): State<api::Scoped>,
    AxPath((key, rest)): AxPath<(String, String)>,
    req: Request<AxumBody>,
) -> Response {
    let (req, ws) = match crate::passthrough::split_upgrade(req).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let grpc = ws.is_none() && crate::passthrough::is_grpc(&req);
    let refuse = |status: tonic::Status| -> Response {
        if grpc {
            status.into_http::<AxumBody>()
        } else {
            (http::StatusCode::NOT_FOUND, status.message().to_owned()).into_response()
        }
    };
    let id = match crate::passthrough::space_from_key(&key) {
        Ok(id) => id,
        Err(_) => return refuse(api::not_found(&key)),
    };
    let child = match scoped.resolve(&id) {
        Ok(c) if c.id == id => c,
        Ok(_) | Err(_) => return refuse(api::not_found(&id)),
    };
    let path = format!("/{}", rest.trim_start_matches('/'));
    if CHILD_ENV_DENIED.contains(&path.as_str()) || path == cua_proto::metadata::SPAWN_WS_PATH {
        return refuse(tonic::Status::permission_denied(
            "a child's spawn channel belongs to its host",
        ));
    }
    let space = match scoped.shared.runtime.spaces().space(&child.id).await {
        Ok(s) => s,
        Err(e) => return crate::passthrough::error_response(e.into(), grpc),
    };
    let headers = if ws.is_some() {
        match space.websocket_headers().await {
            Ok(h) => h,
            Err(e) => return crate::passthrough::error_response(e.into(), false),
        }
    } else {
        vec![]
    };
    let env = match space.spacesd() {
        Ok(env) => env,
        Err(e) => return crate::passthrough::error_response(e.into(), grpc),
    };
    crate::passthrough::forward(env, headers, &rest, req, ws).await
}

/// Serves `app` on one relayed stream (HTTP/1.1 or h2c).
pub(crate) async fn serve_stream<T>(app: axum::Router, io: T)
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use hyper_util::{
        rt::{TokioExecutor, TokioIo},
        server::conn::auto::Builder,
        service::TowerToHyperService,
    };
    let svc = TowerToHyperService::new(app);
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http2()
        .initial_stream_window_size(cua_spacesd_client::transport::STREAM_WINDOW)
        .initial_connection_window_size(cua_spacesd_client::transport::CONNECTION_WINDOW)
        .max_frame_size(cua_spacesd_client::transport::MAX_FRAME);
    let _ = builder
        .serve_connection_with_upgrades(TokioIo::new(io), svc)
        .await;
}
