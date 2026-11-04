//! The egress policy of runner Spaces.
//!
//! A job from a pull request is untrusted code. By default
//! ([`crate::config::EgressMode::DenyPrivate`]) a runner's Space may reach
//! the internet but not the networks around it: the private ranges
//! (RFC 1918), carrier-grade NAT and tailnets (100.64.0.0/10), link-local
//! and cloud metadata (169.254.0.0/16), the benchmarking range the VM
//! namespaces use (198.18.0.0/15), IPv6 unique-local (fc00::/7) and
//! link-local (fe80::/10). The host's own services sit behind the guest's
//! gateway, which is in one of those ranges, so they are covered too.
//!
//! The Space's runtime enforces the policy on the host, outside the guest
//! (`cua spaces egress`): containers (runc, gVisor) on any Docker engine,
//! Firecracker microVMs and QEMU VMs on Linux. **macOS VMs (Lume) refuse a
//! policy**: their network is Apple's shared vmnet NAT, which only pf can
//! filter, and pf needs root. A pool then starts its macOS runners without
//! one (and records `egress_unenforced` with the runtime's reason) unless
//! `require_egress` is set, which refuses them instead.
//!
//! DNS keeps working where the guest's resolver is itself in a denied
//! range (QEMU's 10.0.2.3, a microVM's gateway, the host's resolver on a
//! container bridge): before the runner starts, the pool reads the guest's
//! resolver addresses and allows exactly those ([`resolver_allowances`]).
//! An allowed resolver is reachable on every port; rules are by address.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// The destinations a runner Space may not reach by default.
pub const DEFAULT_DENY: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "198.18.0.0/15",
    "fc00::/7",
    "fe80::/10",
];

/// An egress policy: `deny` blocks, `allow` wins over `deny` (the Spaces
/// model, `cua spaces egress`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// Exceptions to `deny`.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Blocked destinations.
    #[serde(default)]
    pub deny: Vec<String>,
}

impl EgressPolicy {
    /// No entries: no policy.
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Every entry is an address, a CIDR or `all`.
    pub fn check(&self) -> Result<(), String> {
        for e in self.allow.iter().chain(&self.deny) {
            Cidr::parse(e).ok_or_else(|| {
                format!(
                    "egress entry {e:?}: use an IPv4/IPv6 address or CIDR (rules are by IP/CIDR, \
                     not hostnames)"
                )
            })?;
        }
        Ok(())
    }

    /// Whether the policy blocks `ip`.
    pub fn blocks(&self, ip: IpAddr) -> bool {
        let hit = |list: &[String]| {
            list.iter()
                .filter_map(|e| Cidr::parse(e))
                .any(|c| c.contains(ip))
        };
        hit(&self.deny) && !hit(&self.allow)
    }
}

/// The pool's policy: [`DEFAULT_DENY`] plus `extra_deny`, except
/// `allow`. `None` when the mode is off.
pub fn pool_policy(
    mode: crate::config::EgressMode,
    extra_deny: &[String],
    allow: &[String],
) -> Option<EgressPolicy> {
    match mode {
        crate::config::EgressMode::Off => None,
        crate::config::EgressMode::DenyPrivate => {
            let mut deny: Vec<String> = DEFAULT_DENY.iter().map(|s| s.to_string()).collect();
            for d in extra_deny {
                if !deny.contains(d) {
                    deny.push(d.clone());
                }
            }
            Some(EgressPolicy {
                allow: allow.to_vec(),
                deny,
            })
        }
    }
}

/// The resolver addresses in a guest's `resolv.conf` text(s) that
/// `policy` blocks, as `/32` (or `/128`) allow entries, so DNS keeps
/// working. Loopback resolvers (systemd-resolved's 127.0.0.53, Docker's
/// 127.0.0.11) are never filtered and need nothing.
pub fn resolver_allowances(resolv_conf: &str, policy: &EgressPolicy) -> Vec<String> {
    let mut out = Vec::new();
    for line in resolv_conf.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("nameserver") {
            continue;
        }
        let Some(addr) = words.next() else { continue };
        // A scoped IPv6 address (fe80::1%eth0) is link-local: keep it
        // blocked rather than guess the scope.
        let Ok(ip) = addr.parse::<IpAddr>() else {
            continue;
        };
        if ip.is_loopback() || !policy.blocks(ip) {
            continue;
        }
        let entry = match ip {
            IpAddr::V4(_) => format!("{ip}/32"),
            IpAddr::V6(_) => format!("{ip}/128"),
        };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

/// One address family's network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cidr {
    addr: IpAddr,
    prefix: u8,
    all: bool,
}

impl Cidr {
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s == "all" {
            return Some(Cidr {
                addr: IpAddr::from([0, 0, 0, 0]),
                prefix: 0,
                all: true,
            });
        }
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = a.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match p {
            Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max)?,
            None => max,
        };
        Some(Cidr {
            addr,
            prefix,
            all: false,
        })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        if self.all {
            return true;
        }
        match (self.addr, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                (u32::from(n) & mask) == (u32::from(i) & mask)
            }
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                (u128::from(n) & mask) == (u128::from(i) & mask)
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EgressMode;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_default_policy_blocks_the_networks_around_the_space() {
        let p = pool_policy(EgressMode::DenyPrivate, &[], &[]).unwrap();
        for blocked in [
            "10.0.2.2",
            "172.17.0.1",
            "192.168.64.1",
            "100.101.102.103",
            "169.254.169.254",
            "198.18.0.1",
            "fd7a:115c:a1e0::1",
            "fe80::1",
        ] {
            assert!(p.blocks(ip(blocked)), "{blocked}");
        }
        for open in ["140.82.112.3", "1.1.1.1", "2606:4700::1111", "127.0.0.1"] {
            assert!(!p.blocks(ip(open)), "{open}");
        }
        assert!(pool_policy(EgressMode::Off, &[], &[]).is_none());
    }

    #[test]
    fn allow_wins_and_extra_denies_add() {
        let p = pool_policy(
            EgressMode::DenyPrivate,
            &["203.0.113.0/24".into()],
            &["10.1.2.3".into()],
        )
        .unwrap();
        assert!(p.blocks(ip("203.0.113.9")));
        assert!(!p.blocks(ip("10.1.2.3")));
        assert!(p.blocks(ip("10.1.2.4")));
        assert!(p.check().is_ok());
        let bad = EgressPolicy {
            allow: vec![],
            deny: vec!["github.com".into()],
        };
        assert!(bad.check().unwrap_err().contains("github.com"));
    }

    #[test]
    fn a_blocked_resolver_is_allowed_exactly() {
        let p = pool_policy(EgressMode::DenyPrivate, &[], &[]).unwrap();
        let conf = "# comment\nnameserver 10.0.2.3\nnameserver 127.0.0.53\nnameserver 8.8.8.8\n\
                    nameserver 10.0.2.3\nnameserver fd00::53\nsearch lan\n";
        assert_eq!(
            resolver_allowances(conf, &p),
            ["10.0.2.3/32", "fd00::53/128"]
        );
        // A microVM's resolver is its gateway.
        assert_eq!(
            resolver_allowances("nameserver 172.16.0.1", &p),
            ["172.16.0.1/32"]
        );
        let mut with = p.clone();
        with.allow.extend(resolver_allowances(conf, &p));
        assert!(!with.blocks(ip("10.0.2.3")));
        assert!(with.blocks(ip("10.0.2.2")));
    }
}
