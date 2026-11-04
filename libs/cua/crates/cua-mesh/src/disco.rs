// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Path discovery ("disco") messages: ping, pong and call-me-maybe, sealed
//! between two nodes' disco keys so neither the relay nor anyone on the path
//! can forge or read them.
//!
//! ```text
//! packet  = "CUAD" | version(1) | sender disco key(32) | nonce(24) | sealed
//! sealed  = XChaCha20-Poly1305(key, nonce, message)
//! key     = HKDF-SHA256(X25519(own disco secret, peer disco key),
//!                       salt "cua-mesh-disco-v1", info min(keys)|max(keys))
//! message = Ping   1 | tx(12) | sender node key(32)
//!         | Pong   2 | tx(12) | observed source (16-byte IP, port u16)
//!         | CallMeMaybe 3 | endpoint(18)*
//! ```

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use chacha20poly1305::aead::{Aead as _, KeyInit as _};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::key::{PublicKey, SecretKey};

/// Every disco packet starts with this.
pub const MAGIC: &[u8; 4] = b"CUAD";
const VERSION: u8 = 1;
const HEADER: usize = 4 + 1 + 32 + 24;

/// A ping/pong transaction id.
pub type TxId = [u8; 12];

/// A disco message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Are you there on this path?
    Ping {
        /// Transaction id the pong echoes.
        tx: TxId,
        /// The sender's node key.
        node: PublicKey,
    },
    /// Yes; and this is the address your ping came from.
    Pong {
        /// The ping's transaction id.
        tx: TxId,
        /// Where the ping came from, as the receiver saw it.
        src: SocketAddr,
    },
    /// Ping me back on these endpoints (hole punching).
    CallMeMaybe {
        /// The sender's candidate endpoints.
        endpoints: Vec<SocketAddr>,
    },
}

/// True when `packet` looks like a disco packet.
pub fn is_disco(packet: &[u8]) -> bool {
    packet.len() >= HEADER && &packet[..4] == MAGIC
}

/// The sender's disco key of a disco packet.
pub fn sender(packet: &[u8]) -> Option<PublicKey> {
    if !is_disco(packet) || packet[4] != VERSION {
        return None;
    }
    Some(PublicKey(packet[5..37].try_into().ok()?))
}

fn shared_key(own: &SecretKey, peer: &PublicKey) -> [u8; 32] {
    let me = own.public();
    let (lo, hi) = if me.0 <= peer.0 {
        (me, *peer)
    } else {
        (*peer, me)
    };
    let mut info = [0u8; 64];
    info[..32].copy_from_slice(&lo.0);
    info[32..].copy_from_slice(&hi.0);
    let hk = Hkdf::<Sha256>::new(Some(b"cua-mesh-disco-v1"), &own.agree(peer));
    let mut okm = [0u8; 32];
    hk.expand(&info, &mut okm)
        .expect("32 bytes is a valid HKDF length");
    okm
}

/// A sealer for one peer (the X25519 and HKDF done once).
#[derive(Clone)]
pub struct Sealer {
    own: PublicKey,
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealer")
    }
}

impl Sealer {
    /// The sealer between `own` and `peer` disco keys.
    pub fn new(own: &SecretKey, peer: &PublicKey) -> Self {
        let key = shared_key(own, peer);
        Self {
            own: own.public(),
            cipher: XChaCha20Poly1305::new((&key).into()),
        }
    }

    /// Seals `msg` into a disco packet.
    pub fn seal(&self, msg: &Message) -> Vec<u8> {
        let nonce_bytes: [u8; 24] = crate::key::os_random();
        let nonce = XNonce::from_slice(&nonce_bytes);
        let sealed = self
            .cipher
            .encrypt(nonce, encode(msg).as_slice())
            .expect("XChaCha20-Poly1305 encryption does not fail");
        let mut out = Vec::with_capacity(HEADER + sealed.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.own.0);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&sealed);
        out
    }

    /// Opens a disco packet sealed by the peer. `None` when it does not
    /// authenticate or does not parse.
    pub fn open(&self, packet: &[u8]) -> Option<Message> {
        if !is_disco(packet) || packet[4] != VERSION {
            return None;
        }
        let nonce = XNonce::from_slice(&packet[37..HEADER]);
        let plain = self.cipher.decrypt(nonce, &packet[HEADER..]).ok()?;
        decode(&plain)
    }
}

fn put_addr(out: &mut Vec<u8>, a: &SocketAddr) {
    let ip = match a.ip() {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    };
    out.extend_from_slice(&ip.octets());
    out.extend_from_slice(&a.port().to_be_bytes());
}

fn get_addr(b: &[u8]) -> Option<SocketAddr> {
    let ip = Ipv6Addr::from(<[u8; 16]>::try_from(b.get(..16)?).ok()?);
    let port = u16::from_be_bytes([*b.get(16)?, *b.get(17)?]);
    let ip = match ip.to_ipv4_mapped() {
        Some(v4) => IpAddr::V4(v4),
        None => IpAddr::V6(ip),
    };
    Some(SocketAddr::new(ip, port))
}

fn encode(msg: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    match msg {
        Message::Ping { tx, node } => {
            out.push(1);
            out.extend_from_slice(tx);
            out.extend_from_slice(&node.0);
        }
        Message::Pong { tx, src } => {
            out.push(2);
            out.extend_from_slice(tx);
            put_addr(&mut out, src);
        }
        Message::CallMeMaybe { endpoints } => {
            out.push(3);
            for e in endpoints.iter().take(32) {
                put_addr(&mut out, e);
            }
        }
    }
    out
}

fn decode(b: &[u8]) -> Option<Message> {
    let (&kind, rest) = b.split_first()?;
    match kind {
        1 if rest.len() == 12 + 32 => Some(Message::Ping {
            tx: rest[..12].try_into().ok()?,
            node: PublicKey(rest[12..44].try_into().ok()?),
        }),
        2 if rest.len() == 12 + 18 => Some(Message::Pong {
            tx: rest[..12].try_into().ok()?,
            src: get_addr(&rest[12..])?,
        }),
        3 if rest.len() % 18 == 0 && rest.len() <= 32 * 18 => Some(Message::CallMeMaybe {
            endpoints: rest.chunks(18).filter_map(get_addr).collect(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open() {
        let a = SecretKey::generate();
        let b = SecretKey::generate();
        let ab = Sealer::new(&a, &b.public());
        let ba = Sealer::new(&b, &a.public());
        let msgs = [
            Message::Ping {
                tx: [7; 12],
                node: SecretKey::generate().public(),
            },
            Message::Pong {
                tx: [9; 12],
                src: "203.0.113.7:41641".parse().unwrap(),
            },
            Message::Pong {
                tx: [1; 12],
                src: "[2001:db8::1]:5".parse().unwrap(),
            },
            Message::CallMeMaybe {
                endpoints: vec![
                    "192.168.1.5:41641".parse().unwrap(),
                    "[2001:db8::2]:41641".parse().unwrap(),
                ],
            },
        ];
        for m in msgs {
            let p = ab.seal(&m);
            assert!(is_disco(&p));
            assert_eq!(sender(&p), Some(a.public()));
            assert_eq!(ba.open(&p), Some(m));
        }
    }

    #[test]
    fn forgeries_fail() {
        let a = SecretKey::generate();
        let b = SecretKey::generate();
        let c = SecretKey::generate();
        let p = Sealer::new(&a, &b.public()).seal(&Message::CallMeMaybe { endpoints: vec![] });
        // A third party cannot open it.
        assert!(Sealer::new(&c, &a.public()).open(&p).is_none());
        let mut tampered = p.clone();
        let n = tampered.len();
        tampered[n - 1] ^= 1;
        assert!(Sealer::new(&b, &a.public()).open(&tampered).is_none());
    }
}
