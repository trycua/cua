// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The broker's records as the shells see them: the redacted views
//! `cua_keyvault` serves over `keyvault.sock` (snake_case), plus the page's
//! overview wrapper (camelCase). Values are never here: `ListItems` is the
//! redacted view and the broker has no "reveal".
//!
//! These mirror `cua_keyvault`'s serde output field for field; the
//! `wire_drift` test decodes the real types through them.

use serde::{Deserialize, Serialize};

/// A cookie's metadata (never its value).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvCookieInfo {
    /// Name.
    pub name: String,
    /// Domain.
    pub domain: String,
    /// Session cookie.
    pub session: bool,
    /// Expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<i64>,
}

/// What an item holds, as counts and names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvItemSummary {
    /// Cookies.
    #[serde(default)]
    pub cookies: Vec<KvCookieInfo>,
    /// Storage origins.
    #[serde(default)]
    pub storage_origins: Vec<String>,
    /// Saved passwords.
    #[serde(default)]
    pub passwords: u32,
    /// Files.
    #[serde(default)]
    pub files: Vec<String>,
    /// Keychain services.
    #[serde(default)]
    pub keychain_services: Vec<String>,
    /// Bytes.
    #[serde(default)]
    pub bytes: u64,
}

/// An item's policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvItemPolicy {
    /// Allowed targets (empty: any).
    #[serde(default)]
    pub allowed_targets: Vec<String>,
    /// Delivery lifetime.
    pub ttl_secs: u64,
    /// Unattended rules may use it.
    pub unattended: bool,
}

/// One saved item (redacted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvItem {
    /// Id.
    pub id: String,
    /// `browser_site`, `site_passwords`, `app_session`.
    pub kind: String,
    /// Label.
    pub label: String,
    /// Provider.
    pub provider_id: String,
    /// App.
    pub app_display: String,
    /// Site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// Account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Profile.
    #[serde(default)]
    pub source: String,
    /// Summary.
    pub summary: KvItemSummary,
    /// Warnings.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Identity providers always ask.
    #[serde(default)]
    pub identity_provider: bool,
    /// Policy.
    pub policy: KvItemPolicy,
    /// Created.
    pub created_ms: u64,
    /// Updated.
    pub updated_ms: u64,
    /// Revision.
    pub rev: u64,
    /// Digest.
    pub record_digest: String,
}

/// How a caller is signed (`cua_keyvault::caller::Signing`: tagged by
/// `kind`, snake_case).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KvSigning {
    /// Team-signed.
    Signed {
        /// Team.
        team_id: String,
        /// Signing identifier.
        identifier: String,
        /// cdhash.
        cdhash: String,
    },
    /// Ad hoc.
    AdHoc {
        /// Signing identifier.
        identifier: String,
        /// cdhash.
        cdhash: String,
    },
    /// Unsigned.
    Unsigned,
    /// Unknown platform.
    Unknown,
}

/// A verified caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvCaller {
    /// Pid.
    pub pid: i32,
    /// Uid.
    pub uid: u32,
    /// Executable.
    #[serde(default)]
    pub path: Option<String>,
    /// Signing.
    pub signing: KvSigning,
    /// Satisfied the Cua requirement.
    pub first_party: bool,
    /// The OS verified the signature.
    pub os_verified: bool,
    /// Launched by.
    #[serde(default)]
    pub launched_by: Option<String>,
    /// OS-verified leaf name.
    #[serde(default)]
    pub verified_name: Option<String>,
}

/// What a request names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KvSelector {
    /// An existing item.
    Item {
        /// Id.
        id: String,
    },
    /// A site of an app.
    Site {
        /// App.
        app: String,
        /// Site.
        site: String,
        /// Account.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
    /// A whole app.
    App {
        /// App.
        app: String,
    },
    /// A site's saved password, used to sign in (never delivered).
    Login {
        /// Site.
        site: String,
    },
}

/// A caller's request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvAccessRequest {
    /// What.
    pub selectors: Vec<KvSelector>,
    /// Where.
    pub targets: Vec<String>,
    /// Actions.
    #[serde(default)]
    pub actions: Vec<String>,
    /// For how long.
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// Uses.
    #[serde(default)]
    pub uses: Option<u32>,
    /// The caller's reason (unverified).
    #[serde(default)]
    pub reason: String,
    /// The caller's name for itself (unverified).
    #[serde(default)]
    pub claimed_name: Option<String>,
    /// The named agent the request is for (unverified).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// A request waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvPending {
    /// Id.
    pub id: String,
    /// Verified caller.
    pub caller: KvCaller,
    /// Fingerprint.
    pub caller_fp: String,
    /// One-line verified identity.
    pub caller_display: String,
    /// The request.
    pub request: KvAccessRequest,
    /// Existing items it resolves to.
    pub items: Vec<KvItem>,
    /// Selectors approval would import.
    #[serde(default)]
    pub needs_import: Vec<KvSelector>,
    /// Created.
    pub created_ms: u64,
}

/// Live access a request was granted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvGrant {
    /// Id.
    pub id: String,
    /// Request.
    #[serde(default)]
    pub request_id: String,
    /// Caller fingerprint.
    pub caller_fp: String,
    /// Caller.
    pub caller_display: String,
    /// Items.
    pub items: Vec<String>,
    /// Targets.
    pub targets: Vec<String>,
    /// Actions.
    #[serde(default)]
    pub actions: Vec<String>,
    /// Created.
    pub created_ms: u64,
    /// Expires.
    pub not_after_ms: u64,
    /// Uses left (none: unlimited until expiry).
    #[serde(default)]
    pub uses_left: Option<u32>,
    /// Revoked.
    #[serde(default)]
    pub revoked: bool,
    /// The named agent it was approved for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// A caller an unattended rule names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvRuleCaller {
    /// Fingerprint.
    pub fp: String,
    /// Display.
    pub display: String,
}

/// An unattended rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvRule {
    /// Id.
    pub id: String,
    /// Items.
    pub items: Vec<String>,
    /// Targets (`*`: any Space).
    pub targets: Vec<String>,
    /// Callers.
    pub callers: Vec<KvRuleCaller>,
    /// Created.
    pub created_ms: u64,
    /// Expires.
    pub not_after_ms: u64,
    /// Enabled.
    pub enabled: bool,
    /// Note.
    #[serde(default)]
    pub note: String,
}

/// A delivered copy in a Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvDelivery {
    /// Import id.
    pub import_id: String,
    /// Target.
    pub target: String,
    /// Provider.
    pub provider_id: String,
    /// Items.
    pub items: Vec<String>,
    /// Caller fingerprint.
    pub caller_fp: String,
    /// Delivered.
    pub delivered_ms: u64,
    /// Wiped at ([`KV_NO_EXPIRY`]: only when wiped).
    pub expires_ms: u64,
    /// Already wiped.
    #[serde(default)]
    pub wiped: bool,
}

/// A delivery's `expires_ms` when it never expires on its own (auto-wipe
/// off): it stays until wiped.
pub const KV_NO_EXPIRY: u64 = 0;

/// One audit log entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvAuditEntry {
    /// Sequence.
    pub seq: u64,
    /// Time.
    pub ts_ms: u64,
    /// Kind (`consent.allow`, ...).
    pub kind: String,
    /// Actor.
    pub actor: String,
    /// Caller fingerprint.
    #[serde(default)]
    pub caller_fp: String,
    /// Item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `ok`, `allow`, `deny`, `error`.
    pub decision: String,
    /// Detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Written without the MAC key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authless: Option<bool>,
}

/// The broker's status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvStatus {
    /// Version.
    pub version: String,
    /// A vault exists.
    pub initialized: bool,
    /// Unlocked.
    pub unlocked: bool,
    /// The kill switch is on.
    pub disabled: bool,
    /// This app is first party.
    pub caller_first_party: bool,
    /// How the broker sees this app.
    pub caller_display: String,
    /// Items.
    pub items: u32,
    /// Pending requests.
    pub pending: u32,
    /// `auto` or `presence`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlock_policy: Option<String>,
    /// Delivered copies wipe themselves after their TTL (off by default;
    /// none: not told, a broker before the setting or a third party).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_wipe: Option<bool>,
    /// The daemon can create the OS key store protector (setup offers Touch
    /// ID); false for a development daemon, which is passphrase-only.
    #[serde(default)]
    pub os_protector_available: bool,
    /// A passphrase can always be chosen.
    #[serde(default)]
    pub passphrase_available: bool,
    /// Protector kinds that can unlock this vault now (`macos-keychain`,
    /// `windows-credential`, `passphrase`, `recovery`).
    #[serde(default)]
    pub unlock_protectors: Vec<String>,
}

/// The audit chain check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvVerification {
    /// Intact.
    pub ok: bool,
    /// Entries.
    pub entries: u64,
    /// Entries without a MAC.
    pub unauthenticated: u64,
    /// First tampered line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tampered_line: Option<u64>,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Everything the Keyvault page shows, in one read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyvaultOverview {
    /// `ready`, or why not: `not_running`, `impostor`, `connect`,
    /// `no_vault`, `locked`, `not_first_party`, `unsupported`, `error`.
    pub availability: String,
    /// A sentence when not ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The broker's status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<KvStatus>,
    /// The daemon was checked against Cua's signature.
    pub server_verified: bool,
    /// Items.
    #[serde(default)]
    pub items: Vec<KvItem>,
    /// Pending requests.
    #[serde(default)]
    pub pending: Vec<KvPending>,
    /// Grants.
    #[serde(default)]
    pub grants: Vec<KvGrant>,
    /// Rules.
    #[serde(default)]
    pub rules: Vec<KvRule>,
    /// Deliveries.
    #[serde(default)]
    pub deliveries: Vec<KvDelivery>,
    /// Audit tail, oldest first.
    #[serde(default)]
    pub audit: Vec<KvAuditEntry>,
    /// Chain check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_verification: Option<KvVerification>,
    /// Sections that failed while the rest loaded.
    #[serde(default)]
    pub partial_errors: Vec<String>,
}

/// A page action: one broker request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum KvCommand {
    /// Create the vault with the OS key store (the daemon asks for
    /// presence). A passphrase never travels in a command: the shells pass
    /// it to `KeyvaultCommands::setup_with_passphrase` directly.
    Setup,
    /// Unlock with the OS key store (a passphrase goes to
    /// `KeyvaultCommands::unlock_with_passphrase`).
    Unlock,
    /// The kill switch (turning it off asks for Touch ID).
    #[serde(rename_all = "camelCase")]
    SetDisabled {
        /// On.
        disabled: bool,
    },
    /// Per-item unattended (turning on asks for Touch ID).
    #[serde(rename_all = "camelCase")]
    SetUnattended {
        /// Items.
        item_ids: Vec<String>,
        /// On.
        unattended: bool,
    },
    /// Revoke a grant (`*`: all).
    RevokeGrant {
        /// Id.
        id: String,
    },
    /// Remove a rule.
    RemoveRule {
        /// Id.
        id: String,
    },
    /// Auto-wipe of delivered copies (turning it off asks for Touch ID).
    #[serde(rename_all = "camelCase")]
    SetAutoWipe {
        /// On.
        on: bool,
    },
    /// Wipe a Space's copies.
    Release {
        /// Target.
        target: String,
    },
    /// Approve a request for exactly these items (the daemon asks for
    /// presence). `None` keeps everything the request asked for, which the
    /// approval sheet only sends when the user ticked every row.
    #[serde(rename_all = "camelCase")]
    Approve {
        /// Request.
        request_id: String,
        /// Items to keep.
        items: Option<Vec<String>>,
    },
    /// Deny a request.
    #[serde(rename_all = "camelCase")]
    Deny {
        /// Request.
        request_id: String,
    },
}
