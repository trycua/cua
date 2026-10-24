// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which reverse proxies in front of the relay may tell it a client's
//! address (`X-Forwarded-For`).
//!
//! The header is only as honest as whoever wrote it. A relay reached
//! directly lets the client write the whole header, so by default
//! (`--trusted-proxies` empty) it is ignored and the socket peer is the
//! client. When the socket peer is a configured proxy, the address is the
//! right-most `X-Forwarded-For` entry that is not itself a configured proxy.

use std::net::IpAddr;
use std::str::FromStr;

/// One CIDR block (`10.42.0.0/16`, `::1/128`, or a bare address).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                mask_v4(self.prefix) & u32::from(net) == mask_v4(self.prefix) & u32::from(ip)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                mask_v6(self.prefix) & u128::from(net) == mask_v6(self.prefix) & u128::from(ip)
            }
            _ => false,
        }
    }
}

fn mask_v4(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
}

fn mask_v6(prefix: u8) -> u128 {
    u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0)
}

impl FromStr for Cidr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = addr
            .parse()
            .map_err(|_| format!("`{s}` is not an IP address or CIDR"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("`{s}`: prefix length must be 0..={max}"))?,
            None => max,
        };
        Ok(Self { addr, prefix })
    }
}

/// The proxies allowed to set `X-Forwarded-For`. Empty: none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustedProxies(Vec<Cidr>);

impl TrustedProxies {
    /// Parses a comma-separated CIDR list (empty entries are skipped).
    pub fn parse(list: &str) -> Result<Self, String> {
        list.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::parse)
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn trusts(&self, ip: IpAddr) -> bool {
        let ip = unmap(ip);
        self.0.iter().any(|c| c.contains(ip))
    }

    /// The client address of a request whose TCP peer is `peer` and whose
    /// `X-Forwarded-For` header lines are `xff` (in order).
    ///
    /// Without a trusted peer the header is ignored. Otherwise: the
    /// right-most entry that is not a trusted proxy. If that entry is not an
    /// address (a proxy that does not append addresses), or every entry is a
    /// proxy, the peer is the answer.
    pub fn client_ip<'a>(&self, peer: IpAddr, xff: impl Iterator<Item = &'a str>) -> IpAddr {
        let peer = unmap(peer);
        if !self.trusts(peer) {
            return peer;
        }
        let entries: Vec<&str> = xff.flat_map(|h| h.split(',')).map(str::trim).collect();
        for entry in entries.iter().rev() {
            match entry.parse::<IpAddr>() {
                Ok(ip) if self.trusts(ip) => continue,
                Ok(ip) => return unmap(ip),
                Err(_) => return peer,
            }
        }
        peer
    }
}

fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }
    fn tp(s: &str) -> TrustedProxies {
        TrustedProxies::parse(s).unwrap()
    }
    fn get(t: &TrustedProxies, peer: &str, xff: &[&str]) -> IpAddr {
        t.client_ip(ip(peer), xff.iter().copied())
    }

    #[test]
    fn no_trusted_proxies_ignores_spoofed_header() {
        let t = TrustedProxies::default();
        assert_eq!(get(&t, "198.51.100.4", &["1.2.3.4"]), ip("198.51.100.4"));
        assert_eq!(
            get(&t, "198.51.100.4", &["1.2.3.4, 5.6.7.8"]),
            ip("198.51.100.4")
        );
    }

    #[test]
    fn untrusted_peer_cannot_use_the_header_even_with_proxies_configured() {
        let t = tp("10.42.0.0/16");
        assert_eq!(get(&t, "198.51.100.4", &["1.2.3.4"]), ip("198.51.100.4"));
    }

    #[test]
    fn trusted_proxy_gives_the_forwarded_client() {
        let t = tp("10.42.0.0/16");
        assert_eq!(get(&t, "10.42.1.9", &["203.0.113.7"]), ip("203.0.113.7"));
    }

    #[test]
    fn a_client_supplied_prefix_does_not_win() {
        // The client sent "9.9.9.9"; the proxy appended the real address.
        let t = tp("10.42.0.0/16");
        assert_eq!(
            get(&t, "10.42.1.9", &["9.9.9.9, 203.0.113.7"]),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn chain_skips_trusted_hops() {
        let t = tp("10.42.0.0/16, 192.0.2.0/24");
        assert_eq!(
            get(
                &t,
                "10.42.1.9",
                &["9.9.9.9, 203.0.113.7, 192.0.2.5, 10.42.3.3"]
            ),
            ip("203.0.113.7")
        );
        // Several header lines count as one list.
        assert_eq!(
            get(&t, "10.42.1.9", &["9.9.9.9", "203.0.113.7, 192.0.2.5"]),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn malformed_or_missing_falls_back_to_the_peer() {
        let t = tp("10.42.0.0/16");
        assert_eq!(get(&t, "10.42.1.9", &[]), ip("10.42.1.9"));
        assert_eq!(get(&t, "10.42.1.9", &["not-an-ip"]), ip("10.42.1.9"));
        assert_eq!(
            get(&t, "10.42.1.9", &["203.0.113.7, garbage"]),
            ip("10.42.1.9")
        );
        assert_eq!(get(&t, "10.42.1.9", &[""]), ip("10.42.1.9"));
        assert_eq!(get(&t, "10.42.1.9", &["10.42.2.2"]), ip("10.42.1.9"));
    }

    #[test]
    fn ipv6_and_mapped_addresses() {
        let t = tp("::1/128, fd00::/8");
        assert_eq!(get(&t, "::1", &["2001:db8::7"]), ip("2001:db8::7"));
        assert_eq!(
            get(&t, "fd00::5", &["2001:db8::7, fd00::9"]),
            ip("2001:db8::7")
        );
        let t = tp("127.0.0.1/32");
        assert_eq!(
            get(&t, "::ffff:127.0.0.1", &["203.0.113.7"]),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn cidr_parsing() {
        assert!(TrustedProxies::parse("").unwrap().is_empty());
        assert!(TrustedProxies::parse("10.0.0.0/33").is_err());
        assert!(TrustedProxies::parse("10.0.0.0/x").is_err());
        assert!(TrustedProxies::parse("nope").is_err());
        let t = tp("10.0.0.1, 0.0.0.0/0");
        assert!(t.trusts(ip("8.8.8.8")));
        let t = tp("10.0.0.1");
        assert!(t.trusts(ip("10.0.0.1")) && !t.trusts(ip("10.0.0.2")));
    }
}
