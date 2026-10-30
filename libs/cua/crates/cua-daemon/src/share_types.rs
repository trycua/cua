//! Client-side view of `cua share`: the types the SDK, CLI and MCP use.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use cua_proto::daemon::v1 as pb;

/// What a share points at (paths absolute).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareTarget {
    /// A port on the daemon's machine.
    Port(u16),
    /// A port in a Space.
    SpacePort {
        /// The Space.
        space: String,
        /// The port in it.
        port: u16,
    },
    /// A file.
    File(PathBuf),
    /// A folder.
    Dir(PathBuf),
}

impl ShareTarget {
    /// Parses what `cua share` takes (`3000`, `space:dev:3000`, a path;
    /// relative paths resolve against `cwd`).
    pub fn parse(input: &str, cwd: &std::path::Path) -> crate::Result<Self> {
        use cua_share::Target;
        match Target::parse(input, cwd).map_err(|e| crate::Error::InvalidArgument(e.to_string()))? {
            Target::Port { port } => Ok(Self::Port(port)),
            Target::SpacePort { space, port } => Ok(Self::SpacePort { space, port }),
            Target::File { path } => Ok(Self::File(path)),
            Target::Dir { path } => Ok(Self::Dir(path)),
        }
    }

    /// A short description (`port 3000`, `space:dev:3000`, a path).
    pub fn describe(&self) -> String {
        match self {
            Self::Port(p) => format!("port {p}"),
            Self::SpacePort { space, port } => format!("space:{space}:{port}"),
            Self::File(p) | Self::Dir(p) => p.display().to_string(),
        }
    }

    pub(crate) fn to_pb(&self) -> pb::ShareTarget {
        match self {
            Self::Port(p) => pb::ShareTarget {
                kind: pb::ShareTargetKind::Port as i32,
                port: *p as u32,
                ..Default::default()
            },
            Self::SpacePort { space, port } => pb::ShareTarget {
                kind: pb::ShareTargetKind::SpacePort as i32,
                space: space.clone(),
                port: *port as u32,
                ..Default::default()
            },
            Self::File(p) => pb::ShareTarget {
                kind: pb::ShareTargetKind::File as i32,
                path: p.to_string_lossy().into_owned(),
                ..Default::default()
            },
            Self::Dir(p) => pb::ShareTarget {
                kind: pb::ShareTargetKind::Dir as i32,
                path: p.to_string_lossy().into_owned(),
                ..Default::default()
            },
        }
    }

    pub(crate) fn from_pb(t: &pb::ShareTarget) -> Self {
        match pb::ShareTargetKind::try_from(t.kind).unwrap_or(pb::ShareTargetKind::Unspecified) {
            pb::ShareTargetKind::SpacePort => Self::SpacePort {
                space: t.space.clone(),
                port: t.port as u16,
            },
            pb::ShareTargetKind::File => Self::File(PathBuf::from(&t.path)),
            pb::ShareTargetKind::Dir => Self::Dir(PathBuf::from(&t.path)),
            _ => Self::Port(t.port as u16),
        }
    }
}

/// Who may open a share.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShareAccess {
    /// The owner's account, on any of the owner's devices.
    #[default]
    Private,
    /// Members of the owner's team.
    Team,
    /// Anyone with the link, until it ends.
    Public,
}

impl ShareAccess {
    /// `private`, `team`, `public`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Team => "team",
            Self::Public => "public",
        }
    }

    pub(crate) fn to_pb(self) -> i32 {
        match self {
            Self::Private => pb::ShareAccess::Private as i32,
            Self::Team => pb::ShareAccess::Team as i32,
            Self::Public => pb::ShareAccess::Public as i32,
        }
    }

    pub(crate) fn from_pb(v: i32) -> Self {
        match pb::ShareAccess::try_from(v).unwrap_or(pb::ShareAccess::Unspecified) {
            pb::ShareAccess::Team => Self::Team,
            pb::ShareAccess::Public => Self::Public,
            _ => Self::Private,
        }
    }
}

/// One share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareInfo {
    /// Handle for stop (`share-...`).
    pub id: String,
    /// The link.
    pub url: String,
    /// Name for lists.
    pub name: Option<String>,
    /// What it serves.
    pub target: ShareTarget,
    /// Who may open it.
    pub access: ShareAccess,
    /// Who made it.
    pub owner: String,
    /// When it was made.
    pub created_at: Option<SystemTime>,
    /// When it ends; none: until stopped.
    pub expires_at: Option<SystemTime>,
    /// Limits of this link worth showing next to it.
    pub notices: Vec<String>,
}

impl ShareInfo {
    pub(crate) fn from_pb(s: &pb::Share) -> Self {
        let t = |t: &pbjson_types::Timestamp| crate::convert::from_ts(t);
        Self {
            id: s.id.clone(),
            url: s.url.clone(),
            name: (!s.name.is_empty()).then(|| s.name.clone()),
            target: s
                .target
                .as_ref()
                .map(ShareTarget::from_pb)
                .unwrap_or(ShareTarget::Port(0)),
            access: ShareAccess::from_pb(s.access),
            owner: s.owner.clone(),
            created_at: s.created_at.as_ref().map(t),
            expires_at: s.expires_at.as_ref().map(t),
            notices: s.notices.clone(),
        }
    }

    /// Time left, when it ends.
    pub fn remaining(&self) -> Option<Duration> {
        self.expires_at
            .map(|e| e.duration_since(SystemTime::now()).unwrap_or_default())
    }
}

/// What opening a link once returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareCheck {
    /// HTTP status (0: could not open it).
    pub status: u16,
    /// 2xx.
    pub ok: bool,
    /// `link` (the real link, through the relay) or `local`.
    pub via: String,
    /// A human line.
    pub detail: String,
}
