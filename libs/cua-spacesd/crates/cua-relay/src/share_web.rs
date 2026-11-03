// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua share` links through the relay: share origins, the browser
//! sign-in, anonymous public links and their rate limits.
//!
//! # Share origins
//!
//! In host mode (`CUA_RELAY_HOST_DOMAIN`) every share gets its own origin,
//! one DNS label under the host domain:
//!
//! - `s<16 hex>-<machine hex>.<domain>`: a private or team share. The 16 hex
//!   are the share id; the machine hex is a prefix (12 or more characters)
//!   of the id of the machine that serves it.
//! - `p<40 hex>-<machine hex>.<domain>`: a public share. The 40 hex are its
//!   token (160 bits), so the origin is itself the unguessable link.
//!
//! The relay rewrites the path to `/share/<id>/<path>` (private) or
//! `/share/pub/<token>/<path>` (public) for the machine, whose spacesd
//! hands it to the cua daemon's share gateway. The shared app sees its own
//! root, so root-relative links work, and one share's cookies and storage
//! never reach another share or the machine's own origin.
//!
//! # Browser sign-in
//!
//! A phone has no cua client to carry an account token, so a browser opening
//! a private share origin without a credential is sent through an OIDC
//! authorization-code + PKCE sign-in (`/v1/web/login`, `/v1/web/callback`,
//! both on the relay's own origin) and comes back to the share origin's
//! `/_cua/session` with a one-time, 60-second, relay-signed code. That
//! endpoint trades the code for an HttpOnly, host-only `cua_share_session`
//! cookie: a relay-signed identity bound to the machine, the share id and
//! the share's host, valid for at most 12 hours. The relay accepts it as the
//! account credential for that one private share origin and nowhere else
//! (never a public share, the bare machine origin, path mode, gRPC or any
//! other route), and runs the same machine-role, team and revocation
//! checks it runs for a bearer token on every request.
//!
//! **Device enrollment.** A browser has no enrolled device, so a share
//! session skips the device-enrollment gate. That is a deliberate decision,
//! limited to this one cookie on this one origin: the cookie reaches only
//! the HTTP of a share the owner chose to hand out, never a desktop, a
//! shell or any gRPC; and the sign-in itself is the account's own OIDC
//! login (with whatever second factor the realm demands).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::assertion::now_secs;
use crate::oidc::{Identity, OrgClaim};
use crate::server::Relay;

/// The cookie that carries a share session.
pub const SESSION_COOKIE: &str = "cua_share_session";
/// The share origin's endpoint that trades a login code for the cookie.
pub const SESSION_PATH: &str = "/_cua/session";
/// How long a share session lasts at most.
pub const SESSION_TTL_SECS: u64 = 12 * 3600;
/// How long a login code lives.
const CODE_TTL_SECS: u64 = 60;
/// How long a started login waits for the identity provider.
const LOGIN_TTL: Duration = Duration::from_secs(600);
const MAX_PENDING_LOGINS: usize = 4096;
const TYP_CODE: &str = "cua-share-code";
const TYP_SESSION: &str = "cua-share-session";

/// The browser sign-in's OIDC client. Without it the sign-in endpoints do
/// not exist and a browser without a token is refused with 401, as before.
#[derive(Debug, Clone)]
pub struct ShareWebConfig {
    /// OIDC client id registered for `<public url>/v1/web/callback`.
    pub client_id: String,
    /// Client secret, for a confidential client (public clients use PKCE
    /// alone).
    pub client_secret: Option<String>,
}

/// Rate limits for anonymous (public share) requests: a token bucket per
/// client address and one per machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShareLimits {
    /// Sustained requests per second per client address.
    pub per_ip_rps: f64,
    /// Burst per client address.
    pub per_ip_burst: f64,
    /// Sustained requests per second per machine, all addresses together.
    pub per_machine_rps: f64,
    /// Burst per machine.
    pub per_machine_burst: f64,
}

impl Default for ShareLimits {
    fn default() -> Self {
        Self {
            per_ip_rps: 20.0,
            per_ip_burst: 60.0,
            per_machine_rps: 200.0,
            per_machine_burst: 600.0,
        }
    }
}

/// Kind of a share link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareKind {
    /// Private or team: needs a verified account.
    Private,
    /// Public: the token is the credential.
    Public,
}

/// A share origin label, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareLabel {
    /// Private or public.
    pub kind: ShareKind,
    /// The share id (private) or token (public), lowercase hex.
    pub sid: String,
    /// A prefix of the serving machine's id.
    pub machine_prefix: String,
}

/// Parses `s<16 hex>-<machine hex>` / `p<40 hex>-<machine hex>`.
pub fn parse_label(label: &str) -> Option<ShareLabel> {
    if label.len() > 63 {
        return None;
    }
    let (kind, sid_len) = match label.as_bytes().first()? {
        b's' => (ShareKind::Private, 16),
        b'p' => (ShareKind::Public, 40),
        _ => return None,
    };
    let rest = &label[1..];
    let (sid, machine) = rest.split_once('-')?;
    let hex = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    (sid.len() == sid_len && hex(sid) && machine.len() >= 12 && hex(machine)).then(|| ShareLabel {
        kind,
        sid: sid.to_owned(),
        machine_prefix: machine.to_owned(),
    })
}

/// The path the machine sees for a request on a share origin.
pub fn machine_path(label: &ShareLabel, path: &str, query: Option<&str>) -> String {
    let base = match label.kind {
        ShareKind::Private => format!("/share/{}", label.sid),
        ShareKind::Public => format!("/share/pub/{}", label.sid),
    };
    let path = if path.is_empty() { "/" } else { path };
    match query {
        Some(q) => format!("{base}{path}?{q}"),
        None => format!("{base}{path}"),
    }
}

/// Share kind and id of a machine path (`/share/pub/<token>/…`,
/// `/share/<id>/…`), for requests that did not come in on a share origin.
pub fn share_of_path(path: &str) -> Option<(ShareKind, String)> {
    let rest = path.strip_prefix("/share/")?;
    if let Some(public) = rest.strip_prefix("pub/") {
        let token = public.split('/').next().unwrap_or("");
        return Some((ShareKind::Public, token.to_owned()));
    }
    let sid = rest.split('/').next().unwrap_or("");
    Some((ShareKind::Private, sid.to_owned()))
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Default)]
struct Buckets {
    by_ip: HashMap<String, Bucket>,
    by_machine: HashMap<String, Bucket>,
}

fn take(
    map: &mut HashMap<String, Bucket>,
    key: &str,
    rps: f64,
    burst: f64,
    now: Instant,
) -> Result<(), Duration> {
    if map.len() > 20_000 {
        map.retain(|_, b| now.saturating_duration_since(b.last) < Duration::from_secs(120));
    }
    let bucket = map.entry(key.to_owned()).or_insert(Bucket {
        tokens: burst,
        last: now,
    });
    let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * rps).min(burst);
    bucket.last = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        Ok(())
    } else {
        Err(Duration::from_secs_f64(
            ((1.0 - bucket.tokens) / rps).max(0.05),
        ))
    }
}

struct PendingLogin {
    verifier: String,
    return_url: url::Url,
    at: Instant,
}

/// The relay's share state.
pub struct ShareState {
    logins: Mutex<HashMap<String, PendingLogin>>,
    used_codes: Mutex<HashMap<String, u64>>,
    buckets: Mutex<Buckets>,
    discovery: tokio::sync::OnceCell<(String, String)>,
    http: reqwest::Client,
}

impl Default for ShareState {
    fn default() -> Self {
        Self {
            logins: Mutex::default(),
            used_codes: Mutex::default(),
            buckets: Mutex::default(),
            discovery: tokio::sync::OnceCell::new(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("http client"),
        }
    }
}

/// Claims of a login code or a share session (told apart by the token
/// type, `typ`, which only the relay's key signs).
#[derive(Debug, Serialize, Deserialize)]
struct SessionClaims {
    sub: String,
    acct: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default)]
    orgs: Vec<(String, Option<String>, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_time: Option<u64>,
    #[serde(default)]
    mfa: bool,
    /// The machine id the share lives on.
    mid: String,
    /// The share id.
    sid: String,
    /// The share origin's host (with port) the token is for.
    host: String,
    iat: u64,
    exp: u64,
    jti: String,
}

impl SessionClaims {
    fn new(who: &Identity, mid: &str, sid: &str, host: &str, ttl: u64) -> Self {
        let now = now_secs();
        Self {
            sub: who.user.clone(),
            acct: who.account.clone(),
            email: who.email.clone(),
            name: who.name.clone(),
            orgs: who
                .orgs
                .iter()
                .map(|o| (o.alias.clone(), o.id.clone(), o.role.to_owned()))
                .collect(),
            auth_time: who.auth_time,
            mfa: who.mfa,
            mid: mid.to_owned(),
            sid: sid.to_owned(),
            host: host.to_ascii_lowercase(),
            iat: now,
            exp: now + ttl,
            jti: crate::os_random_hex(16),
        }
    }

    fn identity(&self) -> Identity {
        Identity {
            user: self.sub.clone(),
            account: self.acct.clone(),
            email: self.email.clone(),
            name: self.name.clone(),
            auth_time: self.auth_time,
            mfa: self.mfa,
            orgs: self
                .orgs
                .iter()
                .map(|(alias, id, role)| OrgClaim {
                    alias: alias.clone(),
                    id: id.clone(),
                    role: match role.as_str() {
                        "owner" => "owner",
                        "admin" => "admin",
                        _ => "member",
                    },
                })
                .collect(),
        }
    }

    fn live(&self) -> bool {
        let now = now_secs();
        self.exp > now && self.iat <= now + crate::assertion::LEEWAY_SECS
    }
}

/// What the cookie of a request says.
#[derive(Debug)]
pub enum Cookie {
    /// No share cookie.
    Absent,
    /// A valid session for this machine, share and host.
    Valid(Identity),
    /// A cookie that is forged, expired or for another share.
    Invalid,
}

/// The value of cookie `name` in a `Cookie` header set.
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// Removes the share session cookie from the request's `Cookie` headers, so
/// the shared app never sees it.
pub fn strip_session_cookie(headers: &mut HeaderMap) {
    let kept: Vec<String> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|p| !p.is_empty() && p.split_once('=').map(|(k, _)| k) != Some(SESSION_COOKIE))
        .map(str::to_owned)
        .collect();
    headers.remove(header::COOKIE);
    if !kept.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&kept.join("; ")) {
            headers.insert(header::COOKIE, v);
        }
    }
}

/// Rewrites a share app's `Set-Cookie` headers so they cannot reach beyond
/// the share's own origin: `Domain` is dropped (host-only), and the relay's
/// own session cookie cannot be set or cleared by the app.
pub fn sanitize_set_cookies(headers: &mut HeaderMap) {
    let cookies: Vec<String> = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    headers.remove(header::SET_COOKIE);
    for cookie in cookies {
        let mut parts = cookie.split(';');
        let first = parts.next().unwrap_or("").trim();
        let name = first.split_once('=').map(|(k, _)| k.trim()).unwrap_or("");
        if name.is_empty() || name.eq_ignore_ascii_case(SESSION_COOKIE) {
            continue;
        }
        let mut out = vec![first.to_owned()];
        for attr in parts {
            let a = attr.trim();
            if a.to_ascii_lowercase().starts_with("domain") {
                continue;
            }
            out.push(a.to_owned());
        }
        if let Ok(v) = HeaderValue::from_str(&out.join("; ")) {
            headers.append(header::SET_COOKIE, v);
        }
    }
}

/// True when a browser is navigating (it asked for a page).
pub fn is_page_navigation(method: &axum::http::Method, headers: &HeaderMap) -> bool {
    (method == axum::http::Method::GET || method == axum::http::Method::HEAD)
        && headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html"))
}

fn same_path_ok(to: &str) -> bool {
    to.starts_with('/')
        && !to.starts_with("//")
        && !to.contains('\\')
        && !to.bytes().any(|b| b < 0x20 || b == 0x7f)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.as_ref())
}

fn page(status: StatusCode, message: &str) -> Response {
    let body = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\">\
         <title>Cua</title><p style=\"font:16px system-ui;margin:3em auto;max-width:28em;padding:0 1em\">{message}</p>"
    );
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response()
}

fn redirect(location: &str) -> Response {
    let mut r = Response::new(axum::body::Body::empty());
    *r.status_mut() = StatusCode::FOUND;
    if let Ok(v) = HeaderValue::from_str(location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    r
}

impl Relay {
    /// The part of `host` before the host domain (`None` outside host mode
    /// or for a foreign host).
    pub(crate) fn host_label(&self, host: &str) -> Option<String> {
        let domain = self.config.host_domain.as_deref()?;
        let host = host.split(':').next()?.to_ascii_lowercase();
        Some(host.strip_suffix(domain)?.strip_suffix('.')?.to_owned())
    }

    /// The share label a request's `Host` names, with the machine it
    /// resolves to. `None`: not a share origin, or the machine prefix names
    /// none or several machines.
    pub(crate) fn share_origin(&self, host: &str) -> Option<(ShareLabel, String)> {
        let label = parse_label(&self.host_label(host)?)?;
        let mut ids: std::collections::BTreeSet<String> = self.machine_ids().into_iter().collect();
        ids.extend(self.directory.all().into_iter().map(|m| m.id));
        let mut matches = ids
            .into_iter()
            .filter(|i| i.starts_with(&label.machine_prefix));
        let id = matches.next()?;
        matches.next().is_none().then_some((label, id))
    }

    /// True when `host` is syntactically a share origin of this relay
    /// (whatever the machine resolves to), so the relay serves nothing of
    /// its own there.
    pub(crate) fn is_share_host(&self, host: &str) -> bool {
        self.host_label(host)
            .and_then(|l| parse_label(&l))
            .is_some()
    }

    /// Whether the browser sign-in is configured (it also needs a public
    /// URL, so the login lives on the relay's own origin).
    pub(crate) fn share_web_enabled(&self) -> bool {
        self.config.share_web.is_some()
            && self.config.public_url.is_some()
            && self.config.host_domain.is_some()
            && self.config.oidc.is_some()
    }

    /// The scheme share URLs use.
    pub(crate) fn share_scheme(&self, headers: &HeaderMap) -> &'static str {
        match self.config.public_url.as_deref() {
            Some(u) if u.starts_with("https://") => "https",
            Some(_) => "http",
            None => {
                if headers
                    .get("x-forwarded-proto")
                    .and_then(|h| h.to_str().ok())
                    == Some("https")
                {
                    "https"
                } else {
                    "http"
                }
            }
        }
    }

    /// What the share cookie of a request on the private share origin
    /// `host` (share `sid` on machine `mid`) says.
    pub(crate) fn share_cookie(
        &self,
        headers: &HeaderMap,
        mid: &str,
        sid: &str,
        host: &str,
    ) -> Cookie {
        let Some(raw) = cookie_value(headers, SESSION_COOKIE) else {
            return Cookie::Absent;
        };
        let claims = self
            .key
            .verify_json::<SessionClaims>(TYP_SESSION, raw)
            .ok()
            .filter(|c| {
                c.live()
                    && c.mid == mid
                    && c.sid == sid
                    && c.host == host.to_ascii_lowercase()
                    && c.exp.saturating_sub(c.iat) <= SESSION_TTL_SECS
            });
        match claims {
            Some(c) => Cookie::Valid(c.identity()),
            None => Cookie::Invalid,
        }
    }

    /// Takes one anonymous request from `client` and `machine`'s buckets.
    pub(crate) fn share_rate_limit(&self, client: &str, machine: &str) -> Result<(), Duration> {
        let l = self.config.share_limits;
        let now = Instant::now();
        let mut b = self.share.buckets.lock().expect("buckets");
        take(&mut b.by_ip, client, l.per_ip_rps, l.per_ip_burst, now)?;
        take(
            &mut b.by_machine,
            machine,
            l.per_machine_rps,
            l.per_machine_burst,
            now,
        )
    }

    /// The redirect that sends a browser without a credential to the
    /// sign-in (`url`: the page it asked for).
    pub(crate) fn share_login_redirect(&self, url: &str) -> Response {
        let base = self
            .config
            .public_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        q.append_pair("return", url);
        redirect(&format!("{base}/v1/web/login?{}", q.finish()))
    }

    async fn discover(&self) -> Result<&(String, String), String> {
        let oidc = self.config.oidc.as_ref().ok_or("no OIDC issuer")?;
        self.share
            .discovery
            .get_or_try_init(|| async {
                let issuer = oidc.config().issuer.trim_end_matches('/').to_owned();
                let doc: serde_json::Value = self
                    .share
                    .http
                    .get(format!("{issuer}/.well-known/openid-configuration"))
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("OIDC discovery: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("OIDC discovery: {e}"))?;
                let get = |k: &str| {
                    doc[k]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("OIDC discovery has no {k}"))
                };
                Ok((get("authorization_endpoint")?, get("token_endpoint")?))
            })
            .await
    }

    /// `return` must be a private share origin of this relay on a machine
    /// that exists: the sign-in never redirects anywhere else.
    fn valid_return(&self, raw: &str) -> Option<(url::Url, ShareLabel, String)> {
        let url = url::Url::parse(raw).ok()?;
        if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
            return None;
        }
        let (label, mid) = self.share_origin(url.host_str()?)?;
        (label.kind == ShareKind::Private).then_some((url, label, mid))
    }
}

fn authority(url: &url::Url) -> String {
    match url.port() {
        Some(p) => format!("{}:{p}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_owned(),
    }
}

/// `GET /v1/web/login?return=<share url>`: starts the OIDC sign-in.
pub async fn login(
    State(relay): State<Relay>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !relay.share_web_enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some((return_url, _, _)) = q.get("return").and_then(|r| relay.valid_return(r)) else {
        crate::auth_log::share_event("web_login_refused", "bad_return");
        return page(
            StatusCode::BAD_REQUEST,
            "This link is not a Cua share link.",
        );
    };
    let (auth_endpoint, _) = match relay.discover().await {
        Ok(d) => d.clone(),
        Err(e) => {
            tracing::warn!(error = %e, "share sign-in: identity provider unavailable");
            crate::auth_log::share_event("web_login_failed", "idp_unavailable");
            return page(
                StatusCode::BAD_GATEWAY,
                "Sign-in is unavailable right now. Try again in a minute.",
            );
        }
    };
    let web = relay.config.share_web.as_ref().expect("enabled");
    let verifier = format!("{}{}", crate::os_random_hex(32), crate::os_random_hex(16));
    let state = crate::os_random_hex(24);
    {
        let mut logins = relay.share.logins.lock().expect("logins");
        let now = Instant::now();
        logins.retain(|_, l| now.saturating_duration_since(l.at) < LOGIN_TTL);
        if logins.len() >= MAX_PENDING_LOGINS {
            crate::auth_log::share_event("web_login_failed", "too_many_pending");
            return page(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many sign-ins at once. Try again in a minute.",
            );
        }
        logins.insert(
            state.clone(),
            PendingLogin {
                verifier: verifier.clone(),
                return_url,
                at: now,
            },
        );
    }
    let redirect_uri = format!(
        "{}/v1/web/callback",
        relay
            .config
            .public_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/')
    );
    let Ok(mut target) = url::Url::parse(&auth_endpoint) else {
        return page(StatusCode::BAD_GATEWAY, "Sign-in is unavailable right now.");
    };
    target
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &web.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", "openid")
        .append_pair("state", &state)
        .append_pair("code_challenge", &pkce_challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    redirect(target.as_str())
}

/// `GET /v1/web/callback?code&state`: finishes the sign-in and sends the
/// browser back to its share origin with a one-time code.
pub async fn callback(
    State(relay): State<Relay>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !relay.share_web_enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let pending = q
        .get("state")
        .and_then(|s| relay.share.logins.lock().expect("logins").remove(s))
        .filter(|l| l.at.elapsed() < LOGIN_TTL);
    let Some(pending) = pending else {
        crate::auth_log::share_event("web_login_failed", "unknown_state");
        return page(
            StatusCode::BAD_REQUEST,
            "This sign-in expired. Open the link again.",
        );
    };
    if q.contains_key("error") {
        crate::auth_log::share_event("web_login_failed", "idp_error");
        return page(StatusCode::FORBIDDEN, "Sign-in was cancelled or refused.");
    }
    let Some(code) = q.get("code") else {
        crate::auth_log::share_event("web_login_failed", "no_code");
        return page(
            StatusCode::BAD_REQUEST,
            "Sign-in did not finish. Open the link again.",
        );
    };
    let identity = match exchange(&relay, code, &pending.verifier).await {
        Ok(i) => i,
        Err(reason) => {
            crate::auth_log::share_event("web_login_failed", reason);
            return page(
                StatusCode::FORBIDDEN,
                "Sign-in did not work. Open the link again.",
            );
        }
    };
    let Some((label, mid)) = relay.share_origin(pending.return_url.host_str().unwrap_or("")) else {
        crate::auth_log::share_event("web_login_failed", "share_gone");
        return page(StatusCode::NOT_FOUND, "This link is no longer available.");
    };
    let host = authority(&pending.return_url);
    let claims = SessionClaims::new(&identity, &mid, &label.sid, &host, CODE_TTL_SECS);
    let token = relay.key.sign_json(TYP_CODE, &claims);
    let mut to = pending.return_url.path().to_owned();
    if let Some(query) = pending.return_url.query() {
        to.push('?');
        to.push_str(query);
    }
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.append_pair("code", &token);
    q.append_pair("to", &to);
    crate::auth_log::share_event("web_login_ok", "code_issued");
    redirect(&format!(
        "{}://{host}{SESSION_PATH}?{}",
        pending.return_url.scheme(),
        q.finish()
    ))
}

async fn exchange(relay: &Relay, code: &str, verifier: &str) -> Result<Identity, &'static str> {
    let (_, token_endpoint) = relay.discover().await.map_err(|_| "idp_unavailable")?;
    let web = relay.config.share_web.as_ref().ok_or("disabled")?;
    let redirect_uri = format!(
        "{}/v1/web/callback",
        relay
            .config
            .public_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/')
    );
    let mut form = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code.to_owned()),
        ("redirect_uri", redirect_uri),
        ("client_id", web.client_id.clone()),
        ("code_verifier", verifier.to_owned()),
    ];
    if let Some(secret) = &web.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let response = relay
        .share
        .http
        .post(token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|_| "token_endpoint_unreachable")?;
    if !response.status().is_success() {
        return Err("token_endpoint_refused");
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "token_endpoint_invalid")?;
    let access = body["access_token"].as_str().ok_or("no_access_token")?;
    let oidc = relay.config.oidc.as_ref().ok_or("disabled")?;
    oidc.validate(access).await.map_err(|_| "token_invalid")
}

/// `GET /_cua/session?code&to` on a private share origin: trades the
/// login code for the session cookie.
pub fn session(relay: &Relay, request: &Request) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let fail = |why: &'static str| {
        crate::auth_log::share_event("web_session_refused", why);
        page(
            StatusCode::FORBIDDEN,
            "This sign-in link did not work. Open the share link again.",
        )
    };
    if request.method() != axum::http::Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some((label, mid)) = relay
        .share_origin(&host)
        .filter(|(l, _)| l.kind == ShareKind::Private)
    else {
        return fail("not_a_share_origin");
    };
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let Some(code) = query.get("code") else {
        return fail("no_code");
    };
    let claims = match relay.key.verify_json::<SessionClaims>(TYP_CODE, code) {
        Ok(c) if c.live() && c.mid == mid && c.sid == label.sid && c.host == host => c,
        _ => return fail("bad_code"),
    };
    {
        let mut used = relay.share.used_codes.lock().expect("used codes");
        let now = now_secs();
        used.retain(|_, exp| *exp > now);
        if used.insert(claims.jti.clone(), claims.exp).is_some() {
            return fail("code_reused");
        }
    }
    let who = claims.identity();
    let session = SessionClaims::new(&who, &mid, &label.sid, &host, SESSION_TTL_SECS);
    let token = relay.key.sign_json(TYP_SESSION, &session);
    let secure = if relay.share_scheme(headers) == "https" {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_TTL_SECS}{secure}"
    );
    let to = query
        .get("to")
        .map(String::as_str)
        .filter(|t| same_path_ok(t))
        .unwrap_or("/");
    let mut response = redirect(to);
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, v);
    }
    crate::auth_log::share_event("web_session_issued", "cookie_set");
    response
}

/// The routes of the browser sign-in (on the relay's own origin).
pub fn routes() -> axum::Router<Relay> {
    axum::Router::new()
        .route("/v1/web/login", axum::routing::get(login))
        .route("/v1/web/callback", axum::routing::get(callback))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn labels_parse_and_map_to_machine_paths() {
        let private = format!("s{}-{}", "a".repeat(16), &MACHINE[..12]);
        let l = parse_label(&private).unwrap();
        assert_eq!(l.kind, ShareKind::Private);
        assert_eq!(l.machine_prefix, &MACHINE[..12]);
        assert_eq!(
            machine_path(&l, "/app/x", Some("q=1")),
            format!("/share/{}/app/x?q=1", "a".repeat(16))
        );
        assert_eq!(
            machine_path(&l, "", None),
            format!("/share/{}/", "a".repeat(16))
        );
        let public = format!("p{}-{}", "b".repeat(40), &MACHINE[..16]);
        let l = parse_label(&public).unwrap();
        assert_eq!(l.kind, ShareKind::Public);
        assert_eq!(
            machine_path(&l, "/", None),
            format!("/share/pub/{}/", "b".repeat(40))
        );
        // Wrong lengths, kinds, characters, too-short machine prefixes, and
        // labels past the DNS limit are not share labels.
        for bad in [
            format!("s{}-{}", "a".repeat(15), &MACHINE[..12]),
            format!("p{}-{}", "a".repeat(16), &MACHINE[..12]),
            format!("x{}-{}", "a".repeat(16), &MACHINE[..12]),
            format!("s{}-{}", "A".repeat(16), &MACHINE[..12]),
            format!("s{}-{}", "a".repeat(16), &MACHINE[..11]),
            format!("p{}-{}", "b".repeat(40), MACHINE),
            "machine-0001".to_owned(),
        ] {
            assert!(parse_label(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn paths_name_their_share() {
        assert_eq!(
            share_of_path("/share/pub/tok/x"),
            Some((ShareKind::Public, "tok".into()))
        );
        assert_eq!(
            share_of_path("/share/abc/x"),
            Some((ShareKind::Private, "abc".into()))
        );
        assert_eq!(share_of_path("/sharex/abc"), None);
        assert_eq!(share_of_path("/viewer/x"), None);
    }

    #[test]
    fn token_buckets_refill_and_refuse() {
        let mut map = HashMap::new();
        let t0 = Instant::now();
        for _ in 0..3 {
            take(&mut map, "ip", 1.0, 3.0, t0).unwrap();
        }
        let wait = take(&mut map, "ip", 1.0, 3.0, t0).unwrap_err();
        assert!(wait >= Duration::from_millis(900), "{wait:?}");
        // Another key has its own bucket; time refills.
        take(&mut map, "other", 1.0, 3.0, t0).unwrap();
        take(&mut map, "ip", 1.0, 3.0, t0 + Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn cookies_are_stripped_and_app_cookies_confined() {
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("a=1; {SESSION_COOKIE}=tok; b=2")).unwrap(),
        );
        strip_session_cookie(&mut h);
        assert_eq!(h.get(header::COOKIE).unwrap(), "a=1; b=2");
        let mut only = HeaderMap::new();
        only.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}=tok")).unwrap(),
        );
        strip_session_cookie(&mut only);
        assert!(only.get(header::COOKIE).is_none());

        let mut r = HeaderMap::new();
        for c in [
            "sid=1; Domain=relay.example; Path=/; HttpOnly",
            "cua_share_session=evil; Path=/",
            "CUA_SHARE_SESSION=; Max-Age=0",
            "k=v; domain=.example.com; Secure",
        ] {
            r.append(header::SET_COOKIE, HeaderValue::from_str(c).unwrap());
        }
        sanitize_set_cookies(&mut r);
        let kept: Vec<_> = r
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect();
        assert_eq!(kept, ["sid=1; Path=/; HttpOnly", "k=v; Secure"]);
    }

    #[test]
    fn return_paths_stay_on_the_origin() {
        assert!(same_path_ok("/a/b?c=1"));
        assert!(!same_path_ok("//evil.example/"));
        assert!(!same_path_ok("https://evil.example/"));
        assert!(!same_path_ok("/\\evil.example"));
        assert!(!same_path_ok("/a\r\nSet-Cookie: x=1"));
    }

    #[test]
    fn pkce_matches_the_rfc_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    fn cookie_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("x=1; {SESSION_COOKIE}={token}")).unwrap(),
        );
        h
    }

    #[test]
    fn cookies_are_checked_for_machine_share_host_and_expiry() {
        let relay = Relay::new(crate::server::RelayConfig {
            host_domain: Some("relay.test".into()),
            ..Default::default()
        });
        let who = Identity {
            user: "ada".into(),
            account: "ada".into(),
            email: Some("ada@example.com".into()),
            name: None,
            auth_time: None,
            mfa: false,
            orgs: vec![OrgClaim {
                alias: "acme".into(),
                id: Some("o1".into()),
                role: "admin",
            }],
        };
        let host = "sabc-0123456789ab.relay.test";
        let good = SessionClaims::new(&who, "machine-1", "sid1", host, 60);
        let token = relay.key.sign_json(TYP_SESSION, &good);
        let check = |token: &str, mid: &str, sid: &str, host: &str| {
            relay.share_cookie(&cookie_headers(token), mid, sid, host)
        };
        // Valid for exactly this machine, share and host, and the identity
        // (organizations included) round-trips.
        match check(&token, "machine-1", "sid1", &host.to_uppercase()) {
            Cookie::Valid(i) => assert_eq!(i, who),
            other => panic!("{other:?}"),
        }
        for (mid, sid, host) in [
            ("machine-2", "sid1", host),
            ("machine-1", "sid2", host),
            ("machine-1", "sid1", "sdef-0123456789ab.relay.test"),
        ] {
            assert!(
                matches!(check(&token, mid, sid, host), Cookie::Invalid),
                "{mid} {sid} {host}"
            );
        }
        // Expired, from the future, longer than 12 hours, another type
        // (a login code), another key.
        let mut old = SessionClaims::new(&who, "machine-1", "sid1", host, 60);
        old.iat -= 600;
        old.exp -= 600;
        let mut future = SessionClaims::new(&who, "machine-1", "sid1", host, 60);
        future.iat += 3600;
        future.exp += 3600;
        let long = SessionClaims::new(&who, "machine-1", "sid1", host, SESSION_TTL_SECS + 3600);
        for bad in [
            relay.key.sign_json(TYP_SESSION, &old),
            relay.key.sign_json(TYP_SESSION, &future),
            relay.key.sign_json(TYP_SESSION, &long),
            relay.key.sign_json(TYP_CODE, &good),
            crate::assertion::RelayKey::generate().sign_json(TYP_SESSION, &good),
            "garbage".to_owned(),
        ] {
            assert!(matches!(
                check(&bad, "machine-1", "sid1", host),
                Cookie::Invalid
            ));
        }
        assert!(matches!(
            relay.share_cookie(&HeaderMap::new(), "machine-1", "sid1", host),
            Cookie::Absent
        ));
    }
}
