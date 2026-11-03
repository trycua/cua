// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pairing codes: "add a machine" by link or QR code.
//!
//! An enrolled device of an account mints a short-lived, single-use code
//! (`POST /v1/pairing-codes`). The machine being added registers with it
//! instead of an account token (`POST /v1/machines` with
//! `authorization: Bearer cpc_<code>`), so it never needs a browser
//! sign-in: it joins the account (and, when the code names a team the
//! minting account manages, that team's machines) and the code is spent. Codes expire (30 minutes by
//! default) and their owner can revoke them before use.
//!
//! - `POST   /v1/pairing-codes`       (account, enrolled device) -> code (shown once)
//! - `GET    /v1/pairing-codes`       (account, enrolled device) -> {"codes": [...]} (no codes)
//! - `DELETE /v1/pairing-codes/{id}`  (account, enrolled device) -> 204
//!
//! Only the SHA-256 of a code is kept, like machine tokens. A code is 12
//! Crockford base32 characters (60 bits), shown as `XXXX-XXXX-XXXX`; case,
//! dashes and spaces do not matter, and `I`/`L` read as `1`, `O` as `0`.

// Handlers short-circuit with ready-made responses.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::assertion::{now_secs, remove_stale_temps, write_private_with};
use crate::devices::AuditEvent;
use crate::oidc::Identity;
use crate::server::Relay;
use crate::storage::{store, Storage};

/// Prefix of a pairing code presented as a bearer token.
pub const PAIRING_TOKEN_PREFIX: &str = "cpc_";
/// Characters of a code.
pub const CODE_LEN: usize = 12;
/// Default lifetime of a code.
pub const DEFAULT_TTL_SECS: u64 = 30 * 60;
/// Shortest lifetime a caller may ask for.
pub const MIN_TTL_SECS: u64 = 60;
/// Longest lifetime a caller may ask for.
pub const MAX_TTL_SECS: u64 = 24 * 60 * 60;
/// Unused, unexpired codes an account may hold at once.
pub const MAX_ACTIVE_PER_ACCOUNT: usize = 10;
/// Spent, revoked and expired codes stay listed this long.
const KEEP_SECS: u64 = 24 * 60 * 60;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A code in canonical form (12 upper-case Crockford characters), from
/// what a person typed or a link carried (with or without the `cpc_`
/// prefix, dashes or spaces, any case). `None` when it cannot be a code.
pub fn normalize(input: &str) -> Option<String> {
    let s = input.trim();
    let s = s.strip_prefix(PAIRING_TOKEN_PREFIX).unwrap_or(s);
    let mut out = String::with_capacity(CODE_LEN);
    for c in s.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' => continue,
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// `XXXX-XXXX-XXXX`.
pub fn display(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

fn new_code() -> String {
    use rand::RngCore as _;
    let mut bytes = [0u8; CODE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[(*b & 31) as usize] as char)
        .collect()
}

fn new_id() -> String {
    format!("pc_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

fn hash(code: &str) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, code.as_bytes()).as_ref())
}

/// One code as stored (its hash, never the code).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRecord {
    /// `pc_…`.
    pub id: String,
    /// SHA-256 (hex) of the canonical code.
    pub code_sha256: String,
    /// Account the machine joins.
    pub account: String,
    /// User who minted it.
    pub user: String,
    /// The owner's verified email, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The owner's display name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
    /// Name for the machine (else the machine's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Team (`team-…`) the machine is added to (the minting account was its
    /// owner or an admin at mint time, and must still be when it is used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// Minting device (`dev_…`), when an enrolled device minted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Minted with an enrolled device's session (or where enrollment is
    /// off): the machine it adds starts confirmed.
    #[serde(default)]
    pub proven: bool,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds.
    pub expires_at: u64,
    /// When a machine registered with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_at: Option<u64>,
    /// The machine it added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// When its owner revoked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
}

impl PairingRecord {
    /// `active`, `used`, `revoked` or `expired` at `now`.
    pub fn state(&self, now: u64) -> &'static str {
        if self.used_at.is_some() {
            "used"
        } else if self.revoked_at.is_some() {
            "revoked"
        } else if now >= self.expires_at {
            "expired"
        } else {
            "active"
        }
    }

    /// The identity the machine registers as.
    pub fn identity(&self) -> Identity {
        Identity {
            user: self.user.clone(),
            account: self.account.clone(),
            email: self.email.clone(),
            name: self.owner_name.clone(),
            auth_time: None,
            mfa: false,
            orgs: Vec::new(),
        }
    }

    fn view(&self, now: u64, code: Option<String>) -> PairingView {
        PairingView {
            id: self.id.clone(),
            code,
            state: self.state(now).into(),
            name: self.name.clone(),
            team: self.team.clone(),
            created_at: self.created_at,
            expires_at: self.expires_at,
            used_at: self.used_at,
            machine: self.machine.clone(),
        }
    }
}

/// A code as the API returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingView {
    /// `pc_…`.
    pub id: String,
    /// `XXXX-XXXX-XXXX`, only in the reply that minted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// `active`, `used`, `revoked` or `expired`.
    pub state: String,
    /// Name for the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds.
    pub expires_at: u64,
    /// When it was used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_at: Option<u64>,
    /// The machine it added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

/// Why a code was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingError {
    /// Malformed, unknown, used, revoked or expired: one answer for all, so
    /// a guess learns nothing.
    Invalid,
    /// 400.
    BadRequest(String),
    /// 404 (another account's id, or none).
    NotFound,
    /// 403.
    Forbidden(String),
    /// 429.
    TooMany,
    /// 500.
    Storage(String),
}

impl std::fmt::Display for PairingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => f.write_str(
                "this pairing code is not valid (used, expired, revoked or mistyped); make a new one in the Cua Spaces app",
            ),
            Self::BadRequest(m) | Self::Storage(m) | Self::Forbidden(m) => f.write_str(m),
            Self::NotFound => f.write_str("no such pairing code or team"),
            Self::TooMany => write!(
                f,
                "at most {MAX_ACTIVE_PER_ACCOUNT} unused pairing codes at once; revoke one or wait for it to expire"
            ),
        }
    }
}

impl PairingError {
    fn status(&self) -> StatusCode {
        match self {
            Self::Invalid => StatusCode::UNAUTHORIZED,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::TooMany => StatusCode::TOO_MANY_REQUESTS,
            Self::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// What minting a code needs.
pub struct Mint<'a> {
    /// The minting account.
    pub who: &'a Identity,
    /// Name for the machine.
    pub name: Option<String>,
    /// Team.
    pub team: Option<String>,
    /// Lifetime (clamped to [`MIN_TTL_SECS`]..=[`MAX_TTL_SECS`]).
    pub ttl_secs: Option<u64>,
    /// Minting device.
    pub device: Option<String>,
    /// See [`PairingRecord::proven`].
    pub proven: bool,
}

/// The codes, keyed by id. Persisted as JSON (atomic replace) when a file
/// is configured.
#[derive(Debug, Default)]
pub struct PairingStore {
    path: Option<PathBuf>,
    records: Mutex<BTreeMap<String, PairingRecord>>,
    /// Where write failures are counted and reported.
    storage: std::sync::Arc<Storage>,
}

fn clean_text(s: Option<String>, max: usize, what: &str) -> Result<Option<String>, PairingError> {
    let Some(s) = s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if s.chars().count() > max || s.chars().any(char::is_control) {
        return Err(PairingError::BadRequest(format!("invalid {what}")));
    }
    Ok(Some(s))
}

impl PairingStore {
    /// In memory only.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads (or starts) the store persisted at `path`.
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        Self::open_with(path, Default::default())
    }

    /// [`PairingStore::open`], reporting writes to `storage`.
    pub fn open_with(path: PathBuf, storage: std::sync::Arc<Storage>) -> std::io::Result<Self> {
        remove_stale_temps(&path)?;
        let records = match std::fs::read(&path) {
            Ok(raw) => {
                let list: Vec<PairingRecord> = serde_json::from_slice(&raw)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                list.into_iter().map(|r| (r.id.clone(), r)).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: Some(path),
            records: Mutex::new(records),
            storage,
        })
    }

    /// Whether the pairing file is writable: no write failed lately
    /// without a successful one since (true in memory).
    pub fn writes_ok(&self) -> bool {
        self.storage.store_ok(store::PAIRING)
    }

    /// Writes `next` (dropping records past [`KEEP_SECS`]: compact JSON
    /// streamed to a temp file that replaces the file) and swaps it in. On
    /// a failed write the records stay as they were and the caller gets the
    /// error.
    fn commit(
        &self,
        records: &mut BTreeMap<String, PairingRecord>,
        mut next: BTreeMap<String, PairingRecord>,
        now: u64,
    ) -> Result<(), PairingError> {
        next.retain(|_, r| r.expires_at.max(r.used_at.unwrap_or(0)) + KEEP_SECS > now);
        if let Some(path) = &self.path {
            let written = (|| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_private_with(path, |out| {
                    serde_json::to_writer(&mut *out, &next.values().collect::<Vec<_>>())
                        .map_err(std::io::Error::other)
                })
            })();
            self.storage.record(store::PAIRING, &written);
            written.map_err(|e| PairingError::Storage(e.to_string()))?;
        }
        *records = next;
        Ok(())
    }

    /// Mints a code; returns its record and the code (canonical form).
    pub fn mint(&self, m: Mint<'_>) -> Result<(PairingRecord, String), PairingError> {
        let name = clean_text(m.name, 64, "machine name")?;
        let team = clean_text(m.team, 128, "team")?;
        let ttl = m
            .ttl_secs
            .unwrap_or(DEFAULT_TTL_SECS)
            .clamp(MIN_TTL_SECS, MAX_TTL_SECS);
        let now = now_secs();
        let mut records = self.records.lock().expect("pairing");
        let active = records
            .values()
            .filter(|r| r.account == m.who.account && r.state(now) == "active")
            .count();
        if active >= MAX_ACTIVE_PER_ACCOUNT {
            return Err(PairingError::TooMany);
        }
        let code = new_code();
        let record = PairingRecord {
            id: new_id(),
            code_sha256: hash(&code),
            account: m.who.account.clone(),
            user: m.who.user.clone(),
            email: m.who.email.clone(),
            owner_name: m.who.name.clone(),
            name,
            team,
            created_by: m.device,
            proven: m.proven,
            created_at: now,
            expires_at: now + ttl,
            used_at: None,
            machine: None,
            revoked_at: None,
        };
        let mut next = records.clone();
        next.insert(record.id.clone(), record.clone());
        self.commit(&mut records, next, now)?;
        Ok((record, code))
    }

    /// The active record for a presented code (not spent).
    pub fn peek(&self, presented: &str) -> Result<PairingRecord, PairingError> {
        let code = normalize(presented).ok_or(PairingError::Invalid)?;
        let digest = hash(&code);
        let now = now_secs();
        let records = self.records.lock().expect("pairing");
        records
            .values()
            .find(|r| {
                subtle::ConstantTimeEq::ct_eq(r.code_sha256.as_bytes(), digest.as_bytes()).into()
            })
            .filter(|r| r.state(now) == "active")
            .cloned()
            .ok_or(PairingError::Invalid)
    }

    /// Spends code `id` on `machine`, if it is still active (single use:
    /// of two concurrent registrations only one gets it).
    pub fn spend(&self, id: &str, machine: &str) -> Result<PairingRecord, PairingError> {
        let now = now_secs();
        let mut records = self.records.lock().expect("pairing");
        let mut next = records.clone();
        let r = next.get_mut(id).ok_or(PairingError::Invalid)?;
        if r.state(now) != "active" {
            return Err(PairingError::Invalid);
        }
        r.used_at = Some(now);
        r.machine = Some(machine.to_owned());
        let spent = r.clone();
        self.commit(&mut records, next, now)?;
        Ok(spent)
    }

    /// Undoes [`PairingStore::spend`] when the registration then failed, so
    /// the person can try again with the same code.
    pub fn unspend(&self, id: &str) {
        let now = now_secs();
        let mut records = self.records.lock().expect("pairing");
        let mut next = records.clone();
        if let Some(r) = next.get_mut(id) {
            r.used_at = None;
            r.machine = None;
            // A failed write is counted (store `pairing`); the code then
            // stays spent.
            if let Err(e) = self.commit(&mut records, next, now) {
                tracing::warn!(error = %e, "pairing code not restored");
            }
        }
    }

    /// The account's codes, newest first.
    pub fn list(&self, account: &str) -> Vec<PairingRecord> {
        let records = self.records.lock().expect("pairing");
        let mut out: Vec<_> = records
            .values()
            .filter(|r| r.account == account)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        out
    }

    /// Emails the codes hold for `account`.
    pub fn emails_of(&self, account: &str) -> Vec<String> {
        let records = self.records.lock().expect("pairing");
        records
            .values()
            .filter(|r| r.account == account || r.user == account)
            .filter_map(|r| r.email.clone())
            .collect()
    }

    /// Account deletion: revokes and purges every code `account` minted or
    /// that adds machines to it, whatever its state. Returns how many were
    /// removed. The file is rewritten every call, so a retry after a failed
    /// write finishes the job.
    pub fn erase_account(&self, account: &str) -> Result<usize, PairingError> {
        let now = now_secs();
        let mut records = self.records.lock().expect("pairing");
        let mut next = records.clone();
        next.retain(|_, r| r.account != account && r.user != account);
        let removed = records.len() - next.len();
        self.commit(&mut records, next, now)?;
        Ok(removed)
    }

    /// Revokes the account's code `id` (a no-op for one already used,
    /// revoked or expired).
    pub fn revoke(&self, account: &str, id: &str) -> Result<PairingRecord, PairingError> {
        let now = now_secs();
        let mut records = self.records.lock().expect("pairing");
        let mut next = records.clone();
        let r = next
            .get_mut(id)
            .filter(|r| r.account == account)
            .ok_or(PairingError::NotFound)?;
        if r.state(now) == "active" {
            r.revoked_at = Some(now);
        }
        let out = r.clone();
        self.commit(&mut records, next, now)?;
        Ok(out)
    }
}

fn error(e: PairingError) -> Response {
    (
        e.status(),
        Json(serde_json::json!({"error": e.to_string()})),
    )
        .into_response()
}

#[derive(Deserialize)]
struct MintBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    team: Option<String>,
    #[serde(default)]
    ttl_secs: Option<u64>,
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/v1/pairing-codes", get(list).post(mint))
        .route("/v1/pairing-codes/{id}", delete(revoke))
}

/// The enrolled account caller of a pairing-code route.
async fn account(
    relay: &Relay,
    headers: &HeaderMap,
) -> Result<(Identity, Option<String>, bool), Response> {
    let (who, gate) = crate::api::account_caller(relay, headers).await?;
    let gate = gate.map_err(|m| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": m}))).into_response()
    })?;
    let proven = matches!(
        gate,
        crate::device_api::Gate::Device(_) | crate::device_api::Gate::Off
    ) || who.mfa;
    Ok((who, gate.device().map(str::to_owned), proven))
}

async fn mint(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(body): Json<MintBody>,
) -> Response {
    let (who, device, proven) = match account(&relay, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    // A team code: only the team's owner or an admin adds machines to it.
    if let Some(team) = body
        .team
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        match relay.teams.get_for(team, &who.account) {
            Ok((_, role)) if role.manages() => {}
            Ok(_) => {
                return error(PairingError::Forbidden(
                    "only the team's owner or an admin can add machines to it".into(),
                ))
            }
            Err(_) => return error(PairingError::NotFound),
        }
    }
    match relay.pairing.mint(Mint {
        who: &who,
        name: body.name,
        team: body.team,
        ttl_secs: body.ttl_secs,
        device: device.clone(),
        proven,
    }) {
        Ok((record, code)) => {
            relay.devices.audit(
                &who.account,
                AuditEvent::new("pairing_code_created")
                    .device(device.as_deref())
                    .detail(match &record.team {
                        Some(t) => {
                            format!("{} (team {t}), expires {}", record.id, record.expires_at)
                        }
                        None => format!("{}, expires {}", record.id, record.expires_at),
                    }),
            );
            let view = record.view(now_secs(), Some(display(&code)));
            (StatusCode::CREATED, Json(view)).into_response()
        }
        Err(e) => error(e),
    }
}

async fn list(State(relay): State<Relay>, headers: HeaderMap) -> Response {
    let (who, _, _) = match account(&relay, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let now = now_secs();
    let codes: Vec<_> = relay
        .pairing
        .list(&who.account)
        .iter()
        .map(|r| r.view(now, None))
        .collect();
    Json(serde_json::json!({ "codes": codes })).into_response()
}

async fn revoke(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (who, device, _) = match account(&relay, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    match relay.pairing.revoke(&who.account, &id) {
        Ok(record) => {
            if record.revoked_at.is_some() {
                relay.devices.audit(
                    &who.account,
                    AuditEvent::new("pairing_code_revoked")
                        .device(device.as_deref())
                        .detail(record.id.clone()),
                );
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ada() -> Identity {
        Identity {
            user: "ada-user".into(),
            account: "ada".into(),
            email: Some("ada@example.com".into()),
            name: None,
            auth_time: None,
            mfa: false,
            orgs: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_is_counted_and_mints_or_spends_nothing() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let storage = std::sync::Arc::new(Storage::new(Some(state.clone()), Default::default()));
        let path = state.join("relay.pairing.json");
        let store = PairingStore::open_with(path.clone(), storage.clone()).unwrap();
        let (first, code) = mint(&store, None);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains('\n'), "compact JSON: {raw}");
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o500)).unwrap();
        let minted = store.mint(Mint {
            who: &ada(),
            name: None,
            team: None,
            ttl_secs: None,
            device: None,
            proven: true,
        });
        let spent = store.spend(&first.id, "machine-0001");
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Root ignores directory permissions; nothing to check then.
        if storage.failures(store::PAIRING) > 0 {
            assert!(
                matches!(minted, Err(PairingError::Storage(_))),
                "{minted:?}"
            );
            assert!(matches!(spent, Err(PairingError::Storage(_))), "{spent:?}");
            assert_eq!(store.list("ada").len(), 1, "nothing minted");
            assert_eq!(store.peek(&code).unwrap().id, first.id, "still unspent");
            assert!(!store.writes_ok());
            assert!(!storage.report().0);
            // Writable again: the spend goes through.
            store.spend(&first.id, "machine-0001").unwrap();
        }
        assert!(store.writes_ok());
        let again = PairingStore::open(path).unwrap();
        assert!(again.peek(&code).is_err(), "spent across a restart");
    }

    fn mint(store: &PairingStore, ttl: Option<u64>) -> (PairingRecord, String) {
        store
            .mint(Mint {
                who: &ada(),
                name: None,
                team: Some("team-1".into()),
                ttl_secs: ttl,
                device: Some("dev_1".into()),
                proven: true,
            })
            .unwrap()
    }

    #[test]
    fn codes_normalize_like_people_type_them() {
        assert_eq!(normalize("abcd-efgh-jkmn").as_deref(), Some("ABCDEFGHJKMN"));
        assert_eq!(
            normalize(" cpc_ABCD EFGH JKMN ").as_deref(),
            Some("ABCDEFGHJKMN")
        );
        assert_eq!(normalize("OIL0-0000-0000").as_deref(), Some("011000000000"));
        assert_eq!(normalize("ABCD-EFGH-JKM"), None, "too short");
        assert_eq!(normalize("ABCD-EFGH-JKMU"), None, "U is not Crockford");
        assert_eq!(normalize("ABCD-EFGH-JKMNP"), None, "too long");
        assert_eq!(display("ABCDEFGHJKMN"), "ABCD-EFGH-JKMN");
        let c = new_code();
        assert_eq!(normalize(&display(&c)).as_deref(), Some(c.as_str()));
    }

    #[test]
    fn a_code_is_spent_once_and_can_be_restored_after_a_failed_registration() {
        let store = PairingStore::in_memory();
        let (record, code) = mint(&store, None);
        assert_eq!(record.expires_at - record.created_at, DEFAULT_TTL_SECS);
        let found = store.peek(&display(&code).to_lowercase()).unwrap();
        assert_eq!(found.id, record.id);
        store.spend(&record.id, "machine-1").unwrap();
        assert_eq!(store.peek(&code), Err(PairingError::Invalid));
        assert_eq!(
            store.spend(&record.id, "machine-2"),
            Err(PairingError::Invalid)
        );
        store.unspend(&record.id);
        assert!(store.peek(&code).is_ok());
    }

    #[test]
    fn revoked_and_expired_codes_are_refused_and_limits_hold() {
        let store = PairingStore::in_memory();
        let (record, code) = mint(&store, None);
        assert_eq!(store.revoke("eve", &record.id), Err(PairingError::NotFound));
        store.revoke("ada", &record.id).unwrap();
        assert_eq!(store.peek(&code), Err(PairingError::Invalid));
        assert_eq!(store.list("ada")[0].state(now_secs()), "revoked");

        // Lifetimes clamp; an expired code is refused.
        let (short, _) = mint(&store, Some(1));
        assert_eq!(short.expires_at - short.created_at, MIN_TTL_SECS);
        let (long, _) = mint(&store, Some(10 * MAX_TTL_SECS));
        assert_eq!(long.expires_at - long.created_at, MAX_TTL_SECS);
        assert_eq!(long.state(long.expires_at), "expired");

        for _ in 2..MAX_ACTIVE_PER_ACCOUNT {
            mint(&store, None);
        }
        let too_many = store.mint(Mint {
            who: &ada(),
            name: None,
            team: None,
            ttl_secs: None,
            device: None,
            proven: false,
        });
        assert_eq!(too_many.unwrap_err(), PairingError::TooMany);
    }

    #[test]
    fn only_hashes_are_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pairing.json");
        let store = PairingStore::open(path.clone()).unwrap();
        let (record, code) = mint(&store, None);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(&code));
        assert!(raw.contains(&record.code_sha256));
        let reopened = PairingStore::open(path).unwrap();
        assert_eq!(reopened.peek(&code).unwrap().id, record.id);
    }
}
