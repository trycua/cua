//! `Cua.shares()`: links to things you run (`cua share`). The cua daemon
//! owns the shares, so a link keeps working after the process that made it
//! exits; an embedded SDK asks the daemon (starting one if needed).

use super::{Backend, Cua, run};
use crate::Result;
use std::time::{SystemTime, UNIX_EPOCH};

/// Who may open a share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ShareAccess {
    /// Only the owner's cua.ai account, on any of the owner's devices.
    Private,
    /// Members of the owner's team.
    Team,
    /// Anyone with the link, until it expires or is stopped.
    Public,
}

impl From<ShareAccess> for cua_daemon_core::ShareAccess {
    fn from(a: ShareAccess) -> Self {
        match a {
            ShareAccess::Private => Self::Private,
            ShareAccess::Team => Self::Team,
            ShareAccess::Public => Self::Public,
        }
    }
}

impl From<cua_daemon_core::ShareAccess> for ShareAccess {
    fn from(a: cua_daemon_core::ShareAccess) -> Self {
        match a {
            cua_daemon_core::ShareAccess::Private => Self::Private,
            cua_daemon_core::ShareAccess::Team => Self::Team,
            cua_daemon_core::ShareAccess::Public => Self::Public,
        }
    }
}

/// One share.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Share {
    /// Handle for `Shares.stop` (`share-...`).
    pub id: String,
    /// The link.
    pub url: String,
    /// Name for lists.
    pub name: Option<String>,
    /// Who may open it.
    pub access: ShareAccess,
    /// `port`, `space_port`, `file` or `dir`.
    pub kind: String,
    /// What it serves (`port 3000`, `space:dev:3000`, a path).
    pub target: String,
    /// The account that made it.
    pub owner: String,
    /// When it was made (unix seconds).
    pub created_at_unix: Option<i64>,
    /// When it ends (unix seconds); none: until stopped.
    pub expires_at_unix: Option<i64>,
    /// Limits of this link worth showing next to it (for example that
    /// browsers cannot open it yet).
    pub notices: Vec<String>,
    /// A public TCP share: what people connect to (a name they can type,
    /// or `host:port`). `None`: an HTTP link.
    pub tcp_connect_with: Option<String>,
    /// A public TCP share: its `host:port`.
    pub tcp_address: Option<String>,
    /// A public TCP share: how people reach it, `host` (the Space's own
    /// host in your cloud), `edge` (a share edge in your cloud) or
    /// `relay` (Cua).
    pub route: Option<String>,
    /// The plain line for that route ("Through your AWS account").
    pub route_line: Option<String>,
    /// Who may connect (CIDRs); empty: anyone.
    pub allow: Vec<String>,
    /// Set while the share waits for its owner: `approval` or `consent`.
    pub waiting: Option<String>,
    /// What to show the owner while it waits.
    pub waiting_text: Option<String>,
}

/// Where public TCP shares land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ShareLanding {
    /// The Space's own cloud host, else a share edge, else Cua's relay.
    Auto,
    /// Only through your own clouds.
    OnlyMyCloud,
    /// Cua's relay unless the Space runs on its own cloud host.
    AllowRelay,
}

/// The share settings.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ShareSettings {
    /// Where public TCP shares land.
    pub landing: ShareLanding,
    /// Whether Cua's relay may carry public TCP shares at all.
    pub relay_fallback: bool,
    /// The most data public TCP shares move in a day, in MiB (0: no cap).
    pub daily_cap_mib: u64,
    /// The allowlist new shares start with (CIDRs; empty: anyone).
    pub default_allow: Vec<String>,
    /// Ask before an agent's share goes public.
    pub approve_agent_shares: bool,
    /// Spaces whose agents may share without asking.
    pub always_allow_spaces: Vec<String>,
    /// Clouds you agreed may hold a share edge.
    pub edge_consent: Vec<String>,
    /// Bytes public TCP shares moved today (read only).
    pub used_today: u64,
    /// The share edge's cloud and host, when there is one (read only).
    pub edge_cloud: Option<String>,
    /// The share edge's address (read only).
    pub edge_host: Option<String>,
    /// About what the edge costs a month, in US dollars (read only).
    pub edge_monthly_usd: Option<f64>,
}

impl From<cua_daemon_core::ShareSettingsInfo> for ShareSettings {
    fn from(i: cua_daemon_core::ShareSettingsInfo) -> Self {
        use cua_daemon_core::share_route::Landing;
        let s = i.settings;
        ShareSettings {
            landing: match s.landing {
                Landing::Auto => ShareLanding::Auto,
                Landing::OnlyMyCloud => ShareLanding::OnlyMyCloud,
                Landing::AllowRelay => ShareLanding::AllowRelay,
            },
            relay_fallback: s.relay_fallback,
            daily_cap_mib: s.daily_cap_mib,
            default_allow: s.default_allow,
            approve_agent_shares: s.approve_agent_shares,
            always_allow_spaces: s.always_allow_spaces,
            edge_consent: s.edge_consent,
            used_today: i.used_today,
            edge_cloud: i.edge.as_ref().map(|e| e.0.clone()),
            edge_host: i.edge.as_ref().map(|e| e.1.clone()),
            edge_monthly_usd: i.edge.as_ref().map(|e| e.2),
        }
    }
}

impl From<&ShareSettings> for cua_daemon_core::share_route::ShareSettings {
    fn from(s: &ShareSettings) -> Self {
        use cua_daemon_core::share_route::Landing;
        cua_daemon_core::share_route::ShareSettings {
            landing: match s.landing {
                ShareLanding::Auto => Landing::Auto,
                ShareLanding::OnlyMyCloud => Landing::OnlyMyCloud,
                ShareLanding::AllowRelay => Landing::AllowRelay,
            },
            relay_fallback: s.relay_fallback,
            daily_cap_mib: s.daily_cap_mib,
            default_allow: s.default_allow.clone(),
            approve_agent_shares: s.approve_agent_shares,
            always_allow_spaces: s.always_allow_spaces.clone(),
            edge_consent: s.edge_consent.clone(),
        }
    }
}

fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl From<cua_daemon_core::ShareInfo> for Share {
    fn from(s: cua_daemon_core::ShareInfo) -> Self {
        use cua_daemon_core::ShareTarget as T;
        let kind = match &s.target {
            T::Port(_) => "port",
            T::SpacePort { .. } => "space_port",
            T::File(_) => "file",
            T::Dir(_) => "dir",
        };
        Share {
            id: s.id,
            url: s.url,
            name: s.name,
            access: s.access.into(),
            kind: kind.into(),
            target: s.target.describe(),
            owner: s.owner,
            created_at_unix: s.created_at.map(unix),
            expires_at_unix: s.expires_at.map(unix),
            notices: s.notices,
            tcp_connect_with: s.tcp.as_ref().map(|t| t.connect_with().to_owned()),
            tcp_address: s.tcp.as_ref().map(|t| t.address.clone()),
            route: s.tcp.as_ref().map(|t| t.route.clone()),
            route_line: s
                .tcp
                .as_ref()
                .map(|t| t.route_line.clone())
                .or_else(|| s.waiting.as_ref().map(|w| w.route_line.clone())),
            allow: s.tcp.map(|t| t.allow).unwrap_or_default(),
            waiting: s.waiting.as_ref().map(|w| w.reason.clone()),
            waiting_text: s.waiting.map(|w| w.text),
        }
    }
}

/// What opening a link once returned.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ShareCheck {
    /// 2xx.
    pub ok: bool,
    /// HTTP status (0: the link could not be opened).
    pub status: u32,
    /// `link` (the real link, through the relay, as the owner) or `local`.
    pub via: String,
    /// A short human line.
    pub detail: String,
}

impl From<cua_daemon_core::ShareCheck> for ShareCheck {
    fn from(c: cua_daemon_core::ShareCheck) -> Self {
        ShareCheck {
            ok: c.ok,
            status: c.status as u32,
            via: c.via,
            detail: c.detail,
        }
    }
}

async fn daemon(backend: &Backend) -> Result<cua_daemon_core::client::DaemonClient> {
    match backend {
        Backend::Daemon(d) => Ok(d.clone()),
        Backend::Embedded(_) => Ok(cua_daemon_core::client::ensure_daemon().await?),
    }
}

#[uniffi::export]
impl Cua {
    /// Shares `target` and returns a link that opens on the owner's other
    /// signed-in devices, never a localhost link: a port (`"3000"`), a port in
    /// a Space (`"space:dev:3000"`), or a file or folder (`"./report.html"`,
    /// read-only; relative paths resolve against this process's directory).
    /// `access` defaults to private; a public share lasts one hour unless
    /// `ttl_seconds` (60 to 86400) says otherwise. The cua daemon owns the
    /// share, so it keeps working after this process exits.
    #[uniffi::method(default(access = None, ttl_seconds = None, name = None))]
    pub async fn share(
        &self,
        target: String,
        access: Option<ShareAccess>,
        ttl_seconds: Option<u32>,
        name: Option<String>,
    ) -> Result<Share> {
        let backend = self.backend.clone();
        let cwd =
            std::env::current_dir().map_err(|e| crate::CuaError::InvalidArgument(e.to_string()))?;
        let parsed = cua_daemon_core::ShareTarget::parse(&target, &cwd)?;
        let ttl = ttl_seconds.map(super::secs);
        run(async move {
            let d = daemon(&backend).await?;
            let s = d
                .create_share(
                    &parsed,
                    access.unwrap_or(ShareAccess::Private).into(),
                    ttl,
                    name,
                )
                .await?;
            Ok(s.into())
        })
        .await
    }

    /// Shares a raw TCP service in a Space (a game server, for example) at a
    /// public address anyone can connect to, and returns it
    /// (`tcp_connect_with`). `target` is a port in a Space
    /// (`"space:mc:25565"`); inside a Space, a bare port (`"25565"`) is its
    /// own. `service` (`"minecraft"`) adds a name people can type instead of
    /// the address. Lasts 24 hours unless `ttl_seconds` (60 to 86400) says
    /// less. It reaches only that one port of that Space; admin and
    /// remote-access ports are refused. An experiment (Public TCP shares),
    /// off unless turned on.
    ///
    /// `route` is `auto` (default: the Space's own host in your cloud,
    /// else a share edge in your linked cloud, else Cua's relay), `host`,
    /// `edge` or `relay`. `allow` limits who may connect (CIDRs).
    /// `check_host` (Minecraft) drops connections that did not dial this
    /// address. `consent` agrees to a share edge's cost now; without it the
    /// first share through a new edge comes back `waiting` until
    /// `approve_share`.
    #[uniffi::method(default(
        service = None,
        ttl_seconds = None,
        name = None,
        route = None,
        allow = None,
        check_host = false,
        consent = false
    ))]
    #[allow(clippy::too_many_arguments)]
    pub async fn share_tcp(
        &self,
        target: String,
        service: Option<String>,
        ttl_seconds: Option<u32>,
        name: Option<String>,
        route: Option<String>,
        allow: Option<Vec<String>>,
        check_host: bool,
        consent: bool,
    ) -> Result<Share> {
        let backend = self.backend.clone();
        let cwd =
            std::env::current_dir().map_err(|e| crate::CuaError::InvalidArgument(e.to_string()))?;
        let parsed = cua_daemon_core::ShareTarget::parse(&target, &cwd)?;
        if matches!(
            parsed,
            cua_daemon_core::ShareTarget::File(_) | cua_daemon_core::ShareTarget::Dir(_)
        ) {
            return Err(crate::CuaError::InvalidArgument(
                "share_tcp shares a port in a Space (space:NAME:PORT)".into(),
            ));
        }
        let ttl = ttl_seconds.map(super::secs);
        run(async move {
            let d = daemon(&backend).await?;
            let opts = cua_daemon_core::TcpShareOptions {
                service,
                route: route.filter(|r| !r.is_empty() && r != "auto"),
                allow: allow.unwrap_or_default(),
                check_host,
                consent,
            };
            let s = d.create_tcp_share(&parsed, ttl, name, &opts).await?;
            Ok(s.into())
        })
        .await
    }

    /// The active shares, newest first.
    pub async fn shares(&self) -> Result<Vec<Share>> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            Ok(d.list_shares()
                .await?
                .into_iter()
                .map(Share::from)
                .collect())
        })
        .await
    }

    /// Stops a share (its id or its name). The link stops working at once.
    /// Returns the ids stopped.
    pub async fn stop_share(&self, id: String) -> Result<Vec<String>> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            Ok(d.stop_share(Some(&id), false).await?)
        })
        .await
    }

    /// Lets a waiting share go live (an agent's public share, or the first
    /// through a share edge). `always`: its Space may share without asking
    /// from now on.
    #[uniffi::method(default(always = false))]
    pub async fn approve_share(&self, id: String, always: bool) -> Result<Share> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            Ok(d.approve_share(&id, always).await?.into())
        })
        .await
    }

    /// Turns down a waiting share.
    pub async fn deny_share(&self, id: String) -> Result<()> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            Ok(d.deny_share(&id).await?)
        })
        .await
    }

    /// The share settings, with today's data and the share edge.
    pub async fn share_settings(&self) -> Result<ShareSettings> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            Ok(d.share_settings().await?.into())
        })
        .await
    }

    /// Saves the share settings (the read-only fields are ignored).
    pub async fn set_share_settings(&self, settings: ShareSettings) -> Result<ShareSettings> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            d.set_share_settings(&(&settings).into()).await?;
            Ok(d.share_settings().await?.into())
        })
        .await
    }

    /// Removes the share edge from your cloud now. Returns the ids of the
    /// shares that ended with it.
    pub async fn remove_share_edge(&self) -> Result<Vec<String>> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            Ok(d.remove_share_edge().await?)
        })
        .await
    }

    /// Stops every share. Returns the ids stopped.
    pub async fn stop_all_shares(&self) -> Result<Vec<String>> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            Ok(d.stop_share(None, true).await?)
        })
        .await
    }

    /// Opens a share's link once, as its owner, so a link is known to work
    /// before it is handed to anyone. `path` is below the link.
    #[uniffi::method(default(path = None))]
    pub async fn check_share(&self, id: String, path: Option<String>) -> Result<ShareCheck> {
        let backend = self.backend.clone();
        run(async move {
            let d = daemon(&backend).await?;
            Ok(d.check_share(&id, path.as_deref()).await?.into())
        })
        .await
    }
}
