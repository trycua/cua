// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Installing saved logins into the destination's OWN Chromium-family
//! browser, re-encrypted under the destination's OWN Safe Storage key.
//!
//! The sender decrypts the logins the user ticked in the review on the
//! source and hands them across already plaintext inside the reserved
//! [`cua_teleport_bundle::logins::LOGINS_ENTRY`]. Like
//! [`crate::cookies::install_cookies`], this module never receives the
//! source's key: it derives this machine's own key, encrypts each password
//! under it and writes the row into the browser's real `Login Data`
//! database, discovering the live columns with `PRAGMA table_info` and only
//! setting the ones it knows the meaning of.
//!
//! Unlike cookies, there is no schema this module can safely create from
//! nothing: Chrome migrates `Login Data` from a version number, and a
//! hand-made table at the wrong version would be razed on first launch. So
//! when the browser has never been launched here (no `Login Data` yet) the
//! passwords are skipped with a notice back to the sender instead of
//! guessing.

use std::path::Path;

use cua_teleport_bundle::chromium_crypto;
use cua_teleport_bundle::logins::LoginItem;
use cua_teleport_bundle::Platform;

use crate::cookies::{chrome_now_utc, ensure_safe_storage_secret};
use crate::host::HostEffects;
use crate::ledger::ImportRecord;
use crate::{Result, TeleportError};

/// The columns this module fills when the table has them.
const KNOWN_COLUMNS: &[&str] = &[
    "origin_url",
    "action_url",
    "username_element",
    "username_value",
    "password_element",
    "password_value",
    "signon_realm",
    "date_created",
    "date_last_used",
    "date_password_modified",
    "blacklisted_by_user",
    "scheme",
    "preferred",
    "times_used",
];

/// Installs `items` into `profile_dir`'s `Login Data`. Returns how many rows
/// were written.
pub fn install_logins(
    host: &dyn HostEffects,
    profile_dir: &Path,
    service: &'static str,
    platform: Platform,
    items: &[LoginItem],
    record: &mut ImportRecord,
) -> Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }
    if platform == Platform::Windows {
        return Err(TeleportError::Provider(
            "re-encrypting saved passwords for a Windows destination (DPAPI) is not supported yet"
                .into(),
        ));
    }
    let db = profile_dir.join("Login Data");
    if !db.is_file() {
        // Not a failure of the whole import (the cookies and files still
        // land): the passwords wait for a browser that has run once.
        record.notices.push(
            "Saved passwords were not sent: the browser here has no password store yet. Open it \
             once in this Space, then send the passwords again."
                .into(),
        );
        return Ok(0);
    }
    let secret = ensure_safe_storage_secret(host, platform, service, record)
        .map_err(|e| TeleportError::Provider(format!("Safe Storage key for {service}: {e}")))?;
    let rounds = match platform {
        Platform::Linux => chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
        Platform::MacOS | Platform::Windows => chromium_crypto::MACOS_PBKDF2_ROUNDS,
    };
    let key = chromium_crypto::derive_key(&secret, rounds);
    let conn = rusqlite::Connection::open(&db)
        .map_err(|e| TeleportError::Provider(format!("opening Login Data failed: {e}")))?;
    let present = columns(&conn)
        .map_err(|e| TeleportError::Provider(format!("reading Login Data schema failed: {e}")))?;
    for required in [
        "origin_url",
        "username_value",
        "password_value",
        "signon_realm",
    ] {
        if !present.contains(required) {
            return Err(TeleportError::Provider(format!(
                "the destination's Login Data table has no {required:?} column; this Chrome \
                 version's schema is not one this build can write passwords into"
            )));
        }
    }
    let cols: Vec<&str> = KNOWN_COLUMNS
        .iter()
        .copied()
        .filter(|c| present.contains(*c))
        .collect();
    let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "INSERT OR REPLACE INTO logins ({}) VALUES ({})",
        cols.join(", "),
        placeholders.join(", ")
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| TeleportError::Provider(format!("preparing login insert failed: {e}")))?;
    let now = chrome_now_utc();
    let mut written = 0;
    for item in items {
        let encrypted = chromium_crypto::encrypt_v10(&key, &item.password);
        let args: Vec<rusqlite::types::Value> = cols
            .iter()
            .map(|c| match *c {
                "origin_url" => item.origin.clone().into(),
                "action_url" => item.signon_realm.clone().into(),
                "username_element" | "password_element" => String::new().into(),
                "username_value" => item.username.clone().into(),
                "password_value" => encrypted.clone().into(),
                "signon_realm" => item.signon_realm.clone().into(),
                "date_created" | "date_password_modified" => now.into(),
                "date_last_used"
                | "scheme"
                | "blacklisted_by_user"
                | "preferred"
                | "times_used" => 0i64.into(),
                other => unreachable!("column {other} was filtered out of KNOWN_COLUMNS above"),
            })
            .collect();
        stmt.execute(rusqlite::params_from_iter(args))
            .map_err(|e| TeleportError::Provider(format!("writing a saved login failed: {e}")))?;
        record.login_row_written(&db, &item.origin, &item.username, &item.signon_realm);
        written += 1;
    }
    Ok(written)
}

fn columns(conn: &rusqlite::Connection) -> rusqlite::Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(\"logins\")")?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names.into_iter().map(|n| n.to_ascii_lowercase()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::FakeHost;

    fn login(origin: &str, user: &str, pw: &[u8]) -> LoginItem {
        LoginItem {
            origin: origin.into(),
            username: user.into(),
            password: pw.to_vec(),
            signon_realm: cua_teleport_bundle::logins::signon_realm(origin),
        }
    }

    /// A `Login Data` the way a Chrome that already ran has it (trimmed to the
    /// columns that matter plus ones this module must tolerate).
    fn existing(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("Login Data");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE logins (origin_url VARCHAR NOT NULL, action_url VARCHAR,
             username_element VARCHAR, username_value VARCHAR, password_element VARCHAR,
             password_value BLOB, submit_element VARCHAR, signon_realm VARCHAR NOT NULL,
             date_created INTEGER NOT NULL, blacklisted_by_user INTEGER NOT NULL,
             scheme INTEGER NOT NULL, password_type INTEGER, times_used INTEGER,
             date_last_used INTEGER NOT NULL DEFAULT 0, date_password_modified INTEGER NOT NULL DEFAULT 0,
             UNIQUE (origin_url, username_element, username_value, password_element, signon_realm));
             INSERT INTO logins (origin_url, action_url, username_element, username_value,
              password_element, password_value, signon_realm, date_created, blacklisted_by_user, scheme)
             VALUES ('https://mine.example', 'https://mine.example/', '', 'me', '', x'7631300102',
              'https://mine.example/', 1, 0, 0);",
        )
        .unwrap();
        db
    }

    #[test]
    fn logins_are_reencrypted_under_the_destinations_key_beside_the_browsers_own() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("Default");
        std::fs::create_dir(&dir).unwrap();
        let db = existing(&dir);
        let host = FakeHost::new();
        let mut record = ImportRecord::default();
        let n = install_logins(
            &host,
            &dir,
            "Chrome Safe Storage",
            Platform::Linux,
            &[login("https://github.com", "octo", b"gh-pw-1234")],
            &mut record,
        )
        .unwrap();
        assert_eq!(n, 1);
        let conn = rusqlite::Connection::open(&db).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM logins", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "the browser's own login is still there");
        let (enc, realm): (Vec<u8>, String) = conn
            .query_row(
                "SELECT password_value, signon_realm FROM logins WHERE username_value = 'octo'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(realm, "https://github.com/");
        assert!(enc.starts_with(b"v10"));
        assert!(
            !enc.windows(10).any(|w| w == b"gh-pw-1234"),
            "never plaintext"
        );
        let key = chromium_crypto::derive_key(
            chromium_crypto::LINUX_V10_PASSWORD,
            chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
        );
        let plain = chromium_crypto::decrypt_prefixed(&key, &enc).unwrap().1;
        assert_eq!(&*plain, b"gh-pw-1234");
        assert_eq!(record.login_rows.len(), 1);

        // Wipe removes exactly this login.
        let ledger = crate::ledger::Ledger::new("i", "chrome", &record, 0, 1);
        let removed = crate::ledger::wipe(&ledger, home.path(), &host);
        assert_eq!(removed.login_rows_removed, 1, "{:?}", removed.refused);
        let left: i64 = conn
            .query_row("SELECT count(*) FROM logins", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
    }

    #[test]
    fn a_browser_never_launched_gets_a_notice_and_nothing_is_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = ImportRecord::default();
        let n = install_logins(
            &FakeHost::new(),
            dir.path(),
            "Chrome Safe Storage",
            Platform::Linux,
            &[login("https://github.com", "octo", b"pw")],
            &mut record,
        )
        .unwrap();
        assert_eq!(n, 0);
        assert!(
            record.notices[0].contains("Open it once"),
            "{:?}",
            record.notices
        );
        assert!(!dir.path().join("Login Data").exists());
    }

    #[test]
    fn empty_input_and_windows_are_refused_without_touching_anything() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = ImportRecord::default();
        assert_eq!(
            install_logins(
                &FakeHost::new(),
                dir.path(),
                "x",
                Platform::Linux,
                &[],
                &mut rec
            )
            .unwrap(),
            0
        );
        assert!(install_logins(
            &FakeHost::new(),
            dir.path(),
            "x",
            Platform::Windows,
            &[login("https://a.test", "u", b"p")],
            &mut rec
        )
        .is_err());
    }
}
