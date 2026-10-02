// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What an import left in the guest, and how to take it back out.
//!
//! Every importer reports what it wrote into an [`ImportRecord`]: each file
//! it wrote, each directory it created (not ones that already existed),
//! each Keychain item it installed, and each cookie row it merged into an
//! existing (not ledger-owned) `Cookies` database. Nothing is inferred
//! after the fact.
//!
//! After a successful import the server persists the record as a
//! [`Ledger`] in a [`LedgerStore`]: one JSON file per import, mode 0600, in a
//! 0700 directory. The default root is `/run/cua/teleport-ledger` when
//! `/run/cua` exists and is writable (tmpfs in Cua images, so a reboot also
//! forgets the paths it names), else `<data_dir>/teleport/ledger`.
//!
//! [`wipe`] undoes a ledger: it removes the ledgered files, the directories
//! the import created and the Keychain items it installed, and `DELETE`s
//! its cookie rows out of a `Cookies` database that is itself untouched
//! (never named in `files`), through the injected [`HostEffects`]. It never
//! follows a symlink and refuses any path that is not strictly inside the
//! destination home.

use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::host::HostEffects;

/// Current ledger file format.
pub const LEDGER_VERSION: u32 = 1;

/// A Keychain item an import installed (never its secret).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeychainRef {
    /// Item service name.
    pub service: String,
    /// Item account name.
    pub account: String,
}

/// One cookie row an import wrote into an existing (not ledger-owned)
/// `Cookies` database, identified by Chromium's own uniqueness for a row
/// (`host_key`, `name`, `path`): enough for `wipe` to `DELETE` exactly this
/// row later without touching the database file itself or any other row in
/// it, pre-existing or from another import.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CookieRowRef {
    /// Absolute path of the `Cookies` database.
    pub db: PathBuf,
    /// `cookies.host_key`.
    pub host_key: String,
    /// `cookies.name`.
    pub name: String,
    /// `cookies.path`.
    pub path: String,
}

/// One saved login an import wrote into an existing `Login Data` database,
/// identified by what Chromium matches a login on, so `wipe` deletes exactly
/// this row and nothing the browser already held.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LoginRowRef {
    /// Absolute path of the `Login Data` database.
    pub db: PathBuf,
    /// `logins.origin_url`.
    pub origin_url: String,
    /// `logins.username_value`.
    pub username_value: String,
    /// `logins.signon_realm`.
    pub signon_realm: String,
}

/// The localStorage keys an import wrote into a Chromium `Local Storage`
/// LevelDB, so `wipe` can delete exactly those (hex, as LevelDB keys are
/// arbitrary bytes) and leave the browser's own values alone.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StorageKeysRef {
    /// Absolute path of the `Local Storage/leveldb` directory.
    pub db: PathBuf,
    /// Data keys and created `META:` keys, hex.
    pub keys: Vec<String>,
}

/// What one import wrote, as reported by the importer while it wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportRecord {
    /// Absolute paths of files written (created or overwritten).
    pub files: Vec<PathBuf>,
    /// Absolute paths of directories this import created, parents first.
    pub created_dirs: Vec<PathBuf>,
    /// Keychain items installed.
    pub keychain_items: Vec<KeychainRef>,
    /// Cookie rows written into an existing `Cookies` database (never
    /// ledgered as a whole file: an importer that owns the database will
    /// have already called [`Self::file_written`] for it, so this is only
    /// the merge case, into a database the import did not create).
    pub cookie_rows: Vec<CookieRowRef>,
    /// localStorage keys written into a `Local Storage` LevelDB.
    pub local_storage: Vec<StorageKeysRef>,
    /// Saved logins written into an existing `Login Data` database.
    pub login_rows: Vec<LoginRowRef>,
    /// Things the user should be told about that did not fail the import
    /// (e.g. the app will ask once for a password). Reported back to the
    /// client, never ledgered.
    pub notices: Vec<String>,
}

impl ImportRecord {
    /// `std::fs::create_dir_all`, recording each directory that did not
    /// exist before.
    pub fn create_dir_all(&mut self, dir: &Path) -> io::Result<()> {
        let mut missing = Vec::new();
        let mut cursor = Some(dir);
        while let Some(path) = cursor {
            if path.as_os_str().is_empty() || std::fs::symlink_metadata(path).is_ok() {
                break;
            }
            missing.push(path.to_path_buf());
            cursor = path.parent();
        }
        std::fs::create_dir_all(dir)?;
        for path in missing.into_iter().rev() {
            if !self.created_dirs.contains(&path) {
                self.created_dirs.push(path);
            }
        }
        Ok(())
    }

    /// Records a written file.
    pub fn file_written(&mut self, path: &Path) {
        if !self.files.iter().any(|p| p == path) {
            self.files.push(path.to_path_buf());
        }
    }

    /// Records an installed Keychain item.
    pub fn keychain_installed(&mut self, service: &str, account: &str) {
        let item = KeychainRef {
            service: service.to_owned(),
            account: account.to_owned(),
        };
        if !self.keychain_items.contains(&item) {
            self.keychain_items.push(item);
        }
    }

    /// Records a cookie row written into an existing `Cookies` database
    /// (`db`), identified the same way Chromium's own unique index does.
    pub fn cookie_row_written(&mut self, db: &Path, host_key: &str, name: &str, path: &str) {
        let row = CookieRowRef {
            db: db.to_path_buf(),
            host_key: host_key.to_owned(),
            name: name.to_owned(),
            path: path.to_owned(),
        };
        if !self.cookie_rows.contains(&row) {
            self.cookie_rows.push(row);
        }
    }

    /// Records a saved login written into an existing `Login Data` database.
    pub fn login_row_written(
        &mut self,
        db: &Path,
        origin_url: &str,
        username_value: &str,
        signon_realm: &str,
    ) {
        let row = LoginRowRef {
            db: db.to_path_buf(),
            origin_url: origin_url.to_owned(),
            username_value: username_value.to_owned(),
            signon_realm: signon_realm.to_owned(),
        };
        if !self.login_rows.contains(&row) {
            self.login_rows.push(row);
        }
    }

    /// Records localStorage keys written into the LevelDB at `db`.
    pub fn local_storage_written(&mut self, db: &Path, written: &cua_chromium_storage::Written) {
        let keys: Vec<String> = written
            .keys
            .iter()
            .chain(&written.meta_keys)
            .map(hex::encode)
            .collect();
        if keys.is_empty() {
            return;
        }
        match self.local_storage.iter_mut().find(|r| r.db == db) {
            Some(r) => {
                for k in keys {
                    if !r.keys.contains(&k) {
                        r.keys.push(k);
                    }
                }
            }
            None => self.local_storage.push(StorageKeysRef {
                db: db.to_path_buf(),
                keys,
            }),
        }
    }

    /// Adds everything in `other` that is not already here.
    pub fn merge(&mut self, other: &ImportRecord) {
        for f in &other.files {
            self.file_written(f);
        }
        for r in &other.cookie_rows {
            if !self.cookie_rows.contains(r) {
                self.cookie_rows.push(r.clone());
            }
        }
        for r in &other.login_rows {
            if !self.login_rows.contains(r) {
                self.login_rows.push(r.clone());
            }
        }
        for r in &other.local_storage {
            for k in &r.keys {
                match self.local_storage.iter_mut().find(|x| x.db == r.db) {
                    Some(x) if !x.keys.contains(k) => x.keys.push(k.clone()),
                    Some(_) => {}
                    None => self.local_storage.push(StorageKeysRef {
                        db: r.db.clone(),
                        keys: vec![k.clone()],
                    }),
                }
            }
        }
        for d in &other.created_dirs {
            if !self.created_dirs.contains(d) {
                self.created_dirs.push(d.clone());
            }
        }
        for k in &other.keychain_items {
            self.keychain_installed(&k.service, &k.account);
        }
    }

    /// True if nothing was written.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
            && self.created_dirs.is_empty()
            && self.keychain_items.is_empty()
            && self.cookie_rows.is_empty()
            && self.local_storage.is_empty()
            && self.login_rows.is_empty()
    }
}

/// The persisted record of one committed import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// Format version ([`LEDGER_VERSION`]).
    pub version: u32,
    /// Client-chosen import id.
    pub import_id: String,
    /// Provider that imported the bundle.
    pub provider: String,
    /// Absolute paths of files written.
    pub files: Vec<PathBuf>,
    /// Absolute paths of directories the import created.
    pub directories: Vec<PathBuf>,
    /// Keychain items installed.
    pub keychain_items: Vec<KeychainRef>,
    /// Cookie rows written into an existing `Cookies` database. `default`:
    /// a ledger from before this field existed names none, correctly (it
    /// predates row-level cookie tracking, not a ledger that lost rows).
    #[serde(default)]
    pub cookie_rows: Vec<CookieRowRef>,
    /// localStorage keys written into a `Local Storage` LevelDB.
    #[serde(default)]
    pub local_storage: Vec<StorageKeysRef>,
    /// Saved logins written into an existing `Login Data` database.
    #[serde(default)]
    pub login_rows: Vec<LoginRowRef>,
    /// Unix ms after which the import is wiped; 0 = never.
    pub expires_at_ms: u64,
    /// Unix ms of the import.
    pub imported_at_ms: u64,
}

impl Ledger {
    /// A ledger for `record`.
    pub fn new(
        import_id: &str,
        provider: &str,
        record: &ImportRecord,
        expires_at_ms: u64,
        imported_at_ms: u64,
    ) -> Self {
        Self {
            version: LEDGER_VERSION,
            import_id: import_id.to_owned(),
            provider: provider.to_owned(),
            files: record.files.clone(),
            directories: record.created_dirs.clone(),
            keychain_items: record.keychain_items.clone(),
            cookie_rows: record.cookie_rows.clone(),
            local_storage: record.local_storage.clone(),
            login_rows: record.login_rows.clone(),
            expires_at_ms,
            imported_at_ms,
        }
    }

    /// The ledger as a record (for merging a re-import of the same id).
    pub fn record(&self) -> ImportRecord {
        ImportRecord {
            files: self.files.clone(),
            created_dirs: self.directories.clone(),
            keychain_items: self.keychain_items.clone(),
            cookie_rows: self.cookie_rows.clone(),
            local_storage: self.local_storage.clone(),
            login_rows: self.login_rows.clone(),
            notices: Vec::new(),
        }
    }

    /// True once `expires_at_ms` is set and not after `now_ms`.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.expires_at_ms != 0 && self.expires_at_ms <= now_ms
    }
}

/// A ledger file that could not be read or parsed. It stays on disk.
#[derive(Debug)]
pub struct UnreadableLedger {
    /// The file.
    pub path: PathBuf,
    /// Why.
    pub error: String,
}

/// One JSON file per import under a 0700 directory.
#[derive(Clone, Debug)]
pub struct LedgerStore {
    root: PathBuf,
}

/// The tmpfs parent preferred for ledgers.
pub const RUN_CUA: &str = "/run/cua";

impl LedgerStore {
    /// A store rooted at `root` (created 0700 on first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `/run/cua/teleport-ledger` when `/run/cua` exists and is writable,
    /// else `<data_dir>/teleport/ledger`.
    pub fn default_root(data_dir: &Path) -> PathBuf {
        Self::default_root_with(Path::new(RUN_CUA), data_dir)
    }

    /// [`Self::default_root`] with an explicit run directory (testable).
    pub fn default_root_with(run_dir: &Path, data_dir: &Path) -> PathBuf {
        if dir_writable(run_dir) {
            run_dir.join("teleport-ledger")
        } else {
            data_dir.join("teleport").join("ledger")
        }
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the ledger for `import_id` lives: the id is hashed, so any id
    /// maps to one safe file name.
    pub fn path_for(&self, import_id: &str) -> PathBuf {
        let digest = Sha256::digest(import_id.as_bytes());
        self.root.join(format!("{}.json", hex::encode(digest)))
    }

    fn ensure_root(&self) -> io::Result<()> {
        if let Ok(meta) = std::fs::symlink_metadata(&self.root) {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(io::Error::other(format!(
                    "ledger root {} is not a directory",
                    self.root.display()
                )));
            }
        } else {
            create_private_dir_all(&self.root)?;
        }
        set_mode(&self.root, 0o700)
    }

    /// Writes (replaces) a ledger atomically: a 0600 temp file created with
    /// `O_EXCL`, then renamed over the old one.
    pub fn write(&self, ledger: &Ledger) -> io::Result<()> {
        self.ensure_root()?;
        let path = self.path_for(&ledger.import_id);
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let json = serde_json::to_vec_pretty(ledger).map_err(io::Error::other)?;
        let mut file = create_new_private(&tmp)?;
        let written = file.write_all(&json).and_then(|_| file.sync_all());
        drop(file);
        if let Err(e) = written.and_then(|_| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(())
    }

    /// The ledger for `import_id`: `Ok(None)` when there is none, `Err` when
    /// it exists but cannot be read.
    pub fn read(&self, import_id: &str) -> io::Result<Option<Ledger>> {
        let path = self.path_for(import_id);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let ledger: Ledger = serde_json::from_slice(&bytes).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: {e}", path.display()),
                    )
                })?;
                if ledger.import_id != import_id {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: names another import", path.display()),
                    ));
                }
                Ok(Some(ledger))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every ledger, plus the files that could not be read (left in place).
    pub fn list(&self) -> (Vec<Ledger>, Vec<UnreadableLedger>) {
        let mut ledgers = Vec::new();
        let mut unreadable = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return (ledgers, unreadable),
            Err(e) => {
                unreadable.push(UnreadableLedger {
                    path: self.root.clone(),
                    error: e.to_string(),
                });
                return (ledgers, unreadable);
            }
        };
        // Bounded: a ledger directory holds one file per live import.
        for entry in entries.take(100_000) {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(e) => {
                    unreadable.push(UnreadableLedger {
                        path: self.root.clone(),
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let parsed = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    serde_json::from_slice::<Ledger>(&bytes).map_err(|e| e.to_string())
                })
                .and_then(|ledger| {
                    if self.path_for(&ledger.import_id) == path {
                        Ok(ledger)
                    } else {
                        Err("file name does not match its import id".to_owned())
                    }
                });
            match parsed {
                Ok(ledger) => ledgers.push(ledger),
                Err(error) => unreadable.push(UnreadableLedger { path, error }),
            }
        }
        ledgers.sort_by(|a, b| a.import_id.cmp(&b.import_id));
        (ledgers, unreadable)
    }

    /// Removes the ledger for `import_id` (missing is fine).
    pub fn remove(&self, import_id: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path_for(import_id)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// What [`wipe`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WipeReport {
    /// Paths removed (files, and created directories left empty).
    pub removed_paths: Vec<PathBuf>,
    /// Keychain items removed.
    pub keychain_items_removed: u32,
    /// Cookie rows deleted from an existing `Cookies` database (the
    /// database itself is never in `removed_paths`: it was not this
    /// import's file to remove).
    pub cookie_rows_removed: u32,
    /// localStorage keys deleted from a `Local Storage` LevelDB (the
    /// database itself is never removed).
    pub local_storage_keys_removed: u32,
    /// Saved logins deleted from an existing `Login Data` database.
    pub login_rows_removed: u32,
    /// Ledgered paths refused (outside the home, `..`, symlinks), with why.
    /// Never retried: a refused entry is not trusted.
    pub refused: Vec<(PathBuf, String)>,
    /// Entries that failed for an I/O reason and should be retried.
    pub failed: ImportRecord,
    /// Their errors, for logging (paths only, never contents).
    pub errors: Vec<String>,
}

impl WipeReport {
    /// True when nothing is left to retry.
    pub fn complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Why a ledgered path may not be touched.
fn confine(path: &Path, dest_home: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("not an absolute path".into());
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("contains '.' or '..'".into());
    }
    let rel = path
        .strip_prefix(dest_home)
        .map_err(|_| format!("outside the destination home {}", dest_home.display()))?;
    if rel.as_os_str().is_empty() {
        return Err("is the destination home itself".into());
    }
    Ok(rel.to_path_buf())
}

enum Probe {
    /// The path (or one of its parents) is gone.
    Missing,
    /// A plain file or directory, reached without crossing a symlink.
    Present(std::fs::Metadata),
}

/// Walks from `dest_home` down to `path` with `lstat`, refusing any symlink
/// on the way or at the end.
fn probe(dest_home: &Path, rel: &Path) -> Result<Probe, String> {
    let mut cursor = dest_home.to_path_buf();
    let count = rel.components().count();
    for (i, part) in rel.components().enumerate() {
        cursor.push(part);
        match std::fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!("{} is a symlink", cursor.display()));
            }
            Ok(meta) if i + 1 == count => return Ok(Probe::Present(meta)),
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(Probe::Missing),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Probe::Missing),
            Err(e) => return Err(format!("{}: {e}", cursor.display())),
        }
    }
    Ok(Probe::Missing)
}

/// Deletes exactly one row from `db`'s `cookies` table, by the same
/// (`host_key`, `name`, `path`) uniqueness [`crate::cookies::install_cookies`]
/// inserts with. A row that is not there (already gone, or never written --
/// a retried wipe) is not an error: the goal state is "this row is absent"
/// either way.
fn delete_cookie_row(db: &Path, host_key: &str, name: &str, path: &str) -> rusqlite::Result<usize> {
    let conn = rusqlite::Connection::open(db)?;
    conn.execute(
        "DELETE FROM cookies WHERE host_key = ?1 AND name = ?2 AND path = ?3",
        rusqlite::params![host_key, name, path],
    )
}

/// Deletes exactly one saved login from a `Login Data` database. A row that
/// is not there is not an error.
fn delete_login_row(row: &LoginRowRef) -> rusqlite::Result<usize> {
    let conn = rusqlite::Connection::open(&row.db)?;
    conn.execute(
        "DELETE FROM logins WHERE origin_url = ?1 AND username_value = ?2 AND signon_realm = ?3",
        rusqlite::params![row.origin_url, row.username_value, row.signon_realm],
    )
}

/// Undoes `ledger` under `dest_home`: removes its files, the directories it
/// created (see below), its Keychain items and its cookie rows (through
/// `host` for the Keychain; everything else directly).
///
/// Created directories are removed deepest first, and only once empty: a
/// directory one import created may since hold another import's files, or
/// other apps' state (`~/Library/Application Support` on a fresh home), so
/// nothing the ledger does not name is ever deleted. Files an app wrote
/// after the import therefore keep their directory.
pub fn wipe(ledger: &Ledger, dest_home: &Path, host: &dyn HostEffects) -> WipeReport {
    let mut report = WipeReport::default();
    for path in &ledger.files {
        let rel = match confine(path, dest_home) {
            Ok(rel) => rel,
            Err(why) => {
                report.refused.push((path.clone(), why));
                continue;
            }
        };
        match probe(dest_home, &rel) {
            Ok(Probe::Missing) => {}
            Ok(Probe::Present(meta)) if meta.is_dir() => report
                .refused
                .push((path.clone(), "ledgered as a file but is a directory".into())),
            Ok(Probe::Present(_)) => match std::fs::remove_file(path) {
                Ok(()) => report.removed_paths.push(path.clone()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    report.errors.push(format!("{}: {e}", path.display()));
                    report.failed.file_written(path);
                }
            },
            Err(why) => report.refused.push((path.clone(), why)),
        }
    }
    for row in &ledger.cookie_rows {
        let rel = match confine(&row.db, dest_home) {
            Ok(rel) => rel,
            Err(why) => {
                report.refused.push((row.db.clone(), why));
                continue;
            }
        };
        match probe(dest_home, &rel) {
            // The database is gone entirely (the whole profile was removed
            // some other way): nothing to delete a row out of.
            Ok(Probe::Missing) => {}
            Ok(Probe::Present(meta)) if meta.is_dir() => {
                let why = "ledgered as a Cookies database but is a directory".into();
                report.refused.push((row.db.clone(), why));
            }
            Ok(Probe::Present(_)) => {
                match delete_cookie_row(&row.db, &row.host_key, &row.name, &row.path) {
                    // Counts rows actually deleted: a second wipe removes nothing.
                    Ok(n) => report.cookie_rows_removed += n as u32,
                    Err(e) => {
                        report.errors.push(format!("{}: {e}", row.db.display()));
                        report.failed.cookie_row_written(
                            &row.db,
                            &row.host_key,
                            &row.name,
                            &row.path,
                        );
                    }
                }
            }
            Err(why) => report.refused.push((row.db.clone(), why)),
        }
    }
    for row in &ledger.login_rows {
        let rel = match confine(&row.db, dest_home) {
            Ok(rel) => rel,
            Err(why) => {
                report.refused.push((row.db.clone(), why));
                continue;
            }
        };
        match probe(dest_home, &rel) {
            Ok(Probe::Missing) => {}
            Ok(Probe::Present(meta)) if meta.is_dir() => report.refused.push((
                row.db.clone(),
                "ledgered as a Login Data database but is a directory".into(),
            )),
            Ok(Probe::Present(_)) => match delete_login_row(row) {
                Ok(n) => report.login_rows_removed += n as u32,
                Err(e) => {
                    report.errors.push(format!("{}: {e}", row.db.display()));
                    report.failed.login_rows.push(row.clone());
                }
            },
            Err(why) => report.refused.push((row.db.clone(), why)),
        }
    }
    for r in &ledger.local_storage {
        let rel = match confine(&r.db, dest_home) {
            Ok(rel) => rel,
            Err(why) => {
                report.refused.push((r.db.clone(), why));
                continue;
            }
        };
        match probe(dest_home, &rel) {
            Ok(Probe::Missing) => {}
            Ok(Probe::Present(meta)) if !meta.is_dir() => report.refused.push((
                r.db.clone(),
                "ledgered as a LevelDB directory but is a file".into(),
            )),
            Ok(Probe::Present(_)) => {
                let keys: Vec<Vec<u8>> =
                    r.keys.iter().filter_map(|k| hex::decode(k).ok()).collect();
                match cua_chromium_storage::delete(&r.db, &keys) {
                    Ok(n) => report.local_storage_keys_removed += n as u32,
                    Err(e) => {
                        report.errors.push(format!("{}: {e}", r.db.display()));
                        report.failed.local_storage.push(r.clone());
                    }
                }
            }
            Err(why) => report.refused.push((r.db.clone(), why)),
        }
    }
    let mut dirs: Vec<(PathBuf, PathBuf)> = Vec::new();
    for path in &ledger.directories {
        match confine(path, dest_home) {
            Ok(rel) => dirs.push((path.clone(), rel)),
            Err(why) => report.refused.push((path.clone(), why)),
        }
    }
    dirs.sort_by_key(|(_, rel)| std::cmp::Reverse(rel.components().count()));
    for (path, rel) in dirs {
        match probe(dest_home, &rel) {
            Ok(Probe::Missing) => {}
            Ok(Probe::Present(meta)) if !meta.is_dir() => report
                .refused
                .push((path.clone(), "ledgered as a directory but is a file".into())),
            Ok(Probe::Present(_)) => {
                match std::fs::remove_dir(&path) {
                    Ok(()) => report.removed_paths.push(path.clone()),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    // Not empty: it holds something the ledger does not
                    // name. Kept, and not retried.
                    Err(_) if std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_some()) => {}
                    Err(e) => {
                        report.errors.push(format!("{}: {e}", path.display()));
                        report.failed.created_dirs.push(path.clone());
                    }
                }
            }
            Err(why) => report.refused.push((path.clone(), why)),
        }
    }
    for item in &ledger.keychain_items {
        match crate::keychain::remove_generic(host, &item.service, &item.account) {
            Ok(true) => report.keychain_items_removed += 1,
            Ok(false) => {}
            Err(e) => {
                report
                    .errors
                    .push(format!("keychain item {:?}: {e}", item.service));
                report
                    .failed
                    .keychain_installed(&item.service, &item.account);
            }
        }
    }
    report
}

/// The outcome of wiping one ledger in a store.
#[derive(Debug)]
pub struct Wiped {
    /// Import id.
    pub import_id: String,
    /// What was done.
    pub report: WipeReport,
}

impl LedgerStore {
    /// Wipes `ledger` and then removes it, or, when some entries failed for
    /// an I/O reason, rewrites it with just those so a later sweep retries.
    pub fn wipe_and_forget(
        &self,
        ledger: &Ledger,
        dest_home: &Path,
        host: &dyn HostEffects,
    ) -> io::Result<WipeReport> {
        let report = wipe(ledger, dest_home, host);
        if report.complete() {
            self.remove(&ledger.import_id)?;
        } else {
            let mut rest = Ledger::new(
                &ledger.import_id,
                &ledger.provider,
                &report.failed,
                ledger.expires_at_ms,
                ledger.imported_at_ms,
            );
            rest.version = ledger.version;
            self.write(&rest)?;
        }
        Ok(report)
    }

    /// Wipes every readable ledger that expired at `now_ms`. Unreadable
    /// ledgers are returned untouched for the caller to log; they are never
    /// dropped.
    pub fn sweep_expired(
        &self,
        now_ms: u64,
        dest_home: &Path,
        host: &dyn HostEffects,
    ) -> (Vec<Wiped>, Vec<UnreadableLedger>) {
        let (ledgers, mut unreadable) = self.list();
        let mut wiped = Vec::new();
        for ledger in ledgers.into_iter().filter(|l| l.expired(now_ms)) {
            match self.wipe_and_forget(&ledger, dest_home, host) {
                Ok(report) => wiped.push(Wiped {
                    import_id: ledger.import_id,
                    report,
                }),
                Err(e) => unreadable.push(UnreadableLedger {
                    path: self.path_for(&ledger.import_id),
                    error: e.to_string(),
                }),
            }
        }
        (wiped, unreadable)
    }
}

/// Unix time now, in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn dir_writable(dir: &Path) -> bool {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        _ => return false,
    }
    // Probe by creating (and removing) a unique file: permission bits alone
    // do not account for read-only mounts.
    let probe = dir.join(format!(".cua-ledger-probe-{}", std::process::id()));
    match create_new_private(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Creates `path` with `O_EXCL` and mode 0600.
pub fn create_new_private(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `create_dir_all` whose new directories are 0700, and `dir` itself set to
/// 0700 even if it existed.
pub fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    set_mode(dir, 0o700)
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::FakeHost;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn ledger(id: &str, files: &[PathBuf], dirs: &[PathBuf], expires: u64) -> Ledger {
        Ledger {
            version: LEDGER_VERSION,
            import_id: id.into(),
            provider: "chrome".into(),
            files: files.to_vec(),
            directories: dirs.to_vec(),
            keychain_items: vec![],
            cookie_rows: vec![],
            local_storage: vec![],
            login_rows: vec![],
            expires_at_ms: expires,
            imported_at_ms: 1,
        }
    }

    #[test]
    fn record_notes_only_directories_it_created() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".config")).unwrap();
        let mut record = ImportRecord::default();
        record
            .create_dir_all(&home.path().join(".config/app/Default"))
            .unwrap();
        record.create_dir_all(&home.path().join(".config")).unwrap();
        assert_eq!(
            record.created_dirs,
            vec![
                home.path().join(".config/app"),
                home.path().join(".config/app/Default")
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn store_is_private_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = LedgerStore::new(dir.path().join("ledger"));
        let l = ledger("imp/../weird id", &[dir.path().join("a")], &[], 5);
        store.write(&l).unwrap();
        assert_eq!(mode(store.root()), 0o700);
        let path = store.path_for(&l.import_id);
        assert_eq!(path.parent().unwrap(), store.root());
        assert_eq!(mode(&path), 0o600);
        assert_eq!(store.read(&l.import_id).unwrap(), Some(l.clone()));
        assert_eq!(store.read("other").unwrap(), None);
        let (all, bad) = store.list();
        assert_eq!(all, vec![l.clone()]);
        assert!(bad.is_empty());
        store.remove(&l.import_id).unwrap();
        assert!(store.list().0.is_empty());
    }

    #[test]
    fn default_root_prefers_a_writable_run_dir() {
        let run = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        assert_eq!(
            LedgerStore::default_root_with(run.path(), data.path()),
            run.path().join("teleport-ledger")
        );
        assert_eq!(
            LedgerStore::default_root_with(&run.path().join("missing"), data.path()),
            data.path().join("teleport/ledger")
        );
        // The probe leaves nothing behind.
        assert_eq!(std::fs::read_dir(run.path()).unwrap().count(), 0);
    }

    #[test]
    fn wipe_removes_files_and_created_dirs_only() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".config/other")).unwrap();
        std::fs::write(h.join(".config/other/keep"), b"k").unwrap();
        let mut record = ImportRecord::default();
        record
            .create_dir_all(&h.join(".config/app/Default"))
            .unwrap();
        std::fs::write(h.join(".config/app/Default/Cookies"), b"c").unwrap();
        record.file_written(&h.join(".config/app/Default/Cookies"));
        // Written by the app after the import, inside a created dir.
        std::fs::write(h.join(".config/app/Default/Cookies-journal"), b"j").unwrap();
        let l = Ledger::new("x", "chrome", &record, 0, 1);
        let report = wipe(&l, h, &FakeHost::new());
        assert!(report.refused.is_empty(), "{:?}", report.refused);
        assert!(report.complete());
        assert!(!h.join(".config/app/Default/Cookies").exists());
        // Unledgered content keeps its (created) directory.
        assert!(h.join(".config/app/Default/Cookies-journal").exists());
        assert!(h.join(".config/other/keep").exists());
        assert!(report
            .removed_paths
            .contains(&h.join(".config/app/Default/Cookies")));
        std::fs::remove_file(h.join(".config/app/Default/Cookies-journal")).unwrap();
        let report = wipe(&l, h, &FakeHost::new());
        assert_eq!(
            report.removed_paths,
            vec![h.join(".config/app/Default"), h.join(".config/app")]
        );
        assert!(h.join(".config/other/keep").exists());
    }

    /// A `Cookies` database the import did not create: `wipe` must `DELETE`
    /// only the rows it ledgered, never the database file (not in
    /// `removed_paths`) and never a row some other session (pre-existing,
    /// or another import) left in the same table.
    #[test]
    fn wipe_deletes_only_its_own_cookie_rows_never_the_database_or_other_rows() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".config/app/Default")).unwrap();
        let db = h.join(".config/app/Default/Cookies");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (
                host_key TEXT NOT NULL,
                name TEXT NOT NULL,
                value TEXT NOT NULL,
                path TEXT NOT NULL,
                UNIQUE (host_key, name, path)
            );",
        )
        .unwrap();
        // A row that predates this import, at the destination's own site.
        conn.execute(
            "INSERT INTO cookies (host_key, name, value, path) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["example.test", "pre_existing", "keep-me", "/"],
        )
        .unwrap();
        // The row this import wrote.
        conn.execute(
            "INSERT INTO cookies (host_key, name, value, path) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![".github.com", "user_session", "teleported", "/"],
        )
        .unwrap();
        drop(conn);

        let mut record = ImportRecord::default();
        record.cookie_row_written(&db, ".github.com", "user_session", "/");
        let l = Ledger::new("x", "chrome", &record, 0, 1);
        let report = wipe(&l, h, &FakeHost::new());
        assert!(report.refused.is_empty(), "{:?}", report.refused);
        assert!(report.complete(), "{:?}", report.errors);
        assert_eq!(report.cookie_rows_removed, 1);
        // The database file itself was never touched as a "removed path".
        assert!(db.is_file());
        assert!(!report.removed_paths.contains(&db));

        let conn = rusqlite::Connection::open(&db).unwrap();
        let mut stmt = conn
            .prepare("SELECT host_key, name FROM cookies ORDER BY host_key")
            .unwrap();
        let remaining: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            remaining,
            vec![("example.test".to_string(), "pre_existing".to_string())]
        );

        // Wiping again (the row already gone) is not an error.
        let report = wipe(&l, h, &FakeHost::new());
        assert!(report.complete(), "{:?}", report.errors);
        assert_eq!(report.cookie_rows_removed, 0);
    }

    #[test]
    fn created_dirs_are_removed_only_when_empty() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let mut record = ImportRecord::default();
        record.create_dir_all(&h.join(".claude")).unwrap();
        std::fs::write(h.join(".claude/.credentials.json"), b"c").unwrap();
        record.file_written(&h.join(".claude/.credentials.json"));
        std::fs::write(h.join(".claude/later.txt"), b"l").unwrap();
        let report = wipe(
            &Ledger::new("x", "claude-code", &record, 0, 1),
            h,
            &FakeHost::new(),
        );
        assert!(report.complete());
        assert!(!h.join(".claude/.credentials.json").exists());
        assert!(h.join(".claude/later.txt").exists());
        std::fs::remove_file(h.join(".claude/later.txt")).unwrap();
        let report = wipe(
            &Ledger::new("x", "claude-code", &record, 0, 1),
            h,
            &FakeHost::new(),
        );
        assert_eq!(report.removed_paths, vec![h.join(".claude")]);
    }

    #[test]
    fn traversal_and_outside_paths_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim");
        std::fs::write(&victim, b"v").unwrap();
        let h = home.path();
        let l = ledger(
            "x",
            &[
                victim.clone(),
                h.join("../")
                    .join(outside.path().file_name().unwrap())
                    .join("victim"),
                PathBuf::from("relative/file"),
                h.to_path_buf(),
            ],
            &[outside.path().to_path_buf(), h.to_path_buf()],
            0,
        );
        let report = wipe(&l, h, &FakeHost::new());
        assert_eq!(report.refused.len(), 6, "{:?}", report.refused);
        assert!(report.removed_paths.is_empty());
        assert!(victim.exists());
        assert!(h.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::write(outside.path().join("victim"), b"v").unwrap();
        std::fs::create_dir(outside.path().join("dir")).unwrap();
        std::fs::write(outside.path().join("dir/inner"), b"i").unwrap();
        // A ledgered file that is now a symlink, a ledgered path under a
        // symlinked directory, and a ledgered directory that is a symlink.
        std::os::unix::fs::symlink(outside.path().join("victim"), h.join("link")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("dir"), h.join("dirlink")).unwrap();
        std::fs::create_dir(h.join("real")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("dir"), h.join("real/sub")).unwrap();
        let l = ledger(
            "x",
            &[h.join("link"), h.join("dirlink/inner")],
            &[h.join("real/sub"), h.join("dirlink")],
            0,
        );
        let report = wipe(&l, h, &FakeHost::new());
        assert_eq!(report.refused.len(), 4, "{:?}", report.refused);
        assert!(report.removed_paths.is_empty());
        assert!(outside.path().join("victim").exists());
        assert!(outside.path().join("dir/inner").exists());
        assert!(h.join("link").symlink_metadata().is_ok());
    }

    /// The Keyvault broker sends `expires_at_ms: 0` while auto-wipe is off:
    /// such an import never expires on its own, however late it is.
    #[test]
    fn a_zero_expiry_never_expires() {
        let forever = ledger("kv", &[], &[], 0);
        assert!(!forever.expired(0));
        assert!(!forever.expired(u64::MAX));
        let timed = ledger("kv", &[], &[], 5_000);
        assert!(!timed.expired(4_999));
        assert!(timed.expired(5_000));
    }

    #[test]
    fn sweep_wipes_expired_but_not_live_and_keeps_unreadable() {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let h = home.path();
        let store = LedgerStore::new(state.path().join("ledger"));
        for name in ["old", "live", "forever"] {
            std::fs::write(h.join(name), b"x").unwrap();
        }
        store
            .write(&ledger("old", &[h.join("old")], &[], 1_000))
            .unwrap();
        store
            .write(&ledger("live", &[h.join("live")], &[], 5_000))
            .unwrap();
        store
            .write(&ledger("forever", &[h.join("forever")], &[], 0))
            .unwrap();
        let garbage = store.root().join("garbage.json");
        std::fs::write(&garbage, b"{not json").unwrap();
        let (wiped, unreadable) = store.sweep_expired(2_000, h, &FakeHost::new());
        assert_eq!(wiped.len(), 1);
        assert_eq!(wiped[0].import_id, "old");
        assert_eq!(unreadable.len(), 1);
        assert_eq!(unreadable[0].path, garbage);
        assert!(garbage.exists(), "an unreadable ledger is never dropped");
        assert!(!h.join("old").exists());
        assert!(h.join("live").exists() && h.join("forever").exists());
        assert!(store.read("old").unwrap().is_none());
        assert!(store.read("live").unwrap().is_some());
    }
}
