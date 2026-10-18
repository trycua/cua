// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the vault holds: items, their policies, grants, unattended rules,
//! deliveries and settings. Everything here lives inside the sealed
//! metadata (`meta.sealed`), except item payloads (`items/<id>.sealed`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::audit::Head;
use crate::capability::Action;

/// What a secret item is.
///
/// The vault is a list of items grouped by source app. Each item is one
/// secret: a cookie, a localStorage value, a password, or a file (the
/// catch-all for bookmarks, session files and non-browser app state).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// One cookie: key is the cookie name, domain its host.
    Cookie,
    /// One localStorage value: key is the storage key, domain its origin host.
    LocalStorage,
    /// One saved password: key is the username, domain its site. Passwords
    /// stay sealed: they sign in through site login and are never delivered
    /// as files.
    Password,
    /// A file: key is its path, relative to the app's data.
    File,
}

impl ItemKind {
    /// Label used in the UI.
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Cookie => "Cookie",
            ItemKind::LocalStorage => "Local storage",
            ItemKind::Password => "Password",
            ItemKind::File => "File",
        }
    }

    /// Stable tag used in unique keys and the wire.
    pub fn tag(self) -> &'static str {
        match self {
            ItemKind::Cookie => "cookie",
            ItemKind::LocalStorage => "local_storage",
            ItemKind::Password => "password",
            ItemKind::File => "file",
        }
    }

    /// Whether items of this kind are keyed by path (files) rather than by
    /// domain and key.
    pub fn keyed_by_path(self) -> bool {
        self == ItemKind::File
    }

    /// Whether an item of this kind can be delivered into a Space as app
    /// state. Passwords never are.
    pub fn deliverable(self) -> bool {
        self != ItemKind::Password
    }
}

/// The identity of an item within its app: a file's relative path, a
/// cookie's host plus name plus path, a localStorage value's origin plus
/// key, or a password's origin plus username. The kind is part of it so a
/// cookie and a storage value that share a domain and a name stay two
/// items. Saving the same app again upserts by this key and never
/// duplicates.
pub fn unique_key(kind: ItemKind, domain: Option<&str>, key: &str, path: Option<&str>) -> String {
    if kind.keyed_by_path() {
        format!("{}\u{1f}{key}", kind.tag())
    } else {
        format!(
            "{}\u{1f}{}\u{1f}{key}\u{1f}{}",
            kind.tag(),
            domain.unwrap_or("").trim().to_ascii_lowercase(),
            if kind == ItemKind::Cookie {
                path.unwrap_or("/")
            } else {
                ""
            }
        )
    }
}

/// Domains whose session unlocks other apps (red-team F7): such an item
/// can never be unlocked for unattended use and gets a short delivery
/// lifetime.
pub fn is_identity_provider(domain: &str) -> bool {
    const IDPS: [&str; 12] = [
        "google.com",
        "googleusercontent.com",
        "microsoft.com",
        "microsoftonline.com",
        "live.com",
        "office.com",
        "apple.com",
        "icloud.com",
        "okta.com",
        "auth0.com",
        "onelogin.com",
        "duosecurity.com",
    ];
    let d = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    IDPS.iter()
        .any(|i| d == *i || d.ends_with(&format!(".{i}")))
}

/// Per-item policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemPolicy {
    /// Targets (Space names) this item may ever go to. Empty: any target,
    /// but always with consent or a rule.
    #[serde(default)]
    pub allowed_targets: Vec<String>,
    /// Seconds a delivered copy lives on a target before it is wiped.
    pub ttl_secs: u64,
    /// May appear in unattended rules.
    pub unattended: bool,
}

/// Default time a delivered copy lives on a target (1 hour; red-team F12:
/// a compromised Space can read what it holds for as long as it holds it).
pub const DEFAULT_TTL_SECS: u64 = 3600;
/// Default for identity-provider sites (15 minutes; red-team F7).
pub const IDP_TTL_SECS: u64 = 15 * 60;
/// Upper bound on a delivered copy's lifetime (30 days).
pub const MAX_TTL_SECS: u64 = 30 * 24 * 3600;
/// A delivery's `expires_ms` when it never expires on its own: the copy
/// stays until it is wiped (Wipe, deleting the item, the kill switch). The
/// receiver reads `0` the same way (`ImportOptions.expires_at_ms`).
pub const NO_EXPIRY: u64 = 0;

impl Default for ItemPolicy {
    fn default() -> Self {
        Self {
            allowed_targets: Vec::new(),
            ttl_secs: DEFAULT_TTL_SECS,
            unattended: false,
        }
    }
}

impl ItemPolicy {
    /// Whether `target` is allowed.
    pub fn allows_target(&self, target: &str) -> bool {
        self.allowed_targets.is_empty() || self.allowed_targets.iter().any(|t| t == target)
    }

    /// True when `new` grants anything `self` does not (so it needs user
    /// presence).
    pub fn widened_by(&self, new: &ItemPolicy) -> bool {
        let targets_widen = if new.allowed_targets.is_empty() {
            !self.allowed_targets.is_empty()
        } else {
            new.allowed_targets.iter().any(|t| !self.allows_target(t))
        };
        targets_widen || new.ttl_secs > self.ttl_secs || (new.unattended && !self.unattended)
    }
}

/// A vault item's metadata. It never holds the value: that is the sealed
/// payload (`items/<id>.sealed`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMeta {
    /// Item id.
    pub id: String,
    /// Type.
    pub kind: ItemKind,
    /// Source app: the teleport provider id (`chrome`, `slack`).
    pub provider_id: String,
    /// App display name.
    pub app_display: String,
    /// Domain: a cookie's host, a storage value's origin host, a password's
    /// site. `None` for files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Key: a cookie name, a storage key, a username, or a file's path.
    pub key: String,
    /// A cookie's path (part of its key: the same name on the same host can
    /// exist under several paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Where it was read from (profile name).
    #[serde(default)]
    pub source: String,
    /// A cookie without an expiry (dies with the browser session).
    #[serde(default)]
    pub session: bool,
    /// A cookie's expiry, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<i64>,
    /// Payload bytes.
    #[serde(default)]
    pub bytes: u64,
    /// The content-addressed blob a big file's bytes live in (SHA-256, hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// An identity provider (Google, Microsoft, Apple, Okta, ...): its
    /// session unlocks every federated app, so it can never be unlocked
    /// for unattended use and defaults to a short TTL (red-team F7).
    #[serde(default)]
    pub identity_provider: bool,
    /// Policy. `policy.unattended` is the unlock: the lock icon's open
    /// state. A locked item needs the user's approval for every use.
    pub policy: ItemPolicy,
    /// Created, Unix ms.
    pub created_ms: u64,
    /// Last saved (upserted), Unix ms.
    pub updated_ms: u64,
    /// Payload revision.
    pub rev: u64,
    /// SHA-256 of the item record file (detects swaps and rollback).
    pub record_digest: String,
}

impl ItemMeta {
    /// This item's key within its app (see [`unique_key`]).
    pub fn unique_key(&self) -> String {
        unique_key(
            self.kind,
            self.domain.as_deref(),
            &self.key,
            self.path.as_deref(),
        )
    }

    /// A one-line name: `domain / key`, or the file's path.
    pub fn label(&self) -> String {
        match (&self.domain, self.kind.keyed_by_path()) {
            (Some(d), false) if !d.is_empty() => format!("{d} / {}", self.key),
            _ => self.key.clone(),
        }
    }

    /// Locked: every use needs the user's approval.
    pub fn locked(&self) -> bool {
        !self.policy.unattended
    }

    /// A copy for bulk enumeration without presence: app, type, lock state
    /// and timestamps are kept, but the domain and key are stripped. Those
    /// names are themselves sensitive (they map the user's whole logged-in
    /// life: which banks, which employers' SSO, internal hostnames), so
    /// they are only returned inside a browse window the user opened with
    /// presence, never to a third party (red-team F17).
    pub fn redacted(&self) -> ItemMeta {
        let mut m = self.clone();
        m.domain = None;
        m.key = String::new();
        m.path = None;
        m.source = String::new();
        m.blob = None;
        m
    }

    /// A new item to save: its type, app, domain (none for a file) and key,
    /// with the default policy (locked). The vault fills in the id, times,
    /// revision and digest.
    pub fn draft(
        kind: ItemKind,
        provider_id: &str,
        app_display: &str,
        domain: Option<&str>,
        key: &str,
    ) -> ItemMeta {
        ItemMeta {
            id: String::new(),
            kind,
            provider_id: provider_id.into(),
            app_display: app_display.into(),
            domain: domain.map(str::to_string),
            key: key.into(),
            path: None,
            source: String::new(),
            session: false,
            expires_ms: None,
            bytes: 0,
            blob: None,
            identity_provider: false,
            policy: ItemPolicy::default(),
            created_ms: 0,
            updated_ms: 0,
            rev: 0,
            record_digest: String::new(),
        }
    }
}

/// A bundle entry inside an item payload.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadEntry {
    /// Bundle-relative path.
    pub rel_path: String,
    /// Unix mode.
    pub mode: u32,
    /// Base64 bytes.
    pub data: String,
}

impl std::fmt::Debug for PayloadEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayloadEntry")
            .field("rel_path", &self.rel_path)
            .field("mode", &self.mode)
            .field(
                "data",
                &format_args!("<{} base64 chars redacted>", self.data.len()),
            )
            .finish()
    }
}

impl Drop for PayloadEntry {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

/// An item's secret payload: one canonical JSON record, stored decrypted
/// inside the sealed item (the vault encrypts it at rest). The schema is
/// versioned (`cookie@1`) so a format change is a new schema with a codec
/// that reads the old one. Big file content is not inlined: the record
/// names a content-addressed blob (see [`crate::record::FileRecord`]).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemPayload {
    /// Provider id (the receiver's importer).
    pub provider_id: String,
    /// `tabs` or `full`.
    pub scope: String,
    /// The record's schema: `cookie@1`, `local_storage@1`, `password@1` or
    /// `file@1`.
    pub schema: String,
    /// The record: canonical JSON text. Wiped on drop.
    pub record: String,
}

impl std::fmt::Debug for ItemPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemPayload")
            .field("provider_id", &self.provider_id)
            .field("schema", &self.schema)
            .field(
                "record",
                &format_args!("<{} bytes redacted>", self.record.len()),
            )
            .finish()
    }
}

impl Drop for ItemPayload {
    fn drop(&mut self) {
        self.record.zeroize();
    }
}

/// One saved login inside the vault. `Debug` never prints the password, and
/// every field is wiped on drop.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, zeroize::ZeroizeOnDrop)]
pub struct LoginRecord {
    /// Origin the login belongs to (`https://github.com`, no path).
    pub origin: String,
    /// Username (may be empty).
    pub username: String,
    /// Password.
    pub password: String,
}

impl std::fmt::Debug for LoginRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRecord")
            .field("origin", &self.origin)
            .field("username", &mask_username(&self.username))
            .field("password", &"<redacted>")
            .finish()
    }
}

/// A username as the audit log and tool results show it: the first
/// character, then `***`, keeping an email's domain (`a***@example.test`).
pub fn mask_username(u: &str) -> String {
    if u.is_empty() {
        return String::new();
    }
    let first: String = u.chars().take(1).collect();
    match u.split_once('@') {
        Some((_, domain)) => format!("{first}***@{domain}"),
        None => format!("{first}***"),
    }
}

impl ItemPayload {
    /// This payload with another record (same provider, scope and schema).
    pub fn with_record(&self, record: String) -> ItemPayload {
        ItemPayload {
            provider_id: self.provider_id.clone(),
            scope: self.scope.clone(),
            schema: self.schema.clone(),
            record,
        }
    }

    /// A password item's payload: its one saved login.
    pub fn password(provider_id: &str, login: &LoginRecord) -> crate::Result<ItemPayload> {
        Ok(ItemPayload {
            provider_id: provider_id.into(),
            scope: "full".into(),
            schema: crate::record::PASSWORD_V1.into(),
            record: serde_json::to_string(login)?,
        })
    }

    /// The saved login a password item holds. Anything else is refused.
    pub fn login(&self) -> crate::Result<LoginRecord> {
        if self.schema != crate::record::PASSWORD_V1 {
            return Err(crate::Error::Corrupt(format!(
                "{} is not a saved login",
                self.schema
            )));
        }
        serde_json::from_str(&self.record)
            .map_err(|_| crate::Error::Corrupt("the saved login is not valid".into()))
    }
}

/// A user's consent, recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Grant id.
    pub id: String,
    /// The consent request it answered.
    #[serde(default)]
    pub request_id: String,
    /// Caller fingerprint it is bound to.
    pub caller_fp: String,
    /// Caller display identity at grant time.
    pub caller_display: String,
    /// Items.
    pub items: Vec<String>,
    /// Targets.
    pub targets: Vec<String>,
    /// The immutable id each target name resolved to when the grant was made
    /// (red-team F1). Delivery re-resolves the name and refuses if the id has
    /// changed, so a Space renamed or re-created under the same name cannot
    /// silently receive the session.
    #[serde(default)]
    pub target_ids: BTreeMap<String, String>,
    /// Actions.
    pub actions: Vec<Action>,
    /// Created, Unix ms.
    pub created_ms: u64,
    /// Expiry, Unix ms.
    pub not_after_ms: u64,
    /// Uses left (`None`: unlimited until expiry).
    #[serde(default)]
    pub uses_left: Option<u32>,
    /// Revoked.
    #[serde(default)]
    pub revoked: bool,
    /// The named agent the user approved this for (from the request), so
    /// every use is audited under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Minted without asking, because every item it covers was unlocked
    /// (allowed unattended). Locking one of them revokes it.
    #[serde(default)]
    pub unattended: bool,
}

impl Grant {
    /// Live: not revoked, not expired, uses left.
    pub fn is_live(&self, now_ms: u64) -> bool {
        !self.revoked && now_ms < self.not_after_ms && self.uses_left != Some(0)
    }
}

/// A caller named in an unattended rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleCaller {
    /// Fingerprint.
    pub fp: String,
    /// Display identity when the rule was made.
    pub display: String,
}

/// A user-authored unattended teleport rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnattendedRule {
    /// Rule id.
    pub id: String,
    /// Items it covers.
    pub items: Vec<String>,
    /// Targets (Space names); `*` is any target.
    pub targets: Vec<String>,
    /// The immutable id each concretely-named target resolved to when the rule
    /// was authored (red-team F1). Delivery refuses if a named target's id has
    /// changed. `*` cannot be pinned, so a `*` rule remains an explicit
    /// "any Space" opt-in with its own UI warning.
    #[serde(default)]
    pub target_ids: BTreeMap<String, String>,
    /// Callers it covers.
    pub callers: Vec<RuleCaller>,
    /// Created, Unix ms.
    pub created_ms: u64,
    /// Expiry, Unix ms.
    pub not_after_ms: u64,
    /// Enabled.
    pub enabled: bool,
    /// Note.
    #[serde(default)]
    pub note: String,
}

/// Default and maximum rule lifetimes.
pub const DEFAULT_RULE_SECS: u64 = 7 * 24 * 3600;
/// Rules cannot outlive this.
pub const MAX_RULE_SECS: u64 = 90 * 24 * 3600;
/// Default third-party grant lifetime (15 minutes).
pub const DEFAULT_GRANT_SECS: u64 = 15 * 60;
/// Maximum third-party grant lifetime ("until revoked" is capped here).
pub const MAX_GRANT_SECS: u64 = 30 * 24 * 3600;

impl UnattendedRule {
    /// Whether this rule lets `caller_fp` teleport `item` to `target` now.
    pub fn matches(&self, caller_fp: &str, item: &str, target: &str, now_ms: u64) -> bool {
        self.enabled
            && now_ms < self.not_after_ms
            && self.items.iter().any(|i| i == item)
            && self.targets.iter().any(|t| t == "*" || t == target)
            && self.callers.iter().any(|c| c.fp == caller_fp)
    }
}

/// A copy of items delivered into a target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// The receiver's import id (what `WipeImport` takes).
    pub import_id: String,
    /// Target Space.
    pub target: String,
    /// Provider id.
    pub provider_id: String,
    /// Items in this import.
    pub items: Vec<String>,
    /// Who caused it.
    pub caller_fp: String,
    /// Delivered, Unix ms.
    pub delivered_ms: u64,
    /// When the receiver wipes it on its own ([`NO_EXPIRY`]: never).
    pub expires_ms: u64,
    /// Wiped (or superseded) already.
    #[serde(default)]
    pub wiped: bool,
}

impl Delivery {
    /// Not wiped and not past its expiry (one without expiry stays live).
    pub fn live(&self, now_ms: u64) -> bool {
        !self.wiped && (self.expires_ms == NO_EXPIRY || self.expires_ms > now_ms)
    }
}

/// When a copy delivered at `now_ms` expires: `ttl_secs` later while
/// auto-wipe is on, else never ([`NO_EXPIRY`]).
pub fn delivery_expiry(auto_wipe: bool, now_ms: u64, ttl_secs: u64) -> u64 {
    if auto_wipe {
        now_ms + ttl_secs * 1000
    } else {
        NO_EXPIRY
    }
}

/// How the vault unlocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnlockPolicy {
    /// The OS protector unlocks at daemon start (unattended rules work while
    /// the user is logged in).
    #[default]
    Auto,
    /// Every unlock needs user presence; the vault auto-locks when idle.
    Presence,
}

/// Vault settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The global kill switch: refuses every teleport, import and approval.
    pub disabled: bool,
    /// Unlock policy.
    pub unlock_policy: UnlockPolicy,
    /// Idle minutes before auto-lock (`presence` policy).
    pub auto_lock_minutes: u32,
    /// Delivered copies wipe themselves after their item's `ttl_secs`. Off
    /// (the default) they stay until wiped ([`NO_EXPIRY`]).
    #[serde(default)]
    pub auto_wipe: bool,
    /// The user chose "Never ask again" on the unlock prompt: unlocking an
    /// item (allowing unattended access) skips the explanation prompt. It
    /// never skips the presence check (Touch ID) the broker asks for.
    #[serde(default)]
    pub skip_unlock_prompt: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            disabled: false,
            unlock_policy: UnlockPolicy::Auto,
            auto_lock_minutes: 15,
            auto_wipe: false,
            skip_unlock_prompt: false,
        }
    }
}

/// The sealed metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// Bumped on every write.
    pub generation: u64,
    /// Revocation epoch: the kill switch and "revoke all" bump it, killing
    /// every outstanding token.
    pub epoch: u64,
    /// Settings.
    pub settings: Settings,
    /// Items by id.
    pub items: BTreeMap<String, ItemMeta>,
    /// Grants.
    pub grants: Vec<Grant>,
    /// Unattended rules.
    pub rules: Vec<UnattendedRule>,
    /// Deliveries (live and recently wiped).
    pub deliveries: Vec<Delivery>,
    /// Site icons (registrable domain to base64 PNG), read from the source
    /// browser's own local favicon store when its items were saved. Not
    /// secret, small and bounded; pruned with the last item of a site.
    #[serde(default)]
    pub favicons: BTreeMap<String, String>,
    /// The audit chain head this metadata vouches for.
    pub audit_head: Head,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening_needs_presence_and_narrowing_does_not() {
        let base = ItemPolicy {
            allowed_targets: vec!["a".into()],
            ttl_secs: 100,
            unattended: false,
        };
        let narrower = ItemPolicy {
            ttl_secs: 50,
            ..base.clone()
        };
        assert!(!base.widened_by(&narrower));
        assert!(base.widened_by(&ItemPolicy {
            allowed_targets: vec![],
            ..base.clone()
        }));
        assert!(base.widened_by(&ItemPolicy {
            allowed_targets: vec!["b".into()],
            ..base.clone()
        }));
        assert!(base.widened_by(&ItemPolicy {
            ttl_secs: 101,
            ..base.clone()
        }));
        assert!(base.widened_by(&ItemPolicy {
            unattended: true,
            ..base.clone()
        }));
    }

    #[test]
    fn rules_and_grants_expire() {
        let r = UnattendedRule {
            id: "r".into(),
            items: vec!["i".into()],
            targets: vec!["s".into()],
            target_ids: Default::default(),
            callers: vec![RuleCaller {
                fp: "fp".into(),
                display: "x".into(),
            }],
            created_ms: 0,
            not_after_ms: 100,
            enabled: true,
            note: String::new(),
        };
        assert!(r.matches("fp", "i", "s", 99));
        assert!(!r.matches("fp", "i", "s", 100));
        assert!(!r.matches("other", "i", "s", 1));
        assert!(!r.matches("fp", "j", "s", 1));
        assert!(!r.matches("fp", "i", "t", 1));
        let g = Grant {
            id: "g".into(),
            request_id: String::new(),
            caller_fp: "fp".into(),
            caller_display: "x".into(),
            items: vec![],
            targets: vec![],
            target_ids: Default::default(),
            actions: vec![],
            created_ms: 0,
            not_after_ms: 10,
            uses_left: Some(1),
            revoked: false,
            agent: None,
            unattended: false,
        };
        assert!(g.is_live(5));
        assert!(!g.is_live(10));
        assert!(
            !Grant {
                uses_left: Some(0),
                ..g.clone()
            }
            .is_live(5)
        );
        assert!(!Grant { revoked: true, ..g }.is_live(5));
    }

    #[test]
    fn payload_debug_redacts_data() {
        let e = PayloadEntry {
            rel_path: "Cookies".into(),
            mode: 0o600,
            data: "c2VjcmV0".into(),
        };
        let s = format!("{e:?}");
        assert!(!s.contains("c2VjcmV0"), "{s}");
    }

    fn meta(kind: ItemKind, domain: Option<&str>, key: &str) -> ItemMeta {
        ItemMeta {
            id: "i".into(),
            kind,
            provider_id: "chrome".into(),
            app_display: "Chrome".into(),
            domain: domain.map(str::to_string),
            key: key.into(),
            path: None,
            source: "Default".into(),
            session: false,
            expires_ms: None,
            bytes: 0,
            blob: None,
            identity_provider: false,
            policy: ItemPolicy::default(),
            created_ms: 0,
            updated_ms: 0,
            rev: 1,
            record_digest: String::new(),
        }
    }

    #[test]
    fn a_files_key_is_its_path_and_the_rest_are_domain_plus_key() {
        let f = meta(ItemKind::File, Some("ignored.test"), "Default/Bookmarks");
        assert_eq!(
            f.unique_key(),
            meta(ItemKind::File, None, "Default/Bookmarks").unique_key()
        );
        assert_ne!(
            f.unique_key(),
            meta(ItemKind::File, None, "Default/Preferences").unique_key()
        );
        let c = meta(ItemKind::Cookie, Some("GitHub.com"), "sid");
        assert_eq!(
            c.unique_key(),
            meta(ItemKind::Cookie, Some("github.com"), "sid").unique_key()
        );
        assert_ne!(
            c.unique_key(),
            meta(ItemKind::Cookie, Some("github.com"), "other").unique_key()
        );
        assert_ne!(
            c.unique_key(),
            meta(ItemKind::Cookie, Some("gitlab.com"), "sid").unique_key()
        );
        assert_ne!(
            c.unique_key(),
            meta(ItemKind::LocalStorage, Some("github.com"), "sid").unique_key(),
            "a cookie and a storage value stay two items"
        );
        assert_ne!(
            c.unique_key(),
            meta(ItemKind::Password, Some("github.com"), "sid").unique_key()
        );
        // A cookie is host + name + path; a leading dot is a different cookie.
        let mut under_path = meta(ItemKind::Cookie, Some("GitHub.com"), "sid");
        under_path.path = Some("/api".into());
        assert_ne!(c.unique_key(), under_path.unique_key());
        let mut root = meta(ItemKind::Cookie, Some("GitHub.com"), "sid");
        root.path = Some("/".into());
        assert_eq!(
            c.unique_key(),
            root.unique_key(),
            "no path means the root path"
        );
        assert_ne!(
            c.unique_key(),
            meta(ItemKind::Cookie, Some(".github.com"), "sid").unique_key(),
            "a domain cookie and a host-only cookie are two cookies"
        );
        // A password is origin + username; a storage value is origin + key.
        assert_ne!(
            meta(ItemKind::Password, Some("https://a.test"), "u").unique_key(),
            meta(ItemKind::Password, Some("https://b.test"), "u").unique_key()
        );
    }

    #[test]
    fn labels_and_locks() {
        assert_eq!(
            meta(ItemKind::Cookie, Some("github.com"), "sid").label(),
            "github.com / sid"
        );
        assert_eq!(meta(ItemKind::File, None, "a/b").label(), "a/b");
        let mut m = meta(ItemKind::Cookie, Some("github.com"), "sid");
        assert!(m.locked(), "items are locked until the user unlocks them");
        m.policy.unattended = true;
        assert!(!m.locked());
    }

    #[test]
    fn redaction_keeps_the_app_and_drops_the_names() {
        let r = meta(ItemKind::Cookie, Some("bank.example"), "sid").redacted();
        assert_eq!(r.provider_id, "chrome");
        assert_eq!(r.kind, ItemKind::Cookie);
        assert!(r.domain.is_none() && r.key.is_empty() && r.source.is_empty());
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("bank.example"), "{json}");
    }

    #[test]
    fn identity_providers_are_recognised_with_subdomains() {
        assert!(is_identity_provider("accounts.google.com"));
        assert!(is_identity_provider(".login.microsoftonline.com"));
        assert!(is_identity_provider("okta.com"));
        assert!(!is_identity_provider("notgoogle.com"));
        assert!(!is_identity_provider("github.com"));
    }
}
