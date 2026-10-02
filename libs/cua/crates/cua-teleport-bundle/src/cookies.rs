// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The reserved `cookies.json` bundle entry.
//!
//! A Chromium-family browser's cookies are encrypted at rest with a
//! per-machine "Safe Storage" key ([`crate::chromium_crypto`]), so a raw copy
//! of `Cookies` only works when the source and destination share that key --
//! which they never do. This entry instead carries the cookies **already
//! decrypted** by the sender (`cua_teleport::browser_cookies`), plaintext on
//! the wire inside the same integrity-checked, owner-only-mode bundle every
//! other entry travels in. The receiver
//! (`cua_spacesd_teleport::cookies`) re-encrypts each value under the
//! DESTINATION's own Safe Storage key -- creating one if the destination has
//! none yet -- and writes them into a real `Cookies` database. The source's
//! key never appears in this entry or crosses the wire at all.
//!
//! This is the same shape [`crate::keychain`] uses for the Safe Storage key
//! itself (carried for apps where the source's raw key IS reused verbatim);
//! cookies deliberately use a distinct, reserved entry so the receiver can
//! tell "reinstall this exact secret" from "re-encrypt these values under
//! your own secret" and never confuses the two.

use serde::{Deserialize, Serialize};

/// One decrypted cookie row, as the sender captured it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieItem {
    /// The `host_key` column (`.github.com`, `api.github.com`, …), exactly as
    /// the source browser stored it.
    pub host_key: String,
    /// Cookie name.
    pub name: String,
    /// Decrypted value, base64 on the wire so arbitrary bytes survive.
    #[serde(with = "crate::keychain::b64")]
    pub value: Vec<u8>,
    /// Cookie path.
    pub path: String,
    /// Expiry, Chrome's own `chrome_utc` microseconds since 1601-01-01 (0 for
    /// a session cookie). Carried through unchanged.
    pub expires_utc: i64,
    pub is_secure: bool,
    pub is_httponly: bool,
    /// Chrome's `samesite` enum (-1 unspecified, 0 none, 1 lax, 2 strict).
    #[serde(default)]
    pub samesite: i64,
    /// Every other column of Chrome's current cookie schema the source had
    /// (flattened into the same JSON object), so the destination's row is
    /// the source's row. All optional: an older sender omits them and the
    /// receiver fills Chrome's defaults.
    #[serde(flatten)]
    pub extra: CookieExtra,
}

/// The cookie columns beyond the core ones.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieExtra {
    /// `creation_utc` (Chrome microseconds since 1601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_utc: Option<i64>,
    /// `last_access_utc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_access_utc: Option<i64>,
    /// `last_update_utc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update_utc: Option<i64>,
    /// Chrome's priority (0 low, 1 medium, 2 high).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    /// 0 unset, 1 non-secure, 2 secure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_scheme: Option<i64>,
    /// -1 unspecified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_port: Option<i64>,
    /// 0 unknown, 1 http, 2 script, 3 other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<i64>,
    /// `has_cross_site_ancestor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_cross_site_ancestor: Option<i64>,
    /// The top-level site of a partitioned (CHIPS) cookie (written back as
    /// `top_frame_site_key`, so it stays partitioned); absent or empty for an
    /// unpartitioned cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_key: Option<String>,
}

impl Drop for CookieItem {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.value.zeroize();
    }
}

/// The reserved bundle entry a provider packs decrypted cookies into. Import
/// recognizes this path and re-encrypts + installs the items instead of
/// writing a file verbatim.
pub const COOKIES_ENTRY: &str = "cookies.json";

/// Serializes cookies for the [`COOKIES_ENTRY`] bundle entry.
pub fn serialize(items: &[CookieItem]) -> Vec<u8> {
    serde_json::to_vec(items).unwrap_or_else(|_| b"[]".to_vec())
}

/// Parses a [`COOKIES_ENTRY`] payload. Malformed input yields no items (the
/// receiver installs what it can; a garbled entry installs nothing).
pub fn parse(bytes: &[u8]) -> Vec<CookieItem> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_items_through_serialize() {
        let items = vec![
            CookieItem {
                host_key: ".github.com".into(),
                name: "user_session".into(),
                value: b"s3cret-session-value".to_vec(),
                path: "/".into(),
                expires_utc: 13_397_000_000_000_000,
                is_secure: true,
                is_httponly: true,
                samesite: 1,
                extra: CookieExtra::default(),
            },
            CookieItem {
                host_key: "api.github.com".into(),
                name: "_gh_sess".into(),
                value: vec![0, 1, 2, 250, 255],
                path: "/".into(),
                expires_utc: 0,
                is_secure: true,
                is_httponly: false,
                samesite: -1,
                extra: CookieExtra::default(),
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
    fn the_entry_path_is_distinct_from_the_keychain_entry() {
        assert_ne!(COOKIES_ENTRY, crate::keychain::KEYCHAIN_ENTRY);
    }
}
