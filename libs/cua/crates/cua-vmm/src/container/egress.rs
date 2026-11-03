//! Egress policies of container sandboxes, enforced on the engine's host
//! ([`crate::egress`]).
//!
//! The rules live in the engine's own network namespace (the Linux host,
//! or the engine VM of Docker Desktop, Colima or the built-in runtime),
//! written as root through the engine's privileged helper container
//! ([`super::snapshot::Helper`]): nothing in a container, gVisor or runc,
//! root or not, can reach them.
//!
//! Every container gets a MAC address of its own at create
//! ([`crate::egress::engine_mac`], recorded in [`LABEL_MAC`]), so its rules
//! are in place before the container starts, keyed by that MAC; once it
//! runs, they are also bound to its bridge port, which must carry that
//! MAC (no frame can pass for another container's). A container created
//! with a policy runs without `CAP_NET_RAW` (it could otherwise send frames
//! with another MAC in the moment between its start and that binding; ping
//! still works through ICMP datagram sockets). A container started with a
//! policy it was not created for is first recreated with both, from a
//! commit of its filesystem.
//!
//! The policy is kept per container under `~/.cua/docker/egress/<name>.json`:
//! every start applies it again (an engine or host restart drops the
//! rules), and forks inherit it.
//!
//! **Secure default** ([`crate::network`]). Unless `CUA_SECURE_DEFAULT=0`,
//! every container is policed, policy or not: its network settings
//! (`internet` when none were given; an old egress policy becomes `custom`)
//! are compiled with what the engine host says (its resolvers, its own
//! addresses, its host aliases, the tree's subnet and other trees'), and the
//! rules always include the platform denies, `fib daddr type local` for
//! the engine host's own addresses and the default denies. Every container
//! runs without `CAP_NET_RAW`, with its own MAC and the anti-spoof rules,
//! on a Docker network of its own tree (`cua-t-<12 hex>`, inter-container
//! traffic on inside the tree): Spaces of other trees are on other bridges
//! and blocked. DNS keeps working: Docker's embedded resolver on a
//! user-defined network forwards to the engine host's resolvers, and those
//! are allowed on port 53.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{ContainerRuntime, engine_err};
use crate::egress::{self, BridgePort, EgressPolicy, EngineTarget, Net};
use crate::error::{Result, VmmError};
use crate::host;
use crate::network::{Mode, NetworkContext, NetworkSettings};
use sha2::{Digest, Sha256};
use std::net::IpAddr;

/// On a container: the MAC address it was created with.
pub const LABEL_MAC: &str = "ai.cua.mac";

/// The Docker network a container joins when it names none.
const DEFAULT_NETWORK: &str = "bridge";

/// On a tree's Docker network: the tree it is for.
pub const LABEL_TREE: &str = "ai.cua.tree";

/// The name prefix of a tree's Docker network.
pub const TREE_NETWORK_PREFIX: &str = "cua-t-";

/// Host aliases of Docker Desktop's VM (`host.docker.internal`, the
/// gateway), blocked when the engine host is on its network.
const DESKTOP_ALIASES: [&str; 3] = ["192.168.65.254", "192.168.65.1", "192.168.65.2"];
/// The host alias of a Lima VM (Colima: `host.lima.internal`).
const LIMA_ALIASES: [&str; 1] = ["192.168.5.2"];

/// How long one engine-host command may take.
const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

/// What is kept for one container.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    /// The policy (empty: none, set on purpose).
    pub policy: EgressPolicy,
    /// The nft table its rules were last written to, so a new MAC (a
    /// recreate) does not leave the old one behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// Its network settings ([`crate::network`]); `None`: kept from
    /// before them (migrated from `policy` at its next start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkSettings>,
    /// The tree it belongs to; `None`: its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
}

fn dir() -> PathBuf {
    host::cua_home().join("docker").join("egress")
}

fn path(name: &str) -> PathBuf {
    dir().join(format!("{name}.json"))
}

/// The policy kept for container `name`; `None` when none was ever set.
pub(crate) fn kept(name: &str) -> Option<Kept> {
    let bytes = std::fs::read(path(name)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Keeps `k` for container `name` (atomically).
pub(crate) fn keep(name: &str, k: &Kept) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = dir().join(format!(".{name}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(k)?)?;
    std::fs::rename(&tmp, path(name))?;
    Ok(())
}

/// Forgets what is kept for container `name`.
pub(crate) fn forget(name: &str) {
    let _ = std::fs::remove_file(path(name));
}

/// Gives container `to` the policy, network settings and tree of `from`
/// (a fork, a rename, a checkpoint). A fork of a child stays in its tree;
/// a fork of a top-level sandbox is a tree of its own, as its Space is.
pub(crate) fn copy(from: &str, to: &str) -> Result<()> {
    match kept(from) {
        Some(k) => keep(
            to,
            &Kept {
                policy: k.policy,
                table: None,
                network: k.network,
                tree: k.tree,
            },
        ),
        None => Ok(()),
    }
}

/// Whether the secure default applies ([`Mode::from_env`]).
pub(crate) fn secure() -> bool {
    Mode::from_env() == Mode::Secure
}

/// The network settings in force for `name` under the secure default
/// (`None` under the old open default): its own, or its old policy
/// migrated.
pub(crate) fn settings(name: &str) -> Option<NetworkSettings> {
    if !secure() {
        return None;
    }
    let k = kept(name).unwrap_or_default();
    Some(
        k.network
            .unwrap_or_else(|| NetworkSettings::migrate(&k.policy, false)),
    )
}

/// The tree `name` belongs to.
pub(crate) fn tree_of(name: &str) -> String {
    kept(name)
        .and_then(|k| k.tree)
        .unwrap_or_else(|| name.to_string())
}

/// The Docker network of tree `tree` for the cua home tagged `home_tag`:
/// `cua-t-<12 hex>`.
pub fn tree_network(home_tag: &str, tree: &str) -> String {
    let d = Sha256::digest(format!("{home_tag}:{tree}").as_bytes());
    let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
    format!("{TREE_NETWORK_PREFIX}{}", &hex[..12])
}

/// Whether container `name` is policed: always under the secure default,
/// else when its policy blocks anything.
pub(crate) fn blocks(name: &str) -> bool {
    secure() || kept(name).is_some_and(|k| policy_blocks(&k.policy))
}

/// What a stopped container must have before it may start policed: the
/// network it must be on, its MAC labelled and configured alike, and no
/// `CAP_NET_RAW`. `true`: it must be recreated.
pub(crate) fn must_recreate(
    expected_network: Option<&str>,
    networks: &[String],
    labelled_mac: Option<&str>,
    configured_mac: Option<&str>,
    raw_dropped: bool,
) -> bool {
    let wrong_network = expected_network.is_some_and(|n| networks != [n.to_string()]);
    labelled_mac.is_none() || labelled_mac != configured_mac || !raw_dropped || wrong_network
}

/// The engine host's facts from [`PROBE`]'s output (`dns=`, `addr=`,
/// `alias=` lines) and the networks' own: the tree's subnets (allowed), the
/// gateway (a host alias), other trees' subnets (blocked) and the daemon's
/// ports.
pub(crate) fn context_from(
    probe: &str,
    gateways: &[IpAddr],
    tree: &[Net],
    other_trees: &[Net],
    daemon_ports: &[u16],
) -> NetworkContext {
    let ips = |key: &str| -> Vec<IpAddr> {
        let mut out: Vec<IpAddr> = Vec::new();
        for ip in probe.lines().filter_map(|l| {
            l.strip_prefix(&format!("{key}="))
                .and_then(|v| v.split('/').next())
                .and_then(|v| v.trim().parse::<IpAddr>().ok())
        }) {
            if !out.contains(&ip) {
                out.push(ip);
            }
        }
        out
    };
    let addrs = ips("addr");
    let mut aliases: Vec<IpAddr> = gateways.to_vec();
    aliases.extend(ips("alias"));
    let on = |net: &str| {
        let n = Net::parse(net).expect("a valid network");
        addrs.iter().any(|a| n.contains(*a))
    };
    if on("192.168.65.0/24") {
        aliases.extend(
            DESKTOP_ALIASES
                .iter()
                .filter_map(|a| a.parse::<IpAddr>().ok()),
        );
    }
    if on("192.168.5.0/24") {
        aliases.extend(LIMA_ALIASES.iter().filter_map(|a| a.parse::<IpAddr>().ok()));
    }
    let mut host_aliases: Vec<IpAddr> = Vec::new();
    for a in aliases {
        if !a.is_loopback() && !host_aliases.contains(&a) {
            host_aliases.push(a);
        }
    }
    NetworkContext {
        // Docker's embedded resolver (127.0.0.11) answers inside the
        // container; it forwards to these.
        resolvers: ips("dns")
            .into_iter()
            .filter(|i| !i.is_loopback())
            .collect(),
        host_aliases,
        host_addrs: addrs.into_iter().filter(|i| !i.is_loopback()).collect(),
        tree: tree.to_vec(),
        other_trees: other_trees.to_vec(),
        daemon_ports: daemon_ports.to_vec(),
    }
}

/// Whether `policy` blocks any destination (an allow list alone does not).
pub(crate) fn policy_blocks(policy: &EgressPolicy) -> bool {
    policy.parse().is_ok_and(|r| r.blocks_anything())
}

/// The MAC address a new container named `name` gets.
pub(crate) fn mac_for(name: &str) -> String {
    egress::engine_mac(&format!("{}:{name}", super::home_tag()))
}

/// The engine-host script that reports the bridge's MAC address (`gw=`),
/// the host end of container process `$2`'s `eth0` (`veth=`, when `$2` is
/// not 0), and whether nft is there (`nft=missing`).
const PROBE: &str = r#"br="$1"; pid="$2"
ip -o link show dev "$br" 2>/dev/null | sed -n 's/.*link\/ether \([0-9a-fA-F:]*\).*/gw=\1/p'
if [ "$pid" != 0 ]; then
  idx=$(nsenter -t "$pid" -n ip -o link show dev eth0 2>/dev/null | sed -n 's/^[0-9]*: eth0@if\([0-9]*\):.*/\1/p')
  [ -n "$idx" ] && ip -o link | sed -n "s/^$idx: \([^:@]*\)[@:].*/veth=\1/p"
fi
command -v nft >/dev/null 2>&1 || echo nft=missing
for f in /etc/resolv.conf /run/systemd/resolve/resolv.conf; do
  [ -r "$f" ] && sed -n 's/^nameserver[[:space:]]*\([0-9a-fA-F:.]*\).*/dns=\1/p' "$f"
done
ip -o addr show 2>/dev/null | awk '{print "addr=" $4}'
for h in host.docker.internal gateway.docker.internal host.lima.internal; do
  getent hosts "$h" 2>/dev/null | awk '{print "alias=" $1}'
done
"#;

/// The engine-host script that feeds `$1` to `nft -f -`.
const APPLY: &str = r#"printf '%s' "$1" | nft -f -"#;

impl ContainerRuntime {
    /// Runs `script` with `args` as root in the engine host's own network
    /// namespace (the helper's chroot, then `nsenter` into pid 1's).
    async fn engine_sh(&self, script: &str, args: &[&str]) -> Result<super::snapshot::HelperOut> {
        let helper = self.helper();
        helper.ensure().await?;
        let mut argv = vec!["nsenter", "-t", "1", "-n", "--", "sh", "-c", script, "sh"];
        argv.extend_from_slice(args);
        helper.run(&argv, HELPER_TIMEOUT).await
    }

    /// The bridge device of Docker network `net` (`docker0` for `bridge`).
    async fn bridge_device(&self, net: &str) -> Result<String> {
        let n = self
            .docker
            .inspect_network(net, None)
            .await
            .map_err(engine_err)?;
        if n.driver.as_deref() != Some("bridge") {
            return Err(VmmError::Unsupported {
                backend: "container",
                op: "an egress policy on a network other than a bridge (the policy is enforced \
                     on the container's bridge port)",
            });
        }
        let dev = n
            .options
            .as_ref()
            .and_then(|o| o.get("com.docker.network.bridge.name").cloned())
            .or_else(|| {
                n.id.as_deref()
                    .filter(|id| id.len() >= 12)
                    .map(|id| format!("br-{}", &id[..12]))
            })
            .ok_or_else(|| VmmError::Engine(format!("network {net} has no bridge device")))?;
        if !egress::valid_ifname(&dev) {
            return Err(VmmError::Engine(format!(
                "network {net}: unexpected bridge name {dev:?}"
            )));
        }
        Ok(dev)
    }

    /// The Docker network of `name`'s tree, created when missing (a
    /// bridge with inter-container traffic on: the tree's members reach
    /// each other directly).
    pub(crate) async fn ensure_tree_network(&self, name: &str) -> Result<String> {
        let tree = tree_of(name);
        let net = tree_network(&super::home_tag(), &tree);
        if self.docker.inspect_network(&net, None).await.is_ok() {
            return Ok(net);
        }
        let req = bollard::models::NetworkCreateRequest {
            name: net.clone(),
            driver: Some("bridge".into()),
            enable_ipv6: Some(false),
            options: Some(
                [(
                    "com.docker.network.bridge.enable_icc".to_string(),
                    "true".to_string(),
                )]
                .into_iter()
                .collect(),
            ),
            labels: Some(
                [
                    (LABEL_TREE.to_string(), tree),
                    ("ai.cua.home".to_string(), super::home_tag()),
                ]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        };
        match self.docker.create_network(req).await {
            Ok(_) => Ok(net),
            // Another start made it first.
            Err(e) => match self.docker.inspect_network(&net, None).await {
                Ok(_) => Ok(net),
                Err(_) => Err(VmmError::Engine(format!(
                    "could not create the network of {name}'s tree ({net}): {e}"
                ))),
            },
        }
    }

    /// Removes the Docker network of tree `tree` once no container is on
    /// it (its last member was deleted).
    pub(crate) async fn drop_tree_network_if_empty(&self, tree: &str) {
        let net = tree_network(&super::home_tag(), tree);
        let Ok(n) = self.docker.inspect_network(&net, None).await else {
            return;
        };
        if n.containers.as_ref().is_none_or(|c| c.is_empty())
            && let Err(e) = self.docker.remove_network(&net).await
        {
            tracing::debug!(network = %net, error = %e, "could not remove an empty tree network");
        }
    }

    /// The `NetworkingConfig` that gives a new container `name` its own MAC
    /// on the network it belongs on, with the MAC: its tree's network under
    /// the secure default (an error when it can't be made: never an open
    /// container), else the engine's default bridge (`None` when the engine
    /// has none: such an engine refuses policies).
    pub(crate) async fn mac_config(
        &self,
        name: &str,
    ) -> Result<Option<(bollard::models::NetworkingConfig, String)>> {
        let net = if secure() {
            self.ensure_tree_network(name).await?
        } else {
            let Ok(n) = self.docker.inspect_network(DEFAULT_NETWORK, None).await else {
                return Ok(None);
            };
            if n.driver.as_deref() != Some("bridge") {
                return Ok(None);
            }
            DEFAULT_NETWORK.to_string()
        };
        let mac = mac_for(name);
        let endpoint = bollard::models::EndpointSettings {
            mac_address: Some(mac.clone()),
            ..Default::default()
        };
        Ok(Some((
            bollard::models::NetworkingConfig {
                endpoints_config: Some([(net, endpoint)].into_iter().collect()),
            },
            mac,
        )))
    }

    /// The subnets and gateways of Docker network `net`.
    async fn subnets(&self, net: &str) -> (Vec<Net>, Vec<IpAddr>) {
        let Ok(n) = self.docker.inspect_network(net, None).await else {
            return (vec![], vec![]);
        };
        let configs = n.ipam.and_then(|i| i.config).unwrap_or_default();
        let nets = configs
            .iter()
            .filter_map(|c| c.subnet.as_deref())
            .filter_map(|s| Net::parse(s).ok())
            .collect();
        let gws = configs
            .iter()
            .filter_map(|c| c.gateway.as_deref())
            .filter_map(|g| g.parse().ok())
            .collect();
        (nets, gws)
    }

    /// The subnets of every other tree's network on this engine, and of its
    /// default bridge: Spaces there are blocked.
    async fn other_tree_subnets(&self, own: &str) -> Vec<Net> {
        let filters = [("label".to_string(), vec![LABEL_TREE.to_string()])]
            .into_iter()
            .collect();
        let mut names: Vec<String> = self
            .docker
            .list_networks(Some(bollard::query_parameters::ListNetworksOptions {
                filters: Some(filters),
            }))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|n| n.name)
            .filter(|n| n != own)
            .collect();
        if own != DEFAULT_NETWORK {
            names.push(DEFAULT_NETWORK.to_string());
        }
        let mut out = Vec::new();
        for n in names {
            out.extend(self.subnets(&n).await.0);
        }
        out
    }

    /// Where container `i`'s rules go: its network, MAC and address, its
    /// bridge port when it runs (`running`), the bridge and its MAC.
    async fn engine_target(
        &self,
        i: &bollard::models::ContainerInspectResponse,
        running: bool,
    ) -> Result<(EngineTarget, NetworkContext)> {
        let name = i
            .name
            .as_deref()
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_string();
        let mode = i
            .host_config
            .as_ref()
            .and_then(|h| h.network_mode.clone())
            .unwrap_or_default();
        let nets = i
            .network_settings
            .as_ref()
            .and_then(|n| n.networks.clone())
            .unwrap_or_default();
        let unsupported = |op: &'static str| VmmError::Unsupported {
            backend: "container",
            op,
        };
        if mode == "host" || mode == "none" || mode.starts_with("container:") {
            return Err(unsupported(
                "an egress policy on a container without a network of its own (host, none or \
                 another container's)",
            ));
        }
        if nets.len() != 1 {
            return Err(unsupported(
                "an egress policy on a container on several networks (the policy is enforced \
                 on its one bridge port)",
            ));
        }
        let (net, ep) = nets.into_iter().next().expect("one network");
        let mac = ep
            .mac_address
            .as_deref()
            .and_then(egress::normalize_mac)
            .ok_or_else(|| {
                VmmError::other(format!(
                    "container {name} has no MAC address on {net} yet; start it once, or \
                     recreate it"
                ))
            })?;
        let bridge = self.bridge_device(&net).await?;
        let pid = if running {
            i.state.as_ref().and_then(|s| s.pid).unwrap_or(0)
        } else {
            0
        };
        let out = self.engine_sh(PROBE, &[&bridge, &pid.to_string()]).await?;
        let field = |k: &str| {
            out.stdout
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")).map(str::to_string))
        };
        if field("nft").as_deref() == Some("missing") {
            return Err(VmmError::missing(
                "nft (nftables) on the container engine's host",
                "install nftables where the engine runs (Ubuntu: sudo apt-get install nftables); \
                 egress policies are enforced there",
            ));
        }
        let gateway_mac = field("gw")
            .as_deref()
            .and_then(egress::normalize_mac)
            .ok_or_else(|| {
                VmmError::Engine(format!(
                    "could not read the MAC address of bridge {bridge} on the engine's host: {}",
                    out.stderr.trim()
                ))
            })?;
        let port = if running {
            let veth = field("veth")
                .filter(|v| egress::valid_ifname(v))
                .ok_or_else(|| {
                    VmmError::Engine(format!(
                        "could not find the bridge port of container {name} on the engine's \
                         host: {}",
                        out.stderr.trim()
                    ))
                })?;
            Some(BridgePort {
                veth,
                ipv4: ep
                    .ip_address
                    .as_deref()
                    .and_then(|a| a.parse::<Ipv4Addr>().ok()),
            })
        } else {
            None
        };
        let tree_net = net.starts_with(TREE_NETWORK_PREFIX);
        let ctx = if secure() {
            let (subnets, gateways) = self.subnets(&net).await;
            context_from(
                &out.stdout,
                &gateways,
                // The tree's own bridge: its members. The default bridge is
                // shared, so it has none.
                if tree_net { &subnets } else { &[] },
                &self.other_tree_subnets(&net).await,
                &crate::network::daemon_ports(),
            )
        } else {
            NetworkContext::default()
        };
        Ok((
            EngineTarget {
                table: egress::engine_table(&mac),
                bridge,
                gateway_mac,
                mac,
                port,
                isolate_l2: !tree_net,
            },
            ctx,
        ))
    }

    /// Writes `text` with nft on the engine host.
    async fn engine_apply(&self, text: &str) -> Result<()> {
        let out = self.engine_sh(APPLY, &[text]).await?;
        if out.code != 0 {
            return Err(VmmError::Engine(format!(
                "the engine's host refused the egress rules: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    /// Writes `policy` for container `name` on the engine host (keyed by
    /// its MAC; bound to its bridge port as well when it runs) and keeps it.
    pub(crate) async fn police(&self, name: &str, policy: &EgressPolicy) -> Result<()> {
        let legacy = egress::check(crate::BackendKind::Container, policy)?;
        let i = self
            .inspect(name)
            .await?
            .ok_or_else(|| VmmError::NotFound(name.into()))?;
        let previous_kept = kept(name);
        let previous = previous_kept.as_ref().and_then(|k| k.table.clone());
        let network = settings(name);
        if network.is_none() && !legacy.blocks_anything() {
            // Nothing to filter: remove what was there.
            if let Some(t) = &previous {
                self.engine_apply(&egress::engine_remove(t)).await?;
            }
            return keep(
                name,
                &Kept {
                    policy: policy.clone(),
                    table: None,
                    ..previous_kept.unwrap_or_default()
                },
            );
        }
        let running = matches!(
            Self::status_from(&i),
            crate::Status::Running | crate::Status::Paused
        );
        let (target, ctx) = self.engine_target(&i, running).await?;
        let rules = match &network {
            Some(s) => crate::network::compile_mode(s, &ctx, Mode::Secure)?,
            None => legacy,
        };
        let mut text = String::new();
        if let Some(t) = previous.as_ref().filter(|t| **t != target.table) {
            text.push_str(&egress::engine_remove(t));
        }
        text.push_str(&egress::engine_ruleset(&target, &rules));
        self.engine_apply(&text).await?;
        keep(
            name,
            &Kept {
                policy: policy.clone(),
                table: Some(target.table),
                network: network.or(previous_kept.as_ref().and_then(|k| k.network.clone())),
                tree: previous_kept.and_then(|k| k.tree),
            },
        )
    }

    /// Removes container `name`'s rules from the engine host and forgets
    /// its policy (the container is gone).
    pub(crate) async fn unpolice(&self, name: &str) {
        if let Some(t) = kept(name).and_then(|k| k.table)
            && let Err(e) = self.engine_apply(&egress::engine_remove(&t)).await
        {
            tracing::warn!(container = name, error = %e, "could not remove a deleted container's egress rules");
        }
        let tree = tree_of(name);
        forget(name);
        // Its tree's network goes with the tree's last member.
        self.drop_tree_network_if_empty(&tree).await;
    }

    /// Whether stopped container `i` must be recreated before it may start
    /// under the policy kept for it: it lacks its own MAC or still has
    /// `CAP_NET_RAW`.
    pub(crate) fn needs_recreate(
        &self,
        name: &str,
        i: &bollard::models::ContainerInspectResponse,
    ) -> bool {
        if !blocks(name) {
            return false;
        }
        let labelled = i
            .config
            .as_ref()
            .and_then(|c| c.labels.as_ref())
            .and_then(|l| l.get(LABEL_MAC))
            .and_then(|m| egress::normalize_mac(m));
        let configured = i
            .network_settings
            .as_ref()
            .and_then(|n| n.networks.as_ref())
            .and_then(|n| n.values().next())
            .and_then(|e| e.mac_address.as_deref())
            .and_then(egress::normalize_mac);
        let raw_dropped = i
            .host_config
            .as_ref()
            .and_then(|h| h.cap_drop.as_ref())
            .is_some_and(|c| c.iter().any(|x| x.trim_start_matches("CAP_") == "NET_RAW"));
        let networks: Vec<String> = i
            .network_settings
            .as_ref()
            .and_then(|n| n.networks.as_ref())
            .map(|n| n.keys().cloned().collect())
            .unwrap_or_default();
        // Under the secure default it must be on its tree's network.
        let expected = secure().then(|| tree_network(&super::home_tag(), &tree_of(name)));
        must_recreate(
            expected.as_deref(),
            &networks,
            labelled.as_deref(),
            configured.as_deref(),
            raw_dropped,
        )
    }

    /// Starts container `name` under the policy kept for it: the rules are
    /// written (keyed by its MAC) before it starts and bound to its bridge
    /// port once it runs. A container whose rules cannot be written does
    /// not start, and one whose port cannot be bound is killed.
    pub(crate) async fn start_policed(&self, name: &str) -> Result<()> {
        // Under the secure default every container is policed.
        let policy = kept(name)
            .map(|k| k.policy)
            .filter(|p| secure() || policy_blocks(p))
            .or_else(|| secure().then(EgressPolicy::default));
        if let Some(p) = &policy {
            self.police(name, p).await?;
        }
        self.docker
            .start_container(name, None)
            .await
            .map_err(engine_err)?;
        if let Some(p) = &policy
            && let Err(e) = self.police(name, p).await
        {
            let _ = self
                .docker
                .kill_container(
                    name,
                    None::<bollard::query_parameters::KillContainerOptions>,
                )
                .await;
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_and_apply_scripts_take_their_arguments_positionally() {
        // Nothing of the caller's is pasted into the scripts: names and
        // the ruleset arrive as $1/$2.
        assert!(PROBE.contains(r#"br="$1"; pid="$2""#));
        assert!(APPLY.contains(r#""$1""#));
    }

    #[test]
    fn one_network_per_tree() {
        let a = tree_network("home1", "local:root");
        assert_eq!(a, tree_network("home1", "local:root"));
        assert!(a.starts_with(TREE_NETWORK_PREFIX));
        assert_eq!(a.len(), TREE_NETWORK_PREFIX.len() + 12);
        assert_ne!(a, tree_network("home1", "local:other"));
        assert_ne!(a, tree_network("home2", "local:root"), "per cua home");
    }

    #[test]
    fn a_container_off_its_tree_network_is_recreated() {
        let net = "cua-t-0123456789ab".to_string();
        let mac = Some("02:aa:bb:cc:dd:ee");
        // Right network, MAC and capabilities: kept.
        assert!(!must_recreate(
            Some(&net),
            std::slice::from_ref(&net),
            mac,
            mac,
            true
        ));
        // On the default bridge (made before the secure default): recreated.
        assert!(must_recreate(
            Some(&net),
            &["bridge".into()],
            mac,
            mac,
            true
        ));
        // Still has NET_RAW, or its MAC differs, or has none: recreated.
        assert!(must_recreate(
            Some(&net),
            std::slice::from_ref(&net),
            mac,
            mac,
            false
        ));
        assert!(must_recreate(
            Some(&net),
            std::slice::from_ref(&net),
            mac,
            Some("02:00:00:00:00:01"),
            true
        ));
        assert!(must_recreate(None, &["bridge".into()], None, None, true));
        // The old default: any network will do.
        assert!(!must_recreate(None, &["bridge".into()], mac, mac, true));
    }

    #[test]
    fn the_engine_hosts_facts_become_the_context() {
        let probe = "gw=0a:de:71:61:ef:e1\nveth=veth1\n\
                     dns=127.0.0.53\ndns=10.0.0.2\ndns=10.0.0.2\n\
                     addr=127.0.0.1/8\naddr=10.0.1.5/24\naddr=172.18.0.1/16\naddr=fe80::1/64\n\
                     alias=172.17.0.1\n";
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let tree = [Net::parse("172.18.0.0/16").unwrap()];
        let others = [Net::parse("172.17.0.0/16").unwrap()];
        let c = context_from(probe, &[ip("172.18.0.1")], &tree, &others, &[4100]);
        assert_eq!(
            c.resolvers,
            [ip("10.0.0.2")],
            "loopback resolvers are the engine's own"
        );
        assert_eq!(c.host_aliases, [ip("172.18.0.1"), ip("172.17.0.1")]);
        assert_eq!(
            c.host_addrs,
            [ip("10.0.1.5"), ip("172.18.0.1"), ip("fe80::1")]
        );
        assert_eq!(c.tree, tree);
        assert_eq!(c.other_trees, others);
        assert_eq!(c.daemon_ports, [4100]);
        // Docker Desktop's and Lima's host aliases, when the engine is there.
        let c = context_from("addr=192.168.65.3/24\n", &[], &[], &[], &[]);
        assert!(c.host_aliases.contains(&ip("192.168.65.254")));
        let c = context_from("addr=192.168.5.15/24\n", &[], &[], &[], &[]);
        assert!(c.host_aliases.contains(&ip("192.168.5.2")));
        let c = context_from("addr=10.0.1.5/24\n", &[], &[], &[], &[]);
        assert!(c.host_aliases.is_empty());
        // Compiled: the gateway and the engine's addresses are closed, the
        // tree is open, other trees and the resolver as they should be.
        let c = context_from(probe, &[ip("172.18.0.1")], &tree, &others, &[4100]);
        let r = crate::network::compile_mode(
            &NetworkSettings::internet(),
            &c,
            crate::network::Mode::Secure,
        )
        .unwrap();
        assert!(!r.allows(ip("172.18.0.1")));
        assert!(!r.allows(ip("10.0.1.5")));
        assert!(r.allows(ip("172.18.0.7")));
        assert!(!r.allows(ip("172.17.0.4")));
        assert!(r.allows_port(ip("10.0.0.2"), crate::egress::Proto::Udp, 53));
        assert!(!r.allows_port(ip("10.0.0.2"), crate::egress::Proto::Tcp, 22));
        assert!(r.rules.iter().any(|r| r.host_local && !r.accept));
    }

    #[test]
    fn the_probe_reads_resolvers_addresses_and_aliases() {
        assert!(PROBE.contains("dns=\\1"), "{PROBE}");
        assert!(PROBE.contains("/run/systemd/resolve/resolv.conf"));
        assert!(PROBE.contains("print \"addr=\" $4"));
        assert!(PROBE.contains("host.docker.internal"));
    }

    #[test]
    fn a_mac_per_container_name() {
        assert_eq!(mac_for("web"), mac_for("web"));
        assert_ne!(mac_for("web"), mac_for("db"));
        assert!(egress::normalize_mac(&mac_for("web")).is_some());
    }

    #[test]
    fn policy_blocks_only_with_a_deny_entry() {
        let p = |a: &[&str], d: &[&str]| EgressPolicy {
            allow: a.iter().map(|s| s.to_string()).collect(),
            deny: d.iter().map(|s| s.to_string()).collect(),
        };
        assert!(policy_blocks(&p(&[], &["all"])));
        assert!(policy_blocks(&p(&["1.1.1.1"], &["10.0.0.0/8"])));
        assert!(!policy_blocks(&p(&["1.1.1.1"], &[])));
        assert!(!policy_blocks(&p(&[], &[])));
        assert!(!policy_blocks(&p(&[], &["example.com"])));
    }
}
