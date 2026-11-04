// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The web viewer for a phone's web view, as the relay serves it to a
//! request that carries no credential.
//!
//! A web view can set headers on a page's first request only, so the viewer
//! page itself must load without one; its own calls then carry a viewer
//! ticket (see `grants::header_ticket`). The relay serves the page's static
//! files **itself** (the viewer embedded in this build, `cua-spacesd-html5`)
//! and never forwards such a request to a machine. That settles three
//! things at once:
//!
//! - a machine cannot serve its own `/viewer` HTML under the relay's
//!   origin (in path mode, `/m/<id>` machines share the relay's origin);
//! - the answer does not depend on the machine at all: an unknown id, an
//!   offline machine, one that is not the requester's, one that stopped
//!   sharing all get the same bytes, with no lookup to time;
//! - the cost of the path is bounded: static files of a known size and a
//!   per-address limit.
//!
//! The page is sent with `Content-Security-Policy: ... sandbox
//! allow-scripts` (no `allow-same-origin`): it runs in an opaque origin, so
//! even if it were hostile it could neither read the relay's origin nor use
//! its storage. The things that breaks are listed in the README ("The phone
//! app"). Its cross-origin calls need CORS, so the relay answers the
//! preflight of the viewer's own calls itself, the same for every machine id.
//!
//! A request with any credential (a bearer, a ticket, a signed link) is not
//! handled here and goes the normal way.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;

/// Sustained requests per second per address on the unauthenticated viewer
/// path (page files and preflights), and the burst.
pub const RATE_PER_SEC: f64 = 10.0;
pub const BURST: f64 = 40.0;
/// Ticket calls refused by the machine, per address, before the relay stops
/// forwarding that address's ticket calls for the rest of the window.
pub const TICKET_FAILURES: u32 = 30;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// Longest path and query the unauthenticated path takes.
const PATH_MAX: usize = 200;
const QUERY_MAX: usize = 512;

/// What an unauthenticated request is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A viewer file: the normalized path (`/viewer`, `/viewer/`, `/viewer/x.js`).
    Page(String),
    /// The CORS preflight of one of the viewer's own calls.
    Preflight,
}

/// `path` percent-decoded once, then checked against a strict allowlist: the
/// characters `A-Za-z0-9._~-` and `/`; no `%` left, no backslash, NUL or
/// control character, no `.` or `..` segment, no empty segment (`//`). Anything
/// else is `None`.
pub fn normalize(path: &str) -> Option<String> {
    if path.is_empty() || path.len() > PATH_MAX * 3 || !path.starts_with('/') {
        return None;
    }
    let raw = path.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            let v = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            out.push(v);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    if decoded.len() > PATH_MAX
        || !decoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~-/".contains(&b))
    {
        return None;
    }
    // (`%`, backslash, NUL and control characters are outside the set.)
    if decoded.contains("//") {
        return None;
    }
    let trimmed = decoded.strip_suffix('/').unwrap_or(&decoded);
    if trimmed.split('/').skip(1).any(|s| s == "." || s == "..") {
        return None;
    }
    Some(decoded)
}

fn query_ok(q: &str) -> bool {
    q.len() <= QUERY_MAX && q.bytes().all(|b| (0x21..0x7f).contains(&b))
}

/// Whether `headers` or `query` hold any credential: such a request is not
/// the unauthenticated viewer path.
pub fn carries_credential(headers: &HeaderMap, query: Option<&str>) -> bool {
    const NAMES: [&str; 4] = [
        "authorization",
        "x-cua-relay-authorization",
        "x-cua-env-authorization",
        "x-cua-device-session",
    ];
    NAMES.iter().any(|n| headers.contains_key(*n))
        || headers.contains_key(header::SEC_WEBSOCKET_PROTOCOL)
        || query.is_some_and(|q| {
            q.split('&')
                .any(|p| p.starts_with("ticket=") || p.starts_with("sig="))
        })
}

/// What the unauthenticated request is, if the relay answers it itself.
/// `forwarded` is the path and query as the machine would see them.
pub fn classify(method: &Method, forwarded: &str) -> Option<Kind> {
    let (path, query) = match forwarded.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (forwarded, None),
    };
    if query.is_some_and(|q| !query_ok(q)) {
        return None;
    }
    let norm = normalize(path)?;
    match *method {
        Method::GET | Method::HEAD => {
            let is_viewer = norm == "/viewer" || norm.starts_with("/viewer/");
            is_viewer.then_some(Kind::Page(norm))
        }
        Method::OPTIONS => {
            // The viewer's own calls: gRPC-Web services and its files.
            let ok =
                norm.starts_with("/cua.env.v1.") || norm == "/files" || norm.starts_with("/files/");
            ok.then_some(Kind::Preflight)
        }
        _ => None,
    }
}

fn mime_headers(r: &mut Response) {
    let h = r.headers_mut();
    // Static and public: any origin may load it (the page runs in an opaque
    // origin, so it loads its own files cross-origin).
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
}

/// The response for `kind`. Never looks at a machine.
pub fn respond(kind: Kind, head: bool) -> Response {
    match kind {
        Kind::Page(path) if path == "/viewer" => {
            // Relative, so it survives the `/m/<id>` prefix.
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::PERMANENT_REDIRECT;
            r.headers_mut()
                .insert(header::LOCATION, HeaderValue::from_static("viewer/"));
            mime_headers(&mut r);
            r
        }
        Kind::Page(path) => {
            let file = path.strip_prefix("/viewer/").unwrap_or("");
            let file = if file.is_empty() { "index.html" } else { file };
            let mut r = cua_spacesd_html5::serve(file);
            // The viewer's own policy, plus the sandbox: an opaque origin.
            let csp = format!("{}; sandbox allow-scripts", cua_spacesd_html5::CSP);
            if r.status() == StatusCode::OK {
                if let Ok(v) = HeaderValue::from_str(&csp) {
                    r.headers_mut().insert(header::CONTENT_SECURITY_POLICY, v);
                }
                if file == "index.html" {
                    r.headers_mut()
                        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                }
            } else {
                // The same for every miss: no hint which files exist beyond the viewer's.
                r.headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            }
            mime_headers(&mut r);
            if head {
                *r.body_mut() = Body::empty();
            }
            r
        }
        Kind::Preflight => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::NO_CONTENT;
            let h = r.headers_mut();
            h.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET, POST, PUT, DELETE, HEAD, OPTIONS"),
            );
            h.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static(
                    "x-cua-env-authorization, content-type, x-grpc-web, x-user-agent, grpc-timeout, \
                     grpc-accept-encoding, grpc-encoding, range, connect-protocol-version",
                ),
            );
            h.insert(
                header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_static("600"),
            );
            h.insert(header::VARY, HeaderValue::from_static("origin"));
            mime_headers(&mut r);
            r
        }
    }
}

#[derive(Default)]
struct Inner {
    buckets: HashMap<String, (f64, Instant)>,
    failures: HashMap<String, (Instant, u32)>,
    /// Device enrollment calls per account: (window start, calls).
    device_calls: HashMap<String, (Instant, u32)>,
}

/// The limits of the unauthenticated viewer path and of header tickets that
/// machines refuse, per client address.
#[derive(Default)]
pub struct ViewerLimits {
    inner: Mutex<Inner>,
}

/// Device enrollment calls (register, session, approve) one account may make
/// per [`DEVICE_CALL_WINDOW`] with phone support on.
pub const DEVICE_CALLS_PER_WINDOW: u32 = 40;
/// The window of [`DEVICE_CALLS_PER_WINDOW`].
pub const DEVICE_CALL_WINDOW: Duration = Duration::from_secs(60);

impl ViewerLimits {
    /// Counts a device enrollment call of `account`; `Err(wait)` when it made
    /// more than [`DEVICE_CALLS_PER_WINDOW`] in the window.
    pub fn device_call(&self, account: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut g = self.inner.lock().expect("viewer limits");
        if g.device_calls.len() > 20_000 {
            g.device_calls
                .retain(|_, (at, _)| now.saturating_duration_since(*at) < DEVICE_CALL_WINDOW);
        }
        let slot = g.device_calls.entry(account.to_owned()).or_insert((now, 0));
        if now.saturating_duration_since(slot.0) >= DEVICE_CALL_WINDOW {
            *slot = (now, 0);
        }
        slot.1 += 1;
        if slot.1 <= DEVICE_CALLS_PER_WINDOW {
            Ok(())
        } else {
            Err(DEVICE_CALL_WINDOW.saturating_sub(now.saturating_duration_since(slot.0)))
        }
    }

    /// Takes one request from `client`'s bucket.
    pub fn take(&self, client: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut g = self.inner.lock().expect("viewer limits");
        if g.buckets.len() > 20_000 {
            g.buckets
                .retain(|_, (_, at)| now.saturating_duration_since(*at) < Duration::from_secs(120));
        }
        let (tokens, last) = g.buckets.entry(client.to_owned()).or_insert((BURST, now));
        let elapsed = now.saturating_duration_since(*last).as_secs_f64();
        *tokens = (*tokens + elapsed * RATE_PER_SEC).min(BURST);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64(
                ((1.0 - *tokens) / RATE_PER_SEC).max(0.05),
            ))
        }
    }

    /// How long `client`'s ticket calls are refused, when too many failed at
    /// machines lately.
    pub fn ticket_blocked(&self, client: &str) -> Option<Duration> {
        let now = Instant::now();
        let g = self.inner.lock().expect("viewer limits");
        let (at, n) = g.failures.get(client)?;
        let age = now.saturating_duration_since(*at);
        (*n >= TICKET_FAILURES && age < FAILURE_WINDOW).then(|| FAILURE_WINDOW - age)
    }

    /// A ticket call of `client` was refused by its machine.
    pub fn ticket_failed(&self, client: &str) {
        let now = Instant::now();
        let mut g = self.inner.lock().expect("viewer limits");
        if g.failures.len() > 20_000 {
            g.failures
                .retain(|_, (at, _)| now.saturating_duration_since(*at) < FAILURE_WINDOW);
        }
        let e = g.failures.entry(client.to_owned()).or_insert((now, 0));
        if now.saturating_duration_since(e.0) >= FAILURE_WINDOW {
            *e = (now, 0);
        }
        e.1 += 1;
    }
}

/// Whether a machine's answer to a ticket call says the ticket was refused:
/// HTTP 400 (a ticket the machine cannot even read), 401 or 403, or a gRPC status of `UNAUTHENTICATED` (16) or
/// `PERMISSION_DENIED` (7) in the headers (a trailers-only answer).
pub fn refused_by_machine(status: StatusCode, headers: &HeaderMap) -> bool {
    if status == StatusCode::BAD_REQUEST
        || status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
    {
        return true;
    }
    headers
        .get("grpc-status")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s == "16" || s == "7")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_decoded_once_and_checked_against_an_allowlist() {
        for ok in [
            "/viewer",
            "/viewer/",
            "/viewer/viewer.js",
            "/viewer/a-b_c.d~e/x",
            "/viewer/%76iewer.js",
        ] {
            assert!(normalize(ok).is_some(), "{ok}");
        }
        assert_eq!(
            normalize("/viewer/%76iewer.js").as_deref(),
            Some("/viewer/viewer.js")
        );
        for bad in [
            "",
            "viewer",
            "/viewer/../x",
            "/viewer/./x",
            "/viewer/%2e%2e/x",
            "/viewer/%2E%2E/x",
            "/viewer/%252e%252e/x",
            "/viewer/%252f",
            "/viewer//x",
            "/viewer/%2f%2fx",
            "/viewer/a%5cb",
            "/viewer/a\\b",
            "/viewer/%00",
            "/viewer/%0a",
            "/viewer/%7f",
            "/viewer/a b",
            "/viewer/a;b",
            "/viewer/a:b",
            "/viewer/%zz",
            "/viewer/%2",
            "/viewer/\u{e9}",
            "/viewer/%c3%a9",
        ] {
            assert_eq!(normalize(bad), None, "{bad:?}");
        }
        assert_eq!(normalize(&format!("/viewer/{}", "a".repeat(300))), None);
    }

    #[test]
    fn only_get_head_and_the_viewers_own_preflights_are_answered() {
        let page = |m: Method, p: &str| classify(&m, p);
        assert_eq!(
            page(Method::GET, "/viewer/"),
            Some(Kind::Page("/viewer/".into()))
        );
        assert!(page(Method::HEAD, "/viewer").is_some());
        assert!(page(Method::GET, "/viewer/?files=%2Fhome").is_some());
        for (m, p) in [
            (Method::POST, "/viewer/"),
            (Method::PUT, "/viewer/x"),
            (Method::DELETE, "/viewer/x"),
            (Method::GET, "/viewerx/"),
            (Method::GET, "/viewerx"),
            (Method::GET, "/viewer/../files"),
            (Method::GET, "/files"),
            (Method::GET, "/cua.env.v1.SystemService/Health"),
            (Method::OPTIONS, "/viewer/"),
            (Method::OPTIONS, "/cua.env.v1.X/../Y"),
            (Method::OPTIONS, "/other"),
            (Method::GET, "/viewer/?a=b c"),
        ] {
            assert_eq!(classify(&m, p), None, "{m} {p}");
        }
        assert_eq!(
            classify(
                &Method::OPTIONS,
                "/cua.env.v1.ComputerService/GetScreenSize"
            ),
            Some(Kind::Preflight)
        );
        assert_eq!(
            classify(&Method::OPTIONS, "/files?path=a&x=1"),
            Some(Kind::Preflight)
        );
    }

    #[test]
    fn credentials_take_a_request_out_of_this_path() {
        let mut h = HeaderMap::new();
        assert!(!carries_credential(&h, None));
        assert!(!carries_credential(&h, Some("files=%2Fhome")));
        assert!(carries_credential(&h, Some("a=1&ticket=v1.x")));
        h.insert("authorization", "Bearer x".parse().unwrap());
        assert!(carries_credential(&h, None));
        let mut h = HeaderMap::new();
        h.insert("x-cua-env-authorization", "Bearer y".parse().unwrap());
        assert!(carries_credential(&h, None));
    }

    #[test]
    fn the_page_is_sandboxed_static_and_the_same_for_a_miss() {
        let index = respond(Kind::Page("/viewer/".into()), false);
        assert_eq!(index.status(), StatusCode::OK);
        let csp = index.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_string();
        assert!(csp.ends_with("; sandbox allow-scripts"), "{csp}");
        assert!(!csp.contains("allow-same-origin"));
        assert_eq!(index.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(index.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(index.headers()[header::REFERRER_POLICY], "no-referrer");
        let js = respond(Kind::Page("/viewer/viewer.js".into()), false);
        assert_eq!(js.status(), StatusCode::OK);
        assert!(js.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox allow-scripts"));
        let miss = respond(Kind::Page("/viewer/nothing.js".into()), false);
        assert_eq!(miss.status(), StatusCode::NOT_FOUND);
        assert_eq!(miss.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        let redirect = respond(Kind::Page("/viewer".into()), false);
        assert_eq!(redirect.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(redirect.headers()[header::LOCATION], "viewer/");
        let head = respond(Kind::Page("/viewer/".into()), true);
        assert!(head.status().is_success());
    }

    #[test]
    fn every_embedded_file_is_small() {
        // The unauthenticated path serves these and nothing else.
        for name in [
            "index.html",
            "viewer.js",
            "index.css",
            "audioWorklet.js",
            "captureWorklet.js",
        ] {
            let n = cua_spacesd_html5::asset(name).map_or(0, <[u8]>::len);
            assert!(n > 0 && n < 1024 * 1024, "{name}: {n}");
        }
    }

    #[test]
    fn the_limit_refuses_a_flood_and_forgets_it() {
        let l = ViewerLimits::default();
        let mut refused = 0;
        for _ in 0..(BURST as usize + 20) {
            if l.take("1.2.3.4").is_err() {
                refused += 1;
            }
        }
        assert!(refused >= 15, "{refused}");
        assert!(l.take("5.6.7.8").is_ok(), "another address is not limited");
    }

    #[test]
    fn ticket_failures_block_an_address_for_the_window() {
        let l = ViewerLimits::default();
        assert!(l.ticket_blocked("a").is_none());
        for _ in 0..TICKET_FAILURES {
            l.ticket_failed("a");
        }
        assert!(l.ticket_blocked("a").is_some());
        assert!(l.ticket_blocked("b").is_none());
    }

    #[test]
    fn a_machines_refusal_is_read_from_the_status_or_the_grpc_headers() {
        let mut h = HeaderMap::new();
        assert!(!refused_by_machine(StatusCode::OK, &h));
        assert!(refused_by_machine(StatusCode::UNAUTHORIZED, &h));
        assert!(refused_by_machine(StatusCode::BAD_REQUEST, &h));
        h.insert("grpc-status", "16".parse().unwrap());
        assert!(refused_by_machine(StatusCode::OK, &h));
        h.insert("grpc-status", "0".parse().unwrap());
        assert!(!refused_by_machine(StatusCode::OK, &h));
    }
}
