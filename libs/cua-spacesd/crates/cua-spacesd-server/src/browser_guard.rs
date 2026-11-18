// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Browser guard for token-less loopback servers.
//!
//! Without a token, a loopback bind trusts every caller
//! ([`crate::auth::AccessMode::OpenLoopback`]): that is meant for local
//! processes, not for web pages the user happens to have open. Browsers can
//! reach `127.0.0.1` too, so while no token is configured requests must
//! name a loopback host (`Host` / `:authority`) and, when they carry an
//! `Origin`, a loopback (or Tauri app) origin. Once a token is configured
//! every request is authenticated and the guard steps aside, so browser
//! clients holding tickets keep working from any origin.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::StatusCode;

use crate::auth::AccessMode;
use crate::context::ServerContext;

/// True for `localhost`, `*.localhost`, `127.0.0.0/8` and `::1` (an
/// optional port and IPv6 brackets are accepted).
pub fn is_loopback_host(authority: &str) -> bool {
    let authority = authority.trim();
    let host = if let Some(rest) = authority.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, _)) => host,
            None => return false,
        }
    } else {
        match authority.rsplit_once(':') {
            // `host:port`; a bare IPv6 address without brackets has more colons.
            Some((host, port))
                if !host.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) =>
            {
                host
            }
            Some(_) => authority,
            None => authority,
        }
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// True for `http(s)://<loopback host>[:port]` and the Tauri app origins
/// (`tauri://localhost`, `http(s)://tauri.localhost`). `null` and every
/// other origin are refused.
pub fn is_trusted_origin(origin: &str) -> bool {
    let origin = origin.trim();
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    if rest.contains('/') || rest.contains('@') {
        return false;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => is_loopback_host(rest),
        "tauri" => rest.eq_ignore_ascii_case("localhost"),
        _ => false,
    }
}

/// Why a request to a token-less loopback server is refused, if it is.
pub fn refusal(headers: &http::HeaderMap, uri: &http::Uri) -> Option<&'static str> {
    let host = headers
        .get(http::header::HOST)
        .map(|v| v.to_str().unwrap_or(""))
        .map(str::to_owned)
        .or_else(|| uri.authority().map(|a| a.as_str().to_owned()));
    if let Some(host) = host {
        if !is_loopback_host(&host) {
            return Some("this server has no token and only answers loopback host names");
        }
    }
    for origin in headers.get_all(http::header::ORIGIN) {
        if !origin.to_str().is_ok_and(is_trusted_origin) {
            return Some("this server has no token and refuses cross-origin browser requests");
        }
    }
    None
}

/// Axum middleware: applies [`refusal`] while the server is an open
/// loopback bind without a token.
pub async fn guard(State(ctx): State<ServerContext>, request: Request, next: Next) -> Response {
    if ctx.access_mode() == AccessMode::OpenLoopback && !ctx.auth().has_token() {
        if let Some(reason) = refusal(request.headers(), request.uri()) {
            tracing::debug!(path = %request.uri().path(), reason, "refused browser request");
            return (StatusCode::FORBIDDEN, reason).into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts() {
        for ok in [
            "localhost",
            "localhost:3211",
            "LOCALHOST.",
            "app.localhost:80",
            "127.0.0.1",
            "127.0.0.1:3211",
            "127.1.2.3:1",
            "[::1]",
            "[::1]:3211",
            "::1",
        ] {
            assert!(is_loopback_host(ok), "{ok}");
        }
        for bad in [
            "",
            "evil.example",
            "evil.example:3211",
            "localhost.evil.example",
            "127.0.0.1.nip.io",
            "10.0.0.5:3211",
            "0.0.0.0:3211",
            "[::2]:1",
            "[::1",
        ] {
            assert!(!is_loopback_host(bad), "{bad}");
        }
    }

    #[test]
    fn trusted_origins() {
        for ok in [
            "http://localhost:5173",
            "https://127.0.0.1",
            "http://[::1]:3211",
            "tauri://localhost",
            "http://tauri.localhost",
        ] {
            assert!(is_trusted_origin(ok), "{ok}");
        }
        for bad in [
            "null",
            "https://evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1@evil.example",
            "file://",
            "chrome-extension://abc",
            "tauri://evil",
        ] {
            assert!(!is_trusted_origin(bad), "{bad}");
        }
    }

    #[test]
    fn refusal_checks_host_and_origin() {
        let uri: http::Uri = "/cua.env.v1.ProcessService/Start".parse().unwrap();
        let mut h = http::HeaderMap::new();
        assert!(
            refusal(&h, &uri).is_none(),
            "no host, no origin: local tool"
        );
        h.insert(http::header::HOST, "127.0.0.1:3211".parse().unwrap());
        assert!(refusal(&h, &uri).is_none());
        h.insert(
            http::header::ORIGIN,
            "https://evil.example".parse().unwrap(),
        );
        assert!(refusal(&h, &uri).is_some());
        h.insert(
            http::header::ORIGIN,
            "http://localhost:1420".parse().unwrap(),
        );
        assert!(refusal(&h, &uri).is_none());
        h.insert(
            http::header::HOST,
            "rebound.evil.example:3211".parse().unwrap(),
        );
        assert!(refusal(&h, &uri).is_some());
        // h2: `:authority` lives in the URI.
        let h2: http::Uri = "http://rebound.evil.example:3211/x".parse().unwrap();
        assert!(refusal(&http::HeaderMap::new(), &h2).is_some());
    }
}
