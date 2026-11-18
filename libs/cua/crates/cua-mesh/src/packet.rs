// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The IP packets inside the tunnel: IPv6 + UDP between mesh addresses.
//!
//! WireGuard carries IP packets, and the receiver checks each packet's inner
//! source address against the sending peer's key (cryptokey routing). The
//! mesh only ever carries UDP (QUIC streams and forwarded UDP), so this is
//! the whole IP layer: a fixed 40-byte IPv6 header and an 8-byte UDP header.

use std::net::{Ipv6Addr, SocketAddrV6};

/// IPv6 header length.
pub const IPV6_HEADER: usize = 40;
/// UDP header length.
pub const UDP_HEADER: usize = 8;
/// Bytes of headers before a UDP payload.
pub const OVERHEAD: usize = IPV6_HEADER + UDP_HEADER;
/// The tunnel MTU (inner IP packet), as WireGuard's default.
pub const TUNNEL_MTU: usize = 1420;
/// The largest UDP payload one tunnel packet carries.
pub const MAX_UDP_PAYLOAD: usize = TUNNEL_MTU - OVERHEAD;

const NEXT_HEADER_UDP: u8 = 17;

/// Builds an IPv6/UDP packet. Returns `None` when `payload` is larger than
/// [`MAX_UDP_PAYLOAD`].
pub fn build_udp(src: SocketAddrV6, dst: SocketAddrV6, payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() > MAX_UDP_PAYLOAD {
        return None;
    }
    let udp_len = (UDP_HEADER + payload.len()) as u16;
    let mut p = Vec::with_capacity(OVERHEAD + payload.len());
    // Version 6, traffic class 0, flow label 0.
    p.extend_from_slice(&[0x60, 0, 0, 0]);
    p.extend_from_slice(&udp_len.to_be_bytes());
    p.push(NEXT_HEADER_UDP);
    p.push(64); // hop limit
    p.extend_from_slice(&src.ip().octets());
    p.extend_from_slice(&dst.ip().octets());
    p.extend_from_slice(&src.port().to_be_bytes());
    p.extend_from_slice(&dst.port().to_be_bytes());
    p.extend_from_slice(&udp_len.to_be_bytes());
    p.extend_from_slice(&[0, 0]); // checksum, filled below
    p.extend_from_slice(payload);
    let sum = udp_checksum(src.ip(), dst.ip(), &p[IPV6_HEADER..]);
    p[IPV6_HEADER + 6..IPV6_HEADER + 8].copy_from_slice(&sum.to_be_bytes());
    Some(p)
}

/// A parsed IPv6/UDP packet.
#[derive(Debug, PartialEq, Eq)]
pub struct UdpPacket<'a> {
    /// Source.
    pub src: SocketAddrV6,
    /// Destination.
    pub dst: SocketAddrV6,
    /// UDP payload.
    pub payload: &'a [u8],
}

/// Parses an IPv6/UDP packet (no extension headers), verifying lengths and
/// the UDP checksum. Anything else is `None`.
pub fn parse_udp(p: &[u8]) -> Option<UdpPacket<'_>> {
    if p.len() < OVERHEAD || p[0] >> 4 != 6 || p[6] != NEXT_HEADER_UDP {
        return None;
    }
    let payload_len = u16::from_be_bytes([p[4], p[5]]) as usize;
    if IPV6_HEADER + payload_len > p.len() || payload_len < UDP_HEADER {
        return None;
    }
    let src_ip = Ipv6Addr::from(<[u8; 16]>::try_from(&p[8..24]).ok()?);
    let dst_ip = Ipv6Addr::from(<[u8; 16]>::try_from(&p[24..40]).ok()?);
    let udp = &p[IPV6_HEADER..IPV6_HEADER + payload_len];
    let udp_len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
    if udp_len != payload_len {
        return None;
    }
    // A correct checksum makes the one's-complement sum over the pseudo
    // header and the whole datagram all ones.
    if fold(sum_words(udp, pseudo_header(&src_ip, &dst_ip, udp.len()))) != 0xffff {
        return None;
    }
    let sport = u16::from_be_bytes([udp[0], udp[1]]);
    let dport = u16::from_be_bytes([udp[2], udp[3]]);
    Some(UdpPacket {
        src: SocketAddrV6::new(src_ip, sport, 0, 0),
        dst: SocketAddrV6::new(dst_ip, dport, 0, 0),
        payload: &udp[UDP_HEADER..],
    })
}

/// The source address of an IPv6 packet (for cryptokey routing checks).
pub fn source(p: &[u8]) -> Option<Ipv6Addr> {
    if p.len() < IPV6_HEADER || p[0] >> 4 != 6 {
        return None;
    }
    Some(Ipv6Addr::from(<[u8; 16]>::try_from(&p[8..24]).ok()?))
}

fn pseudo_header(src: &Ipv6Addr, dst: &Ipv6Addr, len: usize) -> u32 {
    let mut sum = 0u32;
    for chunk in src.octets().chunks(2).chain(dst.octets().chunks(2)) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    sum += (len as u32) >> 16;
    sum += (len as u32) & 0xffff;
    sum += NEXT_HEADER_UDP as u32;
    sum
}

fn sum_words(data: &[u8], mut sum: u32) -> u32 {
    let mut chunks = data.chunks_exact(2);
    for c in &mut chunks {
        sum = sum.wrapping_add(u16::from_be_bytes([c[0], c[1]]) as u32);
    }
    if let [last] = chunks.remainder() {
        sum = sum.wrapping_add((*last as u32) << 8);
    }
    sum
}

fn fold(mut sum: u32) -> u16 {
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum as u16
}

/// The UDP checksum over `udp` (header with a zero checksum field, plus
/// payload). Zero is sent as `0xffff` (IPv6 forbids a zero checksum).
fn udp_checksum(src: &Ipv6Addr, dst: &Ipv6Addr, udp: &[u8]) -> u16 {
    let mut sum = pseudo_header(src, dst, udp.len());
    // Skip the checksum field itself.
    sum = sum_words(&udp[..6], sum);
    sum = sum_words(&udp[8..], sum);
    let c = !fold(sum);
    if c == 0 { 0xffff } else { c }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(last: u16, port: u16) -> SocketAddrV6 {
        SocketAddrV6::new(
            Ipv6Addr::new(0xfd5c, 0xa000, 0, 0, 0, 0, 0, last),
            port,
            0,
            0,
        )
    }

    #[test]
    fn round_trip() {
        for len in [0usize, 1, 2, 3, 100, 1371, MAX_UDP_PAYLOAD] {
            let payload: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let p = build_udp(a(1, 4000), a(2, 7443), &payload).unwrap();
            assert_eq!(p.len(), OVERHEAD + len);
            let parsed = parse_udp(&p).unwrap();
            assert_eq!(parsed.src, a(1, 4000));
            assert_eq!(parsed.dst, a(2, 7443));
            assert_eq!(parsed.payload, &payload[..]);
            assert_eq!(source(&p), Some(*a(1, 0).ip()));
        }
        assert!(build_udp(a(1, 1), a(2, 2), &vec![0; MAX_UDP_PAYLOAD + 1]).is_none());
    }

    #[test]
    fn corruption_is_rejected() {
        let mut p = build_udp(a(1, 4000), a(2, 7443), b"hello mesh").unwrap();
        let last = p.len() - 1;
        p[last] ^= 0x40;
        assert!(parse_udp(&p).is_none());
        assert!(parse_udp(&p[..10]).is_none());
        let mut v4 = build_udp(a(1, 1), a(2, 2), b"x").unwrap();
        v4[0] = 0x45;
        assert!(parse_udp(&v4).is_none());
    }
}
