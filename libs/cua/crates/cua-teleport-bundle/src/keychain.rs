// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The reserved `keychain.json` bundle entry.
//!
//! Chromium/Electron apps (Chrome, Slack, Discord, …) encrypt their local
//! cookies and auth tokens with an app-specific **"Safe Storage"** key held in
//! the macOS login Keychain, and some apps keep their auth token there too. A
//! teleport that preserves the login therefore carries those items: the sender
//! reads them (`cua_teleport::keychain`), packs them into [`KEYCHAIN_ENTRY`]
//! with [`serialize`], and the receiver parses them with [`parse`] and
//! installs them in the destination Keychain (`cua_spacesd_teleport::keychain`).
//! This module is only the wire format.

use serde::{Deserialize, Serialize};

/// One generic-password Keychain item (service + account + secret bytes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeychainItem {
    pub service: String,
    #[serde(default)]
    pub account: String,
    /// The raw secret, base64-encoded on the wire so arbitrary bytes survive.
    #[serde(with = "b64")]
    pub secret: Vec<u8>,
    /// Destination app bundle to TRUST for this item (e.g.
    /// "/Applications/Unity Hub.app"). Installed with `-T <app>` so that exact
    /// app reads it silently — items added by the `security` tool are otherwise
    /// not in the app's ACL, so the app hits an "allow access?" prompt even in
    /// an unlocked keychain. `None` falls back to `-A` (all apps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_app: Option<String>,
}

/// The reserved bundle entry a provider packs Keychain items into. Import
/// recognizes this path and installs the items instead of writing a file.
pub const KEYCHAIN_ENTRY: &str = "keychain.json";

/// Serialize Keychain items for the [`KEYCHAIN_ENTRY`] bundle entry.
pub fn serialize(items: &[KeychainItem]) -> Vec<u8> {
    serde_json::to_vec(items).unwrap_or_else(|_| b"[]".to_vec())
}

/// Parse a [`KEYCHAIN_ENTRY`] payload. Malformed input yields no items (the
/// receiver installs what it can; a garbled entry installs nothing).
pub fn parse(bytes: &[u8]) -> Vec<KeychainItem> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

/// base64 (de)serialization for the secret field.
pub(crate) mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};

    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            out.push(CHARS[(n >> 18 & 63) as usize] as char);
            out.push(CHARS[(n >> 12 & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                CHARS[(n >> 6 & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                CHARS[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        fn val(c: u8) -> Option<u32> {
            match c {
                b'A'..=b'Z' => Some((c - b'A') as u32),
                b'a'..=b'z' => Some((c - b'a' + 26) as u32),
                b'0'..=b'9' => Some((c - b'0' + 52) as u32),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        }
        let clean: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
        let mut out = Vec::with_capacity(clean.len() / 4 * 3);
        for chunk in clean.chunks(4) {
            if chunk.len() < 2 {
                return None;
            }
            let pad = chunk.iter().filter(|&&c| c == b'=').count();
            let mut n = 0u32;
            for (i, &c) in chunk.iter().enumerate() {
                n |= if c == b'=' { 0 } else { val(c)? } << (18 - 6 * i);
            }
            out.push((n >> 16) as u8);
            if pad < 2 {
                out.push((n >> 8) as u8);
            }
            if pad < 1 {
                out.push(n as u8);
            }
        }
        Some(out)
    }

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        decode(&text).ok_or_else(|| serde::de::Error::custom("invalid base64 secret"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_items_through_serialize() {
        let items = vec![
            KeychainItem {
                service: "Slack Safe Storage".into(),
                account: "Slack Key".into(),
                secret: vec![0, 1, 2, 250, 255, b'A'],
                trust_app: None,
            },
            KeychainItem {
                service: "Discord Safe Storage".into(),
                account: "Discord Key".into(),
                secret: b"v10-some-base64-ish==".to_vec(),
                trust_app: Some("/Applications/Discord.app".into()),
            },
        ];
        assert_eq!(parse(&serialize(&items)), items);
    }

    #[test]
    fn garbage_parses_as_no_items() {
        assert!(parse(b"not json").is_empty());
        assert!(parse(b"[]").is_empty());
    }

    #[test]
    fn base64_handles_all_pad_lengths() {
        for raw in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\x00\xff\x10"] {
            let enc = b64::encode(raw);
            assert_eq!(b64::decode(&enc).as_deref(), Some(raw), "roundtrip {raw:?}");
        }
    }
}
