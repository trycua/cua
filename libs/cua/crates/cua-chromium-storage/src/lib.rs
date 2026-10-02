// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Chromium localStorage, read and written as items.
//!
//! A Chromium-family browser keeps `localStorage` in a LevelDB directory,
//! `<profile>/Local Storage/leveldb`. Its keys are
//!
//! ```text
//! VERSION                       "1"
//! META:<origin>                 protobuf { last_modified, size_bytes }
//! METAACCESS:<origin>           protobuf { last_accessed }
//! _<origin> 0x00 <fmt> <key>    the value:  <fmt> <bytes>
//! ```
//!
//! where `<fmt>` is `0x01` for Latin-1 text and `0x00` for UTF-16LE, in keys
//! and in values alike. [`read`] turns the data keys into
//! [`LocalStorageItem`]s from a private copy of the directory (so a running
//! browser's files and lock are never touched); [`write`] puts items back
//! into a destination directory **while that browser is not running**
//! (LevelDB takes an exclusive lock, and a running browser holds it), adding
//! `META:` for an origin that has none and `VERSION` for a new database;
//! [`delete`] takes written keys out again.
//!
//! Nothing here reads or writes anything outside the directory it is given.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use rusty_leveldb::{DB, LdbIterator, Options};

pub use cua_teleport_bundle::local_storage::{LOCAL_STORAGE_ENTRY, LocalStorageItem};

/// Why a read or write failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The directory is missing or unreadable.
    #[error("localStorage store: {0}")]
    Io(String),
    /// LevelDB refused (damaged files, or a running browser holds the lock).
    #[error("localStorage database: {0}")]
    Db(String),
    /// The store is bigger than this build reads into memory.
    #[error("localStorage is {0} MiB, more than the {1} MiB this build reads")]
    TooLarge(u64, u64),
}

/// A result.
pub type Result<T> = std::result::Result<T, Error>;

/// The most a read copies and holds (the database's files, in MiB).
pub const MAX_STORE_MIB: u64 = 256;

/// `<profile>/Local Storage/leveldb`.
pub fn store_dir(profile_dir: &Path) -> PathBuf {
    profile_dir.join("Local Storage").join("leveldb")
}

const DATA_PREFIX: u8 = b'_';
const META_PREFIX: &[u8] = b"META:";
const VERSION_KEY: &[u8] = b"VERSION";

fn db_err(e: rusty_leveldb::Status) -> Error {
    Error::Db(e.to_string())
}

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Text stored the way Chromium stores it: a format byte (`0x01` Latin-1,
/// `0x00` UTF-16LE) then the characters. Returns the text and whether it is
/// the exact content (a lone surrogate, or an unknown format byte, is not).
fn decode_text(stored: &[u8]) -> (String, bool) {
    match stored.split_first() {
        Some((1, rest)) => (rest.iter().map(|&b| b as char).collect(), true),
        Some((0, rest)) if rest.len() % 2 == 0 => {
            let units: Vec<u16> = rest
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            match String::from_utf16(&units) {
                Ok(s) => (s, true),
                Err(_) => (String::from_utf16_lossy(&units), false),
            }
        }
        Some((_, rest)) => (String::from_utf8_lossy(rest).into_owned(), false),
        None => (String::new(), true),
    }
}

/// The inverse of [`decode_text`]: Latin-1 when every character fits (as
/// Chromium writes it), else UTF-16LE.
fn encode_text(text: &str) -> Vec<u8> {
    if text.chars().all(|c| (c as u32) < 256) {
        let mut out = Vec::with_capacity(1 + text.len());
        out.push(1);
        out.extend(text.chars().map(|c| c as u32 as u8));
        out
    } else {
        let mut out = vec![0u8];
        for u in text.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out
    }
}

/// `_<origin>\0<fmt><key>` for an item (its exact stored key bytes when it
/// carries them).
fn data_key(item: &LocalStorageItem) -> Vec<u8> {
    let mut k = Vec::with_capacity(item.origin.len() + item.key.len() + 4);
    k.push(DATA_PREFIX);
    k.extend_from_slice(item.origin.as_bytes());
    k.push(0);
    match item.key_raw.as_deref().and_then(|r| b64().decode(r).ok()) {
        Some(raw) => k.extend_from_slice(&raw),
        None => k.extend_from_slice(&encode_text(&item.key)),
    }
    k
}

fn value_bytes(item: &LocalStorageItem) -> Vec<u8> {
    match item.value_raw.as_deref().and_then(|r| b64().decode(r).ok()) {
        Some(raw) => raw,
        None => encode_text(&item.value),
    }
}

/// Parses a data key: the origin and the stored key bytes (format byte
/// included). `None` for `VERSION`, `META:`, `METAACCESS:` and anything else.
fn parse_data_key(k: &[u8]) -> Option<(String, &[u8])> {
    let rest = k.strip_prefix(&[DATA_PREFIX])?;
    let nul = rest.iter().position(|&b| b == 0)?;
    let origin = std::str::from_utf8(&rest[..nul]).ok()?;
    Some((origin.to_string(), &rest[nul + 1..]))
}

/// A private copy of `dir` (every file but `LOCK`), removed on drop.
fn private_copy(dir: &Path) -> Result<tempfile::TempDir> {
    let mut total = 0u64;
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))? {
        let e = e.map_err(|e| Error::Io(e.to_string()))?;
        let meta = e.metadata().map_err(|e| Error::Io(e.to_string()))?;
        if meta.is_file() && e.file_name() != "LOCK" {
            total += meta.len();
            files.push(e.path());
        }
    }
    if total > MAX_STORE_MIB * 1024 * 1024 {
        return Err(Error::TooLarge(total / (1024 * 1024), MAX_STORE_MIB));
    }
    let tmp = tempfile::Builder::new()
        .prefix("cua-localstorage-")
        .tempdir()
        .map_err(|e| Error::Io(e.to_string()))?;
    for f in files {
        let name = f.file_name().unwrap_or_default();
        std::fs::copy(&f, tmp.path().join(name)).map_err(|e| Error::Io(e.to_string()))?;
    }
    Ok(tmp)
}

/// Every localStorage value in `dir` (a `Local Storage/leveldb` directory),
/// read from a private copy. A directory that is not there is no items.
pub fn read(dir: &Path) -> Result<Vec<LocalStorageItem>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let copy = private_copy(dir)?;
    let opts = Options {
        create_if_missing: false,
        ..Options::default()
    };
    let mut db = DB::open(copy.path(), opts).map_err(db_err)?;
    let mut it = db.new_iter().map_err(db_err)?;
    let mut out = Vec::new();
    while let Some((k, v)) = it.next() {
        let Some((origin, key_bytes)) = parse_data_key(&k) else {
            continue;
        };
        let (key, key_exact) = decode_text(key_bytes);
        let (value, value_exact) = decode_text(&v);
        out.push(LocalStorageItem {
            origin,
            key,
            value,
            key_raw: (!key_exact).then(|| b64().encode(key_bytes)),
            value_raw: (!value_exact).then(|| b64().encode(&v)),
        });
    }
    drop(it);
    let _ = db.close();
    Ok(out)
}

/// How many values each origin holds, without decoding any (what a review
/// shows before anything is saved). Read from a private copy.
pub fn count_by_origin(dir: &Path) -> Result<std::collections::BTreeMap<String, u32>> {
    let mut counts = std::collections::BTreeMap::new();
    if !dir.is_dir() {
        return Ok(counts);
    }
    let copy = private_copy(dir)?;
    let opts = Options {
        create_if_missing: false,
        ..Options::default()
    };
    let mut db = DB::open(copy.path(), opts).map_err(db_err)?;
    let mut it = db.new_iter().map_err(db_err)?;
    while let Some((k, _)) = it.next() {
        if let Some((origin, _)) = parse_data_key(&k) {
            *counts.entry(origin).or_insert(0) += 1;
        }
    }
    drop(it);
    let _ = db.close();
    Ok(counts)
}

fn varint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// `META:` value: `last_modified` (field 1, Chrome microseconds since 1601)
/// and `size_bytes` (field 2).
fn meta_value(last_modified: i64, size_bytes: u64) -> Vec<u8> {
    let mut out = vec![0x08];
    varint(last_modified.max(0) as u64, &mut out);
    out.push(0x10);
    varint(size_bytes, &mut out);
    out
}

/// What [`write`] put in, so a wipe can take exactly it out again.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Written {
    /// Data keys written.
    pub keys: Vec<Vec<u8>>,
    /// `META:` keys this write created (the origin had none).
    pub meta_keys: Vec<Vec<u8>>,
}

/// Writes `items` into `dir`, creating the database when there is none. The
/// destination browser must not be running: LevelDB's lock is exclusive, so
/// a running browser makes this fail rather than corrupt anything.
/// `now_chrome_micros` stamps a new origin's `META:`.
pub fn write(dir: &Path, items: &[LocalStorageItem], now_chrome_micros: i64) -> Result<Written> {
    if items.is_empty() {
        return Ok(Written::default());
    }
    std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    let opts = Options {
        create_if_missing: true,
        ..Options::default()
    };
    let mut db = DB::open(dir, opts).map_err(|e| {
        Error::Db(format!(
            "{e}; close the browser in the destination and try again"
        ))
    })?;
    let mut written = Written::default();
    if db.get(VERSION_KEY).is_none() {
        db.put(VERSION_KEY, b"1").map_err(db_err)?;
    }
    let mut sizes: std::collections::BTreeMap<&str, u64> = Default::default();
    for item in items {
        let key = data_key(item);
        let value = value_bytes(item);
        *sizes.entry(item.origin.as_str()).or_default() += (key.len() + value.len()) as u64;
        db.put(&key, &value).map_err(db_err)?;
        written.keys.push(key);
    }
    for (origin, size) in sizes {
        let mut meta = META_PREFIX.to_vec();
        meta.extend_from_slice(origin.as_bytes());
        if db.get(&meta).is_none() {
            db.put(&meta, &meta_value(now_chrome_micros, size))
                .map_err(db_err)?;
            written.meta_keys.push(meta);
        }
    }
    db.flush().map_err(db_err)?;
    db.close().map_err(db_err)?;
    Ok(written)
}

/// Deletes `keys` (what [`write`] returned) from `dir`. Returns how many
/// were present.
pub fn delete(dir: &Path, keys: &[Vec<u8>]) -> Result<usize> {
    // No database here (never created, or its files were removed): nothing
    // to delete out of.
    if keys.is_empty() || !dir.join("CURRENT").is_file() {
        return Ok(0);
    }
    let opts = Options {
        create_if_missing: false,
        ..Options::default()
    };
    let mut db = DB::open(dir, opts).map_err(db_err)?;
    let mut n = 0;
    for k in keys {
        if db.get(k).is_some() {
            db.delete(k).map_err(db_err)?;
            n += 1;
        }
    }
    db.flush().map_err(db_err)?;
    db.close().map_err(db_err)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusty_leveldb::CompressorId;
    use rusty_leveldb::compressor::SnappyCompressor;

    fn item(origin: &str, key: &str, value: &str) -> LocalStorageItem {
        LocalStorageItem {
            origin: origin.into(),
            key: key.into(),
            value: value.into(),
            key_raw: None,
            value_raw: None,
        }
    }

    /// Builds a store the way Chromium lays it out, key by key.
    fn chromium_store(dir: &Path, compress: bool) {
        let mut opts = Options::default();
        if compress {
            opts.compressor = SnappyCompressor::ID;
        }
        let mut db = DB::open(dir, opts).unwrap();
        db.put(b"VERSION", b"1").unwrap();
        db.put(
            b"META:https://github.com",
            &meta_value(13_300_000_000_000_000, 40),
        )
        .unwrap();
        db.put(b"METAACCESS:https://github.com", &[0x08, 0x01])
            .unwrap();
        // `_https://github.com` NUL 0x01 `color_mode` -> 0x01 `dark`
        db.put(b"_https://github.com\x00\x01color_mode", b"\x01dark")
            .unwrap();
        // A UTF-16 key and value ("naïve" fits Latin-1, "日本" does not).
        let mut k = b"_https://a.example\x00\x00".to_vec();
        for u in "日本".encode_utf16() {
            k.extend_from_slice(&u.to_le_bytes());
        }
        let mut v = vec![0u8];
        for u in "こんにちは".encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        db.put(&k, &v).unwrap();
        db.put(b"_https://a.example\x00\x01na\xefve", b"\x01caf\xe9")
            .unwrap();
        // A lone surrogate cannot be text.
        db.put(b"_https://a.example\x00\x01bad", &[0, 0x00, 0xd8])
            .unwrap();
        if compress {
            db.compact_range(b"\x00", b"\xff").unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();
    }

    fn sorted(mut v: Vec<LocalStorageItem>) -> Vec<LocalStorageItem> {
        v.sort_by(|a, b| (&a.origin, &a.key).cmp(&(&b.origin, &b.key)));
        v
    }

    #[test]
    fn reads_chromiums_layout_skipping_meta_and_decoding_latin1_and_utf16() {
        let dir = tempfile::tempdir().unwrap();
        chromium_store(dir.path(), false);
        let items = sorted(read(dir.path()).unwrap());
        assert_eq!(items.len(), 4);
        let counts = count_by_origin(dir.path()).unwrap();
        assert_eq!(counts["https://github.com"], 1);
        assert_eq!(counts["https://a.example"], 3);
        let find = |o: &str, k: &str| items.iter().find(|i| i.origin == o && i.key == k).unwrap();
        assert_eq!(find("https://github.com", "color_mode").value, "dark");
        assert_eq!(find("https://a.example", "日本").value, "こんにちは");
        assert_eq!(find("https://a.example", "na\u{ef}ve").value, "caf\u{e9}");
        let bad = find("https://a.example", "bad");
        assert!(bad.value_raw.is_some(), "kept exactly, not guessed");
        assert!(find("https://a.example", "日本").value_raw.is_none());
        // The source directory was only read from a copy: still opens, no LOCK left behind by us.
        assert!(dir.path().join("CURRENT").exists());
    }

    #[test]
    fn write_then_read_round_trips_and_adds_version_and_meta() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("Local Storage/leveldb");
        let items = vec![
            item("https://github.com", "color_mode", "dark"),
            item("https://github.com", "tz", "Europe/Zurich"),
            item("https://a.example", "日本", "こんにちは 🙂"),
            LocalStorageItem {
                origin: "https://a.example".into(),
                key: "bad".into(),
                value: "\u{fffd}".into(),
                key_raw: None,
                value_raw: Some(b64().encode([0u8, 0x00, 0xd8])),
            },
        ];
        let w = write(&store, &items, 13_300_000_000_000_000).unwrap();
        assert_eq!(w.keys.len(), 4);
        assert_eq!(w.meta_keys.len(), 2, "one META per new origin");
        assert_eq!(sorted(read(&store).unwrap()), sorted(items.clone()));
        // The database Chrome opens has VERSION and META for each origin.
        let mut db = DB::open(
            &store,
            Options {
                create_if_missing: false,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(&*db.get(b"VERSION").unwrap(), b"1");
        assert!(db.get(b"META:https://github.com").is_some());
        // Latin-1 where it fits, UTF-16 where it does not, as Chromium writes.
        assert_eq!(
            &*db.get(b"_https://github.com\x00\x01color_mode").unwrap(),
            b"\x01dark"
        );
        db.close().unwrap();
        // Writing again updates in place and keeps the existing META.
        let again = write(
            &store,
            &[item("https://github.com", "color_mode", "light")],
            1,
        )
        .unwrap();
        assert!(again.meta_keys.is_empty());
        let now = read(&store).unwrap();
        assert_eq!(now.len(), 4);
        assert_eq!(
            now.iter().find(|i| i.key == "color_mode").unwrap().value,
            "light"
        );
    }

    #[test]
    fn writing_into_an_existing_chromium_store_keeps_what_was_there_and_wipe_removes_only_ours() {
        let dir = tempfile::tempdir().unwrap();
        chromium_store(dir.path(), false);
        let w = write(
            dir.path(),
            &[item("https://new.example", "k", "v")],
            13_300_000_000_000_001,
        )
        .unwrap();
        assert_eq!(read(dir.path()).unwrap().len(), 5);
        assert_eq!(delete(dir.path(), &w.keys).unwrap(), 1);
        assert_eq!(delete(dir.path(), &w.meta_keys).unwrap(), 1);
        let left = read(dir.path()).unwrap();
        assert_eq!(left.len(), 4);
        assert!(left.iter().all(|i| i.origin != "https://new.example"));
    }

    #[test]
    fn a_snappy_compressed_table_reads_as_chromium_writes_them() {
        let dir = tempfile::tempdir().unwrap();
        chromium_store(dir.path(), true);
        assert!(
            std::fs::read_dir(dir.path()).unwrap().any(|e| e
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "ldb")),
            "compaction wrote a table"
        );
        assert_eq!(read(dir.path()).unwrap().len(), 4);
    }

    #[test]
    fn a_missing_store_is_no_items_and_a_locked_one_fails_clearly() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(&dir.path().join("none")).unwrap().is_empty());
        let store = dir.path().join("s");
        chromium_store(
            &{
                std::fs::create_dir_all(&store).unwrap();
                store.clone()
            },
            false,
        );
        // Hold the lock like a running browser does.
        let held = DB::open(&store, Options::default()).unwrap();
        let err = write(&store, &[item("https://x.example", "k", "v")], 1).unwrap_err();
        assert!(err.to_string().contains("close the browser"), "{err}");
        drop(held);
    }

    #[test]
    fn text_codec_matches_chromiums_choice() {
        assert_eq!(encode_text("caf\u{e9}"), b"\x01caf\xe9");
        assert_eq!(encode_text("日"), vec![0, 0xe5, 0x65]);
        assert_eq!(decode_text(b"\x01abc"), ("abc".to_string(), true));
        assert_eq!(decode_text(&[0, b'a', 0]), ("a".to_string(), true));
        assert!(!decode_text(&[9, 1, 2]).1);
        assert_eq!(parse_data_key(b"META:https://x"), None);
        assert_eq!(parse_data_key(b"VERSION"), None);
    }
}
