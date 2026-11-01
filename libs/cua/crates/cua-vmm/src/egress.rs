//! Egress policy of a sandbox, enforced by the host outside the guest.
//!
//! A policy ([`EgressPolicy`]) has two lists of IPv4/IPv6 addresses and
//! CIDRs (the word `all` is `0.0.0.0/0` and `::/0`). Everything is allowed
//! unless a rule says otherwise:
//!
//! - every `deny` entry blocks that destination;
//! - every `allow` entry is an exception that wins over `deny` (`deny:
//!   ["all"]` with `allow: ["1.1.1.1"]` leaves only 1.1.1.1 reachable);
//! - an `allow` list with no `deny` blocks nothing;
//! - both lists empty: no policy.
//!
//! Rules are by address: a hostname is refused. The destination is the
//! address the guest sends to. DNS therefore works only while the
//! resolver the guest uses is not blocked (or is allowed); on VM kinds that
//! is the virtual network's own resolver (`10.0.2.3` under QEMU, the
//! gateway `172.16.0.1` in a Firecracker microVM).
//!
//! Three things are never filtered: loopback inside the guest, packets
//! that answer a connection made *to* the guest (cua-spacesd, published
//! ports) and IPv6 neighbor discovery.
//!
//! **Where it is enforced.** Always on the host, at a point the guest
//! (root included) cannot reach ([`support`] says where for each backend):
//!
//! - containers (runc and gVisor): nftables in the container engine's own
//!   network namespace, on the Space's bridge port ([`engine_ruleset`]);
//! - Firecracker microVMs and QEMU VMs on Linux: iptables in the network
//!   namespace `pasta` creates around the VMM ([`ns_restore`]).
//!
//! Backends without such a point refuse a policy
//! ([`crate::VmmError::Unsupported`]) instead of filtering inside the guest.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::BackendKind;

/// Which destinations a sandbox may open connections to (see the module
/// docs).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// Destinations that stay reachable even where `deny` blocks them.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Destinations that are blocked (`all` blocks everything).
    #[serde(default)]
    pub deny: Vec<String>,
}

/// A policy entry that is not an address, a CIDR or `all`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PolicyError(pub String);

impl From<PolicyError> for crate::VmmError {
    fn from(e: PolicyError) -> Self {
        crate::VmmError::Invalid(e.0)
    }
}

impl EgressPolicy {
    /// No allow and no deny entries: no policy.
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Checks the entries: addresses, CIDRs and `all` only (a hostname is
    /// refused).
    pub fn parse(&self) -> Result<Rules, PolicyError> {
        Ok(Rules {
            allow: parse_list(&self.allow)?,
            deny: parse_list(&self.deny)?,
        })
    }
}

/// One destination: a network of one address family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Net {
    /// IPv4 network address and prefix length.
    V4(Ipv4Addr, u8),
    /// IPv6 network address and prefix length.
    V6(Ipv6Addr, u8),
}

impl Net {
    /// `0.0.0.0/0` or `::/0`.
    pub fn is_all(&self) -> bool {
        matches!(self, Net::V4(_, 0) | Net::V6(_, 0))
    }

    /// An IPv6 network.
    pub fn is_v6(&self) -> bool {
        matches!(self, Net::V6(..))
    }

    /// `10.0.0.0/8`, `fd00::/8`.
    pub fn cidr(&self) -> String {
        match self {
            Net::V4(a, p) => format!("{a}/{p}"),
            Net::V6(a, p) => format!("{a}/{p}"),
        }
    }

    /// Whether `ip` is in this network.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self, ip) {
            (Net::V4(net, p), IpAddr::V4(a)) => {
                let mask = if *p == 0 {
                    0
                } else {
                    u32::MAX << (32 - u32::from(*p))
                };
                u32::from(a) & mask == u32::from(*net)
            }
            (Net::V6(net, p), IpAddr::V6(a)) => {
                let mask = if *p == 0 {
                    0
                } else {
                    u128::MAX << (128 - u32::from(*p))
                };
                u128::from(a) & mask == u128::from(*net)
            }
            _ => false,
        }
    }
}

/// A checked policy: entries parsed, host bits cleared, duplicates dropped
/// (first spelling wins, order kept).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// Allowed destinations.
    pub allow: Vec<Net>,
    /// Blocked destinations.
    pub deny: Vec<Net>,
}

/// Most entries in one list.
pub const MAX_ENTRIES: usize = 1024;

/// Parses one entry: `all`, an address, or `address/prefix`.
fn parse_entry(entry: &str) -> Result<Vec<Net>, PolicyError> {
    let e = entry.trim();
    if e.eq_ignore_ascii_case("all") {
        return Ok(vec![
            Net::V4(Ipv4Addr::UNSPECIFIED, 0),
            Net::V6(Ipv6Addr::UNSPECIFIED, 0),
        ]);
    }
    let (addr, prefix) = match e.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (e, None),
    };
    let ip: IpAddr = addr.parse().map_err(|_| {
        let hostname = !e.is_empty()
            && e.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '*'));
        PolicyError(if hostname {
            format!("egress entry {entry:?}: rules are by IP/CIDR, not by hostname")
        } else {
            format!(
                "egress entry {entry:?}: expected an IPv4/IPv6 address, an address/prefix \
                 (CIDR), or \"all\""
            )
        })
    })?;
    let bits = if ip.is_ipv4() { 32 } else { 128 };
    let prefix: u8 = match prefix {
        None => bits,
        Some(p) => p.parse::<u8>().ok().filter(|p| *p <= bits).ok_or_else(|| {
            PolicyError(format!(
                "egress entry {entry:?}: the prefix is a number from 0 to {bits}"
            ))
        })?,
    };
    let net = match ip {
        IpAddr::V4(a) => Net::V4(a, prefix),
        IpAddr::V6(a) => Net::V6(a, prefix),
    };
    // Host bits cleared: the network itself.
    Ok(vec![match net {
        Net::V4(a, p) => {
            let mask = if p == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(p))
            };
            Net::V4(Ipv4Addr::from(u32::from(a) & mask), p)
        }
        Net::V6(a, p) => {
            let mask = if p == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(p))
            };
            Net::V6(Ipv6Addr::from(u128::from(a) & mask), p)
        }
    }])
}

fn parse_list(list: &[String]) -> Result<Vec<Net>, PolicyError> {
    if list.len() > MAX_ENTRIES {
        return Err(PolicyError(format!(
            "{} egress entries in one list: at most {MAX_ENTRIES}",
            list.len()
        )));
    }
    let mut out: Vec<Net> = Vec::new();
    for entry in list {
        for net in parse_entry(entry)? {
            if !out.contains(&net) {
                out.push(net);
            }
        }
    }
    Ok(out)
}

impl Rules {
    /// Whether nothing is filtered (no entries).
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// The rules in the order they are evaluated, as (net, accept): every
    /// `allow` entry first (an exception wins), then every `deny` entry.
    pub fn ordered(&self) -> impl Iterator<Item = (&Net, bool)> {
        self.allow
            .iter()
            .map(|n| (n, true))
            .chain(self.deny.iter().map(|n| (n, false)))
    }

    /// Whether any destination is blocked (an allow list alone blocks
    /// nothing).
    pub fn blocks_anything(&self) -> bool {
        !self.deny.is_empty()
    }

    /// Whether the guest may reach `ip` under these rules.
    pub fn allows(&self, ip: IpAddr) -> bool {
        self.ordered()
            .find(|(n, _)| n.contains(ip))
            .is_none_or(|(_, accept)| accept)
    }
}

/// Where a policy is enforced, as [`crate::Runtime::set_egress`] and a
/// started instance report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    /// By the host, outside the guest: nothing in the guest (root
    /// included) can change it.
    Host,
}

impl Enforcement {
    /// `host`.
    pub fn as_str(self) -> &'static str {
        match self {
            Enforcement::Host => "host",
        }
    }
}

/// What a backend does with a policy on this host OS ([`support`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// The host enforces it, at `point`.
    Host {
        /// Where, for people.
        point: &'static str,
    },
    /// Refused: `why` says what and why (`an egress policy (...)`), as
    /// [`crate::VmmError::Unsupported`]'s operation.
    Unsupported {
        /// For people.
        why: &'static str,
    },
}

/// The engine-host point of the container backends.
const ENGINE_POINT: &str = "nftables in the container engine's own network namespace, on the \
     Space's bridge port (outside the container, runc and gVisor alike)";
/// The namespace point of the VMs.
const NS_POINT: &str = "iptables in the network namespace pasta creates around the VMM process";

/// Where `backend` enforces an egress policy on the OS this was built for,
/// or why it refuses one. Every backend has an answer: the match has no
/// wildcard, so a new backend does not build until it says.
pub fn support(backend: BackendKind) -> Support {
    support_on(backend, std::env::consts::OS)
}

/// [`support`] for host OS `os` (`linux`, `macos`, `windows`).
pub fn support_on(backend: BackendKind, os: &str) -> Support {
    let linux = os == "linux";
    match backend {
        BackendKind::Container | BackendKind::Managed => Support::Host {
            point: ENGINE_POINT,
        },
        BackendKind::Firecracker if linux => Support::Host { point: NS_POINT },
        BackendKind::Firecracker => Support::Unsupported {
            why: "an egress policy (Firecracker microVMs run on Linux only)",
        },
        BackendKind::Qemu if linux => Support::Host { point: NS_POINT },
        BackendKind::Qemu => Support::Unsupported {
            why: "an egress policy (QEMU's user-mode network makes the guest's connections from \
                  the QEMU process itself, and this OS has no way to filter one process's \
                  connections without root: pf on macOS, WFP on Windows)",
        },
        BackendKind::Lume => Support::Unsupported {
            why: "an egress policy (macOS VMs share Apple's vmnet NAT, which only pf can filter, \
                  and pf needs root)",
        },
        BackendKind::Containerization => Support::Unsupported {
            why: "an egress policy (Apple's container network, vmnet, can only be filtered with \
                  pf, which needs root)",
        },
        BackendKind::Vz => Support::Unsupported {
            why: "an egress policy (a macOS microVM's network goes through vmnet or its userspace \
                  proxy, and neither filters by destination for one VM without root)",
        },
        BackendKind::QemuDocker => Support::Unsupported {
            why: "an egress policy (no runtime of this build starts QEMU inside a container)",
        },
    }
}

/// Every backend, for drift guards over [`support`].
pub const BACKENDS: [BackendKind; 8] = [
    BackendKind::Lume,
    BackendKind::Qemu,
    BackendKind::QemuDocker,
    BackendKind::Container,
    BackendKind::Managed,
    BackendKind::Firecracker,
    BackendKind::Containerization,
    BackendKind::Vz,
];

/// [`crate::VmmError::Unsupported`] for `backend`, or `None` when it
/// enforces policies here.
pub fn refusal(backend: BackendKind) -> Option<crate::VmmError> {
    match support(backend) {
        Support::Host { .. } => None,
        Support::Unsupported { why } => Some(crate::VmmError::Unsupported {
            backend: backend.as_str(),
            op: why,
        }),
    }
}

/// Checks `policy` for `backend` before anything is created: its entries,
/// and that this backend enforces policies here. Returns the rules
/// (empty for no policy, which every backend takes).
pub fn check(backend: BackendKind, policy: &EgressPolicy) -> crate::Result<Rules> {
    let rules = policy.parse()?;
    if rules.is_empty() {
        return Ok(rules);
    }
    match refusal(backend) {
        Some(e) => Err(e),
        None => Ok(rules),
    }
}

// ------------------------------------------------------- container engine

/// The nft table names of one sandbox, from a key that identifies it on
/// its engine (cua home and name): `cua_egress_<12 hex>`.
pub fn engine_table(key: &str) -> String {
    format!("cua_egress_{}", &hex(&Sha256::digest(key.as_bytes()))[..12])
}

/// The MAC address a container sandbox gets at create, from the same key:
/// locally administered, unicast (`02:...`). Known before the container
/// starts, so its rules can be in place before its first instruction.
pub fn engine_mac(key: &str) -> String {
    let d = Sha256::digest(format!("mac:{key}").as_bytes());
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        d[0], d[1], d[2], d[3], d[4]
    )
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A MAC address as `aa:bb:cc:dd:ee:ff` (lowercase), or `None`.
pub fn normalize_mac(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    (parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit())))
    .then(|| parts.join(":").to_ascii_lowercase())
}

/// A network interface name nft can take verbatim.
pub fn valid_ifname(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 16
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The port a running container sits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgePort {
    /// The host end of its veth (a port of the bridge).
    pub veth: String,
    /// Its IPv4 address on the bridge.
    pub ipv4: Option<Ipv4Addr>,
}

/// Where one container sandbox's rules apply on its engine host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineTarget {
    /// [`engine_table`].
    pub table: String,
    /// The bridge the container's network is (`docker0`).
    pub bridge: String,
    /// The bridge's own MAC address (the container's gateway).
    pub gateway_mac: String,
    /// The container's MAC ([`engine_mac`]).
    pub mac: String,
    /// Its bridge port, once it runs (`None` before its first start).
    pub port: Option<BridgePort>,
}

/// The nft input that removes one sandbox's tables in one transaction
/// (whether or not they exist).
pub fn engine_remove(table: &str) -> String {
    format!(
        "add table bridge {table}\ndelete table bridge {table}\n\
         add table inet {table}\ndelete table inet {table}\n"
    )
}

/// The nft input that replaces one container sandbox's rules on its
/// engine host in one transaction (empty rules: [`engine_remove`]):
///
/// - `bridge` table: frames from the sandbox's MAC go to the gateway, to
///   broadcast or to IPv6 multicast only, so every packet it sends enters
///   the host's IP stack (no direct path to a sibling container); once it
///   runs, frames from its bridge port with another source MAC, or ARP for
///   another address, are dropped (it cannot pass for another container);
/// - `inet` table, before NAT and filtering: packets from that MAC on the
///   bridge go through the policy (replies to connections made to the
///   sandbox and IPv6 neighbor discovery pass; then allow, then deny).
pub fn engine_ruleset(t: &EngineTarget, rules: &Rules) -> String {
    let mut out = engine_remove(&t.table);
    if !rules.blocks_anything() {
        return out;
    }
    let table = &t.table;
    let mac = &t.mac;
    let mut pre = String::new();
    if let Some(port) = &t.port {
        pre.push_str(&format!(
            "    iifname \"{}\" ether saddr != {mac} drop\n",
            port.veth
        ));
        if let Some(ip) = port.ipv4 {
            pre.push_str(&format!(
                "    iifname \"{}\" ether type arp arp saddr ip != {ip} drop\n",
                port.veth
            ));
        }
    }
    out.push_str(&format!(
        "table bridge {table} {{\n\
         \x20 chain pre {{\n\
         \x20   type filter hook prerouting priority -200; policy accept;\n\
         {pre}\
         \x20   ether saddr {mac} jump l2\n\
         \x20 }}\n\
         \x20 chain l2 {{\n\
         \x20   ether daddr {{ {gw}, ff:ff:ff:ff:ff:ff }} return\n\
         \x20   ether daddr & ff:ff:00:00:00:00 == 33:33:00:00:00:00 return\n\
         \x20   drop\n\
         \x20 }}\n\
         }}\n",
        gw = t.gateway_mac,
    ));
    out.push_str(&format!(
        "table inet {table} {{\n\
         \x20 chain pre {{\n\
         \x20   type filter hook prerouting priority -150; policy accept;\n\
         \x20   iifname \"{bridge}\" ether saddr {mac} jump verdict\n\
         \x20 }}\n\
         \x20 chain verdict {{\n\
         \x20   ct direction reply accept\n\
         \x20   icmpv6 type {{ nd-router-solicit, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert }} accept\n",
        bridge = t.bridge,
    ));
    for (net, accept) in rules.ordered() {
        out.push_str(&nft_rule(net, accept));
    }
    out.push_str("  }\n}\n");
    out
}

fn nft_rule(net: &Net, accept: bool) -> String {
    let verdict = if accept { "accept" } else { "drop" };
    match (net.is_all(), net.is_v6()) {
        (true, false) => format!("    meta nfproto ipv4 {verdict}\n"),
        (true, true) => format!("    meta nfproto ipv6 {verdict}\n"),
        (false, false) => format!("    ip daddr {} {verdict}\n", net.cidr()),
        (false, true) => format!("    ip6 daddr {} {verdict}\n", net.cidr()),
    }
}

// ------------------------------------------------------ VMM namespaces

/// The iptables chain of a VMM's network namespace.
pub const NS_CHAIN: &str = "CUA_EGRESS";

/// The built-in resolver of a VM's virtual network: the address the guest
/// sends DNS to (`guest`), and the one its queries leave the namespace for
/// (`forward`, pasta's DNS forward). Queries to `forward` get the policy's
/// verdict for `guest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolver {
    /// What the guest sees (`10.0.2.3` under QEMU).
    pub guest: Ipv4Addr,
    /// Where its queries go in the namespace.
    pub forward: Ipv4Addr,
}

/// The `iptables-restore` (`v6`: `ip6tables-restore`) input that fills
/// [`NS_CHAIN`] with `rules`, for `--noflush` (declaring the chain empties
/// it; nothing else in the table changes). Destinations match the
/// connection's original destination (`--ctorigdst`), so an address the
/// namespace rewrites (the microVM's DNS) is judged as the guest sent it.
/// The jumps to the chain are [`ns_jumps`].
pub fn ns_restore(rules: &Rules, v6: bool, resolver: Option<Resolver>) -> String {
    format!(
        "*filter\n:{NS_CHAIN} - [0:0]\n{}COMMIT\n",
        ns_rules(rules, v6, resolver)
    )
}

/// The `-A` lines of [`NS_CHAIN`] for `rules` (see [`ns_restore`]).
pub fn ns_rules(rules: &Rules, v6: bool, resolver: Option<Resolver>) -> String {
    let mut out = String::new();
    if !rules.is_empty() {
        out.push_str(&format!(
            "-A {NS_CHAIN} -o lo -j ACCEPT\n-A {NS_CHAIN} -m conntrack --ctdir REPLY -j ACCEPT\n"
        ));
        if v6 {
            for t in [133, 134, 135, 136] {
                out.push_str(&format!(
                    "-A {NS_CHAIN} -p ipv6-icmp -m icmp6 --icmpv6-type {t} -j ACCEPT\n"
                ));
            }
        } else if let Some(r) = resolver {
            let verdict = if rules.allows(IpAddr::V4(r.guest)) {
                "ACCEPT"
            } else {
                "DROP"
            };
            for proto in ["udp", "tcp"] {
                out.push_str(&format!(
                    "-A {NS_CHAIN} -d {}/32 -p {proto} --dport 53 -j {verdict}\n",
                    r.forward
                ));
            }
        }
        for (net, accept) in rules.ordered().filter(|(n, _)| n.is_v6() == v6) {
            out.push_str(&format!(
                "-A {NS_CHAIN} -m conntrack --ctorigdst {} -j {}\n",
                net.cidr(),
                if accept { "ACCEPT" } else { "DROP" }
            ));
        }
    }
    out
}

/// Which traffic of a VMM's namespace is the guest's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NsHook {
    /// Routed from the guest's tap device (Firecracker): `FORWARD` and
    /// `INPUT` from it.
    Tap(&'static str),
    /// Opened by the VMM process itself (QEMU's user-mode network):
    /// `OUTPUT`.
    Output,
}

/// The rules (for an `iptables-restore` filter section) that send the
/// guest's traffic through [`NS_CHAIN`].
pub fn ns_jumps(hook: NsHook) -> String {
    match hook {
        NsHook::Tap(dev) => {
            format!("-A FORWARD -i {dev} -j {NS_CHAIN}\n-A INPUT -i {dev} -j {NS_CHAIN}\n")
        }
        NsHook::Output => format!("-A OUTPUT -j {NS_CHAIN}\n"),
    }
}

/// A shell script, run in the namespace, that installs `rules` for both
/// families: the chain refilled with `iptables-restore --noflush` (and
/// `ip6tables-restore`), and the jumps to it added when missing. Exits
/// non-zero, with the tool's words, when the namespace refuses.
pub fn ns_script(rules: &Rules, hook: NsHook, resolver: Option<Resolver>) -> String {
    let jumps = |bin: &str| -> String {
        let lines: Vec<(&str, String)> = match hook {
            NsHook::Tap(dev) => vec![
                ("FORWARD", format!(" -i {dev}")),
                ("INPUT", format!(" -i {dev}")),
            ],
            NsHook::Output => vec![("OUTPUT", String::new())],
        };
        lines
            .iter()
            .map(|(chain, m)| {
                format!(
                    "{bin} -C {chain}{m} -j {NS_CHAIN} 2>/dev/null || {bin} -I {chain} 1{m} -j {NS_CHAIN}\n"
                )
            })
            .collect()
    };
    let mut s = String::from("set -e\n");
    s.push_str(&format!(
        "iptables-restore --noflush <<'CUA_EGRESS_EOF'\n{}CUA_EGRESS_EOF\n",
        ns_restore(rules, false, resolver)
    ));
    s.push_str(&jumps("iptables"));
    // IPv6: only where the namespace has it (pasta gives it IPv6 when the
    // host has a route); without ip6tables there is no IPv6 to filter.
    s.push_str(
        "if command -v ip6tables-restore >/dev/null 2>&1 && [ -e /proc/net/if_inet6 ]; then\n",
    );
    s.push_str(&format!(
        "ip6tables-restore --noflush <<'CUA_EGRESS_EOF'\n{}CUA_EGRESS_EOF\n",
        ns_restore(rules, true, None)
    ));
    s.push_str(&jumps("ip6tables"));
    s.push_str("fi\n");
    s
}

/// Runs `script` in the user and network namespaces of process `pid` (a
/// VMM under pasta), as the user that owns them.
pub async fn ns_apply(pid: u32, script: &str) -> crate::Result<()> {
    let out = tokio::process::Command::new("nsenter")
        .args(["-t", &pid.to_string(), "-U", "-n", "--preserve-credentials"])
        .args(["sh", "-c", script])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| crate::VmmError::other(format!("nsenter: {e}")))?;
    if !out.status.success() {
        return Err(crate::VmmError::other(format!(
            "the VM's network namespace refused the egress rules: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(allow: &[&str], deny: &[&str]) -> EgressPolicy {
        EgressPolicy {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn rules(allow: &[&str], deny: &[&str]) -> Rules {
        policy(allow, deny).parse().unwrap()
    }

    #[test]
    fn entries_are_normalised_and_deduplicated() {
        let r = rules(
            &[],
            &["10.1.2.3/8", "10.0.0.0/8", "ALL", "all", "::1", "0.0.0.0/0"],
        );
        assert_eq!(
            r.deny,
            [
                Net::V4(Ipv4Addr::new(10, 0, 0, 0), 8),
                Net::V4(Ipv4Addr::UNSPECIFIED, 0),
                Net::V6(Ipv6Addr::UNSPECIFIED, 0),
                Net::V6(Ipv6Addr::LOCALHOST, 128),
            ]
        );
        assert_eq!(
            rules(&[], &["2001:db8::1/32"]).deny,
            [Net::V6("2001:db8::".parse().unwrap(), 32)]
        );
        assert_eq!(
            rules(&[], &["1.2.3.4/0"]).deny,
            [Net::V4(Ipv4Addr::UNSPECIFIED, 0)]
        );
    }

    #[test]
    fn bad_entries_are_refused() {
        let refused = |entry: &str| policy(&[], &[entry]).parse().unwrap_err().0;
        for host in ["example.com", "*.example.com", "localhost", "db_1"] {
            assert!(refused(host).contains("by IP/CIDR"), "{host}");
        }
        for bad in [
            "10.0.0.0/33",
            "::/129",
            "10.0.0.0/x",
            "10.0.0.0/",
            "1.2.3",
            "",
            " ",
            "1.2.3.4;rm",
            "1.2.3.4\" accept",
        ] {
            assert!(refused(bad).contains("egress entry"), "{bad:?}");
        }
        let many: Vec<String> = (0..=MAX_ENTRIES)
            .map(|i| format!("10.{}.{}.0/24", i / 256, i % 256))
            .collect();
        let e = EgressPolicy {
            allow: vec![],
            deny: many,
        }
        .parse()
        .unwrap_err();
        assert!(e.0.contains("at most 1024"), "{e}");
        // A VmmError says it is the caller's input.
        let v: crate::VmmError = e.into();
        assert!(matches!(v, crate::VmmError::Invalid(_)));
    }

    #[test]
    fn allow_wins_over_deny_and_the_default_is_allowed() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let r = rules(&["1.1.1.1", "10.1.0.0/16"], &["all"]);
        assert!(r.allows(ip("1.1.1.1")));
        assert!(r.allows(ip("10.1.200.3")));
        assert!(!r.allows(ip("10.2.0.1")));
        assert!(!r.allows(ip("8.8.8.8")));
        assert!(!r.allows(ip("2606:4700::1111")));
        let r = rules(&[], &["10.0.0.0/8"]);
        assert!(r.allows(ip("8.8.8.8")));
        assert!(!r.allows(ip("10.0.2.3")));
        assert!(rules(&["1.1.1.1"], &[]).allows(ip("9.9.9.9")));
        assert!(Rules::default().allows(ip("9.9.9.9")));
    }

    fn target(port: bool) -> EngineTarget {
        EngineTarget {
            table: engine_table("home:web"),
            bridge: "docker0".into(),
            gateway_mac: "0a:de:71:61:ef:e1".into(),
            mac: engine_mac("home:web"),
            port: port.then(|| BridgePort {
                veth: "veth1a2b3c4".into(),
                ipv4: Some(Ipv4Addr::new(172, 17, 0, 2)),
            }),
        }
    }

    #[test]
    fn engine_names_are_stable_and_distinct() {
        assert_eq!(engine_table("a:b"), engine_table("a:b"));
        assert_ne!(engine_table("a:b"), engine_table("a:c"));
        assert!(engine_table("a:b").starts_with("cua_egress_"));
        assert_eq!(engine_table("a:b").len(), "cua_egress_".len() + 12);
        let mac = engine_mac("a:b");
        assert_eq!(normalize_mac(&mac).as_deref(), Some(mac.as_str()));
        assert!(
            mac.starts_with("02:"),
            "locally administered unicast: {mac}"
        );
        assert_ne!(engine_mac("a:b"), engine_mac("a:c"));
        assert_eq!(
            normalize_mac("0A:DE:71:61:EF:E1").as_deref(),
            Some("0a:de:71:61:ef:e1")
        );
        assert_eq!(normalize_mac("0a:de:71:61:ef"), None);
        assert_eq!(normalize_mac("0a:de:71:61:ef:zz"), None);
        assert!(valid_ifname("veth1a2b3c4") && valid_ifname("docker0") && valid_ifname("br-1f2e"));
        assert!(!valid_ifname("") && !valid_ifname("a\"b") && !valid_ifname("x y"));
        assert!(!valid_ifname("sixteen-chars-xx"));
    }

    #[test]
    fn engine_ruleset_before_and_after_start() {
        let t = target(false);
        let table = &t.table;
        let mac = &t.mac;
        let r = rules(&["1.1.1.1", "2606:4700::1111"], &["all"]);
        let text = engine_ruleset(&t, &r);
        assert!(text.starts_with(&engine_remove(table)), "{text}");
        // Before the first start: keyed by MAC only.
        assert!(!text.contains("iifname \"veth"), "{text}");
        assert!(
            text.contains(&format!("ether saddr {mac} jump l2")),
            "{text}"
        );
        assert!(text.contains("ether daddr { 0a:de:71:61:ef:e1, ff:ff:ff:ff:ff:ff } return"));
        assert!(text.contains(&format!(
            "iifname \"docker0\" ether saddr {mac} jump verdict"
        )));
        // Replies and neighbor discovery first, then allow, then deny.
        let pos = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}\n{text}"))
        };
        assert!(pos("ct direction reply accept") < pos("ip daddr 1.1.1.1/32 accept"));
        assert!(pos("nd-neighbor-solicit") < pos("ip daddr 1.1.1.1/32 accept"));
        assert!(pos("ip daddr 1.1.1.1/32 accept") < pos("ip6 daddr 2606:4700::1111/128 accept"));
        assert!(pos("ip6 daddr 2606:4700::1111/128 accept") < pos("meta nfproto ipv4 drop"));
        assert!(pos("meta nfproto ipv4 drop") < pos("meta nfproto ipv6 drop"));
        // Running: its bridge port must carry its MAC and its address.
        let text = engine_ruleset(&target(true), &r);
        assert!(text.contains(&format!(
            "iifname \"veth1a2b3c4\" ether saddr != {mac} drop"
        )));
        assert!(
            text.contains("iifname \"veth1a2b3c4\" ether type arp arp saddr ip != 172.17.0.2 drop")
        );
        assert!(
            text.find("ether saddr != ").unwrap() < text.find("jump l2").unwrap(),
            "the anti-spoof rules come first: {text}"
        );
        // No policy: only the removal.
        assert_eq!(
            engine_ruleset(&target(true), &Rules::default()),
            engine_remove(table)
        );
        // An allow list alone blocks nothing: no rules at all.
        assert_eq!(
            engine_ruleset(&t, &rules(&["1.1.1.1"], &[])),
            engine_remove(table)
        );
    }

    #[test]
    fn ns_chain_text() {
        let r = rules(&["1.1.1.1", "2606:4700::1111"], &["all"]);
        let v4 = ns_restore(&r, false, None);
        assert_eq!(
            v4,
            "*filter\n:CUA_EGRESS - [0:0]\n-A CUA_EGRESS -o lo -j ACCEPT\n-A CUA_EGRESS -m conntrack --ctdir REPLY -j ACCEPT\n-A CUA_EGRESS -m conntrack --ctorigdst 1.1.1.1/32 -j ACCEPT\n-A CUA_EGRESS -m conntrack --ctorigdst 0.0.0.0/0 -j DROP\nCOMMIT\n"
        );
        let v6 = ns_restore(&r, true, None);
        assert!(v6.contains("--icmpv6-type 135 -j ACCEPT"));
        assert!(v6.contains(
            "-A CUA_EGRESS -m conntrack --ctorigdst 2606:4700::1111/128 -j ACCEPT\n-A CUA_EGRESS -m conntrack --ctorigdst ::/0 -j DROP\nCOMMIT\n"
        ));
        // No policy: an empty chain (the jumps stay; nothing is filtered).
        assert_eq!(
            ns_restore(&Rules::default(), false, None),
            "*filter\n:CUA_EGRESS - [0:0]\nCOMMIT\n"
        );
    }

    #[test]
    fn the_built_in_resolver_gets_the_verdict_of_its_guest_address() {
        let resolver = Resolver {
            guest: Ipv4Addr::new(10, 0, 2, 3),
            forward: Ipv4Addr::new(169, 254, 1, 53),
        };
        // deny all: the resolver is blocked like everything else...
        let text = ns_restore(&rules(&["1.1.1.1"], &["all"]), false, Some(resolver));
        assert!(
            text.contains("-d 169.254.1.53/32 -p udp --dport 53 -j DROP"),
            "{text}"
        );
        assert!(
            text.contains("-d 169.254.1.53/32 -p tcp --dport 53 -j DROP"),
            "{text}"
        );
        // ...unless its guest address is allowed...
        let text = ns_restore(&rules(&["10.0.2.3"], &["all"]), false, Some(resolver));
        assert!(
            text.contains("-d 169.254.1.53/32 -p udp --dport 53 -j ACCEPT"),
            "{text}"
        );
        // ...and it comes before the generic rules.
        assert!(text.find("--dport 53").unwrap() < text.find("--ctorigdst 0.0.0.0/0").unwrap());
        // A policy that does not cover it leaves DNS working.
        let text = ns_restore(&rules(&[], &["192.168.0.0/16"]), false, Some(resolver));
        assert!(text.contains("-p udp --dport 53 -j ACCEPT"), "{text}");
    }

    #[test]
    fn ns_script_installs_the_jumps_once() {
        let r = rules(&[], &["all"]);
        let s = ns_script(&r, NsHook::Tap("tap0"), None);
        assert!(s.contains("iptables-restore --noflush"), "{s}");
        assert!(s.contains(
            "iptables -C FORWARD -i tap0 -j CUA_EGRESS 2>/dev/null || iptables -I FORWARD 1 -i tap0 -j CUA_EGRESS"
        ), "{s}");
        assert!(s.contains("iptables -C INPUT -i tap0 -j CUA_EGRESS"), "{s}");
        assert!(s.contains("ip6tables-restore --noflush"), "{s}");
        let s = ns_script(&r, NsHook::Output, None);
        assert!(
            s.contains(
                "iptables -C OUTPUT -j CUA_EGRESS 2>/dev/null || iptables -I OUTPUT 1 -j CUA_EGRESS"
            ),
            "{s}"
        );
        assert_eq!(ns_jumps(NsHook::Output), "-A OUTPUT -j CUA_EGRESS\n");
        assert_eq!(
            ns_jumps(NsHook::Tap("tap0")),
            "-A FORWARD -i tap0 -j CUA_EGRESS\n-A INPUT -i tap0 -j CUA_EGRESS\n"
        );
    }

    /// The namespace script, run by a real `sh` against stub tools that
    /// log their arguments: what it feeds `iptables-restore`, and the
    /// jumps it adds (only when missing).
    #[cfg(unix)]
    #[test]
    fn ns_script_runs_against_the_tools() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls");
        for (name, target) in [("sh", "/bin/sh"), ("cat", "/bin/cat")] {
            std::os::unix::fs::symlink(target, dir.path().join(name)).unwrap();
        }
        // `-C` fails (no jump yet) for iptables; ip6tables has them.
        for (name, body) in [
            ("iptables-restore", "cat > \"$(dirname \"$0\")/v4\""),
            ("ip6tables-restore", "cat > \"$(dirname \"$0\")/v6\""),
            ("iptables", "[ \"$1\" = -C ] && exit 1; exit 0"),
            ("ip6tables", "exit 0"),
            ("dirname", "echo \"${1%/*}\""),
        ] {
            let path = dir.path().join(name);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\necho \"{name} $*\" >> \"{}\"\n{body}\n",
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let r = rules(&["1.1.1.1"], &["all"]);
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(ns_script(&r, NsHook::Tap("tap0"), None))
            .env("PATH", dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("iptables-restore --noflush"), "{calls}");
        assert!(
            calls.contains("iptables -I FORWARD 1 -i tap0 -j CUA_EGRESS"),
            "{calls}"
        );
        assert!(
            calls.contains("iptables -I INPUT 1 -i tap0 -j CUA_EGRESS"),
            "{calls}"
        );
        assert!(
            !calls.contains("ip6tables -I"),
            "present jumps are kept: {calls}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("v4")).unwrap(),
            ns_restore(&r, false, None)
        );
        if std::path::Path::new("/proc/net/if_inet6").exists() {
            assert_eq!(
                std::fs::read_to_string(dir.path().join("v6")).unwrap(),
                ns_restore(&r, true, None)
            );
        }
    }

    /// Every backend has an answer on every host OS, and the ones that
    /// enforce do it on the host: a new backend fails here (and does not
    /// build, since [`support_on`] matches without a wildcard) until it
    /// says where its policies are enforced or why it refuses them.
    #[test]
    fn every_backend_has_an_egress_answer() {
        for os in ["linux", "macos", "windows"] {
            for b in BACKENDS {
                match support_on(b, os) {
                    Support::Host { point } => assert!(!point.is_empty()),
                    Support::Unsupported { why } => {
                        assert!(why.starts_with("an egress policy ("), "{b} on {os}: {why}");
                        assert!(why.len() > 40, "{b} on {os}: say why ({why})")
                    }
                }
            }
        }
        // The list is complete: one entry per variant.
        let mut names: Vec<&str> = BACKENDS.iter().map(|b| b.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), BACKENDS.len());
        let exhaustive = |b: BackendKind| match b {
            BackendKind::Lume
            | BackendKind::Qemu
            | BackendKind::QemuDocker
            | BackendKind::Container
            | BackendKind::Managed
            | BackendKind::Firecracker
            | BackendKind::Containerization
            | BackendKind::Vz => BACKENDS.contains(&b),
        };
        assert!(BACKENDS.iter().all(|b| exhaustive(*b)));
        // What enforces where (the table in the docs).
        let host = |b, os| matches!(support_on(b, os), Support::Host { .. });
        assert!(host(BackendKind::Container, "linux") && host(BackendKind::Container, "macos"));
        assert!(host(BackendKind::Managed, "macos"));
        assert!(host(BackendKind::Qemu, "linux") && !host(BackendKind::Qemu, "macos"));
        assert!(host(BackendKind::Firecracker, "linux"));
        assert!(!host(BackendKind::Lume, "macos") && !host(BackendKind::Containerization, "macos"));
    }
}
