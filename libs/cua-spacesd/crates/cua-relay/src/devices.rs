// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Client-device enrollment of the account mode.
//!
//! A device that acts as a client of an account (lists its machines,
//! connects to them) holds a P-256 key pair (the OS keychain on macOS and
//! Windows, a 0600 file elsewhere) and is enrolled with a second factor:
//!
//! - approval from an already enrolled device of the same account, by the
//!   one-time code the new device shows or by picking it from the pending
//!   list;
//! - a fresh interactive sign-in (`auth_time` of the account token no older
//!   than the bootstrap window), plus proof of the device key, the way
//!   signing in adds a device to a tailnet. Three cases need no approval:
//!   an account's first device (or its only one, after it expired); a
//!   re-key of a machine that already has a live device on the account
//!   (same machine id, a new key -- another build, or the old key was
//!   lost); and, for a verified email, a token whose authentication context
//!   proves a second factor (`acr`/`amr`; see [`crate::oidc::Identity::mfa`]).
//!   A brand-new device on an account that already has a strong enrolled
//!   device, from a sign-in that does not prove a second factor, still
//!   needs an approval: a verified email alone is not enough (closes the
//!   "any account compromise silently enrolls a device" gap). During the
//!   migration grace period (ended by default; see [`DevicePolicy::grace_secs`])
//!   an account with no enrolled device may also bootstrap without a fresh
//!   sign-in (audited).
//!
//! A device may report a machine id (a hash of the host's hardware or
//! install identity, keyed per account here). When a new key of the same
//! machine enrolls (another build of the app keeps its own key, the key was
//! lost), it replaces the machine's older records: they are revoked as
//! superseded, their sessions end, and listings show the machine once. The
//! machine id is a claim, never a factor for *whom* it replaces; it is,
//! however, one of the ways a fresh sign-in alone is allowed to enroll
//! without an approval (a re-key of a machine the account already trusts).
//!
//! Enrollment lasts `device_ttl` (30 days by default); after that the device
//! needs one approval or a fresh sign-in again. Per session the device proves its key
//! (`POST /v1/devices/session`, a signed timestamp) and gets a short-lived
//! opaque session token it sends as `x-cua-device-session` with its account
//! token. Hosts are never involved: a hosted-only machine registers with its
//! machine token and is never enrolled as a client.
//!
//! Every enrollment change, device session and machine access is recorded
//! in the account's audit log (bounded, persisted with the devices).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::assertion::{now_secs, write_private};

/// Header carrying the device session token.
pub const DEVICE_SESSION_HEADER: &str = "x-cua-device-session";
/// Response header set while an unenrolled device is let through during the
/// migration grace period: `required; enforce-after=<unix seconds>`.
pub const ENROLLMENT_HEADER: &str = "x-cua-device-enrollment";
/// Device id prefix.
pub const DEVICE_ID_PREFIX: &str = "dev_";
/// Audit events kept per account (S9; raised from an earlier 500 so a
/// chattier account does not push older, still-relevant events out of
/// range as quickly; export the full chain with `GET /v1/audit?format=jsonl`
/// before it does either way).
pub const AUDIT_LIMIT: usize = 5_000;
/// How long a one-time enrollment code is valid.
pub const CODE_TTL_SECS: u64 = 600;
/// Accepted clock skew of signed timestamps.
pub const PROOF_SKEW_SECS: u64 = 120;
/// Minimum time between two recorded accesses of one machine by one
/// device (or user).
pub const ACCESS_AUDIT_INTERVAL_SECS: u64 = 600;
/// Devices one account may register.
pub const MAX_DEVICES_PER_ACCOUNT: usize = 64;
/// `enrolled_by` of a device enrolled by a fresh interactive sign-in.
pub const BY_FRESH_SIGN_IN: &str = "bootstrap:fresh-sign-in";
/// `enrolled_by` of a device enrolled during the migration grace period.
pub const BY_GRACE: &str = "bootstrap:grace";

/// Domain-separated message a device signs to register itself. (The
/// account is not part of it: a device key belongs to one account, which
/// the store enforces.)
pub fn register_message(device: &str, ts: u64) -> String {
    format!("cua-device-register/v1\n{device}\n{ts}")
}

/// Domain-separated message a device signs to open a session.
pub fn session_message(device: &str, ts: u64) -> String {
    format!("cua-device-session/v1\n{device}\n{ts}")
}

/// The device id of an uncompressed P-256 public key.
pub fn device_id(public_key: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, public_key);
    format!("{DEVICE_ID_PREFIX}{}", &hex::encode(digest.as_ref())[..24])
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// Decodes a base64url uncompressed P-256 public key (65 bytes).
pub fn decode_public_key(key: &str) -> Result<Vec<u8>, String> {
    let raw = b64()
        .decode(key.trim())
        .map_err(|_| "public_key is not base64url".to_owned())?;
    if raw.len() != 65 || raw[0] != 4 {
        return Err("public_key must be an uncompressed P-256 point".into());
    }
    Ok(raw)
}

/// Verifies a base64url fixed-size (r‖s) ECDSA P-256 SHA-256 signature.
pub fn verify(public_key: &[u8], message: &str, signature: &str) -> bool {
    let Ok(sig) = b64().decode(signature.trim()) else {
        return false;
    };
    ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, public_key)
        .verify(message.as_bytes(), &sig)
        .is_ok()
}

fn sha256_hex(text: &str) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, text.as_bytes()).as_ref())
}

/// The stored form of a device's reported machine id: keyed by the account,
/// so one machine used with two accounts cannot be linked across them.
/// `None` for an empty or oversized id.
fn machine_key(account: &str, machine_id: &str) -> Option<String> {
    let id = machine_id.trim();
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return None;
    }
    Some(sha256_hex(&format!(
        "cua-device-machine/v1\n{account}\n{id}"
    )))
}

/// Where a device stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    /// Registered, waiting for an approval.
    Pending,
    /// Enrolled and within its TTL.
    Enrolled,
    /// Enrolled once; re-verification is due.
    Expired,
    /// Revoked (terminal).
    Revoked,
}

/// A client device of an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    /// `dev_…`, derived from the public key.
    pub id: String,
    /// Owning account.
    pub account: String,
    /// User who registered it.
    pub user: String,
    /// Display name.
    pub name: String,
    /// Uncompressed P-256 public key, base64url.
    pub public_key: String,
    /// Registration time.
    pub created_at: u64,
    /// Last enrollment (approval or bootstrap).
    #[serde(default)]
    pub enrolled_at: Option<u64>,
    /// End of the current enrollment.
    #[serde(default)]
    pub enrolled_until: Option<u64>,
    /// Approving device id, or [`BY_FRESH_SIGN_IN`] / [`BY_GRACE`].
    #[serde(default)]
    pub enrolled_by: Option<String>,
    /// Revocation time.
    #[serde(default)]
    pub revoked_at: Option<u64>,
    /// The device of the same machine that replaced this one (a re-key);
    /// set with `revoked_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// The machine the device runs on ([`machine_key`] of the id it
    /// reported), when it reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// Enrolled through a real second factor: a fresh sign-in, or an
    /// approval from a device that was. Devices bootstrapped during the
    /// grace period (and those they approve) are not.
    #[serde(default)]
    pub strong: bool,
    /// Last session opened.
    #[serde(default)]
    pub last_seen: Option<u64>,
    /// Operating system the device reported (`macos`, `windows`, `linux`).
    #[serde(default)]
    pub platform: String,
    /// SHA-256 of the pending one-time code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_sha256: Option<String>,
    /// When the pending code expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_expires: Option<u64>,
}

impl DeviceRecord {
    /// The state at `now`.
    pub fn state(&self, now: u64) -> DeviceState {
        if self.revoked_at.is_some() {
            return DeviceState::Revoked;
        }
        match self.enrolled_until {
            Some(until) if until > now => DeviceState::Enrolled,
            Some(_) => DeviceState::Expired,
            None => DeviceState::Pending,
        }
    }
}

/// One audit event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unix seconds.
    pub ts: u64,
    /// `device_registered`, `device_enrolled`, `device_rekeyed`,
    /// `device_renamed`, `device_revoked`, `device_session`, `machine_access`,
    /// `shared_access`, `unenrolled_access`, `share_added`,
    /// `share_removed`, `machine_registered`, `machine_removed`,
    /// `sharing_stopped`, `sharing_started`.
    pub kind: String,
    /// Acting device, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Machine concerned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// Other party (user id or email, approving device, allowlist entry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Free-form detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AuditEvent {
    /// An event of `kind` now.
    pub fn new(kind: &str) -> Self {
        Self {
            ts: now_secs(),
            kind: kind.into(),
            device: None,
            machine: None,
            subject: None,
            detail: None,
        }
    }

    /// With the acting device.
    pub fn device(mut self, device: Option<&str>) -> Self {
        self.device = device.map(str::to_owned);
        self
    }

    /// With the machine.
    pub fn machine(mut self, machine: &str) -> Self {
        self.machine = Some(machine.to_owned());
        self
    }

    /// With the other party.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// With a detail.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The "previous hash" of an account's first audit entry ever.
pub const AUDIT_GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One audit log entry, hash-chained (S9), like the Keyvault's own
/// (`cua_keyvault::audit`): `hash` covers `prev`, `seq` and this entry's
/// event; `mac` is an HMAC-SHA256 of `hash` under a key only the running
/// relay holds (generated on first use, next to the devices file, 0600;
/// never derivable from the devices/audit log files alone). Editing,
/// reordering or deleting an entry breaks the hash chain from that point
/// on for anyone re-deriving it from the log file; producing a `mac` that
/// still verifies needs the relay's own audit key, so a copy of the log
/// file alone (a backup, a disk snapshot) cannot be edited undetectably.
/// Retention still trims the oldest entries ([`AUDIT_LIMIT`], raised from
/// an earlier 500): the oldest *retained* entry's own `hash`/`mac` are
/// still checked by [`DeviceStore::verify_audit_chain`], but nothing before
/// it is, since it is gone.
///
/// `#[serde(default)]` on the chain fields lets this also deserialize a
/// pre-S9 devices file, whose audit entries are plain [`AuditEvent`]s with
/// none of them: those come back with `seq == 0`, a sentinel
/// [`chain_append`] never produces (it starts at 1), which
/// [`DeviceStore::open`] uses to detect and migrate them (rebuild the chain
/// from the existing events) instead of refusing to start on an upgrade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// 1-based, per account, never reused (kept across trims). `0` for an
    /// entry read from a pre-S9 file, before [`DeviceStore::open`] migrates
    /// it.
    #[serde(default)]
    pub seq: u64,
    /// The event.
    #[serde(flatten)]
    pub event: AuditEvent,
    /// The previous *retained* entry's `hash`, or [`AUDIT_GENESIS`].
    /// Empty for an unmigrated pre-S9 entry.
    #[serde(default)]
    pub prev: String,
    /// SHA-256 of `prev`, `seq` and the event's canonical JSON, hex.
    /// Empty for an unmigrated pre-S9 entry.
    #[serde(default)]
    pub hash: String,
    /// HMAC-SHA256 of `hash` under the relay's audit key, hex. Empty for an
    /// unmigrated pre-S9 entry.
    #[serde(default)]
    pub mac: String,
}

impl std::ops::Deref for AuditEntry {
    type Target = AuditEvent;
    fn deref(&self) -> &AuditEvent {
        &self.event
    }
}

fn chain_hash(prev: &str, seq: u64, event: &AuditEvent) -> String {
    let body = serde_json::to_vec(event).expect("audit event serializes");
    let mut input = Vec::with_capacity(prev.len() + 8 + body.len());
    input.extend_from_slice(prev.as_bytes());
    input.extend_from_slice(&seq.to_be_bytes());
    input.extend_from_slice(&body);
    hex::encode(ring::digest::digest(&ring::digest::SHA256, &input).as_ref())
}

fn chain_mac(key: &[u8; 32], hash: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    hex::encode(ring::hmac::sign(&key, hash.as_bytes()).as_ref())
}

/// Appends `event` to `log`, hash-chained from its current tail.
fn chain_append(log: &mut Vec<AuditEntry>, event: AuditEvent, audit_key: &[u8; 32]) {
    let prev = log
        .last()
        .map(|e| e.hash.clone())
        .unwrap_or_else(|| AUDIT_GENESIS.to_owned());
    let seq = log.last().map(|e| e.seq).unwrap_or(0) + 1;
    let hash = chain_hash(&prev, seq, &event);
    let mac = chain_mac(audit_key, &hash);
    log.push(AuditEntry {
        seq,
        event,
        prev,
        hash,
        mac,
    });
}

/// Why chain verification failed: the entry's sequence number and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainBroken {
    /// The first entry found broken.
    pub seq: u64,
    /// `"mac"` (the entry was edited, or produced with a different audit
    /// key) or `"prev"` (an entry is missing, reordered, or its neighbor
    /// was edited).
    pub reason: &'static str,
}

/// Verifies `log`'s hash chain is internally consistent under `audit_key`:
/// every entry's `mac` matches its `hash`, and every entry after the first
/// chains from its predecessor's `hash`. The first retained entry's own
/// `prev` is not checked against anything earlier (it may be
/// [`AUDIT_GENESIS`], or the hash of an entry retention already dropped).
pub fn verify_audit_chain(log: &[AuditEntry], audit_key: &[u8; 32]) -> Result<(), ChainBroken> {
    let mut prev_hash: Option<&str> = None;
    for entry in log {
        if chain_hash(&entry.prev, entry.seq, &entry.event) != entry.hash
            || chain_mac(audit_key, &entry.hash) != entry.mac
        {
            return Err(ChainBroken {
                seq: entry.seq,
                reason: "mac",
            });
        }
        if let Some(prev) = prev_hash {
            if entry.prev != prev {
                return Err(ChainBroken {
                    seq: entry.seq,
                    reason: "prev",
                });
            }
        }
        prev_hash = Some(&entry.hash);
    }
    Ok(())
}

/// Rebuilds the hash chain (S9) for every account whose persisted log
/// predates it. A pre-S9 devices file's audit entries are plain
/// [`AuditEvent`]s with no `seq`/`prev`/`hash`/`mac`; [`AuditEntry`]'s
/// `#[serde(default)]` on those fields lets them deserialize at all, but
/// leaves every one with the sentinel `seq == 0` ([`chain_append`] starts
/// at 1 and never produces it). An account whose log has one is migrated in
/// place: its events, in their existing order, are re-appended through
/// [`chain_append`] so the log becomes a genuine chain under `audit_key`.
/// Mixing migrated and already-chained entries in one log never happens in
/// practice (a file is either pre-S9 throughout or not), so finding the
/// sentinel anywhere in an account's log is enough to rebuild that whole
/// log rather than require it to already be entry-by-entry consistent.
/// Returns whether anything was migrated (the caller persists the result).
fn migrate_legacy_audit(state: &mut State, audit_key: &[u8; 32]) -> bool {
    let mut migrated = false;
    for log in state.audit.values_mut() {
        if log.iter().any(|entry| entry.seq == 0) {
            let events: Vec<AuditEvent> =
                std::mem::take(log).into_iter().map(|e| e.event).collect();
            for event in events {
                chain_append(log, event, audit_key);
            }
            migrated = true;
        }
    }
    migrated
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct State {
    /// Start of the migration grace period (first start with devices).
    #[serde(default)]
    grace_started_at: u64,
    #[serde(default)]
    devices: BTreeMap<String, DeviceRecord>,
    #[serde(default)]
    audit: BTreeMap<String, Vec<AuditEntry>>,
}

#[derive(Debug, Clone)]
struct Session {
    account: String,
    device: String,
    expires: u64,
}

/// Errors, mapped to HTTP statuses by the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    /// 400.
    Invalid(String),
    /// 401: bad proof.
    Proof(String),
    /// 403.
    Forbidden(String),
    /// 404.
    NotFound,
    /// 404: no pending device shows this code.
    CodeNotFound,
    /// 409.
    Conflict(String),
    /// 500.
    Storage(String),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m)
            | Self::Proof(m)
            | Self::Forbidden(m)
            | Self::Conflict(m)
            | Self::Storage(m) => f.write_str(m),
            Self::NotFound => f.write_str("no such device"),
            Self::CodeNotFound => f.write_str(
                "no device is waiting with this code (codes expire after 10 minutes); \
                 approve by id instead (`cua devices approve <device id>`, ids in `cua devices ls`)",
            ),
        }
    }
}

/// How a registration ended.
#[derive(Debug, Clone)]
pub struct Registered {
    /// The device.
    pub device: DeviceRecord,
    /// One-time code to confirm from an enrolled device (pending only).
    pub code: Option<String>,
    /// Older devices of the same machine this enrollment replaced.
    pub superseded: Vec<String>,
}

/// A device's registration request, its key already proven by the caller's
/// signature over [`register_message`].
#[derive(Debug, Clone, Default)]
pub struct Registration<'a> {
    /// Owning account.
    pub account: &'a str,
    /// User registering it.
    pub user: &'a str,
    /// Uncompressed P-256 public key, base64url.
    pub public_key: &'a str,
    /// Display name.
    pub name: &'a str,
    /// Operating system (`macos`, `windows`, `linux`).
    pub platform: &'a str,
    /// The machine id the device reports, if any.
    pub machine_id: Option<&'a str>,
    /// Signed timestamp.
    pub ts: u64,
    /// Signature over [`register_message`].
    pub sig: &'a str,
    /// Asks to enroll without an approval.
    pub bootstrap: bool,
    /// The account token's `auth_time`.
    pub auth_time: Option<u64>,
    /// The issuer verified the account's email.
    pub email_verified: bool,
    /// The account token's authentication context proves a second factor
    /// ([`crate::oidc::Identity::mfa`]).
    pub mfa: bool,
}

/// Device store settings.
#[derive(Debug, Clone, Copy)]
pub struct DevicePolicy {
    /// Enrollment lifetime.
    pub ttl_secs: u64,
    /// Migration grace period (unenrolled access allowed, flagged).
    pub grace_secs: u64,
    /// Maximum `auth_time` age for a bootstrap enrollment.
    pub bootstrap_max_auth_age_secs: u64,
    /// Device session lifetime.
    pub session_ttl_secs: u64,
}

impl Default for DevicePolicy {
    fn default() -> Self {
        Self {
            ttl_secs: 30 * 86_400,
            // Safe default: ended. The migration grace period (S3) is a
            // time-boxed exception for the rollout, not a steady state: a
            // stolen account token alone reaches every machine while it
            // runs. Set `CUA_RELAY_DEVICE_GRACE_DAYS` explicitly (and only
            // for the rollout window) to open one.
            grace_secs: 0,
            bootstrap_max_auth_age_secs: 600,
            session_ttl_secs: 3600,
        }
    }
}

/// The device store.
#[derive(Debug)]
pub struct DeviceStore {
    path: Option<PathBuf>,
    policy: DevicePolicy,
    state: Mutex<State>,
    sessions: Mutex<HashMap<String, Session>>,
    /// Signed timestamps already used (replay guard), with their expiry.
    proofs: Mutex<HashMap<String, u64>>,
    /// Last recorded access per (account, actor, machine).
    throttle: Mutex<HashMap<String, u64>>,
    /// HMAC key for the audit hash chain (S9): random per process for an
    /// in-memory store, loaded or created (0600, next to the devices file)
    /// for a persisted one.
    audit_key: [u8; 32],
}

fn random_audit_key() -> [u8; 32] {
    use rand::RngCore as _;
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
}

/// The audit key next to `devices_path` (created 0600 on first use).
fn load_or_create_audit_key(devices_path: &std::path::Path) -> std::io::Result<[u8; 32]> {
    let key_path = PathBuf::from(format!("{}.audit-key", devices_path.display()));
    if let Ok(raw) = std::fs::read(&key_path) {
        return raw.try_into().map_err(|raw: Vec<u8>| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "audit key {} is {} bytes, not 32",
                    key_path.display(),
                    raw.len()
                ),
            )
        });
    }
    let key = random_audit_key();
    write_private(&key_path, &key)?;
    Ok(key)
}

fn new_code() -> String {
    use rand::Rng as _;
    // No 0/O, 1/I/L: read aloud or typed from another screen.
    const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rngs::OsRng;
    let mut code = String::with_capacity(9);
    for i in 0..8 {
        if i == 4 {
            code.push('-');
        }
        code.push(ALPHABET[rng.gen_range(0..ALPHABET.len())] as char);
    }
    code
}

fn normalize_code(code: &str) -> String {
    let compact: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if compact.len() == 8 {
        format!("{}-{}", &compact[..4], &compact[4..])
    } else {
        compact
    }
}

fn new_session_token() -> String {
    use rand::RngCore as _;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("cds_{}", b64().encode(bytes))
}

fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A reported platform: a short lower-case word (`macos`, `windows`,
/// `linux`), else empty.
fn clean_platform(platform: &str) -> String {
    let p = platform.trim().to_ascii_lowercase();
    if p.len() <= 16 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        p
    } else {
        String::new()
    }
}

impl DeviceStore {
    /// An in-memory store whose grace period starts now.
    pub fn in_memory(policy: DevicePolicy) -> Self {
        Self::with_state(
            None,
            policy,
            State {
                grace_started_at: now_secs(),
                ..State::default()
            },
            random_audit_key(),
        )
    }

    fn with_state(
        path: Option<PathBuf>,
        policy: DevicePolicy,
        state: State,
        audit_key: [u8; 32],
    ) -> Self {
        Self {
            path,
            policy,
            state: Mutex::new(state),
            sessions: Mutex::default(),
            proofs: Mutex::default(),
            throttle: Mutex::default(),
            audit_key,
        }
    }

    /// Loads (or starts, recording the start of the grace period) the store
    /// persisted at `path`.
    pub fn open(path: PathBuf, policy: DevicePolicy) -> std::io::Result<Self> {
        let mut state = match std::fs::read(&path) {
            Ok(raw) => serde_json::from_slice::<State>(&raw)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e),
        };
        let fresh = state.grace_started_at == 0;
        let audit_key = load_or_create_audit_key(&path)?;
        // S9 landed after accounts already had audit history: migrate a
        // pre-chain log (see `migrate_legacy_audit`) instead of failing to
        // deserialize it and refusing to start.
        let migrated = migrate_legacy_audit(&mut state, &audit_key);
        let store = Self::with_state(Some(path), policy, state, audit_key);
        if fresh || migrated {
            let mut state = store.state.lock().expect("devices");
            if fresh {
                state.grace_started_at = now_secs();
            }
            store
                .persist(&state)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        Ok(store)
    }

    /// The policy.
    pub fn policy(&self) -> DevicePolicy {
        self.policy
    }

    /// When unenrolled access stops being let through.
    pub fn enforce_after(&self) -> u64 {
        self.state
            .lock()
            .expect("devices")
            .grace_started_at
            .saturating_add(self.policy.grace_secs)
    }

    /// Whether the migration grace period is still running.
    pub fn in_grace(&self, now: u64) -> bool {
        now < self.enforce_after()
    }

    fn persist(&self, state: &State) -> Result<(), DeviceError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let raw =
            serde_json::to_vec_pretty(state).map_err(|e| DeviceError::Storage(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DeviceError::Storage(e.to_string()))?;
        }
        write_private(path, &raw).map_err(|e| DeviceError::Storage(e.to_string()))
    }

    /// Applies `change` to a copy of the state and persists it before
    /// making it current.
    fn mutate<T>(
        &self,
        change: impl FnOnce(&mut State) -> Result<T, DeviceError>,
    ) -> Result<T, DeviceError> {
        let mut state = self.state.lock().expect("devices");
        let mut next = state.clone();
        let out = change(&mut next)?;
        self.persist(&next)?;
        *state = next;
        Ok(out)
    }

    fn push_audit(state: &mut State, account: &str, event: AuditEvent, audit_key: &[u8; 32]) {
        let log = state.audit.entry(account.to_owned()).or_default();
        chain_append(log, event, audit_key);
        if log.len() > AUDIT_LIMIT {
            let excess = log.len() - AUDIT_LIMIT;
            log.drain(..excess);
        }
    }

    /// Records `event` in `account`'s audit log (best effort).
    pub fn audit(&self, account: &str, event: AuditEvent) {
        let audit_key = self.audit_key;
        if let Err(e) = self.mutate(|s| {
            Self::push_audit(s, account, event, &audit_key);
            Ok(())
        }) {
            tracing::warn!(error = %e, "audit event not persisted");
        }
    }

    /// Records `event` unless the same `key` was recorded within
    /// [`ACCESS_AUDIT_INTERVAL_SECS`].
    pub fn audit_throttled(&self, key: &str, account: &str, event: AuditEvent) {
        let now = now_secs();
        {
            let mut throttle = self.throttle.lock().expect("throttle");
            if throttle
                .get(key)
                .is_some_and(|last| now.saturating_sub(*last) < ACCESS_AUDIT_INTERVAL_SECS)
            {
                return;
            }
            if throttle.len() > 10_000 {
                throttle.retain(|_, last| now.saturating_sub(*last) < ACCESS_AUDIT_INTERVAL_SECS);
            }
            throttle.insert(key.to_owned(), now);
        }
        self.audit(account, event);
    }

    /// `account`'s audit log, hash-chained entries, newest last (at most
    /// `limit`; S9).
    pub fn audit_log(&self, account: &str, limit: usize) -> Vec<AuditEntry> {
        let state = self.state.lock().expect("devices");
        let log = state.audit.get(account).cloned().unwrap_or_default();
        let skip = log.len().saturating_sub(limit);
        log.into_iter().skip(skip).collect()
    }

    /// Verifies `account`'s full retained audit chain (S9). `Ok(())` for an
    /// empty log.
    pub fn verify_audit_chain(&self, account: &str) -> Result<(), ChainBroken> {
        let state = self.state.lock().expect("devices");
        let empty = Vec::new();
        let log = state.audit.get(account).unwrap_or(&empty);
        verify_audit_chain(log, &self.audit_key)
    }

    /// A device by id.
    pub fn get(&self, id: &str) -> Option<DeviceRecord> {
        self.state.lock().expect("devices").devices.get(id).cloned()
    }

    /// `account`'s devices: revoked ones included, those a newer key of the
    /// same machine replaced left out (the audit log keeps them).
    pub fn list(&self, account: &str) -> Vec<DeviceRecord> {
        self.state
            .lock()
            .expect("devices")
            .devices
            .values()
            .filter(|d| d.account == account && d.superseded_by.is_none())
            .cloned()
            .collect()
    }

    fn has_enrolled(state: &State, account: &str, now: u64, strong_only: bool) -> bool {
        state.devices.values().any(|d| {
            d.account == account
                && d.state(now) == DeviceState::Enrolled
                && (d.strong || !strong_only)
        })
    }

    fn check_proof(
        &self,
        key: &[u8],
        message: &str,
        ts: u64,
        sig: &str,
    ) -> Result<(), DeviceError> {
        let now = now_secs();
        if ts.abs_diff(now) > PROOF_SKEW_SECS {
            return Err(DeviceError::Proof(
                "the device clock is off or the proof is stale".into(),
            ));
        }
        if !verify(key, message, sig) {
            return Err(DeviceError::Proof("invalid device signature".into()));
        }
        // Keyed by the signature's `r` (the first half): an ECDSA signature
        // has a second valid form with the same `r`, so the whole signature
        // would not catch a replay of that form.
        let raw = b64().decode(sig.trim()).unwrap_or_default();
        let r = &raw[..raw.len().min(32)];
        let mut proofs = self.proofs.lock().expect("proofs");
        proofs.retain(|_, exp| *exp > now);
        let fingerprint = hex::encode(ring::digest::digest(&ring::digest::SHA256, r).as_ref());
        if proofs.contains_key(&fingerprint) {
            return Err(DeviceError::Proof("device proof already used".into()));
        }
        proofs.insert(fingerprint, now + 2 * PROOF_SKEW_SECS + 1);
        Ok(())
    }

    /// Revokes `account`'s other live devices on `machine` as superseded by
    /// `by` (a re-key of that machine), recording each; returns their ids.
    fn supersede(
        state: &mut State,
        account: &str,
        by: &str,
        machine: Option<&str>,
        now: u64,
        audit_key: &[u8; 32],
    ) -> Vec<String> {
        let Some(machine) = machine else {
            return Vec::new();
        };
        let mut replaced = Vec::new();
        for d in state.devices.values_mut() {
            if d.account == account
                && d.id != by
                && d.revoked_at.is_none()
                && d.machine.as_deref() == Some(machine)
            {
                d.revoked_at = Some(now);
                d.superseded_by = Some(by.to_owned());
                d.code_sha256 = None;
                d.code_expires = None;
                replaced.push((d.id.clone(), d.name.clone()));
            }
        }
        for (id, name) in &replaced {
            Self::push_audit(
                state,
                account,
                AuditEvent::new("device_rekeyed")
                    .device(Some(by))
                    .subject(id.clone())
                    .detail(format!("replaced {name} on the same machine")),
                audit_key,
            );
        }
        replaced.into_iter().map(|(id, _)| id).collect()
    }

    fn end_sessions(&self, devices: &[String]) {
        if devices.is_empty() {
            return;
        }
        self.sessions
            .lock()
            .expect("sessions")
            .retain(|_, s| !devices.contains(&s.device));
    }

    /// Registers (or re-registers) a device, its key proven by `r.sig` over
    /// [`register_message`].
    ///
    /// - A revoked key never comes back.
    /// - `bootstrap` asks to enroll without an approval. A fresh sign-in
    ///   (`auth_time` within the bootstrap window) grants it: for the
    ///   account's first device (while no device enrolled through a real
    ///   second factor exists), and for any other device when the account's
    ///   email is verified. During the migration grace period an account
    ///   with no enrolled device bootstraps without one.
    /// - A fresh sign-in newer than an enrolled device's enrollment
    ///   re-verifies it; otherwise an enrolled device stays as it is.
    /// - Else the device is (again) pending with a fresh one-time code.
    ///
    /// An enrollment replaces the older devices of the same machine.
    pub fn register(&self, r: &Registration<'_>) -> Result<Registered, DeviceError> {
        let account = r.account;
        let raw = decode_public_key(r.public_key).map_err(DeviceError::Invalid)?;
        let id = device_id(&raw);
        self.check_proof(&raw, &register_message(&id, r.ts), r.ts, r.sig)?;
        let now = now_secs();
        let name = clean_name(r.name);
        let grace = self.in_grace(now);
        let policy = self.policy;
        let machine = r.machine_id.and_then(|m| machine_key(account, m));
        let audit_key = self.audit_key;
        let out = self.mutate(|state| {
            if let Some(existing) = state.devices.get(&id) {
                if existing.account != account {
                    return Err(DeviceError::Conflict(
                        "this device key belongs to another account".into(),
                    ));
                }
                if existing.state(now) == DeviceState::Revoked {
                    return Err(DeviceError::Forbidden(match &existing.superseded_by {
                        Some(by) => format!(
                            "this device key was replaced by {by} on the same machine; create a new device key to enroll again"
                        ),
                        None => {
                            "this device was revoked; create a new device key to enroll again".into()
                        }
                    }));
                }
            } else if state
                .devices
                .values()
                .filter(|d| d.account == account && d.revoked_at.is_none())
                .count()
                >= MAX_DEVICES_PER_ACCOUNT
            {
                return Err(DeviceError::Forbidden(format!(
                    "an account may register at most {MAX_DEVICES_PER_ACCOUNT} devices; revoke one first"
                )));
            }
            let fresh = !state.devices.contains_key(&id);
            let mut device = state.devices.get(&id).cloned().unwrap_or(DeviceRecord {
                id: id.clone(),
                account: account.to_owned(),
                user: r.user.to_owned(),
                name: if name.is_empty() {
                    "Unnamed device".into()
                } else {
                    name.clone()
                },
                public_key: r.public_key.trim().to_owned(),
                created_at: now,
                enrolled_at: None,
                enrolled_until: None,
                enrolled_by: None,
                revoked_at: None,
                superseded_by: None,
                machine: None,
                strong: false,
                last_seen: None,
                platform: String::new(),
                code_sha256: None,
                code_expires: None,
            });
            if !name.is_empty() {
                device.name = name.clone();
            }
            let platform = clean_platform(r.platform);
            if !platform.is_empty() {
                device.platform = platform;
            }
            if machine.is_some() {
                device.machine = machine.clone();
            }
            if fresh {
                Self::push_audit(
                    state,
                    account,
                    AuditEvent::new("device_registered")
                        .device(Some(&id))
                        .detail(device.name.clone()),
                    &audit_key,
                );
            }
            let fresh_sign_in = r.auth_time.filter(|t| {
                *t <= now + PROOF_SKEW_SECS
                    && now.saturating_sub(*t) <= policy.bootstrap_max_auth_age_secs
            });
            let enrolled = device.state(now) == DeviceState::Enrolled;
            if enrolled {
                // A sign-in newer than the enrollment re-verifies it.
                let reverify = r.bootstrap
                    && fresh_sign_in.is_some_and(|t| t > device.enrolled_at.unwrap_or(0));
                if !reverify {
                    state.devices.insert(id.clone(), device.clone());
                    return Ok(Registered {
                        device,
                        code: None,
                        superseded: Vec::new(),
                    });
                }
            }
            // The same machine already has a live device on this account: a
            // re-key (another build kept its own key, or the old one was
            // lost), not a new device, so it needs no extra proof (S4).
            let is_rekey = machine.as_deref().is_some_and(|m| {
                state
                    .devices
                    .values()
                    .any(|d| d.account == account && d.id != id && d.revoked_at.is_none() && d.machine.as_deref() == Some(m))
            });
            // A fresh sign-in enrolls: the first device (while no device
            // enrolled through a real second factor exists, so an owner can
            // recover from devices someone enrolled on the grace period's
            // word, and revoke them); a re-key of an already-enrolled
            // machine; and, for a brand-new device on an account that
            // already has a strong enrolled device, only when the token
            // also proves a second factor (`Identity::mfa`) -- a verified
            // email alone is no longer enough for that case (S4). The other
            // route for a brand-new device is an approval from an enrolled
            // device ([`Self::approve`]). The grace period bootstraps only
            // an account's very first device.
            let by = if !r.bootstrap {
                None
            } else if fresh_sign_in.is_some()
                && (enrolled
                    || is_rekey
                    || !Self::has_enrolled(state, account, now, true)
                    || (r.email_verified && r.mfa))
            {
                Some(BY_FRESH_SIGN_IN)
            } else if grace && !Self::has_enrolled(state, account, now, false) {
                Some(BY_GRACE)
            } else {
                None
            };
            if let Some(by) = by {
                device.enrolled_at = Some(now);
                device.enrolled_until = Some(now + policy.ttl_secs);
                device.enrolled_by = Some(by.into());
                device.strong = by == BY_FRESH_SIGN_IN;
                device.code_sha256 = None;
                device.code_expires = None;
                Self::push_audit(
                    state,
                    account,
                    AuditEvent::new("device_enrolled")
                        .device(Some(&id))
                        .subject(by)
                        .detail(match (by, enrolled) {
                            (BY_FRESH_SIGN_IN, true) => "re-verified by fresh sign-in",
                            (BY_FRESH_SIGN_IN, false) => "enrolled by fresh sign-in",
                            _ => "enrolled during the grace period",
                        }),
                    &audit_key,
                );
                state.devices.insert(id.clone(), device.clone());
                let superseded = Self::supersede(
                    state,
                    account,
                    &id,
                    device.machine.as_deref(),
                    now,
                    &audit_key,
                );
                return Ok(Registered {
                    device,
                    code: None,
                    superseded,
                });
            }
            let code = new_code();
            device.code_sha256 = Some(sha256_hex(&code));
            device.code_expires = Some(now + CODE_TTL_SECS);
            state.devices.insert(id.clone(), device.clone());
            Ok(Registered {
                device,
                code: Some(code),
                superseded: Vec::new(),
            })
        })?;
        self.end_sessions(&out.superseded);
        Ok(out)
    }

    /// `approver` (an enrolled device of `account`) approves the pending or
    /// expired device named by `code` or `target`. A code that is a device
    /// id (`dev_…`) names that device.
    pub fn approve(
        &self,
        account: &str,
        approver: &str,
        code: Option<&str>,
        target: Option<&str>,
    ) -> Result<DeviceRecord, DeviceError> {
        let now = now_secs();
        let ttl = self.policy.ttl_secs;
        let (code, target) = match (code.map(str::trim), target) {
            (Some(c), None) if c.starts_with(DEVICE_ID_PREFIX) => (None, Some(c)),
            (c, t) => (c, t),
        };
        let audit_key = self.audit_key;
        let (approved, superseded) = self.mutate(|state| {
            let approving = state
                .devices
                .get(approver)
                .filter(|d| d.account == account && d.state(now) == DeviceState::Enrolled)
                .ok_or_else(|| DeviceError::Forbidden("approve from an enrolled device".into()))?;
            let approving_id = approving.id.clone();
            let approving_strong = approving.strong;
            let id = match (code, target) {
                (Some(code), _) => {
                    let hash = sha256_hex(&normalize_code(code));
                    state
                        .devices
                        .values()
                        .find(|d| {
                            d.account == account
                                && d.code_sha256.as_deref() == Some(hash.as_str())
                                && d.code_expires.is_some_and(|e| e > now)
                        })
                        .map(|d| d.id.clone())
                        .ok_or(DeviceError::CodeNotFound)?
                }
                (None, Some(target)) => target.trim().to_owned(),
                (None, None) => {
                    return Err(DeviceError::Invalid("give a code or a device id".into()))
                }
            };
            if id == approving_id {
                return Err(DeviceError::Forbidden(
                    "a device cannot approve itself".into(),
                ));
            }
            let device = state
                .devices
                .get_mut(&id)
                .filter(|d| d.account == account)
                .ok_or(DeviceError::NotFound)?;
            match device.state(now) {
                DeviceState::Revoked => {
                    return Err(DeviceError::Forbidden("the device was revoked".into()))
                }
                DeviceState::Pending | DeviceState::Expired | DeviceState::Enrolled => {}
            }
            device.enrolled_at = Some(now);
            device.enrolled_until = Some(now + ttl);
            device.enrolled_by = Some(approving_id.clone());
            device.strong = approving_strong;
            device.code_sha256 = None;
            device.code_expires = None;
            let approved = device.clone();
            Self::push_audit(
                state,
                account,
                AuditEvent::new("device_enrolled")
                    .device(Some(&approved.id))
                    .subject(approving_id),
                &audit_key,
            );
            let superseded = Self::supersede(
                state,
                account,
                &approved.id,
                approved.machine.as_deref(),
                now,
                &audit_key,
            );
            Ok((approved, superseded))
        })?;
        self.end_sessions(&superseded);
        Ok(approved)
    }

    /// Opens a device session: `sig` over [`session_message`] with the
    /// device key. Returns the session token and its expiry.
    pub fn open_session(
        &self,
        account: &str,
        device: &str,
        ts: u64,
        sig: &str,
    ) -> Result<(String, u64), DeviceError> {
        let now = now_secs();
        let record = self
            .get(device)
            .filter(|d| d.account == account)
            .ok_or(DeviceError::NotFound)?;
        let raw = decode_public_key(&record.public_key).map_err(DeviceError::Invalid)?;
        self.check_proof(&raw, &session_message(device, ts), ts, sig)?;
        let until = match record.state(now) {
            DeviceState::Enrolled => record.enrolled_until.unwrap_or(now),
            DeviceState::Pending => {
                return Err(DeviceError::Forbidden(
                    "this device is waiting for approval from an enrolled device".into(),
                ))
            }
            DeviceState::Expired => {
                return Err(DeviceError::Forbidden(
                    "this device needs re-verification: approve it from an enrolled device".into(),
                ))
            }
            DeviceState::Revoked => {
                return Err(DeviceError::Forbidden("this device was revoked".into()))
            }
        };
        let expires = (now + self.policy.session_ttl_secs).min(until);
        let token = new_session_token();
        {
            let mut sessions = self.sessions.lock().expect("sessions");
            sessions.retain(|_, s| s.expires > now);
            sessions.insert(
                sha256_hex(&token),
                Session {
                    account: account.to_owned(),
                    device: device.to_owned(),
                    expires,
                },
            );
        }
        let audit_key = self.audit_key;
        let _ = self.mutate(|state| {
            if let Some(d) = state.devices.get_mut(device) {
                d.last_seen = Some(now);
            }
            Self::push_audit(
                state,
                account,
                AuditEvent::new("device_session").device(Some(device)),
                &audit_key,
            );
            Ok(())
        });
        Ok((token, expires))
    }

    /// The enrolled device behind session `token` for `account`, if the
    /// session is live and the device still enrolled.
    pub fn session_device(&self, account: &str, token: &str) -> Option<String> {
        let now = now_secs();
        let session = self
            .sessions
            .lock()
            .expect("sessions")
            .get(&sha256_hex(token))
            .cloned()?;
        if session.account != account || session.expires <= now {
            return None;
        }
        self.get(&session.device)
            .filter(|d| d.state(now) == DeviceState::Enrolled)
            .map(|d| d.id)
    }

    /// Renames `id`.
    pub fn rename(
        &self,
        account: &str,
        actor: &str,
        id: &str,
        name: &str,
    ) -> Result<DeviceRecord, DeviceError> {
        let name = clean_name(name);
        if name.is_empty() {
            return Err(DeviceError::Invalid("empty name".into()));
        }
        let audit_key = self.audit_key;
        self.mutate(|state| {
            let device = state
                .devices
                .get_mut(id)
                .filter(|d| d.account == account)
                .ok_or(DeviceError::NotFound)?;
            device.name = name.clone();
            let renamed = device.clone();
            Self::push_audit(
                state,
                account,
                AuditEvent::new("device_renamed")
                    .device(Some(actor))
                    .subject(id)
                    .detail(name),
                &audit_key,
            );
            Ok(renamed)
        })
    }

    /// Revokes `id` and ends its sessions.
    pub fn revoke(
        &self,
        account: &str,
        actor: &str,
        id: &str,
    ) -> Result<DeviceRecord, DeviceError> {
        let now = now_secs();
        let audit_key = self.audit_key;
        let revoked = self.mutate(|state| {
            let device = state
                .devices
                .get_mut(id)
                .filter(|d| d.account == account)
                .ok_or(DeviceError::NotFound)?;
            if device.revoked_at.is_none() {
                device.revoked_at = Some(now);
            }
            device.code_sha256 = None;
            device.code_expires = None;
            let revoked = device.clone();
            Self::push_audit(
                state,
                account,
                AuditEvent::new("device_revoked")
                    .device(Some(actor))
                    .subject(id),
                &audit_key,
            );
            Ok(revoked)
        })?;
        self.sessions
            .lock()
            .expect("sessions")
            .retain(|_, s| s.device != id);
        Ok(revoked)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ring::signature::KeyPair as _;

    /// A device key for tests.
    pub(crate) struct TestKey {
        pair: ring::signature::EcdsaKeyPair,
    }

    impl TestKey {
        pub(crate) fn new() -> Self {
            let rng = ring::rand::SystemRandom::new();
            let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
            let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
            Self {
                pair: ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap(),
            }
        }

        pub(crate) fn public(&self) -> String {
            b64().encode(self.pair.public_key().as_ref())
        }

        pub(crate) fn id(&self) -> String {
            device_id(self.pair.public_key().as_ref())
        }

        pub(crate) fn sign(&self, message: &str) -> String {
            let rng = ring::rand::SystemRandom::new();
            b64().encode(self.pair.sign(&rng, message.as_bytes()).unwrap().as_ref())
        }
    }

    /// Registers `key` with an unverified email (the rules before a
    /// verified email let a fresh sign-in enroll further devices).
    fn register(
        store: &DeviceStore,
        account: &str,
        key: &TestKey,
        bootstrap: bool,
        auth_time: Option<u64>,
    ) -> Result<Registered, DeviceError> {
        register_on(
            store, account, key, bootstrap, auth_time, false, None, false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn register_on(
        store: &DeviceStore,
        account: &str,
        key: &TestKey,
        bootstrap: bool,
        auth_time: Option<u64>,
        email_verified: bool,
        machine_id: Option<&str>,
        mfa: bool,
    ) -> Result<Registered, DeviceError> {
        let ts = now_secs();
        let public = key.public();
        let sig = key.sign(&register_message(&key.id(), ts));
        store.register(&Registration {
            account,
            user: account,
            public_key: &public,
            name: "laptop",
            platform: "macos",
            machine_id,
            ts,
            sig: &sig,
            bootstrap,
            auth_time,
            email_verified,
            mfa,
        })
    }

    fn session(store: &DeviceStore, account: &str, key: &TestKey) -> Result<String, DeviceError> {
        let ts = now_secs();
        store
            .open_session(
                account,
                &key.id(),
                ts,
                &key.sign(&session_message(&key.id(), ts)),
            )
            .map(|(t, _)| t)
    }

    fn enforcing() -> DevicePolicy {
        DevicePolicy {
            grace_secs: 0,
            ..DevicePolicy::default()
        }
    }

    /// A policy with a grace period running (the safe default is ended; a
    /// few tests exercise the rollout exception explicitly).
    fn with_grace() -> DevicePolicy {
        DevicePolicy {
            grace_secs: 14 * 86_400,
            ..DevicePolicy::default()
        }
    }

    #[test]
    fn first_device_bootstraps_only_with_a_fresh_sign_in_after_the_grace_period() {
        let store = DeviceStore::in_memory(enforcing());
        let first = TestKey::new();
        // A stale sign-in (a long-lived session) is not enough.
        let stale = register(&store, "acct", &first, true, Some(now_secs() - 3600)).unwrap();
        assert_eq!(stale.device.state(now_secs()), DeviceState::Pending);
        assert!(stale.code.is_some());
        assert!(session(&store, "acct", &first).is_err());
        // No auth_time claim at all: not enough either.
        let first_again = register(&store, "acct", &first, true, None).unwrap();
        assert!(first_again.code.is_some());
        let fresh = register(&store, "acct", &first, true, Some(now_secs() - 5)).unwrap();
        assert_eq!(fresh.device.state(now_secs()), DeviceState::Enrolled);
        assert_eq!(
            fresh.device.enrolled_by.as_deref(),
            Some("bootstrap:fresh-sign-in")
        );
        let token = session(&store, "acct", &first).unwrap();
        assert_eq!(
            store.session_device("acct", &token).as_deref(),
            Some(first.id().as_str())
        );
        // The session is bound to its account.
        assert!(store.session_device("other", &token).is_none());

        // Without a verified email, a second device cannot bootstrap, even
        // with a fresh sign-in.
        let second = TestKey::new();
        let pending = register(&store, "acct", &second, true, Some(now_secs())).unwrap();
        assert_eq!(pending.device.state(now_secs()), DeviceState::Pending);
        let code = pending.code.unwrap();
        // ...until an enrolled device confirms its code.
        assert!(matches!(
            store.approve("acct", &second.id(), Some(&code), None),
            Err(DeviceError::Forbidden(_))
        ));
        assert!(matches!(
            store.approve("other", &first.id(), Some(&code), None),
            Err(DeviceError::Forbidden(_))
        ));
        let approved = store
            .approve(
                "acct",
                &first.id(),
                Some(&code.to_lowercase().replace('-', "")),
                None,
            )
            .unwrap();
        assert_eq!(approved.enrolled_by.as_deref(), Some(first.id().as_str()));
        assert!(session(&store, "acct", &second).is_ok());
        // The code is single use.
        assert!(store
            .approve("acct", &first.id(), Some(&code), None)
            .is_err());

        let kinds: Vec<_> = store
            .audit_log("acct", 100)
            .into_iter()
            .map(|e| e.kind.clone())
            .collect();
        assert!(kinds.contains(&"device_registered".to_string()));
        assert!(kinds.contains(&"device_enrolled".to_string()));
        assert!(kinds.contains(&"device_session".to_string()));
        assert!(store.audit_log("other", 100).is_empty());
    }

    #[test]
    fn grace_period_lets_the_first_device_bootstrap_without_a_fresh_sign_in() {
        let store = DeviceStore::in_memory(with_grace());
        assert!(store.in_grace(now_secs()));
        let key = TestKey::new();
        let r = register(&store, "acct", &key, true, None).unwrap();
        assert_eq!(r.device.enrolled_by.as_deref(), Some("bootstrap:grace"));
        // The platform the device reported is kept (cleaned).
        assert_eq!(r.device.platform, "macos");
        assert_eq!(clean_platform(" Windows "), "windows");
        assert_eq!(clean_platform("mac os; drop"), "");
        // Only the first one.
        let other = TestKey::new();
        assert!(register(&store, "acct", &other, true, None)
            .unwrap()
            .code
            .is_some());
    }

    #[test]
    fn a_fresh_sign_in_recovers_from_devices_enrolled_on_the_grace_periods_word() {
        // During the grace period someone with a stolen session enrolls first.
        let store = DeviceStore::in_memory(with_grace());
        let thief = TestKey::new();
        let r = register(&store, "acct", &thief, true, None).unwrap();
        assert_eq!(r.device.enrolled_by.as_deref(), Some("bootstrap:grace"));
        assert!(!r.device.strong);
        // Devices it approves are not strong either.
        let accomplice = TestKey::new();
        let code = register(&store, "acct", &accomplice, false, None)
            .unwrap()
            .code
            .unwrap();
        assert!(
            !store
                .approve("acct", &thief.id(), Some(&code), None)
                .unwrap()
                .strong
        );
        // The owner signs in freshly and still bootstraps, then revokes them.
        let owner = TestKey::new();
        let r = register(&store, "acct", &owner, true, Some(now_secs())).unwrap();
        assert_eq!(r.device.state(now_secs()), DeviceState::Enrolled);
        assert!(r.device.strong);
        store.revoke("acct", &owner.id(), &thief.id()).unwrap();
        store.revoke("acct", &owner.id(), &accomplice.id()).unwrap();
        // Now a strong device exists: a fresh sign-in alone no longer
        // enrolls another one.
        let next = TestKey::new();
        assert!(register(&store, "acct", &next, true, Some(now_secs()))
            .unwrap()
            .code
            .is_some());
    }

    #[test]
    fn a_malleated_signature_is_still_a_replay() {
        let store = DeviceStore::in_memory(enforcing());
        let key = TestKey::new();
        register(&store, "acct", &key, true, Some(now_secs())).unwrap();
        let ts = now_secs();
        let sig = key.sign(&session_message(&key.id(), ts));
        store.open_session("acct", &key.id(), ts, &sig).unwrap();
        // (r, n - s) verifies too.
        let raw = b64().decode(&sig).unwrap();
        const N: [u8; 32] = [
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2,
            0xfc, 0x63, 0x25, 0x51,
        ];
        let s = &raw[32..];
        let mut twin = [0u8; 32];
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let v = N[i] as i16 - s[i] as i16 - borrow;
            borrow = i16::from(v < 0);
            twin[i] = v.rem_euclid(256) as u8;
        }
        let mut malleated = raw[..32].to_vec();
        malleated.extend_from_slice(&twin);
        let malleated = b64().encode(malleated);
        assert!(verify(
            &decode_public_key(&key.public()).unwrap(),
            &session_message(&key.id(), ts),
            &malleated
        ));
        assert!(matches!(
            store.open_session("acct", &key.id(), ts, &malleated),
            Err(DeviceError::Proof(_))
        ));
    }

    #[test]
    fn proofs_are_bound_fresh_and_single_use() {
        let store = DeviceStore::in_memory(enforcing());
        let key = TestKey::new();
        register(&store, "acct", &key, true, Some(now_secs())).unwrap();
        let ts = now_secs();
        let sig = key.sign(&session_message(&key.id(), ts));
        assert!(store.open_session("acct", &key.id(), ts, &sig).is_ok());
        // Replayed.
        assert!(matches!(
            store.open_session("acct", &key.id(), ts, &sig),
            Err(DeviceError::Proof(_))
        ));
        // Stale.
        let old = ts - 3600;
        let sig = key.sign(&session_message(&key.id(), old));
        assert!(store.open_session("acct", &key.id(), old, &sig).is_err());
        // Another key's signature.
        let intruder = TestKey::new();
        let sig = intruder.sign(&session_message(&key.id(), ts));
        assert!(store.open_session("acct", &key.id(), ts, &sig).is_err());
        // A key registered to one account cannot join another.
        let ts = now_secs();
        let sig = key.sign(&register_message(&key.id(), ts));
        let public = key.public();
        assert!(matches!(
            store.register(&Registration {
                account: "other",
                user: "other",
                public_key: &public,
                name: "x",
                ts,
                sig: &sig,
                bootstrap: true,
                auth_time: Some(ts),
                email_verified: true,
                ..Registration::default()
            }),
            Err(DeviceError::Conflict(_))
        ));
    }

    #[test]
    fn expiry_needs_one_approval_and_revocation_ends_sessions() {
        let store = DeviceStore::in_memory(DevicePolicy {
            grace_secs: 0,
            ttl_secs: 1,
            ..DevicePolicy::default()
        });
        let a = TestKey::new();
        let b = TestKey::new();
        register(&store, "acct", &a, true, Some(now_secs())).unwrap();
        let code = register(&store, "acct", &b, false, None)
            .unwrap()
            .code
            .unwrap();
        store.approve("acct", &a.id(), Some(&code), None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2100));
        assert_eq!(
            store.get(&a.id()).unwrap().state(now_secs()),
            DeviceState::Expired
        );
        assert!(session(&store, "acct", &a).is_err());
        // With no enrolled device left, a fresh sign-in re-verifies.
        let again = register(&store, "acct", &a, true, Some(now_secs())).unwrap();
        assert_eq!(again.device.state(now_secs()), DeviceState::Enrolled);
        // b (expired) needs an approval from a; by id this time.
        store.approve("acct", &a.id(), None, Some(&b.id())).unwrap();
        let token = session(&store, "acct", &b).unwrap();
        store.revoke("acct", &a.id(), &b.id()).unwrap();
        assert!(store.session_device("acct", &token).is_none());
        assert!(session(&store, "acct", &b).is_err());
        assert!(matches!(
            register(&store, "acct", &b, false, None),
            Err(DeviceError::Forbidden(_))
        ));
    }

    #[test]
    fn state_persists_with_the_grace_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let store = DeviceStore::open(path.clone(), enforcing()).unwrap();
        let key = TestKey::new();
        register(&store, "acct", &key, true, Some(now_secs())).unwrap();
        let started = store.enforce_after();
        drop(store);
        let reopened = DeviceStore::open(path.clone(), enforcing()).unwrap();
        assert_eq!(reopened.enforce_after(), started);
        assert_eq!(
            reopened.get(&key.id()).unwrap().state(now_secs()),
            DeviceState::Enrolled
        );
        // Sessions are not persisted: a restart asks devices for a new one.
        assert!(!std::fs::read_to_string(&path).unwrap().contains("cds_"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn the_audit_key_persists_0600_next_to_the_devices_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let key_path = dir.path().join("devices.json.audit-key");
        let store = DeviceStore::open(path.clone(), enforcing()).unwrap();
        store.audit("acct", AuditEvent::new("device_registered"));
        assert!(key_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(store);
        // A restart keeps the same key: the chain still verifies (a new
        // random key on every start would break every mac written before).
        let reopened = DeviceStore::open(path, enforcing()).unwrap();
        assert!(reopened.verify_audit_chain("acct").is_ok());
        reopened.audit("acct", AuditEvent::new("device_enrolled"));
        assert!(reopened.verify_audit_chain("acct").is_ok());
        assert_eq!(reopened.audit_log("acct", 10).len(), 2);
    }

    /// Reproduces the relay.cua.ai outage after the S9 hardening deploy
    /// (cua-sdk ae91099e0): the production state volume's `devices.json`
    /// predates the hash-chained audit log, so its `audit` entries are
    /// plain [`AuditEvent`]s with no `seq`/`prev`/`hash`/`mac`. Before this
    /// fix, deserializing them into `Vec<AuditEntry>` failed with "missing
    /// field `seq`", `DeviceStore::open` returned `Err`, `Relay::try_new`
    /// propagated it, and `main` exited non-zero without ever binding the
    /// listener, so `/healthz` never answered, the pod never passed its
    /// readiness probe under the production `securityContext` (non-root,
    /// read-only rootfs), and Traefik had no ready backend ("503 no
    /// available server"), exactly as it did in production. `DeviceStore`
    /// must instead migrate the file in place and start normally.
    #[test]
    fn opening_a_pre_chain_devices_file_migrates_its_audit_log_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        // Byte-for-byte the shape the pre-S9 relay (cua-sdk 4f62b9c91)
        // persists: no `confirmed`-style chain fields on an audit entry.
        let legacy = serde_json::json!({
            "grace_started_at": 1_790_000_000u64,
            "devices": {},
            "audit": {
                "acct": [
                    {
                        "ts": 1_790_000_001u64,
                        "kind": "unenrolled_access",
                        "subject": "ada@example.com"
                    },
                    {
                        "ts": 1_790_000_002u64,
                        "kind": "machine_registered",
                        "machine": "machine-0001",
                        "detail": "test-machine"
                    }
                ]
            }
        });
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        // The bug: this used to be `Err` ("missing field `seq`"), which is
        // exactly what made `Relay::try_new` (and so the whole process)
        // fail at startup on the production volume.
        let store = DeviceStore::open(path.clone(), enforcing())
            .expect("a pre-chain devices file must load, not fail to start");

        // The migrated chain verifies and keeps the original events, in
        // order, now sequenced from 1.
        assert!(store.verify_audit_chain("acct").is_ok());
        let log = store.audit_log("acct", 10);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].seq, 1);
        assert_eq!(log[0].kind, "unenrolled_access");
        assert_eq!(log[1].seq, 2);
        assert_eq!(log[1].kind, "machine_registered");
        assert_eq!(log[1].machine.as_deref(), Some("machine-0001"));

        // The migration is persisted, so a second restart (the pod
        // restarting again) does not re-migrate or duplicate entries, and
        // the chain still verifies under the same (now persisted) key.
        drop(store);
        let reopened = DeviceStore::open(path, enforcing()).unwrap();
        assert!(reopened.verify_audit_chain("acct").is_ok());
        assert_eq!(reopened.audit_log("acct", 10).len(), 2);
    }

    #[test]
    fn audit_is_bounded_and_throttled() {
        let store = DeviceStore::in_memory(DevicePolicy::default());
        for _ in 0..(AUDIT_LIMIT + 10) {
            store.audit("acct", AuditEvent::new("machine_access"));
        }
        assert_eq!(store.audit_log("acct", usize::MAX).len(), AUDIT_LIMIT);
        let other = DeviceStore::in_memory(with_grace());
        other.audit_throttled("k", "acct", AuditEvent::new("machine_access"));
        other.audit_throttled("k", "acct", AuditEvent::new("machine_access"));
        other.audit_throttled("k2", "acct", AuditEvent::new("machine_access"));
        assert_eq!(other.audit_log("acct", 100).len(), 2);
    }

    #[test]
    fn the_audit_log_is_a_verifiable_hash_chain() {
        let store = DeviceStore::in_memory(DevicePolicy::default());
        store.audit("acct", AuditEvent::new("device_registered"));
        store.audit("acct", AuditEvent::new("device_enrolled"));
        store.audit("acct", AuditEvent::new("device_session"));
        assert!(store.verify_audit_chain("acct").is_ok());
        // Untouched, or a different, empty account: also fine.
        assert!(store.verify_audit_chain("nobody").is_ok());

        let log = store.audit_log("acct", 100);
        assert_eq!(log[0].prev, AUDIT_GENESIS);
        assert_eq!(log[1].prev, log[0].hash);
        assert_eq!(log[2].prev, log[1].hash);
        assert_ne!(log[0].hash, log[1].hash);

        // Editing the middle entry's content breaks the chain from there:
        // its own hash/mac no longer match, and the entry after it no
        // longer chains from a hash that is still valid.
        let mut tampered = store.state.lock().unwrap();
        tampered.audit.get_mut("acct").unwrap()[1].event.kind = "device_revoked".into();
        drop(tampered);
        let err = store.verify_audit_chain("acct").unwrap_err();
        assert_eq!(err.seq, 2);
        assert_eq!(err.reason, "mac");

        // The wrong key never verifies any of it, even untampered.
        let store2 = DeviceStore::in_memory(DevicePolicy::default());
        store2.audit("acct", AuditEvent::new("device_registered"));
        let wrong_key = [7u8; 32];
        let log = store2.audit_log("acct", 100);
        assert!(super::verify_audit_chain(&log, &wrong_key).is_err());
        assert!(store2.verify_audit_chain("acct").is_ok());
    }

    #[test]
    fn an_edited_log_file_does_not_verify_without_the_audit_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let store = DeviceStore::open(path.clone(), DevicePolicy::default()).unwrap();
        store.audit("acct", AuditEvent::new("device_registered"));
        store.audit("acct", AuditEvent::new("device_enrolled"));
        drop(store);

        // Someone with only the state file (a backup, a disk snapshot) edits
        // an event and recomputes the plain hash chain (no secret needed for
        // that much), but cannot reproduce a valid mac without the audit
        // key, which lives in a separate file.
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let entries = value["audit"]["acct"].as_array_mut().unwrap();
        entries[0]["kind"] = "device_revoked".into();
        let prev = entries[0]["hash"].as_str().unwrap().to_owned();
        let seq = entries[0]["seq"].as_u64().unwrap();
        let event: AuditEvent = serde_json::from_value(entries[0].clone()).unwrap();
        entries[0]["hash"] = chain_hash(AUDIT_GENESIS, seq, &event).into();
        entries[1]["prev"] = prev.clone().into();
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let reopened = DeviceStore::open(path, DevicePolicy::default()).unwrap();
        // The mac on the edited entry no longer matches (it was never
        // recomputed, since that needs the audit key), so the chain still
        // does not verify.
        assert!(reopened.verify_audit_chain("acct").is_err());
    }

    #[test]
    fn a_fresh_sign_in_enrolls_another_device_but_a_stale_one_does_not() {
        let store = DeviceStore::in_memory(enforcing());
        let first = TestKey::new();
        register_on(
            &store,
            "acct",
            &first,
            true,
            Some(now_secs()),
            true,
            None,
            true,
        )
        .unwrap();
        // A long-lived session (signed in an hour ago) does not enroll a
        // second device: it waits for an approval.
        let second = TestKey::new();
        let stale = register_on(
            &store,
            "acct",
            &second,
            true,
            Some(now_secs() - 3600),
            true,
            None,
            true,
        )
        .unwrap();
        assert_eq!(stale.device.state(now_secs()), DeviceState::Pending);
        assert!(stale.code.is_some());
        // Nor does a sign-in without an auth_time claim, or one that did not
        // ask to enroll.
        assert!(
            register_on(&store, "acct", &second, true, None, true, None, true)
                .unwrap()
                .code
                .is_some()
        );
        assert!(register_on(
            &store,
            "acct",
            &second,
            false,
            Some(now_secs()),
            true,
            None,
            true
        )
        .unwrap()
        .code
        .is_some());
        // A fresh sign-in (verified email) enrolls it at once.
        let fresh = register_on(
            &store,
            "acct",
            &second,
            true,
            Some(now_secs() - 5),
            true,
            None,
            true,
        )
        .unwrap();
        assert_eq!(fresh.device.state(now_secs()), DeviceState::Enrolled);
        assert_eq!(fresh.device.enrolled_by.as_deref(), Some(BY_FRESH_SIGN_IN));
        assert!(fresh.device.strong);
        assert!(fresh.code.is_none());
        assert!(session(&store, "acct", &second).is_ok());
        // Both devices stay: different machines (none reported).
        assert!(store.get(&first.id()).unwrap().revoked_at.is_none());
        // Audited as enrolled by a fresh sign-in.
        let log = store.audit_log("acct", 100);
        let enrolled = log.iter().rfind(|e| e.kind == "device_enrolled").unwrap();
        assert_eq!(enrolled.device.as_deref(), Some(second.id().as_str()));
        assert_eq!(enrolled.subject.as_deref(), Some(BY_FRESH_SIGN_IN));
        assert_eq!(
            enrolled.detail.as_deref(),
            Some("enrolled by fresh sign-in")
        );
    }

    #[test]
    fn a_brand_new_device_needs_mfa_or_approval_once_the_account_has_a_strong_device() {
        let store = DeviceStore::in_memory(enforcing());
        let first = TestKey::new();
        register_on(
            &store,
            "acct",
            &first,
            true,
            Some(now_secs()),
            true,
            None,
            true,
        )
        .unwrap();
        assert!(store.get(&first.id()).unwrap().strong);
        // A second, unrelated device (no machine id in common): a fresh
        // sign-in with a verified email but NO proof of a second factor is
        // no longer enough on its own (S4) -- it waits for approval.
        let second = TestKey::new();
        let pending = register_on(
            &store,
            "acct",
            &second,
            true,
            Some(now_secs()),
            true,
            None,
            false,
        )
        .unwrap();
        assert_eq!(pending.device.state(now_secs()), DeviceState::Pending);
        assert!(pending.code.is_some());
        // The same fresh sign-in, this time proving MFA, enrolls it at once.
        let enrolled = register_on(
            &store,
            "acct",
            &second,
            true,
            Some(now_secs()),
            true,
            None,
            true,
        )
        .unwrap();
        assert_eq!(enrolled.device.state(now_secs()), DeviceState::Enrolled);
        assert_eq!(
            enrolled.device.enrolled_by.as_deref(),
            Some(BY_FRESH_SIGN_IN)
        );
        assert!(enrolled.device.strong);
        // Or, without MFA, an approval from the first (enrolled) device
        // still works, exactly like before.
        let third = TestKey::new();
        let code = register_on(&store, "acct", &third, false, None, true, None, false)
            .unwrap()
            .code
            .unwrap();
        let approved = store
            .approve("acct", &first.id(), Some(&code), None)
            .unwrap();
        assert_eq!(approved.state(now_secs()), DeviceState::Enrolled);
    }

    #[test]
    fn a_new_key_of_the_same_machine_replaces_the_old_record() {
        let store = DeviceStore::in_memory(enforcing());
        let old = TestKey::new();
        let other_machine = TestKey::new();
        register_on(
            &store,
            "acct",
            &old,
            true,
            Some(now_secs()),
            true,
            Some("hw-1"),
            true,
        )
        .unwrap();
        register_on(
            &store,
            "acct",
            &other_machine,
            true,
            Some(now_secs()),
            true,
            Some("hw-2"),
            true,
        )
        .unwrap();
        let old_session = session(&store, "acct", &old).unwrap();

        // Another build on the same machine made a new key: registered
        // without a fresh sign-in it waits, and replaces nothing yet.
        let new = TestKey::new();
        let pending = register_on(
            &store,
            "acct",
            &new,
            true,
            Some(now_secs() - 3600),
            true,
            Some("hw-1"),
            true,
        )
        .unwrap();
        assert!(pending.code.is_some());
        assert!(pending.superseded.is_empty());
        assert_eq!(
            store.get(&old.id()).unwrap().state(now_secs()),
            DeviceState::Enrolled
        );

        // A fresh sign-in enrolls it and it replaces the old key.
        let r = register_on(
            &store,
            "acct",
            &new,
            true,
            Some(now_secs()),
            true,
            Some(" hw-1 "),
            true,
        )
        .unwrap();
        assert_eq!(r.device.state(now_secs()), DeviceState::Enrolled);
        assert_eq!(r.superseded, vec![old.id()]);
        let replaced = store.get(&old.id()).unwrap();
        assert_eq!(replaced.state(now_secs()), DeviceState::Revoked);
        assert_eq!(replaced.superseded_by.as_deref(), Some(new.id().as_str()));
        // Its session ends; the other machine's device is untouched.
        assert!(store.session_device("acct", &old_session).is_none());
        assert_eq!(
            store.get(&other_machine.id()).unwrap().state(now_secs()),
            DeviceState::Enrolled
        );
        // The machine lists once.
        let ids: Vec<_> = store.list("acct").into_iter().map(|d| d.id).collect();
        assert!(ids.contains(&new.id()) && ids.contains(&other_machine.id()));
        assert!(!ids.contains(&old.id()));
        // Audited.
        let rekeyed = store
            .audit_log("acct", 100)
            .into_iter()
            .find(|e| e.kind == "device_rekeyed")
            .unwrap();
        assert_eq!(rekeyed.device.as_deref(), Some(new.id().as_str()));
        assert_eq!(rekeyed.subject.as_deref(), Some(old.id().as_str()));
        // The replaced key cannot come back.
        let err = register_on(
            &store,
            "acct",
            &old,
            true,
            Some(now_secs()),
            true,
            Some("hw-1"),
            true,
        )
        .unwrap_err();
        assert!(
            matches!(&err, DeviceError::Forbidden(m) if m.contains("replaced by")),
            "{err:?}"
        );

        // An approval replaces too.
        let newer = TestKey::new();
        let code = register_on(
            &store,
            "acct",
            &newer,
            false,
            None,
            true,
            Some("hw-2"),
            true,
        )
        .unwrap()
        .code
        .unwrap();
        store.approve("acct", &new.id(), Some(&code), None).unwrap();
        assert_eq!(
            store
                .get(&other_machine.id())
                .unwrap()
                .superseded_by
                .as_deref(),
            Some(newer.id().as_str())
        );

        // The same machine id on another account is a different machine.
        let theirs = TestKey::new();
        let r = register_on(
            &store,
            "bob",
            &theirs,
            true,
            Some(now_secs()),
            true,
            Some("hw-1"),
            true,
        )
        .unwrap();
        assert!(r.superseded.is_empty());
        assert_ne!(
            store.get(&theirs.id()).unwrap().machine,
            store.get(&new.id()).unwrap().machine
        );
        assert!(machine_key("acct", "").is_none());
        assert!(machine_key("acct", &"x".repeat(200)).is_none());
    }

    #[test]
    fn a_fresh_sign_in_re_verifies_and_renews_only_once_per_sign_in() {
        let store = DeviceStore::in_memory(DevicePolicy {
            grace_secs: 0,
            ttl_secs: 1,
            ..DevicePolicy::default()
        });
        let a = TestKey::new();
        let b = TestKey::new();
        register_on(&store, "acct", &a, true, Some(now_secs()), true, None, true).unwrap();
        register_on(&store, "acct", &b, true, Some(now_secs()), true, None, true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2100));
        // Both expired: re-verification is due (one approval, or a sign-in).
        assert_eq!(
            store.get(&b.id()).unwrap().state(now_secs()),
            DeviceState::Expired
        );
        assert!(session(&store, "acct", &b).is_err());
        // A new sign-in re-verifies it.
        let signed_in = now_secs();
        let r = register_on(&store, "acct", &b, true, Some(signed_in), true, None, true).unwrap();
        assert_eq!(r.device.state(now_secs()), DeviceState::Enrolled);
        let until = r.device.enrolled_until;
        // Registering again with the same sign-in keeps the enrollment.
        let again =
            register_on(&store, "acct", &b, true, Some(signed_in), true, None, true).unwrap();
        assert_eq!(again.device.enrolled_until, until);
        assert_eq!(
            store
                .audit_log("acct", 100)
                .iter()
                .filter(|e| e.kind == "device_enrolled" && e.device.as_deref() == Some(&b.id()))
                .count(),
            2
        );
    }

    #[test]
    fn approve_takes_a_device_id_for_a_code_and_explains_an_unknown_code() {
        let store = DeviceStore::in_memory(enforcing());
        let a = TestKey::new();
        let b = TestKey::new();
        register(&store, "acct", &a, true, Some(now_secs())).unwrap();
        register(&store, "acct", &b, false, None).unwrap();
        let err = store
            .approve("acct", &a.id(), Some("ZZZZ-ZZZZ"), None)
            .unwrap_err();
        assert_eq!(err, DeviceError::CodeNotFound);
        assert!(err.to_string().contains("approve by id"), "{err}");
        // `cua devices approve dev_…` sends the id as the code.
        let approved = store
            .approve("acct", &a.id(), Some(&format!(" {} ", b.id())), None)
            .unwrap();
        assert_eq!(approved.id, b.id());
        assert_eq!(approved.state(now_secs()), DeviceState::Enrolled);
        assert_eq!(
            store.approve("acct", &a.id(), Some("dev_000000000000000000000000"), None),
            Err(DeviceError::NotFound)
        );
    }
}
