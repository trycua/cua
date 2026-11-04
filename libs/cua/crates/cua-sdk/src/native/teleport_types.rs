//! Teleport records shared by `Space::teleport_manifest` (feature `spaces`)
//! and `Cua.teleport()` (feature `teleport`): one manifest shape for every
//! consent UI.

use serde::Deserialize;

/// One item a teleport would move.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct TeleportItem {
    /// Bundle-relative path (what an approval names).
    pub relative_path: String,
    /// Label.
    pub label: String,
    /// Estimated bytes.
    pub estimated_bytes: u64,
    /// Credentials, cookies, tokens, transcripts.
    pub is_sensitive: bool,
    /// In the default selection.
    pub is_checked_by_default: bool,
    /// Count of things it holds.
    #[serde(default)]
    pub count: Option<u64>,
    /// Noun for `count`.
    #[serde(default)]
    pub count_noun: Option<String>,
}

/// Exactly what would leave this machine.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct TeleportManifest {
    /// Provider id (`firefox`, `chrome`, `claude-code`, ...).
    pub app: String,
    /// Display name.
    pub display_name: String,
    /// `full` or `tabs`.
    pub scope: String,
    /// Items.
    pub items: Vec<TeleportItem>,
    /// Provider total.
    pub total_estimated_bytes: u64,
    /// Provider caveats.
    pub notes: Vec<String>,
}
