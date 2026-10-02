// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Installing decrypted cookies into the destination's OWN Chromium-family
//! browser, re-encrypted under the destination's OWN Safe Storage key.
//!
//! The sender (`cua_teleport::browser_cookies`) decrypts cookies on the
//! source and hands them across already plaintext, inside the reserved
//! [`cua_teleport_bundle::cookies::COOKIES_ENTRY`] bundle entry. This module
//! is the other half: it never receives, reads, or reinstalls the source's
//! Safe Storage key. Instead it:
//!
//! 1. reads this machine's own Safe Storage Keychain item for the target
//!    browser, or generates and installs a fresh one if this machine has
//!    never had that browser signed in before ([`ensure_safe_storage_secret`]);
//! 2. derives the AES key from THAT secret with the same PBKDF2 scheme every
//!    Chromium browser uses ([`cua_teleport_bundle::chromium_crypto`]);
//! 3. re-encrypts each cookie value under it and writes the row into the
//!    destination's real `Cookies` SQLite database ([`install_cookies`]).
//!
//! [`install_cookies`] writes into the browser's own `Cookies` database when
//! one exists, discovering its live columns via `PRAGMA table_info` and only
//! ever setting the columns it knows the semantics of. When the browser has
//! never been launched on this machine (a fresh Space), there is no database
//! yet, so [`create_cookies_db`] creates one in Chrome's current schema with
//! the `meta` version Chrome expects; Chrome then adopts it as its own on
//! first launch instead of discarding or migrating it.

use std::path::Path;

use cua_teleport_bundle::chromium_crypto;
use cua_teleport_bundle::cookies::CookieItem;
use cua_teleport_bundle::keychain::KeychainItem;
use cua_teleport_bundle::Platform;

use crate::host::HostEffects;
use crate::keychain::{install_generic_noted, read_secret, trusted_app_for};
use crate::ledger::ImportRecord;
use crate::{Result, TeleportError};

/// The Keychain account name a Chromium-family browser's Safe Storage item
/// uses, derived from its service name (`"Chrome Safe Storage"` ->
/// `"Chrome"`). Every Chromium browser installs its Safe Storage item this
/// way, so the fresh items this module creates use the same convention the
/// browser itself would, and an existing item (however it got there) is
/// still found by [`read_secret`].
fn account_for_service(service: &str) -> &str {
    service.strip_suffix(" Safe Storage").unwrap_or(service)
}

/// Reads this machine's existing Safe Storage secret for `service`, or
/// generates a fresh one and installs it, if none exists yet. Never touches,
/// receives, or is given the SOURCE machine's secret: this is the
/// destination's own key from the moment it exists.
///
/// The generated secret is 32 bytes of OS randomness, hex-encoded to a plain
/// ASCII string. A Chromium `os_crypt` key holder never inspects the format,
/// only its randomness, so this is the same size class Chromium itself
/// generates when it first needs one.
///
/// A freshly installed item is recorded in `record`, so `WipeImport` can
/// remove it again along with the rows it protects; a REUSED existing item
/// is not recorded (it predates this import and outlives it).
pub fn ensure_safe_storage_secret(
    host: &dyn HostEffects,
    platform: Platform,
    service: &'static str,
    record: &mut ImportRecord,
) -> std::io::Result<Vec<u8>> {
    // Linux Chrome's `v10` key comes from the FIXED "peanuts" password when
    // no key-store secret exists (the common case in a guest): there is no
    // per-machine secret to read or create, and using a Keychain-style random
    // one here would derive a key the destination's own (real) Chrome never
    // would. A real Linux Chrome reading this database derives the exact
    // same fixed key on its own, with no Keychain involved at all.
    if platform == Platform::Linux {
        return Ok(chromium_crypto::LINUX_V10_PASSWORD.to_vec());
    }
    let account = account_for_service(service);
    if let Some(existing) = read_secret(host, service, account) {
        return Ok(existing);
    }
    let mut raw = [0u8; 32];
    // rand 0.8 (this workspace's pinned version): `thread_rng()`, not the
    // 0.9 `rand::rng()` free function.
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut raw);
    let secret = hex::encode(raw).into_bytes();
    // Created with the browser trusted from the start: its team id goes in the
    // item's partition list, so the browser reads its own key without the
    // "wants to access key ... enter the login keychain password" dialog.
    install_generic_noted(
        host,
        &KeychainItem {
            service: service.to_string(),
            account: account.to_string(),
            secret: secret.clone(),
            trust_app: trusted_app_for(service).map(str::to_string),
        },
        &mut record.notices,
    )?;
    record.keychain_installed(service, account);
    Ok(secret)
}

/// The PBKDF2 round count for `platform`'s Safe Storage key derivation (see
/// [`cua_teleport_bundle::chromium_crypto`]'s table). Windows is refused
/// before this is called.
fn pbkdf2_rounds_for(platform: Platform) -> u32 {
    match platform {
        Platform::Linux => chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
        Platform::MacOS | Platform::Windows => chromium_crypto::MACOS_PBKDF2_ROUNDS,
    }
}

/// The columns [`install_cookies`] knows how to fill, in the order the
/// `INSERT` statement below names them. Every one of these has existed in
/// Chromium's `cookies` table since at least 2018; a newer column Chrome
/// added (e.g. `top_frame_site_key`, `has_cross_site_ancestor`) is left to
/// whatever default that Chrome version's own `CREATE TABLE` gave it, which
/// is why this module never runs `CREATE TABLE` itself.
const KNOWN_COLUMNS: &[&str] = &[
    "host_key",
    "name",
    "value",
    "encrypted_value",
    "path",
    "expires_utc",
    "is_secure",
    "is_httponly",
    "samesite",
    "creation_utc",
    "has_expires",
    "is_persistent",
    // The rest of Chrome's current schema: written from the source's row when
    // it had them, else at Chrome's own defaults, so the destination's row
    // is the source's row (a partitioned cookie stays partitioned).
    "last_access_utc",
    "last_update_utc",
    "priority",
    "source_scheme",
    "source_port",
    "source_type",
    "has_cross_site_ancestor",
    "top_frame_site_key",
];

/// Microseconds since the Windows/Chrome epoch (1601-01-01 UTC) for the
/// current time -- the unit every Chrome `*_utc` cookie column uses,
/// including `creation_utc`, which is `NOT NULL` with no default in Chrome's
/// real schema and therefore always needs an explicit value.
pub(crate) fn chrome_now_utc() -> i64 {
    const UNIX_TO_CHROME_EPOCH_MICROS: i64 = 11_644_473_600_000_000;
    let unix_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    unix_micros + UNIX_TO_CHROME_EPOCH_MICROS
}

/// The `Cookies` meta version from which Chrome (130 and later) puts SHA-256
/// of the row's `host_key` in front of each value before encrypting it, and
/// drops a row whose digest does not match on read.
const HOST_KEY_DIGEST_META_VERSION: i64 = 24;

/// The destination `Cookies` database's `meta` version (0 when unreadable).
fn cookies_meta_version(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|v| v.trim().parse().ok())
    .unwrap_or(0)
}

/// What to encrypt for one cookie: the value, behind its `host_key` digest
/// when the destination's Chrome expects one.
fn plaintext_for(digest: bool, host_key: &str, value: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut out = Vec::with_capacity(32 + value.len());
    if digest {
        out.extend_from_slice(&Sha256::digest(host_key.as_bytes()));
    }
    out.extend_from_slice(value);
    out
}

/// Installs `items` into `profile_dir`'s `Cookies` database, re-encrypted
/// under this machine's own Safe Storage key for `service` (created if this
/// is the first cookie ever installed for that browser here). Returns how
/// many rows were written.
///
/// A profile with no `Cookies` database yet (the browser was never launched
/// here) gets one created in Chrome's current schema first
/// ([`create_cookies_db`]); an existing database is written into as it is.
///
/// Fails with a named error, rather than guessing a schema, when:
/// - the `cookies` table is missing a column this module needs to identify a
///   row (`host_key`, `name`, `path` -- Chromium's own unique index).
pub fn install_cookies(
    host: &dyn HostEffects,
    profile_dir: &Path,
    service: &'static str,
    platform: Platform,
    items: &[CookieItem],
    record: &mut ImportRecord,
) -> Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }
    if platform == Platform::Windows {
        return Err(TeleportError::Provider(
            "re-encrypting cookies for a Windows destination (DPAPI) is not supported yet".into(),
        ));
    }
    let secret = ensure_safe_storage_secret(host, platform, service, record)
        .map_err(|e| TeleportError::Provider(format!("Safe Storage key for {service}: {e}")))?;
    let key = chromium_crypto::derive_key(&secret, pbkdf2_rounds_for(platform));
    // Which file Chrome reads depends on its version (see
    // [`cookies_db_paths`]), and the destination's Chrome may not have run
    // yet, so write every location it could use. The unused copy is inert.
    let mut written = 0;
    for db in cookies_db_paths(profile_dir) {
        written = written.max(install_into_db(&db, &key, items, record)?);
    }
    Ok(written)
}

/// Writes `items`, encrypted under `key`, into the cookie database at `db`
/// (created first when absent). Returns how many rows were written.
fn install_into_db(
    db: &Path,
    key: &[u8; 16],
    items: &[CookieItem],
    record: &mut ImportRecord,
) -> Result<usize> {
    if !db.is_file() {
        // Never-launched browser: create the database Chrome itself would.
        create_cookies_db(db)
            .map_err(|e| TeleportError::Provider(format!("creating Cookies failed: {e}")))?;
    }
    let conn = rusqlite::Connection::open(db)
        .map_err(|e| TeleportError::Provider(format!("opening Cookies failed: {e}")))?;
    let present = existing_columns(&conn, "cookies")
        .map_err(|e| TeleportError::Provider(format!("reading Cookies schema failed: {e}")))?;
    let mut columns: Vec<&str> = KNOWN_COLUMNS
        .iter()
        .copied()
        .filter(|c| present.contains(&c.to_ascii_lowercase()))
        .collect();
    for required in ["host_key", "name", "path"] {
        if !columns.contains(&required) {
            return Err(TeleportError::Provider(format!(
                "the destination's Cookies table has no {required:?} column; this Chrome \
                 version's schema is not one this build can write cookies into"
            )));
        }
    }
    // `value` always ships empty for an encrypted row (this is what Chrome
    // itself writes once a value migrates to `encrypted_value`); if a
    // profile's table somehow lacks `value` entirely, drop it rather than
    // failing, since it carries no information here.
    columns.retain(|c| *c != "value" || present.contains("value"));
    let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "INSERT OR REPLACE INTO cookies ({}) VALUES ({})",
        columns.join(", "),
        placeholders.join(", ")
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| TeleportError::Provider(format!("preparing cookie insert failed: {e}")))?;

    let now = chrome_now_utc();
    let digest_values = cookies_meta_version(&conn) >= HOST_KEY_DIGEST_META_VERSION;
    let mut written = 0usize;
    for item in items {
        let plain = plaintext_for(digest_values, &item.host_key, &item.value);
        let encrypted = chromium_crypto::encrypt_v10(key, &plain);
        let mut args: Vec<rusqlite::types::Value> = Vec::with_capacity(columns.len());
        for column in &columns {
            args.push(match *column {
                "host_key" => item.host_key.clone().into(),
                "name" => item.name.clone().into(),
                "value" => String::new().into(),
                "encrypted_value" => encrypted.clone().into(),
                "path" => item.path.clone().into(),
                "expires_utc" => item.expires_utc.into(),
                "is_secure" => (item.is_secure as i64).into(),
                "is_httponly" => (item.is_httponly as i64).into(),
                "samesite" => item.samesite.into(),
                // Chrome's own invariant: a persistent cookie has an expiry,
                // a session cookie (`expires_utc == 0`) does not.
                "has_expires" | "is_persistent" => ((item.expires_utc != 0) as i64).into(),
                // The source's creation time when it had one; else this
                // import is the creation event on this machine.
                "creation_utc" => item.extra.creation_utc.unwrap_or(now).into(),
                "last_access_utc" => item.extra.last_access_utc.unwrap_or(0).into(),
                "last_update_utc" => item.extra.last_update_utc.unwrap_or(now).into(),
                "priority" => item.extra.priority.unwrap_or(1).into(),
                "source_scheme" => item.extra.source_scheme.unwrap_or(0).into(),
                "source_port" => item.extra.source_port.unwrap_or(-1).into(),
                "source_type" => item.extra.source_type.unwrap_or(0).into(),
                "has_cross_site_ancestor" => item.extra.has_cross_site_ancestor.unwrap_or(0).into(),
                "top_frame_site_key" => item.extra.partition_key.clone().unwrap_or_default().into(),
                other => unreachable!("column {other} was filtered out of KNOWN_COLUMNS above"),
            });
        }
        stmt.execute(rusqlite::params_from_iter(args))
            .map_err(|e| TeleportError::Provider(format!("writing a cookie failed: {e}")))?;
        // `Cookies` is an existing database this import does not own (see
        // the module doc): never ledgered as a whole file, or `WipeImport`
        // would delete every cookie the destination's own browser already
        // had. Instead, the exact row -- by Chromium's own uniqueness for
        // one (`host_key`, `name`, `path`) -- so wipe can `DELETE` only
        // this row later.
        record.cookie_row_written(db, &item.host_key, &item.name, &item.path);
        written += 1;
    }
    Ok(written)
}

/// The `Cookies` meta version this module creates fresh databases at: the
/// first one with the `host_key` digest ([`HOST_KEY_DIGEST_META_VERSION`]),
/// i.e. Chrome 130+, which [`install_cookies`] then encrypts for.
const CREATED_META_VERSION: i64 = HOST_KEY_DIGEST_META_VERSION;

/// `last_compatible_version` Chrome's cookie store writes (its
/// `kCompatibleVersionNumber`): any Chrome at or above it opens the file.
const CREATED_META_COMPATIBLE_VERSION: i64 = 5;

/// Every place `profile_dir`'s cookie database may live. Chrome moved it to
/// `Network/Cookies` in 96 and, in current builds (154 verified in a macOS
/// Space), reads `Cookies` in the profile root again; Chromium forks differ
/// too. Existing databases are all written into; with none (a destination
/// Chrome that never ran, so its version is unknown) both are created, since
/// the one Chrome does not read is harmless and the one it does is not left
/// missing -- a missing one is a silently signed-out profile.
fn cookies_db_paths(profile_dir: &Path) -> Vec<std::path::PathBuf> {
    let network = profile_dir.join("Network").join("Cookies");
    let legacy = profile_dir.join("Cookies");
    let existing: Vec<_> = [&network, &legacy]
        .into_iter()
        .filter(|p| p.is_file())
        .cloned()
        .collect();
    if existing.is_empty() {
        vec![network, legacy]
    } else {
        existing
    }
}

/// Creates an empty `Cookies` database at `db` the way a first Chrome launch
/// would (Chrome's current `cookies` table plus its `meta` table at
/// [`CREATED_META_VERSION`]), so a later real launch adopts it. Every column
/// Chrome fills itself has a default, so [`install_cookies`]'s partial
/// `INSERT` is valid. Safe to call if another writer created it first.
fn create_cookies_db(db: &Path) -> std::io::Result<()> {
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let err = |e: rusqlite::Error| std::io::Error::other(e.to_string());
    let conn = rusqlite::Connection::open(db).map_err(err)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
         CREATE TABLE IF NOT EXISTS cookies (
            creation_utc INTEGER NOT NULL,
            host_key TEXT NOT NULL,
            top_frame_site_key TEXT NOT NULL DEFAULT '',
            name TEXT NOT NULL,
            value TEXT NOT NULL DEFAULT '',
            encrypted_value BLOB NOT NULL DEFAULT X'',
            path TEXT NOT NULL,
            expires_utc INTEGER NOT NULL,
            is_secure INTEGER NOT NULL,
            is_httponly INTEGER NOT NULL,
            last_access_utc INTEGER NOT NULL DEFAULT 0,
            has_expires INTEGER NOT NULL DEFAULT 1,
            is_persistent INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 1,
            samesite INTEGER NOT NULL DEFAULT -1,
            source_scheme INTEGER NOT NULL DEFAULT 0,
            source_port INTEGER NOT NULL DEFAULT -1,
            last_update_utc INTEGER NOT NULL DEFAULT 0,
            source_type INTEGER NOT NULL DEFAULT 0,
            has_cross_site_ancestor INTEGER NOT NULL DEFAULT 0,
            UNIQUE (host_key, top_frame_site_key, has_cross_site_ancestor, name, path, source_scheme, source_port)
         );
         CREATE INDEX IF NOT EXISTS domain ON cookies(host_key);",
    )
    .map_err(err)?;
    for (key, value) in [
        ("version", CREATED_META_VERSION),
        ("last_compatible_version", CREATED_META_COMPATIBLE_VERSION),
    ] {
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value.to_string()],
        )
        .map_err(err)?;
    }
    drop(conn);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(db, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Lowercased column names of `table`, via `PRAGMA table_info` (never guessed
/// or hardcoded as a `CREATE TABLE`, since the live schema is whatever this
/// machine's own Chrome created).
fn existing_columns(
    conn: &rusqlite::Connection,
    table: &str,
) -> rusqlite::Result<std::collections::HashSet<String>> {
    let sql = format!("PRAGMA table_info(\"{table}\")");
    let mut stmt = conn.prepare(&sql)?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names.into_iter().map(|n| n.to_ascii_lowercase()).collect())
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_chrome_130_destination_gets_the_host_key_digest() {
        use sha2::{Digest, Sha256};
        let with = plaintext_for(true, ".github.com", b"session=abc");
        assert_eq!(&with[..32], Sha256::digest(b".github.com").as_slice());
        assert_eq!(&with[32..], b"session=abc");
        assert_eq!(
            plaintext_for(false, ".github.com", b"session=abc"),
            b"session=abc"
        );
    }
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::host::EffectKind;
    use crate::host::{FakeHost, HostOutput};
    use std::sync::Arc;

    /// A `Cookies` database shaped the way a real Chrome first-run creates
    /// it (a representative modern schema, including columns this module
    /// does not know about, to prove unknown columns are tolerated).
    #[cfg(target_os = "macos")]
    fn first_run_cookies_db(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("Cookies");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (
                creation_utc INTEGER NOT NULL,
                host_key TEXT NOT NULL,
                top_frame_site_key TEXT NOT NULL DEFAULT '',
                name TEXT NOT NULL,
                value TEXT NOT NULL,
                encrypted_value BLOB NOT NULL DEFAULT '',
                path TEXT NOT NULL,
                expires_utc INTEGER NOT NULL,
                is_secure INTEGER NOT NULL,
                is_httponly INTEGER NOT NULL,
                last_access_utc INTEGER NOT NULL DEFAULT 0,
                has_expires INTEGER NOT NULL DEFAULT 1,
                is_persistent INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 1,
                samesite INTEGER NOT NULL DEFAULT -1,
                source_scheme INTEGER NOT NULL DEFAULT 0,
                source_port INTEGER NOT NULL DEFAULT -1,
                UNIQUE (host_key, top_frame_site_key, name, path)
            );",
        )
        .unwrap();
        db
    }

    fn item(host_key: &str, name: &str, value: &str) -> CookieItem {
        CookieItem {
            host_key: host_key.into(),
            name: name.into(),
            value: value.as_bytes().to_vec(),
            path: "/".into(),
            expires_utc: 0,
            is_secure: true,
            is_httponly: true,
            samesite: 1,
            extra: Default::default(),
        }
    }

    fn keychain_fake(home: &Path) -> Arc<FakeHost> {
        // No pre-existing item: a secret READ (`find-generic-password ... -w`)
        // fails, so `ensure_safe_storage_secret` takes the create path. Every
        // other call -- the interactive `security -i` add, `install_generic`'s
        // own post-install PRESENCE check (`find-generic-password` WITHOUT
        // `-w`, attributes only), and `set-partition-list` -- succeeds, since
        // those happen only after (and because) the item was just installed.
        Arc::new(FakeHost::new().with_home(home).with_responder(|c| {
            if c.args.iter().any(|a| a == "find-generic-password")
                && c.args.iter().any(|a| a == "-w")
            {
                Ok(HostOutput::failed())
            } else {
                Ok(HostOutput::ok(""))
            }
        }))
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn creates_a_fresh_key_when_none_exists_and_writes_readable_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = first_run_cookies_db(dir.path());
        let host = keychain_fake(dir.path());

        let items = vec![
            item(".github.com", "user_session", "gh-session-abc"),
            item("api.github.com", "_gh_sess", "gh-api-xyz"),
        ];
        let mut record = ImportRecord::default();
        let written = install_cookies(
            host.as_ref(),
            dir.path(),
            "Chrome Safe Storage",
            Platform::MacOS,
            &items,
            &mut record,
        )
        .unwrap();
        assert_eq!(written, 2);

        // Each row is ledgered by its own identity, so a later WipeImport
        // can delete exactly these two rows without touching the database.
        assert_eq!(record.cookie_rows.len(), 2);
        assert!(record.cookie_rows.iter().any(|r| {
            r.host_key == ".github.com" && r.name == "user_session" && r.db == db_path
        }));
        assert!(record
            .cookie_rows
            .iter()
            .any(|r| r.host_key == "api.github.com" && r.name == "_gh_sess"));

        // A fresh Safe Storage item was created (not read from nowhere) and
        // recorded, so a later WipeImport can remove it.
        assert!(
            host.calls_of(EffectKind::KeychainWrite)
                .iter()
                .any(|c| c.program == "security"),
            "expected a Keychain write creating the fresh item"
        );
        assert_eq!(record.keychain_items.len(), 1);
        assert_eq!(record.keychain_items[0].service, "Chrome Safe Storage");

        // The rows are readable back with the SAME key this call generated.
        // The fake has no real Keychain state, so re-derive the secret from
        // what `install_generic` actually wrote: the `-X <hex>` field of the
        // recorded `security -q -i` stdin line (see `add_command_line`).
        let install_call = host
            .calls_of(EffectKind::KeychainWrite)
            .into_iter()
            .find(|c| c.args == vec!["-q".to_string(), "-i".to_string()])
            .expect("the interactive add-generic-password call was recorded");
        let stdin = install_call.stdin.as_ref().unwrap();
        let stdin_text = String::from_utf8(stdin.as_bytes().to_vec()).unwrap();
        let hex_secret = stdin_text
            .split("-X ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .expect("the -X hex secret is present in the install line");
        let secret = hex::decode(hex_secret).unwrap();
        let key = chromium_crypto::derive_key(&secret, chromium_crypto::MACOS_PBKDF2_ROUNDS);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let mut stmt = conn
            .prepare("SELECT host_key, encrypted_value FROM cookies ORDER BY host_key")
            .unwrap();
        let rows: Vec<(String, Vec<u8>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        // ORDER BY host_key: ".github.com" < "api.github.com" lexically.
        let (host_key, encrypted) = &rows[1];
        assert_eq!(host_key, "api.github.com");
        let plain = chromium_crypto::decrypt_prefixed(&key, encrypted)
            .unwrap()
            .1;
        assert_eq!(&*plain, b"gh-api-xyz");

        // Columns this module does not know about kept their table default,
        // proving the live schema was discovered, not overwritten.
        let last_access: i64 = conn
            .query_row(
                "SELECT last_access_utc FROM cookies WHERE host_key = 'api.github.com'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_access, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reuses_an_existing_key_instead_of_creating_a_second_one() {
        let dir = tempfile::tempdir().unwrap();
        first_run_cookies_db(dir.path());
        let existing_secret = b"already-on-this-machine";
        // spacesd's own keychain, left by an earlier teleport.
        let keychains = dir.path().join("Library/Keychains");
        std::fs::create_dir_all(&keychains).unwrap();
        std::fs::write(keychains.join("cua.keychain-db"), b"").unwrap();
        let host = Arc::new(
            FakeHost::new()
                .with_home(dir.path())
                .with_responder(move |c| {
                    if c.args.iter().any(|a| a == "-w") {
                        Ok(HostOutput::ok(existing_secret.to_vec()))
                    } else {
                        Ok(HostOutput::ok(""))
                    }
                }),
        );
        let mut record = ImportRecord::default();
        let secret = ensure_safe_storage_secret(
            host.as_ref(),
            Platform::MacOS,
            "Chrome Safe Storage",
            &mut record,
        )
        .unwrap();
        assert_eq!(secret, existing_secret);
        // No item was created (no "security -i" interactive-add call), and
        // nothing is recorded: a reused item predates this import.
        assert!(
            !host
                .calls_of(EffectKind::KeychainWrite)
                .iter()
                .any(|c| c.args.iter().any(|a| a == "-i")),
            "must not overwrite an existing Safe Storage item"
        );
        assert!(record.keychain_items.is_empty());
    }

    /// Cookies for a browser that was never launched: the database is
    /// created in Chrome's current schema (at the `Network/Cookies` path
    /// current Chrome reads), and the rows read back with the Linux key.
    #[test]
    fn never_launched_profile_gets_a_chrome_schema_database_and_the_cookies() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        let host = Arc::new(FakeHost::new());
        let mut record = ImportRecord::default();
        let written = install_cookies(
            host.as_ref(),
            &profile,
            "Chrome Safe Storage",
            Platform::Linux,
            &[item(".github.com", "user_session", "gh-session-abc")],
            &mut record,
        )
        .unwrap();
        assert_eq!(written, 1);
        let db = profile.join("Network/Cookies");
        assert!(db.is_file());
        // The root `Cookies` is the one Chrome 154 reads: both are written.
        assert!(profile.join("Cookies").is_file());
        assert_eq!(record.cookie_rows.len(), 2);
        assert_eq!(record.cookie_rows[0].db, db);

        let conn = rusqlite::Connection::open(&db).unwrap();
        assert_eq!(cookies_meta_version(&conn), CREATED_META_VERSION);
        let compat: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_compatible_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(compat, "5");
        let columns = existing_columns(&conn, "cookies").unwrap();
        for c in [
            "top_frame_site_key",
            "has_cross_site_ancestor",
            "source_type",
        ] {
            assert!(columns.contains(c), "missing {c}");
        }
        // Readable as Chrome reads it: v10 under the fixed Linux key, host
        // digest in front of the value (meta version 24).
        let encrypted: Vec<u8> = conn
            .query_row(
                "SELECT encrypted_value FROM cookies WHERE host_key = '.github.com'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let key = chromium_crypto::derive_key(
            chromium_crypto::LINUX_V10_PASSWORD,
            chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
        );
        let plain = chromium_crypto::decrypt_prefixed(&key, &encrypted)
            .unwrap()
            .1;
        assert_eq!(
            &plain[..32],
            <sha2::Sha256 as sha2::Digest>::digest(b".github.com").as_slice()
        );
        assert_eq!(&plain[32..], b"gh-session-abc");
        // Nothing was installed in a keychain on Linux.
        assert!(host.calls().is_empty());
    }

    /// Every attribute the source row had lands in the destination row, and a
    /// partitioned cookie stays partitioned next to the same name unpartitioned.
    #[test]
    fn every_attribute_is_written_and_a_partitioned_cookie_stays_partitioned() {
        use cua_teleport_bundle::cookies::CookieExtra;
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        let host = Arc::new(FakeHost::new());
        let mut record = ImportRecord::default();
        let mut full = item(".example.com", "__Host-sid", "partitioned-value");
        full.expires_utc = 13_400_000_000_000_000;
        full.extra = CookieExtra {
            creation_utc: Some(13_300_000_000_000_001),
            last_access_utc: Some(13_300_000_000_000_002),
            last_update_utc: Some(13_300_000_000_000_003),
            priority: Some(2),
            source_scheme: Some(2),
            source_port: Some(443),
            source_type: Some(1),
            has_cross_site_ancestor: Some(1),
            partition_key: Some("https://top.example".into()),
        };
        let plain = item(".example.com", "__Host-sid", "plain-value");
        install_cookies(
            host.as_ref(),
            &profile,
            "Chrome Safe Storage",
            Platform::Linux,
            &[full, plain],
            &mut record,
        )
        .unwrap();
        let conn = rusqlite::Connection::open(profile.join("Network/Cookies")).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM cookies", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "partitioned and unpartitioned are two cookies");
        #[allow(clippy::type_complexity)]
        let got: (i64, i64, i64, i64, i64, i64, i64, i64, i64, String) = conn
            .query_row(
                "SELECT creation_utc, last_access_utc, last_update_utc, priority, source_scheme, \
                 source_port, source_type, has_cross_site_ancestor, expires_utc, top_frame_site_key \
                 FROM cookies WHERE top_frame_site_key != ''",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            got,
            (
                13_300_000_000_000_001,
                13_300_000_000_000_002,
                13_300_000_000_000_003,
                2,
                2,
                443,
                1,
                1,
                13_400_000_000_000_000,
                "https://top.example".to_string()
            )
        );
        // The unpartitioned one got Chrome's defaults, with no partition.
        let (prio, scheme, port, key): (i64, i64, i64, String) = conn
            .query_row(
                "SELECT priority, source_scheme, source_port, top_frame_site_key FROM cookies \
                 WHERE top_frame_site_key = ''",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((prio, scheme, port, key.as_str()), (1, 0, -1, ""));
    }

    #[test]
    fn never_launched_profile_creates_the_keychain_key_on_macos() {
        let dir = tempfile::tempdir().unwrap();
        let host = keychain_fake(dir.path());
        let mut record = ImportRecord::default();
        install_cookies(
            host.as_ref(),
            dir.path(),
            "Chrome Safe Storage",
            Platform::MacOS,
            &[item("a.test", "n", "v")],
            &mut record,
        )
        .unwrap();
        assert!(dir.path().join("Network/Cookies").is_file());
        assert_eq!(record.keychain_items.len(), 1);
    }

    #[test]
    fn an_existing_legacy_database_is_written_into_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = {
            let db = dir.path().join("Cookies");
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE cookies (creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL,
                 name TEXT NOT NULL, value TEXT NOT NULL DEFAULT '', encrypted_value BLOB NOT NULL DEFAULT '',
                 path TEXT NOT NULL, expires_utc INTEGER NOT NULL, is_secure INTEGER NOT NULL DEFAULT 0,
                 is_httponly INTEGER NOT NULL DEFAULT 0, samesite INTEGER NOT NULL DEFAULT -1);",
            )
            .unwrap();
            db
        };
        let mut record = ImportRecord::default();
        install_cookies(
            &FakeHost::new(),
            dir.path(),
            "Chrome Safe Storage",
            Platform::Linux,
            &[item("a.test", "n", "v")],
            &mut record,
        )
        .unwrap();
        assert!(!dir.path().join("Network").exists());
        let n: i64 = rusqlite::Connection::open(&legacy)
            .unwrap()
            .query_row("SELECT count(*) FROM cookies", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn empty_items_is_a_no_op_that_never_touches_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::new());
        let mut record = ImportRecord::default();
        let written = install_cookies(
            host.as_ref(),
            dir.path(),
            "Chrome Safe Storage",
            Platform::MacOS,
            &[],
            &mut record,
        )
        .unwrap();
        assert_eq!(written, 0);
        assert!(host.calls().is_empty());
    }

    #[test]
    fn account_name_strips_the_safe_storage_suffix() {
        assert_eq!(account_for_service("Chrome Safe Storage"), "Chrome");
        assert_eq!(
            account_for_service("Microsoft Edge Safe Storage"),
            "Microsoft Edge"
        );
        assert_eq!(account_for_service("Weird"), "Weird");
    }
}
