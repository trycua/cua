// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The record layer: what the vault stores, and the codecs between it and an
//! app's native state.
//!
//! The vault holds one **record** per secret. A record is canonical JSON
//! with a versioned schema (`cookie@1`, `local_storage@1`, `password@1`,
//! `file@1`), kept decrypted inside the sealed item. Nothing about how an
//! app encrypts its own storage lives in a record: a cookie is its host,
//! name, path, value and attributes, not a `v10` blob keyed to one machine.
//!
//! A [`RecordCodec`] is the only thing that knows an app's native format:
//!
//! - [`RecordCodec::export`] turns the entries a provider read from the app
//!   (decrypted cookie rows, files) into records, and reports what it could
//!   not or must not export ([`Unavailable`]);
//! - [`RecordCodec::import`] turns records back into the entries the
//!   receiver installs. The receiver re-encrypts each cookie under the
//!   destination's own key (macOS `v10`, Linux `v10`/`v11`, Chrome 130's
//!   host-key digest), so a Chromium format change stays inside that code
//!   and never reaches the vault.
//!
//! Sending part of an app is filtering records. Everything above this
//! module (search, multi-select, locks, the review) works on records
//! without knowing which app they came from.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::model::{ItemKind, ItemMeta, ItemPayload, LoginRecord, PayloadEntry};
use crate::{Error, Result};

/// Cookie schema, version 1.
pub const COOKIE_V1: &str = "cookie@1";
/// localStorage value schema, version 1.
pub const LOCAL_STORAGE_V1: &str = "local_storage@1";
/// Saved password schema, version 1.
pub const PASSWORD_V1: &str = "password@1";
/// File schema, version 1.
pub const FILE_V1: &str = "file@1";

/// The native bundle entry that carries decrypted cookie rows (a JSON array;
/// `cua_teleport_bundle::cookies::COOKIES_ENTRY`).
pub const COOKIES_ENTRY: &str = "cookies.json";
/// The native bundle entry that carries localStorage values, a JSON array of
/// `{origin, key, value}`.
pub const LOCAL_STORAGE_ENTRY: &str = "localstorage.json";

/// The reserved bundle entry carrying saved passwords the user ticked, for
/// the receiver to re-encrypt under its own browser key
/// (`cua_teleport_bundle::logins::LOGINS_ENTRY`).
pub const LOGINS_ENTRY: &str = "logins.json";

/// Files larger than this are stored as content-addressed blobs and
/// referenced by hash, never inlined in the record's JSON.
pub const INLINE_FILE_LIMIT: usize = 64 * 1024;

/// The schema an item kind's records use.
pub fn schema_for(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Cookie => COOKIE_V1,
        ItemKind::LocalStorage => LOCAL_STORAGE_V1,
        ItemKind::Password => PASSWORD_V1,
        ItemKind::File => FILE_V1,
    }
}

/// The kind a schema belongs to (`None` for an unknown or newer schema).
pub fn kind_of_schema(schema: &str) -> Option<ItemKind> {
    [
        ItemKind::Cookie,
        ItemKind::LocalStorage,
        ItemKind::Password,
        ItemKind::File,
    ]
    .into_iter()
    .find(|k| schema_for(*k) == schema)
}

mod b64vec {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
}

/// A cookie, `cookie@1`. Lossless over what a browser stores: every field
/// beyond the first eight is optional because not every source reports it
/// (a reader that does not fill them leaves them out; a codec that can
/// write them back does). Keyed by `host_key` + `name` + `path`.
///
/// Fields are declared in alphabetical order: that is the canonical JSON
/// order, so equal cookies serialize to equal text.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieRecord {
    /// Creation time, Chrome microseconds since 1601 (kept when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_utc: Option<i64>,
    /// Expiry, Chrome microseconds since 1601; `0` is a session cookie.
    pub expires_utc: i64,
    /// Chrome's `has_cross_site_ancestor` (kept when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_cross_site_ancestor: Option<i64>,
    /// Host key, exactly as the browser stores it (`.github.com`).
    pub host_key: String,
    /// `HttpOnly`.
    pub http_only: bool,
    /// Chrome's `last_access_utc` (kept when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_access_utc: Option<i64>,
    /// Last update, Chrome microseconds since 1601 (kept when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update_utc: Option<i64>,
    /// Name.
    pub name: String,
    /// Partition key of a CHIPS (partitioned) cookie: the top-level site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_key: Option<String>,
    /// Path.
    pub path: String,
    /// Chrome's cookie priority (0 low, 1 medium, 2 high).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    /// Chrome's `samesite` (-1 unspecified, 0 none, 1 lax, 2 strict).
    pub same_site: i64,
    /// `Secure`.
    pub secure: bool,
    /// The port the cookie was set from (`-1`: unspecified).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_port: Option<i64>,
    /// Chrome's source scheme (0 unset, 1 non-secure, 2 secure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_scheme: Option<i64>,
    /// Chrome's `source_type` (0 unknown, 1 http, 2 script, 3 other).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<i64>,
    /// The value, decrypted.
    #[serde(with = "b64vec")]
    pub value: Vec<u8>,
}

impl CookieRecord {
    /// The item key: the cookie's name, and for a partitioned (CHIPS) cookie
    /// its partition too, so a partitioned cookie never collides with (and
    /// is never merged into) the same name set without a partition.
    pub fn key(&self) -> String {
        match self.partition_key.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => format!("{} (partitioned: {p})", self.name),
            None => self.name.clone(),
        }
    }
}

impl std::fmt::Debug for CookieRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieRecord")
            .field("host_key", &self.host_key)
            .field("name", &self.name)
            .field("path", &self.path)
            .field("value", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl Drop for CookieRecord {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl CookieRecord {
    /// Whether the cookie has no expiry.
    pub fn session(&self) -> bool {
        self.expires_utc == 0
    }

    /// The expiry in Unix ms (`None` for a session cookie).
    pub fn expires_ms(&self) -> Option<i64> {
        const CHROME_EPOCH_OFFSET_MS: i64 = 11_644_473_600_000;
        (self.expires_utc != 0).then_some(self.expires_utc / 1000 - CHROME_EPOCH_OFFSET_MS)
    }
}

/// A localStorage value, `local_storage@1`. Keyed by `origin` + `key`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalStorageRecord {
    /// The storage key.
    pub key: String,
    /// The origin (`https://github.com`).
    pub origin: String,
    /// The value.
    pub value: String,
    /// The exact stored key bytes (base64), only when `key` cannot hold them
    /// losslessly (a lone UTF-16 surrogate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_raw: Option<String>,
    /// The exact stored value bytes (base64), only when `value` cannot hold
    /// them losslessly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_raw: Option<String>,
}

impl std::fmt::Debug for LocalStorageRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalStorageRecord")
            .field("origin", &self.origin)
            .field("key", &self.key)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl Drop for LocalStorageRecord {
    fn drop(&mut self) {
        self.value.zeroize();
        if let Some(raw) = &mut self.value_raw {
            raw.zeroize();
        }
    }
}

/// A file, `file@1`. Keyed by its path. The bytes are inline (`content`,
/// base64) when small, or in a content-addressed encrypted blob (`blob`,
/// the SHA-256) when not; the JSON never carries a big file's bytes.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    /// SHA-256 of the blob holding the bytes (hex), for a big file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// The bytes, base64, for a small file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Unix mode.
    pub mode: u32,
    /// Path relative to the app's data.
    pub path: String,
    /// SHA-256 of the bytes (hex).
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
}

impl std::fmt::Debug for FileRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileRecord")
            .field("path", &self.path)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl Drop for FileRecord {
    fn drop(&mut self) {
        if let Some(c) = self.content.as_mut() {
            c.zeroize();
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(crate::crypto::sha256(bytes))
}

/// A record a codec exported, with the facts the vault indexes it by.
#[derive(Clone, Debug)]
pub struct NewRecord {
    /// Type.
    pub kind: ItemKind,
    /// Domain (none for a file).
    pub domain: Option<String>,
    /// Key (name, storage key, username or path).
    pub key: String,
    /// A cookie's path.
    pub path: Option<String>,
    /// A cookie without an expiry.
    pub session: bool,
    /// A cookie's expiry, Unix ms.
    pub expires_ms: Option<i64>,
    /// Payload bytes (for display).
    pub bytes: u64,
    /// The record: canonical JSON.
    pub record: String,
}

impl NewRecord {
    /// This record as an item to save.
    pub fn into_item(
        self,
        provider_id: &str,
        app_display: &str,
        source: &str,
        scope: &str,
    ) -> (ItemMeta, ItemPayload) {
        let mut meta = ItemMeta::draft(
            self.kind,
            provider_id,
            app_display,
            self.domain.as_deref(),
            &self.key,
        );
        meta.path = self.path;
        meta.session = self.session;
        meta.expires_ms = self.expires_ms;
        meta.bytes = self.bytes;
        meta.source = source.into();
        let payload = ItemPayload {
            provider_id: provider_id.into(),
            scope: scope.into(),
            schema: schema_for(self.kind).into(),
            record: self.record.clone(),
        };
        (meta, payload)
    }
}

/// Something a codec did not export, and why. Never silent: the review and
/// the vault list say so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unavailable {
    /// What it is (a path, or "N cookies").
    pub what: String,
    /// Why it was not exported.
    pub reason: String,
    /// It can be exported, but only after the user explicitly opts in (a
    /// consent-gated item), as opposed to not at all.
    pub needs_opt_in: bool,
}

/// What a codec exported.
#[derive(Debug, Default)]
pub struct Exported {
    /// The records.
    pub records: Vec<NewRecord>,
    /// What was left out.
    pub unavailable: Vec<Unavailable>,
}

/// What the user explicitly chose, for consent-gated items.
#[derive(Clone, Debug, Default)]
pub struct ExportOptions {
    /// Paths the user explicitly selected. A consent-gated path is exported
    /// only when it is named here.
    pub opted_in: Vec<String>,
}

/// An app's native format, behind one interface.
pub trait RecordCodec: Send + Sync {
    /// The codec's name.
    fn id(&self) -> &'static str;

    /// Native entries to records, reporting what was left out.
    fn export(&self, entries: Vec<PayloadEntry>, opts: &ExportOptions) -> Result<Exported>;

    /// Records back to the native entries the receiver installs. Passwords
    /// are only the reserved logins entry, and only when the user ticked
    /// them (the broker withholds them otherwise). Files that
    /// were stored as blobs must already be inlined (see
    /// [`crate::store::Vault::inline_blobs`]).
    fn import(&self, payloads: &[ItemPayload]) -> Result<Vec<PayloadEntry>>;
}

/// Cookie rows as a bundle carries them
/// (`cua_teleport_bundle::cookies::CookieItem`'s wire shape).
#[derive(Serialize, Deserialize)]
struct CookieRow {
    host_key: String,
    name: String,
    #[serde(with = "b64vec")]
    value: Vec<u8>,
    path: String,
    expires_utc: i64,
    is_secure: bool,
    is_httponly: bool,
    #[serde(default)]
    samesite: i64,
    #[serde(flatten)]
    extra: CookieExtra,
}

impl Drop for CookieRow {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

/// The cookie columns beyond the core ones, as the bundle carries them
/// (`cua_teleport_bundle::cookies::CookieExtra`, flattened).
#[derive(Default, Serialize, Deserialize)]
struct CookieExtra {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creation_utc: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_access_utc: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_update_utc: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_scheme: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_port: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_type: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    has_cross_site_ancestor: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    partition_key: Option<String>,
}

/// A saved login as a bundle carries it
/// (`cua_teleport_bundle::logins::LoginItem`'s wire shape).
#[derive(Serialize, Deserialize)]
struct LoginRow {
    origin: String,
    username: String,
    #[serde(with = "b64vec")]
    password: Vec<u8>,
    signon_realm: String,
}

impl Drop for LoginRow {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
struct StorageRow {
    origin: String,
    key: String,
    value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_raw: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value_raw: Option<String>,
}

impl Drop for StorageRow {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn entry_bytes(e: &PayloadEntry) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    Ok(zeroize::Zeroizing::new(b64().decode(&e.data).map_err(
        |_| Error::Corrupt(format!("{} is not base64", e.rel_path)),
    )?))
}

/// A cookie record, ready to save.
pub fn cookie_record(c: &CookieRecord) -> Result<NewRecord> {
    Ok(NewRecord {
        kind: ItemKind::Cookie,
        domain: Some(c.host_key.clone()),
        key: c.key(),
        path: Some(c.path.clone()),
        session: c.session(),
        expires_ms: c.expires_ms(),
        bytes: c.value.len() as u64,
        record: serde_json::to_string(c)?,
    })
}

/// A localStorage record, ready to save.
pub fn local_storage_record(v: &LocalStorageRecord) -> Result<NewRecord> {
    Ok(NewRecord {
        kind: ItemKind::LocalStorage,
        domain: Some(v.origin.clone()),
        key: v.key.clone(),
        path: None,
        session: false,
        expires_ms: None,
        bytes: v.value.len() as u64,
        record: serde_json::to_string(v)?,
    })
}

/// A saved-password record, ready to save.
pub fn password_record(l: &LoginRecord) -> Result<NewRecord> {
    Ok(NewRecord {
        kind: ItemKind::Password,
        domain: Some(l.origin.clone()),
        key: l.username.clone(),
        path: None,
        session: false,
        expires_ms: None,
        bytes: l.password.len() as u64,
        record: serde_json::to_string(l)?,
    })
}

/// A file record, ready to save (the bytes inline; the vault moves a big
/// file's bytes into a blob when it stores it).
pub fn file_record(path: &str, mode: u32, bytes: &[u8]) -> Result<NewRecord> {
    let f = FileRecord {
        blob: None,
        content: Some(b64().encode(bytes)),
        mode,
        path: path.into(),
        sha256: sha256_hex(bytes),
        size: bytes.len() as u64,
    };
    Ok(NewRecord {
        kind: ItemKind::File,
        domain: None,
        key: path.into(),
        path: None,
        session: false,
        expires_ms: None,
        bytes: bytes.len() as u64,
        record: serde_json::to_string(&f)?,
    })
}

/// The codec every app gets: cookie rows and localStorage rows become
/// records, every other entry becomes a file record.
pub struct GenericCodec;

impl RecordCodec for GenericCodec {
    fn id(&self) -> &'static str {
        "generic"
    }

    fn export(&self, entries: Vec<PayloadEntry>, _opts: &ExportOptions) -> Result<Exported> {
        let mut out = Exported::default();
        for e in &entries {
            match e.rel_path.as_str() {
                COOKIES_ENTRY => {
                    let bytes = entry_bytes(e)?;
                    let rows: Vec<CookieRow> = serde_json::from_slice(&bytes)
                        .map_err(|_| Error::Corrupt("cookies.json is not valid".into()))?;
                    for r in &rows {
                        out.records.push(cookie_record(&CookieRecord {
                            creation_utc: r.extra.creation_utc,
                            expires_utc: r.expires_utc,
                            has_cross_site_ancestor: r.extra.has_cross_site_ancestor,
                            host_key: r.host_key.clone(),
                            http_only: r.is_httponly,
                            last_access_utc: r.extra.last_access_utc,
                            last_update_utc: r.extra.last_update_utc,
                            name: r.name.clone(),
                            partition_key: r.extra.partition_key.clone().filter(|k| !k.is_empty()),
                            path: r.path.clone(),
                            priority: r.extra.priority,
                            same_site: r.samesite,
                            secure: r.is_secure,
                            source_port: r.extra.source_port,
                            source_scheme: r.extra.source_scheme,
                            source_type: r.extra.source_type,
                            value: r.value.clone(),
                        })?);
                    }
                }
                LOCAL_STORAGE_ENTRY => {
                    let bytes = entry_bytes(e)?;
                    let rows: Vec<StorageRow> = serde_json::from_slice(&bytes)
                        .map_err(|_| Error::Corrupt("localstorage.json is not valid".into()))?;
                    for r in &rows {
                        out.records.push(local_storage_record(&LocalStorageRecord {
                            key: r.key.clone(),
                            origin: r.origin.clone(),
                            value: r.value.clone(),
                            key_raw: r.key_raw.clone(),
                            value_raw: r.value_raw.clone(),
                        })?);
                    }
                }
                path => {
                    let bytes = entry_bytes(e)?;
                    out.records.push(file_record(path, e.mode, &bytes)?);
                }
            }
        }
        Ok(out)
    }

    fn import(&self, payloads: &[ItemPayload]) -> Result<Vec<PayloadEntry>> {
        let mut cookies: Vec<CookieRow> = Vec::new();
        let mut storage: Vec<StorageRow> = Vec::new();
        let mut logins: Vec<LoginRow> = Vec::new();
        let mut files: Vec<PayloadEntry> = Vec::new();
        for p in payloads {
            match p.schema.as_str() {
                COOKIE_V1 => {
                    let c: CookieRecord = serde_json::from_str(&p.record)
                        .map_err(|_| Error::Corrupt("a cookie record is not valid".into()))?;
                    cookies.push(CookieRow {
                        host_key: c.host_key.clone(),
                        name: c.name.clone(),
                        value: c.value.clone(),
                        path: c.path.clone(),
                        expires_utc: c.expires_utc,
                        is_secure: c.secure,
                        is_httponly: c.http_only,
                        samesite: c.same_site,
                        extra: CookieExtra {
                            creation_utc: c.creation_utc,
                            last_access_utc: c.last_access_utc,
                            last_update_utc: c.last_update_utc,
                            priority: c.priority,
                            source_scheme: c.source_scheme,
                            source_port: c.source_port,
                            source_type: c.source_type,
                            has_cross_site_ancestor: c.has_cross_site_ancestor,
                            partition_key: c.partition_key.clone(),
                        },
                    });
                }
                LOCAL_STORAGE_V1 => {
                    let v: LocalStorageRecord = serde_json::from_str(&p.record)
                        .map_err(|_| Error::Corrupt("a localStorage record is not valid".into()))?;
                    storage.push(StorageRow {
                        origin: v.origin.clone(),
                        key: v.key.clone(),
                        value: v.value.clone(),
                        key_raw: v.key_raw.clone(),
                        value_raw: v.value_raw.clone(),
                    });
                }
                FILE_V1 => {
                    let f: FileRecord = serde_json::from_str(&p.record)
                        .map_err(|_| Error::Corrupt("a file record is not valid".into()))?;
                    let content = f.content.clone().ok_or_else(|| {
                        Error::Corrupt(format!(
                            "{} has no bytes to deliver (blob not inlined)",
                            f.path
                        ))
                    })?;
                    let entry = PayloadEntry {
                        rel_path: f.path.clone(),
                        mode: f.mode,
                        data: content,
                    };
                    // A file named twice is written once; the later one wins.
                    match files.iter_mut().find(|x| x.rel_path == entry.rel_path) {
                        Some(slot) => *slot = entry,
                        None => files.push(entry),
                    }
                }
                // A saved login is here only because the user ticked it (the
                // broker withholds passwords otherwise): it rides in its own
                // reserved entry, re-encrypted by the receiver.
                PASSWORD_V1 => {
                    let l: LoginRecord = serde_json::from_str(&p.record)
                        .map_err(|_| Error::Corrupt("a password record is not valid".into()))?;
                    logins.push(LoginRow {
                        origin: l.origin.clone(),
                        username: l.username.clone(),
                        password: l.password.as_bytes().to_vec(),
                        signon_realm: format!("{}/", l.origin.trim_end_matches('/')),
                    });
                }
                other => {
                    return Err(Error::Unsupported(format!(
                        "this build cannot deliver {other} records"
                    )));
                }
            }
        }
        // Rows already carry a `value` the receiver re-encrypts; join them.
        if !cookies.is_empty() {
            let json = zeroize::Zeroizing::new(serde_json::to_vec(&cookies)?);
            files.push(PayloadEntry {
                rel_path: COOKIES_ENTRY.into(),
                mode: 0o600,
                data: b64().encode(json.as_slice()),
            });
        }
        if !logins.is_empty() {
            let json = zeroize::Zeroizing::new(serde_json::to_vec(&logins)?);
            files.push(PayloadEntry {
                rel_path: LOGINS_ENTRY.into(),
                mode: 0o600,
                data: b64().encode(json.as_slice()),
            });
        }
        if !storage.is_empty() {
            let json = zeroize::Zeroizing::new(serde_json::to_vec(&storage)?);
            files.push(PayloadEntry {
                rel_path: LOCAL_STORAGE_ENTRY.into(),
                mode: 0o600,
                data: b64().encode(json.as_slice()),
            });
        }
        Ok(files)
    }
}

/// Chromium-family browsers (Chrome, Brave, Edge, Arc, Vivaldi): the generic
/// codec, withholding what must not leave without the user's explicit say.
pub struct ChromiumCodec;

/// Chromium files that are never exported unless the user explicitly picked
/// them, with why.
const CHROMIUM_GATED: [(&str, &str); 2] = [
    (
        "Web Data",
        "holds the browser's sign-in refresh tokens, which outlive cookies and grant account access",
    ),
    (
        "Login Data",
        "its passwords are encrypted for this Mac; saved passwords are kept as password items instead",
    ),
];

fn gated_reason(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    CHROMIUM_GATED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, why)| *why)
}

impl RecordCodec for ChromiumCodec {
    fn id(&self) -> &'static str {
        "chromium"
    }

    fn export(&self, entries: Vec<PayloadEntry>, opts: &ExportOptions) -> Result<Exported> {
        let mut unavailable = Vec::new();
        let kept: Vec<PayloadEntry> = entries
            .into_iter()
            .filter(|e| match gated_reason(&e.rel_path) {
                Some(why) if !opts.opted_in.iter().any(|p| p == &e.rel_path) => {
                    unavailable.push(Unavailable {
                        what: e.rel_path.clone(),
                        reason: why.into(),
                        needs_opt_in: true,
                    });
                    false
                }
                _ => true,
            })
            .collect();
        let mut out = GenericCodec.export(kept, opts)?;
        out.unavailable.extend(unavailable);
        Ok(out)
    }

    fn import(&self, payloads: &[ItemPayload]) -> Result<Vec<PayloadEntry>> {
        GenericCodec.import(payloads)
    }
}

/// The codec for a provider id.
pub fn codec_for(provider_id: &str) -> Box<dyn RecordCodec> {
    match provider_id {
        "chrome" | "chromium" | "brave" | "edge" | "arc" | "vivaldi" | "opera" => {
            Box::new(ChromiumCodec)
        }
        _ => Box::new(GenericCodec),
    }
}

/// Whether a cookie name looks like it keeps a sign-in (a session, an
/// authentication token), as opposed to analytics or preferences. It picks
/// the default for a minimal teleport: the sites that need to stay signed
/// in. A heuristic; the user reviews and changes the selection.
pub fn looks_like_signin(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const NOT: [&str; 8] = [
        "_ga",
        "_gid",
        "_gat",
        "_fbp",
        "__utm",
        "_hj",
        "optimizely",
        "ajs_",
    ];
    if NOT.iter().any(|p| n.starts_with(p)) {
        return false;
    }
    const YES: [&str; 11] = [
        "session",
        "sess",
        "sid",
        "auth",
        "token",
        "login",
        "logged_in",
        "jwt",
        "csrf",
        "user",
        "secure",
    ];
    YES.iter()
        .any(|y| n == *y || n.contains(y) && !n.contains("pref") && !n.contains("theme"))
}

/// The host of a cookie domain, an origin or a host: scheme, credentials,
/// port and path are dropped, a leading dot too, and it is lower-cased
/// (`https://App.GitHub.com:8443/x` and `.app.github.com` are
/// `app.github.com`).
pub fn host_of(domain: &str) -> String {
    let d = domain.trim();
    let d = d.split_once("://").map_or(d, |(_, r)| r);
    let d = d.split(['/', '?', '#']).next().unwrap_or(d);
    let d = d.rsplit_once('@').map_or(d, |(_, r)| r);
    let d = match d.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) && !h.contains(':') => h,
        _ => d,
    };
    d.trim_start_matches('.').to_ascii_lowercase()
}

/// A conservative registrable domain for a cookie host, an origin or a host:
/// the last two labels of [`host_of`] (three under a few known two-label
/// suffixes). Used to group items by site; the grouping never decides
/// access.
pub fn site_of(domain: &str) -> String {
    const MULTI: [&str; 10] = [
        "co.uk", "org.uk", "gov.uk", "ac.uk", "co.jp", "com.au", "com.br", "co.nz", "co.in",
        "com.cn",
    ];
    let host = host_of(domain);
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 || host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return host;
    }
    let last_two = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
    if MULTI.contains(&last_two.as_str()) {
        return format!("{}.{last_two}", labels[labels.len() - 3]);
    }
    last_two
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(host: &str, name: &str, path: &str, value: &[u8]) -> CookieRecord {
        CookieRecord {
            creation_utc: None,
            expires_utc: 13_397_000_000_000_000,
            host_key: host.into(),
            http_only: true,
            last_update_utc: None,
            name: name.into(),
            partition_key: None,
            last_access_utc: None,
            source_type: None,
            has_cross_site_ancestor: None,
            path: path.into(),
            priority: None,
            same_site: 1,
            secure: true,
            source_port: None,
            source_scheme: None,
            value: value.to_vec(),
        }
    }

    fn entry(path: &str, data: &[u8]) -> PayloadEntry {
        PayloadEntry {
            rel_path: path.into(),
            mode: 0o600,
            data: b64().encode(data),
        }
    }

    // ---- golden JSON: the schemas are a stored format ----

    #[test]
    fn cookie_v1_golden() {
        let c = cookie(".github.com", "user_session", "/", b"s3cret");
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            r#"{"expires_utc":13397000000000000,"host_key":".github.com","http_only":true,"name":"user_session","path":"/","same_site":1,"secure":true,"value":"czNjcmV0"}"#
        );
        let mut full = c.clone();
        full.creation_utc = Some(1);
        full.last_update_utc = Some(2);
        full.partition_key = Some("https://top.example".into());
        full.priority = Some(2);
        full.source_port = Some(443);
        full.source_scheme = Some(2);
        full.source_type = Some(1);
        full.last_access_utc = Some(3);
        full.has_cross_site_ancestor = Some(1);
        assert_eq!(
            serde_json::to_string(&full).unwrap(),
            r#"{"creation_utc":1,"expires_utc":13397000000000000,"has_cross_site_ancestor":1,"host_key":".github.com","http_only":true,"last_access_utc":3,"last_update_utc":2,"name":"user_session","partition_key":"https://top.example","path":"/","priority":2,"same_site":1,"secure":true,"source_port":443,"source_scheme":2,"source_type":1,"value":"czNjcmV0"}"#
        );
        // A partitioned cookie is its own item, never merged into the same
        // name without a partition.
        assert_eq!(c.key(), "user_session");
        assert_eq!(
            full.key(),
            "user_session (partitioned: https://top.example)"
        );
        let back: CookieRecord =
            serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
        assert_eq!(back, full);
    }

    #[test]
    fn local_storage_v1_golden() {
        let v = LocalStorageRecord {
            key: "token".into(),
            origin: "https://app.example".into(),
            value: "abc".into(),
            key_raw: None,
            value_raw: None,
        };
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"key":"token","origin":"https://app.example","value":"abc"}"#
        );
        // A value that is not valid text keeps its exact stored bytes.
        let raw = LocalStorageRecord {
            key: "bad".into(),
            origin: "https://app.example".into(),
            value: "\u{fffd}".into(),
            key_raw: None,
            value_raw: Some("AAA=".into()),
        };
        assert_eq!(
            serde_json::to_string(&raw).unwrap(),
            "{\"key\":\"bad\",\"origin\":\"https://app.example\",\"value\":\"\u{fffd}\",\"value_raw\":\"AAA=\"}"
        );
    }

    #[test]
    fn password_v1_golden() {
        let l = LoginRecord {
            origin: "https://github.com".into(),
            username: "octo".into(),
            password: "pw".into(),
        };
        assert_eq!(
            serde_json::to_string(&l).unwrap(),
            r#"{"origin":"https://github.com","username":"octo","password":"pw"}"#
        );
    }

    #[test]
    fn file_v1_golden_inline_and_blob() {
        let n = file_record("Default/Bookmarks", 0o600, b"{}").unwrap();
        assert_eq!(
            n.record,
            r#"{"content":"e30=","mode":384,"path":"Default/Bookmarks","sha256":"44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a","size":2}"#
        );
        let blob = FileRecord {
            blob: Some("ab".repeat(32)),
            content: None,
            mode: 0o600,
            path: "Default/History".into(),
            sha256: "ab".repeat(32),
            size: 9_999_999,
        };
        assert_eq!(
            serde_json::to_string(&blob).unwrap(),
            format!(
                r#"{{"blob":"{h}","mode":384,"path":"Default/History","sha256":"{h}","size":9999999}}"#,
                h = "ab".repeat(32)
            )
        );
    }

    #[test]
    fn schemas_and_kinds_agree() {
        for k in [
            ItemKind::Cookie,
            ItemKind::LocalStorage,
            ItemKind::Password,
            ItemKind::File,
        ] {
            assert_eq!(kind_of_schema(schema_for(k)), Some(k));
        }
        assert_eq!(
            kind_of_schema("cookie@2"),
            None,
            "a newer schema is not guessed at"
        );
    }

    // ---- codec round trips ----

    fn payload_of(n: NewRecord) -> ItemPayload {
        let (_, p) = n.into_item("chrome", "Chrome", "Default", "full");
        p
    }

    #[test]
    fn export_then_import_round_trips_cookies_and_files() {
        let rows = serde_json::json!([
            {"host_key": ".github.com", "name": "a", "value": "djE=", "path": "/", "expires_utc": 0,
             "is_secure": true, "is_httponly": false, "samesite": -1},
            {"host_key": "api.github.com", "name": "b", "value": "djI=", "path": "/v3", "expires_utc": 5,
             "is_secure": false, "is_httponly": true, "samesite": 2}
        ]);
        let exported = ChromiumCodec
            .export(
                vec![
                    entry(COOKIES_ENTRY, rows.to_string().as_bytes()),
                    entry("Default/Bookmarks", b"{}"),
                    entry("Local State", b"ls"),
                ],
                &ExportOptions::default(),
            )
            .unwrap();
        assert!(exported.unavailable.is_empty());
        let kinds: Vec<_> = exported.records.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ItemKind::Cookie,
                ItemKind::Cookie,
                ItemKind::File,
                ItemKind::File
            ]
        );
        assert_eq!(exported.records[0].domain.as_deref(), Some(".github.com"));
        assert_eq!(exported.records[1].path.as_deref(), Some("/v3"));
        assert!(exported.records[0].session && !exported.records[1].session);

        let payloads: Vec<ItemPayload> = exported.records.into_iter().map(payload_of).collect();
        let entries = ChromiumCodec.import(&payloads).unwrap();
        let by = |p: &str| {
            entries
                .iter()
                .find(|e| e.rel_path == p)
                .map(|e| b64().decode(&e.data).unwrap())
        };
        let back: serde_json::Value = serde_json::from_slice(&by(COOKIES_ENTRY).unwrap()).unwrap();
        assert_eq!(back, rows, "the receiver gets the rows the provider read");
        assert_eq!(by("Default/Bookmarks").unwrap(), b"{}");
        assert_eq!(by("Local State").unwrap(), b"ls");
    }

    /// Native entries (the bundle's JSON rows) to records and back: every
    /// cookie attribute and every localStorage value, including one whose
    /// bytes are not valid text, comes back as it went in.
    #[test]
    fn every_cookie_attribute_and_localstorage_value_round_trips_native_to_record_and_back() {
        let cookies = serde_json::json!([
            {"host_key": ".example.com", "name": "sid", "value": "djE=", "path": "/",
             "expires_utc": 13_400_000_000_000_000i64, "is_secure": true, "is_httponly": true,
             "samesite": 1, "creation_utc": 11, "last_access_utc": 12, "last_update_utc": 13,
             "priority": 2, "source_scheme": 2, "source_port": 443, "source_type": 1,
             "has_cross_site_ancestor": 1, "partition_key": "https://top.example"},
            {"host_key": ".example.com", "name": "sid", "value": "djI=", "path": "/",
             "expires_utc": 0, "is_secure": false, "is_httponly": false, "samesite": -1}
        ]);
        let storage = serde_json::json!([
            {"origin": "https://github.com", "key": "color_mode", "value": "dark"},
            {"origin": "https://a.example", "key": "日本", "value": "こんにちは \u{1F642}"},
            {"origin": "https://a.example", "key": "bad", "value": "\u{fffd}", "value_raw": "AAAA2A=="}
        ]);
        let exported = ChromiumCodec
            .export(
                vec![
                    entry(COOKIES_ENTRY, cookies.to_string().as_bytes()),
                    entry(LOCAL_STORAGE_ENTRY, storage.to_string().as_bytes()),
                ],
                &ExportOptions::default(),
            )
            .unwrap();
        // The two cookies are two items: one partitioned, one not.
        let keys: Vec<(ItemKind, String)> = exported
            .records
            .iter()
            .map(|r| (r.kind, r.key.clone()))
            .collect();
        assert!(keys.contains(&(ItemKind::Cookie, "sid".into())));
        assert!(keys.contains(&(
            ItemKind::Cookie,
            "sid (partitioned: https://top.example)".into()
        )));
        assert_eq!(
            exported
                .records
                .iter()
                .filter(|r| r.kind == ItemKind::LocalStorage)
                .count(),
            3
        );
        let payloads: Vec<ItemPayload> = exported.records.into_iter().map(payload_of).collect();
        let entries = ChromiumCodec.import(&payloads).unwrap();
        let by = |p: &str| -> serde_json::Value {
            let e = entries.iter().find(|e| e.rel_path == p).unwrap();
            serde_json::from_slice(&b64().decode(&e.data).unwrap()).unwrap()
        };
        let sorted = |mut v: serde_json::Value, k: &[&str]| {
            v.as_array_mut().unwrap().sort_by_key(|x| {
                k.iter()
                    .map(|f| x[*f].as_str().unwrap_or_default().to_string())
                    .collect::<Vec<_>>()
            });
            v
        };
        assert_eq!(
            sorted(by(COOKIES_ENTRY), &["host_key", "name", "partition_key"]),
            sorted(cookies, &["host_key", "name", "partition_key"]),
            "the receiver gets every attribute the source had"
        );
        assert_eq!(
            sorted(by(LOCAL_STORAGE_ENTRY), &["origin", "key"]),
            sorted(storage, &["origin", "key"])
        );
    }

    #[test]
    fn selective_send_is_filtering_records_and_files_dedupe() {
        let mk = |h: &str| payload_of(cookie_record(&cookie(h, "sid", "/", b"v")).unwrap());
        let all = [mk(".a.test"), mk(".b.test"), mk(".c.test")];
        let chosen: Vec<ItemPayload> = all
            .iter()
            .filter(|p| p.record.contains(".a.test") || p.record.contains(".c.test"))
            .cloned()
            .collect();
        let entries = GenericCodec.import(&chosen).unwrap();
        let json: Vec<serde_json::Value> =
            serde_json::from_slice(&b64().decode(&entries[0].data).unwrap()).unwrap();
        assert_eq!(json.len(), 2);
        assert!(json.iter().all(|r| r["host_key"] != ".b.test"));

        let twice = [
            payload_of(file_record("x", 0o600, b"old").unwrap()),
            payload_of(file_record("x", 0o600, b"new").unwrap()),
        ];
        let entries = GenericCodec.import(&twice).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(b64().decode(&entries[0].data).unwrap(), b"new");
    }

    #[test]
    fn a_password_is_only_the_reserved_logins_entry_and_unknown_schemas_are_refused() {
        let pw = ItemPayload::password(
            "chrome",
            &LoginRecord {
                origin: "https://github.com".into(),
                username: "octo".into(),
                password: "pw".into(),
            },
        )
        .unwrap();
        // Never a file of app state: only the reserved entry the receiver
        // re-encrypts (the broker passes a password here only when the user
        // ticked it).
        let entries = GenericCodec.import(&[pw]).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel_path, LOGINS_ENTRY);
        let rows: serde_json::Value =
            serde_json::from_slice(&b64().decode(&entries[0].data).unwrap()).unwrap();
        assert_eq!(rows[0]["username"], "octo");
        assert_eq!(rows[0]["signon_realm"], "https://github.com/");
        assert_eq!(rows[0]["password"], b64().encode(b"pw"));
        let newer = ItemPayload {
            provider_id: "chrome".into(),
            scope: "full".into(),
            schema: "cookie@2".into(),
            record: "{}".into(),
        };
        assert!(matches!(
            GenericCodec.import(&[newer]),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn consent_gated_files_are_withheld_unless_explicitly_chosen() {
        let entries = || {
            vec![
                entry("Default/Web Data", b"tokens"),
                entry("Default/Login Data", b"pw"),
                entry("Default/Bookmarks", b"{}"),
            ]
        };
        let out = ChromiumCodec
            .export(entries(), &ExportOptions::default())
            .unwrap();
        assert_eq!(out.records.len(), 1, "only the bookmarks");
        assert_eq!(out.unavailable.len(), 2);
        assert!(out.unavailable.iter().all(|u| u.needs_opt_in));
        assert!(out.unavailable[0].reason.contains("refresh tokens"));
        // The user picked Web Data themselves: it is exported.
        let out = ChromiumCodec
            .export(
                entries(),
                &ExportOptions {
                    opted_in: vec!["Default/Web Data".into()],
                },
            )
            .unwrap();
        assert_eq!(out.records.len(), 2);
        assert_eq!(out.unavailable.len(), 1);
        // The generic codec has no gates.
        assert_eq!(
            GenericCodec
                .export(entries(), &ExportOptions::default())
                .unwrap()
                .records
                .len(),
            3
        );
        assert_eq!(codec_for("brave").id(), "chromium");
        assert_eq!(codec_for("slack").id(), "generic");
    }

    #[test]
    fn garbled_cookie_rows_are_an_error_not_a_silent_drop() {
        assert!(
            GenericCodec
                .export(
                    vec![entry(COOKIES_ENTRY, b"not json")],
                    &ExportOptions::default()
                )
                .is_err()
        );
    }

    #[test]
    fn debug_never_prints_a_value() {
        let c = cookie(".a.test", "sid", "/", b"VERY-SECRET");
        assert!(!format!("{c:?}").contains("VERY-SECRET"));
        let p = payload_of(cookie_record(&c).unwrap());
        assert!(
            !format!("{p:?}").contains("czNjcmV0") && !format!("{p:?}").contains("VkVSWS1TRUNSRVQ")
        );
    }

    #[test]
    fn signin_cookies_are_told_from_analytics() {
        for yes in [
            "user_session",
            "_gh_sess",
            "SID",
            "auth_token",
            "logged_in",
            "__Secure-1PSID",
            "JSESSIONID",
            "token_v2",
        ] {
            assert!(looks_like_signin(yes), "{yes}");
        }
        for no in [
            "_ga",
            "_gid",
            "_fbp",
            "__utma",
            "ajs_anonymous_id",
            "theme",
            "NID_pref",
            "lang",
        ] {
            assert!(!looks_like_signin(no), "{no}");
        }
    }

    #[test]
    fn site_of_groups_hosts_origins_and_cookie_domains() {
        assert_eq!(site_of(".github.com"), "github.com");
        assert_eq!(site_of("api.github.com"), "github.com");
        assert_eq!(site_of("https://gist.github.com:8443/x"), "github.com");
        assert_eq!(site_of("www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_of("localhost"), "localhost");
        assert_eq!(site_of("127.0.0.1"), "127.0.0.1");
        assert_eq!(site_of("http://127.0.0.1:8000"), "127.0.0.1");
    }
}
