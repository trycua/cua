//! `cua share` in the daemon: the share records, the gateway that serves
//! them on a Unix socket, the port forwards into Spaces, and the sweep that
//! ends shares at their TTL. The host's relay-joined cua-spacesd forwards
//! `/share/...` to the gateway; this module never listens on a network
//! address.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_share::{
    Access, Gateway, GatewayConfig, GatewayHandle, Owner, REFUSED_TCP_PORTS, Share, ShareStore,
    TCP_TTL, Target, TcpEndpoint, UpstreamResolver, check_ttl, random_hex,
};

/// Public TCP shares one tree of Spaces (a Space and every child it made)
/// may hold at once. The relay also caps each account and machine.
pub const TCP_SHARES_PER_TREE: usize = 2;

/// How often the daemon renews its TCP shares' leases on the relay (the
/// relay closes a port whose lease lapses: a crashed daemon or a computer
/// cut off loses its ports within the lease).
pub const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(3 * 60);

use crate::runtime::Runtime;
use crate::{Error, Result};

mod routes;
pub use routes::{EDGE_JOIN_WAIT, PENDING_TTL, budget, utc_day, waiting_for};

/// A Space port the gateway reaches through a daemon forward.
struct SpaceForward {
    forward_id: String,
    base: String,
}

/// The daemon's shares.
pub struct ShareHub {
    home: PathBuf,
    store: Arc<ShareStore>,
    audit: Arc<AuditFile>,
    space_forwards: Mutex<HashMap<(String, u16), SpaceForward>>,
    /// Guards the side files (waiting shares).
    side: Mutex<()>,
    /// This hub, for work it hands to background tasks.
    me: std::sync::Weak<ShareHub>,
    /// The owner's clouds, for the host and edge routes.
    #[cfg(feature = "byoc")]
    cloud: Arc<cua_byoc::cloud_shares::CloudShares>,
    // Held only to keep the gateway running (dropping it stops it); the gateway
    // starts only on Unix today.
    #[cfg_attr(not(unix), allow(dead_code))]
    gateway: Mutex<Option<GatewayHandle>>,
}

/// The hash-chained share audit: `<cua home>/share/audit.jsonl`.
pub struct AuditFile {
    log: cua_spaces::audit::AuditLog,
}

impl cua_share::AuditSink for AuditFile {
    fn record(&self, principal: &str, action: &str, handle: &str, detail: &str) {
        if let Err(e) = self.log.append(principal, action, handle, detail) {
            tracing::warn!(error = %e, "share audit write failed");
        }
    }
}

/// What a create asked for, after the surface parsed its input.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateShare {
    /// The target (paths absolute).
    pub target: Target,
    /// Who may open it.
    pub access: Access,
    /// Lifetime; public shares default to one hour.
    pub ttl: Option<Duration>,
    /// A name for lists.
    pub name: Option<String>,
    /// A public TCP share (a raw TCP service such as a game server) instead
    /// of an HTTP link. Needs a Space port, public access and the
    /// experiment.
    pub tcp: Option<TcpRequest>,
    /// The Space asking, when an agent in it asks through its spawn
    /// channel: the target must be in its own tree, and the share ends with
    /// it.
    pub origin: Option<String>,
}

/// What a public TCP share asks for.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TcpRequest {
    /// SRV service (`minecraft`): also a name people can type instead of
    /// the port.
    pub service: Option<String>,
    /// `auto` (or empty), `host`, `edge` or `relay`.
    #[serde(default)]
    pub route: String,
    /// Who may connect (CIDRs); `None` or empty: the settings' default.
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    /// Drop Minecraft Java connections that did not dial the address.
    #[serde(default)]
    pub check_host: bool,
    /// The owner agrees now to the share edge the route needs (never set
    /// on a Space's spawn channel).
    #[serde(default)]
    pub consent: bool,
}

/// The outcome of opening a link once.
#[derive(Clone, Debug)]
pub struct CheckResult {
    /// HTTP status.
    pub status: u16,
    /// 2xx.
    pub ok: bool,
    /// `link` or `local`.
    pub via: String,
    /// A human line.
    pub detail: String,
}

struct Resolver {
    rt: Runtime,
    hub: std::sync::Weak<ShareHub>,
}

impl UpstreamResolver for Resolver {
    fn resolve_space<'a>(
        &'a self,
        space: &'a str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let Some(hub) = self.hub.upgrade() else {
                return Err("the daemon is shutting down".to_string());
            };
            hub.space_base(&self.rt, space, port)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| *v > 0.0)
}

impl ShareHub {
    /// Opens the records under `<home>/share`.
    pub fn open(home: &std::path::Path) -> Result<Arc<Self>> {
        let dir = home.join("share");
        let store = ShareStore::open(dir.join("shares.json"))
            .map_err(|e| Error::Internal(e.to_string()))?;
        let audit = Arc::new(AuditFile {
            log: cua_spaces::audit::AuditLog::new(dir.join("audit.jsonl")),
        });
        Ok(Arc::new_cyclic(|me| Self {
            home: home.to_path_buf(),
            store: Arc::new(store),
            audit,
            space_forwards: Mutex::new(HashMap::new()),
            side: Mutex::new(()),
            me: me.clone(),
            #[cfg(feature = "byoc")]
            cloud: Arc::new(cua_byoc::cloud_shares::CloudShares::builtin(home)),
            gateway: Mutex::new(None),
        }))
    }

    /// The gateway socket.
    pub fn socket(&self) -> PathBuf {
        self.home.join("share").join("gateway.sock")
    }

    /// Starts the gateway on its socket and the TTL sweep. A socket another
    /// daemon already serves is left alone (this daemon then has no shares).
    pub async fn start(self: &Arc<Self>, rt: &Runtime) -> Result<()> {
        let mut cfg = GatewayConfig::default();
        if let Some(v) = env_f64("CUA_SHARE_PUBLIC_RPS") {
            cfg.public_rps = v;
        }
        if let Some(v) = env_f64("CUA_SHARE_PUBLIC_BURST") {
            cfg.public_burst = v;
        }
        let gw = Arc::new(Gateway::new(
            self.store.clone(),
            Arc::new(Resolver {
                rt: rt.clone(),
                hub: Arc::downgrade(self),
            }),
            self.audit.clone(),
            cfg,
        ));
        #[cfg(unix)]
        {
            let handle = gw
                .serve_unix(&self.socket())
                .await
                .map_err(|e| Error::Internal(format!("share gateway: {e}")))?;
            *self.gateway.lock().unwrap() = Some(handle);
        }
        #[cfg(not(unix))]
        let _ = gw;
        let hub = Arc::downgrade(self);
        let rt2 = rt.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                let Some(hub) = hub.upgrade() else { return };
                hub.sweep(&rt2);
                hub.sweep_tcp(&rt2).await;
            }
        });
        let hub = Arc::downgrade(self);
        let rt = rt.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(TCP_KEEPALIVE_INTERVAL);
            loop {
                tick.tick().await;
                let Some(hub) = hub.upgrade() else { return };
                hub.keepalive_tcp(&rt).await;
            }
        });
        Ok(())
    }

    /// Ends TCP shares whose Space, or whose asking Space, is gone. (A
    /// deleted Space ends its shares through [`ShareHub::stop_space`]; this
    /// catches what that missed: a Space that vanished while the daemon was
    /// down, or a tree whose root went away.)
    pub async fn sweep_tcp(&self, rt: &Runtime) {
        let shares: Vec<Share> = self
            .store
            .list()
            .into_iter()
            .filter(|s| s.tcp.is_some() || s.origin.is_some())
            .collect();
        for s in shares {
            let mut gone = false;
            if let Target::SpacePort { space, .. } = &s.target {
                gone |= rt.resolve_key(space).await.is_err();
            }
            if let Some(origin) = &s.origin {
                gone |= rt.resolve_key(origin).await.is_err();
            }
            if gone {
                let removed = self
                    .store
                    .remove_where(|o| o.handle == s.handle)
                    .unwrap_or_default();
                for r in &removed {
                    self.audit_record("system", "stop", r);
                    self.release_forward(rt, r);
                    self.release_tcp(rt, r).await;
                }
            }
        }
    }

    /// Ends the shares whose time is up.
    pub fn sweep(&self, rt: &Runtime) {
        match self.store.purge_expired(cua_share::now_ms()) {
            Ok(gone) => {
                for s in &gone {
                    self.audit_record("system", "expired", s);
                    self.release_forward(rt, s);
                }
                self.spawn_release_tcp(rt, &gone);
            }
            Err(e) => tracing::warn!(error = %e, "share sweep failed"),
        }
    }

    fn audit_record(&self, principal: &str, action: &str, s: &Share) {
        use cua_share::AuditSink as _;
        self.audit
            .record(principal, action, &s.handle, &audit_detail(s));
    }

    async fn space_base(&self, rt: &Runtime, space: &str, port: u16) -> Result<String> {
        let key = (space.to_string(), port);
        if let Some(f) = self.space_forwards.lock().unwrap().get(&key) {
            return Ok(f.base.clone());
        }
        let info = rt.forward(space, port).await?;
        let base = info
            .local_addr
            .as_ref()
            .map(|a| format!("http://{a}"))
            .or_else(|| info.url.clone())
            .ok_or_else(|| Error::Internal("the port forward has no address".into()))?;
        let mut g = self.space_forwards.lock().unwrap();
        if let Some(existing) = g.get(&key) {
            // Lost a race: keep the first forward, close ours.
            let _ = rt.close_forward(&info.id);
            return Ok(existing.base.clone());
        }
        g.insert(
            key,
            SpaceForward {
                forward_id: info.id,
                base: base.clone(),
            },
        );
        Ok(base)
    }

    /// Closes the forward of `s` when no other share uses it.
    fn release_forward(&self, rt: &Runtime, s: &Share) {
        let Target::SpacePort { space, port } = &s.target else {
            return;
        };
        let still_used = self.store.list().iter().any(|o| {
            matches!(&o.target, Target::SpacePort { space: sp, port: p } if sp == space && p == port)
        });
        if still_used {
            return;
        }
        if let Some(f) = self
            .space_forwards
            .lock()
            .unwrap()
            .remove(&(space.clone(), *port))
        {
            let _ = rt.close_forward(&f.forward_id);
        }
    }

    /// Shares the target and returns the share with its link.
    pub async fn create(&self, rt: &Runtime, req: CreateShare) -> Result<Share> {
        if req.tcp.is_some() {
            return self.create_tcp(rt, req, false, None).await;
        }
        let ttl = match (req.access, req.ttl) {
            (_, Some(t)) => Some(check_ttl(t).map_err(|e| Error::InvalidArgument(e.to_string()))?),
            (Access::Public, None) => Some(cua_share::DEFAULT_PUBLIC_TTL),
            _ => None,
        };
        let target = self.validate_target(rt, req.target).await?;
        if let Some(origin) = &req.origin {
            self.check_origin(rt, origin, &target).await?;
        }
        let ctx = crate::share_link::LinkContext::resolve(rt, &self.home).await?;
        let teams = if req.access == Access::Team {
            ctx.owner_teams().await?
        } else {
            Vec::new()
        };
        let key = if req.access == Access::Public {
            random_hex(cua_share::TOKEN_LEN / 2)
        } else {
            random_hex(cua_share::ID_LEN / 2)
        };
        let url = ctx.link(&key, req.access);
        let now = cua_share::now_ms();
        let share = Share {
            handle: format!("share-{}", random_hex(6)),
            key,
            name: req.name.filter(|n| !n.trim().is_empty()),
            target,
            access: req.access,
            owner: Owner {
                account: ctx.owner_account.clone(),
                email: ctx.owner_email.clone(),
            },
            teams,
            created_ms: now,
            expires_ms: ttl.map(|t| now + t.as_millis() as u64),
            url,
            tcp: None,
            origin: req.origin.clone(),
            waiting: None,
        };
        if let Target::SpacePort { space, port } = &share.target {
            // Fail now, not on the first visit, when the port is unreachable.
            self.space_base(rt, space, *port).await?;
        }
        if let Err(e) = self.store.add(share.clone()) {
            return Err(Error::Internal(e.to_string()));
        }
        let principal = match &share.origin {
            Some(origin) => format!("{} (agent in {origin})", ctx.principal()),
            None => ctx.principal(),
        };
        self.audit_record(&principal, "create", &share);
        Ok(share)
    }

    /// A Space asking through its channel may only share a port of a Space
    /// in its own tree (itself or a child it made, at any depth).
    async fn check_origin(&self, rt: &Runtime, origin: &str, target: &Target) -> Result<()> {
        let Target::SpacePort { space, .. } = target else {
            return Err(Error::PermissionDenied(
                "an agent in a Space can only share a port of its own Space or its children".into(),
            ));
        };
        let origin = rt.resolve_key(origin).await?;
        if space == &origin {
            return Ok(());
        }
        let mut at = space.clone();
        // Walk up from the target to the root: the asking Space must be on
        // the way. Bounded by the tree's depth limit.
        for _ in 0..16 {
            let parent = rt
                .spaces()
                .registry()
                .meta(&at)
                .and_then(|m| m.parent.clone());
            match parent {
                Some(p) if p == origin => return Ok(()),
                Some(p) => at = p,
                None => break,
            }
        }
        // Same answer as a Space that does not exist.
        Err(Error::NotFound(format!(
            "no Space {space:?} among this Space's children"
        )))
    }

    /// At most [`TCP_SHARES_PER_TREE`] public TCP shares per tree.
    fn check_tree_cap(&self, rt: &Runtime, target: &Target) -> Result<()> {
        let Target::SpacePort { space, .. } = target else {
            return Ok(());
        };
        let root = tree_root(rt.spaces(), space);
        let in_tree = self
            .store
            .list()
            .iter()
            .filter(|s| s.tcp.is_some())
            .filter(|s| match &s.target {
                Target::SpacePort { space, .. } => tree_root(rt.spaces(), space) == root,
                _ => false,
            })
            .count();
        if in_tree >= TCP_SHARES_PER_TREE {
            return Err(Error::HostCapacity(format!(
                "this tree of Spaces already has {in_tree} public TCP shares (the limit is {TCP_SHARES_PER_TREE}); stop one first"
            )));
        }
        Ok(())
    }

    async fn validate_target(&self, rt: &Runtime, target: Target) -> Result<Target> {
        match target {
            Target::Port { port } => {
                if port == 0 {
                    return Err(Error::InvalidArgument("port must be 1-65535".into()));
                }
                Ok(Target::Port { port })
            }
            Target::SpacePort { space, port } => {
                if port == 0 {
                    return Err(Error::InvalidArgument("port must be 1-65535".into()));
                }
                // The qualified ref, so a deleted Space ends exactly its shares.
                let key = rt.resolve_key(&space).await?;
                Ok(Target::SpacePort { space: key, port })
            }
            Target::File { path } | Target::Dir { path } => {
                if !path.is_absolute() {
                    return Err(Error::InvalidArgument(
                        "the path must be absolute (the client resolves it)".into(),
                    ));
                }
                let real = std::fs::canonicalize(&path)
                    .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
                let meta = std::fs::metadata(&real)
                    .map_err(|e| Error::InvalidArgument(format!("{}: {e}", real.display())))?;
                if meta.is_dir() {
                    Ok(Target::Dir { path: real })
                } else if meta.is_file() {
                    Ok(Target::File { path: real })
                } else {
                    Err(Error::InvalidArgument(format!(
                        "{} is neither a regular file nor a folder",
                        real.display()
                    )))
                }
            }
        }
    }

    /// Active shares, newest first.
    pub fn list(&self) -> Vec<Share> {
        let now = cua_share::now_ms();
        let mut v: Vec<Share> = self
            .store
            .list()
            .into_iter()
            .filter(|s| !s.expired(now))
            .chain(self.pending().into_iter().map(|p| p.share))
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.created_ms));
        v
    }

    /// Active shares a Space's agent made (its channel lists only these).
    pub fn list_from(&self, origin: &str) -> Vec<Share> {
        self.list()
            .into_iter()
            .filter(|s| s.origin.as_deref() == Some(origin))
            .collect()
    }

    /// One share by handle or unique name.
    pub fn find(&self, id: &str) -> Result<Share> {
        if let Some(s) = self.store.by_handle(id) {
            return Ok(s);
        }
        let named: Vec<Share> = self
            .store
            .list()
            .into_iter()
            .filter(|s| s.name.as_deref() == Some(id))
            .collect();
        match named.len() {
            0 => Err(Error::NotFound(format!(
                "no share {id}; `cua share ls` lists them"
            ))),
            1 => Ok(named.into_iter().next().unwrap()),
            _ => Err(Error::InvalidArgument(format!(
                "{id:?} names {} shares; use the id from `cua share ls`",
                named.len()
            ))),
        }
    }

    /// Stops one share (by id or name) or all of them.
    pub fn stop(&self, rt: &Runtime, id: Option<&str>, all: bool) -> Result<Vec<Share>> {
        // A share still waiting for the owner just goes away.
        if !all
            && let Some(id) = id
            && self.pending().iter().any(|p| p.share.handle == id)
        {
            return Ok(vec![self.deny(id)?]);
        }
        if all {
            for p in self.pending() {
                let _ = self.deny(&p.share.handle);
            }
        }
        let gone = if all {
            self.store.remove_where(|_| true)
        } else {
            let id = id.ok_or_else(|| Error::InvalidArgument("say which share to stop".into()))?;
            let handle = self.find(id)?.handle;
            self.store.remove_where(|s| s.handle == handle)
        }
        .map_err(|e| Error::Internal(e.to_string()))?;
        for s in &gone {
            self.audit_record("user", "stop", s);
            self.release_forward(rt, s);
        }
        self.spawn_release_tcp(rt, &gone);
        Ok(gone)
    }

    /// Ends the public side of `shares` (TCP shares) in the background.
    fn spawn_release_tcp(&self, rt: &Runtime, shares: &[Share]) {
        let tcp: Vec<Share> = shares.iter().filter(|s| s.tcp.is_some()).cloned().collect();
        if tcp.is_empty() {
            return;
        }
        let Some(hub) = self.me.upgrade() else { return };
        let rt = rt.clone();
        tokio::spawn(async move {
            for s in tcp {
                hub.release_tcp(&rt, &s).await;
            }
        });
    }

    /// Stops shares a Space's agent made: one (by id or name) or all.
    /// Shares made by anyone else are "not found" to it.
    pub fn stop_from(
        &self,
        rt: &Runtime,
        origin: &str,
        id: Option<&str>,
        all: bool,
    ) -> Result<Vec<Share>> {
        let mine = self.list_from(origin);
        let handles: Vec<String> = if all {
            mine.iter().map(|s| s.handle.clone()).collect()
        } else {
            let id = id.ok_or_else(|| Error::InvalidArgument("say which share to stop".into()))?;
            let hit: Vec<&Share> = mine
                .iter()
                .filter(|s| s.handle == id || s.name.as_deref() == Some(id))
                .collect();
            match hit.as_slice() {
                [one] => vec![one.handle.clone()],
                [] => {
                    return Err(Error::NotFound(format!("no share {id} made in this Space")));
                }
                _ => {
                    return Err(Error::InvalidArgument(format!(
                        "{id:?} names several shares; use the id"
                    )));
                }
            }
        };
        let mut withdrawn = Vec::new();
        for p in self.pending() {
            if handles.contains(&p.share.handle) {
                withdrawn.extend(self.take_pending_share(&p.share.handle));
            }
        }
        let mut gone = self
            .store
            .remove_where(|s| handles.contains(&s.handle))
            .map_err(|e| Error::Internal(e.to_string()))?;
        for s in &gone {
            self.audit_record(&format!("agent in {origin}"), "stop", s);
            self.release_forward(rt, s);
        }
        self.spawn_release_tcp(rt, &gone);
        for s in &withdrawn {
            self.audit_record(&format!("agent in {origin}"), "withdraw", s);
        }
        gone.extend(withdrawn);
        Ok(gone)
    }

    /// Ends the shares that point into the Space `key` (it was deleted).
    pub fn stop_space(&self, rt: &Runtime, key: &str) {
        for p in self.pending() {
            let into = matches!(&p.share.target, Target::SpacePort { space, .. } if space == key)
                || p.share.origin.as_deref() == Some(key);
            if into {
                let _ = self.take_pending_share(&p.share.handle);
            }
        }
        let gone = self
            .store
            .remove_where(|s| {
                matches!(&s.target, Target::SpacePort { space, .. } if space == key)
                    || s.origin.as_deref() == Some(key)
            })
            .unwrap_or_default();
        for s in &gone {
            self.audit_record("system", "stop", s);
        }
        self.spawn_release_tcp(rt, &gone);
        let mut g = self.space_forwards.lock().unwrap();
        let dead: Vec<(String, u16)> = g.keys().filter(|(sp, _)| sp == key).cloned().collect();
        for k in dead {
            if let Some(f) = g.remove(&k) {
                let _ = rt.close_forward(&f.forward_id);
            }
        }
    }

    /// Opens the share's link once, as its owner would.
    pub async fn check(&self, rt: &Runtime, id: &str, path: &str) -> Result<CheckResult> {
        let share = self.find(id)?;
        let rel = path.trim_start_matches('/');
        let link = format!("{}{}", share.url, rel);
        if let Ok(ctx) = crate::share_link::LinkContext::resolve(rt, &self.home).await {
            let bearer = if share.access == Access::Public {
                None
            } else {
                Some(ctx.access_token().await?)
            };
            match ctx
                .client
                .open_link(&link, bearer.as_deref(), 1 << 20)
                .await
            {
                Ok(p) => {
                    let ok = (200..300).contains(&p.status);
                    return Ok(CheckResult {
                        status: p.status,
                        ok,
                        via: "link".into(),
                        detail: format!(
                            "HTTP {} {} ({} bytes)",
                            p.status,
                            if p.content_type.is_empty() {
                                "-"
                            } else {
                                &p.content_type
                            },
                            p.bytes
                        ),
                    });
                }
                Err(e) => {
                    return Ok(CheckResult {
                        status: 0,
                        ok: false,
                        via: "link".into(),
                        detail: format!("could not open the link: {e}"),
                    });
                }
            }
        }
        // No relay account on this daemon: ask the gateway directly, as the owner.
        let pq = if share.access == Access::Public {
            format!("/share/pub/{}/{rel}", share.key)
        } else {
            format!("/share/{}/{rel}", share.key)
        };
        let caller = cua_share::Caller {
            account: share.owner.account.clone(),
            email: share.owner.email.clone(),
            role: "owner".into(),
            via: "local".into(),
            ..Default::default()
        };
        #[cfg(unix)]
        let r = cua_share::probe_unix(&self.socket(), &pq, Some(&caller)).await;
        #[cfg(not(unix))]
        let r: std::result::Result<cua_share::Probe, String> = {
            let _ = (&pq, &caller);
            Err("checking a link on this machine is not available on Windows yet; open the link from another device to check it".into())
        };
        Ok(match r {
            Ok(p) => CheckResult {
                status: p.status,
                ok: (200..300).contains(&p.status),
                via: "local".into(),
                detail: format!(
                    "HTTP {} {} ({} bytes), checked on this machine only",
                    p.status, p.content_type, p.bytes
                ),
            },
            Err(e) => CheckResult {
                status: 0,
                ok: false,
                via: "local".into(),
                detail: e,
            },
        })
    }
}

/// The audit line of a share: access and target, and for a TCP share its
/// route, address and allowlist.
fn audit_detail(s: &Share) -> String {
    let mut d = format!("{} {}", s.access.as_str(), s.target.describe());
    if let Some(t) = &s.tcp {
        d.push_str(&format!(" route={} address={}", t.route, t.address));
        if let Some(c) = &t.cloud {
            d.push_str(&format!(" cloud={c}"));
        }
        if !t.allow.is_empty() {
            d.push_str(&format!(" allow={}", t.allow.join(",")));
        }
    }
    if let Some(w) = &s.waiting {
        d.push_str(&format!(" waiting={}", w.reason));
    }
    d
}

/// The root of the tree of Spaces `id` is in (itself when it has no
/// parent): the record's `root`, else its `cua.root` label (a child placed
/// elsewhere), else the top of its parent chain.
fn tree_root(spaces: &cua_spaces::Spaces, id: &str) -> String {
    let meta = spaces.registry().meta(id);
    if let Some(root) = meta.as_ref().and_then(|m| m.root.clone()) {
        return root;
    }
    if let Some(root) = meta
        .as_ref()
        .and_then(|m| m.labels.get("cua.root").cloned())
    {
        return root;
    }
    let mut at = id.to_owned();
    for _ in 0..16 {
        match spaces.registry().meta(&at).and_then(|m| m.parent.clone()) {
            Some(p) => at = p,
            None => break,
        }
    }
    at
}

/// The rules of a public TCP share that need nothing but the request: the
/// experiment is on, the target is a Space port (never this computer), the
/// port is not an admin or remote-access one, and the share is public.
pub fn check_tcp_request(req: &CreateShare) -> Result<()> {
    if !cua_vmm::experiments::enabled(cua_vmm::experiments::Gate::TcpShares) {
        return Err(Error::Unsupported(
            cua_vmm::experiments::Gate::TcpShares.off_message(),
        ));
    }
    let Target::SpacePort { port, .. } = &req.target else {
        return Err(Error::InvalidArgument(
            "a public TCP share points at a port in a Space (space:<name>:<port>); it never exposes this computer".into(),
        ));
    };
    if REFUSED_TCP_PORTS.contains(port) {
        return Err(Error::InvalidArgument(format!(
            "port {port} is an admin or remote-access port (a shell, desktop, game console or database) and cannot be shared publicly"
        )));
    }
    if req.access != Access::Public {
        return Err(Error::InvalidArgument(
            "a TCP share is public: anyone with the address can connect (add --public)".into(),
        ));
    }
    if let Some(s) = req.tcp.as_ref().and_then(|t| t.service.as_deref())
        && !(1..=15).contains(&s.len())
    {
        return Err(Error::InvalidArgument(
            "the service name is 1-15 letters (for example minecraft)".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tcp_tests {
    use super::*;

    #[test]
    fn the_audit_names_the_route_address_and_allowlist() {
        let mut s = Share {
            handle: "share-1".into(),
            key: "k".into(),
            name: None,
            target: Target::SpacePort {
                space: "local:mc".into(),
                port: 25565,
            },
            access: Access::Public,
            owner: Owner {
                account: "a".into(),
                email: None,
            },
            teams: vec![],
            created_ms: 0,
            expires_ms: None,
            url: String::new(),
            tcp: Some(TcpEndpoint {
                address: "203.0.113.4:26500".into(),
                host: "203.0.113.4".into(),
                port: 26500,
                srv_name: None,
                service: Some("minecraft".into()),
                route: "host".into(),
                cloud: Some("aws".into()),
                route_line: "Through your AWS account".into(),
                allow: vec!["198.51.100.0/24".into()],
                check_host: true,
                max_bytes: 0,
                slot: 26500,
            }),
            origin: None,
            waiting: None,
        };
        let d = audit_detail(&s);
        assert!(
            d.contains("route=host") && d.contains("address=203.0.113.4:26500"),
            "{d}"
        );
        assert!(
            d.contains("cloud=aws") && d.contains("allow=198.51.100.0/24"),
            "{d}"
        );
        s.tcp = None;
        s.waiting = Some(cua_share::Waiting {
            reason: "approval".into(),
            text: String::new(),
            route_line: String::new(),
        });
        assert!(audit_detail(&s).ends_with("waiting=approval"));
    }

    fn req(target: Target, access: Access) -> CreateShare {
        CreateShare {
            target,
            access,
            ttl: None,
            name: None,
            tcp: Some(TcpRequest {
                service: Some("minecraft".into()),
                ..Default::default()
            }),
            origin: None,
        }
    }

    #[test]
    fn tcp_shares_are_an_experiment_for_space_ports_only_and_public() {
        cua_vmm::experiments::set_enabled(cua_vmm::experiments::Gate::TcpShares, Some(false));
        let space = Target::SpacePort {
            space: "local:mc".into(),
            port: 25565,
        };
        let off = check_tcp_request(&req(space.clone(), Access::Public)).unwrap_err();
        assert!(off.to_string().contains("experiment"), "{off}");
        cua_vmm::experiments::set_enabled(cua_vmm::experiments::Gate::TcpShares, Some(true));
        assert!(check_tcp_request(&req(space.clone(), Access::Public)).is_ok());
        assert!(check_tcp_request(&req(Target::Port { port: 25565 }, Access::Public)).is_err());
        assert!(check_tcp_request(&req(space, Access::Private)).is_err());
        for admin in [22, 25575, 3389, 5900] {
            let t = Target::SpacePort {
                space: "local:mc".into(),
                port: admin,
            };
            assert!(
                check_tcp_request(&req(t, Access::Public)).is_err(),
                "{admin}"
            );
        }
        cua_vmm::experiments::set_enabled(cua_vmm::experiments::Gate::TcpShares, None);
    }
}
