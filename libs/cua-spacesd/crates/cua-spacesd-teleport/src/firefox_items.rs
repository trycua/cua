// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Installing Keyvault items into a Firefox profile: cookies into
//! `cookies.sqlite` (`moz_cookies`, schema 16), localStorage into
//! `storage/default/<origin>/ls/data.sqlite`, saved logins into `logins.json`
//! encrypted under the profile's own `key4.db` master key. Formats:
//! `~/projects/.cua-work/keyvault-v2/browser-formats.md`.
//!
//! The destination Firefox is not running (the import launches it after).

use std::path::Path;

use cua_teleport_bundle::cookies::CookieItem;
use cua_teleport_bundle::local_storage::LocalStorageItem;
use cua_teleport_bundle::logins::LoginItem;

use crate::ledger::ImportRecord;
use crate::{Result, TeleportError};

const CHROME_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;
const COOKIES_SCHEMA_VERSION: i64 = 16;

fn perr(m: impl std::fmt::Display) -> TeleportError {
    TeleportError::Provider(m.to_string())
}

/// `https://top.example` as the `originAttributes` Firefox keys a partitioned
/// cookie by (`^partitionKey=%28https%2Ctop.example%29`).
fn origin_attributes(partition: Option<&str>) -> String {
    let Some(p) = partition.filter(|p| !p.is_empty()) else {
        return String::new();
    };
    match p.split_once("://") {
        Some((scheme, host)) => format!("^partitionKey=%28{scheme}%2C{host}%29"),
        None => String::new(),
    }
}

/// Cookies into `profile/cookies.sqlite` (created at schema 16 when missing).
pub fn install_cookies(
    profile: &Path,
    items: &[CookieItem],
    record: &mut ImportRecord,
) -> Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }
    let db = profile.join("cookies.sqlite");
    let existed = db.is_file();
    std::fs::create_dir_all(profile).map_err(perr)?;
    let conn = rusqlite::Connection::open(&db)
        .map_err(|e| perr(format!("opening cookies.sqlite: {e}")))?;
    conn.execute_batch(&format!(
        "PRAGMA user_version = {COOKIES_SCHEMA_VERSION};
         CREATE TABLE IF NOT EXISTS moz_cookies (id INTEGER PRIMARY KEY, originAttributes TEXT NOT NULL DEFAULT '',
          name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER,
          creationTime INTEGER, isSecure INTEGER, isHttpOnly INTEGER, inBrowserElement INTEGER DEFAULT 0,
          sameSite INTEGER DEFAULT 0, schemeMap INTEGER DEFAULT 0, isPartitionedAttributeSet INTEGER DEFAULT 0,
          CONSTRAINT moz_uniqueid UNIQUE (name, host, path, originAttributes));"
    ))
    .map_err(|e| perr(format!("creating moz_cookies: {e}")))?;
    if !existed {
        record.file_written(&db);
    }
    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    let mut stmt = conn
        .prepare(
            "INSERT OR REPLACE INTO moz_cookies (originAttributes, name, value, host, path, expiry,
             lastAccessed, creationTime, isSecure, isHttpOnly, sameSite, schemeMap, isPartitionedAttributeSet)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )
        .map_err(perr)?;
    let mut n = 0;
    for c in items {
        let attrs = origin_attributes(c.extra.partition_key.as_deref());
        let micros = |chrome: Option<i64>| {
            chrome
                .filter(|v| *v > CHROME_EPOCH_OFFSET_MICROS)
                .map(|v| v - CHROME_EPOCH_OFFSET_MICROS)
                .unwrap_or(now_us)
        };
        // Chromium's microseconds since 1601 to Firefox's milliseconds since
        // 1970; a session cookie (0) stays 0 and Firefox treats it as one.
        let expiry_ms = if c.expires_utc > CHROME_EPOCH_OFFSET_MICROS {
            (c.expires_utc - CHROME_EPOCH_OFFSET_MICROS) / 1000
        } else {
            0
        };
        stmt.execute(rusqlite::params![
            attrs,
            c.name,
            String::from_utf8_lossy(&c.value),
            c.host_key,
            c.path,
            expiry_ms,
            micros(c.extra.last_access_utc),
            micros(c.extra.creation_utc),
            c.is_secure as i64,
            c.is_httponly as i64,
            c.samesite.clamp(0, 2),
            if c.is_secure { 2 } else { 1 },
            (!attrs.is_empty()) as i64,
        ])
        .map_err(|e| perr(format!("writing a cookie: {e}")))?;
        record.cookie_row_written(&db, &c.host_key, &c.name, &c.path);
        n += 1;
    }
    Ok(n)
}

/// `https://a.example:8443` as Firefox's `storage/default` directory name.
fn origin_dir(origin: &str) -> String {
    match origin.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}+++{}", rest.replace(':', "+")),
        None => origin.to_string(),
    }
}

/// localStorage into `profile/storage/default/<origin>/ls/data.sqlite`. Values
/// are written UTF-8, uncompressed (a form Firefox reads as it does its own).
pub fn install_local_storage(
    profile: &Path,
    items: &[LocalStorageItem],
    record: &mut ImportRecord,
) -> Result<usize> {
    let mut by_origin: std::collections::BTreeMap<&str, Vec<&LocalStorageItem>> =
        Default::default();
    for i in items {
        by_origin.entry(i.origin.as_str()).or_default().push(i);
    }
    let mut n = 0;
    for (origin, rows) in by_origin {
        let dir = profile
            .join("storage/default")
            .join(origin_dir(origin))
            .join("ls");
        record.create_dir_all(&dir)?;
        let db = dir.join("data.sqlite");
        let existed = db.is_file();
        let conn = rusqlite::Connection::open(&db)
            .map_err(|e| perr(format!("opening data.sqlite: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS database (origin TEXT NOT NULL, usage INTEGER NOT NULL DEFAULT 0,
              last_vacuum_time INTEGER NOT NULL DEFAULT 0, last_analyze_time INTEGER NOT NULL DEFAULT 0,
              last_vacuum_size INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS data (key TEXT PRIMARY KEY, utf16_length INTEGER NOT NULL,
              conversion_type INTEGER NOT NULL, compression_type INTEGER NOT NULL,
              last_access_time INTEGER NOT NULL DEFAULT 0, value BLOB NOT NULL);",
        )
        .map_err(perr)?;
        if !existed {
            record.file_written(&db);
            conn.execute("INSERT INTO database (origin) VALUES (?1)", [origin])
                .map_err(perr)?;
        }
        let mut usage = 0i64;
        for r in rows {
            usage += (r.key.len() + r.value.len()) as i64;
            conn.execute(
                "INSERT OR REPLACE INTO data (key, utf16_length, conversion_type, compression_type, value)
                 VALUES (?1, ?2, 1, 0, ?3)",
                rusqlite::params![r.key, r.value.encode_utf16().count() as i64, r.value.as_bytes()],
            )
            .map_err(perr)?;
            n += 1;
        }
        conn.execute("UPDATE database SET usage = usage + ?1", [usage])
            .map_err(perr)?;
    }
    Ok(n)
}

fn uuid(rng: &mut impl rand::RngCore) -> String {
    let mut b = [0u8; 16];
    rng.fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{{{}-{}-{}-{}-{}}}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Saved logins into `profile/logins.json`, encrypted under the profile's own
/// `key4.db` master key (empty master password). A profile with no `key4.db`
/// yet, or with a master password, gets a notice and the logins wait: nothing
/// is guessed and nothing is written.
pub fn install_logins(
    profile: &Path,
    items: &[LoginItem],
    record: &mut ImportRecord,
) -> Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }
    let Some(master) = master_key(profile)? else {
        record.notices.push(
            "Saved passwords were not sent: Firefox here has no password key yet. Open it once in \
             this Space, then send the passwords again."
                .into(),
        );
        return Ok(0);
    };
    let key_id = key_id(profile).unwrap_or_else(|| vec![0xf8; 16]);
    let path = profile.join("logins.json");
    let existed = path.is_file();
    let mut doc: serde_json::Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({"nextId": 1, "logins": [], "version": 3}));
    let mut next = doc["nextId"].as_i64().unwrap_or(1);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut rng = rand::thread_rng();
    let mut n = 0;
    for l in items {
        let mut iv = [0u8; 8];
        rand::RngCore::fill_bytes(&mut rng, &mut iv);
        let user = cua_teleport_bundle::firefox_nss::encrypt_sdr(
            &master,
            &key_id,
            &iv,
            l.username.as_bytes(),
        )
        .map_err(perr)?;
        rand::RngCore::fill_bytes(&mut rng, &mut iv);
        let pass =
            cua_teleport_bundle::firefox_nss::encrypt_sdr(&master, &key_id, &iv, &l.password)
                .map_err(perr)?;
        let entry = serde_json::json!({
            "id": next, "hostname": l.origin, "httpRealm": null, "formSubmitURL": l.origin,
            "usernameField": "", "passwordField": "",
            "encryptedUsername": user, "encryptedPassword": pass,
            "guid": uuid(&mut rng), "encType": 1,
            "timeCreated": now_ms, "timeLastUsed": now_ms, "timePasswordChanged": now_ms, "timesUsed": 1
        });
        next += 1;
        if let Some(arr) = doc["logins"].as_array_mut() {
            arr.push(entry);
        }
        n += 1;
    }
    doc["nextId"] = serde_json::json!(next);
    std::fs::write(&path, serde_json::to_vec(&doc).map_err(perr)?).map_err(perr)?;
    if !existed {
        record.file_written(&path);
    }
    Ok(n)
}

fn open_key4(profile: &Path) -> Option<rusqlite::Connection> {
    let p = profile.join("key4.db");
    p.is_file().then(|| {
        rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
    })?
}

fn key_id(profile: &Path) -> Option<Vec<u8>> {
    open_key4(profile)?
        .query_row(
            "SELECT a102 FROM nssPrivate WHERE a102 IS NOT NULL LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok()
}

/// The profile's login master key, `None` when it has no `key4.db`.
fn master_key(profile: &Path) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
    let Some(conn) = open_key4(profile) else {
        return Ok(None);
    };
    let (salt, item2): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT item1, item2 FROM metaData WHERE id = 'password'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| perr(format!("Firefox key4.db has no password entry: {e}")))?;
    let a11: Vec<u8> = conn
        .query_row(
            "SELECT a11 FROM nssPrivate WHERE a11 IS NOT NULL LIMIT 1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| perr(format!("Firefox key4.db has no login key: {e}")))?;
    cua_teleport_bundle::firefox_nss::master_key(&salt, &item2, &a11, b"")
        .map(Some)
        .map_err(|e| perr(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_teleport_bundle::cookies::CookieExtra;
    use cua_teleport_bundle::firefox_nss::testing as nss;

    const MASTER: [u8; 24] = *b"0123456789abcdef01234567";

    fn cookie(host: &str, name: &str, value: &str, partition: Option<&str>) -> CookieItem {
        CookieItem {
            host_key: host.into(),
            name: name.into(),
            value: value.as_bytes().to_vec(),
            path: "/".into(),
            expires_utc: 1_900_000_000_000 * 1000 + CHROME_EPOCH_OFFSET_MICROS,
            is_secure: true,
            is_httponly: true,
            samesite: 1,
            extra: CookieExtra {
                creation_utc: Some(1_690_000_000_000_000 + CHROME_EPOCH_OFFSET_MICROS),
                partition_key: partition.map(str::to_string),
                ..Default::default()
            },
        }
    }

    #[test]
    fn cookies_land_in_moz_cookies_with_units_and_partition_and_wipe_by_row() {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join("Profiles/cua.default-release");
        let mut record = ImportRecord::default();
        let n = install_cookies(
            &profile,
            &[
                cookie(".github.com", "user_session", "abc", None),
                cookie("example.com", "sid", "p", Some("https://top.example")),
            ],
            &mut record,
        )
        .unwrap();
        assert_eq!(n, 2);
        let conn = rusqlite::Connection::open(profile.join("cookies.sqlite")).unwrap();
        let (expiry, attrs, partitioned, created): (i64, String, i64, i64) = conn
            .query_row(
                "SELECT expiry, originAttributes, isPartitionedAttributeSet, creationTime FROM moz_cookies WHERE name = 'sid'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(expiry, 1_900_000_000_000, "milliseconds since 1970");
        assert_eq!(attrs, "^partitionKey=%28https%2Ctop.example%29");
        assert_eq!(partitioned, 1);
        assert_eq!(created, 1_690_000_000_000_000, "microseconds since 1970");
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 16);
        // Wipe deletes exactly those rows (the file was ours, so it goes too).
        let ledger = crate::ledger::Ledger::new("i", "firefox", &record, 0, 1);
        let report = crate::ledger::wipe(&ledger, home.path(), &crate::host::FakeHost::new());
        assert!(report.complete(), "{:?}", report.errors);
        assert!(!profile.join("cookies.sqlite").exists());
    }

    #[test]
    fn local_storage_lands_in_the_origins_ls_database() {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join("p");
        let mut record = ImportRecord::default();
        let items = [LocalStorageItem {
            origin: "https://app.example:8443".into(),
            key: "token".into(),
            value: "caf\u{e9}".into(),
            key_raw: None,
            value_raw: None,
        }];
        assert_eq!(
            install_local_storage(&profile, &items, &mut record).unwrap(),
            1
        );
        let db = profile.join("storage/default/https+++app.example+8443/ls/data.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        let (v, conv): (Vec<u8>, i64) = conn
            .query_row(
                "SELECT value, conversion_type FROM data WHERE key = 'token'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((v.as_slice(), conv), ("caf\u{e9}".as_bytes(), 1));
        let origin: String = conn
            .query_row("SELECT origin FROM database", [], |r| r.get(0))
            .unwrap();
        assert_eq!(origin, "https://app.example:8443");
        assert!(record.files.contains(&db));
    }

    #[test]
    fn logins_are_encrypted_under_the_profiles_own_master_key_or_wait_with_a_notice() {
        let profile = tempfile::tempdir().unwrap();
        // No key4.db yet: a notice, nothing written.
        let mut record = ImportRecord::default();
        let login = LoginItem {
            origin: "https://github.com".into(),
            username: "octo".into(),
            password: b"gh-pw".to_vec(),
            signon_realm: "https://github.com/".into(),
        };
        assert_eq!(
            install_logins(profile.path(), std::slice::from_ref(&login), &mut record).unwrap(),
            0
        );
        assert!(record.notices[0].contains("Open it once"));
        assert!(!profile.path().join("logins.json").exists());
        // With its own key4.db the login is written and reads back.
        let (salt, item2, a11) = nss::key4_blobs(&MASTER, b"", true);
        let conn = rusqlite::Connection::open(profile.path().join("key4.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE metaData (id PRIMARY KEY, item1, item2); CREATE TABLE nssPrivate (id INTEGER PRIMARY KEY, a11, a102);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO metaData VALUES ('password', ?1, ?2)",
            rusqlite::params![salt, item2],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nssPrivate (a11, a102) VALUES (?1, ?2)",
            rusqlite::params![a11, vec![0xf8u8; 16]],
        )
        .unwrap();
        drop(conn);
        let mut record = ImportRecord::default();
        assert_eq!(
            install_logins(profile.path(), &[login], &mut record).unwrap(),
            1
        );
        let doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(profile.path().join("logins.json")).unwrap())
                .unwrap();
        let e = &doc["logins"][0];
        assert_eq!(e["hostname"], "https://github.com");
        let pw = cua_teleport_bundle::firefox_nss::decrypt_sdr(
            &MASTER,
            e["encryptedPassword"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(&*pw, b"gh-pw");
        let user = cua_teleport_bundle::firefox_nss::decrypt_sdr(
            &MASTER,
            e["encryptedUsername"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(&*user, b"octo");
        assert!(e["guid"].as_str().unwrap().starts_with('{'));
    }
}
