// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The host name a Minecraft Java client dialed, read from its first
//! packet (the handshake), so a public share can drop connections that did
//! not come for it: scanners sweeping ports, or a stale address now given
//! to another share. The same check runs on a Space's own cloud host
//! (cua-spacesd's share slots) and on a share edge.
//!
//! The handshake is `VarInt length`, `VarInt 0x00`, `VarInt protocol`,
//! `String address` (`VarInt` byte length + UTF-8), `u16 port`,
//! `VarInt next state`. Forge appends `\0FML…\0` to the address and some
//! clients a trailing dot; both are removed.

/// What the first bytes of a connection say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handshake {
    /// Not enough bytes yet.
    NeedMore,
    /// A Java handshake for this host (lowercase, no dot or suffix).
    Host(String),
    /// Not a Java handshake (a legacy ping, another protocol, garbage).
    NotMinecraft,
}

/// The most bytes read before giving up on a handshake.
pub const MAX_HANDSHAKE: usize = 512;

fn varint(b: &[u8], at: &mut usize) -> Option<Option<u32>> {
    let mut v: u32 = 0;
    for i in 0..5 {
        let Some(&byte) = b.get(*at) else {
            return None; // need more
        };
        *at += 1;
        v |= u32::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some(Some(v));
        }
    }
    Some(None) // too long: not a VarInt
}

/// Parses the start of a connection.
pub fn parse(b: &[u8]) -> Handshake {
    if b.first() == Some(&0xFE) {
        return Handshake::NotMinecraft; // legacy (pre-1.7) ping
    }
    let mut at = 0;
    let len = match varint(b, &mut at) {
        None => return need(b),
        Some(None) => return Handshake::NotMinecraft,
        Some(Some(l)) => l as usize,
    };
    if len == 0 || len > MAX_HANDSHAKE {
        return Handshake::NotMinecraft;
    }
    let body_start = at;
    match varint(b, &mut at) {
        None => return need(b),
        Some(Some(0)) => {}
        _ => return Handshake::NotMinecraft,
    }
    match varint(b, &mut at) {
        None => return need(b),
        Some(None) => return Handshake::NotMinecraft,
        Some(Some(_protocol)) => {}
    }
    let slen = match varint(b, &mut at) {
        None => return need(b),
        Some(None) => return Handshake::NotMinecraft,
        Some(Some(l)) => l as usize,
    };
    if slen == 0 || slen > 255 * 4 || at + slen > body_start + len {
        return Handshake::NotMinecraft;
    }
    if b.len() < at + slen {
        return need(b);
    }
    let Ok(raw) = std::str::from_utf8(&b[at..at + slen]) else {
        return Handshake::NotMinecraft;
    };
    let host = raw
        .split('\0')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return Handshake::NotMinecraft;
    }
    Handshake::Host(host)
}

fn need(b: &[u8]) -> Handshake {
    if b.len() >= MAX_HANDSHAKE {
        Handshake::NotMinecraft
    } else {
        Handshake::NeedMore
    }
}

/// Whether `host` is one of `expected` (case-insensitive, no trailing dot).
pub fn matches(host: &str, expected: &[String]) -> bool {
    expected
        .iter()
        .any(|e| e.trim_end_matches('.').eq_ignore_ascii_case(host))
}

#[cfg(test)]
pub(crate) fn handshake(host: &str, port: u16) -> Vec<u8> {
    fn vi(mut v: u32, out: &mut Vec<u8>) {
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }
    let mut body = Vec::new();
    vi(0, &mut body);
    vi(767, &mut body);
    vi(host.len() as u32, &mut body);
    body.extend_from_slice(host.as_bytes());
    body.extend_from_slice(&port.to_be_bytes());
    vi(1, &mut body);
    let mut out = Vec::new();
    vi(body.len() as u32, &mut out);
    out.extend(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_dialed_host() {
        let h = handshake("mc-1a2b3c4d.m.cua.sh", 25565);
        assert_eq!(parse(&h), Handshake::Host("mc-1a2b3c4d.m.cua.sh".into()));
        // Byte by byte, it needs more until the address is complete.
        for n in 0..h.len() - 3 {
            assert_eq!(parse(&h[..n]), Handshake::NeedMore, "{n}");
        }
        // Trailing data (the next packet) does not matter.
        let mut more = h.clone();
        more.extend_from_slice(&[1, 0]);
        assert!(matches!(parse(&more), Handshake::Host(_)));
    }

    #[test]
    fn forge_suffixes_case_and_dots_are_normalized() {
        assert_eq!(
            parse(&handshake("MC-1A.m.cua.sh.\0FML3\0", 1)),
            Handshake::Host("mc-1a.m.cua.sh".into())
        );
        assert!(matches("mc-1a.m.cua.sh", &["MC-1A.M.CUA.SH.".into()]));
        assert!(!matches("other.m.cua.sh", &["mc-1a.m.cua.sh".into()]));
    }

    #[test]
    fn other_protocols_are_not_minecraft() {
        assert_eq!(parse(&[0xFE, 0x01]), Handshake::NotMinecraft);
        assert_eq!(parse(b"GET / HTTP/1.1\r\n"), Handshake::NotMinecraft);
        assert_eq!(parse(b"SSH-2.0-OpenSSH_9.6\r\n"), Handshake::NotMinecraft);
        assert_eq!(parse(&[0xff; 6]), Handshake::NotMinecraft);
        assert_eq!(parse(&[0u8; 600]), Handshake::NotMinecraft);
        // An address longer than its packet.
        let mut h = handshake("x.example", 1);
        h[0] = 3;
        assert_eq!(parse(&h), Handshake::NotMinecraft);
    }
}
