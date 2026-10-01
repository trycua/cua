// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The TCP peer of a request, and which peers a host in direct mode takes
//! host calls from.
//!
//! [`crate::Server::serve`] puts a [`PeerAddr`] on every request it
//! accepts. A request with none came in process (tests, embedders).
//!
//! The direct listener is plaintext with the host's env token as a bearer,
//! so a host that provides Spaces on it answers `HostSpacesService` only to
//! [`is_private_address`] peers unless its policy says
//! `allow_any_address`. A relayed call reaches the listener over loopback.

use std::net::{IpAddr, SocketAddr};

/// The TCP peer a request arrived from (a request extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

/// The peer of `request`, when it came over TCP.
pub fn peer_of<T>(request: &tonic::Request<T>) -> Option<SocketAddr> {
    request.extensions().get::<PeerAddr>().map(|p| p.0)
}

/// Loopback, Tailscale (100.64.0.0/10 and fd7a:115c:a1e0::/48) or a
/// private LAN address (10/8, 172.16/12, 192.168/16, fc00::/7). An
/// IPv4-mapped IPv6 address counts as its IPv4 address. (The same rule as
/// `cua_host::direct::is_private_address`, which sets the host up.)
pub fn is_private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || (a == 100 && (b & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_address(IpAddr::V4(v4));
            }
            let s = v6.segments();
            v6.is_loopback()
                || (s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0)
                || (s[0] & 0xfe00) == 0xfc00
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_peers() {
        for a in [
            "127.0.0.1",
            "::1",
            "100.64.0.1",
            "100.127.1.2",
            "10.1.2.3",
            "172.20.0.1",
            "192.168.0.10",
            "fd7a:115c:a1e0:ab12::1",
            "fdab::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_private_address(a.parse().unwrap()), "{a}");
        }
        for a in [
            "1.1.1.1",
            "100.128.0.1",
            "203.0.113.7",
            "2606:4700::1111",
            "::ffff:1.1.1.1",
        ] {
            assert!(!is_private_address(a.parse().unwrap()), "{a}");
        }
    }

    #[test]
    fn the_peer_rides_the_request() {
        let mut r = tonic::Request::new(());
        assert_eq!(peer_of(&r), None);
        let addr: SocketAddr = "100.64.1.2:5555".parse().unwrap();
        r.extensions_mut().insert(PeerAddr(addr));
        assert_eq!(peer_of(&r), Some(addr));
    }
}
