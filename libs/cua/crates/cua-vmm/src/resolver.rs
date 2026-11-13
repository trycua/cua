//! Which DNS servers a Space's guest uses, and the file that says so.
//!
//! A container on a user-defined Docker network gets `nameserver
//! 127.0.0.11`: Docker's embedded resolver, which the engine reaches
//! through a rule in the container's own network namespace. gVisor's
//! network stack ignores that rule, so under gVisor the address is a dead
//! end and no name resolves, although the Space reaches the internet by
//! address. Every Space on a secure network is on such a network, so the
//! container runtime gives the guest explicit servers instead, from here:
//!
//! 1. `CUA_GUEST_DNS` (a comma list), when it names a usable server;
//! 2. else the servers of the engine host's own `resolv.conf` (and
//!    systemd-resolved's upstream list), the ones Docker's embedded
//!    resolver would have forwarded to;
//! 3. else public resolvers ([`FALLBACK`]): a host whose only resolver is a
//!    local stub (`127.0.0.53`, a dnsmasq) has none a guest can reach.
//!
//! A server counts only when a guest can reach it: not a loopback, link
//! local, multicast or unspecified address (those are the host's own), not a
//! cloud metadata endpoint (always denied), and an IPv6 server only on a
//! network that has IPv6. One exception: a cloud platform's own resolver
//! ([`PLATFORM_RESOLVERS`]: Azure's `168.63.129.16`, AWS's
//! `169.254.169.253`, GCP's `169.254.169.254`) is kept when the host uses
//! it, so a private DNS zone still resolves. Two of them sit on the
//! metadata or link-local ranges, so the egress rules open each on port 53
//! only, before the metadata deny ([`crate::network::compile`]); its other
//! ports (the wireserver's and metadata server's HTTP) stay closed. At most [`MAX_SERVERS`]
//! (what a `resolv.conf` honours). The list is both written into the guest
//! and allowed on port 53 by the Space's egress rules
//! ([`crate::network::NetworkContext::resolvers`]), so the guest can reach
//! exactly those servers and nothing else on the host or the local network
//! for DNS. A Space whose network is `none` gets no server at all.

use std::net::{IpAddr, Ipv6Addr};

/// The environment variable that names the guests' servers.
pub const ENV: &str = "CUA_GUEST_DNS";

/// Public resolvers used when the host has no server a guest can reach.
pub const FALLBACK: [&str; 2] = ["1.1.1.1", "8.8.8.8"];

/// The most servers a `resolv.conf` honours (glibc's `MAXNS`).
pub const MAX_SERVERS: usize = 3;

/// The DNS resolvers a cloud platform runs at a fixed address. A host on
/// that cloud names one in its `resolv.conf` (directly, or behind the
/// `127.0.0.53` stub), and private DNS zones answer only there. They are
/// exact addresses, never a range: Azure's wireserver, AWS's Route 53
/// resolver (IPv4 and IPv6) and GCP's metadata server (which also answers
/// DNS). The VPC `.2` resolver of AWS is an ordinary private address.
pub const PLATFORM_RESOLVERS: [&str; 4] = [
    "168.63.129.16",
    "169.254.169.253",
    "169.254.169.254",
    "fd00:ec2::253",
];

/// Whether `ip` is one of [`PLATFORM_RESOLVERS`].
pub fn is_platform_resolver(ip: IpAddr) -> bool {
    PLATFORM_RESOLVERS
        .iter()
        .filter_map(|r| r.parse::<IpAddr>().ok())
        .any(|r| r == ip)
}

/// Whether `ip` is a cloud metadata endpoint ([`crate::network::METADATA`]).
/// Those are denied to every Space before any resolver is allowed, so a
/// resolver there would be written into the guest and never answer. It is
/// the only resolver of a GitHub-hosted runner (Azure answers DNS at
/// `168.63.129.16`, behind the `127.0.0.53` stub); see [`usable`].
fn is_metadata(ip: IpAddr) -> bool {
    crate::network::METADATA
        .iter()
        .filter_map(|n| crate::egress::Net::parse(n).ok())
        .any(|n| n.contains(ip))
}

/// Whether a guest could send DNS to `ip`.
pub fn usable(ip: IpAddr, ipv6: bool) -> bool {
    // A platform resolver is reachable on port 53 only; the egress rules
    // open it that way. Everything else here stays as strict as before.
    if is_platform_resolver(ip) {
        return ip.is_ipv4() || ipv6;
    }
    if is_metadata(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(a) => {
            !(a.is_loopback()
                || a.is_unspecified()
                || a.is_link_local()
                || a.is_multicast()
                || a.is_broadcast())
        }
        IpAddr::V6(a) => {
            ipv6 && !(a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || is_link_local_v6(a)
                // IPv4 inside IPv6: judged as the IPv4 address it is.
                || a.to_ipv4_mapped().is_some_and(|m| {
                    is_platform_resolver(IpAddr::V4(m)) || !usable(IpAddr::V4(m), false)
                }))
        }
    }
}

fn is_link_local_v6(a: Ipv6Addr) -> bool {
    a.segments()[0] & 0xffc0 == 0xfe80
}

/// The `nameserver` addresses of a `resolv.conf` (a zone suffix such as
/// `%en0` is dropped), in order.
pub fn nameservers(resolv_conf: &str) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for line in resolv_conf.lines() {
        let mut w = line.split_whitespace();
        if w.next() != Some("nameserver") {
            continue;
        }
        let Some(addr) = w.next() else { continue };
        let addr = addr.split('%').next().unwrap_or(addr);
        if let Ok(ip) = addr.parse::<IpAddr>() {
            out.push(ip);
        }
    }
    out
}

fn push_usable(out: &mut Vec<IpAddr>, ip: IpAddr, ipv6: bool) {
    if usable(ip, ipv6) && !out.contains(&ip) && out.len() < MAX_SERVERS {
        out.push(ip);
    }
}

/// The servers for a guest: `env` (the value of [`ENV`]), else `host` (the
/// addresses the engine host's resolver files name, in order), else
/// [`FALLBACK`]. `ipv6`: the guest's network has IPv6.
pub fn select(env: Option<&str>, host: &[IpAddr], ipv6: bool) -> Vec<IpAddr> {
    let mut out = Vec::new();
    if let Some(list) = env.map(str::trim).filter(|l| !l.is_empty()) {
        for item in list.split(',') {
            if let Ok(ip) = item.trim().parse::<IpAddr>() {
                push_usable(&mut out, ip, ipv6);
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    for ip in host {
        push_usable(&mut out, *ip, ipv6);
    }
    if out.is_empty() {
        for f in FALLBACK {
            if let Ok(ip) = f.parse::<IpAddr>() {
                push_usable(&mut out, ip, ipv6);
            }
        }
    }
    out
}

/// [`select`] with the process environment's [`ENV`].
pub fn select_from_env(host: &[IpAddr], ipv6: bool) -> Vec<IpAddr> {
    select(std::env::var(ENV).ok().as_deref(), host, ipv6)
}

/// The text of the guest's `/etc/resolv.conf` for `servers`. No servers
/// (a Space without network): no `nameserver` line, so a lookup fails at
/// once instead of waiting for a server that is never answered.
pub fn resolv_conf(servers: &[IpAddr]) -> String {
    let mut s = String::from("# written by Cua: the only name servers this Space may ask\n");
    for ip in servers {
        s.push_str(&format!("nameserver {ip}\n"));
    }
    if servers.is_empty() {
        s.push_str("# this Space has no network\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn a_host_resolver_a_guest_can_reach_is_used_in_order() {
        let conf = "search lan\nnameserver 192.168.5.3\nnameserver 10.0.0.2\n";
        assert_eq!(
            select(None, &nameservers(conf), false),
            [ip("192.168.5.3"), ip("10.0.0.2")]
        );
    }

    #[test]
    fn loopback_link_local_and_unspecified_servers_are_the_hosts_own() {
        let conf = "nameserver 127.0.0.53\nnameserver 127.0.0.1\nnameserver 169.254.1.1\n\
                    nameserver 0.0.0.0\nnameserver fe80::1%en0\nnameserver ::1\nnameserver 224.0.0.251\n";
        // Nothing left: public resolvers.
        assert_eq!(
            select(None, &nameservers(conf), true),
            [ip("1.1.1.1"), ip("8.8.8.8")]
        );
        // The usable one among them is kept.
        let conf = "nameserver 127.0.0.53\nnameserver 10.1.2.3\n";
        assert_eq!(select(None, &nameservers(conf), false), [ip("10.1.2.3")]);
    }

    #[test]
    fn a_cloud_platform_resolver_is_kept_not_replaced_by_public_dns() {
        // Azure (GitHub-hosted runner: 127.0.0.53 stub, wireserver upstream):
        // private DNS zones answer only there.
        let conf = "nameserver 127.0.0.53\nnameserver 168.63.129.16\n";
        assert_eq!(
            select(None, &nameservers(conf), false),
            [ip("168.63.129.16")]
        );
        // AWS: the Route 53 resolver; the VPC `.2` resolver is plain private.
        let conf = "nameserver 169.254.169.253\n";
        assert_eq!(
            select(None, &nameservers(conf), false),
            [ip("169.254.169.253")]
        );
        let conf = "nameserver 10.0.0.2\n";
        assert_eq!(select(None, &nameservers(conf), false), [ip("10.0.0.2")]);
        // GCP: the metadata server answers DNS.
        let conf = "nameserver 169.254.169.254\n";
        assert_eq!(
            select(None, &nameservers(conf), false),
            [ip("169.254.169.254")]
        );
        // AWS IPv6 only where the network has IPv6.
        assert!(usable(ip("fd00:ec2::253"), true));
        assert!(!usable(ip("fd00:ec2::253"), false));
        // Kept beside a private resolver, in order.
        let conf = "nameserver 168.63.129.16\nnameserver 10.0.0.2\n";
        assert_eq!(
            select(None, &nameservers(conf), false),
            [ip("168.63.129.16"), ip("10.0.0.2")]
        );
        // Through the environment too.
        assert_eq!(
            select(Some("168.63.129.16"), &[], false),
            [ip("168.63.129.16")]
        );
    }

    #[test]
    fn other_metadata_endpoints_and_look_alikes_stay_rejected() {
        // Metadata endpoints that serve no DNS, a neighbour of a platform
        // resolver, an IPv4-mapped form, loopback and link-local servers.
        for m in [
            "169.254.170.2",
            "fd00:ec2::254",
            "100.100.100.200",
            "169.254.169.252",
            "::ffff:168.63.129.16",
            "::ffff:169.254.169.254",
            "127.0.0.1",
            "127.0.0.53",
            "::1",
            "169.254.1.1",
        ] {
            assert!(!usable(ip(m), true), "{m}");
        }
        // A host with only those falls back to public resolvers.
        let conf = "nameserver 127.0.0.53\nnameserver 169.254.170.2\nnameserver 127.0.0.1\n";
        assert_eq!(
            select(None, &nameservers(conf), false),
            [ip("1.1.1.1"), ip("8.8.8.8")]
        );
    }

    #[test]
    fn a_corporate_resolver_is_used_and_no_public_one_is_added() {
        // Internal names resolve only there; a public server beside it
        // would be a second path out for DNS.
        let conf = "nameserver 10.20.0.53\nnameserver 10.20.0.54\n";
        assert_eq!(
            select(None, &nameservers(conf), false),
            [ip("10.20.0.53"), ip("10.20.0.54")]
        );
    }

    #[test]
    fn ipv6_servers_only_on_a_network_that_has_ipv6() {
        let host = [ip("2001:4860:4860::8888"), ip("10.0.0.2")];
        assert_eq!(select(None, &host, false), [ip("10.0.0.2")]);
        assert_eq!(
            select(None, &host, true),
            [ip("2001:4860:4860::8888"), ip("10.0.0.2")]
        );
        // An IPv4-mapped loopback is still the host's own.
        assert!(!usable(ip("::ffff:127.0.0.1"), true));
        assert!(usable(ip("::ffff:10.0.0.2"), true));
    }

    #[test]
    fn the_environment_overrides_the_host_and_ignores_unusable_entries() {
        let host = [ip("10.0.0.2")];
        assert_eq!(
            select(Some(" 9.9.9.9 , nonsense, 127.0.0.1,1.0.0.1"), &host, false),
            [ip("9.9.9.9"), ip("1.0.0.1")]
        );
        // Nothing usable in it: the host's.
        assert_eq!(select(Some("127.0.0.1"), &host, false), [ip("10.0.0.2")]);
        assert_eq!(select(Some("  "), &host, false), [ip("10.0.0.2")]);
    }

    #[test]
    fn at_most_three_distinct_servers() {
        let conf = "nameserver 10.0.0.1\nnameserver 10.0.0.1\nnameserver 10.0.0.2\n\
                    nameserver 10.0.0.3\nnameserver 10.0.0.4\n";
        assert_eq!(
            select(None, &nameservers(conf), false),
            [ip("10.0.0.1"), ip("10.0.0.2"), ip("10.0.0.3")]
        );
    }

    #[test]
    fn the_file_names_the_servers_and_a_space_without_network_none() {
        let t = resolv_conf(&[ip("10.0.0.2"), ip("1.1.1.1")]);
        assert!(
            t.contains("nameserver 10.0.0.2\nnameserver 1.1.1.1\n"),
            "{t}"
        );
        let none = resolv_conf(&[]);
        assert!(!none.contains("nameserver"), "{none}");
    }
}
