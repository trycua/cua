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

impl From<ShareAccess> for cua_daemon::ShareAccess {
    fn from(a: ShareAccess) -> Self {
        match a {
            ShareAccess::Private => Self::Private,
            ShareAccess::Team => Self::Team,
            ShareAccess::Public => Self::Public,
        }
    }
}

impl From<cua_daemon::ShareAccess> for ShareAccess {
    fn from(a: cua_daemon::ShareAccess) -> Self {
        match a {
            cua_daemon::ShareAccess::Private => Self::Private,
            cua_daemon::ShareAccess::Team => Self::Team,
            cua_daemon::ShareAccess::Public => Self::Public,
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
}

fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl From<cua_daemon::ShareInfo> for Share {
    fn from(s: cua_daemon::ShareInfo) -> Self {
        use cua_daemon::ShareTarget as T;
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

impl From<cua_daemon::ShareCheck> for ShareCheck {
    fn from(c: cua_daemon::ShareCheck) -> Self {
        ShareCheck {
            ok: c.ok,
            status: c.status as u32,
            via: c.via,
            detail: c.detail,
        }
    }
}

async fn daemon(backend: &Backend) -> Result<cua_daemon::client::DaemonClient> {
    match backend {
        Backend::Daemon(d) => Ok(d.clone()),
        Backend::Embedded(_) => Ok(cua_daemon::client::ensure_daemon().await?),
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
        let parsed = cua_daemon::ShareTarget::parse(&target, &cwd)?;
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
