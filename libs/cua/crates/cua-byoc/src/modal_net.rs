// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A Space's network rules as a Modal sandbox's outbound network access.
//!
//! Modal enforces outbound access on its side, outside the guest, as an
//! allow-list of CIDRs and domains (or all of it blocked). A Space's rules
//! are "everything public except a list of denies", so this module turns
//! them into the allow-list that means the same:
//!
//! - the **internet** default is the public IPv4 space minus the platform
//!   and default denies ([`PLATFORM_DENIES`], [`DEFAULT_DENIES`]), as the
//!   smallest list of CIDRs that covers exactly that ([`internet_cidrs`]);
//! - **none** allows nothing but the relay (the Space's own channel: the
//!   guest dials out to it, so blocking it outright would leave a Space that
//!   never comes online) by domain;
//! - **custom** rules start from either of those, remove the user's denies,
//!   add the user's allows and then remove the platform denies again (an
//!   allow never beats a platform deny);
//! - a **domain** rule is refused (the rules the Space keeps are by address,
//!   and Modal resolves a domain allow-list itself), and so are IPv6
//!   entries (the allow-list is IPv4; IPv6 stays blocked).
//!
//! The relay's domain is always allowed, in every mode.

use cua_sandbox_core::Error;
use cua_vmm::egress::{EgressPolicy, Net, Rules};

use crate::api::Result;

/// Never reachable from a Space, whatever it asks: cloud metadata and
/// credential endpoints.
pub const PLATFORM_DENIES: &[&str] = &[
    "169.254.169.254/32",
    "169.254.170.2/32",
    "168.63.129.16/32",
    "100.100.100.200/32",
];

/// Not the public internet: blocked unless the Space lifts them.
pub const DEFAULT_DENIES: &[&str] = &[
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

/// Where a custom rule set starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// The public internet (the default).
    Internet,
    /// Nothing.
    None,
}

/// A Space's network rules, as far as Modal is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkRequest {
    /// Where the rules start.
    pub base: Base,
    /// The user's allows and denies.
    pub rules: Rules,
    /// The relay's host name: always reachable.
    pub relay_host: String,
}

/// What Modal is told: an allow-list (`Open` is no restriction).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Outbound {
    /// No restriction (the platform denies are not applicable here).
    #[default]
    Open,
    /// Only these CIDRs and domains.
    Allowlist {
        /// IPv4 CIDRs.
        cidrs: Vec<String>,
        /// Domains (the relay).
        domains: Vec<String>,
    },
}

/// The request for a Space with `egress` rules (and `no_network`), or
/// `None` when it asks for nothing and is not held to the secure default
/// (`protect`: "internet only" is kept even with no rules). A hostname rule
/// is refused.
pub fn request(
    egress: &EgressPolicy,
    no_network: bool,
    protect: bool,
    relay_url: &str,
) -> Result<Option<NetworkRequest>> {
    if egress.is_empty() && !no_network && !protect {
        return Ok(None);
    }
    let rules = egress.parse().map_err(|e| {
        Error::InvalidArgument(format!(
            "Modal takes network rules by address (an IP or a CIDR), not by name: {e}"
        ))
    })?;
    let relay_host = host_of(relay_url).ok_or_else(|| {
        Error::InvalidArgument("the relay has no host name to keep reachable".into())
    })?;
    Ok(Some(NetworkRequest {
        base: if no_network {
            Base::None
        } else {
            Base::Internet
        },
        rules,
        relay_host,
    }))
}

/// The host of an `https://host[:port]/path` URL.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?.trim();
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Modal's outbound access for `req`.
pub fn outbound(req: &NetworkRequest) -> Result<Outbound> {
    // IPv4 as half-open ranges of the 32-bit space.
    let all = vec![(0u64, 1u64 << 32)];
    let mut set = match req.base {
        Base::Internet => subtract(&all, &ranges(DEFAULT_DENIES)),
        Base::None => vec![],
    };
    let listed = |accept: bool| -> Vec<Net> {
        req.rules
            .rules
            .iter()
            .filter(|r| r.accept == accept)
            .map(|r| r.net)
            .collect()
    };
    let denies = v4_ranges(&listed(false), false)?;
    set = subtract(&set, &denies);
    let allows = v4_ranges(&listed(true), true)?;
    set = union(&set, &allows);
    set = subtract(&set, &ranges(PLATFORM_DENIES));
    Ok(Outbound::Allowlist {
        cidrs: cidrs_of(&set),
        domains: vec![req.relay_host.clone()],
    })
}

/// The CIDRs of the plain internet default: the public IPv4 space minus the
/// platform and default denies.
pub fn internet_cidrs() -> Vec<String> {
    let all = vec![(0u64, 1u64 << 32)];
    let mut denied = ranges(DEFAULT_DENIES);
    denied.extend(ranges(PLATFORM_DENIES));
    cidrs_of(&subtract(&all, &normalize(denied)))
}

fn parse_v4(s: &str) -> (u64, u64) {
    let (a, p) = s.split_once('/').expect("a CIDR");
    let a: std::net::Ipv4Addr = a.parse().expect("an IPv4 address");
    let p: u32 = p.parse().expect("a prefix");
    let start = u64::from(u32::from(a));
    (start, start + (1u64 << (32 - p)))
}

fn ranges(list: &[&str]) -> Vec<(u64, u64)> {
    normalize(list.iter().map(|s| parse_v4(s)).collect())
}

/// The IPv4 entries of `nets` as ranges. IPv6 `::/0` (the other half of
/// `all`) is skipped; another IPv6 entry is refused when it is an allow
/// (Modal's allow-list is IPv4) and skipped when it is a deny (IPv6 is
/// blocked anyway).
fn v4_ranges(nets: &[Net], allow: bool) -> Result<Vec<(u64, u64)>> {
    let mut out = Vec::new();
    for n in nets {
        match n {
            Net::V4(a, p) => {
                let start = u64::from(u32::from(*a));
                out.push((start, start + (1u64 << (32 - u32::from(*p)))));
            }
            Net::V6(_, 0) => {}
            Net::V6(..) if !allow => {}
            Net::V6(..) => {
                return Err(Error::InvalidArgument(format!(
                    "Modal's network allow-list is IPv4 only: {} cannot be allowed",
                    n.cidr()
                )));
            }
        }
    }
    Ok(normalize(out))
}

/// Sorted, merged ranges.
fn normalize(mut r: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    r.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (s, e) in r {
        match out.last_mut() {
            Some(l) if s <= l.1 => l.1 = l.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

fn union(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    normalize(a.iter().chain(b).copied().collect())
}

/// `a` without `b` (both normalized).
fn subtract(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for &(mut s, e) in a {
        for &(bs, be) in b {
            if be <= s || bs >= e {
                continue;
            }
            if bs > s {
                out.push((s, bs));
            }
            s = s.max(be);
            if s >= e {
                break;
            }
        }
        if s < e {
            out.push((s, e));
        }
    }
    out
}

/// The fewest CIDRs that cover `set` exactly.
fn cidrs_of(set: &[(u64, u64)]) -> Vec<String> {
    let mut out = Vec::new();
    for &(mut s, e) in set {
        while s < e {
            let align = if s == 0 {
                32
            } else {
                s.trailing_zeros().min(32)
            };
            let mut size = align;
            while (1u64 << size) > e - s {
                size -= 1;
            }
            out.push(format!(
                "{}/{}",
                std::net::Ipv4Addr::from(s as u32),
                32 - size
            ));
            s += 1u64 << size;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn covered(cidrs: &[String], ip: Ipv4Addr) -> bool {
        cidrs.iter().any(|c| {
            let (s, e) = parse_v4(c);
            let v = u64::from(u32::from(ip));
            v >= s && v < e
        })
    }

    fn total(cidrs: &[String]) -> u64 {
        cidrs
            .iter()
            .map(|c| {
                let (s, e) = parse_v4(c);
                e - s
            })
            .sum()
    }

    #[test]
    fn the_internet_list_covers_every_public_address_and_no_denied_one() {
        let cidrs = internet_cidrs();
        let mut denied = ranges(DEFAULT_DENIES);
        denied.extend(ranges(PLATFORM_DENIES));
        let denied = normalize(denied);
        let denied_total: u64 = denied.iter().map(|(s, e)| e - s).sum();
        // Exact: the list is the whole space minus the denies, no more, no less.
        assert_eq!(total(&cidrs), (1u64 << 32) - denied_total);
        // Disjoint: no address is counted twice.
        let listed = normalize(cidrs.iter().map(|c| parse_v4(c)).collect());
        assert_eq!(
            listed.iter().map(|(s, e)| e - s).sum::<u64>(),
            total(&cidrs)
        );
        for ip in [
            "10.0.0.1",
            "10.255.255.255",
            "127.0.0.1",
            "169.254.169.254",
            "169.254.170.2",
            "168.63.129.16",
            "100.100.100.200",
            // The shared-address block (tailnets, carrier NAT) is blocked whole.
            "100.100.100.199",
            "100.100.100.201",
            "100.64.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "198.18.0.1",
            "192.0.0.8",
        ] {
            assert!(
                !covered(&cidrs, ip.parse().unwrap()),
                "{ip} must be blocked"
            );
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "9.255.255.255",
            "11.0.0.0",
            "100.63.255.255",
            "100.128.0.0",
            "168.63.129.15",
            "168.63.129.17",
            "169.253.255.255",
            "169.255.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.0.1.0",
            "192.167.255.255",
            "192.169.0.0",
            "198.17.255.255",
            "198.20.0.0",
            "223.255.255.255",
        ] {
            assert!(
                covered(&cidrs, ip.parse().unwrap()),
                "{ip} must be reachable"
            );
        }
    }

    #[test]
    fn the_internet_list_is_small_and_minimal() {
        let cidrs = internet_cidrs();
        // Small: the reserved blocks and the platform addresses cut the public
        // space into about a hundred CIDRs.
        assert!(cidrs.len() < 128, "{} CIDRs: {cidrs:?}", cidrs.len());
        // Minimal: no two neighbours of the same size could be one block.
        let r: Vec<(u64, u64)> = cidrs.iter().map(|c| parse_v4(c)).collect();
        for w in r.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (sa, sb) = (a.1 - a.0, b.1 - b.0);
            let merged = sa == sb && a.1 == b.0 && a.0 % (sa * 2) == 0;
            assert!(!merged, "{a:?} and {b:?} could be one CIDR");
        }
        // Sorted, no duplicates.
        assert!(r.windows(2).all(|w| w[0].1 <= w[1].0));
    }

    fn req(base: Base, allow: &[&str], deny: &[&str]) -> NetworkRequest {
        let p = EgressPolicy {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
        };
        NetworkRequest {
            base,
            rules: p.parse().unwrap(),
            relay_host: "relay.cua.ai".into(),
        }
    }

    fn cidrs(o: Outbound) -> (Vec<String>, Vec<String>) {
        match o {
            Outbound::Allowlist { cidrs, domains } => (cidrs, domains),
            Outbound::Open => panic!("open"),
        }
    }

    #[test]
    fn none_allows_only_the_relay_by_name() {
        let (c, d) = cidrs(outbound(&req(Base::None, &[], &[])).unwrap());
        assert!(c.is_empty());
        assert_eq!(d, ["relay.cua.ai"]);
    }

    #[test]
    fn a_custom_deny_is_cut_out_and_an_allow_is_added_back() {
        let (c, d) = cidrs(outbound(&req(Base::Internet, &[], &["8.8.8.0/24"])).unwrap());
        assert!(!covered(&c, "8.8.8.8".parse().unwrap()));
        assert!(covered(&c, "8.8.9.1".parse().unwrap()));
        assert!(covered(&c, "1.1.1.1".parse().unwrap()));
        assert_eq!(d, ["relay.cua.ai"]);
        // deny all + allow one address: only that address.
        let (c, _) = cidrs(outbound(&req(Base::Internet, &["1.1.1.1"], &["all"])).unwrap());
        assert_eq!(c, ["1.1.1.1/32"]);
        // The none base with an allow.
        let (c, _) = cidrs(outbound(&req(Base::None, &["93.184.216.0/24"], &[])).unwrap());
        assert_eq!(c, ["93.184.216.0/24"]);
    }

    #[test]
    fn an_allow_lifts_a_default_deny_but_never_a_platform_deny() {
        let (c, _) = cidrs(
            outbound(&req(
                Base::Internet,
                &["10.1.0.0/16", "169.254.0.0/16"],
                &[],
            ))
            .unwrap(),
        );
        assert!(covered(&c, "10.1.2.3".parse().unwrap()));
        assert!(!covered(&c, "10.2.0.1".parse().unwrap()));
        assert!(covered(&c, "169.254.1.1".parse().unwrap()));
        assert!(!covered(&c, "169.254.169.254".parse().unwrap()));
        assert!(!covered(&c, "169.254.170.2".parse().unwrap()));
    }

    #[test]
    fn names_and_ipv6_allows_are_refused() {
        let p = EgressPolicy {
            allow: vec!["example.com".into()],
            deny: vec![],
        };
        let e = request(&p, false, false, "https://relay.cua.ai").unwrap_err();
        assert!(e.to_string().contains("by address"), "{e}");
        let r = req(Base::Internet, &["2001:db8::/32"], &[]);
        let e = outbound(&r).unwrap_err();
        assert!(e.to_string().contains("IPv4 only"), "{e}");
        // `all` carries an IPv6 half that is fine; an IPv6 deny is a no-op.
        assert!(outbound(&req(Base::Internet, &[], &["all"])).is_ok());
        assert!(outbound(&req(Base::Internet, &[], &["fd00::/8"])).is_ok());
    }

    #[test]
    fn no_rules_means_no_request_and_the_relay_host_is_read() {
        assert_eq!(
            request(&EgressPolicy::default(), false, false, "https://r.cua.ai").unwrap(),
            None
        );
        // Held to the secure default: internet only is kept even with no rules.
        let r = request(&EgressPolicy::default(), false, true, "https://r.cua.ai")
            .unwrap()
            .unwrap();
        assert_eq!(r.base, Base::Internet);
        assert!(r.rules.is_empty());
        let r = request(
            &EgressPolicy::default(),
            true,
            false,
            "https://Relay.cua.ai:8443/x?y",
        )
        .unwrap()
        .unwrap();
        assert_eq!(r.base, Base::None);
        assert_eq!(r.relay_host, "relay.cua.ai");
        assert!(request(&EgressPolicy::default(), true, false, "").is_err());
    }
}
