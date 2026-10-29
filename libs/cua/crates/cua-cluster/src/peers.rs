//! How a gang member reaches its peers.
//!
//! The coordinator places each member on a host and names every member's
//! `ip:port` by rank (`CUA_PEERS`, the hosts' own addresses). A member runs
//! in a Space, so before it starts its host routes those peers
//! ([`PeerRouter`]):
//!
//! - **mesh** (the default): every peer on another machine becomes a
//!   forward over the Cua mesh, listening where the member's Space reaches
//!   this host (a VM's NAT gateway, a container bridge's gateway, loopback
//!   for the machine's own Space), and the member's own entry is its Space's
//!   address, which this host's mesh node splices peers' streams to. Fixed
//!   ports only (MLX's ring, a torchrun rendezvous): what a protocol opens
//!   later on ports of its own choosing does not cross.
//! - **direct** (`CUA_PEER_ROUTE=direct` in the job's environment): the
//!   coordinator's placed addresses as they are. For frameworks that open
//!   dynamic ports between ranks (PyTorch NCCL and Gloo), on dedicated
//!   machines in one network that let each other in on every TCP port (the
//!   `cua cloud add aws` pool security group). Only a member in the machine's own
//!   Space (`kind = "machine"`) can use it: a member in a VM or container
//!   does not hold its host's address.

use async_trait::async_trait;

use crate::model::JobSpec;

/// The job environment variable that picks how ranks reach each other:
/// `mesh` (default) or `direct`.
pub const PEER_ROUTE_ENV: &str = "CUA_PEER_ROUTE";

/// How a gang's ranks reach each other ([`PEER_ROUTE_ENV`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerRoute {
    /// Over the Cua mesh (fixed ports).
    Mesh,
    /// The placed addresses, directly (every port).
    Direct,
}

impl PeerRoute {
    /// The route `job` asks for. Anything but `direct` is the mesh; an
    /// unknown word is an error the member fails with.
    pub fn of(job: &JobSpec) -> Result<Self, String> {
        match job
            .env
            .get(PEER_ROUTE_ENV)
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("" | "mesh" | "auto") => Ok(Self::Mesh),
            Some("direct") => Ok(Self::Direct),
            Some(other) => Err(format!(
                "{PEER_ROUTE_ENV}={other:?}: say \"mesh\" (the default) or \"direct\""
            )),
        }
    }
}

/// A gang member's peers, as its coordinator placed them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GangPeers {
    /// The run (`CUA_RUN_ID`).
    pub run: String,
    /// The gang attempt (`CUA_ATTEMPT`).
    pub attempt: u32,
    /// This member's rank (`CUA_RANK`).
    pub rank: u32,
    /// `ip:port` of every member by rank (`CUA_PEERS`).
    pub peers: Vec<String>,
    /// The machine of every member by rank (`CUA_PEER_MACHINES`; empty
    /// when the coordinator did not say).
    pub machines: Vec<String>,
    /// The pool name of every member by rank (`CUA_PEER_NAMES`; empty
    /// when the coordinator did not say).
    pub names: Vec<String>,
}

impl GangPeers {
    /// From a job's environment, when it is a gang member with peers.
    pub fn of(job: &JobSpec) -> Option<Self> {
        let env = |k: &str| job.env.get(k).cloned().unwrap_or_default();
        let peers: Vec<String> = env("CUA_PEERS")
            .split(',')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        if peers.is_empty() {
            return None;
        }
        Some(Self {
            run: env("CUA_RUN_ID"),
            attempt: env("CUA_ATTEMPT").parse().unwrap_or(0),
            rank: env("CUA_RANK").parse().unwrap_or(0),
            machines: env("CUA_PEER_MACHINES")
                .split(',')
                .filter(|m| !m.is_empty())
                .map(str::to_string)
                .collect(),
            // Kept positional (an empty name stays empty).
            names: Some(env("CUA_PEER_NAMES"))
                .filter(|n| !n.is_empty())
                .map(|n| n.split(',').map(str::to_string).collect())
                .unwrap_or_default(),
            peers,
        })
    }
}

/// Where a member's Space sits on its host's network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberNet {
    /// The member's own address: it listens there, and this host reaches
    /// it there (a VM's or container's private address; loopback for the
    /// machine's own Space).
    pub listen: std::net::IpAddr,
    /// This host's address as the Space reaches it (a VM's NAT gateway, a
    /// container bridge's gateway; loopback for the machine's own Space):
    /// forwards to peers listen there.
    pub gateway: std::net::IpAddr,
}

impl MemberNet {
    /// The machine's own Space: everything on loopback.
    pub fn machine() -> Self {
        Self {
            listen: std::net::Ipv4Addr::LOCALHOST.into(),
            gateway: std::net::Ipv4Addr::LOCALHOST.into(),
        }
    }

    /// Whether the member runs in the machine's own Space.
    pub fn is_machine(&self) -> bool {
        self.listen.is_loopback()
    }
}

/// How a host routes a gang member's rank traffic: before the member
/// starts it may replace each peer's `ip:port` with one the member reaches
/// it at. The member then sees the routed list as `CUA_PEERS` (the
/// coordinator's stays in `CUA_PEERS_PLACED`). An error fails the member
/// before it starts.
#[async_trait]
pub trait PeerRouter: Send + Sync + 'static {
    /// The `ip:port` to use for each peer by rank, or `None` to keep the
    /// coordinator's.
    async fn route(&self, gang: &GangPeers, net: MemberNet) -> Result<Option<RoutedPeers>, String>;
}

/// A gang member's routed peers ([`PeerRouter::route`]).
pub struct RoutedPeers {
    /// `ip:port` by rank (`CUA_PEERS`).
    pub peers: Vec<String>,
    /// Lines for the member's output, before its own (how each peer is
    /// reached).
    pub notes: Vec<String>,
    /// Held while the member runs, dropped when it ends (its forwards, its
    /// port's registration).
    pub hold: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl RoutedPeers {
    /// Just these peers.
    pub fn new(peers: Vec<String>) -> Self {
        Self {
            peers,
            notes: Vec::new(),
            hold: None,
        }
    }
}

impl std::fmt::Debug for RoutedPeers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutedPeers")
            .field("peers", &self.peers)
            .field("notes", &self.notes)
            .finish()
    }
}

/// A member's peers as it sees them, or why it cannot start: the route its
/// job asks for, through `router` (when this host has one), for a member at
/// `net`. `Ok(None)`: not a gang member with peers, or the placed addresses
/// stand.
pub async fn route_member(
    router: Option<&dyn PeerRouter>,
    job: &JobSpec,
    net: MemberNet,
) -> Result<Option<RoutedPeers>, String> {
    let Some(gang) = GangPeers::of(job) else {
        return Ok(None);
    };
    match PeerRoute::of(job)? {
        PeerRoute::Direct if net.is_machine() => {
            let rank = gang.rank as usize;
            let notes = gang
                .peers
                .iter()
                .enumerate()
                .filter(|(r, _)| *r != rank)
                .map(|(r, p)| format!("peer rank {r} direct at {p}"))
                .collect();
            Ok(Some(RoutedPeers {
                peers: gang.peers.clone(),
                notes,
                hold: None,
            }))
        }
        PeerRoute::Direct => Err(format!(
            "{PEER_ROUTE_ENV}=direct needs the computer's own Space (kind \"machine\", on a \
             dedicated host): a member in a VM or container does not hold its host's address"
        )),
        // A gang of one has no peer to reach: it listens at its own
        // address, which in a VM or container is not its host's.
        PeerRoute::Mesh if gang.peers.len() == 1 => {
            if net.is_machine() {
                return Ok(None);
            }
            let port = gang.peers[0]
                .rsplit_once(':')
                .map(|(_, p)| p)
                .unwrap_or_default();
            Ok(Some(RoutedPeers::new(vec![format!(
                "{}:{port}",
                net.listen
            )])))
        }
        PeerRoute::Mesh => {
            let routed = match router {
                Some(r) => r.route(&gang, net).await?,
                None => None,
            };
            match routed {
                Some(r) if r.peers.len() != gang.peers.len() => Err(format!(
                    "the host routed {} peers for a gang of {}",
                    r.peers.len(),
                    gang.peers.len()
                )),
                Some(r) => Ok(Some(r)),
                None if net.is_machine() => Ok(None),
                // A Space cannot take its host's placed address.
                None => Err(
                    "a gang member in a VM or container reaches its peers over the Cua mesh, \
                     and this host has no route for them"
                        .into(),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(rank: u32, route: Option<&str>) -> JobSpec {
        let mut j = JobSpec {
            job_id: "j".into(),
            attempt: 1,
            argv: vec!["true".into()],
            ..Default::default()
        };
        j.env.insert("CUA_RUN_ID".into(), "run-1".into());
        j.env.insert("CUA_ATTEMPT".into(), "1".into());
        j.env.insert("CUA_RANK".into(), rank.to_string());
        j.env
            .insert("CUA_PEERS".into(), "10.0.0.1:47010,10.0.0.2:47010".into());
        if let Some(r) = route {
            j.env.insert(PEER_ROUTE_ENV.into(), r.into());
        }
        j
    }

    struct Fixed;

    #[async_trait]
    impl PeerRouter for Fixed {
        async fn route(
            &self,
            gang: &GangPeers,
            net: MemberNet,
        ) -> Result<Option<RoutedPeers>, String> {
            Ok(Some(RoutedPeers::new(
                (0..gang.peers.len())
                    .map(|r| {
                        if r == gang.rank as usize {
                            format!("{}:47010", net.listen)
                        } else {
                            format!("{}:5000{r}", net.gateway)
                        }
                    })
                    .collect(),
            )))
        }
    }

    #[test]
    fn the_route_is_the_mesh_unless_the_job_says_direct() {
        assert_eq!(PeerRoute::of(&member(0, None)).unwrap(), PeerRoute::Mesh);
        assert_eq!(
            PeerRoute::of(&member(0, Some("Direct"))).unwrap(),
            PeerRoute::Direct
        );
        assert!(
            PeerRoute::of(&member(0, Some("nccl")))
                .unwrap_err()
                .contains("\"direct\"")
        );
    }

    #[tokio::test]
    async fn a_member_in_a_space_gets_its_own_address_and_forwards_at_its_gateway() {
        let vm = MemberNet {
            listen: "192.168.64.7".parse().unwrap(),
            gateway: "192.168.64.1".parse().unwrap(),
        };
        let r = route_member(Some(&Fixed), &member(1, None), vm)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.peers, ["192.168.64.1:50000", "192.168.64.7:47010"]);
        // No router on this host: a Space member cannot start, the
        // machine's own Space keeps the placed addresses.
        let e = route_member(None, &member(1, None), vm).await.unwrap_err();
        assert!(e.contains("over the Cua mesh"), "{e}");
        assert!(
            route_member(None, &member(1, None), MemberNet::machine())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_gang_of_one_in_a_space_listens_at_its_own_address_without_a_router() {
        let mut j = member(0, None);
        j.env.insert("CUA_PEERS".into(), "10.0.0.1:47010".into());
        let vm = MemberNet {
            listen: "192.168.64.7".parse().unwrap(),
            gateway: "192.168.64.1".parse().unwrap(),
        };
        let r = route_member(None, &j, vm).await.unwrap().unwrap();
        assert_eq!(r.peers, ["192.168.64.7:47010"]);
        assert!(
            route_member(None, &j, MemberNet::machine())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn direct_keeps_the_placed_addresses_only_in_the_machines_own_space() {
        let r = route_member(
            Some(&Fixed),
            &member(0, Some("direct")),
            MemberNet::machine(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(r.peers, ["10.0.0.1:47010", "10.0.0.2:47010"]);
        assert_eq!(r.notes, ["peer rank 1 direct at 10.0.0.2:47010"]);
        let vm = MemberNet {
            listen: "172.17.0.3".parse().unwrap(),
            gateway: "172.17.0.1".parse().unwrap(),
        };
        let e = route_member(Some(&Fixed), &member(0, Some("direct")), vm)
            .await
            .unwrap_err();
        assert!(e.contains("kind \"machine\""), "{e}");
    }
}
