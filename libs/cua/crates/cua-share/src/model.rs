//! What a share is: its target, who may open it, and when it ends.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Shortest lifetime of a share that has one.
pub const MIN_TTL: Duration = Duration::from_secs(60);
/// Longest lifetime of a share that has one.
pub const MAX_TTL: Duration = Duration::from_secs(86_400);
/// Lifetime of a public share when none is given.
pub const DEFAULT_PUBLIC_TTL: Duration = Duration::from_secs(3_600);

/// Hex characters in the id of a private or team share (64 bits).
pub const ID_LEN: usize = 16;
/// Hex characters in a public token (160 bits).
pub const TOKEN_LEN: usize = 40;

/// What a share points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// A TCP port on this machine's loopback.
    Port {
        /// The port.
        port: u16,
    },
    /// A TCP port inside a Space.
    SpacePort {
        /// The Space, as `cua sb` commands name it (`dev`, `local:dev`).
        space: String,
        /// The port inside the Space.
        port: u16,
    },
    /// One file, served read-only.
    File {
        /// Canonical absolute path.
        path: PathBuf,
    },
    /// A folder, served read-only with a listing.
    Dir {
        /// Canonical absolute path.
        path: PathBuf,
    },
}

impl Target {
    /// Parses what `cua share` takes: `3000`, `localhost:3000`,
    /// `http://127.0.0.1:3000`, `space:<ref>:3000` or a path (relative ones
    /// resolve against `cwd`). A path must exist.
    pub fn parse(input: &str, cwd: &Path) -> Result<Target> {
        let s = input.trim();
        if s.is_empty() {
            return Err(Error::invalid(
                "share what? give a port, space:<name>:<port> or a path",
            ));
        }
        if let Some(rest) = s.strip_prefix("space:") {
            let (space, port) = rest.rsplit_once(':').ok_or_else(|| {
                Error::invalid(format!(
                    "{input:?}: use space:<name>:<port>, for example space:dev:3000"
                ))
            })?;
            if space.is_empty() {
                return Err(Error::invalid(format!(
                    "{input:?}: the Space name is empty"
                )));
            }
            return Ok(Target::SpacePort {
                space: space.to_string(),
                port: parse_port(port, input)?,
            });
        }
        if let Some(port) = loopback_port(s) {
            return Ok(Target::Port {
                port: parse_port(port, input)?,
            });
        }
        let path = if Path::new(s).is_absolute() {
            PathBuf::from(s)
        } else {
            cwd.join(s)
        };
        let canonical = std::fs::canonicalize(&path).map_err(|e| {
            Error::invalid(format!(
                "{input:?} is not a port, a space:<name>:<port> or an existing path ({e})"
            ))
        })?;
        let meta = std::fs::metadata(&canonical)?;
        if meta.is_dir() {
            Ok(Target::Dir { path: canonical })
        } else if meta.is_file() {
            Ok(Target::File { path: canonical })
        } else {
            Err(Error::invalid(format!(
                "{input:?} is neither a regular file nor a folder"
            )))
        }
    }

    /// A short description for lists (`port 3000`, `dev:3000`, a path).
    pub fn describe(&self) -> String {
        match self {
            Target::Port { port } => format!("port {port}"),
            Target::SpacePort { space, port } => format!("space:{space}:{port}"),
            Target::File { path } | Target::Dir { path } => path.display().to_string(),
        }
    }
}

fn parse_port(p: &str, input: &str) -> Result<u16> {
    match p.trim().parse::<u16>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(Error::invalid(format!(
            "{input:?}: {p:?} is not a port (1-65535)"
        ))),
    }
}

/// The port of `3000`, `:3000`, `localhost:3000`, `127.0.0.1:3000` or an
/// `http(s)://` URL of those; `None` for anything else (a path).
fn loopback_port(s: &str) -> Option<&str> {
    let s = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
        .unwrap_or(s);
    let s = s.split(['/', '?', '#']).next().unwrap_or(s);
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return Some(s);
    }
    for host in ["localhost", "127.0.0.1", "[::1]", "0.0.0.0", ""] {
        if let Some(p) = s.strip_prefix(host).and_then(|r| r.strip_prefix(':'))
            && !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_digit())
        {
            return Some(p);
        }
    }
    None
}

/// Who may open a share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Signed in to the owner's cua.ai account.
    Private,
    /// Signed in to a team of the owner's.
    Team,
    /// Anyone with the link, until it expires or is stopped.
    Public,
}

impl Access {
    /// `private`, `team`, `public`.
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Private => "private",
            Access::Team => "team",
            Access::Public => "public",
        }
    }
}

/// The account that made a share.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    /// Account id (the relay's `acct`).
    pub account: String,
    /// Email, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// How long a public TCP share lasts unless asked otherwise: the most a
/// share may last ([`MAX_TTL`]); the daemon's keep-alive holds its port on
/// the relay meanwhile.
pub const TCP_TTL: Duration = MAX_TTL;

/// Space ports a public TCP share never exposes: remote shells and
/// desktops, game-server consoles (Minecraft RCON), container and cluster
/// control planes, and databases. The relay and cua-spacesd keep their own
/// copies of this rule.
pub const REFUSED_TCP_PORTS: &[u16] = &[
    22, 23, 2375, 2376, 3306, 3389, 5432, 5900, 5901, 6379, 6443, 10250, 25575, 27017,
];

/// Where people reach a public TCP share.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TcpEndpoint {
    /// `host:port` to connect to.
    pub address: String,
    /// The host part.
    pub host: String,
    /// The public port.
    pub port: u16,
    /// A name that finds the port through DNS SRV (`mc-….m.cua.sh`), when
    /// the service has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srv_name: Option<String>,
    /// The SRV service (`minecraft`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// How people reach it: `host` (the Space's own cloud host), `edge`
    /// (a share edge in the owner's cloud) or `relay` (Cua's relay).
    #[serde(default = "relay_route")]
    pub route: String,
    /// The cloud a `host` or `edge` route uses (`aws`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
    /// The plain line shown next to the address ("Through your AWS
    /// account", "Through Cua").
    #[serde(default)]
    pub route_line: String,
    /// Who may connect (CIDRs); empty: anyone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Drop Minecraft Java connections that did not dial this address.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub check_host: bool,
    /// The data the share may move (its part of the daily cap); 0: none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_bytes: u64,
    /// The share slot (host route) or the edge's port (edge route).
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub slot: u16,
}

fn relay_route() -> String {
    "relay".into()
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}

/// Why a share is not live yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    /// `approval` (an agent asked for a public share) or `consent` (the
    /// first share edge in a cloud).
    pub reason: String,
    /// What to show the owner (the consent card's text, or who asked).
    pub text: String,
    /// The route it would take, in plain words.
    #[serde(default)]
    pub route_line: String,
}

/// One share, as the daemon keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    /// `share-` plus 16 hex: the handle for `cua share stop`.
    pub handle: String,
    /// The secret path/host part: 16 hex for private and team shares, 40 hex
    /// (the public token) for public ones.
    pub key: String,
    /// A name for lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What it serves.
    pub target: Target,
    /// Who may open it.
    pub access: Access,
    /// Who made it.
    pub owner: Owner,
    /// Team ids a `team` share admits (the owner's teams when it was made).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<String>,
    /// Unix milliseconds.
    pub created_ms: u64,
    /// Unix milliseconds the share ends at; none for a private or team share
    /// made without `--ttl`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
    /// The link (`tcp://<address>` for a TCP share).
    pub url: String,
    /// A public TCP share: where people connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp: Option<TcpEndpoint>,
    /// The Space whose agent made it (through its spawn channel); the share
    /// ends with that Space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Set while the share waits for the owner (approval or consent): it
    /// serves nothing until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<Waiting>,
}

impl Share {
    /// Whether the share has ended at `now_ms`.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.expires_ms.is_some_and(|e| e <= now_ms)
    }

    /// Whether the key is a public token.
    pub fn is_public(&self) -> bool {
        self.access == Access::Public
    }
}

/// Checks a lifetime against [`MIN_TTL`] and [`MAX_TTL`].
pub fn check_ttl(ttl: Duration) -> Result<Duration> {
    if ttl < MIN_TTL || ttl > MAX_TTL {
        return Err(Error::invalid(format!(
            "--ttl must be between {}s and {}s",
            MIN_TTL.as_secs(),
            MAX_TTL.as_secs()
        )));
    }
    Ok(ttl)
}

/// `n` random bytes as lowercase hex.
pub fn random_hex(n: usize) -> String {
    use rand::RngCore as _;
    let mut b = vec![0u8; n];
    rand::rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Unix milliseconds now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ports_spaces_and_paths() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::write(cwd.join("report.html"), "x").unwrap();
        std::fs::create_dir(cwd.join("build")).unwrap();
        for s in ["3000", ":3000", "localhost:3000", "http://127.0.0.1:3000/x"] {
            assert_eq!(
                Target::parse(s, cwd).unwrap(),
                Target::Port { port: 3000 },
                "{s}"
            );
        }
        assert_eq!(
            Target::parse("space:local:dev:8080", cwd).unwrap(),
            Target::SpacePort {
                space: "local:dev".into(),
                port: 8080
            }
        );
        assert!(matches!(
            Target::parse("./report.html", cwd).unwrap(),
            Target::File { .. }
        ));
        assert!(matches!(
            Target::parse("build/", cwd).unwrap(),
            Target::Dir { .. }
        ));
        for bad in [
            "",
            "0",
            "70000",
            "space:dev",
            "space::3000",
            "./missing",
            "space:dev:x",
        ] {
            assert!(Target::parse(bad, cwd).is_err(), "{bad:?} must fail");
        }
    }

    #[test]
    fn ttl_bounds() {
        assert!(check_ttl(Duration::from_secs(59)).is_err());
        assert!(check_ttl(Duration::from_secs(60)).is_ok());
        assert!(check_ttl(MAX_TTL).is_ok());
        assert!(check_ttl(MAX_TTL + Duration::from_secs(1)).is_err());
    }
}
