//! Which way a Space's cua-spacesd connection goes: on this machine, by
//! address, through the relay, over the mesh (directly or relayed), or
//! through another process's connection. For people (`cua spaces`, logs)
//! and for measurements that must know which path they timed.

use std::sync::Arc;

use cua_mesh::dialer::Dialer;
use cua_mesh::{Path, PublicKey};

/// The way a connected Space's spacesd calls go.
#[derive(Clone)]
pub enum ChannelRoute {
    /// A Space on this machine, over loopback (`host:port`).
    Local(String),
    /// Reached by its address: a LAN, Tailscale or VPN address, or a URL.
    Direct(String),
    /// The relay's HTTPS path (relay base URL).
    Relay(String),
    /// The WireGuard®-based mesh to `peer`: direct when a direct path
    /// answers, relayed ciphertext through the relay otherwise. Evaluated
    /// live, so it shows the path the next call takes.
    Mesh {
        /// The node the connection goes through.
        dialer: Arc<Dialer>,
        /// The machine's node key.
        peer: PublicKey,
    },
    /// The Fleet gateway.
    Cloud,
    /// Another process's connection (the `cua daemon`'s env passthrough),
    /// with the route that process reported.
    Passthrough(String),
}

impl std::fmt::Debug for ChannelRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

/// What a Space's connection is carried by, for badges and status: the
/// one thing a person needs to know is whether the relay can read it
/// ([`Transport::RelayHttps`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// A Space on this machine.
    Local,
    /// Reached by its own address (LAN, Tailscale, VPN, a URL).
    Direct,
    /// The WireGuard®-based mesh: end to end, direct or relayed as
    /// ciphertext.
    Mesh,
    /// The relay's HTTPS path: TLS ends at the relay, which can read the
    /// session in transit.
    RelayHttps,
    /// The Fleet gateway.
    Cloud,
    /// Not known (another process's connection that did not say).
    Unknown,
}

impl Transport {
    /// `local`, `direct`, `mesh`, `relay_https`, `cloud` or `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Direct => "direct",
            Self::Mesh => "mesh",
            Self::RelayHttps => "relay_https",
            Self::Cloud => "cloud",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the relay can read what crosses it.
    pub fn relay_can_read(self) -> bool {
        self == Self::RelayHttps
    }

    /// The transport of a route line as [`ChannelRoute::describe`] writes
    /// it (another process's connection, for example the `cua daemon`'s).
    pub fn of_described(route: &str) -> Self {
        let first = route.split_whitespace().next().unwrap_or_default();
        match first {
            "local" => Self::Local,
            "direct" => Self::Direct,
            "relay" => Self::RelayHttps,
            "mesh" => Self::Mesh,
            "cloud" => Self::Cloud,
            _ => Self::Unknown,
        }
    }
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ChannelRoute {
    /// What carries this connection ([`Transport`]).
    pub fn transport(&self) -> Transport {
        match self {
            Self::Local(_) => Transport::Local,
            Self::Direct(_) => Transport::Direct,
            Self::Relay(_) => Transport::RelayHttps,
            Self::Mesh { .. } => Transport::Mesh,
            Self::Cloud => Transport::Cloud,
            Self::Passthrough(r) => Transport::of_described(r),
        }
    }

    /// One line: `local 127.0.0.1:52011`, `direct 192.168.1.20:3211`,
    /// `relay https://relay.cua.ai`, `mesh direct 192.168.1.20:41641 rtt
    /// 0.41 ms`, `mesh relayed`, `cloud`.
    pub fn describe(&self) -> String {
        match self {
            Self::Local(a) => format!("local {a}"),
            Self::Direct(a) => format!("direct {a}"),
            Self::Relay(u) => format!("relay {u}"),
            Self::Mesh { dialer, peer } => {
                let mesh = dialer.mesh();
                describe_mesh(mesh.path(peer), mesh.rtt(peer))
            }
            Self::Cloud => "cloud".into(),
            Self::Passthrough(r) => r.clone(),
        }
    }

    /// Whether calls go straight to the machine (no relay in between):
    /// local, by address, or a direct mesh path.
    pub fn is_direct(&self) -> bool {
        match self {
            Self::Local(_) | Self::Direct(_) => true,
            Self::Mesh { dialer, peer } => {
                matches!(dialer.mesh().path(peer), Some(Path::Direct(_)))
            }
            Self::Passthrough(r) => {
                r.starts_with("local ") || r.starts_with("direct ") || r.starts_with("mesh direct ")
            }
            Self::Relay(_) | Self::Cloud => false,
        }
    }
}

fn describe_mesh(path: Option<Path>, rtt: Option<std::time::Duration>) -> String {
    match (path, rtt) {
        (Some(Path::Direct(a)), Some(rtt)) => {
            format!("mesh direct {a} rtt {:.2} ms", rtt.as_secs_f64() * 1e3)
        }
        (Some(Path::Direct(a)), None) => format!("mesh direct {a}"),
        (Some(Path::Relay), _) => "mesh relayed".into(),
        (None, _) => "mesh (the machine left the map)".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn describes_each_route() {
        assert_eq!(
            ChannelRoute::Local("127.0.0.1:5".into()).describe(),
            "local 127.0.0.1:5"
        );
        assert_eq!(
            ChannelRoute::Relay("https://relay.cua.ai".into()).describe(),
            "relay https://relay.cua.ai"
        );
        let a: std::net::SocketAddr = "192.168.1.20:41641".parse().unwrap();
        assert_eq!(
            describe_mesh(Some(Path::Direct(a)), Some(Duration::from_micros(410))),
            "mesh direct 192.168.1.20:41641 rtt 0.41 ms"
        );
        assert_eq!(describe_mesh(Some(Path::Relay), None), "mesh relayed");
        assert!(ChannelRoute::Passthrough("mesh direct 1.2.3.4:5".into()).is_direct());
        assert!(!ChannelRoute::Passthrough("mesh relayed".into()).is_direct());
        assert!(!ChannelRoute::Relay("https://r".into()).is_direct());
        assert!(ChannelRoute::Local("127.0.0.1:1".into()).is_direct());
    }

    #[test]
    fn names_the_transport() {
        assert_eq!(
            ChannelRoute::Relay("https://r".into()).transport(),
            Transport::RelayHttps
        );
        assert!(Transport::RelayHttps.relay_can_read());
        assert_eq!(
            ChannelRoute::Passthrough("relay https://relay.cua.ai".into()).transport(),
            Transport::RelayHttps
        );
        assert_eq!(
            ChannelRoute::Passthrough("mesh relayed".into()).transport(),
            Transport::Mesh
        );
        assert_eq!(
            ChannelRoute::Passthrough("mesh direct 1.2.3.4:5 rtt 0.40 ms".into()).transport(),
            Transport::Mesh
        );
        assert!(!Transport::Mesh.relay_can_read());
        assert_eq!(Transport::of_described(""), Transport::Unknown);
        assert_eq!(Transport::RelayHttps.as_str(), "relay_https");
    }
}
