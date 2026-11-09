// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `/share/*`: the host's `cua share` links.
//!
//! The cua daemon owns the share records and serves the shared ports and
//! files on a Unix socket (the share gateway). This route reverse-proxies
//! to it, so the relay (or a direct connection) is the only way in and the
//! gateway never listens on a network address.
//!
//! - `/share/<id>/…` (private or team share): the caller must be a relay
//!   account verified by this machine (the relay's signed assertion), or
//!   hold the machine's env token (the owner, directly). The verified
//!   caller travels to the gateway as JSON in [`CALLER_HEADER`]; any value
//!   the client sent is dropped first, so the gateway can trust it. The
//!   gateway decides who may open which share (owner, team).
//! - `/share/pub/<token>/…` (public share): no credential. The token is the
//!   credential; the gateway checks it, its expiry and its revocation, and
//!   rate-limits it. No caller header is sent.
//!
//! Without `shares` in the host policy (or a gateway socket) every
//! `/share/*` answers 404, and a machine that stopped sharing answers 403.
//! The route is an extension: it is not part of the env spec's path list.
//!
//! Credentials of the client (`authorization`, the relay assertion, the env
//! token header, the caller header) are removed before the request goes to
//! the gateway, which forwards it to apps that must never see them.
//! WebSocket upgrades are spliced through. Request and response bodies
//! stream.

#[cfg(unix)]
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
#[cfg(unix)]
use axum::http::{HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::context::ServerContext;

/// The route prefix.
pub const SHARE_PATH: &str = "/share";
/// Prefix of public (token) shares.
pub const PUBLIC_PREFIX: &str = "/share/pub/";
/// The verified caller, as JSON, for the gateway.
pub const CALLER_HEADER: &str = "x-cua-share-caller";

/// The verified caller the gateway receives.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ShareCaller {
    /// Account id.
    pub account: String,
    /// Verified email, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `owner`, `shared` or `viewer`.
    pub role: String,
    /// Teams the relay admitted the account through.
    pub teams: Vec<String>,
    /// `relay` (relay assertion) or `token` (the env token, directly).
    pub via: &'static str,
}

/// Headers of the client that never reach the gateway.
#[cfg(unix)]
const STRIP: &[&str] = &[
    CALLER_HEADER,
    "authorization",
    "x-cua-env-authorization",
    "x-cua-relay-assertion",
    "x-cua-relay-authorization",
    "x-cua-device-session",
    "x-cua-principal-bin",
    "proxy-authorization",
];

#[cfg(unix)]
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// The request path with a public token cut to its first six characters,
/// for logs.
pub fn redact(path: &str) -> String {
    match path.strip_prefix(PUBLIC_PREFIX) {
        Some(rest) => {
            let (token, tail) = match rest.find('/') {
                Some(i) => rest.split_at(i),
                None => (rest, ""),
            };
            let head: String = token.chars().take(6).collect();
            format!("{PUBLIC_PREFIX}{head}…{tail}")
        }
        None => path.to_owned(),
    }
}

fn refuse(status: StatusCode, message: &'static str) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], message).into_response()
}

/// `ANY /share` and `/share/{*rest}`.
pub async fn handle(State(ctx): State<ServerContext>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let gateway = match ctx.auth().share_gateway() {
        Ok(Some(socket)) => socket,
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "not found"),
        Err(_) => return refuse(StatusCode::FORBIDDEN, "this machine stopped sharing"),
    };
    let headers = request.headers();
    let caller = if path.starts_with(PUBLIC_PREFIX) {
        None
    } else {
        match caller_of(&ctx, headers) {
            Ok(caller) => Some(caller),
            Err(response) => return response,
        }
    };
    match &caller {
        Some(c) => ctx
            .auth()
            .record_access(c.via, &c.account, &format!("share {}", redact(&path))),
        None => {
            ctx.auth()
                .record_access("public", "anonymous", &format!("share {}", redact(&path)))
        }
    }
    proxy(gateway, request, caller).await
}

#[allow(clippy::result_large_err)]
fn caller_of(ctx: &ServerContext, headers: &HeaderMap) -> Result<ShareCaller, Response> {
    match ctx.auth().authenticate_external(headers) {
        Some(Ok(grant)) => {
            let Some(account) = grant.account else {
                return Err(refuse(StatusCode::FORBIDDEN, "no account on this call"));
            };
            Ok(ShareCaller {
                account: account.account,
                email: account.email,
                name: account.name,
                role: if grant.view_only {
                    "viewer".into()
                } else {
                    account.role
                },
                teams: account.teams,
                via: "relay",
            })
        }
        Some(Err(_)) => Err(refuse(StatusCode::FORBIDDEN, "not allowed on this machine")),
        None => {
            // The env token, presented directly: the machine's owner.
            if ctx.auth().has_token() && ctx.auth().check_headers(headers) {
                Ok(ShareCaller {
                    account: "token".into(),
                    email: None,
                    name: None,
                    role: "owner".into(),
                    teams: Vec::new(),
                    via: "token",
                })
            } else {
                Err(refuse(
                    StatusCode::UNAUTHORIZED,
                    "sign in to open this link",
                ))
            }
        }
    }
}

#[cfg(unix)]
async fn proxy(
    socket: std::path::PathBuf,
    request: Request,
    caller: Option<ShareCaller>,
) -> Response {
    use hyper_util::rt::TokioIo;

    let mut request = request;
    let upgrade = request
        .headers()
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let client_upgrade = upgrade.then(|| hyper::upgrade::on(&mut request));
    let (parts, body) = request.into_parts();

    let mut headers = parts.headers.clone();
    for name in STRIP {
        headers.remove(*name);
    }
    for name in HOP_BY_HOP {
        headers.remove(*name);
    }
    if upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        // The WebSocket handshake headers survive (they are not hop-by-hop).
        for name in [
            "sec-websocket-key",
            "sec-websocket-version",
            "sec-websocket-protocol",
            "sec-websocket-extensions",
        ] {
            if let Some(v) = parts.headers.get(name) {
                headers.insert(HeaderName::from_static(name), v.clone());
            }
        }
    }
    if let Some(caller) = &caller {
        if let Ok(json) = serde_json::to_string(caller) {
            if let Ok(v) = HeaderValue::from_str(&json) {
                headers.insert(HeaderName::from_static(CALLER_HEADER), v);
            }
        }
    }
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "/".into());
    let mut upstream = hyper::Request::builder()
        .method(parts.method.clone())
        .uri(path_and_query)
        .version(http::Version::HTTP_11);
    *upstream.headers_mut().expect("builder") = headers;
    let upstream = match upstream.body(body) {
        Ok(r) => r,
        Err(_) => return refuse(StatusCode::BAD_REQUEST, "bad request"),
    };

    let stream = match tokio::net::UnixStream::connect(&socket).await {
        Ok(s) => s,
        Err(_) => {
            return refuse(
                StatusCode::BAD_GATEWAY,
                "the share gateway is not running (cua daemon)",
            )
        }
    };
    let (mut sender, connection) =
        match hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(stream)).await {
            Ok(pair) => pair,
            Err(_) => return refuse(StatusCode::BAD_GATEWAY, "the share gateway did not answer"),
        };
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let mut response = match sender.send_request(upstream).await {
        Ok(r) => r,
        Err(_) => return refuse(StatusCode::BAD_GATEWAY, "the share gateway did not answer"),
    };

    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let Some(client_upgrade) = client_upgrade else {
            return refuse(StatusCode::BAD_GATEWAY, "unexpected protocol switch");
        };
        let gateway_upgrade = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            if let (Ok(client), Ok(gateway)) = (client_upgrade.await, gateway_upgrade.await) {
                let mut client = TokioIo::new(client);
                let mut gateway = TokioIo::new(gateway);
                let _ = tokio::io::copy_bidirectional(&mut client, &mut gateway).await;
            }
        });
        let (head, _) = response.into_parts();
        return Response::from_parts(head, Body::empty());
    }

    let (mut head, body) = response.into_parts();
    for name in HOP_BY_HOP {
        head.headers.remove(*name);
    }
    // Responses are never cached by a shared proxy on the way back.
    head.headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    Response::from_parts(head, Body::new(body))
}

#[cfg(not(unix))]
async fn proxy(_: std::path::PathBuf, _: Request, _: Option<ShareCaller>) -> Response {
    refuse(
        StatusCode::NOT_IMPLEMENTED,
        "shares need a Unix socket gateway; not available on this OS",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_cut_in_logs() {
        assert_eq!(
            redact("/share/pub/abcdef0123456789/x/y"),
            "/share/pub/abcdef…/x/y"
        );
        assert_eq!(redact("/share/pub/ab"), "/share/pub/ab…");
        assert_eq!(
            redact("/share/0123456789abcdef/"),
            "/share/0123456789abcdef/"
        );
    }
}
