//! The share gateway: checks one request against the share records and
//! serves it from disk or forwards it to the port.
//!
//! Requests arrive over a Unix socket (mode 0600, in a 0700 directory) from
//! the host's cua-spacesd, which the relay's routing and sign-in sit in
//! front of. The paths are:
//!
//! - `/share/<id>/...`: a private or team share; needs a verified
//!   [`Caller`] in the [`CALLER_HEADER`] header;
//! - `/share/pub/<token>/...`: a public share; the token is the credential.
//!
//! Anything else is a 404. Every refusal says no more than it must (a wrong
//! id and a stopped share look the same), and unknown paths share one rate
//! limit so a guesser is throttled before it can make a dent in 160 bits.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http::{HeaderValue, Request, Response, StatusCode, header};
use http_body_util::BodyExt as _;
use hyper::body::Incoming;

use cua_sandbox_core::proxy::{Forwarder, ProxyRoute};

use crate::GwBody;
use crate::access::{CALLER_HEADER, Caller, Verdict, decide};
use crate::files::{Root, text};
use crate::limiter::RateLimiter;
use crate::model::{ID_LEN, Share, TOKEN_LEN, Target, now_ms};
use crate::store::ShareStore;

/// Where every share event is recorded (the daemon's hash-chained log).
pub trait AuditSink: Send + Sync {
    /// One line: who did `action` to which share, and a short detail (never
    /// a token, a path's contents or a request body).
    fn record(&self, principal: &str, action: &str, handle: &str, detail: &str);
}

/// Resolves a Space port to a loopback `http://` base (the daemon keeps the
/// port forward open for as long as the share lives).
pub trait UpstreamResolver: Send + Sync {
    /// The base URL of `space`'s `port`, or why it is not reachable.
    fn resolve_space<'a>(
        &'a self,
        space: &'a str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, String>> + Send + 'a>>;
}

/// Limits and intervals.
#[derive(Clone, Debug)]
pub struct GatewayConfig {
    /// Requests a second one public share serves.
    pub public_rps: f64,
    /// Burst of one public share.
    pub public_burst: f64,
    /// Requests a second for ids and tokens that match no share.
    pub unknown_rps: f64,
    /// Burst for ids and tokens that match no share.
    pub unknown_burst: f64,
    /// How often one account's opens of one share are logged again.
    pub open_log_interval: Duration,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            public_rps: 30.0,
            public_burst: 120.0,
            unknown_rps: 5.0,
            unknown_burst: 20.0,
            open_log_interval: Duration::from_secs(300),
        }
    }
}

/// The gateway.
pub struct Gateway {
    store: Arc<ShareStore>,
    resolver: Arc<dyn UpstreamResolver>,
    audit: Arc<dyn AuditSink>,
    forwarder: Forwarder,
    cfg: GatewayConfig,
    public_limit: RateLimiter,
    unknown_limit: RateLimiter,
    opened: Mutex<HashMap<(String, String, String), Instant>>,
}

enum Route<'a> {
    Private { id: &'a str, rest: &'a str },
    Public { token: &'a str, rest: &'a str },
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Splits `/share/<id>/rest` and `/share/pub/<token>/rest`. `rest` keeps its
/// leading `/` and the query.
fn parse_route(path_and_query: &str) -> Option<Route<'_>> {
    let r = path_and_query.strip_prefix("/share/")?;
    let (first, tail) = split_seg(r);
    if first == "pub" {
        let tail = tail.strip_prefix('/')?;
        let (token, rest) = split_seg(tail);
        return is_hex(token, TOKEN_LEN).then_some(Route::Public { token, rest });
    }
    is_hex(first, ID_LEN).then_some(Route::Private {
        id: first,
        rest: tail,
    })
}

/// `seg` up to the first `/` or `?`, and the rest (`/` when nothing is left).
fn split_seg(s: &str) -> (&str, &str) {
    match s.find(['/', '?']) {
        Some(i) if s.as_bytes()[i] == b'?' => (&s[..i], &s[i..]),
        Some(i) => (&s[..i], &s[i..]),
        None => (s, "/"),
    }
}

fn parse_caller(req_headers: &http::HeaderMap) -> std::result::Result<Option<Caller>, ()> {
    let Some(v) = req_headers.get(CALLER_HEADER) else {
        return Ok(None);
    };
    let s = v.to_str().map_err(|_| ())?;
    serde_json::from_str::<Caller>(s).map(Some).map_err(|_| ())
}

fn short(token: &str) -> &str {
    &token[..token.len().min(6)]
}

impl Gateway {
    /// A gateway over `store`.
    pub fn new(
        store: Arc<ShareStore>,
        resolver: Arc<dyn UpstreamResolver>,
        audit: Arc<dyn AuditSink>,
        cfg: GatewayConfig,
    ) -> Self {
        Self {
            public_limit: RateLimiter::new(cfg.public_rps, cfg.public_burst),
            unknown_limit: RateLimiter::new(cfg.unknown_rps, cfg.unknown_burst),
            store,
            resolver,
            audit,
            forwarder: Forwarder::new(),
            cfg,
            opened: Mutex::new(HashMap::new()),
        }
    }

    fn throttled(wait: Duration) -> Response<GwBody> {
        let mut r = text(
            StatusCode::TOO_MANY_REQUESTS,
            "too many requests; slow down",
        );
        let secs = wait.as_secs_f64().ceil().max(1.0) as u64;
        r.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        r
    }

    fn log_once(&self, who: (String, String), action: &str, detail: &str) {
        let key = (who.0.clone(), who.1.clone(), action.to_string());
        let first = {
            let mut g = self.opened.lock().expect("opened");
            if g.len() > 10_000 {
                let ttl = self.cfg.open_log_interval;
                g.retain(|_, t| t.elapsed() < ttl);
            }
            let now = Instant::now();
            match g.get(&key) {
                Some(t) if t.elapsed() < self.cfg.open_log_interval => false,
                _ => {
                    g.insert(key.clone(), now);
                    true
                }
            }
        };
        if first {
            self.audit.record(&who.1, action, &who.0, detail);
        }
    }

    /// Handles one request.
    pub async fn handle(&self, mut req: Request<Incoming>) -> Response<GwBody> {
        let pq = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|| "/".into());
        let caller = match parse_caller(req.headers()) {
            Ok(c) => c,
            Err(()) => return text(StatusCode::BAD_REQUEST, "bad caller"),
        };
        // The credential header is for the gateway only; the app never sees it.
        req.headers_mut().remove(CALLER_HEADER);
        let Some(route) = parse_route(&pq) else {
            return self.unknown();
        };
        let (share, rest, public) = match route {
            Route::Private { id, rest } => (self.store.by_key(id), rest.to_string(), false),
            Route::Public { token, rest } => (self.store.by_token(token), rest.to_string(), true),
        };
        let Some(share) = share else {
            return self.unknown();
        };
        debug_assert_eq!(share.is_public(), public);
        if share.expired(now_ms()) {
            return text(StatusCode::GONE, "this link has ended");
        }
        let principal = match (&caller, public) {
            (Some(c), _) => c.principal(),
            (None, true) => format!("anonymous:{}", short(&share.key)),
            (None, false) => "anonymous".to_string(),
        };
        match decide(&share, caller.as_ref()) {
            Verdict::Allow => {}
            v => {
                self.log_once(
                    (share.handle.clone(), principal),
                    "denied",
                    if v == Verdict::Unauthenticated {
                        "not signed in"
                    } else {
                        "not allowed"
                    },
                );
                return if v == Verdict::Unauthenticated {
                    text(StatusCode::UNAUTHORIZED, "sign in to open this link")
                } else {
                    text(
                        StatusCode::FORBIDDEN,
                        "this link is not shared with your account",
                    )
                };
            }
        }
        if public && let Err(wait) = self.public_limit.check(&share.handle) {
            self.log_once(
                (
                    share.handle.clone(),
                    format!("anonymous:{}", short(&share.key)),
                ),
                "rate-limited",
                "public link over its request rate",
            );
            return Self::throttled(wait);
        }
        self.log_once(
            (share.handle.clone(), principal),
            "open",
            &share.target.describe(),
        );
        self.serve(&share, req, &rest).await
    }

    fn unknown(&self) -> Response<GwBody> {
        match self.unknown_limit.check("unknown") {
            Ok(()) => text(StatusCode::NOT_FOUND, "not found"),
            Err(wait) => Self::throttled(wait),
        }
    }

    async fn serve(&self, share: &Share, req: Request<Incoming>, rest: &str) -> Response<GwBody> {
        match &share.target {
            Target::File { path } => {
                crate::files::serve(&Root::File(path.clone()), req.method(), rest, req.headers())
                    .await
            }
            Target::Dir { path } => {
                crate::files::serve(&Root::Dir(path.clone()), req.method(), rest, req.headers())
                    .await
            }
            Target::Port { port } => {
                self.forward(share, req, rest, format!("http://127.0.0.1:{port}"))
                    .await
            }
            Target::SpacePort { space, port } => {
                match self.resolver.resolve_space(space, *port).await {
                    Ok(base) => self.forward(share, req, rest, base).await,
                    Err(e) => text(
                        StatusCode::BAD_GATEWAY,
                        &format!("the Space is not reachable: {e}"),
                    ),
                }
            }
        }
    }

    async fn forward(
        &self,
        _share: &Share,
        req: Request<Incoming>,
        rest: &str,
        base: String,
    ) -> Response<GwBody> {
        let authority = base
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        let host = req
            .headers()
            .get("x-forwarded-host")
            .or_else(|| req.headers().get(header::HOST))
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let proto = req
            .headers()
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .filter(|p| *p == "http" || *p == "https")
            .unwrap_or("https")
            .to_string();
        let mut headers = vec![
            // Dev servers check Host; the app sees the address it listens on.
            ("host".to_string(), authority.clone()),
            ("x-forwarded-proto".to_string(), proto.clone()),
        ];
        if !host.is_empty() {
            headers.push(("x-forwarded-host".to_string(), host.clone()));
        }
        let route = ProxyRoute {
            url: cua_sandbox_core::proxy::join_url(&base, rest),
            headers,
        };
        let resp = self.forwarder.forward(req, route).await;
        let (mut parts, body) = resp.into_parts();
        // An absolute redirect to the app's own address goes back to the link.
        if let Some(loc) = parts
            .headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            && let Some(rest_of) = loc.strip_prefix(&format!("http://{authority}"))
            && !host.is_empty()
            && let Ok(v) = HeaderValue::from_str(&format!("{proto}://{host}{rest_of}"))
        {
            parts.headers.insert(header::LOCATION, v);
        }
        parts
            .headers
            .entry("x-robots-tag")
            .or_insert(HeaderValue::from_static("noindex"));
        parts
            .headers
            .entry(header::REFERRER_POLICY)
            .or_insert(HeaderValue::from_static("no-referrer"));
        Response::from_parts(
            parts,
            body.map_err(|e| Box::new(e) as crate::BoxError).boxed(),
        )
    }

    /// Serves on a Unix socket at `path` (mode 0600, directory 0700) until
    /// the handle is dropped. A live socket there means another daemon is
    /// serving: an error, never a takeover.
    #[cfg(unix)]
    pub async fn serve_unix(self: Arc<Self>, path: &Path) -> crate::Result<GatewayHandle> {
        use std::os::unix::fs::PermissionsExt as _;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        if path.exists() {
            if tokio::net::UnixStream::connect(path).await.is_ok() {
                return Err(crate::Error::Store(format!(
                    "{} is already served by another process",
                    path.display()
                )));
            }
            std::fs::remove_file(path)?;
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        let gw = self;
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let gw = gw.clone();
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                        let gw = gw.clone();
                        async move { Ok::<_, std::convert::Infallible>(gw.handle(req).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
                        .with_upgrades()
                        .await;
                });
            }
        });
        Ok(GatewayHandle {
            path: path.to_path_buf(),
            task: Some(task),
        })
    }
}

/// A running gateway listener; dropping it stops it and removes the socket.
pub struct GatewayHandle {
    path: PathBuf,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl GatewayHandle {
    /// The socket.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for GatewayHandle {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(all(test, unix))]
mod tests;
