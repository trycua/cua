// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The reserved `localstorage.json` bundle entry.
//!
//! A Chromium-family browser keeps `localStorage` in a LevelDB directory
//! (`Local Storage/leveldb`) whose files are only valid as one consistent set.
//! A raw copy of it works only when the source is quiet and lands on a
//! destination that has no database of its own. This entry instead carries
//! the values as items (`cua_chromium_storage` reads them on the sender and
//! writes them on the receiver, while the destination browser is not
//! running), so the Keyvault can hold, list and update them one by one.
//!
//! Values are text. A value whose stored bytes are not valid text (a lone
//! UTF-16 surrogate) keeps its exact stored bytes in `value_raw`, so nothing
//! is lost; the same for a key in `key_raw`.

use serde::{Deserialize, Serialize};

/// The reserved bundle entry a provider packs localStorage items into.
pub const LOCAL_STORAGE_ENTRY: &str = "localstorage.json";

/// One localStorage value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalStorageItem {
    /// The origin (`https://github.com`), exactly as the browser keyed it.
    pub origin: String,
    /// The storage key.
    pub key: String,
    /// The value.
    pub value: String,
    /// The exact stored key bytes (base64, format byte included), present
    /// only when `key` cannot hold them losslessly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_raw: Option<String>,
    /// The exact stored value bytes (base64, format byte included), present
    /// only when `value` cannot hold them losslessly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_raw: Option<String>,
}

impl std::fmt::Debug for LocalStorageItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalStorageItem")
            .field("origin", &self.origin)
            .field("key", &self.key)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl Drop for LocalStorageItem {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.value.zeroize();
        if let Some(raw) = &mut self.value_raw {
            raw.zeroize();
        }
    }
}

/// Serializes items for the [`LOCAL_STORAGE_ENTRY`] bundle entry.
pub fn serialize(items: &[LocalStorageItem]) -> Vec<u8> {
    serde_json::to_vec(items).unwrap_or_else(|_| b"[]".to_vec())
}

/// Parses a [`LOCAL_STORAGE_ENTRY`] payload. Malformed input yields no items.
pub fn parse(bytes: &[u8]) -> Vec<LocalStorageItem> {
    serde_json::from_slice(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_omits_raw_when_lossless() {
        let items = vec![
            LocalStorageItem {
                origin: "https://github.com".into(),
                key: "color_mode".into(),
                value: "dark".into(),
                key_raw: None,
                value_raw: None,
            },
            LocalStorageItem {
                origin: "https://a.example".into(),
                key: "k".into(),
                value: "\u{fffd}".into(),
                key_raw: None,
                value_raw: Some("AAA=".into()),
            },
        ];
        let json = serialize(&items);
        assert!(!String::from_utf8_lossy(&json).contains("key_raw"));
        assert_eq!(parse(&json), items);
        assert!(parse(b"nope").is_empty());
    }

    #[test]
    fn the_entry_is_distinct_from_the_other_reserved_entries() {
        assert_ne!(LOCAL_STORAGE_ENTRY, crate::cookies::COOKIES_ENTRY);
        assert_ne!(LOCAL_STORAGE_ENTRY, crate::keychain::KEYCHAIN_ENTRY);
    }
}
