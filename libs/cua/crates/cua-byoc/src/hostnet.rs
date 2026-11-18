// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The network guard every cloud machine that runs Spaces installs on its
//! first boot, before any container starts.
//!
//! A Space is untrusted code. On a computer in your cloud account it must not
//! reach the cloud's metadata service (the computer's credentials and user
//! data), the cloud's other private services, your private network, or the
//! machine itself. The guard is the host's own firewall, outside every
//! container, so a Space cannot undo it. It is the floor under whatever
//! network settings a Space has: a Space's own rules can only be stricter.
//!
//! What it blocks for traffic that comes from a container bridge
//! (`docker0`, `br-*`), first match wins:
//!
//! 1. DNS (port 53) to the resolver the computer itself uses, so names work;
//! 2. the **platform denies**: the metadata services of AWS, Google Cloud,
//!    Azure, Alibaba and ECS (`169.254.169.254`, `169.254.170.2`,
//!    `fd00:ec2::254`, `168.63.129.16`, `100.100.100.200`);
//! 3. the computer's **own network**: the VPC or VNet and the subnet the
//!    machine sits in, read at boot;
//! 4. the **default denies**: private, shared, link-local, loopback,
//!    multicast and reserved ranges (IPv4 and IPv6).
//!
//! Everything else (the public internet) is left to Docker. Traffic that
//! targets the computer itself arrives on its INPUT path, where only replies
//! and the local resolver (on Azure) are allowed from a bridge.
//!
//! Azure answers DNS at `168.63.129.16`, the same address that serves the
//! machine's provisioning agent, so Spaces never get it. A small resolver on
//! the host (`169.254.53.53`, on a dummy interface) forwards to it, drops
//! private answers (DNS rebinding) and is what containers use.
//!
//! Containers on a computer are the only Spaces this guard covers by itself:
//! a VM Space runs behind its own network namespace, where the per-Space
//! rules apply.

use crate::bootstrap::sh_quote;

/// Addresses of cloud services a Space never reaches, whatever its settings:
/// AWS and Google Cloud and most clouds' metadata (`169.254.169.254`), the ECS
/// task credentials endpoint, Azure's wire server, Alibaba's metadata.
pub const PLATFORM_DENY_V4: &[&str] = &[
    "169.254.169.254/32",
    "169.254.170.2/32",
    "168.63.129.16/32",
    "100.100.100.200/32",
];

/// The IPv6 form of the AWS metadata service.
pub const PLATFORM_DENY_V6: &[&str] = &["fd00:ec2::254/128"];

/// Ranges blocked unless a Space's network settings lift them where the
/// machine allows it: this host layer never lifts them.
pub const DEFAULT_DENY_V4: &[&str] = &[
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.0.0.0/24",
    "192.168.0.0/16",
    "198.18.0.0/15",
    "224.0.0.0/4",
    "240.0.0.0/4",
];

/// IPv6 default denies. IPv4-mapped addresses are judged as IPv4 by the
/// kernel on the IPv4 path; they are listed so a v6 stack cannot be used to
/// reach a blocked v4 address.
pub const DEFAULT_DENY_V6: &[&str] = &[
    "::1/128",
    "::ffff:0:0/96",
    "64:ff9b::/96",
    "fc00::/7",
    "fe80::/10",
    "ff00::/8",
];

/// The address of the host's own resolver for Spaces on Azure (a dummy
/// interface on the host; the wire server is never reachable from a Space).
pub const LOCAL_DNS: &str = "169.254.53.53";

/// The resolver Azure machines forward to (the wire server).
pub const AZURE_WIRESERVER: &str = "168.63.129.16";

/// Where the guard script is installed.
pub const SCRIPT_PATH: &str = "/usr/local/sbin/cua-host-net";

/// The systemd unit that runs the guard after every Docker start.
pub const UNIT: &str = "cua-host-net.service";

/// Where the guard remembers the computer's own networks (the cloud's
/// metadata service may be off by the time of a later run).
pub const STATE_FILE: &str = "/etc/cua/host-net.cidrs";

/// What the guard needs to know about the computer.
#[derive(Clone, Copy, Debug)]
pub struct HostNet<'a> {
    /// The cloud: `aws`, `gcp`, `azure`, `hetzner` (anything else is treated
    /// like Hetzner: only what the computer's routes show).
    pub provider: &'a str,
    /// Extra private networks to block (the VPC the provider created, known
    /// before boot). The guard also reads the computer's own at boot.
    pub extra_cidrs: &'a [String],
}

impl HostNet<'_> {
    /// Whether the computer's DNS is the Azure wire server, answered by the
    /// host's own resolver.
    pub fn local_dns(&self) -> bool {
        self.provider == "azure"
    }
}

fn list(items: &[&str]) -> String {
    items.join(" ")
}

/// The guard script (POSIX `sh`, run as root). It rebuilds its own chains
/// every time, so it can run again after Docker restarts.
///
/// `CUA_NET_DRY=1` prints each firewall command instead of running it, with
/// `CUA_NET_RESOLVERS` and `CUA_NET_CIDRS` standing in for what it reads
/// from the computer: the tests run it that way.
pub fn script(net: &HostNet<'_>) -> String {
    let extra = net.extra_cidrs.join(" ");
    let local_dns = if net.local_dns() { "1" } else { "0" };
    format!(
        r#"#!/bin/sh
# Cua: keep Spaces off the cloud's metadata service, its private network and
# this computer. Traffic from container bridges only; rebuilt on every run.
set -eu
PATH=/usr/local/sbin:/usr/sbin:/sbin:/usr/local/bin:/usr/bin:/bin
PROVIDER={provider}
LOCAL_DNS={local_dns}
LOCAL_DNS_ADDR={LOCAL_DNS}
WIRESERVER={AZURE_WIRESERVER}
PLATFORM_V4="{platform_v4}"
PLATFORM_V6="{platform_v6}"
DEFAULT_V4="{default_v4}"
DEFAULT_V6="{default_v6}"
EXTRA_CIDRS="{extra}"
STATE={STATE_FILE}
GUARD=CUA-SPACE-GUARD
INPUT_GUARD=CUA-SPACE-INPUT
BRIDGES="docker0 br-+"

run() {{
  if [ "${{CUA_NET_DRY:-0}}" = 1 ]; then echo "$*"; else "$@"; fi
}}

# The resolvers the computer uses (never its own stub or the wire server).
resolvers() {{
  if [ -n "${{CUA_NET_RESOLVERS+x}}" ]; then echo $CUA_NET_RESOLVERS; return; fi
  for f in /run/systemd/resolve/resolv.conf /etc/resolv.conf; do
    [ -r "$f" ] || continue
    out=""
    for ns in $(awk '$1=="nameserver"{{print $2}}' "$f"); do
      case "$ns" in
        127.*|::1|*:*) ;;
        "$WIRESERVER") ;;
        *) out="$out $ns" ;;
      esac
    done
    if [ -n "$out" ]; then echo $out; return; fi
  done
}}

# The computer's own networks: what the cloud says (AWS: the VPC), then
# every network its interfaces sit on. Remembered for later runs.
own_cidrs() {{
  if [ -n "${{CUA_NET_CIDRS+x}}" ]; then echo $CUA_NET_CIDRS; return; fi
  found=""
  if [ "$PROVIDER" = aws ]; then
    t=$(curl -fsS -m 3 -X PUT -H 'X-aws-ec2-metadata-token-ttl-seconds: 60' http://169.254.169.254/latest/api/token 2>/dev/null || true)
    if [ -n "$t" ]; then
      mac=$(curl -fsS -m 3 -H "X-aws-ec2-metadata-token: $t" http://169.254.169.254/latest/meta-data/mac 2>/dev/null || true)
      [ -n "$mac" ] && found=$(curl -fsS -m 3 -H "X-aws-ec2-metadata-token: $t" "http://169.254.169.254/latest/meta-data/network/interfaces/macs/$mac/vpc-ipv4-cidr-blocks" 2>/dev/null || true)
    fi
  fi
  routes=$(ip -4 -o route show scope link 2>/dev/null | awk '$0 !~ / dev (docker|br-|veth|virbr|cua)/ && $1 ~ /\//{{print $1}}' || true)
  if [ -f "$STATE" ]; then found="$found $(cat "$STATE")"; fi
  all=$(echo $found $routes $EXTRA_CIDRS | tr ' ' '\n' | grep -E '^[0-9.]+/[0-9]+$' | sort -u | tr '\n' ' ')
  if [ "${{CUA_NET_DRY:-0}}" != 1 ] && [ -n "$all" ]; then
    mkdir -p /etc/cua && echo "$all" > "$STATE.tmp" && mv "$STATE.tmp" "$STATE"
  fi
  echo $all
}}

fresh() {{
  tool=$1; chain=$2
  run $tool -N "$chain" 2>/dev/null || true
  run $tool -F "$chain"
}}

hook() {{
  tool=$1; parent=$2; chain=$3
  run $tool -N "$parent" 2>/dev/null || true
  for br in $BRIDGES; do
    if [ "${{CUA_NET_DRY:-0}}" = 1 ] || ! $tool -C "$parent" -i "$br" -j "$chain" 2>/dev/null; then
      run $tool -I "$parent" 1 -i "$br" -j "$chain"
    fi
  done
}}

RES=$(resolvers)
CIDRS=$(own_cidrs)

# IPv4, forwarded traffic of containers (DOCKER-USER runs before Docker's rules).
fresh iptables $GUARD
for r in $RES; do
  run iptables -A $GUARD -d "$r" -p udp --dport 53 -j RETURN
  run iptables -A $GUARD -d "$r" -p tcp --dport 53 -j RETURN
done
for n in $PLATFORM_V4; do run iptables -A $GUARD -d "$n" -j DROP; done
for n in $CIDRS; do run iptables -A $GUARD -d "$n" -j DROP; done
for n in $DEFAULT_V4; do run iptables -A $GUARD -d "$n" -j DROP; done
run iptables -A $GUARD -j RETURN
hook iptables DOCKER-USER $GUARD

# IPv4, traffic to the computer itself: replies, and the local resolver.
fresh iptables $INPUT_GUARD
run iptables -A $INPUT_GUARD -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN
if [ "$LOCAL_DNS" = 1 ]; then
  run iptables -A $INPUT_GUARD -d "$LOCAL_DNS_ADDR" -p udp --dport 53 -j RETURN
  run iptables -A $INPUT_GUARD -d "$LOCAL_DNS_ADDR" -p tcp --dport 53 -j RETURN
fi
run iptables -A $INPUT_GUARD -j DROP
hook iptables INPUT $INPUT_GUARD

# IPv6 (Docker does not give containers IPv6 unless asked; if it does, the
# same floor applies).
if [ "${{CUA_NET_DRY:-0}}" = 1 ] || command -v ip6tables >/dev/null 2>&1; then
  fresh ip6tables $GUARD
  for n in $PLATFORM_V6; do run ip6tables -A $GUARD -d "$n" -j DROP; done
  for n in $DEFAULT_V6; do run ip6tables -A $GUARD -d "$n" -j DROP; done
  run ip6tables -A $GUARD -j RETURN
  hook ip6tables DOCKER-USER $GUARD
  fresh ip6tables $INPUT_GUARD
  run ip6tables -A $INPUT_GUARD -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN
  run ip6tables -A $INPUT_GUARD -p ipv6-icmp -j RETURN
  run ip6tables -A $INPUT_GUARD -j DROP
  hook ip6tables INPUT $INPUT_GUARD
fi
"#,
        provider = sh_quote(net.provider),
        platform_v4 = list(PLATFORM_DENY_V4),
        platform_v6 = list(PLATFORM_DENY_V6),
        default_v4 = list(DEFAULT_DENY_V4),
        default_v6 = list(DEFAULT_DENY_V6),
    )
}

/// The unit that runs the guard once Docker is up, and again whenever Docker
/// restarts (a restart can recreate the chains the guard hooks into).
pub fn unit() -> &'static str {
    "[Unit]\n\
     Description=Cua: keep Spaces off the cloud's metadata service and private network\n\
     After=docker.service network-online.target\n\
     Wants=network-online.target\n\
     PartOf=docker.service\n\n\
     [Service]\n\
     Type=oneshot\n\
     RemainAfterExit=yes\n\
     ExecStart=/usr/local/sbin/cua-host-net\n\n\
     [Install]\n\
     WantedBy=multi-user.target docker.service\n"
}

/// The resolver unit for Azure: dnsmasq on a dummy interface, forwarding to
/// the wire server, dropping private answers.
pub fn dns_unit() -> String {
    format!(
        "[Unit]\n\
         Description=Cua: DNS for Spaces (the wire server is not reachable from a Space)\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         Before=docker.service\n\n\
         [Service]\n\
         ExecStartPre=-/sbin/ip link add cua-dns0 type dummy\n\
         ExecStartPre=-/sbin/ip addr add {LOCAL_DNS}/32 dev cua-dns0\n\
         ExecStartPre=/sbin/ip link set cua-dns0 up\n\
         ExecStart=/usr/sbin/dnsmasq --keep-in-foreground --conf-file=/dev/null \
         --listen-address={LOCAL_DNS} --bind-interfaces --no-resolv --no-hosts \
         --server={AZURE_WIRESERVER} --stop-dns-rebind --cache-size=1000\n\
         Restart=always\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// The shell lines that install the guard (and, on Azure, the local
/// resolver) and start it. They go after Docker is installed and before any
/// container runs; a failure fails the boot, so a computer that cannot guard
/// its Spaces never runs one.
pub fn install_snippet(net: &HostNet<'_>) -> String {
    let mut s = String::new();
    s.push_str("# Cua: the host's network guard for Spaces (see hostnet.rs).\n");
    s.push_str("mkdir -p /etc/cua\n");
    s.push_str(&format!(
        "cat > {SCRIPT_PATH} <<'CUA_HOST_NET'\n{}CUA_HOST_NET\nchmod 755 {SCRIPT_PATH}\n",
        script(net)
    ));
    s.push_str(&format!(
        "cat > /etc/systemd/system/{UNIT} <<'CUA_HOST_NET_UNIT'\n{}CUA_HOST_NET_UNIT\n",
        unit()
    ));
    if net.local_dns() {
        s.push_str(&format!(
            "( command -v dnsmasq >/dev/null || apt-get install -y -qq dnsmasq-base ) || {{ echo 'cua: no local resolver for Spaces'; exit 1; }}\n\
             cat > /etc/systemd/system/cua-dns.service <<'CUA_DNS_UNIT'\n{}CUA_DNS_UNIT\n\
             mkdir -p /etc/docker\n\
             python3 - <<'CUA_DNS_DOCKER'\n\
             import json, os\n\
             p = '/etc/docker/daemon.json'\n\
             d = json.load(open(p)) if os.path.exists(p) and os.path.getsize(p) else {{}}\n\
             d['dns'] = ['{LOCAL_DNS}']\n\
             json.dump(d, open(p, 'w'))\n\
             CUA_DNS_DOCKER\n\
             systemctl daemon-reload\n\
             systemctl enable --now cua-dns.service\n\
             systemctl restart docker\n",
            dns_unit()
        ));
    }
    s.push_str(&format!(
        "systemctl daemon-reload\nsystemctl enable --now {UNIT}\n"
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::process::Command;

    /// One firewall command the script printed, as a rule of a chain.
    #[derive(Debug)]
    struct Rule {
        chain: String,
        dst: Option<String>,
        proto: Option<String>,
        dport: Option<u16>,
        ctstate: bool,
        target: String,
    }

    fn run_dry(net: &HostNet<'_>, resolvers: &str, cidrs: &str) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("guard.sh");
        std::fs::write(&path, script(net)).unwrap();
        let out = Command::new("sh")
            .arg(&path)
            .env("CUA_NET_DRY", "1")
            .env("CUA_NET_RESOLVERS", resolvers)
            .env("CUA_NET_CIDRS", cidrs)
            .output()
            .expect("sh");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn rules(lines: &[String], tool: &str) -> Vec<Rule> {
        lines
            .iter()
            .filter_map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                if w.first() != Some(&tool) || w.get(1) != Some(&"-A") {
                    return None;
                }
                let get = |flag: &str| {
                    w.iter()
                        .position(|x| *x == flag)
                        .and_then(|i| w.get(i + 1))
                        .map(|s| s.to_string())
                };
                Some(Rule {
                    chain: w[2].to_string(),
                    dst: get("-d"),
                    proto: get("-p"),
                    dport: get("--dport").and_then(|p| p.parse().ok()),
                    ctstate: w.contains(&"--ctstate"),
                    target: get("-j").unwrap(),
                })
            })
            .collect()
    }

    fn in_net(cidr: &str, ip: Ipv4Addr) -> bool {
        let (a, p) = cidr.split_once('/').unwrap_or((cidr, "32"));
        let a: Ipv4Addr = a.parse().unwrap();
        let p: u32 = p.parse().unwrap();
        let mask = if p == 0 { 0 } else { u32::MAX << (32 - p) };
        u32::from(a) & mask == u32::from(ip) & mask
    }

    /// What the guard does with a new connection from a container to
    /// `ip`:`port` (tcp or udp): first match in the chain wins; a chain
    /// that ends in RETURN hands the packet on to Docker (accepted here).
    fn forwarded(rules: &[Rule], ip: Ipv4Addr, proto: &str, port: u16) -> bool {
        for r in rules.iter().filter(|r| r.chain == "CUA-SPACE-GUARD") {
            let dst_hit = r.dst.as_deref().is_none_or(|d| in_net(d, ip));
            let proto_hit = r.proto.as_deref().is_none_or(|p| p == proto);
            let port_hit = r.dport.is_none_or(|p| p == port);
            if dst_hit && proto_hit && port_hit && !r.ctstate {
                return r.target == "RETURN";
            }
        }
        true
    }

    fn aws() -> HostNet<'static> {
        HostNet {
            provider: "aws",
            extra_cidrs: &[],
        }
    }

    #[test]
    fn the_script_is_valid_sh_for_every_cloud() {
        for provider in ["aws", "gcp", "azure", "hetzner", "x'y"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("g.sh");
            std::fs::write(
                &path,
                script(&HostNet {
                    provider,
                    extra_cidrs: &[],
                }),
            )
            .unwrap();
            let st = Command::new("sh").arg("-n").arg(&path).status().unwrap();
            assert!(st.success(), "{provider}");
        }
    }

    #[test]
    fn a_container_reaches_the_internet_and_dns_but_nothing_private() {
        let lines = run_dry(&aws(), "10.20.0.2", "10.20.0.0/16 10.20.1.0/24");
        let r = rules(&lines, "iptables");
        let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
        // The internet.
        assert!(forwarded(&r, ip("93.184.216.34"), "tcp", 443));
        assert!(forwarded(&r, ip("1.1.1.1"), "udp", 53));
        // The computer's resolver answers DNS on 53 only.
        assert!(forwarded(&r, ip("10.20.0.2"), "udp", 53));
        assert!(forwarded(&r, ip("10.20.0.2"), "tcp", 53));
        assert!(!forwarded(&r, ip("10.20.0.2"), "tcp", 443));
        assert!(!forwarded(&r, ip("10.20.0.2"), "udp", 123));
        // The metadata service of every cloud, by any port.
        for ip_s in [
            "169.254.169.254",
            "169.254.170.2",
            "168.63.129.16",
            "100.100.100.200",
        ] {
            assert!(!forwarded(&r, ip(ip_s), "tcp", 80), "{ip_s}");
            assert!(!forwarded(&r, ip(ip_s), "udp", 53), "{ip_s} dns");
        }
        // The VPC and every default deny.
        assert!(!forwarded(&r, ip("10.20.5.6"), "tcp", 22));
        assert!(!forwarded(&r, ip("172.31.4.4"), "tcp", 22));
        assert!(!forwarded(&r, ip("192.168.1.1"), "tcp", 80));
        assert!(!forwarded(&r, ip("100.64.0.1"), "tcp", 80));
        assert!(!forwarded(&r, ip("127.0.0.1"), "tcp", 80));
        assert!(!forwarded(&r, ip("169.254.1.1"), "tcp", 80));
        assert!(!forwarded(&r, ip("224.0.0.251"), "udp", 5353));
        assert!(!forwarded(&r, ip("255.255.255.255"), "udp", 67));
    }

    #[test]
    fn a_public_vpc_range_is_blocked_too() {
        // A VPC on a public-looking range is still the computer's own network.
        let lines = run_dry(&aws(), "", "44.0.0.0/16");
        let r = rules(&lines, "iptables");
        assert!(!forwarded(&r, "44.0.9.9".parse().unwrap(), "tcp", 22));
        assert!(forwarded(&r, "44.1.0.1".parse().unwrap(), "tcp", 22));
    }

    #[test]
    fn the_google_resolver_is_the_metadata_address_and_only_dns_passes() {
        // Compute Engine's resolver is 169.254.169.254: port 53 only.
        let lines = run_dry(&aws(), "169.254.169.254", "");
        let r = rules(&lines, "iptables");
        let md: Ipv4Addr = "169.254.169.254".parse().unwrap();
        assert!(forwarded(&r, md, "udp", 53));
        assert!(!forwarded(&r, md, "tcp", 80));
        assert!(!forwarded(&r, md, "tcp", 443));
    }

    #[test]
    fn the_machine_itself_takes_only_replies_and_the_azure_resolver() {
        let lines = run_dry(&aws(), "10.0.0.2", "10.0.0.0/16");
        let input: Vec<_> = rules(&lines, "iptables")
            .into_iter()
            .filter(|r| r.chain == "CUA-SPACE-INPUT")
            .collect();
        assert!(input.first().unwrap().ctstate);
        assert_eq!(input.last().unwrap().target, "DROP");
        assert!(input.last().unwrap().dst.is_none());
        assert!(input.iter().all(|r| r.dst.as_deref() != Some(LOCAL_DNS)));
        let az = run_dry(
            &HostNet {
                provider: "azure",
                extra_cidrs: &[],
            },
            "",
            "10.1.0.0/16",
        );
        let input: Vec<_> = rules(&az, "iptables")
            .into_iter()
            .filter(|r| r.chain == "CUA-SPACE-INPUT")
            .collect();
        let dns: Vec<_> = input
            .iter()
            .filter(|r| r.dst.as_deref() == Some(LOCAL_DNS))
            .collect();
        assert_eq!(dns.len(), 2);
        assert!(
            dns.iter()
                .all(|r| r.dport == Some(53) && r.target == "RETURN")
        );
        // Nothing allows the wire server itself.
        let fwd = rules(&az, "iptables");
        assert!(!forwarded(
            &fwd,
            "168.63.129.16".parse().unwrap(),
            "udp",
            53
        ));
    }

    #[test]
    fn the_guard_hooks_both_chains_for_every_bridge_and_both_families() {
        let lines = run_dry(&aws(), "10.0.0.2", "10.0.0.0/16");
        for tool in ["iptables", "ip6tables"] {
            for (parent, chain) in [
                ("DOCKER-USER", "CUA-SPACE-GUARD"),
                ("INPUT", "CUA-SPACE-INPUT"),
            ] {
                for br in ["docker0", "br-+"] {
                    let want = format!("{tool} -I {parent} 1 -i {br} -j {chain}");
                    assert!(lines.contains(&want), "missing {want}");
                }
            }
        }
        let v6 = rules(&lines, "ip6tables");
        let hit = |ip: &str| {
            let ip: std::net::Ipv6Addr = ip.parse().unwrap();
            // Only the simple /128 and the prefixes this test names.
            v6.iter().filter(|r| r.chain == "CUA-SPACE-GUARD").any(|r| {
                r.target == "DROP"
                    && r.dst.as_deref().is_some_and(|d| {
                        let (a, p) = d.split_once('/').unwrap();
                        let a: std::net::Ipv6Addr = a.parse().unwrap();
                        let p: u32 = p.parse().unwrap();
                        let mask = if p == 0 { 0 } else { u128::MAX << (128 - p) };
                        u128::from(a) & mask == u128::from(ip) & mask
                    })
            })
        };
        assert!(hit("fd00:ec2::254"));
        assert!(hit("fe80::1"));
        assert!(hit("fd12:3456::1"));
        assert!(hit("::1"));
        assert!(!hit("2606:4700::1111"));
    }

    #[test]
    fn extra_networks_the_provider_knows_are_blocked() {
        let extra = vec!["44.5.0.0/16".to_string()];
        let net = HostNet {
            provider: "hetzner",
            extra_cidrs: &extra,
        };
        let lines = run_dry(&net, "185.12.64.1", "");
        // The env override replaces discovery, so the extra list is not
        // read in dry mode; a real run merges it. The script must carry it.
        assert!(script(&net).contains("EXTRA_CIDRS=\"44.5.0.0/16\""));
        assert!(forwarded(
            &rules(&lines, "iptables"),
            "185.12.64.1".parse().unwrap(),
            "udp",
            53
        ));
    }

    #[test]
    fn the_lists_are_complete_and_ordered_for_the_floor() {
        // The SPEC's default denies, exactly.
        assert_eq!(DEFAULT_DENY_V4.len(), 11);
        for n in [
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "127.0.0.0/8",
        ] {
            assert!(DEFAULT_DENY_V4.contains(&n));
        }
        let s = script(&aws());
        let platform = s.find("for n in $PLATFORM_V4").unwrap();
        let own = s.find("for n in $CIDRS").unwrap();
        let default = s.find("for n in $DEFAULT_V4").unwrap();
        let dns = s.find("--dport 53 -j RETURN").unwrap();
        assert!(dns < platform && platform < own && own < default);
    }

    #[test]
    fn the_install_snippet_writes_the_guard_and_starts_it_before_containers() {
        let s = install_snippet(&aws());
        assert!(s.contains("cat > /usr/local/sbin/cua-host-net <<'CUA_HOST_NET'"));
        assert!(s.contains("systemctl enable --now cua-host-net.service"));
        assert!(!s.contains("dnsmasq"));
        let az = install_snippet(&HostNet {
            provider: "azure",
            extra_cidrs: &[],
        });
        assert!(az.contains("dnsmasq"));
        assert!(az.contains("--server=168.63.129.16"));
        assert!(az.contains("--stop-dns-rebind"));
        assert!(az.contains("d['dns'] = ['169.254.53.53']"));
        // The resolver and its docker setting come before the guard starts.
        assert!(az.find("cua-dns.service").unwrap() < az.rfind("cua-host-net.service").unwrap());
    }
}
