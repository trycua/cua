//! Local public URLs: a loopback reverse proxy with one random token per
//! URL, checked with its expiry on every request.
//!
//! `public_url` on a local sandbox returns `http://127.0.0.1:<port>/s/<token>/`.
//! Requests under that prefix are forwarded to the service's loopback port
//! (the prefix is stripped); anything else answers 404, an expired or
//! revoked token 410. The `cua daemon` hosts the table so URLs outlive the
//! process that created them; an embedded runtime asks the daemon (starting
//! it when it can) and hosts the proxy itself only as a fallback.

use crate::{Error, Result};
use cua_sandbox_core::proxy::{self, HttpProxy, ProxyRoute, RouteDecision, RouteFuture};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

/// Longest a local public URL may live (the same bound as Fleet's signed
/// service URLs).
pub const MAX_TTL: Duration = Duration::from_secs(86_400);
/// Default lifetime.
pub const DEFAULT_TTL: Duration = Duration::from_secs(3_600);

/// One public URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicUrl {
    /// Id for revocation.
    pub id: String,
    /// The URL.
    pub url: String,
    /// When it stops working.
    pub expires_at: SystemTime,
    /// Sandbox name.
    pub sandbox: String,
    /// Service name.
    pub service: String,
    /// Provider internals (cloud: namespace, claim; local: upstream).
    pub provider_details: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct Share {
    id: String,
    upstream: String,
    expires_at: SystemTime,
    sandbox: String,
    service: String,
}

#[derive(Default)]
struct Table {
    by_token: HashMap<String, Share>,
}

/// The share table and its (lazily started) proxy.
#[derive(Clone, Default)]
pub struct Shares {
    table: Arc<Mutex<Table>>,
    proxy: Arc<tokio::sync::Mutex<Option<HttpProxy>>>,
}

impl std::fmt::Debug for Shares {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shares").finish_non_exhaustive()
    }
}

fn random_hex(bytes: usize) -> String {
    (0..bytes)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

impl Shares {
    /// The proxy's base (`http://127.0.0.1:<port>`), starting it on first use
    /// (`CUA_SHARE_ADDR` picks the address; default an ephemeral loopback
    /// port).
    async fn base(&self) -> Result<String> {
        let mut p = self.proxy.lock().await;
        if let Some(p) = p.as_ref() {
            return Ok(p.url());
        }
        let bind: SocketAddr = std::env::var("CUA_SHARE_ADDR")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse()
                    .map_err(|e| Error::InvalidArgument(format!("CUA_SHARE_ADDR {v:?}: {e}")))
            })
            .transpose()?
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)));
        if !bind.ip().is_loopback() {
            return Err(Error::InvalidArgument(
                "CUA_SHARE_ADDR must be a loopback address".into(),
            ));
        }
        let table = self.table.clone();
        let router = move |pq: &str| -> RouteFuture {
            let d = route(&table, pq, SystemTime::now());
            Box::pin(async move { d })
        };
        let started = HttpProxy::start(bind, Arc::new(router))
            .await
            .map_err(|e| Error::Internal(format!("public URL proxy: {e}")))?;
        let url = started.url();
        *p = Some(started);
        Ok(url)
    }

    /// Shares `upstream` (a loopback `http://` URL) for `ttl`.
    pub async fn create(
        &self,
        upstream: &str,
        ttl: Duration,
        sandbox: &str,
        service: &str,
    ) -> Result<PublicUrl> {
        proxy::require_loopback_http(upstream)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        if ttl.is_zero() || ttl > MAX_TTL {
            return Err(Error::InvalidArgument(format!(
                "ttl must be between 1 second and {} seconds",
                MAX_TTL.as_secs()
            )));
        }
        let base = self.base().await?;
        let token = random_hex(24);
        let id = format!("share-{}", random_hex(6));
        let expires_at = SystemTime::now() + ttl;
        let share = Share {
            id: id.clone(),
            upstream: upstream.trim_end_matches('/').to_string(),
            expires_at,
            sandbox: sandbox.into(),
            service: service.into(),
        };
        {
            let mut t = self.table.lock().unwrap();
            let now = SystemTime::now();
            t.by_token.retain(|_, s| s.expires_at > now);
            t.by_token.insert(token.clone(), share.clone());
        }
        Ok(PublicUrl {
            id,
            url: format!("{base}/s/{token}/"),
            expires_at,
            sandbox: share.sandbox,
            service: share.service,
            provider_details: [("upstream".to_string(), share.upstream)].into(),
        })
    }

    /// Revokes by id. `false` when unknown.
    pub fn revoke(&self, id: &str) -> bool {
        let mut t = self.table.lock().unwrap();
        let before = t.by_token.len();
        t.by_token.retain(|_, s| s.id != id);
        before != t.by_token.len()
    }

    /// Revokes every URL of a sandbox (on delete).
    pub fn revoke_sandbox(&self, sandbox: &str) {
        self.table
            .lock()
            .unwrap()
            .by_token
            .retain(|_, s| s.sandbox != sandbox);
    }

    /// Live URLs of a sandbox.
    pub fn count(&self, sandbox: &str) -> usize {
        let now = SystemTime::now();
        self.table
            .lock()
            .unwrap()
            .by_token
            .values()
            .filter(|s| s.sandbox == sandbox && s.expires_at > now)
            .count()
    }
}

fn route(table: &Mutex<Table>, pq: &str, now: SystemTime) -> RouteDecision {
    let not_found = || RouteDecision::Refuse(404, "not found".into());
    let Some(rest) = pq.strip_prefix("/s/") else {
        return not_found();
    };
    let (token, tail) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut t = table.lock().unwrap();
    let Some(share) = t.by_token.get(token).cloned() else {
        return not_found();
    };
    if share.expires_at <= now {
        t.by_token.remove(token);
        return RouteDecision::Refuse(410, "this URL has expired".into());
    }
    let tail = if tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    RouteDecision::Forward(ProxyRoute {
        url: proxy::join_url(&share.upstream, &tail),
        headers: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn get(url: &str) -> (u16, String) {
        let u = url::Url::parse(url).unwrap();
        let mut s = tokio::net::TcpStream::connect((u.host_str().unwrap(), u.port().unwrap()))
            .await
            .unwrap();
        let pq = match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        };
        s.write_all(
            format!("GET {pq} HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
        let text = String::from_utf8_lossy(&out).to_string();
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        (status, text)
    }

    /// Echoes the request line.
    async fn upstream() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let line = String::from_utf8_lossy(&buf[..n])
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{line}",
                        line.len()
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn tokens_gate_expire_and_revoke() {
        let shares = Shares::default();
        let up = upstream().await;
        let u = shares
            .create(&up, Duration::from_secs(60), "sb", "mcp")
            .await
            .unwrap();
        assert!(
            u.url.starts_with("http://127.0.0.1:") && u.url.contains("/s/"),
            "{}",
            u.url
        );
        let (status, body) = get(&format!("{}mcp?x=1", u.url)).await;
        assert_eq!(status, 200, "{body}");
        assert!(body.ends_with("GET /mcp?x=1 HTTP/1.1"), "{body}");
        // Query directly after the token.
        let bare = u.url.trim_end_matches('/');
        let (status, body) = get(&format!("{bare}?q=2")).await;
        assert_eq!(status, 200);
        assert!(body.ends_with("GET /?q=2 HTTP/1.1"), "{body}");
        // A wrong token or another path never reaches the sandbox.
        let base = u.url.split("/s/").next().unwrap().to_string();
        assert_eq!(get(&format!("{base}/s/nope/mcp")).await.0, 404);
        assert_eq!(get(&format!("{base}/mcp")).await.0, 404);
        assert_eq!(shares.count("sb"), 1);
        assert!(shares.revoke(&u.id));
        assert_eq!(get(&u.url).await.0, 404);
        assert!(!shares.revoke(&u.id));
        // Expiry.
        let short = shares
            .create(&up, Duration::from_millis(1), "sb", "mcp")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(get(&short.url).await.0, 410);
        // Only loopback http upstreams, bounded TTLs.
        assert!(
            shares
                .create("http://10.1.1.1:80", DEFAULT_TTL, "sb", "x")
                .await
                .is_err()
        );
        assert!(shares.create(&up, Duration::ZERO, "sb", "x").await.is_err());
        assert!(shares.create(&up, MAX_TTL * 2, "sb", "x").await.is_err());
        let again = shares.create(&up, DEFAULT_TTL, "sb2", "x").await.unwrap();
        shares.revoke_sandbox("sb2");
        assert_eq!(get(&again.url).await.0, 404);
    }
}
