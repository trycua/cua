//! env passthrough and media bridge.
//!
//! - `/v1/sandboxes/<name>/env/<grpc path>`: forwards any `cua.env.v1` call
//!   through the sandbox's env channel (which attaches the env token, the
//!   Fleet bearer and claim, and the gateway path prefix). The caller's
//!   daemon token never leaves the daemon. `/v1/sandboxes/<name>/env/media`
//!   WebSocket upgrades are relayed to the spacesd's `/media` socket.
//! - `/v1/sandboxes/<name>/svc/<service>/<path>` and
//!   `/v1/spaces/<key>/svc/<service>/<path>`: any HTTP request to a named
//!   service, streamed both ways with headers passed through (MCP streamable
//!   HTTP, SSE, anything else). The daemon attaches the route's credentials.
//! - `/v1/bridge/media?ticket=<bridge ticket>`: relays a media session
//!   opened by `OpenMediaBridge`. The ticket is also accepted as the
//!   WebSocket subprotocol `cua.ticket.<ticket>` (browsers cannot set
//!   headers), or the legacy `rcdp.v2.ticket.<ticket>`.

use crate::{Error, server::Shared};
use axum::{
    body::Body as AxumBody,
    extract::{Path, Query, State, WebSocketUpgrade, ws},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use http::{HeaderValue, Request, StatusCode};
use std::{collections::HashMap, sync::Arc};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};
use tower::ServiceExt;

/// Ticket subprotocol prefix of the media wire (RCDP wire v2).
const TICKET_SUBPROTOCOL_PREFIX: &str = "cua.ticket.";
/// Legacy ticket subprotocol prefix, still accepted.
const LEGACY_TICKET_SUBPROTOCOL_PREFIX: &str = "rcdp.v2.ticket.";

/// The bridge ticket from a `cua.ticket.<ticket>` (or legacy
/// `rcdp.v2.ticket.<ticket>`) `Sec-WebSocket-Protocol` entry.
fn ticket_from_subprotocols(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get_all(http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .find_map(|p| {
            p.strip_prefix(TICKET_SUBPROTOCOL_PREFIX)
                .or_else(|| p.strip_prefix(LEGACY_TICKET_SUBPROTOCOL_PREFIX))
                .map(str::to_string)
        })
}

/// Percent-encodes a path segment (sandbox names are usually plain). A
/// ref's `:` stays readable (`/v1/sandboxes/local:box/...`); it is a valid
/// path character.
pub(crate) fn encode_segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
        .replace("%3A", ":")
}

fn error_response(e: Error, grpc: bool) -> Response {
    if grpc {
        let mut r: Response<AxumBody> = e.to_status().into_http();
        r.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/grpc"),
        );
        r
    } else {
        let code = match e {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
            Error::SpacesdNotAvailable(_) => StatusCode::BAD_GATEWAY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (code, e.to_string()).into_response()
    }
}

/// The passthrough key of a Space id: URL-safe base64 of the id, so ids
/// with `/` and `:` survive every proxy and URL normalizer.
pub fn space_key(id: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id.as_bytes())
}

fn space_from_key(key: &str) -> Result<String, Error> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(key.as_bytes())
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| Error::InvalidArgument(format!("bad Space key {key:?}")))
}

/// WebSocket routes of the spacesd a passthrough relays.
const WS_ROUTES: [&str; 4] = [
    cua_proto::metadata::MEDIA_WS_PATH,
    cua_proto::metadata::TUNNEL_WS_PATH,
    cua_proto::metadata::HOTSPOT_WS_PATH,
    cua_proto::metadata::VOLUME_WS_PATH,
];

fn is_grpc(req: &Request<AxumBody>) -> bool {
    req.headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("application/grpc"))
}

async fn split_upgrade(
    req: Request<AxumBody>,
) -> Result<(Request<AxumBody>, Option<WebSocketUpgrade>), Box<Response>> {
    let is_upgrade = req
        .headers()
        .get(http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if !is_upgrade {
        return Ok((req, None));
    }
    use axum::extract::FromRequestParts;
    let (mut parts, body) = req.into_parts();
    match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => Ok((Request::from_parts(parts, body), Some(ws))),
        Err(e) => Err(Box::new(e.into_response())),
    }
}

/// Forwards one request (or WebSocket) to a spacesd connection.
async fn forward(
    client: &cua_spacesd_client::SpacesdClient,
    ws_headers: Vec<(String, String)>,
    rest: &str,
    req: Request<AxumBody>,
    ws: Option<WebSocketUpgrade>,
) -> Response {
    let grpc = is_grpc(&req);
    if let Some(ws) = ws {
        let path = format!("/{}", rest.trim_start_matches('/'));
        if !WS_ROUTES.contains(&path.as_str()) {
            return error_response(
                Error::NotFound(format!("no WebSocket route {rest:?}")),
                false,
            );
        }
        let query = req.uri().query().unwrap_or_default();
        let url = client.endpoint().ws_url(&format!("{path}?{query}"));
        return relay(ws, url, ws_headers, None).await;
    }
    let (mut parts, body) = req.into_parts();
    let pq = match parts.uri.query() {
        Some(q) => format!("/{}?{q}", rest.trim_start_matches('/')),
        None => format!("/{}", rest.trim_start_matches('/')),
    };
    parts.uri = match format!("http://upstream.invalid{pq}").parse() {
        Ok(u) => u,
        Err(e) => return error_response(Error::InvalidArgument(e.to_string()), grpc),
    };
    // Never forward the daemon token or client-chosen upstream credentials.
    for h in [
        http::header::AUTHORIZATION.as_str(),
        cua_proto::metadata::ENV_AUTHORIZATION,
        cua_spacesd_client::transport::FLEET_CLAIM_HEADER,
        "host",
    ] {
        parts.headers.remove(h);
    }
    let upstream = Request::from_parts(parts, tonic::body::Body::new(body));
    match client.channel().oneshot(upstream).await {
        Ok(resp) => resp.map(AxumBody::new),
        Err(e) => error_response(Error::Env(e.to_string()), grpc),
    }
}

pub(crate) async fn env_passthrough(
    State(shared): State<Arc<Shared>>,
    Path((name, rest)): Path<(String, String)>,
    req: Request<AxumBody>,
) -> Response {
    let (req, ws) = match split_upgrade(req).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let attachment = match shared.runtime.env(&name, None).await {
        Ok(a) => a,
        Err(e) => return error_response(e, ws.is_none() && is_grpc(&req)),
    };
    forward(
        &attachment.client,
        attachment.ws_headers.clone(),
        &rest,
        req,
        ws,
    )
    .await
}

/// `/v1/spaces/<key>/env/<rest>`: the same passthrough for a registered
/// Space, with the Space's own credentials (env token, Fleet bearer and
/// claim) attached here.
pub(crate) async fn space_env_passthrough(
    State(shared): State<Arc<Shared>>,
    Path((key, rest)): Path<(String, String)>,
    req: Request<AxumBody>,
) -> Response {
    let (req, ws) = match split_upgrade(req).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let grpc = ws.is_none() && is_grpc(&req);
    #[cfg(feature = "spaces")]
    {
        let space = match space_from_key(&key) {
            Ok(id) => match shared.runtime.spaces().space(&id).await {
                Ok(s) => s,
                Err(e) => return error_response(e.into(), grpc),
            },
            Err(e) => return error_response(e, grpc),
        };
        let headers = if ws.is_some() {
            match space.websocket_headers().await {
                Ok(h) => h,
                Err(e) => return error_response(e.into(), false),
            }
        } else {
            vec![]
        };
        let env = match space.spacesd() {
            Ok(env) => env,
            Err(e) => return error_response(e.into(), grpc),
        };
        forward(env, headers, &rest, req, ws).await
    }
    #[cfg(not(feature = "spaces"))]
    {
        let _ = (shared, key, rest, req, space_from_key);
        error_response(
            Error::Unsupported("this daemon was built without Spaces".into()),
            grpc,
        )
    }
}

/// Forwards one HTTP request to `endpoint` + `/<rest>`, protocol
/// transparently: any method, the caller's end-to-end headers (hop-by-hop
/// ones and the daemon's own bearer dropped), the request body streamed up
/// and the response body streamed back as the service sends it, so SSE,
/// long-lived streams and large bodies pass through unchanged. Only the wait
/// for the response head is bounded.
async fn proxy(
    endpoint: cua_sandbox_core::ServiceEndpoint,
    daemon_token: &str,
    rest: &str,
    req: Request<AxumBody>,
) -> Response {
    use http_body_util::BodyExt;
    let (parts, body) = req.into_parts();
    let daemon_bearer = format!("Bearer {daemon_token}");
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .filter(|(k, v)| {
            !(*k == http::header::AUTHORIZATION && v.as_bytes() == daemon_bearer.as_bytes())
        })
        .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
        .collect();
    let path = match parts.uri.query() {
        Some(q) => format!("/{rest}?{q}"),
        None => format!("/{rest}"),
    };
    let body = body
        .map_err(|e| Box::new(e) as cua_sandbox_core::http::BodyError)
        .boxed_unsync();
    match cua_sandbox_core::http::open(
        &endpoint,
        parts.method.as_str(),
        &path,
        &headers,
        body,
        Some(std::time::Duration::from_secs(120)),
    )
    .await
    {
        Ok(resp) => {
            let (mut head, body) = resp.into_parts();
            let hop: Vec<http::HeaderName> = head
                .headers
                .keys()
                .filter(|k| cua_sandbox_core::http::is_hop_by_hop(k.as_str()))
                .cloned()
                .collect();
            for k in hop {
                head.headers.remove(k);
            }
            Response::from_parts(head, AxumBody::new(body))
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// `/v1/sandboxes/<name>/svc/<service>/<rest>`: a request to a named service
/// of a sandbox, with its route and credentials (loopback, the Fleet
/// gateway bearer and claim) attached here (see [`proxy`]).
pub(crate) async fn service_passthrough(
    State(shared): State<Arc<Shared>>,
    Path((name, service, rest)): Path<(String, String, String)>,
    req: Request<AxumBody>,
) -> Response {
    let endpoint = match shared.runtime.service_endpoint(&name, &service).await {
        Ok(e) => e,
        Err(e) => return error_response(e, false),
    };
    proxy(endpoint, &shared.token, &rest, req).await
}

/// `/v1/spaces/<key>/svc/<service>/<rest>`: the same for a declared service
/// of a registered Space.
pub(crate) async fn space_service_passthrough(
    State(shared): State<Arc<Shared>>,
    Path((key, service, rest)): Path<(String, String, String)>,
    req: Request<AxumBody>,
) -> Response {
    #[cfg(feature = "spaces")]
    {
        let space = match space_from_key(&key) {
            Ok(id) => match shared.runtime.spaces().space(&id).await {
                Ok(s) => s,
                Err(e) => return error_response(e.into(), false),
            },
            Err(e) => return error_response(e, false),
        };
        let Some(svc) = space.declared_services().get(&service).cloned() else {
            return error_response(
                Error::NotFound(format!("service {service:?} of {}", space.id())),
                false,
            );
        };
        let endpoint = match svc.endpoint().await {
            Ok(e) => e,
            Err(e) => return error_response(e.into(), false),
        };
        proxy(endpoint, &shared.token, &rest, req).await
    }
    #[cfg(not(feature = "spaces"))]
    {
        let _ = (shared, key, service, rest, req, space_from_key);
        error_response(
            Error::Unsupported("this daemon was built without Spaces".into()),
            false,
        )
    }
}

pub(crate) async fn bridge_media(
    State(shared): State<Arc<Shared>>,
    Query(q): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
    req: Request<AxumBody>,
) -> Response {
    let from_protocol = ticket_from_subprotocols(req.headers());
    let Some(ticket) = q.get("ticket").cloned().or(from_protocol) else {
        return (StatusCode::UNAUTHORIZED, "missing bridge ticket").into_response();
    };
    let Some(bridge) = shared.take_bridge(&ticket) else {
        return (StatusCode::UNAUTHORIZED, "unknown or expired bridge ticket").into_response();
    };
    relay(
        ws,
        bridge.upstream_url,
        bridge.headers,
        Some("rcdp.v2".to_string()),
    )
    .await
}

/// Connects upstream first (so auth failures surface as HTTP errors), then
/// accepts the client upgrade and pumps messages both ways.
pub(crate) async fn relay(
    ws: WebSocketUpgrade,
    upstream_url: String,
    headers: Vec<(String, String)>,
    protocol: Option<String>,
) -> Response {
    let mut request = match upstream_url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    for (k, v) in &headers {
        if let (Ok(k), Ok(v)) = (
            http::HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            request.headers_mut().insert(k, v);
        }
    }
    let upstream = match tokio_tungstenite::connect_async(request).await {
        Ok((s, _)) => s,
        Err(tungstenite::Error::Http(resp)) => {
            let code =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            return (code, "upstream media socket refused the upgrade").into_response();
        }
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    let ws = match protocol {
        Some(p) => ws.protocols([p]),
        None => ws,
    };
    ws.on_upgrade(move |client| pump(client, upstream))
}

async fn pump<S>(client: ws::WebSocket, upstream: tokio_tungstenite::WebSocketStream<S>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut up_tx, mut up_rx) = upstream.split();
    let (mut cl_tx, mut cl_rx) = client.split();
    let down = async {
        while let Some(Ok(m)) = up_rx.next().await {
            let Some(m) = to_axum(m) else { continue };
            let close = matches!(m, ws::Message::Close(_));
            if cl_tx.send(m).await.is_err() || close {
                break;
            }
        }
        let _ = cl_tx.close().await;
    };
    let up = async {
        while let Some(Ok(m)) = cl_rx.next().await {
            let m = to_tungstenite(m);
            let close = matches!(m, tungstenite::Message::Close(_));
            if up_tx.send(m).await.is_err() || close {
                break;
            }
        }
        let _ = up_tx.close().await;
    };
    tokio::select! {
        _ = down => {}
        _ = up => {}
    }
}

fn to_axum(m: tungstenite::Message) -> Option<ws::Message> {
    Some(match m {
        tungstenite::Message::Text(t) => ws::Message::Text(t.as_str().into()),
        tungstenite::Message::Binary(b) => ws::Message::Binary(b),
        tungstenite::Message::Ping(b) => ws::Message::Ping(b),
        tungstenite::Message::Pong(b) => ws::Message::Pong(b),
        tungstenite::Message::Close(c) => ws::Message::Close(c.map(|c| ws::CloseFrame {
            code: c.code.into(),
            reason: c.reason.as_str().into(),
        })),
        tungstenite::Message::Frame(_) => return None,
    })
}

fn to_tungstenite(m: ws::Message) -> tungstenite::Message {
    match m {
        ws::Message::Text(t) => tungstenite::Message::Text(t.as_str().into()),
        ws::Message::Binary(b) => tungstenite::Message::Binary(b),
        ws::Message::Ping(b) => tungstenite::Message::Ping(b),
        ws::Message::Pong(b) => tungstenite::Message::Pong(b),
        ws::Message::Close(c) => {
            tungstenite::Message::Close(c.map(|c| tungstenite::protocol::CloseFrame {
                code: c.code.into(),
                reason: c.reason.as_str().into(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> http::HeaderMap {
        let mut h = http::HeaderMap::new();
        h.insert(http::header::SEC_WEBSOCKET_PROTOCOL, value.parse().unwrap());
        h
    }

    #[test]
    fn bridge_ticket_subprotocol_accepts_cua_and_legacy_prefixes() {
        assert_eq!(
            ticket_from_subprotocols(&headers("rcdp.v2, cua.ticket.abc")).as_deref(),
            Some("abc")
        );
        assert_eq!(
            ticket_from_subprotocols(&headers("rcdp.v2, rcdp.v2.ticket.abc")).as_deref(),
            Some("abc")
        );
        assert_eq!(ticket_from_subprotocols(&headers("rcdp.v2")), None);
        assert_eq!(ticket_from_subprotocols(&http::HeaderMap::new()), None);
    }
}
