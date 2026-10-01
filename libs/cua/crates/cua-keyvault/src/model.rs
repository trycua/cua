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

/// Item kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// One site (eTLD+1) of one browser profile: cookies, and optionally
    /// its storage.
    BrowserSite,
    /// Saved passwords for one site (never selected by default).
    SitePasswords,
    /// A whole app session (Electron apps, CLIs): every workspace or
    /// account the app holds.
    AppSession,
}

impl ItemKind {
    /// Label used in the UI.
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::BrowserSite => "Site session",
            ItemKind::SitePasswords => "Saved passwords",
            ItemKind::AppSession => "Whole app session",
        }
    }
}

/// One cookie, described without its value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieInfo {
    /// Cookie name.
    pub name: String,
    /// Host or domain (`.github.com`).
    pub domain: String,
    /// Session cookie (no expiry).
    pub session: bool,
    /// Expiry, Unix ms (persistent cookies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<i64>,
}

/// What an item contains, for "view before sending". Never values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSummary {
    /// Cookies (names and domains).
    #[serde(default)]
    pub cookies: Vec<CookieInfo>,
    /// Storage origins carried (`https://github.com`).
    #[serde(default)]
    pub storage_origins: Vec<String>,
    /// Saved passwords carried (count only).
    #[serde(default)]
    pub passwords: u32,
    /// Bundle paths carried.
    #[serde(default)]
    pub files: Vec<String>,
    /// Keychain services carried (for example `Chrome Safe Storage`).
    #[serde(default)]
    pub keychain_services: Vec<String>,
    /// Payload bytes.
    #[serde(default)]
    pub bytes: u64,
}

impl ItemSummary {
    /// Session cookies.
    pub fn session_cookies(&self) -> usize {
        self.cookies.iter().filter(|c| c.session).count()
    }
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

/// A vault item's metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMeta {
    /// Item id.
    pub id: String,
    /// Kind.
    pub kind: ItemKind,
    /// Label (`github.com (Chrome, Default)`).
    pub label: String,
    /// Teleport provider id (`chrome`, `firefox`, `slack`).
    pub provider_id: String,
    /// App display name.
    pub app_display: String,
    /// Registrable domain, for site items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// Account or workspace, when detectable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Where it was imported from (profile name).
    #[serde(default)]
    pub source: String,
    /// Contents, without values.
    pub summary: ItemSummary,
    /// Warnings shown with the item (device-bound sessions and so on).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// An identity provider (Google, Microsoft, Apple, Okta, ...): its
    /// session unlocks every federated app, so it never goes into
    /// unattended rules and defaults to a short TTL (red-team F7).
    #[serde(default)]
    pub identity_provider: bool,
    /// Policy.
    pub policy: ItemPolicy,
    /// Created, Unix ms.
    pub created_ms: u64,
    /// Last updated, Unix ms.
    pub updated_ms: u64,
    /// Payload revision.
    pub rev: u64,
    /// SHA-256 of the item record file (detects swaps and rollback).
    pub record_digest: String,
}

impl ItemMeta {
    /// A coarse copy for bulk enumeration: labels, kind, site, account and
    /// counts are kept, but the identifying cookie names and domains, storage
    /// origins and keychain-service names are stripped. That detail set is
    /// itself sensitive (it maps the user's whole logged-in life: which banks,
    /// which employers' SSO, internal hostnames), so it is never returned by a
    /// no-presence bulk `ListItems`; `DescribeItem` returns it behind user
    /// presence (red-team F17).
    pub fn redacted(&self) -> ItemMeta {
        let mut m = self.clone();
        for c in &mut m.summary.cookies {
            c.name = String::new();
            c.domain = String::new();
        }
        m.summary.storage_origins.clear();
        m.summary.keychain_services.clear();
        m.summary.files.clear();
        m
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

/// An item's secret payload: what gets delivered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemPayload {
    /// Provider id (the receiver's importer).
    pub provider_id: String,
    /// `tabs` or `full`.
    pub scope: String,
    /// Bundle entries.
    pub entries: Vec<PayloadEntry>,
}

/// The reserved payload entry of a [`ItemKind::SitePasswords`] item that
/// holds its decrypted saved logins (JSON [`LoginRecord`]s, base64 like every
/// entry). It stays sealed in the vault: teleport never delivers it, and only
/// the site-login fill reads it.
pub const LOGINS_ENTRY: &str = ".cua-keyvault/logins.json";

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
    /// The saved logins this payload holds (empty when it holds none).
    pub fn logins(&self) -> crate::Result<Vec<LoginRecord>> {
        use base64::Engine as _;
        let Some(e) = self.entries.iter().find(|e| e.rel_path == LOGINS_ENTRY) else {
            return Ok(Vec::new());
        };
        let mut bytes = zeroize::Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(&e.data)
                .map_err(|_| crate::Error::Corrupt("saved logins entry is not base64".into()))?,
        );
        let out = serde_json::from_slice(&bytes)
            .map_err(|_| crate::Error::Corrupt("saved logins entry is not valid".into()));
        bytes.zeroize();
        out
    }

    /// A payload entry holding `logins` (for [`LOGINS_ENTRY`]).
    pub fn logins_entry(logins: &[LoginRecord]) -> crate::Result<PayloadEntry> {
        use base64::Engine as _;
        let json = zeroize::Zeroizing::new(serde_json::to_vec(logins)?);
        Ok(PayloadEntry {
            rel_path: LOGINS_ENTRY.into(),
            mode: 0o600,
            data: base64::engine::general_purpose::STANDARD.encode(json.as_slice()),
        })
    }

    /// This payload without its reserved entries: what teleport may deliver.
    pub fn deliverable(mut self) -> ItemPayload {
        self.entries.retain(|e| e.rel_path != LOGINS_ENTRY);
        self
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            disabled: false,
            unlock_policy: UnlockPolicy::Auto,
            auto_lock_minutes: 15,
            auto_wipe: false,
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
}
