// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Your own tailnet (Tailscale or Headscale) as the identity of callers.
//!
//! A host set up with `cua host setup --tailnet` serves its direct listener
//! on the machine's tailnet address. With this authenticator, a call from
//! a tailnet peer is identified by the tailnet itself: the local
//! `tailscale whois` (the official client, reached as a separate program,
//! never linked) says which user owns the calling node, and the host
//! policy's owner and share lists (by login name, usually an email) decide
//! what that user may do. The tailnet's own ACLs decide who can reach the
//! machine at all, and its identity provider (SSO, MFA) who those users
//! are, so teammates need no env token. A caller the tailnet does not
//! vouch for still can present the env token, as before.
//!
//! The peer address comes from [`PEER_HEADER`], which the server sets on
//! every request from the TCP connection (and strips from what clients
//! send), so a caller cannot claim another address.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use cua_proto::env::v1::{Principal, PrincipalKind};

use crate::auth::{ExternalAuthenticator, ExternalGrant, RelayCaller};
use crate::relay_account::{color_for, HostPolicy};

/// The request header the server sets to the TCP peer (`ip:port`).
pub const PEER_HEADER: &str = "x-cua-peer-addr";

/// How long a `whois` answer is reused.
const CACHE: Duration = Duration::from_secs(30);

/// Who owns a tailnet node, per `tailscale whois`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoIs {
    /// The user's login name (`alice@example.com`, or `tagged-devices`).
    pub login: String,
    /// Display name.
    pub name: String,
    /// The node's tags (a tagged node has no user).
    pub tags: Vec<String>,
}

/// True for Tailscale's address ranges (100.64.0.0/10, fd7a:115c:a1e0::/48),
/// which Headscale uses too.
pub fn is_tailnet(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (b & 0xc0) == 64
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_tailnet(IpAddr::V4(v4));
            }
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

/// Parses `tailscale whois --json` output.
pub fn parse_whois(json: &str) -> Option<WhoIs> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let profile = v.get("UserProfile")?;
    let login = profile.get("LoginName")?.as_str()?.to_owned();
    let name = profile
        .get("DisplayName")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_owned();
    let tags = v
        .get("Node")
        .and_then(|n| n.get("Tags"))
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Some(WhoIs { login, name, tags })
}

/// The `tailscale` CLI: `CUA_TAILSCALE_BIN`, else `tailscale` on `PATH`,
/// else the macOS app's.
fn tailscale_bins() -> Vec<String> {
    if let Ok(bin) = std::env::var("CUA_TAILSCALE_BIN") {
        if !bin.trim().is_empty() {
            return vec![bin];
        }
    }
    vec![
        "tailscale".into(),
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale".into(),
    ]
}

/// Asks the local tailnet client who owns `peer`.
pub fn whois(peer: SocketAddr) -> Option<WhoIs> {
    for bin in tailscale_bins() {
        let Ok(out) = std::process::Command::new(&bin)
            .args(["whois", "--json", &peer.to_string()])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        if out.status.success() {
            return parse_whois(&String::from_utf8_lossy(&out.stdout));
        }
    }
    None
}

struct Cached {
    at: Instant,
    who: Option<WhoIs>,
}

struct CachedPolicy {
    modified: Option<(SystemTime, u64)>,
    policy: HostPolicy,
}

/// The authenticator a `--tailnet` host installs.
pub struct TailnetAuth {
    policy_file: PathBuf,
    cache: Mutex<HashMap<IpAddr, Cached>>,
    policy: Mutex<CachedPolicy>,
    lookup: Box<dyn Fn(SocketAddr) -> Option<WhoIs> + Send + Sync>,
}

impl TailnetAuth {
    /// Uses the local `tailscale whois` and `policy_file` (the host policy,
    /// re-read when it changes).
    pub fn new(policy_file: PathBuf) -> Self {
        Self::with_lookup(policy_file, whois)
    }

    /// With another lookup (tests).
    pub fn with_lookup(
        policy_file: PathBuf,
        lookup: impl Fn(SocketAddr) -> Option<WhoIs> + Send + Sync + 'static,
    ) -> Self {
        Self {
            policy_file,
            cache: Mutex::new(HashMap::new()),
            policy: Mutex::new(CachedPolicy {
                modified: None,
                policy: HostPolicy::default(),
            }),
            lookup: Box::new(lookup),
        }
    }

    fn policy(&self) -> Result<HostPolicy, String> {
        let path = &self.policy_file;
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let modified = meta.modified().ok().map(|m| (m, meta.len()));
        let mut cache = self.policy.lock().expect("policy");
        if cache.modified.is_none() || cache.modified != modified {
            let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            cache.policy =
                serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
            cache.modified = modified;
        }
        Ok(cache.policy.clone())
    }

    fn who(&self, peer: SocketAddr) -> Option<WhoIs> {
        {
            let cache = self.cache.lock().expect("cache");
            if let Some(c) = cache.get(&peer.ip()) {
                if c.at.elapsed() < CACHE {
                    return c.who.clone();
                }
            }
        }
        let who = (self.lookup)(peer);
        let mut cache = self.cache.lock().expect("cache");
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(
            peer.ip(),
            Cached {
                at: Instant::now(),
                who: who.clone(),
            },
        );
        who
    }
}

impl ExternalAuthenticator for TailnetAuth {
    fn share_gateway(&self) -> Result<Option<std::path::PathBuf>, String> {
        let policy = self.policy()?;
        if !policy.sharing {
            return Err("this machine stopped sharing".into());
        }
        Ok(policy.shares.then_some(policy.shares_gateway).flatten())
    }

    fn authenticate(&self, headers: &http::HeaderMap) -> Option<Result<ExternalGrant, String>> {
        let peer: SocketAddr = headers.get(PEER_HEADER)?.to_str().ok()?.parse().ok()?;
        if !is_tailnet(peer.ip()) {
            return None;
        }
        // A tagged node acts for no user; it can still use the env token.
        let who = self.who(peer).filter(|w| w.tags.is_empty())?;
        let policy = match self.policy() {
            Ok(p) => p,
            // Fail closed on a broken policy (but leave the token path).
            Err(e) => {
                tracing::warn!(error = %e, "tailnet identity: unreadable host policy");
                return None;
            }
        };
        if !policy.sharing {
            return None;
        }
        let login = who.login.as_str();
        let is = |a: &str| a.eq_ignore_ascii_case(login);
        let owner = policy.owner_email.as_deref().is_some_and(is)
            || policy.owner.as_deref().is_some_and(is);
        let viewer = !owner && policy.viewers.iter().any(|a| is(a));
        let allowed = owner || viewer || policy.allow.iter().any(|a| is(a));
        if !allowed {
            // Not shared with this user: the env token (if any) decides.
            return None;
        }
        let principal = Principal {
            id: format!("tailnet:{}", login.to_ascii_lowercase()),
            display_name: if who.name.is_empty() {
                login.to_owned()
            } else {
                who.name.clone()
            },
            color: color_for(login),
            kind: PrincipalKind::Human as i32,
        };
        Some(Ok(ExternalGrant {
            principal,
            view_only: viewer,
            host_only: !policy.share_desktop,
            account: Some(RelayCaller {
                account: login.to_owned(),
                email: login.contains('@').then(|| login.to_owned()),
                name: (!who.name.is_empty()).then(|| who.name.clone()),
                role: if owner { "owner" } else { "shared" }.into(),
                // The tailnet vouched for this caller just now (no relay
                // assertion to date it by).
                issued_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                teams: Vec::new(),
                space: None,
            }),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(peer: &str) -> http::HeaderMap {
        let mut h = http::HeaderMap::new();
        h.insert(PEER_HEADER, peer.parse().unwrap());
        h
    }

    fn auth(policy: &str) -> (TailnetAuth, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.json");
        std::fs::write(&path, policy).unwrap();
        let a = TailnetAuth::with_lookup(path, |peer| match peer.ip().to_string().as_str() {
            "100.101.1.1" => Some(WhoIs {
                login: "Alice@Example.com".into(),
                name: "Alice".into(),
                tags: vec![],
            }),
            "100.101.1.2" => Some(WhoIs {
                login: "bob@example.com".into(),
                name: "Bob".into(),
                tags: vec![],
            }),
            "100.101.1.3" => Some(WhoIs {
                login: "tagged-devices".into(),
                name: String::new(),
                tags: vec!["tag:ci".into()],
            }),
            "100.101.1.4" => Some(WhoIs {
                login: "carol@example.com".into(),
                name: "Carol".into(),
                tags: vec![],
            }),
            _ => None,
        });
        (a, dir)
    }

    #[test]
    fn the_tailnet_identifies_owners_shares_and_viewers() {
        let (a, _d) = auth(
            r#"{"owner_email":"alice@example.com","allow":["bob@example.com"],"viewers":["carol@example.com"],"tailnet_identity":true}"#,
        );
        let g = a
            .authenticate(&headers("100.101.1.1:5000"))
            .unwrap()
            .unwrap();
        assert_eq!(g.account.as_ref().unwrap().role, "owner");
        assert!(!g.view_only);
        assert_eq!(g.principal.display_name, "Alice");
        let g = a
            .authenticate(&headers("100.101.1.2:5000"))
            .unwrap()
            .unwrap();
        assert_eq!(g.account.unwrap().role, "shared");
        let g = a
            .authenticate(&headers("100.101.1.4:5000"))
            .unwrap()
            .unwrap();
        assert!(g.view_only);
        // A tagged node, an unknown node, a non-tailnet peer and a request
        // without the server's peer header fall back to the env token.
        assert!(a.authenticate(&headers("100.101.1.3:5000")).is_none());
        assert!(a.authenticate(&headers("100.101.9.9:5000")).is_none());
        assert!(a.authenticate(&headers("192.168.1.5:5000")).is_none());
        assert!(a.authenticate(&http::HeaderMap::new()).is_none());
    }

    #[test]
    fn stopping_sharing_and_unlisted_users_get_nothing() {
        let (a, _d) = auth(r#"{"owner_email":"alice@example.com","sharing":false}"#);
        assert!(a.authenticate(&headers("100.101.1.1:5000")).is_none());
        let (a, _d) = auth(r#"{"owner_email":"alice@example.com"}"#);
        assert!(a.authenticate(&headers("100.101.1.2:5000")).is_none());
    }

    #[test]
    fn whois_output_parses() {
        let json = r#"{"Node":{"Name":"laptop.example.ts.net.","Tags":null},
            "UserProfile":{"ID":1,"LoginName":"alice@example.com","DisplayName":"Alice"}}"#;
        let w = parse_whois(json).unwrap();
        assert_eq!(w.login, "alice@example.com");
        assert!(w.tags.is_empty());
        assert!(is_tailnet("100.64.0.1".parse().unwrap()));
        assert!(is_tailnet("fd7a:115c:a1e0::1".parse().unwrap()));
        assert!(!is_tailnet("100.128.0.1".parse().unwrap()));
        assert!(!is_tailnet("10.0.0.1".parse().unwrap()));
    }
}
