// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The on-disk vault.
//!
//! ```text
//! <dir>/                 0700
//!   vault.json           header: format, vault id, protector records (wrapped VMK only)
//!   meta.sealed          Meta, AEAD under HKDF(VMK, "meta"), AAD = vault id
//!   items/<id>.sealed    {rev, wrapped item key, payload}; AAD = vault | id | rev
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
pub const FORMAT: u32 = 1;
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
}

impl Keys {
    fn from_vmk(vmk: SecretKey) -> Self {
        Self {
            meta: vmk.derive(b"cua-keyvault/v1/meta"),
            wrap: vmk.derive(b"cua-keyvault/v1/item-wrap"),
            audit: vmk.derive(b"cua-keyvault/v1/audit"),
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
                Error::NoVault(dir.display().to_string())
            } else {
                e.into()
            }
        })?;
        let header: Header =
            serde_json::from_slice(&raw).map_err(|e| Error::Corrupt(format!("vault.json: {e}")))?;
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
        let head = self.audit.head().clone();
        let meta = self.meta.as_mut().ok_or(Error::Locked)?;
        meta.generation += 1;
        meta.audit_head = head;
        let generation = meta.generation;
        let plain = Zeroizing::new(serde_json::to_vec(meta)?);
        let keys = self.keys.as_ref().ok_or(Error::Locked)?;
        let sealed = crypto::seal(&keys.meta, &meta_aad(&self.header.vault_id), &plain)?;
        write_private(&self.dir.join("meta.sealed"), &serde_json::to_vec(&sealed)?)?;
        // Advance the external anti-rollback high-water mark after the write
        // lands (red-team F5).
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
        let meta = self.meta.as_mut().ok_or(Error::Locked)?;
        let mut draft = meta.clone();
        let out = f(&mut draft)?;
        *meta = draft;
        self.save_meta()?;
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

    /// Stores a new item. `meta.id`, `rev`, `record_digest` and timestamps
    /// are assigned here.
    pub fn put_item(&mut self, mut meta: ItemMeta, payload: &ItemPayload) -> Result<String> {
        let id = crypto::random_id()?;
        let digest = self.write_item(&id, 1, payload)?;
        let now = crate::now_ms();
        meta.id = id.clone();
        meta.rev = 1;
        meta.record_digest = digest;
        meta.created_ms = now;
        meta.updated_ms = now;
        let res = self.update_meta(|m| {
            m.items.insert(meta.id.clone(), meta);
            Ok(())
        });
        if res.is_err() {
            let _ = std::fs::remove_file(self.item_path(&id)?);
        }
        res.map(|_| id)
    }

    /// Replaces an item's payload (new revision, new item key).
    pub fn replace_payload(&mut self, id: &str, payload: &ItemPayload) -> Result<()> {
        let rev = self
            .meta()?
            .items
            .get(id)
            .ok_or_else(|| Error::NotFound(format!("item {id}")))?
            .rev
            + 1;
        let digest = self.write_item(id, rev, payload)?;
        self.update_meta(|m| {
            let it = m.items.get_mut(id).expect("checked above");
            it.rev = rev;
            it.record_digest = digest;
            it.updated_ms = crate::now_ms();
            Ok(())
        })
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

    /// Deletes an item (crypto-shredded: the record, with its wrapped key,
    /// is removed) and drops it from grants and rules.
    pub fn delete_item(&mut self, id: &str) -> Result<ItemMeta> {
        let path = self.item_path(id)?;
        let removed = self.update_meta(|m| {
            let it = m
                .items
                .remove(id)
                .ok_or_else(|| Error::NotFound(format!("item {id}")))?;
            for g in &mut m.grants {
                g.items.retain(|i| i != id);
                if g.items.is_empty() {
                    g.revoked = true;
                }
            }
            for r in &mut m.rules {
                r.items.retain(|i| i != id);
            }
            m.rules.retain(|r| !r.items.is_empty());
            Ok(it)
        })?;
        if path.exists() {
            // Overwrite before unlinking (best effort on copy-on-write file
            // systems; the wrapped key is what makes this a shred).
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as usize;
            let _ = std::fs::write(&path, vec![0u8; len]);
            std::fs::remove_file(&path)?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::KdfParams;
    use crate::model::{ItemKind, ItemPolicy, ItemSummary, PayloadEntry};
    use crate::protector::{PassphraseProtector, RecoveryKey, RecoveryProtector};

    pub(crate) fn pp() -> PassphraseProtector {
        PassphraseProtector::with_params("correct horse battery", KdfParams::for_tests().unwrap())
    }

    pub(crate) fn sample_meta() -> ItemMeta {
        ItemMeta {
            id: String::new(),
            kind: ItemKind::BrowserSite,
            label: "github.com (Chrome)".into(),
            provider_id: "chrome".into(),
            app_display: "Google Chrome".into(),
            site: Some("github.com".into()),
            account: None,
            source: "Default".into(),
            summary: ItemSummary::default(),
            warnings: vec![],
            identity_provider: false,
            policy: ItemPolicy::default(),
            created_ms: 0,
            updated_ms: 0,
            rev: 0,
            record_digest: String::new(),
        }
    }

    pub(crate) fn sample_payload(tag: &str) -> ItemPayload {
        ItemPayload {
            provider_id: "chrome".into(),
            scope: "full".into(),
            entries: vec![PayloadEntry {
                rel_path: "Cookies".into(),
                mode: 0o600,
                data: crypto::b64_encode(format!("fixture-{tag}").as_bytes()),
            }],
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
        v.replace_payload(&id, &sample_payload("b")).unwrap();
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
        assert_eq!(removed.site.as_deref(), Some("github.com"));
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
        let b = v.put_item(sample_meta(), &sample_payload("b")).unwrap();
        let pa = dir.join("items").join(format!("{a}.sealed"));
        let pb = dir.join("items").join(format!("{b}.sealed"));
        let old_a = std::fs::read(&pa).unwrap();
        v.replace_payload(&a, &sample_payload("a2")).unwrap();
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
        v.replace_payload(&id, &sample_payload("b")).unwrap();
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
}
