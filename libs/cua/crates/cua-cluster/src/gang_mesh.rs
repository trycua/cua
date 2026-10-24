//! A gang's rank traffic over the Cua mesh (CUA-1783): the host's
//! [`crate::peers::PeerRouter`].
//!
//! Before a gang member starts on this host, every peer on another machine
//! is given a forward over the mesh to that machine's gang port (asked of
//! this machine's mesh node on its control socket, [`cua_mesh::gang`]),
//! listening where the member's Space reaches this host (its gateway;
//! loopback for the machine's own Space). The member's own entry becomes
//! its Space's address (`127.0.0.1` in the machine's own Space), registered
//! in the port registry while it runs so its peers' mesh nodes can reach
//! it. The mesh picks the path: direct on a LAN or across NATs by hole
//! punching, else its encrypted relay. Every cross-machine peer goes this
//! way, the same LAN included.
//!
//! When the mesh is off or unavailable here (no mesh node, no control
//! socket) or cannot reach one of the peers, a member in the machine's own
//! Space keeps the coordinator's placed addresses for every peer; a member
//! in a VM or container cannot (it does not hold its host's address) and
//! fails before it starts. Each member's output says which path each peer
//! takes (`peer rank R via mesh (direct)`, `... via mesh (relayed)`, `...
//! via placed address (direct, no mesh) to NAME port P`), naming the peer
//! by its pool name, never by its IP.

use std::path::PathBuf;

use crate::peers::{GangPeers, MemberNet, PeerRouter, RoutedPeers};
use async_trait::async_trait;

/// Routes gang peers through the mesh node of the machine whose Cua home
/// is `home`.
#[derive(Debug, Clone)]
pub struct MeshPeerRouter {
    #[cfg_attr(not(unix), allow(dead_code))]
    home: PathBuf,
}

impl MeshPeerRouter {
    /// For the Cua home `home` (the host daemon's).
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn port_of(peer: &str) -> Result<u16, String> {
    peer.rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .filter(|p| *p != 0)
        .ok_or_else(|| format!("peer address {peer:?} has no port"))
}

/// What a member keeps while it runs.
#[cfg(unix)]
struct Held {
    _control: cua_mesh::gang::ControlClient,
    _port: cua_mesh::gang::Registration,
}

/// The placed addresses, for a member in the machine's own Space; a
/// member in a VM or container cannot use them.
fn placed(gang: &GangPeers, net: MemberNet, why: &str) -> Result<Option<RoutedPeers>, String> {
    if !net.is_machine() {
        return Err(format!(
            "a gang member in a VM or container reaches its peers over the Cua mesh: {why}"
        ));
    }
    let mut notes = vec![format!("rank traffic uses the placed addresses: {why}")];
    for (r, p) in gang.peers.iter().enumerate() {
        if r != gang.rank as usize {
            // The member's output names the peer and its port, never its IP
            // (often a private one); the host's log keeps the address.
            tracing::info!(run = %gang.run, rank = gang.rank, peer = r, address = %p, "placed address");
            let who = gang
                .names
                .get(r)
                .or_else(|| gang.machines.get(r))
                .filter(|n| !n.is_empty())
                .map(String::as_str)
                .unwrap_or("its computer");
            let port = p.rsplit_once(':').map(|(_, port)| port).unwrap_or("?");
            notes.push(format!(
                "peer rank {r} via placed address (direct, no mesh) to {who} port {port}"
            ));
        }
    }
    for n in &notes {
        tracing::info!(run = %gang.run, rank = gang.rank, "{n}");
    }
    Ok(Some(RoutedPeers {
        peers: gang.peers.clone(),
        notes,
        hold: None,
    }))
}

#[async_trait]
impl PeerRouter for MeshPeerRouter {
    async fn route(&self, gang: &GangPeers, net: MemberNet) -> Result<Option<RoutedPeers>, String> {
        let rank = gang.rank as usize;
        if rank >= gang.peers.len() {
            return Err(format!("rank {rank} of a gang of {}", gang.peers.len()));
        }
        if gang.machines.len() != gang.peers.len() {
            return placed(
                gang,
                net,
                "the coordinator did not name the peers' computers",
            );
        }
        let mine = &gang.machines[rank];
        if gang.machines.iter().all(|m| m == mine) && net.is_machine() {
            // Every member on this machine: nothing crosses a network.
            return Ok(None);
        }
        self.route_mesh(gang, net).await
    }
}

impl MeshPeerRouter {
    #[cfg(unix)]
    async fn route_mesh(
        &self,
        gang: &GangPeers,
        net: MemberNet,
    ) -> Result<Option<RoutedPeers>, String> {
        use cua_mesh::gang::{ControlClient, PortRegistry};
        let rank = gang.rank as usize;
        let mine = &gang.machines[rank];
        let own_port = port_of(&gang.peers[rank])?;
        let control = cua_mesh::gang::control_path(&self.home);
        let mut ctl = match ControlClient::connect(&control).await {
            Ok(c) => c,
            // No control socket: this machine's host runs without the mesh
            // (CUA_MESH unset), which is a setting, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return placed(gang, net, "the mesh is off on this computer");
            }
            Err(e) => {
                return placed(
                    gang,
                    net,
                    &format!("the mesh is unavailable on this computer ({e})"),
                );
            }
        };
        // Registered before any forward, so a peer that starts first is
        // let in as soon as this member listens.
        let registry = PortRegistry::new(cua_mesh::gang::ports_dir(&self.home));
        let label = format!("{} attempt {} rank {rank}", gang.run, gang.attempt);
        // A member in a Space listens at its Space's address: this
        // machine's mesh node takes peers' streams there.
        let target = (!net.is_machine()).then(|| std::net::SocketAddr::new(net.listen, own_port));
        let reg = registry
            .register_to(own_port, &label, target)
            .map_err(|e| format!("registering gang port {own_port}: {e}"))?;
        let bind = (!net.is_machine()).then_some(net.gateway);
        let mut peers = Vec::with_capacity(gang.peers.len());
        let mut notes = Vec::new();
        for (r, (addr, machine)) in gang.peers.iter().zip(&gang.machines).enumerate() {
            let port = port_of(addr)?;
            if r == rank {
                peers.push(std::net::SocketAddr::new(net.listen, port).to_string());
                continue;
            }
            if machine == mine {
                if !net.is_machine() {
                    return Err(format!(
                        "rank {r} runs on this computer too, in another Space: a gang places \
                         one member per machine"
                    ));
                }
                peers.push(format!("127.0.0.1:{port}"));
                notes.push(format!("peer rank {r} on this computer"));
                continue;
            }
            match ctl.forward_on(machine, port, bind).await {
                Ok(f) => {
                    let path = if f.path.is_empty() {
                        "finding a path".to_string()
                    } else {
                        f.path.clone()
                    };
                    // The pool's name for the peer (the mesh's own peer name
                    // is the OS host name, often an internal one).
                    let who = gang
                        .names
                        .get(r)
                        .filter(|n| !n.is_empty())
                        .unwrap_or(machine);
                    notes.push(format!(
                        "peer rank {r} via mesh ({path}) to {who} port {port}"
                    ));
                    peers.push(f.local.to_string());
                }
                Err(e) => {
                    // All or nothing: a member that binds loopback cannot
                    // also take a peer's direct connection.
                    drop(reg);
                    return placed(
                        gang,
                        net,
                        &format!("the mesh cannot reach rank {r} on {machine} ({e})"),
                    );
                }
            }
        }
        for n in &notes {
            tracing::info!(run = %gang.run, rank, "{n}");
        }
        Ok(Some(RoutedPeers {
            peers,
            notes,
            hold: Some(Box::new(Held {
                _control: ctl,
                _port: reg,
            })),
        }))
    }

    #[cfg(not(unix))]
    async fn route_mesh(
        &self,
        gang: &GangPeers,
        net: MemberNet,
    ) -> Result<Option<RoutedPeers>, String> {
        placed(gang, net, "gang traffic over the mesh needs macOS or Linux")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gang(rank: u32, machines: &[&str]) -> GangPeers {
        GangPeers {
            run: "run-1".into(),
            attempt: 1,
            rank,
            peers: (0..machines.len())
                .map(|i| format!("10.0.0.{}:47010", i + 1))
                .collect(),
            machines: machines.iter().map(|m| m.to_string()).collect(),
            names: Vec::new(),
        }
    }

    #[tokio::test]
    async fn without_a_mesh_node_the_placed_addresses_stay_and_say_so() {
        let home = tempfile::tempdir().unwrap();
        let r = MeshPeerRouter::new(home.path())
            .route(&gang(0, &["m-a", "m-b"]), MemberNet::machine())
            .await
            .unwrap()
            .expect("routed");
        assert_eq!(r.peers, ["10.0.0.1:47010", "10.0.0.2:47010"]);
        assert!(
            r.notes[0]
                == "rank traffic uses the placed addresses: the mesh is off on this computer",
            "{:?}",
            r.notes
        );
        assert_eq!(
            r.notes[1],
            "peer rank 1 via placed address (direct, no mesh) to m-b port 47010"
        );
        assert!(
            r.notes.iter().all(|n| !n.contains("10.0.0.")),
            "{:?}",
            r.notes
        );
        // The pool's name wins over the machine id.
        let mut named = gang(1, &["m-a", "m-b"]);
        named.names = vec!["aws-1".into(), "aws-2".into()];
        let r = MeshPeerRouter::new(home.path())
            .route(&named, MemberNet::machine())
            .await
            .unwrap()
            .expect("routed");
        assert_eq!(
            r.notes[1],
            "peer rank 0 via placed address (direct, no mesh) to aws-1 port 47010"
        );
        assert!(r.hold.is_none());
        // A member in a VM cannot use its host's placed address.
        let vm = MemberNet {
            listen: "192.168.64.7".parse().unwrap(),
            gateway: "192.168.64.1".parse().unwrap(),
        };
        let e = MeshPeerRouter::new(home.path())
            .route(&gang(0, &["m-a", "m-b"]), vm)
            .await
            .unwrap_err();
        assert!(e.contains("the mesh is off on this computer"), "{e}");
    }

    #[tokio::test]
    async fn one_machine_or_unknown_machines_route_nothing_over_the_mesh() {
        let home = tempfile::tempdir().unwrap();
        let router = MeshPeerRouter::new(home.path());
        assert!(
            router
                .route(&gang(1, &["m-a", "m-a"]), MemberNet::machine())
                .await
                .unwrap()
                .is_none()
        );
        let mut g = gang(0, &["m-a", "m-b"]);
        g.machines.clear();
        let r = router
            .route(&g, MemberNet::machine())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.peers, g.peers);
        assert!(r.notes[0].contains("did not name the peers' computers"));
    }
}
