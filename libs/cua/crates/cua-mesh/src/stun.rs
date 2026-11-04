// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The STUN binding exchange (RFC 8489), just enough to learn the public
//! address a NAT maps our UDP socket to: the request, the success response
//! with `XOR-MAPPED-ADDRESS`, and the responder side the coordinator runs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAGIC_COOKIE: u32 = 0x2112_a442;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const ATTR_SOFTWARE: u16 = 0x8022;

/// A STUN transaction id.
pub type TxId = [u8; 12];

/// A binding request with transaction id `tx`.
pub fn request(tx: TxId) -> Vec<u8> {
    let mut p = Vec::with_capacity(20);
    p.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    p.extend_from_slice(&tx);
    p
}

/// True when `p` looks like a STUN message (the two zero bits, the magic
/// cookie and a plausible length). WireGuard and disco packets never do.
pub fn is_stun(p: &[u8]) -> bool {
    p.len() >= 20
        && p[0] & 0xc0 == 0
        && u32::from_be_bytes([p[4], p[5], p[6], p[7]]) == MAGIC_COOKIE
        && (u16::from_be_bytes([p[2], p[3]]) as usize) == p.len() - 20
}

/// The transaction id of a binding request (`None` for anything else).
pub fn parse_request(p: &[u8]) -> Option<TxId> {
    if !is_stun(p) || u16::from_be_bytes([p[0], p[1]]) != BINDING_REQUEST {
        return None;
    }
    p[8..20].try_into().ok()
}

/// The success response to a binding request from `observed`.
pub fn response(tx: TxId, observed: SocketAddr) -> Vec<u8> {
    let mut attr = Vec::new();
    let xport = observed.port() ^ (MAGIC_COOKIE >> 16) as u16;
    match observed.ip() {
        IpAddr::V4(v4) => {
            attr.extend_from_slice(&[0, 0x01]);
            attr.extend_from_slice(&xport.to_be_bytes());
            let x = u32::from(v4) ^ MAGIC_COOKIE;
            attr.extend_from_slice(&x.to_be_bytes());
        }
        IpAddr::V6(v6) => {
            attr.extend_from_slice(&[0, 0x02]);
            attr.extend_from_slice(&xport.to_be_bytes());
            let mut key = [0u8; 16];
            key[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
            key[4..].copy_from_slice(&tx);
            for (o, k) in v6.octets().iter().zip(key) {
                attr.push(o ^ k);
            }
        }
    }
    let software = b"cua-mesh";
    let len = 4 + attr.len() + 4 + software.len();
    let mut p = Vec::with_capacity(20 + len);
    p.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    p.extend_from_slice(&tx);
    p.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
    p.extend_from_slice(&(attr.len() as u16).to_be_bytes());
    p.extend_from_slice(&attr);
    p.extend_from_slice(&ATTR_SOFTWARE.to_be_bytes());
    p.extend_from_slice(&(software.len() as u16).to_be_bytes());
    p.extend_from_slice(software);
    p
}

/// Parses a binding success response: its transaction id and the mapped
/// address (`XOR-MAPPED-ADDRESS`, else `MAPPED-ADDRESS`).
pub fn parse_response(p: &[u8]) -> Option<(TxId, SocketAddr)> {
    if !is_stun(p) || u16::from_be_bytes([p[0], p[1]]) != BINDING_SUCCESS {
        return None;
    }
    let tx: TxId = p[8..20].try_into().ok()?;
    let mut rest = &p[20..];
    let mut plain = None;
    while rest.len() >= 4 {
        let kind = u16::from_be_bytes([rest[0], rest[1]]);
        let len = u16::from_be_bytes([rest[2], rest[3]]) as usize;
        let value = rest.get(4..4 + len)?;
        match kind {
            ATTR_XOR_MAPPED_ADDRESS => return Some((tx, decode_addr(value, Some(&tx))?)),
            ATTR_MAPPED_ADDRESS => plain = decode_addr(value, None),
            _ => {}
        }
        let padded = (len + 3) & !3;
        rest = rest.get(4 + padded..).unwrap_or(&[]);
    }
    plain.map(|a| (tx, a))
}

fn decode_addr(v: &[u8], xor_tx: Option<&TxId>) -> Option<SocketAddr> {
    if v.len() < 4 {
        return None;
    }
    let mut port = u16::from_be_bytes([v[2], v[3]]);
    if xor_tx.is_some() {
        port ^= (MAGIC_COOKIE >> 16) as u16;
    }
    match v[1] {
        0x01 if v.len() >= 8 => {
            let mut ip = u32::from_be_bytes([v[4], v[5], v[6], v[7]]);
            if xor_tx.is_some() {
                ip ^= MAGIC_COOKIE;
            }
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), port))
        }
        0x02 if v.len() >= 20 => {
            let mut o: [u8; 16] = v[4..20].try_into().ok()?;
            if let Some(tx) = xor_tx {
                let mut key = [0u8; 16];
                key[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
                key[4..].copy_from_slice(tx);
                for (b, k) in o.iter_mut().zip(key) {
                    *b ^= k;
                }
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(o)), port))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_round_trip() {
        for observed in ["203.0.113.9:61000", "[2001:db8::42]:3478"] {
            let observed: SocketAddr = observed.parse().unwrap();
            let tx: TxId = rand::random();
            let req = request(tx);
            assert!(is_stun(&req));
            assert_eq!(parse_request(&req), Some(tx));
            let resp = response(tx, observed);
            assert!(is_stun(&resp));
            assert_eq!(parse_response(&resp), Some((tx, observed)));
            assert_eq!(parse_request(&resp), None);
        }
    }

    #[test]
    fn known_vector() {
        // RFC 5769 section 2.2 (IPv4 response), attributes other than
        // XOR-MAPPED-ADDRESS removed: 192.0.2.1:32853.
        let mut p = vec![0x01, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xa4, 0x42];
        p.extend_from_slice(&[
            0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae,
        ]);
        p.extend_from_slice(&[
            0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0xa1, 0x47, 0xe1, 0x12, 0xa6, 0x43,
        ]);
        let (_, a) = parse_response(&p).unwrap();
        assert_eq!(a, "192.0.2.1:32853".parse().unwrap());
    }

    #[test]
    fn not_stun() {
        assert!(!is_stun(b"CUAD"));
        let mut wg = vec![4u8, 0, 0, 0];
        wg.extend_from_slice(&[0; 28]);
        assert!(!is_stun(&wg));
    }
}
