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
//! and blocked.
//!
//! **DNS.** A user-defined network gives a container `nameserver
//! 127.0.0.11` (Docker's embedded resolver), which gVisor cannot reach. So
//! every policed container gets its own `/etc/resolv.conf`, a file kept on
//! the engine host and bind-mounted over the guest's
//! ([`crate::resolver`]): the engine host's resolvers, or the configured or
//! public ones when it has none a guest can reach. Exactly those servers
//! are allowed on port 53 ([`NetworkContext::resolvers`]); a Space whose
//! network is `none` gets a file without a server. The file is rewritten at
//! every start and every change of the network settings. A container made
//! before this has no such mount: its `resolv.conf` is written through an
//! exec once it runs.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{ContainerRuntime, engine_err};
use crate::egress::{self, BridgePort, EgressPolicy, EngineTarget, Net};
use crate::error::{Result, VmmError};
use crate::host;
use crate::network::{Mode, NetworkContext, NetworkPreset, NetworkSettings};
use crate::resolver;
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
    /// The counted (drop) rules of `table`, in order, with their reason and
    /// network, for `cua network log` ([`egress::drop_labels`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drop_labels: Vec<(String, String)>,
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
                drop_labels: vec![],
            },
        ),
        None => Ok(()),
    }
}

/// After a warm-pool claim: `to` stays in the tree `from` was in (`from`'s own tree when it had none), since the
/// container did not move networks when it was renamed.
pub(crate) fn keep_tree_of_claimed(from: &str, to: &str) -> Result<()> {
    let Some(mut k) = kept(to) else { return Ok(()) };
    k.tree = Some(claimed_tree(kept(from).and_then(|f| f.tree), from));
    keep(to, &k)
}

/// The tree a claimed container stays in: the one the member had, else the member's own.
pub(crate) fn claimed_tree(member_tree: Option<String>, member: &str) -> String {
    member_tree.unwrap_or_else(|| member.to_string())
}

#[cfg(test)]
mod claimed_tree_tests {
    use super::claimed_tree;

    #[test]
    fn a_claimed_member_keeps_the_network_of_its_own_tree() {
        assert_eq!(claimed_tree(None, "cua-pool-1"), "cua-pool-1");
        assert_eq!(claimed_tree(Some("root".into()), "cua-pool-1"), "root");
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
    let tree = kept(name)
        .and_then(|k| k.tree)
        .unwrap_or_else(|| name.to_string());
    canonical_tree(&tree, |root| kept(root).and_then(|k| k.tree))
}

/// A tree named by its root Space (`local:<name>`, what a child is told) is the tree the root's container really sits
/// in: a warm-claimed home keeps its pool member's tree, so its children must join THAT network, not a new one.
pub(crate) fn canonical_tree(tree: &str, root_tree: impl Fn(&str) -> Option<String>) -> String {
    match tree.strip_prefix("local:").and_then(root_tree) {
        Some(real) => real,
        None => tree.to_string(),
    }
}

#[cfg(test)]
mod canonical_tree_tests {
    use super::canonical_tree;

    #[test]
    fn the_children_of_a_claimed_home_join_the_homes_real_network() {
        let kept = |root: &str| (root == "fancy-civet").then(|| "cua-pool-1".to_string());
        assert_eq!(canonical_tree("local:fancy-civet", kept), "cua-pool-1");
        // A home made cold has no kept tree: its tree is its id, as before.
        assert_eq!(canonical_tree("local:cold", kept), "local:cold");
        assert_eq!(canonical_tree("not-a-space-id", kept), "not-a-space-id");
    }
}

/// The name container `name` of tree `tree` answers to on its tree's
/// network: `<space>.<tree>`.
pub fn tree_alias(name: &str, tree: &str) -> String {
    crate::tree::member_name(name, tree)
}

/// Tree networks this process made lately, by name: one in flight has no container yet and must not be pruned.
fn made_lately() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>
{
    static M: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    > = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

fn note_made(name: &str) {
    if let Ok(mut m) = made_lately().lock() {
        m.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(300));
        m.insert(name.to_string(), std::time::Instant::now());
    }
}

fn recently_made(name: &str) -> bool {
    made_lately().lock().is_ok_and(|m| {
        m.get(name)
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(300))
    })
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

/// The MAC address a container's rules are keyed by: the one its endpoint
/// reports, else, for a container that is not running, the one labelled
/// at creation. Docker 29 reports no MAC for a stopped container (the
/// endpoint's `MacAddress` is empty until it runs again), so without the
/// label every stopped Space "must be recreated" and none could restart
/// under the secure default. The MAC is pinned at creation next to the
/// label, and the rules are keyed again by the address the running
/// container really has right after it starts, so a mismatch cannot go
/// unnoticed.
pub(crate) fn rules_mac(
    reported: Option<&str>,
    labelled: Option<&str>,
    running: bool,
) -> Option<String> {
    reported.and_then(egress::normalize_mac).or_else(|| {
        (!running)
            .then(|| labelled.and_then(egress::normalize_mac))
            .flatten()
    })
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
    // What a guest may ask ([`resolver::select_from_env`]). A resolver may
    // be an address of the engine host itself (Colima's VM is its own
    // resolver): it is opened on port 53 only, as it always was, before the
    // host's closed addresses ([`crate::network`]).
    let resolvers = resolver::select_from_env(&ips("dns"), false);
    NetworkContext {
        // Written into the guest's resolv.conf and allowed on port 53:
        // the same list ([`resolver`]).
        resolvers,
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
if nft add table bridge cua_probe_br 2>/dev/null; then nft delete table bridge cua_probe_br 2>/dev/null; echo bridgefam=ok; else echo bridgefam=missing; fi
if nft -f - 2>/dev/null <<'EOT'
add table inet cua_probe_fib
add chain inet cua_probe_fib c
add rule inet cua_probe_fib c fib daddr type local counter
EOT
then echo fib=ok; else echo fib=missing; fi
nft delete table inet cua_probe_fib 2>/dev/null
for f in /etc/resolv.conf /run/systemd/resolve/resolv.conf; do
  [ -r "$f" ] && sed -n 's/^nameserver[[:space:]]*\([0-9a-fA-F:.]*\).*/dns=\1/p' "$f"
done
ip -o addr show 2>/dev/null | awk '{print "addr=" $4}'
for h in host.docker.internal gateway.docker.internal host.lima.internal; do
  getent hosts "$h" 2>/dev/null | awk '{print "alias=" $1}'
done
"#;

/// Where on the engine host each container's `resolv.conf` is kept.
const RESOLV_DIR: &str = "/var/lib/cua/resolv";

/// The guest path the file is mounted at.
const RESOLV_TARGET: &str = "/etc/resolv.conf";

/// The engine-host script that writes `$1` as container `$2`'s resolv.conf
/// (in place, so a mounted file follows).
const WRITE_RESOLV: &str = r#"mkdir -p /var/lib/cua/resolv && printf '%s' "$1" > "/var/lib/cua/resolv/$2.conf" && chmod 644 "/var/lib/cua/resolv/$2.conf""#;

/// The engine-host script that removes container `$1`'s resolv.conf.
const REMOVE_RESOLV: &str = r#"rm -f "/var/lib/cua/resolv/$1.conf""#;

/// The bind that mounts container `name`'s resolv.conf (engine host path).
pub(crate) fn resolv_bind(name: &str) -> String {
    format!("{RESOLV_DIR}/{name}.conf:{RESOLV_TARGET}")
}

/// Whether `binds` mount a resolv.conf of ours.
pub(crate) fn has_resolv_bind(binds: Option<&Vec<String>>) -> bool {
    binds.is_some_and(|b| {
        b.iter()
            .any(|b| b.starts_with(RESOLV_DIR) && b.ends_with(&format!(":{RESOLV_TARGET}")))
    })
}

/// Whether `name` is safe as a file name on the engine host.
fn file_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The servers a guest of `settings` is given from `ctx`: none without a
/// network.
pub(crate) fn guest_servers(
    settings: Option<&NetworkSettings>,
    ctx: &NetworkContext,
) -> Vec<IpAddr> {
    match settings {
        Some(s) if s.preset == NetworkPreset::None => Vec::new(),
        _ => ctx.resolvers.clone(),
    }
}

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

    /// The `/etc/resolv.conf` mount for a container about to be created
    /// (`None` under the old open default, which keeps Docker's own), with
    /// the servers it names. The file is on the engine host before the
    /// container exists, so Docker never makes a directory of the path.
    pub(crate) async fn prepare_resolv(&self, name: &str) -> Result<Option<(String, Vec<IpAddr>)>> {
        if !secure() {
            return Ok(None);
        }
        if !file_name_ok(name) {
            return Err(VmmError::invalid(format!("bad container name {name:?}")));
        }
        let ctx = self.dns_context().await?;
        let servers = guest_servers(settings(name).as_ref(), &ctx);
        self.write_resolv_file(name, &servers).await?;
        Ok(Some((resolv_bind(name), servers)))
    }

    /// What the engine host says about its resolvers (the part of
    /// [`PROBE`] that matters here).
    async fn dns_context(&self) -> Result<NetworkContext> {
        let out = self.engine_sh(PROBE, &["docker0", "0"]).await?;
        Ok(context_from(&out.stdout, &[], &[], &[], &[]))
    }

    async fn write_resolv_file(&self, name: &str, servers: &[IpAddr]) -> Result<()> {
        let text = resolver::resolv_conf(servers);
        let out = self.engine_sh(WRITE_RESOLV, &[&text, name]).await?;
        if out.code != 0 {
            return Err(VmmError::Engine(format!(
                "could not write the resolv.conf of {name} on the engine's host: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    /// Gives container `name` (`i`, running or about to be) `servers` to
    /// ask: its mounted file is rewritten, and a container made before
    /// there was one gets its `/etc/resolv.conf` written through an exec
    /// (best effort, once it runs).
    async fn set_guest_dns(
        &self,
        name: &str,
        i: &bollard::models::ContainerInspectResponse,
        servers: &[IpAddr],
    ) -> Result<()> {
        if has_resolv_bind(i.host_config.as_ref().and_then(|h| h.binds.as_ref())) {
            return self.write_resolv_file(name, servers).await;
        }
        let running = matches!(Self::status_from(i), crate::Status::Running);
        if !running || !file_name_ok(name) {
            return Ok(());
        }
        use crate::exec::{ExecRequest, GuestExec};
        let text = resolver::resolv_conf(servers);
        let req = ExecRequest::sh(format!("cat > {RESOLV_TARGET}"))
            .stdin(text.into_bytes())
            .timeout(HELPER_TIMEOUT);
        let req = ExecRequest {
            user: Some("0".into()),
            ..req
        };
        if let Err(e) = self.exec_handle(name).exec(req).await {
            tracing::warn!(container = name, error = %e, "could not write the resolv.conf of a container made before Cua mounted one");
        }
        Ok(())
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
        // A new tree takes an address pool: give back those of trees nobody is on first.
        let freed = self.prune_empty_tree_networks().await;
        if freed > 0 {
            tracing::info!(freed, "removed tree networks no container was on");
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
            Ok(_) => {
                note_made(&net);
                Ok(net)
            }
            // Another start made it first.
            Err(e) => match self.docker.inspect_network(&net, None).await {
                Ok(_) => Ok(net),
                Err(_) => Err(VmmError::Engine(format!(
                    "could not create the network of {name}'s tree ({net}): {e}"
                ))),
            },
        }
    }

    /// Removes every tree network of this home that no container is on. A container can be removed on paths that never
    /// reach [`Self::unpolice`] (a warm-pool member, a failed start, a delete after a crash), and each tree holds an address
    /// pool of the engine's few: 26 leaked ones made the next "create two child Spaces" fail with "all predefined address
    /// pools have been fully subnetted". Run before a new tree network is made.
    pub(crate) async fn prune_empty_tree_networks(&self) -> usize {
        let filters = [(
            "label".to_string(),
            vec![format!("ai.cua.home={}", super::home_tag())],
        )]
        .into_iter()
        .collect();
        let listed = self
            .docker
            .list_networks(Some(bollard::query_parameters::ListNetworksOptions {
                filters: Some(filters),
            }))
            .await
            .unwrap_or_default();
        let mut removed = 0;
        for name in listed.into_iter().filter_map(|n| n.name) {
            if !name.starts_with(TREE_NETWORK_PREFIX) {
                continue;
            }
            let Ok(n) = self.docker.inspect_network(&name, None).await else {
                continue;
            };
            // A network this process just made for a start in flight has no container yet: leave it.
            if recently_made(&name) || n.containers.as_ref().is_some_and(|c| !c.is_empty()) {
                continue;
            }
            if self.docker.remove_network(&name).await.is_ok() {
                removed += 1;
            }
        }
        removed
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
        // On its tree's network, its tree reaches it by name too:
        // `<space>.<tree>` (crate::tree::member_name).
        let aliases = secure().then(|| vec![tree_alias(name, &tree_of(name))]);
        let endpoint = bollard::models::EndpointSettings {
            mac_address: Some(mac.clone()),
            aliases,
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
        let label_mac = i
            .config
            .as_ref()
            .and_then(|c| c.labels.as_ref())
            .and_then(|l| l.get(LABEL_MAC))
            .map(String::as_str);
        let mac = rules_mac(ep.mac_address.as_deref(), label_mac, running).ok_or_else(|| {
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
        let bridge_family = field("bridgefam").as_deref() != Some("missing");
        let fib = field("fib").as_deref() != Some("missing");
        let host_addrs: Vec<std::net::IpAddr> = out
            .stdout
            .lines()
            .filter_map(|l| l.strip_prefix("addr="))
            .filter_map(|a| a.split('/').next()?.parse().ok())
            .collect();
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
                bridge_family,
                fib,
                host_addrs,
            },
            ctx,
        ))
    }

    /// Whether the engine host's kernel has the nftables `bridge` family
    /// (true when it cannot be told: the rules are then written as before).
    async fn engine_bridge_family(&self) -> bool {
        let script = r#"if nft add table bridge cua_probe_br 2>/dev/null; then nft delete table bridge cua_probe_br 2>/dev/null; echo bridgefam=ok; else echo bridgefam=missing; fi"#;
        match self.engine_sh(script, &[]).await {
            Ok(o) => !o.stdout.contains("bridgefam=missing"),
            Err(_) => true,
        }
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
                let family = self.engine_bridge_family().await;
                self.engine_apply(&egress::engine_remove_for(t, family))
                    .await?;
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
        if secure() {
            let servers = guest_servers(network.as_ref(), &ctx);
            self.set_guest_dns(name, &i, &servers).await?;
        }
        let rules = match &network {
            Some(s) => crate::network::compile_mode(s, &ctx, Mode::Secure)?,
            None => legacy,
        };
        if !target.bridge_family {
            if !rules.user_rules_in_tree().is_empty() {
                return Err(VmmError::Engine(egress::NO_BRIDGE_FAMILY.into()));
            }
            tracing::warn!(container = name, "{}", egress::BRIDGE_FAMILY_LIMITS);
        }
        let mut text = String::new();
        if let Some(t) = previous.as_ref().filter(|t| **t != target.table) {
            text.push_str(&egress::engine_remove_for(t, target.bridge_family));
        }
        text.push_str(&egress::engine_ruleset(&target, &rules));
        if let Err(e) = self.engine_apply(&text).await {
            // A deny inside the tree needs the kernel's bridge conntrack:
            // say so plainly when that is what the host lacks.
            if !rules.user_rules_in_tree().is_empty()
                && self
                    .engine_apply(egress::BRIDGE_CONNTRACK_PROBE)
                    .await
                    .is_err()
            {
                return Err(VmmError::Engine(egress::NO_BRIDGE_CONNTRACK.into()));
            }
            return Err(e);
        }
        keep(
            name,
            &Kept {
                policy: policy.clone(),
                table: Some(target.table),
                network: network.or(previous_kept.as_ref().and_then(|k| k.network.clone())),
                tree: previous_kept.and_then(|k| k.tree),
                // The `inet` chain's drops, then the bridge's tree chain's.
                drop_labels: egress::drop_labels(&rules)
                    .into_iter()
                    .chain(egress::tree_drop_labels(&rules))
                    .collect(),
            },
        )
    }

    /// What container `name` tried to reach and its rules refused: the
    /// counters of its drop rules on the engine host (by rule, so the
    /// destination is the rule's network).
    pub(crate) async fn blocked(&self, name: &str) -> Result<Vec<egress::NetworkEvent>> {
        let Some(k) = kept(name) else {
            return Ok(vec![]);
        };
        let Some(table) = k.table else {
            return Ok(vec![]);
        };
        let out = self
            .engine_sh(r#"nft -j list chain inet "$1" verdict"#, &[&table])
            .await?;
        if out.code != 0 {
            // No table (yet): nothing counted.
            return Ok(vec![]);
        }
        let mut counts = egress::nft_drop_counters(&out.stdout);
        // Its denies of members of its own tree, on the bridge (when it has
        // any): their counters follow, as their labels do.
        if k.drop_labels.len() > counts.len() {
            let tree = self
                .engine_sh(r#"nft -j list chain bridge "$1" tree"#, &[&table])
                .await?;
            if tree.code == 0 {
                counts.extend(egress::nft_drop_counters(&tree.stdout));
            }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Ok(egress::merge_events(egress::stamp_counted(
            name,
            egress::drop_events(&k.drop_labels, &counts),
            now,
        )))
    }

    /// Removes container `name`'s rules from the engine host and forgets
    /// its policy (the container is gone).
    pub(crate) async fn unpolice(&self, name: &str) {
        if let Some(t) = kept(name).and_then(|k| k.table)
            && let family = self.engine_bridge_family().await
            && let Err(e) = self
                .engine_apply(&egress::engine_remove_for(&t, family))
                .await
        {
            tracing::warn!(container = name, error = %e, "could not remove a deleted container's egress rules");
        }
        if file_name_ok(name)
            && let Err(e) = self.engine_sh(REMOVE_RESOLV, &[name]).await
        {
            tracing::debug!(container = name, error = %e, "could not remove a deleted container's resolv.conf");
        }
        let tree = tree_of(name);
        egress::forget_counted(name);
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
        let running = matches!(
            Self::status_from(i),
            crate::Status::Running | crate::Status::Paused
        );
        let configured = rules_mac(
            i.network_settings
                .as_ref()
                .and_then(|n| n.networks.as_ref())
                .and_then(|n| n.values().next())
                .and_then(|e| e.mac_address.as_deref()),
            labelled.as_deref(),
            running,
        );
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
        self.fix_gvisor_dns(name).await;
        Ok(())
    }

    /// Gives a gVisor container a resolver that answers.
    ///
    /// On a user-defined network (every tree's, under the secure default)
    /// Docker points the container at its embedded resolver, 127.0.0.11, which
    /// gVisor's network stack cannot reach: every lookup by name is refused,
    /// so nothing in the Space can `git clone` or install. The engine host's
    /// own resolvers are reachable (the egress rules allow them on port 53),
    /// so the container's resolv.conf is rewritten to name them. Best effort:
    /// a container that keeps the embedded resolver is as it was before.
    async fn fix_gvisor_dns(&self, name: &str) {
        let runtime = self
            .docker
            .inspect_container(name, None)
            .await
            .ok()
            .and_then(|i| i.host_config)
            .and_then(|h| h.runtime)
            .unwrap_or_default();
        if runtime != "runsc" {
            return;
        }
        let Ok(current) = self.exec_capture(name, &["cat", "/etc/resolv.conf"]).await else {
            return;
        };
        if !uses_embedded_resolver(&current) {
            return;
        }
        let Ok(probe) = self
            .engine_sh(
                r#"for f in /etc/resolv.conf /run/systemd/resolve/resolv.conf; do
  [ -r "$f" ] && sed -n 's/^nameserver[[:space:]]*\([0-9a-fA-F:.]*\).*/dns=\1/p' "$f"
done"#,
                &[],
            )
            .await
        else {
            return;
        };
        let resolvers: Vec<IpAddr> = probe
            .stdout
            .lines()
            .filter_map(|l| l.strip_prefix("dns=").and_then(|v| v.trim().parse().ok()))
            .filter(|ip: &IpAddr| !ip.is_loopback())
            .collect();
        let Some(text) = resolv_conf_for(&resolvers) else {
            tracing::warn!(
                container = name,
                "no engine resolver to give a gVisor container"
            );
            return;
        };
        let write = format!(
            "printf '%s' '{}' > /etc/resolv.conf",
            text.replace('\'', "")
        );
        if let Err(e) = self
            .exec_capture_as(name, &["sh", "-c", &write], Some("0"))
            .await
        {
            tracing::warn!(
                container = name,
                "could not set the container's resolver: {e}"
            );
        }
    }
}

/// Whether a resolv.conf names only Docker's embedded resolver.
pub(crate) fn uses_embedded_resolver(resolv_conf: &str) -> bool {
    let names: Vec<&str> = resolv_conf
        .lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .map(str::trim)
        .collect();
    !names.is_empty() && names.iter().all(|n| *n == "127.0.0.11")
}

/// The resolv.conf that names `resolvers` (none: nothing to write).
pub(crate) fn resolv_conf_for(resolvers: &[IpAddr]) -> Option<String> {
    if resolvers.is_empty() {
        return None;
    }
    let mut out = String::new();
    for r in resolvers.iter().take(3) {
        out.push_str(&format!("nameserver {r}\n"));
    }
    out.push_str("options ndots:0\n");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gvisor_container_is_given_the_engine_hosts_resolvers() {
        let embedded = "# Generated by Docker\nnameserver 127.0.0.11\noptions ndots:0\n";
        assert!(uses_embedded_resolver(embedded));
        assert!(!uses_embedded_resolver("nameserver 192.168.5.1\n"));
        assert!(!uses_embedded_resolver(
            "nameserver 127.0.0.11\nnameserver 1.1.1.1\n"
        ));
        assert!(!uses_embedded_resolver(""));
        let ips: Vec<IpAddr> = vec!["192.168.5.1".parse().unwrap(), "1.1.1.1".parse().unwrap()];
        assert_eq!(
            resolv_conf_for(&ips).as_deref(),
            Some("nameserver 192.168.5.1\nnameserver 1.1.1.1\noptions ndots:0\n")
        );
        assert!(resolv_conf_for(&[]).is_none());
    }

    #[test]
    fn the_probe_and_apply_scripts_take_their_arguments_positionally() {
        // Nothing of the caller's is pasted into the scripts: names and
        // the ruleset arrive as $1/$2.
        assert!(PROBE.contains(r#"br="$1"; pid="$2""#));
        assert!(APPLY.contains(r#""$1""#));
    }

    #[test]
    fn one_network_per_tree() {
        assert_eq!(tree_alias("worker_1", "local:Swarm"), "worker-1.swarm");
        assert_eq!(
            tree_alias("swarm", "swarm"),
            "swarm.swarm",
            "a top-level Space"
        );
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
    fn a_stopped_container_is_keyed_by_its_labelled_mac() {
        let label = Some("02:AA:BB:CC:DD:EE");
        // Running: only what the endpoint reports counts.
        assert_eq!(
            rules_mac(Some("02:aa:bb:cc:dd:ee"), label, true).as_deref(),
            Some("02:aa:bb:cc:dd:ee")
        );
        assert_eq!(rules_mac(None, label, true), None);
        assert_eq!(rules_mac(Some(""), label, true), None);
        // Stopped, on an engine that reports no MAC for it (Docker 29): the
        // label pinned at creation stands in, so it can start again.
        assert_eq!(
            rules_mac(Some(""), label, false).as_deref(),
            Some("02:aa:bb:cc:dd:ee")
        );
        assert_eq!(
            rules_mac(None, label, false).as_deref(),
            Some("02:aa:bb:cc:dd:ee")
        );
        // Nothing labelled: nothing to key by (recreate it).
        assert_eq!(rules_mac(None, None, false), None);
        // The label alone never makes a stopped container look recreated.
        let mac = rules_mac(None, label, false);
        assert!(!must_recreate(
            None,
            &["bridge".into()],
            Some("02:aa:bb:cc:dd:ee"),
            mac.as_deref(),
            true
        ));
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
    fn a_guest_is_given_exactly_the_servers_the_rules_allow_on_port_53() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let tree = [Net::parse("172.18.0.0/16").unwrap()];
        // The engine host's resolver is a local stub (Lima, systemd-resolved
        // without upstream, dnsmasq): the guest cannot reach it, so it is
        // given the public ones, and those are what is allowed.
        let c = context_from(
            "dns=127.0.0.53\naddr=10.0.1.5/24\n",
            &[ip("172.18.0.1")],
            &tree,
            &[],
            &[4100],
        );
        assert_eq!(c.resolvers, [ip("1.1.1.1"), ip("8.8.8.8")]);
        let r = crate::network::compile_mode(
            &NetworkSettings::internet(),
            &c,
            crate::network::Mode::Secure,
        )
        .unwrap();
        for s in &c.resolvers {
            assert!(r.allows_port(*s, crate::egress::Proto::Udp, 53));
            assert!(r.allows_port(*s, crate::egress::Proto::Tcp, 53));
        }
        // The engine host's other addresses and the bridge's gateway stay
        // closed on port 53 as on every other port.
        assert!(!r.allows_port(ip("10.0.1.5"), crate::egress::Proto::Udp, 53));
        assert!(!r.allows_port(ip("172.18.0.1"), crate::egress::Proto::Udp, 53));
        // A host that is its own resolver (Colima's VM, dnsmasq on its LAN
        // address): the guest asks it on port 53 and nothing else of it.
        let c = context_from("dns=10.0.1.5\naddr=10.0.1.5/24\n", &[], &tree, &[], &[]);
        assert_eq!(c.resolvers, [ip("10.0.1.5")]);
        let r = crate::network::compile_mode(
            &NetworkSettings::internet(),
            &c,
            crate::network::Mode::Secure,
        )
        .unwrap();
        assert!(r.allows_port(ip("10.0.1.5"), crate::egress::Proto::Udp, 53));
        assert!(!r.allows_port(ip("10.0.1.5"), crate::egress::Proto::Tcp, 22));
        assert!(!r.allows_port(ip("10.0.1.5"), crate::egress::Proto::Tcp, 4100));
        // A corporate resolver on the local network is allowed on 53 only.
        let c = context_from("dns=10.20.0.53\n", &[], &tree, &[], &[]);
        let r = crate::network::compile_mode(
            &NetworkSettings::internet(),
            &c,
            crate::network::Mode::Secure,
        )
        .unwrap();
        assert!(r.allows_port(ip("10.20.0.53"), crate::egress::Proto::Udp, 53));
        assert!(!r.allows_port(ip("10.20.0.53"), crate::egress::Proto::Tcp, 22));
        assert!(!r.allows_port(ip("10.20.0.54"), crate::egress::Proto::Udp, 53));
    }

    #[test]
    fn a_space_without_network_has_no_server_and_no_dns_rule() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let c = context_from("dns=10.0.0.2\n", &[], &[], &[], &[]);
        let none = NetworkSettings {
            preset: NetworkPreset::None,
            ..NetworkSettings::internet()
        };
        assert!(guest_servers(Some(&none), &c).is_empty());
        assert_eq!(
            guest_servers(Some(&NetworkSettings::internet()), &c),
            [ip("10.0.0.2")]
        );
        assert_eq!(guest_servers(None, &c), [ip("10.0.0.2")]);
        let r = crate::network::compile_mode(&none, &c, crate::network::Mode::Secure).unwrap();
        assert!(!r.allows_port(ip("10.0.0.2"), crate::egress::Proto::Udp, 53));
        assert!(!r.allows(ip("1.1.1.1")));
    }

    #[test]
    fn the_resolv_conf_mount_is_recognised_and_its_name_is_safe() {
        assert_eq!(
            resolv_bind("web"),
            "/var/lib/cua/resolv/web.conf:/etc/resolv.conf"
        );
        assert!(has_resolv_bind(Some(&vec![
            "/a:/b".into(),
            resolv_bind("web")
        ])));
        assert!(!has_resolv_bind(Some(&vec!["/a:/etc/resolv.conf".into()])));
        assert!(!has_resolv_bind(None));
        assert!(file_name_ok("cua-sb_1.x"));
        for bad in ["", ".hidden", "a/b", "a b", "..", "a;rm"] {
            assert!(!file_name_ok(bad), "{bad:?}");
        }
        // Nothing of the caller's is pasted into the scripts.
        assert!(WRITE_RESOLV.contains(r#""$1""#) && WRITE_RESOLV.contains(r#"$2.conf"#));
        assert!(REMOVE_RESOLV.contains(r#"$1.conf"#));
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
