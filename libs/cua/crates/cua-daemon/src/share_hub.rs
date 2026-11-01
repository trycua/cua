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
    Access, Gateway, GatewayConfig, GatewayHandle, Owner, Share, ShareStore, Target,
    UpstreamResolver, check_ttl, random_hex,
};

use crate::runtime::Runtime;
use crate::{Error, Result};

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
#[derive(Clone, Debug)]
pub struct CreateShare {
    /// The target (paths absolute).
    pub target: Target,
    /// Who may open it.
    pub access: Access,
    /// Lifetime; public shares default to one hour.
    pub ttl: Option<Duration>,
    /// A name for lists.
    pub name: Option<String>,
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
        Ok(Arc::new(Self {
            home: home.to_path_buf(),
            store: Arc::new(store),
            audit,
            space_forwards: Mutex::new(HashMap::new()),
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
        let rt = rt.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                let Some(hub) = hub.upgrade() else { return };
                hub.sweep(&rt);
            }
        });
        Ok(())
    }

    /// Ends the shares whose time is up.
    pub fn sweep(&self, rt: &Runtime) {
        match self.store.purge_expired(cua_share::now_ms()) {
            Ok(gone) => {
                for s in gone {
                    self.audit_record("system", "expired", &s);
                    self.release_forward(rt, &s);
                }
            }
            Err(e) => tracing::warn!(error = %e, "share sweep failed"),
        }
    }

    fn audit_record(&self, principal: &str, action: &str, s: &Share) {
        use cua_share::AuditSink as _;
        self.audit.record(
            principal,
            action,
            &s.handle,
            &format!("{} {}", s.access.as_str(), s.target.describe()),
        );
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
        let ttl = match (req.access, req.ttl) {
            (_, Some(t)) => Some(check_ttl(t).map_err(|e| Error::InvalidArgument(e.to_string()))?),
            (Access::Public, None) => Some(cua_share::DEFAULT_PUBLIC_TTL),
            _ => None,
        };
        let target = self.validate_target(rt, req.target).await?;
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
        };
        if let Target::SpacePort { space, port } = &share.target {
            // Fail now, not on the first visit, when the port is unreachable.
            self.space_base(rt, space, *port).await?;
        }
        self.store
            .add(share.clone())
            .map_err(|e| Error::Internal(e.to_string()))?;
        self.audit_record(&ctx.principal(), "create", &share);
        Ok(share)
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
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.created_ms));
        v
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
        Ok(gone)
    }

    /// Ends the shares that point into the Space `key` (it was deleted).
    pub fn stop_space(&self, rt: &Runtime, key: &str) {
        let gone = self
            .store
            .remove_where(|s| matches!(&s.target, Target::SpacePort { space, .. } if space == key))
            .unwrap_or_default();
        for s in &gone {
            self.audit_record("system", "stop", s);
        }
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
