// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pending deletions: the Spaces of a removed computer that its owner asked
//! to delete (`DELETE /v1/machines/{id}?spaces=delete`, see [`crate::api`]).
//!
//! A computer can be removed while it is offline, so the relay cannot
//! delete its Spaces on it. The relay forgets their records at once (they
//! leave every list), and keeps a pending-deletion record per computer:
//! the Spaces' ids and names, who asked, and the computer's token hash at
//! that moment. The computer (its cua-spacesd) reads the record whenever it
//! dials the relay, deletes each listed Space through its cua daemon, and
//! acks each one; the last ack clears the record.
//!
//! - `GET /v1/machines/{id}/pending-deletions`: `{computer, spaces: [{id,
//!   name}], requested_at, expires_at}` (an empty `spaces` when nothing is
//!   pending).
//! - `POST /v1/machines/{id}/pending-deletions/{space}/ack`: `204`;
//!   idempotent (a Space that is not pending is already done).
//!
//! Only that computer's machine token may call them: the token it had when
//! it was removed (the record keeps its hash), or the one it got when it was
//! added again with the same id. Another computer's token gets `403`, an
//! account token `403`, an unknown token `401`.
//!
//! Deleted means deleted: adding the computer again keeps the record, and
//! registering a pending Space id again is refused (`410`).
//!
//! A record expires [`TTL_SECS`] (90 days) after the request: a computer
//! that never comes back must not pin it forever. It persists in
//! `<state>.pending-deletions.json` (atomic replace, private file mode) and
//! is erased with the account ([`crate::account_erase`]).

// Handlers short-circuit with ready-made responses.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::error;
use crate::assertion::{now_secs, remove_stale_temps, write_private_with};
use crate::directory::{hash_token, MACHINE_TOKEN_PREFIX};
use crate::server::{bearer, Relay};
use crate::storage::{store, Storage};

/// How long a pending-deletion record lives: 90 days.
pub const TTL_SECS: u64 = 90 * 86_400;

/// One Space to delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSpace {
    /// Its relay machine id (what the computer's daemon knows it by).
    pub id: String,
    /// Its name when it was deleted (for people and logs).
    #[serde(default)]
    pub name: String,
}

/// A removed computer's Spaces to delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstone {
    /// The owning account.
    pub owner: String,
    /// SHA-256 (hex) of the computer's machine token when it was removed.
    pub token_sha256: String,
    /// Who asked (the device or account that removed it), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
    /// Unix seconds of the (latest) request.
    pub requested_at: u64,
    /// What is left to delete.
    pub spaces: Vec<PendingSpace>,
}

impl Tombstone {
    fn expired(&self, now: u64) -> bool {
        now >= self.requested_at.saturating_add(TTL_SECS)
    }
}

/// Every computer's pending deletions, persisted as JSON (atomic replace)
/// when a file is configured.
#[derive(Debug, Default)]
pub struct PendingDeletions {
    path: Option<PathBuf>,
    computers: Mutex<BTreeMap<String, Tombstone>>,
    storage: Arc<Storage>,
}

impl PendingDeletions {
    /// In memory only.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads (or starts) the store persisted at `path`, reporting writes to
    /// `storage`. Expired records are dropped (on the next write).
    pub fn open_with(path: PathBuf, storage: Arc<Storage>) -> std::io::Result<Self> {
        remove_stale_temps(&path)?;
        let mut computers: BTreeMap<String, Tombstone> = match std::fs::read(&path) {
            Ok(raw) => serde_json::from_slice(&raw)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        let now = now_secs();
        computers.retain(|_, t| !t.expired(now));
        Ok(Self {
            path: Some(path),
            computers: Mutex::new(computers),
            storage,
        })
    }

    /// Writes `next` and swaps it in; on a failed write nothing changes.
    fn commit(
        &self,
        computers: &mut BTreeMap<String, Tombstone>,
        next: BTreeMap<String, Tombstone>,
    ) -> Result<(), String> {
        if let Some(path) = &self.path {
            let written = (|| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_private_with(path, |out| {
                    serde_json::to_writer(&mut *out, &next).map_err(std::io::Error::other)
                })
            })();
            self.storage.record(store::PENDING_DELETIONS, &written);
            written.map_err(|e| e.to_string())?;
        }
        *computers = next;
        Ok(())
    }

    /// Records that `computer` (owned by `owner`, whose token hash was
    /// `token_sha256`) must delete `spaces`. A record already there gains
    /// them (and the newest token hash); nothing to delete records nothing.
    pub fn record(
        &self,
        computer: &str,
        owner: &str,
        token_sha256: &str,
        requested_by: Option<String>,
        spaces: Vec<PendingSpace>,
    ) -> Result<(), String> {
        self.record_at(
            computer,
            owner,
            token_sha256,
            requested_by,
            spaces,
            now_secs(),
        )
    }

    /// [`Self::record`] at `now`.
    pub fn record_at(
        &self,
        computer: &str,
        owner: &str,
        token_sha256: &str,
        requested_by: Option<String>,
        spaces: Vec<PendingSpace>,
        now: u64,
    ) -> Result<(), String> {
        if spaces.is_empty() {
            return Ok(());
        }
        let mut computers = self.computers.lock().expect("pending deletions");
        let mut next = computers.clone();
        next.retain(|_, t| !t.expired(now));
        let entry = next
            .entry(computer.to_owned())
            .or_insert_with(|| Tombstone {
                owner: owner.to_owned(),
                token_sha256: String::new(),
                requested_by: None,
                requested_at: now,
                spaces: Vec::new(),
            });
        entry.owner = owner.to_owned();
        entry.token_sha256 = token_sha256.to_owned();
        entry.requested_by = requested_by;
        entry.requested_at = now;
        for s in spaces {
            if !entry.spaces.iter().any(|e| e.id == s.id) {
                entry.spaces.push(s);
            }
        }
        self.commit(&mut computers, next)
    }

    /// `computer`'s record, unless there is none or it expired.
    pub fn get(&self, computer: &str) -> Option<Tombstone> {
        self.get_at(computer, now_secs())
    }

    /// [`Self::get`] at `now`.
    pub fn get_at(&self, computer: &str, now: u64) -> Option<Tombstone> {
        self.computers
            .lock()
            .expect("pending deletions")
            .get(computer)
            .filter(|t| !t.expired(now))
            .cloned()
    }

    /// Whether `space` is waiting to be deleted on some computer (its id
    /// may not be registered again meanwhile).
    pub fn is_pending_space(&self, space: &str) -> bool {
        let now = now_secs();
        self.computers
            .lock()
            .expect("pending deletions")
            .values()
            .any(|t| !t.expired(now) && t.spaces.iter().any(|s| s.id == space))
    }

    /// The computer whose record names `token_sha256`, if any (and not
    /// expired).
    fn computer_for_token(&self, token_sha256: &str) -> Option<String> {
        use subtle::ConstantTimeEq as _;
        let now = now_secs();
        self.computers
            .lock()
            .expect("pending deletions")
            .iter()
            .find(|(_, t)| {
                !t.expired(now)
                    && !t.token_sha256.is_empty()
                    && bool::from(t.token_sha256.as_bytes().ct_eq(token_sha256.as_bytes()))
            })
            .map(|(id, _)| id.clone())
    }

    /// `computer` deleted `space`: it leaves the record, and the last one
    /// clears it. Returns whether it was pending.
    pub fn ack(&self, computer: &str, space: &str) -> Result<bool, String> {
        self.ack_at(computer, space, now_secs())
    }

    /// [`Self::ack`] at a given time (tests).
    pub fn ack_at(&self, computer: &str, space: &str, now: u64) -> Result<bool, String> {
        let mut computers = self.computers.lock().expect("pending deletions");
        let mut next = computers.clone();
        next.retain(|_, t| !t.expired(now));
        let Some(t) = next.get_mut(computer) else {
            return Ok(false);
        };
        let before = t.spaces.len();
        t.spaces.retain(|s| s.id != space);
        let was = t.spaces.len() != before;
        if t.spaces.is_empty() {
            next.remove(computer);
        }
        if !was && next.len() == computers.len() {
            return Ok(false);
        }
        self.commit(&mut computers, next)?;
        Ok(was)
    }

    /// Account deletion: forgets every record of `account`'s computers.
    /// Returns how many there were. The file is rewritten every call, so a
    /// retry after a failed write finishes the job.
    pub fn erase_account(&self, account: &str) -> Result<usize, String> {
        let mut computers = self.computers.lock().expect("pending deletions");
        let mut next = computers.clone();
        let before = next.len();
        next.retain(|_, t| t.owner != account);
        let gone = before - next.len();
        self.commit(&mut computers, next)?;
        Ok(gone)
    }
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/v1/machines/{id}/pending-deletions", get(list))
        .route("/v1/machines/{id}/pending-deletions/{space}/ack", post(ack))
}

/// Lets only computer `id`'s own machine token through: the one it had
/// when removed (kept in the record), or its current one.
fn authorize(relay: &Relay, headers: &HeaderMap, id: &str) -> Result<(), Response> {
    let Some(token) = bearer(headers, header::AUTHORIZATION.as_str()) else {
        return Err(error(StatusCode::UNAUTHORIZED, "missing computer token"));
    };
    if !token.starts_with(MACHINE_TOKEN_PREFIX) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "only the computer itself reads its pending deletions",
        ));
    }
    let hashed = hash_token(token);
    if let Some(t) = relay.pending_deletions.get(id) {
        use subtle::ConstantTimeEq as _;
        if bool::from(t.token_sha256.as_bytes().ct_eq(hashed.as_bytes())) {
            return Ok(());
        }
    }
    let other = match relay.directory.machine_for_token(token) {
        Some(r) if r.id == id => return Ok(()),
        Some(_) => true,
        None => relay
            .pending_deletions
            .computer_for_token(&hashed)
            .is_some(),
    };
    if other {
        return Err(error(
            StatusCode::FORBIDDEN,
            "computer token belongs to another computer id",
        ));
    }
    Err(error(StatusCode::UNAUTHORIZED, "invalid relay token"))
}

/// What `GET .../pending-deletions` answers.
#[derive(Debug, Serialize)]
struct View {
    computer: String,
    spaces: Vec<PendingSpace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

async fn list(State(relay): State<Relay>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    if let Err(r) = authorize(&relay, &headers, &id) {
        return r;
    }
    let t = relay.pending_deletions.get(&id);
    Json(View {
        computer: id,
        requested_at: t.as_ref().map(|t| t.requested_at),
        expires_at: t.as_ref().map(|t| t.requested_at.saturating_add(TTL_SECS)),
        spaces: t.map(|t| t.spaces).unwrap_or_default(),
    })
    .into_response()
}

async fn ack(
    State(relay): State<Relay>,
    Path((id, space)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = authorize(&relay, &headers, &id) {
        return r;
    }
    let space = space.strip_prefix("relay:").unwrap_or(&space);
    match relay.pending_deletions.ack(&id, space) {
        Ok(was) => {
            if was {
                tracing::info!(computer = %id, space, "pending Space deletion acknowledged");
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(id: &str) -> PendingSpace {
        PendingSpace {
            id: id.into(),
            name: format!("{id}-name"),
        }
    }

    #[test]
    fn records_merge_ack_clears_and_expire() {
        let p = PendingDeletions::in_memory();
        p.record_at("c1", "ada", "h1", None, vec![], 100).unwrap();
        assert!(
            p.get_at("c1", 100).is_none(),
            "nothing to delete records nothing"
        );

        p.record_at(
            "c1",
            "ada",
            "h1",
            Some("dev".into()),
            vec![space("s1")],
            100,
        )
        .unwrap();
        p.record_at("c1", "ada", "h2", None, vec![space("s1"), space("s2")], 200)
            .unwrap();
        let t = p.get_at("c1", 200).unwrap();
        assert_eq!(t.spaces, vec![space("s1"), space("s2")]);
        assert_eq!((t.token_sha256.as_str(), t.requested_at), ("h2", 200));

        // Expiry counts from the latest request.
        assert!(p.get_at("c1", 200 + TTL_SECS - 1).is_some());
        assert!(p.get_at("c1", 200 + TTL_SECS).is_none());

        assert!(p.ack_at("c1", "s1", 200).unwrap());
        assert!(!p.ack_at("c1", "s1", 200).unwrap(), "idempotent");
        assert!(!p.ack_at("c1", "nope", 200).unwrap());
        assert!(!p.ack_at("c9", "s2", 200).unwrap());
        assert!(p.ack_at("c1", "s2", 200).unwrap());
        assert!(
            p.computers.lock().unwrap().is_empty(),
            "the last ack clears it"
        );
    }

    #[test]
    fn persists_drops_expired_on_load_and_erases_by_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.pending-deletions.json");
        let p = PendingDeletions::open_with(path.clone(), Arc::default()).unwrap();
        let now = now_secs();
        p.record_at(
            "old",
            "ada",
            "h0",
            None,
            vec![space("s0")],
            now - TTL_SECS - 1,
        )
        .unwrap();
        p.record_at("c1", "ada", "h1", None, vec![space("s1")], now)
            .unwrap();
        p.record_at("c2", "bob", "h2", None, vec![space("s2")], now)
            .unwrap();
        drop(p);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "private file");
        }

        let p = PendingDeletions::open_with(path.clone(), Arc::default()).unwrap();
        assert!(p.get("old").is_none(), "expired on load");
        assert!(p.get("c1").is_some() && p.get("c2").is_some());
        assert!(p.is_pending_space("s1") && !p.is_pending_space("s0"));
        assert_eq!(p.computer_for_token("h2").as_deref(), Some("c2"));

        assert_eq!(p.erase_account("ada").unwrap(), 1);
        assert_eq!(p.erase_account("ada").unwrap(), 0);
        drop(p);
        let p = PendingDeletions::open_with(path, Arc::default()).unwrap();
        assert!(p.get("c1").is_none() && p.get("c2").is_some());
    }
}
