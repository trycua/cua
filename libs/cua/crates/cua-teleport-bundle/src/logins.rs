// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The reserved `logins.json` bundle entry.
//!
//! Saved passwords travel only when the user ticked them in the review. Like
//! cookies ([`crate::cookies`]), they ride **already decrypted** by the sender
//! inside the same integrity-checked, owner-only bundle, and the receiver
//! re-encrypts each one under the DESTINATION browser's own Safe Storage key
//! into its `Login Data` database. The source's key never crosses the wire.

use serde::{Deserialize, Serialize};

/// The reserved bundle entry a saved login rides in.
pub const LOGINS_ENTRY: &str = "logins.json";

/// One saved login, as the sender captured it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginItem {
    /// The page origin (`https://github.com`), the login's `origin_url`.
    pub origin: String,
    /// The username (may be empty).
    pub username: String,
    /// The decrypted password, base64 on the wire so any bytes survive.
    #[serde(with = "crate::keychain::b64")]
    pub password: Vec<u8>,
    /// Chrome's `signon_realm` (the origin with a trailing slash for a
    /// web form), what the browser matches a page against.
    pub signon_realm: String,
}

impl std::fmt::Debug for LoginItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginItem")
            .field("origin", &self.origin)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Drop for LoginItem {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}

/// `signon_realm` for a web login at `origin`.
pub fn signon_realm(origin: &str) -> String {
    format!("{}/", origin.trim_end_matches('/'))
}

/// Serializes logins for the [`LOGINS_ENTRY`] bundle entry.
pub fn serialize(items: &[LoginItem]) -> Vec<u8> {
    serde_json::to_vec(items).unwrap_or_else(|_| b"[]".to_vec())
}

/// Parses a [`LOGINS_ENTRY`] payload; malformed input yields no logins.
pub fn parse(bytes: &[u8]) -> Vec<LoginItem> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_never_prints_the_password() {
        let items = vec![LoginItem {
            origin: "https://github.com".into(),
            username: "octo".into(),
            password: b"pw-\xff".to_vec(),
            signon_realm: signon_realm("https://github.com"),
        }];
        assert_eq!(parse(&serialize(&items)), items);
        assert!(!format!("{:?}", items[0]).contains("pw-"));
        assert_eq!(items[0].signon_realm, "https://github.com/");
        assert!(parse(b"nope").is_empty());
    }

    #[test]
    fn the_entry_is_distinct_from_the_other_reserved_entries() {
        for other in [
            crate::cookies::COOKIES_ENTRY,
            crate::local_storage::LOCAL_STORAGE_ENTRY,
            crate::keychain::KEYCHAIN_ENTRY,
        ] {
            assert_ne!(LOGINS_ENTRY, other);
        }
    }
}
