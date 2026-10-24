// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Machine directory of the account mode: which account owns which
//! machine id, its display name, allowlist, sharing switch and the hash of
//! its machine token. Persisted as JSON (atomic replace) when a state file
//! is configured; in memory otherwise.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::assertion::{now_secs, remove_stale_temps, write_private_with};
use crate::oidc::Identity;
use crate::storage::{store, Storage};

/// Prefix of machine tokens (tells them apart from static relay tokens and
/// account JWTs).
pub const MACHINE_TOKEN_PREFIX: &str = "cmt_";

/// The owner of a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    /// Account id.
    pub id: String,
    /// Email, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One registered machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineRecord {
    /// Machine id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Owner.
    pub owner: Owner,
    /// Account ids or emails the owner shares the machine with.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Account ids or emails that may only watch: presence and the view-only
    /// stream, no input, no processes, no files. An entry here wins over the
    /// same account in `allow`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub viewers: Vec<String>,
    /// False after "stop sharing": no client reaches the machine.
    #[serde(default = "yes")]
    pub sharing: bool,
    /// Who stopped sharing (`machine` or `owner`) while `sharing` is off.
    /// The machine token may only undo a stop it made itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_by: Option<String>,
    /// SHA-256 (hex) of the current machine token.
    pub token_sha256: String,
    /// Registration time (Unix seconds).
    #[serde(default)]
    pub created_at: u64,
    /// For a Space a host provides: that host's machine id. Clients group
    /// such machines under their host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Registered with proof the caller was an enrolled device or a fresh
    /// sign-in whose token proved MFA (S5), or confirmed since by an
    /// enrolled device (`POST /v1/machines/{id}/confirm`). A record
    /// registered with only an account token (no device session, no MFA
    /// claim) starts `false`: apps show it as "new" until confirmed, so a
    /// rogue machine a stolen session added stands out instead of blending
    /// in as one of the owner's own. Records from before this field existed
    /// default to confirmed (never retroactively flagged).
    #[serde(default = "yes")]
    pub confirmed: bool,
    /// What the registering client says about the machine (a Space in the
    /// owner's own cloud: its provider and place): no secrets, at most
    /// [`MAX_META`] keys `[a-z0-9._-]{1,64}` with values of at most 256
    /// characters. Shown to everyone who can see the machine.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, String>,
}

fn yes() -> bool {
    true
}

/// How a caller relates to a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The owning account.
    Owner,
    /// An allowlisted account.
    Shared,
    /// An account the owner shared the machine with to watch only.
    Viewer,
}

impl Role {
    /// `owner` / `shared`.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Shared => "shared",
            Role::Viewer => "viewer",
        }
    }
}

impl MachineRecord {
    /// The caller's role, if any.
    pub fn role_of(&self, who: &Identity) -> Option<Role> {
        if who.account == self.owner.id {
            return Some(Role::Owner);
        }
        let email = who.email.as_deref().unwrap_or("");
        let listed = |list: &[String]| {
            list.iter().any(|a| {
                a == &who.account
                    || a == &who.user
                    || (!email.is_empty() && a.eq_ignore_ascii_case(email))
            })
        };
        if listed(&self.viewers) {
            Some(Role::Viewer)
        } else if listed(&self.allow) {
            Some(Role::Shared)
        } else {
            None
        }
    }
}

/// Directory errors, mapped to HTTP statuses by the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryError {
    /// 400.
    Invalid(String),
    /// 403.
    Forbidden(String),
    /// 404.
    NotFound,
    /// 409.
    Conflict(String),
    /// 500.
    Storage(String),
}

impl std::fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m) | Self::Forbidden(m) | Self::Conflict(m) | Self::Storage(m) => {
                f.write_str(m)
            }
            Self::NotFound => f.write_str("no such computer"),
        }
    }
}

/// Changes applied by `PATCH /v1/machines/{id}`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MachinePatch {
    /// New display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Replacement allowlist.
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    /// Replacement view-only list.
    #[serde(default)]
    pub viewers: Option<Vec<String>>,
    /// Sharing switch.
    #[serde(default)]
    pub sharing: Option<bool>,
    /// Meta keys to set, merged into [`MachineRecord::meta`]: an empty
    /// value removes the key. The merged result is checked with
    /// [`clean_meta`].
    #[serde(default)]
    pub meta: Option<BTreeMap<String, String>>,
}

/// The directory.
#[derive(Debug, Default)]
pub struct Directory {
    path: Option<PathBuf>,
    records: Mutex<BTreeMap<String, MachineRecord>>,
    max_per_account: usize,
    /// Where write failures are counted and reported.
    storage: std::sync::Arc<Storage>,
}

fn hash_token(token: &str) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, token.as_bytes()).as_ref())
}

fn new_token() -> String {
    use rand::RngCore as _;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!(
        "{MACHINE_TOKEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn clean_allow(allow: &[String]) -> Result<Vec<String>, DirectoryError> {
    if allow.len() > 64 {
        return Err(DirectoryError::Invalid(
            "at most 64 allowlist entries".into(),
        ));
    }
    let mut out: Vec<String> = Vec::new();
    for entry in allow {
        let e = entry.trim();
        if e.is_empty() || e.len() > 254 || e.chars().any(|c| c.is_control() || c == ' ') {
            return Err(DirectoryError::Invalid(format!(
                "invalid allowlist entry {entry:?}"
            )));
        }
        let e = if e.contains('@') {
            e.to_ascii_lowercase()
        } else {
            e.to_owned()
        };
        if !out.contains(&e) {
            out.push(e);
        }
    }
    Ok(out)
}

impl Directory {
    /// An in-memory directory.
    pub fn in_memory(max_per_account: usize) -> Self {
        Self {
            path: None,
            records: Mutex::default(),
            max_per_account,
            storage: Default::default(),
        }
    }

    /// Loads (or starts) the directory persisted at `path`, first removing
    /// temp files an interrupted write left behind.
    pub fn open(path: PathBuf, max_per_account: usize) -> std::io::Result<Self> {
        Self::open_with(path, max_per_account, Default::default())
    }

    /// [`Directory::open`], reporting writes to `storage`.
    pub fn open_with(
        path: PathBuf,
        max_per_account: usize,
        storage: std::sync::Arc<Storage>,
    ) -> std::io::Result<Self> {
        remove_stale_temps(&path)?;
        let records = match std::fs::read(&path) {
            Ok(raw) => {
                let list: Vec<MachineRecord> = serde_json::from_slice(&raw)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                list.into_iter().map(|r| (r.id.clone(), r)).collect()
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: Some(path),
            records: Mutex::new(records),
            max_per_account,
            storage,
        })
    }

    /// Whether the directory file is writable: no write failed lately
    /// without a successful one since (true for an in-memory directory).
    pub fn writes_ok(&self) -> bool {
        self.storage.store_ok(store::DIRECTORY)
    }

    /// Writes the directory file: compact JSON streamed to a temp file
    /// that replaces it.
    fn persist(&self, records: &BTreeMap<String, MachineRecord>) -> Result<(), DirectoryError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let written = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_private_with(path, |out| {
                serde_json::to_writer(&mut *out, &records.values().collect::<Vec<_>>())
                    .map_err(std::io::Error::other)
            })
        })();
        self.storage.record(store::DIRECTORY, &written);
        written.map_err(|e| DirectoryError::Storage(e.to_string()))
    }

    /// Makes `id` hold `record` (`None`: removed) once the directory with
    /// it is persisted: on a failed write `records` is left as it was and
    /// the caller gets the error. Nothing is written when nothing changed.
    fn commit(
        &self,
        records: &mut BTreeMap<String, MachineRecord>,
        id: &str,
        record: Option<MachineRecord>,
    ) -> Result<(), DirectoryError> {
        if records.get(id) == record.as_ref() {
            return Ok(());
        }
        let old = match record {
            Some(r) => records.insert(id.to_owned(), r),
            None => records.remove(id),
        };
        if let Err(e) = self.persist(records) {
            match old {
                Some(o) => records.insert(id.to_owned(), o),
                None => records.remove(id),
            };
            return Err(e);
        }
        Ok(())
    }

    /// Registers `id` for `who` (or rotates its token when `who` already
    /// owns it). `confirmed` sets [`MachineRecord::confirmed`] for a brand
    /// new record only; re-registering an existing one leaves it as it is.
    /// Returns the record and the new machine token.
    pub fn register(
        &self,
        who: &Identity,
        id: &str,
        name: Option<&str>,
        allow: Option<&[String]>,
        confirmed: bool,
    ) -> Result<(MachineRecord, String), DirectoryError> {
        if !crate::valid_machine_id(id) {
            return Err(DirectoryError::Invalid("invalid computer id".into()));
        }
        let allow = allow.map(clean_allow).transpose()?;
        let token = new_token();
        let mut records = self.records.lock().expect("directory");
        let record = match records.get(id) {
            Some(existing) if existing.owner.id != who.account => {
                return Err(DirectoryError::Conflict(
                    "computer id registered to another account".into(),
                ))
            }
            Some(existing) => {
                let mut r = existing.clone();
                if let Some(name) = name.map(clean_name).filter(|n| !n.is_empty()) {
                    r.name = name;
                }
                if let Some(allow) = allow {
                    r.allow = allow;
                }
                r.owner.email = who.email.clone().or(r.owner.email);
                r.owner.name = who.name.clone().or(r.owner.name);
                r.token_sha256 = hash_token(&token);
                r
            }
            None => {
                let owned = records
                    .values()
                    .filter(|r| r.owner.id == who.account)
                    .count();
                if owned >= self.max_per_account {
                    return Err(DirectoryError::Forbidden(format!(
                        "an account may register at most {} computers",
                        self.max_per_account
                    )));
                }
                MachineRecord {
                    id: id.to_owned(),
                    name: name
                        .map(clean_name)
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| id.chars().take(12).collect()),
                    owner: Owner {
                        id: who.account.clone(),
                        email: who.email.clone(),
                        name: who.name.clone(),
                    },
                    allow: allow.unwrap_or_default(),
                    viewers: Vec::new(),
                    sharing: true,
                    stopped_by: None,
                    token_sha256: hash_token(&token),
                    created_at: now_secs(),
                    host: None,
                    confirmed,
                    meta: BTreeMap::new(),
                }
            }
        };
        self.commit(&mut records, id, Some(record.clone()))?;
        Ok((record, token))
    }

    /// Marks `id` confirmed (an enrolled device vouched for a machine that
    /// registered without proof; S5).
    pub fn confirm(&self, id: &str) -> Result<MachineRecord, DirectoryError> {
        let mut records = self.records.lock().expect("directory");
        let mut changed = records.get(id).cloned().ok_or(DirectoryError::NotFound)?;
        let record = &mut changed;
        record.confirmed = true;
        let updated = record.clone();
        self.commit(&mut records, id, Some(changed))?;
        Ok(updated)
    }

    /// Replaces a machine's [`MachineRecord::meta`] (checked with
    /// [`clean_meta`]).
    pub fn set_meta(
        &self,
        id: &str,
        meta: BTreeMap<String, String>,
    ) -> Result<MachineRecord, DirectoryError> {
        let meta = clean_meta(meta)?;
        let mut records = self.records.lock().expect("directory");
        let mut changed = records.get(id).cloned().ok_or(DirectoryError::NotFound)?;
        let record = &mut changed;
        record.meta = meta;
        let record = record.clone();
        self.commit(&mut records, id, Some(changed))?;
        Ok(record)
    }

    /// Records the host a machine (a Space that host provides) belongs to.
    pub fn set_host(&self, id: &str, host: Option<&str>) -> Result<MachineRecord, DirectoryError> {
        if let Some(h) = host {
            if !crate::valid_machine_id(h) {
                return Err(DirectoryError::Invalid("invalid host computer id".into()));
            }
        }
        let mut records = self.records.lock().expect("directory");
        let mut changed = records.get(id).cloned().ok_or(DirectoryError::NotFound)?;
        let record = &mut changed;
        record.host = host.map(str::to_owned);
        let record = record.clone();
        self.commit(&mut records, id, Some(changed))?;
        Ok(record)
    }

    /// The machine a machine token belongs to.
    pub fn machine_for_token(&self, token: &str) -> Option<MachineRecord> {
        if !token.starts_with(MACHINE_TOKEN_PREFIX) {
            return None;
        }
        let hashed = hash_token(token);
        self.records
            .lock()
            .expect("directory")
            .values()
            .find(|r| {
                use subtle::ConstantTimeEq as _;
                bool::from(r.token_sha256.as_bytes().ct_eq(hashed.as_bytes()))
            })
            .cloned()
    }

    /// A record by id.
    pub fn get(&self, id: &str) -> Option<MachineRecord> {
        self.records.lock().expect("directory").get(id).cloned()
    }

    /// Every record.
    pub fn all(&self) -> Vec<MachineRecord> {
        self.records
            .lock()
            .expect("directory")
            .values()
            .cloned()
            .collect()
    }

    /// Every record `who` owns or is allowlisted on, with the role.
    pub fn visible_to(&self, who: &Identity) -> Vec<(MachineRecord, Role)> {
        self.records
            .lock()
            .expect("directory")
            .values()
            .filter_map(|r| r.role_of(who).map(|role| (r.clone(), role)))
            .collect()
    }

    /// Applies `patch` to machine `id`.
    pub fn update(&self, id: &str, patch: &MachinePatch) -> Result<MachineRecord, DirectoryError> {
        let allow = patch.allow.as_deref().map(clean_allow).transpose()?;
        let viewers = patch.viewers.as_deref().map(clean_allow).transpose()?;
        let mut records = self.records.lock().expect("directory");
        let mut changed = records.get(id).cloned().ok_or(DirectoryError::NotFound)?;
        let record = &mut changed;
        if let Some(meta) = &patch.meta {
            let mut merged = record.meta.clone();
            for (k, v) in meta {
                if v.is_empty() {
                    merged.remove(k);
                } else {
                    merged.insert(k.clone(), v.clone());
                }
            }
            record.meta = clean_meta(merged)?;
        }
        if let Some(name) = &patch.name {
            let name = clean_name(name);
            if name.is_empty() {
                return Err(DirectoryError::Invalid("empty name".into()));
            }
            record.name = name;
        }
        if let Some(allow) = allow {
            record.allow = allow;
        }
        if let Some(viewers) = viewers {
            record.viewers = viewers;
        }
        if record.allow.len() + record.viewers.len() > 64 {
            return Err(DirectoryError::Invalid(
                "at most 64 shared accounts per computer".into(),
            ));
        }
        if let Some(sharing) = patch.sharing {
            record.sharing = sharing;
            record.stopped_by = (!sharing).then(|| "owner".to_owned());
        }
        let updated = record.clone();
        self.commit(&mut records, id, Some(changed))?;
        Ok(updated)
    }

    /// Removes `people` (emails or account ids, any case) from the
    /// allowlist and the viewers of machine `id`. Returns the record when
    /// something was removed, `None` when nobody was listed (so a repeat
    /// changes nothing).
    pub fn remove_shares(
        &self,
        id: &str,
        people: &[String],
    ) -> Result<Option<MachineRecord>, DirectoryError> {
        let mut records = self.records.lock().expect("directory");
        let Some(mut changed) = records.get(id).cloned() else {
            return Ok(None);
        };
        let gone = |a: &String| people.iter().any(|p| p.eq_ignore_ascii_case(a));
        let before = changed.allow.len() + changed.viewers.len();
        changed.allow.retain(|a| !gone(a));
        changed.viewers.retain(|a| !gone(a));
        if changed.allow.len() + changed.viewers.len() == before {
            return Ok(None);
        }
        let updated = changed.clone();
        self.commit(&mut records, id, Some(changed))?;
        Ok(Some(updated))
    }

    /// Turns sharing on or off; `by` (`machine` or `owner`) is remembered
    /// while it is off.
    pub fn set_sharing(
        &self,
        id: &str,
        sharing: bool,
        by: &str,
    ) -> Result<MachineRecord, DirectoryError> {
        let mut records = self.records.lock().expect("directory");
        let mut changed = records.get(id).cloned().ok_or(DirectoryError::NotFound)?;
        let record = &mut changed;
        record.sharing = sharing;
        record.stopped_by = (!sharing).then(|| by.to_owned());
        let updated = record.clone();
        self.commit(&mut records, id, Some(changed))?;
        Ok(updated)
    }

    /// Removes machine `id` (revoking its token).
    pub fn remove(&self, id: &str) -> Result<MachineRecord, DirectoryError> {
        let mut records = self.records.lock().expect("directory");
        let removed = records.get(id).cloned().ok_or(DirectoryError::NotFound)?;
        self.commit(&mut records, id, None)?;
        Ok(removed)
    }
}

/// At most this many [`MachineRecord::meta`] keys.
pub const MAX_META: usize = 16;

/// Checks machine metadata: at most [`MAX_META`] keys of `[a-z0-9._-]`, 1
/// to 64 characters, and values of at most 256 characters with no control
/// characters.
pub fn clean_meta(
    meta: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, DirectoryError> {
    if meta.len() > MAX_META {
        return Err(DirectoryError::Invalid(format!(
            "at most {MAX_META} meta keys"
        )));
    }
    for (k, v) in &meta {
        let key_ok = (1..=64).contains(&k.len())
            && k.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if !key_ok {
            return Err(DirectoryError::Invalid(format!("invalid meta key {k:?}")));
        }
        if v.chars().count() > 256 || v.chars().any(char::is_control) {
            return Err(DirectoryError::Invalid(format!(
                "invalid meta value for {k:?}"
            )));
        }
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(account: &str, email: Option<&str>) -> Identity {
        Identity {
            user: account.into(),
            account: account.into(),
            email: email.map(str::to_owned),
            name: None,
            auth_time: None,
            mfa: false,
            orgs: Vec::new(),
        }
    }

    #[test]
    fn meta_is_checked_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dir.json");
        let d = Directory::open(path.clone(), 32).unwrap();
        let who = who("acct", None);
        d.register(&who, "cloud-00000000000000aa", Some("n"), None, true)
            .unwrap();
        let meta = BTreeMap::from([
            ("cua.cloud.provider".to_string(), "aws".to_string()),
            (
                "cua.cloud.place".to_string(),
                "AWS \u{b7} us-west-2".to_string(),
            ),
        ]);
        let r = d.set_meta("cloud-00000000000000aa", meta.clone()).unwrap();
        assert_eq!(r.meta, meta);
        let again = Directory::open(path, 32).unwrap();
        assert_eq!(again.get("cloud-00000000000000aa").unwrap().meta, meta);
        for bad in [
            BTreeMap::from([("Upper".to_string(), "x".to_string())]),
            BTreeMap::from([("k".to_string(), "x".repeat(257))]),
            BTreeMap::from([("k".to_string(), "a\nb".to_string())]),
            (0..17)
                .map(|i| (format!("k{i}"), "v".to_string()))
                .collect(),
        ] {
            assert!(matches!(clean_meta(bad), Err(DirectoryError::Invalid(_))));
        }
    }

    #[test]
    fn a_patch_merges_meta_removes_empty_values_and_checks_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dir.json");
        let d = Directory::open(path.clone(), 32).unwrap();
        let who = who("acct", None);
        d.register(&who, "machine-0001", Some("n"), None, true)
            .unwrap();
        d.set_meta(
            "machine-0001",
            BTreeMap::from([("cua.host.os".to_string(), "macos".to_string())]),
        )
        .unwrap();
        let patch = |meta: &[(&str, &str)]| MachinePatch {
            meta: Some(
                meta.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            ..Default::default()
        };
        // New keys join what is there.
        let r = d
            .update("machine-0001", &patch(&[("cua.res", "{\"v\":1}")]))
            .unwrap();
        assert_eq!(r.meta.len(), 2);
        assert_eq!(r.meta["cua.host.os"], "macos");
        assert_eq!(r.meta["cua.res"], "{\"v\":1}");
        // A later value replaces the earlier one; the other key stays.
        let r = d
            .update("machine-0001", &patch(&[("cua.res", "{\"v\":2}")]))
            .unwrap();
        assert_eq!(r.meta["cua.res"], "{\"v\":2}");
        assert_eq!(r.meta["cua.host.os"], "macos");
        // An empty value removes the key.
        let r = d
            .update("machine-0001", &patch(&[("cua.res", "")]))
            .unwrap();
        assert!(!r.meta.contains_key("cua.res"));
        assert_eq!(r.meta["cua.host.os"], "macos");
        // Bad meta changes nothing.
        assert!(matches!(
            d.update("machine-0001", &patch(&[("Bad Key", "x")])),
            Err(DirectoryError::Invalid(_))
        ));
        assert!(matches!(
            d.update("machine-0001", &patch(&[("cua.res", &"x".repeat(257))])),
            Err(DirectoryError::Invalid(_))
        ));
        let again = Directory::open(path, 32).unwrap();
        assert_eq!(again.get("machine-0001").unwrap().meta, r.meta);
        // A patch without meta leaves it alone.
        let r = d.update("machine-0001", &MachinePatch::default()).unwrap();
        assert_eq!(r.meta, again.get("machine-0001").unwrap().meta);
    }

    #[test]
    fn register_share_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/machines.json");
        let d = Directory::open(path.clone(), 2).unwrap();
        let ada = who("ada", Some("ada@example.com"));
        let bob = who("bob", Some("Bob@Example.com"));
        let (rec, token) = d
            .register(&ada, "machine-0001", Some("Ada's Mac"), None, true)
            .unwrap();
        assert!(token.starts_with(MACHINE_TOKEN_PREFIX));
        assert_eq!(d.machine_for_token(&token).unwrap().id, rec.id);
        assert!(d.machine_for_token("cmt_nope").is_none());
        // Someone else cannot take the id; re-registering rotates the token.
        assert!(matches!(
            d.register(&bob, "machine-0001", None, None, true),
            Err(DirectoryError::Conflict(_))
        ));
        let (_, rotated) = d.register(&ada, "machine-0001", None, None, true).unwrap();
        assert!(d.machine_for_token(&token).is_none());
        assert!(d.machine_for_token(&rotated).is_some());
        // Sharing by email (case-insensitive).
        assert_eq!(rec.role_of(&bob), None);
        d.update(
            "machine-0001",
            &MachinePatch {
                allow: Some(vec!["BOB@example.com".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        let visible = d.visible_to(&bob);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].1, Role::Shared);
        assert_eq!(d.visible_to(&ada)[0].1, Role::Owner);
        // A viewer entry wins over the same account in the allowlist.
        let rec = d
            .update(
                "machine-0001",
                &MachinePatch {
                    viewers: Some(vec!["bob@example.com".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(rec.role_of(&bob), Some(Role::Viewer));
        assert_eq!(Role::Viewer.as_str(), "viewer");
        let rec = d
            .update(
                "machine-0001",
                &MachinePatch {
                    viewers: Some(vec![]),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(rec.role_of(&bob), Some(Role::Shared));
        assert!(d
            .update(
                "machine-0001",
                &MachinePatch {
                    viewers: Some(vec!["not valid".into()]),
                    ..Default::default()
                },
            )
            .is_err());
        // Per-account limit.
        d.register(&ada, "machine-0002", None, None, true).unwrap();
        assert!(matches!(
            d.register(&ada, "machine-0003", None, None, true),
            Err(DirectoryError::Forbidden(_))
        ));
        // Persisted and reloaded.
        let reopened = Directory::open(path, 2).unwrap();
        assert_eq!(
            reopened.get("machine-0001").unwrap().allow,
            ["bob@example.com"]
        );
        assert!(reopened.machine_for_token(&rotated).is_some());
        reopened.remove("machine-0001").unwrap();
        assert!(reopened.machine_for_token(&rotated).is_none());
        assert!(matches!(
            d.update("nope-0000", &MachinePatch::default()),
            Err(DirectoryError::NotFound)
        ));
    }

    #[test]
    fn allowlist_is_validated() {
        let d = Directory::in_memory(4);
        let ada = who("ada", None);
        assert!(d
            .register(
                &ada,
                "machine-0001",
                None,
                Some(&["bad entry".into()]),
                true
            )
            .is_err());
        assert!(d.register(&ada, "BAD", None, None, true).is_err());
    }

    #[test]
    fn confirmation_defaults_true_for_legacy_records_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("machines.json");
        let d = Directory::open(path.clone(), 4).unwrap();
        let ada = who("ada", None);
        let (unproven, _) = d.register(&ada, "machine-0001", None, None, false).unwrap();
        assert!(!unproven.confirmed);
        // Re-registering (a token rotation) leaves confirmation as it is.
        let (still_unproven, _) = d.register(&ada, "machine-0001", None, None, true).unwrap();
        assert!(!still_unproven.confirmed);
        let confirmed = d.confirm("machine-0001").unwrap();
        assert!(confirmed.confirmed);
        assert!(matches!(
            d.confirm("nope-0000"),
            Err(DirectoryError::NotFound)
        ));

        // A record written before this field existed deserializes as
        // confirmed (never retroactively flagged as new).
        let legacy = serde_json::json!([{
            "id": "machine-legacy",
            "name": "Old Mac",
            "owner": {"id": "ada"},
            "token_sha256": hash_token("t"),
        }]);
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let reopened = Directory::open(path, 4).unwrap();
        assert!(reopened.get("machine-legacy").unwrap().confirmed);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_write_leaves_the_directory_as_it_was() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let storage = std::sync::Arc::new(Storage::new(Some(state.clone()), Default::default()));
        let d = Directory::open_with(state.join("relay.json"), 4, storage.clone()).unwrap();
        let ada = who("ada", None);
        d.register(&ada, "machine-0001", Some("one"), None, true)
            .unwrap();
        let raw = std::fs::read_to_string(state.join("relay.json")).unwrap();
        assert!(!raw.contains('\n'), "compact JSON: {raw}");
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o500)).unwrap();
        let added = d.register(&ada, "machine-0002", Some("two"), None, true);
        let renamed = d.update(
            "machine-0001",
            &MachinePatch {
                name: Some("renamed".into()),
                ..Default::default()
            },
        );
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        if storage.failures(store::DIRECTORY) > 0 {
            assert!(matches!(added, Err(DirectoryError::Storage(_))));
            assert!(matches!(renamed, Err(DirectoryError::Storage(_))));
            assert!(
                d.get("machine-0002").is_none(),
                "not added in memory either"
            );
            assert_eq!(d.get("machine-0001").unwrap().name, "one");
            assert!(!d.writes_ok());
        }
        // Writable again: the change goes through; an unchanged one writes
        // nothing.
        d.update(
            "machine-0001",
            &MachinePatch {
                name: Some("renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(d.writes_ok());
        let writes = |s: &Storage| {
            s.metrics()
                .lines()
                .find(|l| l.starts_with("cua_relay_state_writes_total{store=\"directory\"}"))
                .unwrap()
                .to_owned()
        };
        let before = writes(&storage);
        d.set_host("machine-0001", None).unwrap();
        assert_eq!(writes(&storage), before);
    }
}
