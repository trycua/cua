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

    #[cfg(any(feature = "client", test))]
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

    #[cfg(any(feature = "client", test))]
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

    #[cfg(any(feature = "client", test))]
    pub(crate) fn to_pb(self) -> i32 {
        match self {
            Self::Private => pb::ShareAccess::Private as i32,
            Self::Team => pb::ShareAccess::Team as i32,
            Self::Public => pb::ShareAccess::Public as i32,
        }
    }

    #[cfg(any(feature = "client", test))]
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
    /// A public TCP share: where people connect. `None`: an HTTP link.
    pub tcp: Option<ShareTcp>,
    /// The Space whose agent made it (through its spawn channel).
    pub origin: Option<String>,
    /// Set while the share waits for the owner (approval, or consent to a
    /// share edge); it serves nothing until approved.
    pub waiting: Option<ShareWaiting>,
}

/// Why a share waits for its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareWaiting {
    /// `approval` or `consent`.
    pub reason: String,
    /// What to show the owner.
    pub text: String,
    /// The route it would take, in plain words.
    pub route_line: String,
}

/// The choices of a public TCP share beyond its target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TcpShareOptions {
    /// SRV service (`minecraft`).
    pub service: Option<String>,
    /// `auto` (`None`), `host`, `edge` or `relay`.
    pub route: Option<String>,
    /// Who may connect (CIDRs or addresses); empty: the settings' default.
    pub allow: Vec<String>,
    /// Drop Minecraft Java connections that did not dial the address.
    pub check_host: bool,
    /// The owner agrees now to a share edge the route needs.
    pub consent: bool,
}

/// The share settings, with today's data and the share edge.
#[derive(Clone, Debug, PartialEq)]
pub struct ShareSettingsInfo {
    /// The settings.
    pub settings: cua_share::route::ShareSettings,
    /// Bytes public TCP shares moved today.
    pub used_today: u64,
    /// The share edge: cloud, host and monthly cost; `None` without one.
    pub edge: Option<(String, String, f64)>,
}

/// Where people reach a public TCP share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareTcp {
    /// `host:port` to connect to.
    pub address: String,
    /// The public port.
    pub port: u16,
    /// `host` (the Space's own cloud host), `edge` (a share edge in the
    /// owner's cloud) or `relay` (Cua).
    pub route: String,
    /// The plain line next to the address ("Through your AWS account").
    pub route_line: String,
    /// The cloud a `host` or `edge` route uses.
    pub cloud: Option<String>,
    /// Who may connect (CIDRs); empty: anyone.
    pub allow: Vec<String>,
    /// A name people can type instead (SRV record), when there is one.
    pub srv_name: Option<String>,
    /// SRV service (`minecraft`), when there is one.
    pub service: Option<String>,
}

impl ShareTcp {
    /// What people connect to: the typeable name when there is one, else
    /// the address.
    pub fn connect_with(&self) -> &str {
        self.srv_name.as_deref().unwrap_or(&self.address)
    }
}

impl ShareInfo {
    #[cfg(any(feature = "client", test))]
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
            tcp: s.tcp.as_ref().map(|t| ShareTcp {
                address: t.address.clone(),
                port: t.port as u16,
                route: if t.route.is_empty() {
                    "relay".into()
                } else {
                    t.route.clone()
                },
                route_line: t.route_line.clone(),
                cloud: (!t.cloud.is_empty()).then(|| t.cloud.clone()),
                allow: t.allow.clone(),
                srv_name: (!t.srv_name.is_empty()).then(|| t.srv_name.clone()),
                service: (!t.service.is_empty()).then(|| t.service.clone()),
            }),
            origin: (!s.origin.is_empty()).then(|| s.origin.clone()),
            waiting: s.waiting.as_ref().map(|w| ShareWaiting {
                reason: w.reason.clone(),
                text: w.text.clone(),
                route_line: w.route_line.clone(),
            }),
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

/// The share settings off the wire.
pub fn settings_from_pb(p: &pb::ShareSettings) -> cua_share::route::ShareSettings {
    use cua_share::route::Landing;
    cua_share::route::ShareSettings {
        landing: match pb::ShareLanding::try_from(p.landing).unwrap_or_default() {
            pb::ShareLanding::OnlyMyCloud => Landing::OnlyMyCloud,
            pb::ShareLanding::AllowRelay => Landing::AllowRelay,
            pb::ShareLanding::Unspecified => Landing::Auto,
        },
        relay_fallback: p.relay_fallback,
        daily_cap_mib: p.daily_cap_mib,
        default_allow: p.default_allow.clone(),
        approve_agent_shares: p.approve_agent_shares,
        always_allow_spaces: p.always_allow_spaces.clone(),
        edge_consent: p.edge_consent.clone(),
    }
}

/// The share settings onto the wire.
pub fn settings_to_pb(s: &cua_share::route::ShareSettings) -> pb::ShareSettings {
    use cua_share::route::Landing;
    pb::ShareSettings {
        landing: match s.landing {
            Landing::Auto => pb::ShareLanding::Unspecified,
            Landing::OnlyMyCloud => pb::ShareLanding::OnlyMyCloud,
            Landing::AllowRelay => pb::ShareLanding::AllowRelay,
        } as i32,
        relay_fallback: s.relay_fallback,
        daily_cap_mib: s.daily_cap_mib,
        default_allow: s.default_allow.clone(),
        approve_agent_shares: s.approve_agent_shares,
        always_allow_spaces: s.always_allow_spaces.clone(),
        edge_consent: s.edge_consent.clone(),
    }
}
