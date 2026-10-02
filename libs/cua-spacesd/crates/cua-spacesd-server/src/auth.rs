// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Root-token authentication, short-lived tickets and the gRPC auth layer.
//!
//! - The **root token** (`CUA_ENV_TOKEN`, `/run/cua/env-token`, or
//!   `SystemService.Init`) authenticates every gRPC call and `/mcp` via
//!   `authorization: Bearer <token>`. Compared in constant time.
//! - **Tickets** are HMAC-SHA256-signed, scope-bound, expiring strings minted
//!   by an authenticated RPC (`Forward`, `StartHotspot`, `OpenMedia`, ...)
//!   and accepted by the side-channel sockets (`/tunnel`, `/hotspot`,
//!   `/media`) in the `ticket` query parameter or as a WebSocket subprotocol
//!   `cua.ticket.<ticket>`. They never reveal the root token.
//! - **Signed file URLs** (`/files`) use their own HMAC over method, path and
//!   expiry (see `http::files`).
//!
//! Every HMAC key is derived from the root token (or, with no token, from a
//! random per-boot secret), so rotating the token revokes every outstanding
//! ticket and URL.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use cua_proto::env::v1::Principal;
use hmac::{Hmac, Mac};
use http::{Request, Response};
use prost::Message as _;
use sha2::Sha256;
use tower::{Layer, Service};

use crate::util::{constant_time_eq, random_id};

type HmacSha256 = Hmac<Sha256>;

/// Longest ticket lifetime the server mints.
pub const MAX_TICKET_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// WebSocket subprotocol prefix carrying a ticket: `cua.ticket.<ticket>`.
pub const TICKET_SUBPROTOCOL_PREFIX: &str = "cua.ticket.";

/// What a ticket may open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TicketScope {
    /// The media WebSocket / QUIC session (`/media`).
    Media,
    /// TCP forwarding (`/tunnel`).
    Tunnel,
    /// Reverse-SOCKS egress (`/hotspot`).
    Hotspot,
    /// The Cua Volume mount's client socket (`/volume`).
    Volume,
    /// File transfer side channels.
    Files,
    /// The web viewer's scoped bearer (`SystemService.CreateViewerTicket`).
    Viewer,
}

impl TicketScope {
    /// Stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Media => "media",
            Self::Tunnel => "tunnel",
            Self::Hotspot => "hotspot",
            Self::Volume => "volume",
            Self::Files => "files",
            Self::Viewer => "viewer",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "media" => Self::Media,
            "tunnel" => Self::Tunnel,
            "hotspot" => Self::Hotspot,
            "volume" => Self::Volume,
            "files" => Self::Files,
            "viewer" => Self::Viewer,
            _ => return None,
        })
    }
}

/// Verified contents of a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketClaims {
    /// Scope the ticket was minted for.
    pub scope: TicketScope,
    /// Resource id the ticket is bound to (forward id, hotspot id, media
    /// session id, ...).
    pub resource: String,
    /// `Principal.id` of the minting caller (may be empty).
    pub principal_id: String,
    /// Expiry.
    pub expires_at: SystemTime,
}

/// What a viewer ticket allows, carried in the ticket's resource field.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ViewerGrant {
    /// Highest `SessionPolicy` (as its proto number) media may open with.
    #[serde(rename = "p")]
    pub policy: i32,
    /// `ComputerService.GetClipboard` / `SetClipboard`.
    #[serde(rename = "c", default)]
    pub clipboard: bool,
    /// Absolute guest directory for `FilesystemService`, if any.
    #[serde(rename = "f", default, skip_serializing_if = "Option::is_none")]
    pub files_root: Option<String>,
    /// Microphone uplink.
    #[serde(rename = "u", default)]
    pub audio_uplink: bool,
    /// Principal id (already `viewer:`-prefixed).
    #[serde(rename = "i", default)]
    pub principal_id: String,
    /// Principal display name.
    #[serde(rename = "n", default)]
    pub display_name: String,
}

impl ViewerGrant {
    /// The principal a viewer ticket acts as.
    pub fn principal(&self) -> Principal {
        Principal {
            id: self.principal_id.clone(),
            display_name: self.display_name.clone(),
            color: String::new(),
            kind: cua_proto::env::v1::PrincipalKind::Human as i32,
        }
    }

    /// Clamps a requested `SessionPolicy` to the grant (view-only <
    /// background-only < allow-activation).
    pub fn clamp_policy(&self, requested: i32) -> i32 {
        requested.min(self.policy)
    }
}

/// Why a ticket was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketError {
    /// Not a ticket at all.
    Malformed,
    /// Signature mismatch (tampered, or the token rotated).
    BadSignature,
    /// Past its expiry.
    Expired,
    /// Minted for a different scope.
    WrongScope,
}

impl std::fmt::Display for TicketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Malformed => "malformed ticket",
            Self::BadSignature => "invalid ticket signature",
            Self::Expired => "ticket expired",
            Self::WrongScope => "ticket not valid for this endpoint",
        })
    }
}

impl std::error::Error for TicketError {}

#[derive(serde::Serialize, serde::Deserialize)]
struct TicketPayload {
    s: String,
    r: String,
    #[serde(default)]
    p: String,
    e: u64,
    n: String,
}

/// What an external credential authorized.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalGrant {
    /// Who.
    pub principal: Principal,
    /// A view-only share: presence and a view-only stream
    /// ([`SHARED_VIEWER_GRPC_METHODS`]), nothing else.
    pub view_only: bool,
    /// The machine does not share its desktop (host setting
    /// `share_desktop` off): only [`HOST_ONLY_GRPC_METHODS`] answer, and
    /// no plain HTTP route does.
    pub host_only: bool,
    /// The relay-verified account behind the call, for services that act
    /// for it (`HostSpacesService`).
    pub account: Option<RelayCaller>,
}

/// The relay-verified account of a call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayCaller {
    /// Account id (the assertion's `acct`).
    pub account: String,
    /// Verified email, when the relay shares it.
    pub email: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// `owner` or `shared` (an editor).
    pub role: String,
}

/// The only calls a relayed caller may make on a machine that does not
/// share its desktop (it only provides Spaces): the capabilities probe and
/// `HostSpacesService`. Everything else, including every plain HTTP route,
/// is refused with `PermissionDenied`.
pub const HOST_ONLY_GRPC_METHODS: &[&str] = &[
    "/cua.env.v1.SystemService/GetCapabilities",
    "/cua.env.v1.SystemService/Health",
    "/cua.env.v1.HostSpacesService/GetHostSpaces",
    "/cua.env.v1.HostSpacesService/CreateHostSpace",
    "/cua.env.v1.HostSpacesService/DeleteHostSpace",
    "/cua.env.v1.HostSpacesService/CancelHostSpace",
    "/cua.env.v1.HostSpacesService/SetHostSpacePower",
    "/cua.env.v1.HostSpacesService/DeleteCloudSpace",
];

impl ExternalGrant {
    /// Full access as `principal`.
    pub fn full(principal: Principal) -> Self {
        Self {
            principal,
            view_only: false,
            host_only: false,
            account: None,
        }
    }

    /// The viewer grant a view-only share acts under: view-only media, no
    /// clipboard, no files, no microphone.
    pub fn viewer_grant(&self) -> ViewerGrant {
        ViewerGrant {
            policy: cua_proto::env::v1::SessionPolicy::ViewOnly as i32,
            clipboard: false,
            files_root: None,
            audio_uplink: false,
            principal_id: self.principal.id.clone(),
            display_name: self.principal.display_name.clone(),
        }
    }
}

/// The only calls a view-only share (a relay account the owner shared the
/// Space with to watch) may make: capabilities, the view-only stream, and
/// presence with its own cursor. Anything else is refused with
/// `PermissionDenied` ("view-only share: ... cannot call ...").
pub const SHARED_VIEWER_GRPC_METHODS: &[&str] = &[
    "/cua.env.v1.SystemService/GetCapabilities",
    "/cua.env.v1.SystemService/Health",
    "/cua.env.v1.StreamService/ListTargets",
    "/cua.env.v1.StreamService/OpenMedia",
    "/cua.env.v1.StreamService/SetPreferences",
    "/cua.env.v1.StreamService/RequestKeyframe",
    "/cua.env.v1.StreamService/CloseMedia",
    "/cua.env.v1.ComputerService/ListDisplays",
    "/cua.env.v1.PresenceService/Join",
    "/cua.env.v1.PresenceService/UpdateCursor",
    "/cua.env.v1.PresenceService/Leave",
];

/// A credential other than the root token, verified by the embedding
/// binary: `cua-spacesd join` in account mode accepts relay-signed
/// principal assertions this way.
pub trait ExternalAuthenticator: Send + Sync {
    /// `None`: the request carries no such credential (fall back to the
    /// token). `Some(Ok(grant))`: authorized as `grant.principal`, view-only
    /// when `grant.view_only`. `Some(Err(reason))`: presented but refused.
    fn authenticate(&self, headers: &http::HeaderMap) -> Option<Result<ExternalGrant, String>>;
}

/// Holds the root token and derives every HMAC key.
pub struct Auth {
    token: RwLock<Option<Arc<str>>>,
    boot_secret: [u8; 32],
    external: RwLock<Option<Arc<dyn ExternalAuthenticator>>>,
    access_log: RwLock<Option<Arc<crate::access_log::AccessLog>>>,
}

impl Auth {
    /// Creates the store with an optional initial token (empty → none).
    pub fn new(token: Option<String>) -> Self {
        use rand::RngCore as _;
        let mut boot_secret = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut boot_secret);
        Self {
            token: RwLock::new(token.filter(|t| !t.is_empty()).map(Arc::from)),
            boot_secret,
            external: RwLock::new(None),
            access_log: RwLock::new(None),
        }
    }

    /// Records every authorized remote access in `log` from now on.
    pub fn set_access_log(&self, log: Arc<crate::access_log::AccessLog>) {
        *self.access_log.write().expect("access log lock") = Some(log);
    }

    /// Records an access (`via`: `relay`, `viewer`, `token`), when an access
    /// log is configured.
    pub fn record_access(&self, via: &str, who: &str, what: &str) {
        let log = self.access_log.read().expect("access log lock").clone();
        if let Some(log) = log {
            log.record(via, who, what);
        }
    }

    /// Records the access `headers` were authorized with for `what`: the
    /// relay-asserted identity, else the token (with any client-claimed
    /// name marked as claimed). Nothing without a token (a loopback server
    /// that trusts every local caller).
    pub fn record_headers(&self, headers: &http::HeaderMap, what: &str) {
        match self.authenticate_external(headers) {
            Some(Ok(g)) if g.view_only => self.record_access(
                "relay",
                &principal_label(&g.principal),
                &format!("refused {what} (view-only share)"),
            ),
            Some(Ok(g)) => self.record_access("relay", &principal_label(&g.principal), what),
            Some(Err(_)) => {}
            None if self.has_token() => {
                self.record_access("token", &token_label(principal_from_headers(headers)), what)
            }
            None => {}
        }
    }

    /// Installs an external authenticator consulted before the token.
    pub fn set_external(&self, external: Arc<dyn ExternalAuthenticator>) {
        *self.external.write().expect("external lock") = Some(external);
    }

    /// Removes the external authenticator (a relay was detached).
    pub fn clear_external(&self) {
        *self.external.write().expect("external lock") = None;
    }

    /// Runs the external authenticator, if any.
    pub fn authenticate_external(
        &self,
        headers: &http::HeaderMap,
    ) -> Option<Result<ExternalGrant, String>> {
        let external = self.external.read().expect("external lock").clone()?;
        external.authenticate(headers)
    }

    /// The current token.
    pub fn token(&self) -> Option<Arc<str>> {
        self.token.read().expect("token lock").clone()
    }

    /// True once a token is configured.
    pub fn has_token(&self) -> bool {
        self.token.read().expect("token lock").is_some()
    }

    /// Installs or rotates the token. Returns true if it changed.
    pub fn set_token(&self, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        let mut guard = self.token.write().expect("token lock");
        if guard.as_deref() == Some(token) {
            return false;
        }
        *guard = Some(Arc::from(token));
        true
    }

    /// Removes the token (await-token-file revocation). Returns true if one
    /// was configured.
    pub fn clear_token(&self) -> bool {
        self.token.write().expect("token lock").take().is_some()
    }

    /// Installs `token` only if none is configured yet (the first bootstrap
    /// `Init` wins atomically). Returns false when a token already exists or
    /// `token` is empty.
    pub fn claim_token(&self, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        let mut guard = self.token.write().expect("token lock");
        if guard.is_some() {
            return false;
        }
        *guard = Some(Arc::from(token));
        true
    }

    /// Checks `authorization: Bearer <token>`. Always true with no token.
    pub fn check_authorization(&self, header: Option<&str>) -> bool {
        let Some(expected) = self.token() else {
            return true;
        };
        let Some(presented) = header.and_then(parse_bearer) else {
            return false;
        };
        constant_time_eq(presented.as_bytes(), expected.as_bytes())
    }

    /// Checks the token in `authorization` **or** `x-cua-env-authorization`
    /// (the Fleet gateway consumes and strips `authorization`, so the SDK
    /// sends the env token in the alternate header through `/api/svc`).
    /// Always true with no token.
    pub fn check_headers(&self, headers: &http::HeaderMap) -> bool {
        match self.authenticate_external(headers) {
            // A view-only share reaches no plain HTTP route (`/mcp`, exec
            // sockets): it watches through tickets its gRPC calls mint. A
            // machine that does not share its desktop serves none to any
            // relayed caller.
            Some(Ok(grant)) => return !grant.view_only && !grant.host_only,
            Some(Err(_)) => return false,
            None => {}
        }
        if !self.has_token() {
            return true;
        }
        [
            http::header::AUTHORIZATION.as_str(),
            cua_proto::metadata::ENV_AUTHORIZATION,
        ]
        .iter()
        .flat_map(|name| headers.get_all(*name))
        .filter_map(|value| value.to_str().ok())
        .any(|value| self.check_authorization(Some(value)))
    }

    /// Derives a purpose-specific HMAC key from the token (or boot secret).
    pub fn derive_key(&self, purpose: &str) -> [u8; 32] {
        let token = self.token();
        let base: &[u8] = match &token {
            Some(token) => token.as_bytes(),
            None => &self.boot_secret,
        };
        let mut mac = HmacSha256::new_from_slice(base).expect("hmac accepts any key");
        mac.update(b"cua-spacesd/");
        mac.update(purpose.as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// Mints a ticket. `ttl` is clamped to [`MAX_TICKET_TTL`].
    pub fn mint_ticket(
        &self,
        scope: TicketScope,
        resource: &str,
        principal_id: &str,
        ttl: Duration,
    ) -> (String, SystemTime) {
        let expires_at = SystemTime::now() + ttl.min(MAX_TICKET_TTL);
        let exp_ms = expires_at
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let payload = TicketPayload {
            s: scope.as_str().to_owned(),
            r: resource.to_owned(),
            p: principal_id.to_owned(),
            e: exp_ms,
            n: random_id(9),
        };
        let payload = serde_json::to_vec(&payload).expect("ticket payload serializes");
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let body = engine.encode(payload);
        let sig = self.sign_ticket(&body);
        (format!("v1.{body}.{}", engine.encode(sig)), expires_at)
    }

    fn sign_ticket(&self, body: &str) -> [u8; 32] {
        let key = self.derive_key("ticket-v1");
        let mut mac = HmacSha256::new_from_slice(&key).expect("hmac key");
        mac.update(body.as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// Verifies a ticket's signature, scope and expiry.
    pub fn validate_ticket(
        &self,
        ticket: &str,
        scope: TicketScope,
    ) -> Result<TicketClaims, TicketError> {
        let mut parts = ticket.split('.');
        let (Some("v1"), Some(body), Some(sig), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(TicketError::Malformed);
        };
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let sig = engine.decode(sig).map_err(|_| TicketError::Malformed)?;
        let expected = self.sign_ticket(body);
        if !constant_time_eq(&sig, &expected) {
            return Err(TicketError::BadSignature);
        }
        let payload = engine.decode(body).map_err(|_| TicketError::Malformed)?;
        let payload: TicketPayload =
            serde_json::from_slice(&payload).map_err(|_| TicketError::Malformed)?;
        let minted_scope = TicketScope::parse(&payload.s).ok_or(TicketError::Malformed)?;
        if minted_scope != scope {
            return Err(TicketError::WrongScope);
        }
        let expires_at = UNIX_EPOCH + Duration::from_millis(payload.e);
        if SystemTime::now() >= expires_at {
            return Err(TicketError::Expired);
        }
        Ok(TicketClaims {
            scope,
            resource: payload.r,
            principal_id: payload.p,
            expires_at,
        })
    }
}

/// Finds a valid viewer ticket among the bearer credentials of `headers`
/// (`authorization` or `x-cua-env-authorization`). `None` when no bearer
/// is a ticket at all; `Some(Err)` when one is, but it is invalid.
pub fn viewer_ticket_from_headers(
    auth: &Auth,
    headers: &http::HeaderMap,
) -> Option<Result<ViewerGrant, TicketError>> {
    let mut refused = None;
    for value in [
        http::header::AUTHORIZATION.as_str(),
        cua_proto::metadata::ENV_AUTHORIZATION,
    ]
    .iter()
    .flat_map(|name| headers.get_all(*name))
    .filter_map(|value| value.to_str().ok())
    {
        let Some(bearer) = parse_bearer(value) else {
            continue;
        };
        if !bearer.starts_with("v1.") {
            continue;
        }
        match auth.validate_ticket(bearer, TicketScope::Viewer) {
            Ok(claims) => match serde_json::from_str::<ViewerGrant>(&claims.resource) {
                Ok(grant) => return Some(Ok(grant)),
                Err(_) => refused = Some(TicketError::Malformed),
            },
            // A ticket for another scope in a bearer header is a mistake,
            // not a viewer.
            Err(TicketError::WrongScope) => refused = Some(TicketError::WrongScope),
            Err(error) => refused = Some(error),
        }
    }
    refused.map(Err)
}

/// Extracts `<token>` from `Bearer <token>` (scheme is case-insensitive).
pub fn parse_bearer(header: &str) -> Option<&str> {
    let (scheme, rest) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim())
        .filter(|t| !t.is_empty())
}

/// A ticket presented on a side-channel request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedTicket {
    /// The ticket.
    pub ticket: String,
    /// The subprotocol that carried it, which the server must echo when
    /// accepting the WebSocket.
    pub subprotocol: Option<String>,
}

/// Finds a ticket in the `ticket` query parameter or in a
/// `Sec-WebSocket-Protocol: cua.ticket.<ticket>` entry.
pub fn ticket_from_parts(uri: &http::Uri, headers: &http::HeaderMap) -> Option<PresentedTicket> {
    if let Some(query) = uri.query() {
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("ticket=") {
                let decoded = percent_encoding::percent_decode_str(value)
                    .decode_utf8()
                    .ok()?
                    .into_owned();
                return Some(PresentedTicket {
                    ticket: decoded,
                    subprotocol: None,
                });
            }
        }
    }
    for value in headers.get_all(http::header::SEC_WEBSOCKET_PROTOCOL) {
        let Ok(value) = value.to_str() else { continue };
        for protocol in value.split(',').map(str::trim) {
            if let Some(ticket) = protocol.strip_prefix(TICKET_SUBPROTOCOL_PREFIX) {
                return Some(PresentedTicket {
                    ticket: ticket.to_owned(),
                    subprotocol: Some(protocol.to_owned()),
                });
            }
        }
    }
    None
}

/// Decodes the `x-cua-principal-bin` header (base64, padded or not).
pub fn principal_from_headers(headers: &http::HeaderMap) -> Option<Principal> {
    let raw = headers
        .get(cua_proto::metadata::PRINCIPAL_BIN)?
        .to_str()
        .ok()?;
    let trimmed = raw.trim().trim_end_matches('=');
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(trimmed)
        .ok()?;
    Principal::decode(bytes.as_slice()).ok()
}

/// Encodes a principal for the `x-cua-principal-bin` header.
pub fn encode_principal(principal: &Principal) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(principal.encode_to_vec())
}

/// `name (id)` of a verified principal, or the id alone.
pub fn principal_label(p: &Principal) -> String {
    if p.display_name.is_empty() || p.display_name == p.id {
        p.id.clone()
    } else {
        format!("{} ({})", p.display_name, p.id)
    }
}

/// A token caller: the token is the identity; a principal it sends is only
/// the client's claim.
pub fn token_label(claimed: Option<Principal>) -> String {
    match claimed
        .map(|p| {
            if p.display_name.is_empty() {
                p.id
            } else {
                p.display_name
            }
        })
        .filter(|n| !n.is_empty())
    {
        Some(name) => format!("token (claims {name})"),
        None => "token".into(),
    }
}

/// The gRPC service of a `/pkg.Service/Method` path (`ProcessService`).
pub fn grpc_service(path: &str) -> &str {
    let service = path.trim_start_matches('/').split('/').next().unwrap_or("");
    service.rsplit('.').next().unwrap_or(service)
}

/// How the gRPC surface treats callers when no token is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Loopback bind without a token: every caller is trusted.
    OpenLoopback,
    /// Non-loopback bind without a token, explicitly allowed for first-claim
    /// bootstrap: only `GetCapabilities`, `Health` and `Init` are reachable
    /// until `Init` installs a token.
    Bootstrap,
    /// The token comes only from a file the orchestrator writes (see
    /// `token_file`): until it holds a token only `GetCapabilities` and
    /// `Health` are reachable, and `Init` never installs a token.
    AwaitTokenFile,
}

/// Paths reachable without a token in [`AccessMode::Bootstrap`].
pub const BOOTSTRAP_METHODS: &[&str] = &[
    "/cua.env.v1.SystemService/GetCapabilities",
    "/cua.env.v1.SystemService/Health",
    "/cua.env.v1.SystemService/Init",
];

/// Paths reachable without a token in [`AccessMode::AwaitTokenFile`].
pub const AWAIT_TOKEN_FILE_METHODS: &[&str] = &[
    "/cua.env.v1.SystemService/GetCapabilities",
    "/cua.env.v1.SystemService/Health",
];

/// Request extension set by [`AuthLayer`] on every authorized gRPC call.
#[derive(Debug, Clone, Default)]
pub struct CallerIdentity {
    /// The decoded `x-cua-principal-bin`, if any.
    pub principal: Option<Principal>,
    /// True when the call presented the configured token.
    pub token_verified: bool,
    /// True when `principal` was asserted by an [`ExternalAuthenticator`]
    /// (a relay-signed identity) or a viewer ticket: services must not let
    /// the request body override its id or display name.
    pub asserted: bool,
    /// Set when the call was authorized by a viewer ticket: services clamp
    /// what they do to the grant.
    pub viewer: Option<Arc<ViewerGrant>>,
    /// The relay-verified account of a relayed call.
    pub account: Option<RelayCaller>,
}

/// Tower layer that authenticates gRPC (and gRPC-Web) calls.
#[derive(Clone)]
pub struct AuthLayer {
    auth: Arc<Auth>,
    mode: AccessMode,
}

impl AuthLayer {
    /// Creates the layer.
    pub fn new(auth: Arc<Auth>, mode: AccessMode) -> Self {
        Self { auth, mode }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            auth: self.auth.clone(),
            mode: self.mode,
        }
    }
}

/// See [`AuthLayer`].
#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    auth: Arc<Auth>,
    mode: AccessMode,
}

impl<S, B, R> Service<Request<B>> for AuthService<S>
where
    S: Service<Request<B>, Response = Response<R>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
    R: Default + Send + 'static,
{
    type Response = Response<R>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut TaskContext<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let has_token = self.auth.has_token();
        let external = self.auth.authenticate_external(req.headers());
        let verdict = if let Some(external) = external {
            match external {
                Ok(grant) if grant.view_only => {
                    let path = req.uri().path().to_owned();
                    let who = principal_label(&grant.principal);
                    if !SHARED_VIEWER_GRPC_METHODS.contains(&path.as_str()) {
                        self.auth.record_access(
                            "relay",
                            &who,
                            &format!("refused {} (view-only share)", grpc_service(&path)),
                        );
                        tracing::debug!(%path, %who, "refused a view-only share");
                        let response = crate::error::status(
                            tonic::Code::PermissionDenied,
                            cua_proto::env::v1::ErrorReason::PermissionDenied,
                            format!("view-only share: {who} cannot call {path}"),
                        )
                        .into_http::<R>();
                        return Box::pin(async move { Ok(response) });
                    }
                    self.auth.record_access("relay", &who, grpc_service(&path));
                    let viewer = Arc::new(grant.viewer_grant());
                    req.extensions_mut().insert(CallerIdentity {
                        principal: Some(grant.principal),
                        token_verified: true,
                        asserted: true,
                        viewer: Some(viewer),
                        account: grant.account,
                    });
                    let mut inner = self.inner.clone();
                    std::mem::swap(&mut self.inner, &mut inner);
                    return Box::pin(inner.call(req));
                }
                Ok(grant) => {
                    let path = req.uri().path().to_owned();
                    if grant.host_only && !HOST_ONLY_GRPC_METHODS.contains(&path.as_str()) {
                        let who = principal_label(&grant.principal);
                        self.auth.record_access(
                            "relay",
                            &who,
                            &format!("refused {} (desktop not shared)", grpc_service(&path)),
                        );
                        tracing::debug!(%path, %who, "refused: this machine does not share its desktop");
                        let response = crate::error::status(
                            tonic::Code::PermissionDenied,
                            cua_proto::env::v1::ErrorReason::PermissionDenied,
                            format!(
                                "this machine does not share its desktop (it only provides Spaces): \
                                 {who} cannot call {path}"
                            ),
                        )
                        .into_http::<R>();
                        return Box::pin(async move { Ok(response) });
                    }
                    self.auth.record_access(
                        "relay",
                        &principal_label(&grant.principal),
                        grpc_service(&path),
                    );
                    req.extensions_mut().insert(CallerIdentity {
                        principal: Some(grant.principal),
                        token_verified: true,
                        asserted: true,
                        viewer: None,
                        account: grant.account,
                    });
                    let mut inner = self.inner.clone();
                    std::mem::swap(&mut self.inner, &mut inner);
                    return Box::pin(inner.call(req));
                }
                Err(reason) => {
                    self.auth.record_access(
                        "relay",
                        "refused",
                        &reason.chars().take(120).collect::<String>(),
                    );
                    tracing::debug!(path = %req.uri().path(), %reason, "rejected external credential");
                    let response = crate::error::status(
                        tonic::Code::PermissionDenied,
                        cua_proto::env::v1::ErrorReason::PermissionDenied,
                        format!("relay assertion refused: {reason}"),
                    )
                    .into_http::<R>();
                    return Box::pin(async move { Ok(response) });
                }
            }
        } else if let Some(viewer) = viewer_ticket_from_headers(&self.auth, req.headers()) {
            let path = req.uri().path().to_owned();
            let refusal = match viewer {
                Ok(grant) if cua_proto::metadata::VIEWER_GRPC_METHODS.contains(&path.as_str()) => {
                    self.auth.record_access(
                        "viewer",
                        &principal_label(&grant.principal()),
                        grpc_service(&path),
                    );
                    req.extensions_mut().insert(CallerIdentity {
                        principal: Some(grant.principal()),
                        token_verified: true,
                        asserted: true,
                        viewer: Some(Arc::new(grant)),
                        account: None,
                    });
                    let mut inner = self.inner.clone();
                    std::mem::swap(&mut self.inner, &mut inner);
                    return Box::pin(inner.call(req));
                }
                Ok(_) => crate::error::status(
                    tonic::Code::PermissionDenied,
                    cua_proto::env::v1::ErrorReason::PermissionDenied,
                    format!("a viewer ticket cannot call {path}"),
                ),
                Err(error) => {
                    crate::error::unauthenticated(format!("viewer ticket refused: {error}"))
                }
            };
            tracing::debug!(%path, "refused viewer call");
            let response = refusal.into_http::<R>();
            return Box::pin(async move { Ok(response) });
        } else if has_token {
            if self.auth.check_headers(req.headers()) {
                Ok(true)
            } else {
                Err("missing or invalid bearer token")
            }
        } else {
            match self.mode {
                AccessMode::OpenLoopback => Ok(false),
                AccessMode::Bootstrap => {
                    if BOOTSTRAP_METHODS.contains(&req.uri().path()) {
                        Ok(false)
                    } else {
                        tracing::debug!(path = %req.uri().path(), "refused: driver is uninitialized (bootstrap)");
                        let response = crate::error::status(
                            tonic::Code::FailedPrecondition,
                            cua_proto::env::v1::ErrorReason::NotInitialized,
                            "uninitialized: call SystemService.Init with a token first (gateway bootstrap)",
                        )
                        .into_http::<R>();
                        return Box::pin(async move { Ok(response) });
                    }
                }
                AccessMode::AwaitTokenFile => {
                    if AWAIT_TOKEN_FILE_METHODS.contains(&req.uri().path()) {
                        Ok(false)
                    } else {
                        tracing::debug!(path = %req.uri().path(), "refused: awaiting the token file");
                        let response = crate::error::status(
                            tonic::Code::FailedPrecondition,
                            cua_proto::env::v1::ErrorReason::NotInitialized,
                            "awaiting token: the driver has no access token yet (it is read from the \
                             claim's token file, never accepted over the network)",
                        )
                        .into_http::<R>();
                        return Box::pin(async move { Ok(response) });
                    }
                }
            }
        };
        match verdict {
            Ok(token_verified) => {
                let principal = principal_from_headers(req.headers());
                if token_verified {
                    self.auth.record_access(
                        "token",
                        &token_label(principal.clone()),
                        grpc_service(req.uri().path()),
                    );
                }
                req.extensions_mut().insert(CallerIdentity {
                    principal,
                    token_verified,
                    asserted: false,
                    viewer: None,
                    account: None,
                });
                let mut inner = self.inner.clone();
                std::mem::swap(&mut self.inner, &mut inner);
                Box::pin(inner.call(req))
            }
            Err(message) => {
                tracing::debug!(path = %req.uri().path(), "rejected unauthenticated call");
                let response = crate::error::unauthenticated(message).into_http::<R>();
                Box::pin(async move { Ok(response) })
            }
        }
    }
}

/// Reads the [`CallerIdentity`] a request was authorized with.
pub fn caller<T>(request: &tonic::Request<T>) -> CallerIdentity {
    request
        .extensions()
        .get::<CallerIdentity>()
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_token_installs_only_the_first_token() {
        let auth = Auth::new(None);
        assert!(!auth.claim_token(""));
        assert!(!auth.has_token());
        assert!(auth.claim_token("first"));
        assert!(!auth.claim_token("second"));
        assert_eq!(auth.token().as_deref(), Some("first"));
        let configured = Auth::new(Some("preset".into()));
        assert!(!configured.claim_token("x"));
        assert_eq!(configured.token().as_deref(), Some("preset"));
    }

    #[test]
    fn clear_token_revokes_tickets_and_reports_change() {
        let auth = Auth::new(Some("one".into()));
        let (ticket, _) = auth.mint_ticket(TicketScope::Media, "m", "", Duration::from_secs(60));
        assert!(auth.clear_token());
        assert!(!auth.clear_token());
        assert!(!auth.has_token());
        assert_eq!(
            auth.validate_ticket(&ticket, TicketScope::Media),
            Err(TicketError::BadSignature)
        );
    }

    #[test]
    fn ticket_round_trip_and_scope_binding() {
        let auth = Auth::new(Some("secret".into()));
        let (ticket, _) = auth.mint_ticket(
            TicketScope::Tunnel,
            "fwd-1",
            "alice",
            Duration::from_secs(60),
        );
        let claims = auth.validate_ticket(&ticket, TicketScope::Tunnel).unwrap();
        assert_eq!(claims.resource, "fwd-1");
        assert_eq!(claims.principal_id, "alice");
        assert_eq!(
            auth.validate_ticket(&ticket, TicketScope::Media),
            Err(TicketError::WrongScope)
        );
    }

    #[test]
    fn ticket_expiry_is_enforced() {
        let auth = Auth::new(None);
        let (ticket, _) = auth.mint_ticket(TicketScope::Media, "m", "", Duration::ZERO);
        assert_eq!(
            auth.validate_ticket(&ticket, TicketScope::Media),
            Err(TicketError::Expired)
        );
    }

    #[test]
    fn tampered_ticket_is_refused() {
        let auth = Auth::new(Some("secret".into()));
        let (ticket, _) = auth.mint_ticket(TicketScope::Hotspot, "h", "", Duration::from_secs(60));
        let mut parts: Vec<String> = ticket.split('.').map(str::to_owned).collect();
        // Swap in a payload for a different resource, keeping the signature.
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut payload: serde_json::Value =
            serde_json::from_slice(&engine.decode(&parts[1]).unwrap()).unwrap();
        payload["r"] = "other".into();
        parts[1] = engine.encode(serde_json::to_vec(&payload).unwrap());
        assert_eq!(
            auth.validate_ticket(&parts.join("."), TicketScope::Hotspot),
            Err(TicketError::BadSignature)
        );
        assert_eq!(
            auth.validate_ticket("garbage", TicketScope::Hotspot),
            Err(TicketError::Malformed)
        );
    }

    #[test]
    fn rotating_the_token_revokes_tickets() {
        let auth = Auth::new(Some("one".into()));
        let (ticket, _) = auth.mint_ticket(TicketScope::Files, "f", "", Duration::from_secs(60));
        assert!(auth.set_token("two"));
        assert_eq!(
            auth.validate_ticket(&ticket, TicketScope::Files),
            Err(TicketError::BadSignature)
        );
    }

    #[test]
    fn bearer_check_is_exact() {
        let auth = Auth::new(Some("tok".into()));
        assert!(auth.check_authorization(Some("Bearer tok")));
        assert!(auth.check_authorization(Some("bearer tok")));
        assert!(!auth.check_authorization(Some("Bearer tok2")));
        assert!(!auth.check_authorization(Some("tok")));
        assert!(!auth.check_authorization(None));
        let open = Auth::new(None);
        assert!(open.check_authorization(None));
    }

    #[test]
    fn either_token_header_is_accepted() {
        let auth = Auth::new(Some("tok".into()));
        let mut headers = http::HeaderMap::new();
        assert!(!auth.check_headers(&headers));
        headers.insert(
            cua_proto::metadata::ENV_AUTHORIZATION,
            "Bearer tok".parse().unwrap(),
        );
        assert!(auth.check_headers(&headers), "x-cua-env-authorization");
        // A gateway credential in `authorization` does not hide the env token.
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer gateway-jwt".parse().unwrap(),
        );
        assert!(auth.check_headers(&headers));
        let mut only_std = http::HeaderMap::new();
        only_std.insert(http::header::AUTHORIZATION, "Bearer tok".parse().unwrap());
        assert!(auth.check_headers(&only_std), "authorization");
        let mut wrong = http::HeaderMap::new();
        wrong.insert(
            cua_proto::metadata::ENV_AUTHORIZATION,
            "Bearer nope".parse().unwrap(),
        );
        assert!(!auth.check_headers(&wrong));
    }

    #[test]
    fn ticket_found_in_query_or_subprotocol() {
        let uri: http::Uri = "/tunnel?x=1&ticket=v1.a.b".parse().unwrap();
        let found = ticket_from_parts(&uri, &http::HeaderMap::new()).unwrap();
        assert_eq!(found.ticket, "v1.a.b");
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::SEC_WEBSOCKET_PROTOCOL,
            "binary, cua.ticket.v1.c.d".parse().unwrap(),
        );
        let found = ticket_from_parts(&"/tunnel".parse().unwrap(), &headers).unwrap();
        assert_eq!(found.ticket, "v1.c.d");
        assert_eq!(found.subprotocol.as_deref(), Some("cua.ticket.v1.c.d"));
    }

    #[test]
    fn principal_header_round_trips() {
        let principal = Principal {
            id: "u1".into(),
            display_name: "Ada".into(),
            color: "#ff0000".into(),
            kind: 1,
        };
        let mut headers = http::HeaderMap::new();
        headers.insert(
            cua_proto::metadata::PRINCIPAL_BIN,
            encode_principal(&principal).parse().unwrap(),
        );
        assert_eq!(principal_from_headers(&headers), Some(principal));
    }
}
