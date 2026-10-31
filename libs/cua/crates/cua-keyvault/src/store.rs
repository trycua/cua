// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The on-disk vault.
//!
//! ```text
//! <dir>/                 0700
//!   vault.json           header: format, vault id, protector records (wrapped VMK only)
//!   meta.sealed          Meta, AEAD under HKDF(VMK, "meta"), AAD = vault id
//!   items/<id>.sealed    {rev, wrapped item key, payload}; AAD = vault | id | rev
//!   blobs/<sha256>.sealed  a big file's bytes, content-addressed; AAD = vault | hash
//!   audit.log            hash-chained JSONL (see `audit`)
//! ```
//!
//! Every file is written 0600 through a temporary file, `fsync` and rename.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::audit::{AuditEntry, AuditEvent, AuditLog, Verification};
use crate::crypto::{self, Sealed, SecretKey};
use crate::model::{ItemMeta, ItemPayload, Meta};
use crate::protector::{Protector, ProtectorRecord};
use crate::{Error, Result};

/// Header format version this build reads and writes.
///
/// Format 2 stores one sealed item per secret (cookie, localStorage value,
/// password or file) grouped by app. Format 1 held whole-app sessions; it
/// was only ever shipped in previews and is not migrated (see
/// [`Error::OldFormat`]).
pub const FORMAT: u32 = 2;
/// Largest item payload accepted (bundles above this are refused).
pub const MAX_ITEM_BYTES: usize = 256 * 1024 * 1024;

/// `vault.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// Format version.
    pub format: u32,
    /// Vault id.
    pub vault_id: String,
    /// Created, Unix ms.
    pub created_ms: u64,
    /// Protectors.
    pub protectors: Vec<ProtectorRecord>,
}

/// What [`Vault::upsert_items`] did with one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// A new item.
    Created,
    /// An existing item whose value changed.
    Updated,
    /// An existing item saved again with the same value (only its
    /// timestamp moved).
    Unchanged,
}

/// One saved item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upsert {
    /// The item id (existing items keep theirs).
    pub id: String,
    /// What happened.
    pub outcome: UpsertOutcome,
}

#[derive(Serialize, Deserialize)]
struct ItemRecord {
    id: String,
    rev: u64,
    wrapped_key: Sealed,
    payload: Sealed,
}

struct Keys {
    vmk: SecretKey,
    meta: SecretKey,
    wrap: SecretKey,
    audit: SecretKey,
    blob: SecretKey,
}

impl Keys {
    fn from_vmk(vmk: SecretKey) -> Self {
        Self {
            meta: vmk.derive(b"cua-keyvault/v1/meta"),
            wrap: vmk.derive(b"cua-keyvault/v1/item-wrap"),
            audit: vmk.derive(b"cua-keyvault/v1/audit"),
            blob: vmk.derive(b"cua-keyvault/v2/blob"),
            vmk,
        }
    }
}

/// A vault on disk; locked until [`Vault::unlock`].
pub struct Vault {
    dir: PathBuf,
    header: Header,
    audit: AuditLog,
    keys: Option<Keys>,
    meta: Option<Meta>,
    /// Monotonic generation anchor kept outside the sealed files, to detect a
    /// whole-vault rollback (red-team F5). `None` disables the check.
    anchor: Option<Box<dyn crate::rollback::GenerationAnchor>>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("dir", &self.dir)
            .field("vault_id", &self.header.vault_id)
            .field("unlocked", &self.keys.is_some())
            .finish_non_exhaustive()
    }
}

/// Writes `bytes` to `path` with mode 0600, atomically (tmp, fsync, rename).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Invalid(format!("{} has no parent", path.display())))?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("f"),
        crypto::random_id()?
    ));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Creates `dir` (and parents) and forces it to 0700.
pub fn private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn meta_aad(vault_id: &str) -> Vec<u8> {
    format!("cua-keyvault/v1/meta|{vault_id}").into_bytes()
}

fn item_aad(vault_id: &str, id: &str, rev: u64, part: &str) -> Vec<u8> {
    format!("cua-keyvault/v1/item-{part}|{vault_id}|{id}|{rev}").into_bytes()
}

fn blob_aad(vault_id: &str, hash: &str) -> Vec<u8> {
    format!("cua-keyvault/v2/blob|{vault_id}|{hash}").into_bytes()
}

fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

impl Vault {
    /// Whether a vault exists in `dir`.
    pub fn exists(dir: &Path) -> bool {
        dir.join("vault.json").is_file()
    }

    /// Creates a vault with the given protectors (at least one) and returns
    /// it unlocked.
    pub fn create(dir: &Path, protectors: &[&dyn Protector]) -> Result<Vault> {
        if protectors.is_empty() {
            return Err(Error::Invalid(
                "a vault needs at least one protector".into(),
            ));
        }
        if Self::exists(dir) {
            return Err(Error::Invalid(format!(
                "a vault already exists in {}",
                dir.display()
            )));
        }
        private_dir(dir)?;
        private_dir(&dir.join("items"))?;
        let vault_id = crypto::random_id()?;
        let vmk = SecretKey::generate()?;
        let mut records = Vec::new();
        for p in protectors {
            records.push(p.enroll(&vault_id, &vmk)?);
        }
        let header = Header {
            format: FORMAT,
            vault_id,
            created_ms: crate::now_ms(),
            protectors: records,
        };
        write_private(
            &dir.join("vault.json"),
            &serde_json::to_vec_pretty(&header)?,
        )?;
        let audit = AuditLog::open(dir.join("audit.log"))?;
        let mut v = Vault {
            dir: dir.to_path_buf(),
            header,
            audit,
            keys: Some(Keys::from_vmk(vmk)),
            meta: Some(Meta::default()),
            anchor: None,
        };
        v.save_meta()?;
        Ok(v)
    }

    /// Opens a vault, locked.
    pub fn open(dir: &Path) -> Result<Vault> {
        let raw = std::fs::read(dir.join("vault.json")).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NoVault(format!("no Keyvault at {}", dir.display()))
            } else {
                e.into()
            }
        })?;
        let header: Header =
            serde_json::from_slice(&raw).map_err(|e| Error::Corrupt(format!("vault.json: {e}")))?;
        if header.format < FORMAT {
            return Err(Error::OldFormat(header.format));
        }
        if header.format != FORMAT {
            return Err(Error::Corrupt(format!(
                "vault format {} is not supported by this build (expects {FORMAT})",
                header.format
            )));
        }
        let audit = AuditLog::open(dir.join("audit.log"))?;
        Ok(Vault {
            dir: dir.to_path_buf(),
            header,
            audit,
            keys: None,
            meta: None,
            anchor: None,
        })
    }

    /// Attaches a monotonic generation anchor kept outside the sealed files, so
    /// a whole-vault rollback is refused at unlock (red-team F5). Attach it
    /// before unlocking. When the vault is already unlocked (just created), the
    /// current generation is recorded immediately.
    pub fn with_anchor(
        mut self,
        anchor: Box<dyn crate::rollback::GenerationAnchor>,
    ) -> Result<Self> {
        if let Some(meta) = &self.meta {
            anchor.record(&self.header.vault_id, meta.generation)?;
        }
        self.anchor = Some(anchor);
        Ok(self)
    }

    /// The header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The vault directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the vault is unlocked.
    pub fn is_unlocked(&self) -> bool {
        self.keys.is_some()
    }

    /// Unlocks with `protector`, trying every record of its kind.
    pub fn unlock(&mut self, protector: &dyn Protector) -> Result<()> {
        let mut last = Error::WrongCredential;
        for rec in self
            .header
            .protectors
            .iter()
            .filter(|r| r.kind == protector.kind())
        {
            match protector.unwrap(&self.header.vault_id, rec) {
                Ok(vmk) => {
                    let keys = Keys::from_vmk(vmk);
                    let meta = self.load_meta(&keys)?;
                    // Anti-rollback: refuse state older than the external
                    // monotonic anchor (red-team F5). Fail closed.
                    if let Some(anchor) = &self.anchor {
                        let last = anchor.last(&self.header.vault_id)?;
                        if meta.generation < last {
                            return Err(Error::Corrupt(format!(
                                "the Keyvault was rolled back (its generation {} is below the \
                                 recorded {last}); refusing to load stale state",
                                meta.generation
                            )));
                        }
                        anchor.record(&self.header.vault_id, meta.generation)?;
                    }
                    self.keys = Some(keys);
                    self.meta = Some(meta);
                    if let Err(e) = self.settle_staged_records() {
                        self.lock();
                        return Err(e);
                    }
                    return Ok(());
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Forgets every key (they are zeroized on drop).
    pub fn lock(&mut self) {
        self.keys = None;
        self.meta = None;
    }

    fn keys(&self) -> Result<&Keys> {
        self.keys.as_ref().ok_or(Error::Locked)
    }

    fn load_meta(&self, keys: &Keys) -> Result<Meta> {
        let raw = std::fs::read(self.dir.join("meta.sealed"))?;
        let sealed: Sealed = serde_json::from_slice(&raw)
            .map_err(|e| Error::Corrupt(format!("meta.sealed: {e}")))?;
        let plain = crypto::open(&keys.meta, &meta_aad(&self.header.vault_id), &sealed)
            .map_err(|_| Error::Corrupt("meta.sealed does not authenticate".into()))?;
        serde_json::from_slice(&plain).map_err(|e| Error::Corrupt(format!("meta: {e}")))
    }

    fn save_meta(&mut self) -> Result<()> {
        let next = self.meta()?.clone();
        let generation = self.write_meta(next)?;
        self.record_generation(generation)
    }

    /// Seals `next` as the following generation and adopts it once the write
    /// lands. On error nothing was committed and the loaded metadata is kept.
    fn write_meta(&mut self, mut next: Meta) -> Result<u64> {
        next.generation += 1;
        next.audit_head = self.audit.head().clone();
        let plain = Zeroizing::new(serde_json::to_vec(&next)?);
        let keys = self.keys.as_ref().ok_or(Error::Locked)?;
        let sealed = crypto::seal(&keys.meta, &meta_aad(&self.header.vault_id), &plain)?;
        write_private(&self.dir.join("meta.sealed"), &serde_json::to_vec(&sealed)?)?;
        let generation = next.generation;
        self.meta = Some(next);
        Ok(generation)
    }

    /// Advances the external anti-rollback high-water mark after a metadata
    /// write lands (red-team F5).
    fn record_generation(&self, generation: u64) -> Result<()> {
        if let Some(anchor) = &self.anchor {
            anchor.record(&self.header.vault_id, generation)?;
        }
        Ok(())
    }

    /// The metadata (unlocked only).
    pub fn meta(&self) -> Result<&Meta> {
        self.meta.as_ref().ok_or(Error::Locked)
    }

    /// Mutates and persists the metadata.
    pub fn update_meta<T>(&mut self, f: impl FnOnce(&mut Meta) -> Result<T>) -> Result<T> {
        let mut draft = self.meta()?.clone();
        let out = f(&mut draft)?;
        let generation = self.write_meta(draft)?;
        self.record_generation(generation)?;
        Ok(out)
    }

    /// Adds a protector (unlocked only).
    pub fn add_protector(&mut self, protector: &dyn Protector) -> Result<ProtectorRecord> {
        let rec = protector.enroll(&self.header.vault_id, &self.keys()?.vmk)?;
        self.header.protectors.push(rec.clone());
        write_private(
            &self.dir.join("vault.json"),
            &serde_json::to_vec_pretty(&self.header)?,
        )?;
        Ok(rec)
    }

    /// Removes a protector record (never the last one).
    pub fn remove_protector(&mut self, id: &str) -> Result<ProtectorRecord> {
        self.keys()?;
        if self.header.protectors.len() <= 1 {
            return Err(Error::Invalid("cannot remove the last protector".into()));
        }
        let idx = self
            .header
            .protectors
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| Error::NotFound(format!("protector {id}")))?;
        let rec = self.header.protectors.remove(idx);
        write_private(
            &self.dir.join("vault.json"),
            &serde_json::to_vec_pretty(&self.header)?,
        )?;
        Ok(rec)
    }

    fn item_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(Error::Invalid(format!("bad item id {id:?}")));
        }
        Ok(self.dir.join("items").join(format!("{id}.sealed")))
    }

    fn blob_path(&self, hash: &str) -> Result<PathBuf> {
        if !valid_hash(hash) {
            return Err(Error::Invalid(format!("bad blob hash {hash:?}")));
        }
        Ok(self.dir.join("blobs").join(format!("{hash}.sealed")))
    }

    /// Seals `bytes` as a content-addressed blob and returns its SHA-256
    /// (hex). The same bytes are stored once, however many items name them.
    pub fn put_blob(&self, bytes: &[u8]) -> Result<String> {
        let keys = self.keys()?;
        if bytes.len() > MAX_ITEM_BYTES {
            return Err(Error::Invalid(format!(
                "a file of {} bytes is over the {MAX_ITEM_BYTES} byte limit",
                bytes.len()
            )));
        }
        let hash = hex::encode(crypto::sha256(bytes));
        let path = self.blob_path(&hash)?;
        if path.exists() {
            return Ok(hash);
        }
        private_dir(&self.dir.join("blobs"))?;
        let sealed = crypto::seal(&keys.blob, &blob_aad(&self.header.vault_id, &hash), bytes)?;
        write_private(&path, &serde_json::to_vec(&sealed)?)?;
        Ok(hash)
    }

    /// Reads a blob, checking it against its own name.
    pub fn read_blob(&self, hash: &str) -> Result<Zeroizing<Vec<u8>>> {
        let keys = self.keys()?;
        let raw = std::fs::read(self.blob_path(hash)?)?;
        let sealed: Sealed = serde_json::from_slice(&raw)
            .map_err(|e| Error::Corrupt(format!("blob {hash}: {e}")))?;
        let plain = crypto::open(&keys.blob, &blob_aad(&self.header.vault_id, hash), &sealed)
            .map_err(|_| Error::Corrupt(format!("blob {hash} does not authenticate")))?;
        if !crypto::ct_eq(
            hex::encode(crypto::sha256(&plain)).as_bytes(),
            hash.as_bytes(),
        ) {
            return Err(Error::Corrupt(format!(
                "blob {hash} does not match its name (swapped)"
            )));
        }
        Ok(plain)
    }

    /// A payload with a big file's bytes put back inline, for delivery.
    pub fn inline_blobs(&self, payload: &ItemPayload) -> Result<ItemPayload> {
        use base64::Engine as _;
        if payload.schema != crate::record::FILE_V1 {
            return Ok(payload.clone());
        }
        let mut f: crate::record::FileRecord = serde_json::from_str(&payload.record)
            .map_err(|_| Error::Corrupt("a file record is not valid".into()))?;
        if let Some(hash) = f.blob.take() {
            let bytes = self.read_blob(&hash)?;
            f.content = Some(base64::engine::general_purpose::STANDARD.encode(bytes.as_slice()));
        }
        Ok(payload.with_record(serde_json::to_string(&f)?))
    }

    /// Moves a big file's bytes out of its record into a blob (the record
    /// then names the hash); anything else is stored as it is.
    fn externalize(&self, meta: &mut ItemMeta, payload: ItemPayload) -> Result<ItemPayload> {
        use base64::Engine as _;
        meta.blob = None;
        if payload.schema != crate::record::FILE_V1 {
            return Ok(payload);
        }
        let mut f: crate::record::FileRecord = serde_json::from_str(&payload.record)
            .map_err(|_| Error::Invalid("a file record is not valid".into()))?;
        let Some(content) = f.content.clone() else {
            meta.blob = f.blob.clone();
            return Ok(payload);
        };
        let bytes = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(content)
                .map_err(|_| Error::Invalid("a file record is not base64".into()))?,
        );
        if bytes.len() <= crate::record::INLINE_FILE_LIMIT {
            return Ok(payload);
        }
        let hash = self.put_blob(&bytes)?;
        f.content = None;
        f.blob = Some(hash.clone());
        meta.blob = Some(hash);
        Ok(payload.with_record(serde_json::to_string(&f)?))
    }

    /// Deletes blobs no item names any more.
    fn gc_blobs(&self) -> Result<()> {
        let dir = self.dir.join("blobs");
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Ok(());
        };
        let live: std::collections::HashSet<&str> = self
            .meta()?
            .items
            .values()
            .filter_map(|i| i.blob.as_deref())
            .collect();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(hash) = name.strip_suffix(".sealed") else {
                continue;
            };
            if !live.contains(hash) {
                let len = e.metadata().map(|m| m.len()).unwrap_or(0) as usize;
                let _ = std::fs::write(e.path(), vec![0u8; len]);
                let _ = std::fs::remove_file(e.path());
            }
        }
        Ok(())
    }

    fn write_item(&self, id: &str, rev: u64, payload: &ItemPayload) -> Result<String> {
        let keys = self.keys()?;
        let vid = &self.header.vault_id;
        let plain = Zeroizing::new(serde_json::to_vec(payload)?);
        if plain.len() > MAX_ITEM_BYTES {
            return Err(Error::Invalid(format!(
                "item payload is {} bytes; the limit is {MAX_ITEM_BYTES}",
                plain.len()
            )));
        }
        let item_key = SecretKey::generate()?;
        let record = ItemRecord {
            id: id.to_string(),
            rev,
            wrapped_key: crypto::seal(
                &keys.wrap,
                &item_aad(vid, id, rev, "key"),
                item_key.expose(),
            )?,
            payload: crypto::seal(&item_key, &item_aad(vid, id, rev, "payload"), &plain)?,
        };
        let bytes = serde_json::to_vec(&record)?;
        let digest = hex::encode(crypto::sha256(&bytes));
        write_private(&self.item_path(id)?, &bytes)?;
        Ok(digest)
    }

    /// Saves one item (see [`Vault::upsert_items`]) and returns its id.
    pub fn put_item(&mut self, meta: ItemMeta, payload: &ItemPayload) -> Result<String> {
        let mut out = self.upsert_items(vec![(meta, payload.clone())])?;
        Ok(out.remove(0).id)
    }

    /// Saves a batch of items, upserting by key: an item whose app and
    /// [`ItemMeta::unique_key`] already exist is updated in place (same id,
    /// same lock state and policy, same creation time), never duplicated.
    /// Later duplicates inside the batch win. The whole batch is one
    /// metadata write. `updated_ms` is bumped for every item saved again;
    /// the sealed record is rewritten only when its value changed.
    pub fn upsert_items(&mut self, items: Vec<(ItemMeta, ItemPayload)>) -> Result<Vec<Upsert>> {
        let now = crate::now_ms();
        self.settle_staged_records()?;
        // (provider, unique key) -> id, for what the vault already holds.
        let mut index: std::collections::HashMap<(String, String), String> = self
            .meta()?
            .items
            .values()
            .map(|i| ((i.provider_id.clone(), i.unique_key()), i.id.clone()))
            .collect();
        // Later duplicates win: keep the last of each key, in first-seen order.
        let mut order: Vec<(String, String)> = Vec::new();
        let mut last: std::collections::HashMap<(String, String), (ItemMeta, ItemPayload)> =
            std::collections::HashMap::new();
        for (m, p) in items {
            let k = (m.provider_id.clone(), m.unique_key());
            if !last.contains_key(&k) {
                order.push(k.clone());
            }
            last.insert(k, (m, p));
        }
        enum Change {
            New(Box<ItemMeta>),
            Saved {
                id: String,
                meta: Box<ItemMeta>,
                written: Option<(u64, String)>,
            },
        }
        let mut changes = Vec::new();
        let mut created_files: Vec<String> = Vec::new();
        // Sealed records this batch overwrote, restored if it never commits.
        let mut overwritten: Vec<(String, Backup)> = Vec::new();
        let mut out = Vec::new();
        let fail = |this: &Self, files: &[String], old: &[(String, Backup)], e: Error| -> Error {
            for id in files {
                if let Ok(p) = this.item_path(id) {
                    let _ = std::fs::remove_file(p);
                }
            }
            let mut unrestored = Vec::new();
            for (id, backup) in old {
                if let Err(restore) = this.item_path(id).and_then(|p| backup.restore(&p)) {
                    unrestored.push(format!("{id} ({restore})"));
                }
            }
            if unrestored.is_empty() {
                e
            } else {
                Error::Corrupt(format!(
                    "{e}; restoring overwritten items also failed: {}",
                    unrestored.join(", ")
                ))
            }
        };
        for k in order {
            let (mut meta, payload) = last.remove(&k).expect("keyed above");
            meta.updated_ms = now;
            let payload = match self.externalize(&mut meta, payload) {
                Ok(p) => p,
                Err(e) => return Err(fail(self, &created_files, &overwritten, e)),
            };
            match index.get(&k).cloned() {
                Some(id) => {
                    let cur = self.meta()?.items[&id].clone();
                    let same = self
                        .read_payload(&id)
                        .map(|p| p == payload)
                        .unwrap_or(false);
                    let written = if same {
                        None
                    } else {
                        let rev = cur.rev + 1;
                        match self.item_path(&id).and_then(|p| Backup::take(&p)) {
                            Ok(backup) => overwritten.push((id.clone(), backup)),
                            Err(e) => return Err(fail(self, &created_files, &overwritten, e)),
                        }
                        match self.write_item(&id, rev, &payload) {
                            Ok(d) => Some((rev, d)),
                            Err(e) => return Err(fail(self, &created_files, &overwritten, e)),
                        }
                    };
                    out.push(Upsert {
                        id: id.clone(),
                        outcome: if same {
                            UpsertOutcome::Unchanged
                        } else {
                            UpsertOutcome::Updated
                        },
                    });
                    changes.push(Change::Saved {
                        id,
                        meta: Box::new(meta),
                        written,
                    });
                }
                None => {
                    let id = crypto::random_id()?;
                    let digest = match self.write_item(&id, 1, &payload) {
                        Ok(d) => d,
                        Err(e) => return Err(fail(self, &created_files, &overwritten, e)),
                    };
                    created_files.push(id.clone());
                    meta.id = id.clone();
                    meta.rev = 1;
                    meta.record_digest = digest;
                    meta.created_ms = now;
                    index.insert(k, id.clone());
                    out.push(Upsert {
                        id,
                        outcome: UpsertOutcome::Created,
                    });
                    changes.push(Change::New(Box::new(meta)));
                }
            }
        }
        let mut draft = self.meta()?.clone();
        let res = (|m: &mut Meta| -> Result<()> {
            for c in changes {
                match c {
                    Change::New(meta) => {
                        m.items.insert(meta.id.clone(), *meta);
                    }
                    Change::Saved { id, meta, written } => {
                        let it = m
                            .items
                            .get_mut(&id)
                            .ok_or_else(|| Error::NotFound(format!("item {id}")))?;
                        // Facts about the secret refresh; the user's choices
                        // (the lock, targets, lifetime) and identity stay.
                        it.updated_ms = meta.updated_ms;
                        it.session = meta.session;
                        it.expires_ms = meta.expires_ms;
                        it.bytes = meta.bytes;
                        it.blob = meta.blob.clone();
                        it.path = meta.path.clone();
                        it.app_display = meta.app_display;
                        it.source = meta.source;
                        if let Some((rev, digest)) = written {
                            it.rev = rev;
                            it.record_digest = digest;
                        }
                    }
                }
            }
            Ok(())
        })(&mut draft)
        .and_then(|()| self.write_meta(draft));
        let generation = match res {
            Ok(generation) => generation,
            Err(e) => return Err(fail(self, &created_files, &overwritten, e)),
        };
        for (_, backup) in &overwritten {
            backup.discard();
        }
        // The committed index now references every written payload, so a
        // later failure must not delete them.
        self.record_generation(generation)?;
        self.gc_blobs()?;
        Ok(out)
    }

    /// Settles records an interrupted upsert staged (`.<id>.sealed.<random>.prev`).
    /// A staged record the sealed index still references replaces an uncommitted
    /// one; every other staged record is removed, so a replaced or deleted item's
    /// old record does not outlive it.
    fn settle_staged_records(&self) -> Result<()> {
        let meta = self.meta()?;
        let entries = match std::fs::read_dir(self.dir.join("items")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let holds = |bytes: &[u8], digest: &str| {
            crypto::ct_eq(
                hex::encode(crypto::sha256(bytes)).as_bytes(),
                digest.as_bytes(),
            )
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with('.') && name.ends_with(".prev")) {
                continue;
            }
            let staged = entry.path();
            let indexed = name[1..]
                .split_once(".sealed.")
                .and_then(|(id, _)| Some((self.item_path(id).ok()?, meta.items.get(id)?)));
            // A staged record the index still references must be read before it
            // can be discarded; restore it unless the current record is the
            // committed one.
            if let Some((path, item)) = indexed
                && holds(
                    &std::fs::read(&staged).map_err(|e| {
                        Error::Corrupt(format!(
                            "could not read the staged record {}: {e}",
                            staged.display()
                        ))
                    })?,
                    &item.record_digest,
                )
                && !std::fs::read(&path).is_ok_and(|bytes| holds(&bytes, &item.record_digest))
            {
                // Keep the staged record (the only one the index trusts) until
                // it is back in place.
                std::fs::rename(&staged, &path).map_err(|e| {
                    Error::Corrupt(format!(
                        "could not restore {} from {}: {e}",
                        path.display(),
                        staged.display()
                    ))
                })?;
            }
            match std::fs::remove_file(&staged) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(Error::Corrupt(format!(
                        "could not remove the staged record {}: {e}",
                        staged.display()
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Decrypts an item's payload after checking its record against the
    /// sealed index (digest and revision).
    pub fn read_payload(&self, id: &str) -> Result<ItemPayload> {
        let keys = self.keys()?;
        let meta = self
            .meta()?
            .items
            .get(id)
            .ok_or_else(|| Error::NotFound(format!("item {id}")))?;
        let bytes = std::fs::read(self.item_path(id)?)?;
        if !crypto::ct_eq(
            hex::encode(crypto::sha256(&bytes)).as_bytes(),
            meta.record_digest.as_bytes(),
        ) {
            return Err(Error::Corrupt(format!(
                "item {id} does not match the sealed index (swapped or rolled back)"
            )));
        }
        let rec: ItemRecord = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Corrupt(format!("item {id}: {e}")))?;
        if rec.id != id || rec.rev != meta.rev {
            return Err(Error::Corrupt(format!(
                "item {id} has the wrong id or revision"
            )));
        }
        let vid = &self.header.vault_id;
        let raw_key = crypto::open(
            &keys.wrap,
            &item_aad(vid, id, rec.rev, "key"),
            &rec.wrapped_key,
        )?;
        let item_key = SecretKey::from_bytes(&raw_key)?;
        let plain = crypto::open(
            &item_key,
            &item_aad(vid, id, rec.rev, "payload"),
            &rec.payload,
        )?;
        serde_json::from_slice(&plain)
            .map_err(|e| Error::Corrupt(format!("item {id} payload: {e}")))
    }

    /// Remembers site icons (site to base64 PNG), one metadata write. The
    /// map stays bounded.
    pub fn set_favicons(&mut self, icons: Vec<(String, String)>) -> Result<()> {
        const MAX: usize = 1024;
        if icons.is_empty() {
            return Ok(());
        }
        self.update_meta(|m| {
            for (site, png) in icons {
                if m.favicons.len() < MAX || m.favicons.contains_key(&site) {
                    m.favicons.insert(site, png);
                }
            }
            Ok(())
        })
    }

    /// Deletes an item (crypto-shredded: the record, with its wrapped key,
    /// is removed) and drops it from grants and rules.
    pub fn delete_item(&mut self, id: &str) -> Result<ItemMeta> {
        self.delete_items(&[id.to_string()])
            .map(|mut v| v.remove(0))
    }

    /// Deletes several items with one metadata write. Fails, deleting
    /// nothing, when any id is unknown.
    pub fn delete_items(&mut self, ids: &[String]) -> Result<Vec<ItemMeta>> {
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<String> = ids
            .iter()
            .filter(|i| seen.insert(i.as_str()))
            .cloned()
            .collect();
        let ids = ids.as_slice();
        let paths = ids
            .iter()
            .map(|i| self.item_path(i))
            .collect::<Result<Vec<_>>>()?;
        let removed = self.update_meta(|m| {
            let mut removed = Vec::new();
            for id in ids {
                let it = m
                    .items
                    .remove(id)
                    .ok_or_else(|| Error::NotFound(format!("item {id}")))?;
                removed.push(it);
            }
            for g in &mut m.grants {
                g.items.retain(|i| !ids.contains(i));
                if g.items.is_empty() {
                    g.revoked = true;
                }
            }
            for r in &mut m.rules {
                r.items.retain(|i| !ids.contains(i));
            }
            m.rules.retain(|r| !r.items.is_empty());
            // An icon goes with the last item of its site.
            let live: std::collections::HashSet<String> = m
                .items
                .values()
                .filter_map(|i| i.domain.as_deref().map(crate::record::site_of))
                .collect();
            m.favicons.retain(|site, _| live.contains(site));
            Ok(removed)
        })?;
        for path in paths {
            if path.exists() {
                // Overwrite before unlinking (best effort on copy-on-write
                // file systems; the wrapped key is what makes this a shred).
                let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as usize;
                let _ = std::fs::write(&path, vec![0u8; len]);
                std::fs::remove_file(&path)?;
            }
        }
        // An interrupted upsert's staged record would outlive the delete.
        self.settle_staged_records()?;
        self.gc_blobs()?;
        Ok(removed)
    }

    /// Appends an audit event (MAC'd when unlocked) and re-anchors the head
    /// in the sealed metadata.
    pub fn audit(&mut self, event: AuditEvent) -> Result<AuditEntry> {
        let key = self.keys.as_ref().map(|k| &k.audit);
        let entry = self.audit.append(event, key)?;
        if self.keys.is_some() {
            self.save_meta()?;
        }
        Ok(entry)
    }

    /// Recent audit entries.
    pub fn audit_tail(&self, limit: usize) -> Result<Vec<AuditEntry>> {
        self.audit.tail(limit)
    }

    /// Verifies the audit chain against the key and the anchored head.
    pub fn verify_audit(&self) -> Result<Verification> {
        let keys = self.keys()?;
        let anchor = self.meta()?.audit_head.clone();
        self.audit.verify(Some(&keys.audit), Some(&anchor))
    }
}

/// The record an upsert replaces, kept on disk under a staged name until the
/// batch commits, so a crash before the commit can still restore it. A hard
/// link keeps it without copying, so restoring needs no free space.
enum Backup {
    /// No record was on disk (an already-missing item); restoring removes the new one.
    Absent,
    Staged(PathBuf),
}

impl Backup {
    fn take(path: &Path) -> Result<Self> {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("item");
        let staged = path.with_file_name(format!(".{name}.{}.prev", crypto::random_id()?));
        match std::fs::hard_link(path, &staged) {
            Ok(()) => Ok(Backup::Staged(staged)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Backup::Absent),
            // A filesystem without hard links: stage a durable copy, or refuse
            // the replacement when even that fails.
            Err(_) => match std::fs::read(path) {
                Ok(bytes) => {
                    write_private(&staged, &bytes)?;
                    Ok(Backup::Staged(staged))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Backup::Absent),
                Err(e) => Err(e.into()),
            },
        }
    }

    fn restore(&self, path: &Path) -> Result<()> {
        match self {
            Backup::Absent => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
            Backup::Staged(staged) => {
                std::fs::rename(staged, path)?;
                // Renaming a link onto another name of the same file is a
                // no-op that leaves both names (the write failed before
                // replacing `path`), so drop the staged name explicitly.
                match std::fs::remove_file(staged) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                    _ => Ok(()),
                }
            }
        }
    }

    fn discard(&self) {
        if let Backup::Staged(staged) = self {
            let _ = std::fs::remove_file(staged);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::KdfParams;
    use crate::model::{ItemKind, ItemPolicy};
    use crate::protector::{PassphraseProtector, RecoveryKey, RecoveryProtector};

    pub(crate) fn pp() -> PassphraseProtector {
        PassphraseProtector::with_params("correct horse battery", KdfParams::for_tests().unwrap())
    }

    pub(crate) fn sample_meta() -> ItemMeta {
        let mut m = ItemMeta::draft(
            ItemKind::Cookie,
            "chrome",
            "Google Chrome",
            Some("github.com"),
            "user_session",
        );
        m.source = "Default".into();
        m.policy = ItemPolicy::default();
        m
    }

    /// A cookie payload whose value is `fixture-<tag>`.
    pub(crate) fn sample_payload(tag: &str) -> ItemPayload {
        ItemPayload {
            provider_id: "chrome".into(),
            scope: "full".into(),
            schema: crate::record::COOKIE_V1.into(),
            record: format!(r#"{{"fixture":"{tag}"}}"#),
        }
    }

    #[test]
    fn create_unlock_items_and_persistence() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let rk = RecoveryKey::generate().unwrap();
        let mut v = Vault::create(&dir, &[&pp(), &RecoveryProtector(rk.clone())]).unwrap();
        let id = v.put_item(sample_meta(), &sample_payload("a")).unwrap();
        assert_eq!(v.read_payload(&id).unwrap(), sample_payload("a"));
        v.put_item(sample_meta(), &sample_payload("b")).unwrap();
        assert_eq!(v.meta().unwrap().items[&id].rev, 2);
        drop(v);

        let mut v = Vault::open(&dir).unwrap();
        assert!(!v.is_unlocked());
        assert!(matches!(v.read_payload(&id), Err(Error::Locked)));
        assert!(matches!(
            v.unlock(&PassphraseProtector::new("wrong passphrase!")),
            Err(Error::WrongCredential)
        ));
        v.unlock(&RecoveryProtector(RecoveryKey::parse(rk.reveal()).unwrap()))
            .unwrap();
        assert_eq!(v.read_payload(&id).unwrap(), sample_payload("b"));
        v.lock();
        v.unlock(&pp()).unwrap();
        let removed = v.delete_item(&id).unwrap();
        assert_eq!(removed.domain.as_deref(), Some("github.com"));
        assert!(!dir.join("items").join(format!("{id}.sealed")).exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&dir.join("vault.json")), 0o600);
            assert_eq!(mode(&dir.join("meta.sealed")), 0o600);
        }
    }

    #[test]
    fn no_plaintext_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        v.put_item(sample_meta(), &sample_payload("PLAINTEXT-MARKER"))
            .unwrap();
        for entry in walk(&dir) {
            let bytes = std::fs::read(&entry).unwrap();
            let hay = String::from_utf8_lossy(&bytes);
            assert!(
                !hay.contains("github.com"),
                "{} leaks metadata",
                entry.display()
            );
            assert!(
                !hay.contains("fixture-PLAINTEXT"),
                "{} leaks payload",
                entry.display()
            );
            assert!(
                !hay.contains(&crypto::b64_encode(b"fixture-PLAINTEXT-MARKER")),
                "{} leaks payload",
                entry.display()
            );
        }
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                out.extend(walk(&e.path()));
            } else {
                out.push(e.path());
            }
        }
        out
    }

    #[test]
    fn swapped_or_rolled_back_items_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        let a = v.put_item(sample_meta(), &sample_payload("a")).unwrap();
        let mut other = sample_meta();
        other.key = "other".into();
        let b = v.put_item(other, &sample_payload("b")).unwrap();
        let pa = dir.join("items").join(format!("{a}.sealed"));
        let pb = dir.join("items").join(format!("{b}.sealed"));
        let old_a = std::fs::read(&pa).unwrap();
        v.put_item(sample_meta(), &sample_payload("a2")).unwrap();
        // Rollback of one item to its previous revision.
        std::fs::write(&pa, &old_a).unwrap();
        assert!(matches!(v.read_payload(&a), Err(Error::Corrupt(_))));
        // Swap b's record into a's slot.
        std::fs::copy(&pb, &pa).unwrap();
        assert!(matches!(v.read_payload(&a), Err(Error::Corrupt(_))));
        // Tampered metadata does not unlock.
        let meta = dir.join("meta.sealed");
        let mut s: Sealed = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
        let mut ct = crypto::b64_decode(&s.ct).unwrap();
        ct[3] ^= 0x40;
        s.ct = crypto::b64_encode(&ct);
        std::fs::write(&meta, serde_json::to_vec(&s).unwrap()).unwrap();
        let mut v2 = Vault::open(&dir).unwrap();
        assert!(matches!(v2.unlock(&pp()), Err(Error::Corrupt(_))));
    }

    #[test]
    fn audit_head_is_anchored_in_the_sealed_meta() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        for i in 0..3 {
            v.audit(AuditEvent {
                kind: format!("test.{i}"),
                actor: "t".into(),
                caller_fp: "fp".into(),
                item: None,
                target: None,
                decision: "ok".into(),
                detail: String::new(),
            })
            .unwrap();
        }
        assert!(v.verify_audit().unwrap().ok());
        // Truncate the log behind the vault's back.
        let log = dir.join("audit.log");
        let text = std::fs::read_to_string(&log).unwrap();
        let first: String = text.lines().take(1).map(|l| format!("{l}\n")).collect();
        std::fs::write(&log, first).unwrap();
        let mut v = Vault::open(&dir).unwrap();
        v.unlock(&pp()).unwrap();
        let ver = v.verify_audit().unwrap();
        assert!(!ver.ok());
    }

    #[test]
    fn whole_vault_rollback_is_detected_by_the_external_anchor() {
        use crate::rollback::FileGenerationAnchor;
        // The anchor lives outside the vault directory (a login-keychain item
        // in production), so restoring the directory cannot roll it back.
        let ext = tempfile::tempdir().unwrap();
        let anchor_path = ext.path().join("gen.json");
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");

        let mut v = Vault::create(&dir, &[&pp()])
            .unwrap()
            .with_anchor(Box::new(FileGenerationAnchor::new(&anchor_path)))
            .unwrap();
        let id = v.put_item(sample_meta(), &sample_payload("a")).unwrap();
        // Snapshot the sealed set at this (older) generation.
        let meta_snap = std::fs::read(dir.join("meta.sealed")).unwrap();
        let item_path = dir.join("items").join(format!("{id}.sealed"));
        let item_snap = std::fs::read(&item_path).unwrap();
        // Move the vault forward: trip the kill switch and rotate the payload.
        v.put_item(sample_meta(), &sample_payload("b")).unwrap();
        v.update_meta(|m| {
            m.settings.disabled = true;
            Ok(())
        })
        .unwrap();
        drop(v);

        // The attacker restores the older sealed set (kill switch off again,
        // old payload). Decryption still succeeds, but the generation regressed.
        std::fs::write(dir.join("meta.sealed"), &meta_snap).unwrap();
        std::fs::write(&item_path, &item_snap).unwrap();

        let mut rolled = Vault::open(&dir)
            .unwrap()
            .with_anchor(Box::new(FileGenerationAnchor::new(&anchor_path)))
            .unwrap();
        let err = rolled.unlock(&pp());
        assert!(
            matches!(err, Err(Error::Corrupt(_))),
            "rollback must fail closed: {err:?}"
        );
        assert!(err.unwrap_err().to_string().contains("rolled back"));

        // Without the external anchor, the same rollback loads cleanly and the
        // kill switch is silently un-tripped: this is exactly what the anchor
        // exists to catch.
        let mut unguarded = Vault::open(&dir).unwrap();
        unguarded.unlock(&pp()).unwrap();
        assert!(!unguarded.meta().unwrap().settings.disabled);
    }

    #[test]
    fn upsert_keeps_committed_payloads_when_the_anchor_write_fails() {
        use crate::rollback::FileGenerationAnchor;
        let ext = tempfile::tempdir().unwrap();
        let anchor_dir = ext.path().join("anchor");
        std::fs::create_dir(&anchor_dir).unwrap();
        let anchor_path = anchor_dir.join("gen.json");
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()])
            .unwrap()
            .with_anchor(Box::new(FileGenerationAnchor::new(&anchor_path)))
            .unwrap();
        let existing = sample_meta();
        let seed = v
            .upsert_items(vec![(existing.clone(), sample_payload("a"))])
            .unwrap();
        let e = seed[0].id.clone();
        let generation = v.meta().unwrap().generation;

        // The anchor write fails after the sealed index lands.
        std::fs::remove_dir_all(&anchor_dir).unwrap();
        let mut fresh = ItemMeta::draft(
            ItemKind::Cookie,
            "chrome",
            "Google Chrome",
            Some("github.com"),
            "_gh_sess",
        );
        fresh.source = "Default".into();
        let err = v
            .upsert_items(vec![
                (existing, sample_payload("b")),
                (fresh, sample_payload("n")),
            ])
            .unwrap_err();
        assert!(matches!(err, Error::Io(_)), "{err:?}");

        // Every committed item stays readable, in memory and after reopening.
        assert_eq!(v.meta().unwrap().generation, generation + 1);
        for vault in [&v, &{
            let mut reopened = Vault::open(&dir).unwrap();
            reopened.unlock(&pp()).unwrap();
            reopened
        }] {
            let meta = vault.meta().unwrap();
            assert_eq!(meta.items.len(), 2);
            assert_eq!(vault.read_payload(&e).unwrap(), sample_payload("b"));
            for id in meta.items.keys() {
                vault.read_payload(id).unwrap();
            }
        }
    }

    #[test]
    fn upsert_restores_overwritten_records_when_the_index_write_fails() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        let existing = sample_meta();
        let e = v
            .upsert_items(vec![(existing.clone(), sample_payload("a"))])
            .unwrap()[0]
            .id
            .clone();
        let mut fresh = ItemMeta::draft(
            ItemKind::Cookie,
            "chrome",
            "Google Chrome",
            Some("github.com"),
            "_gh_sess",
        );
        fresh.source = "Default".into();

        // A directory where the sealed index belongs makes its replacement fail
        // deterministically (even with CAP_DAC_OVERRIDE); items/ stays writable.
        let index = dir.join("meta.sealed");
        let committed = std::fs::read(&index).unwrap();
        std::fs::remove_file(&index).unwrap();
        std::fs::create_dir(&index).unwrap();
        std::fs::write(index.join("occupied"), b"").unwrap();
        let res = v.upsert_items(vec![
            (existing, sample_payload("b")),
            (fresh, sample_payload("n")),
        ]);
        std::fs::remove_dir_all(&index).unwrap();
        std::fs::write(&index, committed).unwrap();
        assert!(matches!(res, Err(Error::Io(_))), "{res:?}");

        let mut reopened = Vault::open(&dir).unwrap();
        reopened.unlock(&pp()).unwrap();
        for vault in [&v, &reopened] {
            let meta = vault.meta().unwrap();
            assert_eq!(meta.items.len(), 1);
            assert_eq!(vault.read_payload(&e).unwrap(), sample_payload("a"));
        }
        assert_eq!(std::fs::read_dir(dir.join("items")).unwrap().count(), 1);
    }

    #[test]
    fn upsert_repairs_an_indexed_item_whose_record_is_missing() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        let e = v
            .upsert_items(vec![(sample_meta(), sample_payload("a"))])
            .unwrap()[0]
            .id
            .clone();
        std::fs::remove_file(dir.join("items").join(format!("{e}.sealed"))).unwrap();
        assert!(v.read_payload(&e).is_err());

        let out = v
            .upsert_items(vec![(sample_meta(), sample_payload("b"))])
            .unwrap();
        assert_eq!(
            (out[0].id.as_str(), out[0].outcome),
            (e.as_str(), UpsertOutcome::Updated)
        );
        assert_eq!(v.read_payload(&e).unwrap(), sample_payload("b"));
        // An ordinary update stages the old record; committing removes it.
        v.upsert_items(vec![(sample_meta(), sample_payload("c"))])
            .unwrap();
        assert_eq!(v.read_payload(&e).unwrap(), sample_payload("c"));
        let names: Vec<_> = std::fs::read_dir(dir.join("items"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(format!("{e}.sealed"))]);
    }

    #[test]
    fn restoring_an_unreplaced_record_drops_the_staged_link() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("item.sealed");
        std::fs::write(&path, b"old record").unwrap();
        // The write failed before replacing `path`: both names are one file.
        let backup = Backup::take(&path).unwrap();
        assert!(matches!(backup, Backup::Staged(_)));
        backup.restore(&path).unwrap();
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("item.sealed")]);
        assert_eq!(std::fs::read(&path).unwrap(), b"old record");
    }

    #[test]
    fn upsert_removes_records_an_interrupted_upsert_staged() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        let stale = dir.join("items").join(".0123.sealed.abcd.prev");
        std::fs::write(&stale, b"old sealed record").unwrap();
        let id = v
            .upsert_items(vec![(sample_meta(), sample_payload("a"))])
            .unwrap()[0]
            .id
            .clone();
        assert!(!stale.exists());
        // Deleting an item also drops a record staged after the last upsert.
        std::fs::write(&stale, b"old sealed record").unwrap();
        v.delete_item(&id).unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn unlock_restores_a_staged_record_the_index_still_references() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        let e = v
            .upsert_items(vec![(sample_meta(), sample_payload("a"))])
            .unwrap()[0]
            .id
            .clone();
        drop(v);
        // A crash after an update replaced the record but before its index
        // committed: the staged link holds the committed record.
        let items = dir.join("items");
        let record = items.join(format!("{e}.sealed"));
        std::fs::hard_link(&record, items.join(format!(".{e}.sealed.crash.prev"))).unwrap();
        write_private(&record, b"uncommitted record").unwrap();

        let mut reopened = Vault::open(&dir).unwrap();
        reopened.unlock(&pp()).unwrap();
        assert_eq!(reopened.read_payload(&e).unwrap(), sample_payload("a"));
        let names: Vec<_> = std::fs::read_dir(&items)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(format!("{e}.sealed"))]);
    }

    #[test]
    fn unlock_keeps_a_staged_record_it_cannot_restore() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        let e = v
            .upsert_items(vec![(sample_meta(), sample_payload("a"))])
            .unwrap()[0]
            .id
            .clone();
        drop(v);
        let items = dir.join("items");
        let record = items.join(format!("{e}.sealed"));
        let staged = items.join(format!(".{e}.sealed.crash.prev"));
        std::fs::hard_link(&record, &staged).unwrap();
        // The canonical name cannot be replaced (a non-empty directory).
        std::fs::remove_file(&record).unwrap();
        std::fs::create_dir(&record).unwrap();
        std::fs::write(record.join("occupied"), b"").unwrap();

        let mut reopened = Vault::open(&dir).unwrap();
        let err = reopened.unlock(&pp()).unwrap_err();
        assert!(err.to_string().contains("could not restore"), "{err}");
        assert!(staged.exists(), "the only committed record must survive");
        assert!(
            reopened.meta().is_err(),
            "a failed recovery leaves the vault locked"
        );

        std::fs::remove_dir_all(&record).unwrap();
        reopened.unlock(&pp()).unwrap();
        assert_eq!(reopened.read_payload(&e).unwrap(), sample_payload("a"));
        assert!(!staged.exists());
    }

    #[test]
    fn unlock_fails_when_staged_records_cannot_be_settled() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        Vault::create(&dir, &[&pp()]).unwrap();
        let items = dir.join("items");

        // A staged name that cannot be removed (a non-empty directory).
        let stuck = items.join(".0123.sealed.abcd.prev");
        std::fs::create_dir(&stuck).unwrap();
        std::fs::write(stuck.join("occupied"), b"").unwrap();
        let mut v = Vault::open(&dir).unwrap();
        let err = v.unlock(&pp()).unwrap_err();
        assert!(
            err.to_string()
                .contains("could not remove the staged record"),
            "{err}"
        );
        assert!(v.meta().is_err());
        std::fs::remove_dir_all(&stuck).unwrap();

        // An items directory that cannot be scanned.
        std::fs::rename(&items, dir.join("items.moved")).unwrap();
        std::fs::write(&items, b"not a directory").unwrap();
        assert!(matches!(v.unlock(&pp()), Err(Error::Io(_))));
        assert!(v.meta().is_err());
        std::fs::remove_file(&items).unwrap();
        std::fs::rename(dir.join("items.moved"), &items).unwrap();
        v.unlock(&pp()).unwrap();
    }

    #[test]
    fn unlock_keeps_a_staged_record_it_cannot_read() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        let e = v
            .upsert_items(vec![(sample_meta(), sample_payload("a"))])
            .unwrap()[0]
            .id
            .clone();
        drop(v);
        let items = dir.join("items");
        let record = items.join(format!("{e}.sealed"));
        // An uncommitted record is in place, and the staged one cannot be read.
        let staged = items.join(format!(".{e}.sealed.crash.prev"));
        std::fs::create_dir(&staged).unwrap();
        std::fs::write(staged.join("occupied"), b"").unwrap();
        write_private(&record, b"uncommitted record").unwrap();

        let mut reopened = Vault::open(&dir).unwrap();
        let err = reopened.unlock(&pp()).unwrap_err();
        assert!(err.to_string().contains("could not read"), "{err}");
        assert!(
            staged.exists(),
            "an unreadable staged record must not be discarded"
        );
        assert!(reopened.meta().is_err());
    }

    #[test]
    fn protectors_can_be_added_and_removed_but_not_the_last() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        let mut v = Vault::create(&dir, &[&pp()]).unwrap();
        let only = v.header().protectors[0].id.clone();
        assert!(v.remove_protector(&only).is_err());
        let rk = RecoveryKey::generate().unwrap();
        let rec = v.add_protector(&RecoveryProtector(rk.clone())).unwrap();
        v.remove_protector(&only).unwrap();
        let mut v = Vault::open(&dir).unwrap();
        assert!(v.unlock(&pp()).is_err());
        v.unlock(&RecoveryProtector(rk)).unwrap();
        assert_eq!(v.header().protectors[0].id, rec.id);
    }

    fn item(kind: ItemKind, domain: Option<&str>, key: &str) -> ItemMeta {
        ItemMeta {
            kind,
            domain: domain.map(str::to_string),
            key: key.into(),
            ..sample_meta()
        }
    }

    fn vault(d: &tempfile::TempDir) -> Vault {
        Vault::create(&d.path().join("kv"), &[&pp()]).unwrap()
    }

    #[test]
    fn saving_an_app_twice_upserts_by_key_and_never_duplicates() {
        let d = tempfile::tempdir().unwrap();
        let mut v = vault(&d);
        let batch = |tag: &str| {
            vec![
                (
                    item(ItemKind::Cookie, Some("github.com"), "user_session"),
                    sample_payload(tag),
                ),
                (
                    item(ItemKind::Cookie, Some("github.com"), "_gh_sess"),
                    sample_payload(tag),
                ),
                (
                    item(ItemKind::File, None, "Default/Bookmarks"),
                    sample_payload(tag),
                ),
            ]
        };
        let first = v.upsert_items(batch("a")).unwrap();
        assert!(first.iter().all(|u| u.outcome == UpsertOutcome::Created));
        let created = v.meta().unwrap().items.clone();
        assert_eq!(created.len(), 3);

        // The user unlocks one item between the two saves.
        let id0 = first[0].id.clone();
        v.update_meta(|m| {
            m.items.get_mut(&id0).unwrap().policy.unattended = true;
            Ok(())
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));

        // Same value: unchanged, timestamps move, nothing duplicates.
        let again = v.upsert_items(batch("a")).unwrap();
        assert_eq!(v.meta().unwrap().items.len(), 3);
        assert!(again.iter().all(|u| u.outcome == UpsertOutcome::Unchanged));
        assert_eq!(
            again.iter().map(|u| &u.id).collect::<Vec<_>>(),
            first.iter().map(|u| &u.id).collect::<Vec<_>>()
        );
        let it = &v.meta().unwrap().items[&id0];
        assert_eq!(it.created_ms, created[&id0].created_ms);
        assert!(it.updated_ms > created[&id0].updated_ms, "updated-at moves");
        assert_eq!(it.rev, 1, "an unchanged value is not rewritten");
        assert!(it.policy.unattended, "saving again keeps the lock state");

        // A changed value: updated in place with a new revision.
        let changed = v.upsert_items(batch("b")).unwrap();
        assert!(changed.iter().all(|u| u.outcome == UpsertOutcome::Updated));
        assert_eq!(v.meta().unwrap().items.len(), 3);
        assert_eq!(v.meta().unwrap().items[&id0].rev, 2);
        assert_eq!(v.read_payload(&id0).unwrap(), sample_payload("b"));
    }

    #[test]
    fn keys_are_per_app_and_per_type() {
        let d = tempfile::tempdir().unwrap();
        let mut v = vault(&d);
        let mut slack_cookie = item(ItemKind::Cookie, Some("github.com"), "user_session");
        slack_cookie.provider_id = "slack".into();
        let out = v
            .upsert_items(vec![
                (
                    item(ItemKind::Cookie, Some("github.com"), "user_session"),
                    sample_payload("1"),
                ),
                // Same domain and key in another app: a different item.
                (slack_cookie, sample_payload("2")),
                // Same domain and key, another type: a different item.
                (
                    item(ItemKind::LocalStorage, Some("github.com"), "user_session"),
                    sample_payload("3"),
                ),
                // Domains compare case-insensitively.
                (
                    item(ItemKind::Cookie, Some("GitHub.com"), "user_session"),
                    sample_payload("4"),
                ),
            ])
            .unwrap();
        assert_eq!(v.meta().unwrap().items.len(), 3);
        assert_eq!(out.len(), 3, "one result per distinct key");
        assert_eq!(v.read_payload(&out[0].id).unwrap(), sample_payload("4"));
    }

    #[test]
    fn delete_items_is_one_write_and_crypto_shreds() {
        let d = tempfile::tempdir().unwrap();
        let mut v = vault(&d);
        let ids: Vec<String> = v
            .upsert_items(vec![
                (
                    item(ItemKind::Cookie, Some("a.test"), "a"),
                    sample_payload("a"),
                ),
                (
                    item(ItemKind::Cookie, Some("b.test"), "b"),
                    sample_payload("b"),
                ),
                (item(ItemKind::File, None, "x/y"), sample_payload("c")),
            ])
            .unwrap()
            .into_iter()
            .map(|u| u.id)
            .collect();
        let before = v.meta().unwrap().generation;
        let removed = v
            .delete_items(&[ids[0].clone(), ids[1].clone(), ids[0].clone()])
            .unwrap();
        assert_eq!(removed.len(), 2);
        assert_eq!(v.meta().unwrap().generation, before + 1);
        assert_eq!(v.meta().unwrap().items.len(), 1);
        for id in &ids[..2] {
            assert!(
                !d.path()
                    .join("kv/items")
                    .join(format!("{id}.sealed"))
                    .exists()
            );
        }
        // An unknown id deletes nothing.
        assert!(v.delete_items(&[ids[2].clone(), "nope".into()]).is_err());
        assert_eq!(v.meta().unwrap().items.len(), 1);
    }

    #[test]
    fn a_format_1_vault_is_reported_not_misread() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("kv");
        drop(Vault::create(&dir, &[&pp()]).unwrap());
        let mut header: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("vault.json")).unwrap()).unwrap();
        header["format"] = 1.into();
        std::fs::write(dir.join("vault.json"), serde_json::to_vec(&header).unwrap()).unwrap();
        assert!(matches!(Vault::open(&dir), Err(Error::OldFormat(1))));
    }

    #[test]
    fn big_files_become_content_addressed_blobs_and_come_back_inline() {
        use base64::Engine as _;
        let d = tempfile::tempdir().unwrap();
        let mut v = vault(&d);
        let big = vec![7u8; crate::record::INLINE_FILE_LIMIT + 1];
        let rec = |path: &str, bytes: &[u8]| {
            let n = crate::record::file_record(path, 0o600, bytes).unwrap();
            let (m, p) = n.into_item("chrome", "Chrome", "Default", "full");
            (m, p)
        };
        let out = v
            .upsert_items(vec![
                rec("Default/History", &big),
                rec("Default/History.copy", &big),
                rec("Default/Bookmarks", b"{}"),
            ])
            .unwrap();
        let meta = v.meta().unwrap().items.clone();
        let hash = meta[&out[0].id].blob.clone().expect("a big file is a blob");
        assert_eq!(
            meta[&out[1].id].blob.as_ref(),
            Some(&hash),
            "same bytes, one blob"
        );
        assert!(meta[&out[2].id].blob.is_none(), "a small file stays inline");
        assert_eq!(
            std::fs::read_dir(d.path().join("kv/blobs"))
                .unwrap()
                .count(),
            1
        );

        // The record never carries the bytes.
        let p = v.read_payload(&out[0].id).unwrap();
        assert!(
            !p.record.contains("content") && p.record.contains(&hash),
            "{}",
            p.record
        );
        let inlined = v.inline_blobs(&p).unwrap();
        let f: crate::record::FileRecord = serde_json::from_str(&inlined.record).unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(f.content.clone().unwrap())
                .unwrap(),
            big
        );
        assert_eq!(&v.read_blob(&hash).unwrap()[..], &big[..]);

        // No plaintext, and a swapped blob is refused.
        let blob = d.path().join("kv/blobs").join(format!("{hash}.sealed"));
        let on_disk = std::fs::read(&blob).unwrap();
        assert!(!String::from_utf8_lossy(&on_disk).contains("BwcHBwcH"));
        std::fs::write(&blob, b"{}").unwrap();
        assert!(v.read_blob(&hash).is_err());
        std::fs::write(&blob, &on_disk).unwrap();

        // Deleting one holder keeps the blob; deleting the last shreds it.
        v.delete_items(&[out[0].id.clone()]).unwrap();
        assert!(blob.exists());
        v.delete_items(&[out[1].id.clone()]).unwrap();
        assert!(!blob.exists(), "no item names it any more");
        // Saving the same big file again is "unchanged".
        let again = v
            .upsert_items(vec![rec("Default/Bookmarks", b"{}")])
            .unwrap();
        assert_eq!(again[0].outcome, UpsertOutcome::Unchanged);
    }
}
